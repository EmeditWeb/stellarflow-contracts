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

/// Multi-signature consensus approval for cross-border parameter changes.
///
/// Issue #539: requires at least 2 unique, active, authorized participants
/// before a cross-border parameter change is committed.  The admin counts
/// as one participant (already authorized by the caller); additional signers
/// from the registered signer set make up the remainder.
///
/// Duplicates are filtered via a Map.  Unregistered addresses are ignored.
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

    for i in 0..signers.len() {
        let signer = signers.get(i).unwrap();

        if seen.contains_key(signer.clone()) {
            continue;
        }
        seen.set(signer.clone(), ());

        let is_registered = authorized_signers.contains_key(signer.clone())
            || data.admin == signer;
        if !is_registered {
            continue;
        }

        let state = get_validator_state(env, &signer);
        let is_active = state == 0 || (state & ACTIVE) != 0;
        if !is_active {
            continue;
        }

        // The admin's auth is already consumed by the outer function before
        // require_multisig is called — do not call require_auth again or the
        // host will abort with a double-auth error.
        if signer != data.admin {
            signer.require_auth();
        }

        valid_count += 1;
    }

    if valid_count < 2 {
        return Err(ContractError::ThresholdNotReached);
    }

    Ok(())
}
