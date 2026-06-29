use soroban_sdk::{contracttype, Address, Env, Symbol};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Subscription(Address),
    AssetPrice(Symbol),
}

pub const RENT_THRESHOLD: u32 = 259_200;
pub const RENT_EXTEND_TO: u32 = 518_400;

pub const ASSET_TTL_THRESHOLD: u32 = 5_000;
pub const ASSET_TTL_EXTEND_TO: u32 = 100_000;

pub fn extend_subscription_rent(env: &Env, consumer_id: Address) {
    let key = DataKey::Subscription(consumer_id);
    env.storage().persistent().extend_ttl(&key, RENT_THRESHOLD, RENT_EXTEND_TO);
}

pub fn check_subscription(env: &Env, consumer_id: Address) -> bool {
    let key = DataKey::Subscription(consumer_id.clone());
    if env.storage().persistent().has(&key) {
        extend_subscription_rent(env, consumer_id);
        true
    } else {
        false
    }
}

pub fn extend_asset_rent(env: &Env, asset: Symbol) -> bool {
    let key = DataKey::AssetPrice(asset);
    if env.storage().persistent().has(&key) {
        env.storage().persistent().extend_ttl(&key, ASSET_TTL_THRESHOLD, ASSET_TTL_EXTEND_TO);
        true
    } else {
        false
    }
}

pub fn preflight_rent_check(env: &Env) {
    env.storage().instance().extend_ttl(0, ASSET_TTL_THRESHOLD);
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeedStakeValue {
    pub amount: u64,
    pub last_active: u64,
}

pub fn check_and_prune_feed_stake(env: &Env, node: Address, asset: u32) -> bool {
    let key = crate::StakingStorageKey::FeedStake(node.clone(), asset);
    if !env.storage().persistent().has(&key) {
        return false;
    }

    let val: FeedStakeValue = env.storage().persistent().get(&key).unwrap();
    let elapsed = env.ledger().timestamp().saturating_sub(val.last_active);

    if elapsed > RENT_THRESHOLD as u64 {
        env.storage().persistent().remove(&key);

        let mut stakes: soroban_sdk::Map<Address, u64> = env
            .storage()
            .instance()
            .get(&crate::STAKE_REGISTRY_KEY)
            .unwrap_or_else(|| soroban_sdk::Map::new(env));
        let node_total = stakes.get(node.clone()).unwrap_or(0);
        let new_node_total = node_total.saturating_sub(val.amount);
        if new_node_total == 0 {
            stakes.remove(node.clone());
        } else {
            stakes.set(node.clone(), new_node_total);
        }
        env.storage().instance().set(&crate::STAKE_REGISTRY_KEY, &stakes);

        let total: u64 = env
            .storage()
            .instance()
            .get(&crate::TOTAL_STAKED_KEY)
            .unwrap_or(0u64);
        let new_total = total.saturating_sub(val.amount);
        env.storage().instance().set(&crate::TOTAL_STAKED_KEY, &new_total);

        true
    } else {
        false
    }
}

pub fn update_feed_stake_activity(env: &Env, node: Address, asset: u32) {
    let key = crate::StakingStorageKey::FeedStake(node, asset);
    if let Some(mut val) = env.storage().persistent().get::<_, FeedStakeValue>(&key) {
        val.last_active = env.ledger().timestamp();
        env.storage().persistent().set(&key, &val);
        env.storage().persistent().extend_ttl(&key, RENT_THRESHOLD, RENT_EXTEND_TO);
    }
}
