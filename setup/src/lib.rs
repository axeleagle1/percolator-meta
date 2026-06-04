//! Default genesis COIN setup: a FIXED-SUPPLY SPL mint of 42,000,000 coins.
//!
//! The genesis COIN is fixed-supply *by construction*. The canonical init mints the
//! entire supply once and then REVOKES the mint authority (sets it to `None`), so no
//! party — not the genesis operator, not the eventual MetaDAO that inherits the keys
//! — can ever mint another coin. The freeze authority is left unset too, so no holder
//! can ever be frozen. This makes the distribution a true fixed pie: every COIN that
//! will ever exist is in the destination account after init.
//!
//! `init_fixed_supply_coin_ixs` returns one self-contained instruction sequence that
//! creates the mint, creates the destination token account, mints the full supply,
//! and revokes the mint authority. Sign it with `[payer, mint, destination,
//! mint_authority]`.

use solana_sdk::instruction::Instruction;
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::system_instruction;

/// The default genesis COIN supply, in whole coins: 42 million.
pub const DEFAULT_COIN_SUPPLY_COINS: u64 = 42_000_000;

/// Default mint decimals for the COIN.
pub const DEFAULT_COIN_DECIMALS: u8 = 6;

/// Raw base-unit supply for `coins` at `decimals` (`coins * 10^decimals`).
/// Panics on overflow — the defaults are far from the u64 ceiling.
pub fn raw_supply(coins: u64, decimals: u8) -> u64 {
    coins
        .checked_mul(10u64.checked_pow(decimals as u32).expect("decimals overflow"))
        .expect("supply overflow")
}

/// Raw base-unit supply for the default 42M coins at the default decimals.
pub fn default_raw_supply() -> u64 {
    raw_supply(DEFAULT_COIN_SUPPLY_COINS, DEFAULT_COIN_DECIMALS)
}

/// Build the canonical default-init sequence for a FIXED-SUPPLY COIN mint holding
/// the full supply in `destination` (a token account owned by `destination_owner`).
///
/// Steps: create the mint account, initialize it (no freeze authority), create the
/// destination token account, mint the entire supply into it, then revoke the mint
/// authority so the supply is fixed forever.
///
/// Required signers for the resulting transaction: `payer`, `mint`, `destination`,
/// `mint_authority`. `mint_authority` is transient — it only exists to mint the
/// initial supply and is revoked in the same transaction.
#[allow(clippy::too_many_arguments)]
pub fn init_fixed_supply_coin_ixs(
    payer: &Pubkey,
    mint: &Pubkey,
    mint_authority: &Pubkey,
    destination: &Pubkey,
    destination_owner: &Pubkey,
    decimals: u8,
    coins: u64,
) -> Vec<Instruction> {
    let amount = raw_supply(coins, decimals);
    let rent = Rent::default();
    let mint_len = spl_token::state::Mint::LEN;
    let acct_len = spl_token::state::Account::LEN;
    vec![
        system_instruction::create_account(
            payer,
            mint,
            rent.minimum_balance(mint_len),
            mint_len as u64,
            &spl_token::ID,
        ),
        spl_token::instruction::initialize_mint(
            &spl_token::ID,
            mint,
            mint_authority,
            None, // no freeze authority: holders can never be frozen
            decimals,
        )
        .expect("initialize_mint"),
        system_instruction::create_account(
            payer,
            destination,
            rent.minimum_balance(acct_len),
            acct_len as u64,
            &spl_token::ID,
        ),
        spl_token::instruction::initialize_account(
            &spl_token::ID,
            destination,
            mint,
            destination_owner,
        )
        .expect("initialize_account"),
        spl_token::instruction::mint_to(
            &spl_token::ID,
            mint,
            destination,
            mint_authority,
            &[],
            amount,
        )
        .expect("mint_to"),
        // Revoke the mint authority -> fixed supply, forever.
        spl_token::instruction::set_authority(
            &spl_token::ID,
            mint,
            None,
            spl_token::instruction::AuthorityType::MintTokens,
            mint_authority,
            &[],
        )
        .expect("set_authority"),
    ]
}

/// The default 42M fixed-supply init at the default decimals.
pub fn default_init_coin_ixs(
    payer: &Pubkey,
    mint: &Pubkey,
    mint_authority: &Pubkey,
    destination: &Pubkey,
    destination_owner: &Pubkey,
) -> Vec<Instruction> {
    init_fixed_supply_coin_ixs(
        payer,
        mint,
        mint_authority,
        destination,
        destination_owner,
        DEFAULT_COIN_DECIMALS,
        DEFAULT_COIN_SUPPLY_COINS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_raw_supply_is_42m_scaled() {
        assert_eq!(default_raw_supply(), 42_000_000 * 1_000_000);
        assert_eq!(raw_supply(1, 0), 1);
        assert_eq!(raw_supply(42_000_000, 0), 42_000_000);
    }
}
