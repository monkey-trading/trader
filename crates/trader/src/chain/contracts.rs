//! The vault's and the tokens' bindings, generated from the ABIs in `abi/`: the entries the bot uses
//! of the vault's and OpenZeppelin's `ERC20`'s, as the pinned engine commit compiles them
//! (`scripts/dev-stack.sh` checks they still match). The token binding has no transfer: the bot
//! only approves the vault, which alone moves funds, and only to the account.

use alloy_sol_types::{Panic, Revert, SolError, SolInterface, sol};

sol!(
    #[sol(all_derives)]
    Vault,
    "abi/Vault.json"
);

sol!(
    #[sol(all_derives)]
    ERC20,
    "abi/ERC20.json"
);

/// What a revert's data says: the vault's or the token's error with its values, a reason string
/// or panic, or the raw data when it is none of these.
pub fn revert_reason(data: &[u8]) -> String {
    if let Ok(error) = Vault::VaultErrors::abi_decode(data) {
        return format!("{error:?}");
    }
    if let Ok(error) = ERC20::ERC20Errors::abi_decode(data) {
        return format!("{error:?}");
    }
    if let Ok(revert) = Revert::abi_decode(data) {
        return revert.reason;
    }
    if let Ok(panic) = Panic::abi_decode(data) {
        return format!("panic {}", panic.code);
    }
    format!("0x{}", hex::encode(data))
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Address, U256};
    use alloy_sol_types::{SolCall, SolEvent, SolValue};

    use super::*;

    /// The calls the bot builds have the vault's and the token's signatures.
    #[test]
    fn bindings_have_the_contracts_signatures() {
        assert_eq!(Vault::depositCall::SIGNATURE, "deposit(uint256)");
        assert_eq!(Vault::depositForCall::SIGNATURE, "depositFor(address,uint256)");
        assert_eq!(Vault::claimEmissionsCall::SIGNATURE, "claimEmissions(address,uint256,uint64,bytes32[])");
        assert_eq!(
            Vault::withdrawCall::SIGNATURE,
            "withdraw((address,uint256,uint64,uint64,uint64,uint64,uint64),uint64,bytes32[],bytes)"
        );
        assert_eq!(Vault::emissionsClaimedCall::SIGNATURE, "emissionsClaimed(address)");
        assert_eq!(Vault::usedNoncesCall::SIGNATURE, "usedNonces(address,uint64)");
        assert_eq!(Vault::depositCountCall::SIGNATURE, "depositCount()");
        assert_eq!(Vault::depositRecordsCall::SIGNATURE, "depositRecords(uint256)");
        assert_eq!(Vault::Deposited::SIGNATURE, "Deposited(uint256,address,address,uint256)");
        assert_eq!(ERC20::approveCall::SIGNATURE, "approve(address,uint256)");
        assert_eq!(ERC20::allowanceCall::SIGNATURE, "allowance(address,address)");
        // Selectors as `cast sig` computes them, independently of these bindings.
        assert_eq!(Vault::depositCall::SELECTOR, [0xb6, 0xb5, 0x5f, 0x25]);
        assert_eq!(Vault::depositForCall::SELECTOR, [0x2f, 0x4f, 0x21, 0xe2]);
        assert_eq!(ERC20::approveCall::SELECTOR, [0x09, 0x5e, 0xa7, 0xb3]);
    }

    /// The vault's errors, the token's, reason strings, panics and unknown data each read as what
    /// they are.
    #[test]
    fn revert_reasons_name_the_error() {
        let nothing = Vault::NothingToClaim { entitled: U256::from(5), claimed: U256::from(5) }.abi_encode();
        assert_eq!(revert_reason(&nothing), "NothingToClaim(NothingToClaim { entitled: 5, claimed: 5 })");
        let short = ERC20::ERC20InsufficientBalance {
            sender: Address::repeat_byte(1),
            balance: U256::from(1),
            needed: U256::from(2),
        }
        .abi_encode();
        assert!(revert_reason(&short).starts_with("ERC20InsufficientBalance("), "{}", revert_reason(&short));
        let string =
            [&[0x08, 0xc3, 0x79, 0xa0][..], &("transfer amount exceeds balance",).abi_encode_params()].concat();
        assert!(revert_reason(&string).contains("transfer amount exceeds balance"), "{}", revert_reason(&string));
        assert_eq!(revert_reason(&Panic { code: U256::from(0x11) }.abi_encode()), "panic 17");
        assert_eq!(revert_reason(&[0xde, 0xad]), "0xdead");
    }
}
