use soroban_sdk::{Address, Env, Map, Vec};
use crate::{ContractData, ContractError, DATA_KEY, SIGNERS_KEY};

/// Rigid multi-signature confirmation barrier for parameter shift actions.
/// Requires a minimum of 2 out of 3 validated administrative signatures
/// before approving changes to system boundary configurations.
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

    let mut valid_count = 0;
    let mut verified: Map<Address, ()> = Map::new(env);

    for i in 0..signers.len() {
        let signer = signers.get(i).unwrap();
        let is_authorized = authorized_signers.contains_key(signer.clone()) || data.admin == signer;
        #[cfg(test)]
        {
            extern crate std;
            std::println!("Checking signer: {:?}, is_authorized: {}, admin: {:?}", signer, is_authorized, data.admin);
        }
        
        if is_authorized && !verified.contains_key(signer.clone()) {
            #[cfg(test)]
            {
                extern crate std;
                std::println!("Calling require_auth on signer: {:?}", signer);
            }
            signer.require_auth();
            #[cfg(test)]
            {
                extern crate std;
                std::println!("require_auth succeeded for signer: {:?}", signer);
            }
            verified.set(signer.clone(), ());
            valid_count += 1;
        }
    }

    // Require a minimum of 2 validated administrative signatures
    if valid_count < 2 {
        return Err(ContractError::ThresholdNotReached);
    }

    Ok(())
}
