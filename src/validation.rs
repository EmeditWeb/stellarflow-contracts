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

use soroban_sdk::{contracttype, symbol_short, Address, Env, Map, Symbol, Vec};

use crate::{AssetId, ContractError, STAKE_REGISTRY_KEY};

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

// ─────────────────────────────────────────────────────────────────────────────
// Gas-Throttled Bundle Processing — Single-Pass Multi-Asset Price Updates
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of assets that may be included in a single price-update
/// bundle.  Hard-capped to keep execution gas within the Soroban transaction
/// budget during high-density network waves.
pub const MAX_BUNDLE_ASSETS: u32 = 20;

/// Pre-computed key index pointer for an asset within a price bundle.
///
/// Built **once** before the main processing loop so that every subsequent
/// validation step uses a direct O(1) pointer rather than scanning maps,
/// recalculating symbols, or performing nested iterations.
///
/// # Flat execution guarantee
/// The `pool_symbol` and `timestamp` fields are pre-loaded from the update
/// payload and cached in the index.  The main loop never needs to re-scan
/// the update vector or recompute the Symbol → AssetId mapping, keeping
/// the execution profile strictly O(n) with no matrix intersections.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleAssetIndex {
    pub asset: AssetId,
    /// Pre-computed pool Symbol for direct O(1) validation access.
    /// Eliminates the match-table dispatch that `asset_id_to_symbol_short`
    /// would otherwise incur on every iteration.
    pub pool_symbol: Symbol,
    /// Pre-loaded payload timestamp — avoids re-scanning the update vector
    /// during the validation loop.
    pub timestamp: u64,
}

/// A single asset's price submission inside a bundled multi-asset update.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetPriceUpdate {
    pub asset: AssetId,
    pub price: u64,
    pub timestamp: u64,
}

/// Aggregated outcome of a bundle-wide validation pass.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleValidationOutcome {
    pub total_assets: u32,
    pub accepted: u32,
}

/// Build a flat index of bundle assets with pre-calculated key pointers.
///
/// This is the foundational optimisation of the gas-throttled design: by
/// computing storage-level pointers (Symbol keys, timestamps) **upfront**
/// in a single O(n) pass, the main processing loop accesses every asset
/// via its pre-cached index entry without scanning maps, re-computing
/// identifiers, or performing matrix-style nested iterations.
///
/// # Gas profile
/// - Allocates exactly `n` index entries – no reallocation or growth.
/// - Returns immediately when the bundle exceeds `MAX_BUNDLE_ASSETS`,
///   preventing gas waste on oversized payloads.
/// - The caller's main loop uses only O(1) field reads from the index,
///   guaranteeing a flat execution profile regardless of bundle size.
pub fn build_bundle_index(
    env: &Env,
    updates: &Vec<AssetPriceUpdate>,
) -> Result<Vec<BundleAssetIndex>, ContractError> {
    let n = updates.len() as u32;
    if n > MAX_BUNDLE_ASSETS {
        return Err(ContractError::BundleAssetLimitExceeded);
    }

    let mut index: Vec<BundleAssetIndex> = Vec::new(env);
    for update in updates.iter() {
        index.push_back(BundleAssetIndex {
            asset: update.asset,
            pool_symbol: asset_id_to_symbol_short(update.asset),
            timestamp: update.timestamp,
        });
    }
    Ok(index)
}

/// Validate a bundled multi-asset price submission using a strict
/// **single-pass linear scan** with pre-calculated key index pointers.
///
/// # How it replaces matrix iterations
///
/// A naive implementation would nest asset-iteration inside each validation
/// check (bond, freshness), producing an O(n × m) execution profile where
/// m is the number of checks.  This function instead:
///
/// 1. Calls `build_bundle_index` upfront to pre-compute every asset's
///    storage Symbol key and payload timestamp (O(n)).
/// 2. Walks the pre-computed index in **one** linear loop, reading every
///    validation input from the index's pre-cached fields (O(n)).
///
/// The result is a **flat** execution profile: every bundle, regardless of
/// composition, executes in exactly one pass with no nested iteration,
/// no Symbol recomputation, and no secondary Vec scans.
///
/// # Arguments
/// * `env` – Soroban host environment.
/// * `node` – Validator submitting the bundle.
/// * `updates` – Packed vector of per-asset price updates.
///
/// # Returns
/// `BundleValidationOutcome` with the count of accepted assets, or:
/// * `BundleAssetLimitExceeded` – bundle size > `MAX_BUNDLE_ASSETS`.
/// * `PremiumPoolAccessDenied` – validator's stake is below minimum.
/// * `StaleTelemetryPayload` – any update's timestamp exceeds the freshness
///    threshold.
pub fn process_price_bundle(
    env: &Env,
    node: &Address,
    updates: &Vec<AssetPriceUpdate>,
) -> Result<BundleValidationOutcome, ContractError> {
    // Phase 1 — pre-compute key index pointers (single O(n) pass).
    // Each index entry carries a pre-computed pool_symbol and timestamp,
    // so Phase 2 never needs to re-scan the update vector.
    let index = build_bundle_index(env, updates)?;

    // Phase 2 — single-pass linear scan using pre-computed index fields.
    let mut accepted: u32 = 0;

    for entry in index.iter() {
        // Bond validation — uses pre-computed pool_symbol (no match dispatch).
        check_bond_capacity(env, node, &entry.pool_symbol)?;

        // Freshness validation — uses pre-loaded timestamp (no Vec re-scan).
        verify_payload_freshness(env, entry.timestamp)?;

        accepted += 1;
    }

    Ok(BundleValidationOutcome {
        total_assets: index.len() as u32,
        accepted,
    })
}

