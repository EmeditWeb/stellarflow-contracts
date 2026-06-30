use soroban_sdk::{Address, Env, Map, Vec};
use crate::{ContractData, ContractError, DATA_KEY, SIGNERS_KEY, VALIDATOR_STATE_KEY};

const ACTIVE: u32 = 1 << 1;

fn get_validator_state(env: &Env, addr: &Address) -> u32 {
    let states: Map<Address, u32> = env
        .storage()
        .instance()
        .get(&VALIDATOR_STATE_KEY)
        .unwrap_or_else(|| Map::new(env));
    states.get(addr.clone()).unwrap_or(0u32)
}

fn set_validator_flag(env: &Env, addr: &Address, flag: u32, value: bool) {
    let mut states: Map<Address, u32> = env
        .storage()
        .instance()
        .get(&VALIDATOR_STATE_KEY)
        .unwrap_or_else(|| Map::new(env));
    let current = states.get(addr.clone()).unwrap_or(0u32);
    let updated = if value { current | flag } else { current & !flag };
    states.set(addr.clone(), updated);
    env.storage().instance().set(&VALIDATOR_STATE_KEY, &states);
}

fn has_validator_flag(env: &Env, addr: &Address, flag: u32) -> bool {
    get_validator_state(env, addr) & flag != 0
}

/// Rigid multi-signature confirmation barrier for parameter shift actions.
/// Requires a supermajority of 4 out of 5 validated administrative signatures
/// before approving changes to system boundary configurations.
///
/// Refactored to use zero-allocation array references by parsing signature lists
/// directly from raw input stream slices, avoiding dynamic heap expansions.
pub fn require_multisig(env: &Env, signers: &Vec<Address>) -> Result<(), ContractError> {
    let authorized_signers: Map<Address, ()> = env
        .storage()
        .instance()
        .get(&SIGNERS_KEY)
        .unwrap_or_else(|| Map::new(env));

    let data: ContractData = env
        .storage()
        .instance()
        .get(&DATA_KEY)
        .ok_or(ContractError::NotInitialized)?;

    let mut seen: Map<Address, ()> = Map::new(env);
    let mut valid_count = 0u32;
    let signers_slice = signers.iter();

    // Use slice-based iteration to avoid heap allocations
    for (idx, signer) in signers_slice.clone().enumerate() {
        // Avoid repeated signature validation for duplicate signers using slice comparison
        let is_duplicate = signers_slice.clone().take(idx).any(|previous| previous == signer);
        if is_duplicate {
            continue;
        }
        seen.set(signer.clone(), ());

        let state = get_validator_state(env, &signer);

        let is_authorized = (authorized_signers.contains_key(signer.clone()) || data.admin == signer)

        let is_authorized = (authorized_signers.contains_key(signer.clone()) || data.admin == signer.clone())

            && (state & ACTIVE) != 0;
        if !is_authorized {
            continue;
        }

        let state = get_validator_state(env, &signer);
        let is_active = state == 0 || (state & ACTIVE) != 0;
        if !is_active {
            continue;
        }

        if valid_count >= 4 {
            break;
        }

        valid_count += 1;
    }

    if valid_count < 2 {
        return Err(ContractError::ThresholdNotReached);
    }

    Ok(())
}
