//! Bond capacity validation for premium asset pool access.
//!
//! Enforces that a validator's active locked stake meets the minimum required
//! bond before it may register profile updates for premium asset corridors.
//! Nodes that fall below the threshold are rejected with
//! `ContractError::PremiumPoolAccessDenied`, preventing under-bonded validators
//! from tracking high-volume asset corridors.
//!
//! Also provides telemetry freshness verification to reject stale data
//! payloads whose timestamps lag the current ledger block time beyond the
//! configured threshold (60 seconds).

use soroban_sdk::{Address, Env, Map, Symbol};

use crate::{
    AssetId, ContractError, CorridorFeeKey, CorridorFeePool, StakingStorageKey,
    STAKE_REGISTRY_KEY,
};
use crate::staking_tiers::AssetFeedMetrics;

/// Minimum stake (in the same units as `StakeRecord.amount`) required to
/// update a validator profile for a premium asset pool.
pub const PREMIUM_POOL_MIN_STAKE: u64 = 1_000;

/// Maximum allowed age (in seconds) for an incoming telemetry payload's
/// ledger timestamp before it is considered stale and rejected.
pub const MAX_TELEMETRY_AGE_SECS: u64 = 60;

/// Minimum cumulative pool/corridor volume required before telemetry derived
/// from an AMM pool may update downstream exchange metrics.
///
/// This economic security floor rejects thinly backed pools whose spot prices
/// are cheap to move with flash-loaned capital. The value uses the same units
/// as `CorridorFeePool.collected`.
pub const MIN_POOL_VOLUME_DEPTH: u64 = 1_000_000;

/// Minimum normalized volume score for explicitly configured feed metrics.
/// Scores below this value represent low-depth regional pools and must not be
/// accepted as a source for exchange telemetry updates.
pub const MIN_POOL_VOLUME_SCORE: u32 = 33;

/// Evaluate whether an asset's underlying AMM/corridor has sufficient economic
/// depth to safely accept telemetry updates.
///
/// The gate considers both on-chain cumulative pool activity and any configured
/// feed volume score. A pool passes if either signal meets the minimum security
/// threshold. This permits admins to bootstrap known-deep pools via metrics,
/// while still rejecting assets whose stored/default metrics and corridor
/// volume indicate a thin market.
pub fn check_liquidity_depth(env: &Env, asset: AssetId) -> Result<(), ContractError> {
    let corridor: CorridorFeePool = env
        .storage()
        .persistent()
        .get(&CorridorFeeKey::Asset(asset))
        .unwrap_or(CorridorFeePool {
            asset,
            collected: 0,
            variable_pool: 0,
        });

    if corridor.collected >= MIN_POOL_VOLUME_DEPTH {
        return Ok(());
    }

    let metrics: Option<AssetFeedMetrics> = env
        .storage()
        .persistent()
        .get(&StakingStorageKey::AssetMetrics(asset));

    if let Some(metrics) = metrics {
        if metrics.volume_score >= MIN_POOL_VOLUME_SCORE {
            return Ok(());
        }
    }

    Err(ContractError::InsufficientLiquidityDepth)
}

/// Return the current locked stake for `node`, or 0 if unregistered.
pub fn get_locked_stake(env: &Env, node: &Address) -> u64 {
    let stakes: Map<Address, u64> = env
        .storage()
        .instance()
        .get(&STAKE_REGISTRY_KEY)
        .unwrap_or_else(|| Map::new(env));
    stakes.get(node.clone()).unwrap_or(0)
}

/// Verify that `node` has sufficient locked stake to update a premium pool
/// validator profile.  Returns `ContractError::PremiumPoolAccessDenied` when
/// the active stake falls below `PREMIUM_POOL_MIN_STAKE`.
pub fn check_bond_capacity(
    env: &Env,
    node: &Address,
    _pool: &Symbol,
) -> Result<(), ContractError> {
    let stake = get_locked_stake(env, node);
    if stake < PREMIUM_POOL_MIN_STAKE {
        return Err(ContractError::PremiumPoolAccessDenied);
    }
    Ok(())
}

/// Validate that an incoming telemetry payload's ledger timestamp is not
/// too far behind the current ledger block time.
///
/// Returns `ContractError::StaleTelemetryPayload` when the payload timestamp
/// lags the current time by more than `MAX_TELEMETRY_AGE_SECS` (60 seconds).
pub fn verify_payload_freshness(
    env: &Env,
    payload_timestamp: u64,
) -> Result<(), ContractError> {
    let current = env.ledger().timestamp();
    if current.saturating_sub(payload_timestamp) > MAX_TELEMETRY_AGE_SECS {
        return Err(ContractError::StaleTelemetryPayload);
    }
    Ok(())
}

#[cfg(test)]
mod freshness_tests {
    use super::*;
    use soroban_sdk::Env;
    use soroban_sdk::testutils::{Ledger, LedgerInfo};

    fn setup() -> Env {
        let env = Env::default();
        env.ledger().set(LedgerInfo {
            timestamp: 1_000_000,
            protocol_version: env.ledger().protocol_version(),
            sequence_number: env.ledger().sequence(),
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 0,
            min_persistent_entry_ttl: 0,
            max_entry_ttl: u32::MAX,
        });
        env
    }

    #[test]
    fn test_fresh_payload_within_60s_passes() {
        let env = setup();
        // Payload timestamp is 30 seconds behind current — within limit.
        let result = verify_payload_freshness(&env, 999_970);
        assert!(result.is_ok());
    }

    #[test]
    fn test_fresh_payload_exactly_at_60s_passes() {
        let env = setup();
        // Payload timestamp is exactly 60 seconds behind — boundary passes.
        let result = verify_payload_freshness(&env, 999_940);
        assert!(result.is_ok());
    }

    #[test]
    fn test_stale_payload_beyond_60s_rejected() {
        let env = setup();
        // Payload timestamp is 61 seconds behind — exceeds limit.
        let result = verify_payload_freshness(&env, 999_939);
        assert_eq!(result, Err(ContractError::StaleTelemetryPayload));
    }

    #[test]
    fn test_payload_from_future_passes() {
        let env = setup();
        // Payload timestamp slightly ahead of current time is allowed.
        let result = verify_payload_freshness(&env, 1_000_010);
        assert!(result.is_ok());
    }

    #[test]
    fn test_payload_at_current_time_passes() {
        let env = setup();
        let result = verify_payload_freshness(&env, 1_000_000);
        assert!(result.is_ok());
    }

    #[test]
    fn test_payload_very_stale_rejected() {
        let env = setup();
        // Payload far in the past.
        let result = verify_payload_freshness(&env, 0);
        assert_eq!(result, Err(ContractError::StaleTelemetryPayload));
    }
}
