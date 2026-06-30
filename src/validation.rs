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

use soroban_sdk::{Address, Env, Map, Symbol, Vec};

use crate::{CONSENSUS_CACHE_KEY, ContractError, STAKE_REGISTRY_KEY};

/// Minimum stake (in the same units as `StakeRecord.amount`) required to
/// update a validator profile for a premium asset pool.
pub const PREMIUM_POOL_MIN_STAKE: u64 = 1_000;

/// Maximum allowed age (in seconds) for an incoming telemetry payload's
/// ledger timestamp before it is considered stale and rejected.
pub const MAX_TELEMETRY_AGE_SECS: u64 = 60;

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

/// Minimum number of independent validator submissions required for
/// a valid consensus round.
pub const MIN_CONSENSUS_DEPTH: u32 = 3;

/// Check that at least `MIN_CONSENSUS_DEPTH` independent validators
/// have supplied parameters in the current block round.
///
/// Reads the consensus participant cache (`CONSENSUS_CACHE_KEY`) from
/// temporary storage to count active submissions.  Reverts the
/// transaction early with `ContractError::IncompleteQuorum` when the
/// count falls below the minimum threshold.
pub fn check_consensus_depth(env: &Env) -> Result<(), ContractError> {
    let participants: Vec<Address> = env
        .storage()
        .temporary()
        .get(&CONSENSUS_CACHE_KEY)
        .unwrap_or_else(|| Vec::new(env));

    if participants.len() < MIN_CONSENSUS_DEPTH {
        return Err(ContractError::IncompleteQuorum);
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

#[cfg(test)]
mod consensus_depth_tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    fn setup_env() -> Env {
        Env::default()
    }

    fn populate_cache(env: &Env, count: u32) {
        let mut participants: Vec<Address> = Vec::new(env);
        for _ in 0..count {
            let node = Address::generate(env);
            participants.push_back(node);
        }
        env.storage()
            .temporary()
            .set(&CONSENSUS_CACHE_KEY, &participants);
    }

    #[test]
    fn test_empty_cache_rejected() {
        let env = setup_env();
        let result = check_consensus_depth(&env);
        assert_eq!(result, Err(ContractError::IncompleteQuorum));
    }

    #[test]
    fn test_one_validator_rejected() {
        let env = setup_env();
        populate_cache(&env, 1);
        let result = check_consensus_depth(&env);
        assert_eq!(result, Err(ContractError::IncompleteQuorum));
    }

    #[test]
    fn test_two_validators_rejected() {
        let env = setup_env();
        populate_cache(&env, 2);
        let result = check_consensus_depth(&env);
        assert_eq!(result, Err(ContractError::IncompleteQuorum));
    }

    #[test]
    fn test_three_validators_accepted() {
        let env = setup_env();
        populate_cache(&env, 3);
        let result = check_consensus_depth(&env);
        assert!(result.is_ok());
    }

    #[test]
    fn test_many_validators_accepted() {
        let env = setup_env();
        populate_cache(&env, 10);
        let result = check_consensus_depth(&env);
        assert!(result.is_ok());
    }
}