/// Map a numeric `AssetId` back to a `Symbol` for legacy validation calls.
///
/// Maintained as a private helper so the bundle processor can reuse
/// `check_bond_capacity` and other Symbol-based APIs without allocating
/// a full lookup table.
fn asset_id_to_symbol_short(id: AssetId) -> Symbol {
    match id {
        3897123275 => symbol_short!("NGN"),
        2654435761 => symbol_short!("KES"),
        4026531840 => symbol_short!("GHS"),
        4160749568 => symbol_short!("CFA"),
        3219226362 => symbol_short!("ZAR"),
        2863311530 => symbol_short!("UGX"),
        0 => symbol_short!("STAKE"),
        1 => symbol_short!("VALUE"),
        _ => symbol_short!("UNK"),
    }
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
mod bundle_processing_tests {
    use super::*;
    use crate::TimeLockedUpgradeContract;
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};

    fn setup_env() -> Env {
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

    fn make_update(asset: AssetId, price: u64, timestamp: u64) -> AssetPriceUpdate {
        AssetPriceUpdate {
            asset,
            price,
            timestamp,
        }
    }

    fn setup_contract_client<'a>(env: &'a Env, contract_id: &Address) -> (crate::TimeLockedUpgradeContractClient<'a>, Address) {
        let admin = Address::generate(env);
        let treasury = Address::generate(env);
        let client = crate::TimeLockedUpgradeContractClient::new(env, contract_id);
        client.initialize(&admin, &treasury);
        (client, admin)
    }

    fn setup_env_with_contract() -> (Env, Address) {
        let env = setup_env();
        let contract_id = env.register_contract(None, TimeLockedUpgradeContract);
        (env, contract_id)
    }

    // ── build_bundle_index tests ──────────────────────────────────────────

    #[test]
    fn test_build_bundle_index_empty() {
        let env = setup_env();
        let updates: Vec<AssetPriceUpdate> = Vec::new(&env);
        let index = build_bundle_index(&env, &updates).unwrap();
        assert!(index.is_empty());
    }

    #[test]
    fn test_build_bundle_index_single_asset() {
        let env = setup_env();
        let mut updates: Vec<AssetPriceUpdate> = Vec::new(&env);
        updates.push_back(make_update(3897123275, 100_000, 999_980));
        let index = build_bundle_index(&env, &updates).unwrap();
        assert_eq!(index.len(), 1);
        assert_eq!(index.get(0).unwrap().asset, 3897123275);
    }

    #[test]
    fn test_build_bundle_index_precomputes_symbol_and_timestamp() {
        let env = setup_env();
        let mut updates: Vec<AssetPriceUpdate> = Vec::new(&env);
        updates.push_back(make_update(3897123275, 100_000, 999_980));
        let index = build_bundle_index(&env, &updates).unwrap();
        assert_eq!(index.get(0).unwrap().pool_symbol, symbol_short!("NGN"));
        assert_eq!(index.get(0).unwrap().timestamp, 999_980);
    }

    #[test]
    fn test_build_bundle_index_exceeds_max() {
        let env = setup_env();
        let mut updates: Vec<AssetPriceUpdate> = Vec::new(&env);
        for i in 0..MAX_BUNDLE_ASSETS + 1 {
            updates.push_back(make_update(i, 100_000, 999_980));
        }
        let result = build_bundle_index(&env, &updates);
        assert_eq!(result, Err(ContractError::BundleAssetLimitExceeded));
    }

    #[test]
    fn test_build_bundle_index_at_max_boundary() {
        let env = setup_env();
        let mut updates: Vec<AssetPriceUpdate> = Vec::new(&env);
        for i in 0..MAX_BUNDLE_ASSETS {
            updates.push_back(make_update(i, 100_000, 999_980));
        }
        let index = build_bundle_index(&env, &updates).unwrap();
        assert_eq!(index.len(), MAX_BUNDLE_ASSETS);
    }

    // ── process_price_bundle tests ────────────────────────────────────────

    #[test]
    fn test_process_price_bundle_single_asset_passes() {
        let (env, contract_id) = setup_env_with_contract();
        env.mock_all_auths();
        let (client, node) = setup_contract_client(&env, &contract_id);
        client.stake_and_register(&node, &PREMIUM_POOL_MIN_STAKE);

        let mut updates: Vec<AssetPriceUpdate> = Vec::new(&env);
        updates.push_back(make_update(3897123275, 100_000, 999_980));

        let result = client.update_prices_bundle(&node, &updates);
        assert_eq!(result.total_assets, 1);
        assert_eq!(result.accepted, 1);
    }

    #[test]
    fn test_process_price_bundle_multiple_assets_passes() {
        let (env, contract_id) = setup_env_with_contract();
        env.mock_all_auths();
        let (client, node) = setup_contract_client(&env, &contract_id);
        client.stake_and_register(&node, &PREMIUM_POOL_MIN_STAKE);

        let mut updates: Vec<AssetPriceUpdate> = Vec::new(&env);
        updates.push_back(make_update(3897123275, 100_000, 999_980));
        updates.push_back(make_update(2654435761, 200_000, 999_970));
        updates.push_back(make_update(4026531840, 150_000, 999_960));

        let result = client.update_prices_bundle(&node, &updates);
        assert_eq!(result.total_assets, 3);
        assert_eq!(result.accepted, 3);
    }

    #[test]
    fn test_process_price_bundle_exceeds_max_assets_rejected() {
        let (env, contract_id) = setup_env_with_contract();
        env.mock_all_auths();
        let (client, node) = setup_contract_client(&env, &contract_id);

        let mut updates: Vec<AssetPriceUpdate> = Vec::new(&env);
        for i in 0..MAX_BUNDLE_ASSETS + 1 {
            updates.push_back(make_update(i, 100_000, 999_980));
        }

        let result = client.try_update_prices_bundle(&node, &updates);
        assert_eq!(result, Err(Ok(ContractError::BundleAssetLimitExceeded)));
    }

    #[test]
    fn test_process_price_bundle_insufficient_stake_rejected() {
        let (env, contract_id) = setup_env_with_contract();
        env.mock_all_auths();
        let (client, node) = setup_contract_client(&env, &contract_id);
        client.stake_and_register(&node, &(PREMIUM_POOL_MIN_STAKE - 1));

        let mut updates: Vec<AssetPriceUpdate> = Vec::new(&env);
        updates.push_back(make_update(3897123275, 100_000, 999_980));

        let result = client.try_update_prices_bundle(&node, &updates);
        assert_eq!(result, Err(Ok(ContractError::PremiumPoolAccessDenied)));
    }

    #[test]
    fn test_process_price_bundle_stale_payload_rejected() {
        let (env, contract_id) = setup_env_with_contract();
        env.mock_all_auths();
        let (client, node) = setup_contract_client(&env, &contract_id);
        client.stake_and_register(&node, &PREMIUM_POOL_MIN_STAKE);

        let mut updates: Vec<AssetPriceUpdate> = Vec::new(&env);
        updates.push_back(make_update(3897123275, 100_000, 999_939));

        let result = client.try_update_prices_bundle(&node, &updates);
        assert_eq!(result, Err(Ok(ContractError::StaleTelemetryPayload)));
    }

    #[test]
    fn test_process_price_bundle_mixed_freshness_rejected_on_first_stale() {
        let (env, contract_id) = setup_env_with_contract();
        env.mock_all_auths();
        let (client, node) = setup_contract_client(&env, &contract_id);
        client.stake_and_register(&node, &PREMIUM_POOL_MIN_STAKE);

        let mut updates: Vec<AssetPriceUpdate> = Vec::new(&env);
        updates.push_back(make_update(3897123275, 100_000, 999_939));
        updates.push_back(make_update(2654435761, 200_000, 999_980));

        let result = client.try_update_prices_bundle(&node, &updates);
        assert_eq!(result, Err(Ok(ContractError::StaleTelemetryPayload)));
    }

    // ── asset_id_to_symbol_short tests ────────────────────────────────────

    #[test]
    fn test_asset_id_to_symbol_short_known_ids() {
        assert_eq!(
            asset_id_to_symbol_short(3897123275),
            symbol_short!("NGN")
        );
        assert_eq!(
            asset_id_to_symbol_short(2654435761),
            symbol_short!("KES")
        );
        assert_eq!(
            asset_id_to_symbol_short(4026531840),
            symbol_short!("GHS")
        );
        assert_eq!(
            asset_id_to_symbol_short(4160749568),
            symbol_short!("CFA")
        );
        assert_eq!(
            asset_id_to_symbol_short(3219226362),
            symbol_short!("ZAR")
        );
        assert_eq!(
            asset_id_to_symbol_short(2863311530),
            symbol_short!("UGX")
        );
        assert_eq!(asset_id_to_symbol_short(0), symbol_short!("STAKE"));
        assert_eq!(asset_id_to_symbol_short(1), symbol_short!("VALUE"));
    }

    #[test]
    fn test_asset_id_to_symbol_short_unknown_returns_unk() {
        assert_eq!(
            asset_id_to_symbol_short(999_999),
            symbol_short!("UNK")
        );
    }
}
