use crate::{AssetId, ContractError, TimeLockedUpgradeContract};
use soroban_sdk::{contracttype, Address, Env};

// ---------------------------------------------------------------------------
// Asset pricing storage (general — unchanged)
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Clone)]
pub struct CorridorFeePool {
    pub asset: AssetId,
    pub collected: u64,
    pub variable_pool: u64,
}

#[contracttype]
pub enum FeesStorageKey {
    CorridorPool(AssetId),
}

impl CorridorFeePool {
    fn new(asset: AssetId) -> Self {
        Self {
            asset,
            collected: 0,
            variable_pool: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Corridor weight profile — separated from asset pricing entries (issue #530)
// ---------------------------------------------------------------------------

/// Dedicated profile holding dynamic corridor weight variables.
/// Kept in its own storage key so audits and state updates never
/// touch the general asset pricing block.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct CorridorWeightProfile {
    pub asset: AssetId,
    pub base_weight: u64,
    pub dynamic_weight: u64,
}

/// Separate storage namespace for corridor weight profiles.
#[contracttype]
pub enum CorridorWeightKey {
    Profile(AssetId),
}

impl CorridorWeightProfile {
    fn new(asset: AssetId) -> Self {
        Self {
            asset,
            base_weight: 0,
            dynamic_weight: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Fee pool functions (unchanged behaviour)
// ---------------------------------------------------------------------------

pub fn add_corridor_fees(
    env: Env,
    admin: Address,
    asset: AssetId,
    collected: u64,
    variable_fee: u64,
) -> Result<CorridorFeePool, ContractError> {
    admin.require_auth();
    let data = TimeLockedUpgradeContract::get_data(env.clone())?;
    if data.admin != admin {
        return Err(ContractError::NotAdmin);
    }
    let key = FeesStorageKey::CorridorPool(asset.clone());
    let mut pool: CorridorFeePool = env
        .storage()
        .instance()
        .get(&key)
        .unwrap_or(CorridorFeePool::new(asset.clone()));
    pool.collected = pool
        .collected
        .checked_add(collected)
        .ok_or(ContractError::Overflow)?;
    pool.variable_pool = pool
        .variable_pool
        .checked_add(variable_fee)
        .ok_or(ContractError::Overflow)?;
    env.storage().instance().set(&key, &pool);
    Ok(pool)
}

pub fn get_corridor_fee_pool(env: Env, asset: AssetId) -> CorridorFeePool {
    let key = FeesStorageKey::CorridorPool(asset.clone());
    env.storage()
        .instance()
        .get(&key)
        .unwrap_or(CorridorFeePool::new(asset))
}

// ---------------------------------------------------------------------------
// Corridor weight profile functions — independent access control (issue #530)
// ---------------------------------------------------------------------------

/// Set or update the corridor weight profile for an asset.
/// Uses its own admin check so weight edits are gated independently
/// from fee pool writes.
pub fn set_corridor_weight(
    env: Env,
    admin: Address,
    asset: AssetId,
    base_weight: u64,
    dynamic_weight: u64,
) -> Result<CorridorWeightProfile, ContractError> {
    admin.require_auth();
    let data = TimeLockedUpgradeContract::get_data(env.clone())?;
    if data.admin != admin {
        return Err(ContractError::NotAdmin);
    }
    let key = CorridorWeightKey::Profile(asset.clone());
    let profile = CorridorWeightProfile {
        asset: asset.clone(),
        base_weight,
        dynamic_weight,
    };
    env.storage().persistent().set(&key, &profile);
    Ok(profile)
}

/// Read the corridor weight profile for an asset.
pub fn get_corridor_weight(env: Env, asset: AssetId) -> CorridorWeightProfile {
    let key = CorridorWeightKey::Profile(asset.clone());
    env.storage()
        .persistent()
        .get(&key)
        .unwrap_or(CorridorWeightProfile::new(asset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TimeLockedUpgradeContractClient;
    use soroban_sdk::testutils::Address as _;

    fn setup() -> (Env, TimeLockedUpgradeContractClient<'static>, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, TimeLockedUpgradeContract);
        let client = TimeLockedUpgradeContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let treasury = Address::generate(&env);
        let attacker = Address::generate(&env);
        client.initialize(&admin, &treasury);
        (env, client, admin, attacker)
    }

    #[test]
    fn corridor_weight_profile_is_isolated_from_fee_pool() {
        let (_, client, admin, _) = setup();
        let asset = 3897123275;

        let pool = client.add_corridor_fees(&admin, &asset, &1_000, &25);
        assert_eq!(pool.collected, 1_000);
        assert_eq!(pool.variable_pool, 25);

        let profile = client.set_corridor_weight(&admin, &asset, &70, &30);
        assert_eq!(profile.asset, asset);
        assert_eq!(profile.base_weight, 70);
        assert_eq!(profile.dynamic_weight, 30);

        let unchanged_pool = client.get_corridor_fee_pool(&asset);
        assert_eq!(unchanged_pool.collected, 1_000);
        assert_eq!(unchanged_pool.variable_pool, 25);

        let stored_profile = client.get_corridor_weight(&asset);
        assert_eq!(stored_profile.base_weight, 70);
        assert_eq!(stored_profile.dynamic_weight, 30);
    }

    #[test]
    fn non_admin_cannot_edit_corridor_weight_profile() {
        let (_, client, _, attacker) = setup();
        let result = client.try_set_corridor_weight(&attacker, &2654435761, &40, &60);

        assert_eq!(result, Err(Ok(ContractError::NotAdmin)));
    }
}
