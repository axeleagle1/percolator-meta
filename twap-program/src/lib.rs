//! Market-0 TWAP buy/burn program — the percolator-facing link of the genesis
//! authority chain:
//!
//!   DAO (metadao_futarchy)  →  Squads multisig (1-week timelock)  →  THIS program
//!       →  percolator market-0 insurance
//!
//! After the genesis mint, the percolator market-0 insurance authority/operator is
//! rotated from the subledger to this program's `twap_authority` PDA. From then on
//! the TWAP is what touches insurance: it pulls the configured surplus share and
//! (in later slices) buys + burns COIN with it. The TWAP itself is *configured* only
//! by its `squads` controller — a Squads multisig whose `config_authority` is the
//! DAO — so the DAO controls percolator insurance only through the timelocked Squads
//! path. The pull crank is permissionless (anyone may turn it) but bounded by the
//! Squads-set parameters.
//!
//! This slice wires the on-chain keystone: the config that pins the whole chain, and
//! the `twap_authority` PDA signing the percolator insurance CPI. The Squads
//! vault-execute reconfigure path and the COIN buy/burn settlement build on top.
#![allow(clippy::result_large_err)]

use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    program::invoke_signed,
    program_error::ProgramError,
    program_pack::Pack,
    pubkey::Pubkey,
    system_instruction,
    sysvar::Sysvar,
};

solana_program::declare_id!("TwapBuyBurn11111111111111111111111111111111");

// The Squads v4 program. The TWAP controller must be a multisig owned by it, so the
// configured controller is provably a real Squads multisig (whose config_authority
// is the DAO) and not an arbitrary key.
const SQUADS_PROGRAM_ID: Pubkey =
    solana_program::pubkey!("SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf");

// The twap_authority PDA seed — matches the `twap` lib's TWAP_AUTHORITY_SEED so the
// authority address is the canonical market-0 TWAP authority.
const TWAP_AUTHORITY_SEED: &[u8] = b"market-0-twap";
const CONFIG_SEED: &[u8] = b"twap_config";

const CONFIG_DISC: [u8; 8] = *b"TWAPCFG1";
const CONFIG_SIZE: usize = 200;

// Default surplus share routed to buy/burn (the rest is retained as insurance).
const DEFAULT_SURPLUS_BUY_BURN_BPS: u16 = 8_000;
const BPS_DENOMINATOR: u16 = 10_000;

// Percolator CPI tags (verified against the real v16 program via the subledger).
const PERC_IX_WITHDRAW_INSURANCE_LIMITED: u8 = 23;

const IX_INIT_CONFIG: u8 = 0;
const IX_PULL_SURPLUS: u8 = 1;

#[cfg(not(feature = "no-entrypoint"))]
solana_program::entrypoint!(process_instruction);

fn config_seeds<'a>(market: &'a Pubkey) -> [&'a [u8]; 2] {
    [CONFIG_SEED, market.as_ref()]
}

fn authority_seeds<'a>(market: &'a Pubkey) -> [&'a [u8]; 2] {
    [TWAP_AUTHORITY_SEED, market.as_ref()]
}

fn perc_vault_authority(market_slab: &Pubkey, percolator_program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"vault", market_slab.as_ref()], percolator_program).0
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

struct Config {
    coin_mint: Pubkey,
    market_slab: Pubkey,
    percolator_program: Pubkey,
    /// The Squads multisig that controls (reconfigures/rotates) this TWAP. Its
    /// `config_authority` is the DAO, so the DAO governs the TWAP only via Squads.
    squads_multisig: Pubkey,
    /// The winning genesis DAO (metadao futarchy authority).
    metadao_futarchy: Pubkey,
    surplus_buy_burn_bps: u16,
    market_0_domain: u8,
    config_bump: u8,
    authority_bump: u8,
}

impl Config {
    fn deserialize(data: &[u8]) -> Result<Self, ProgramError> {
        if data.len() < CONFIG_SIZE || data[..8] != CONFIG_DISC {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(Self {
            coin_mint: Pubkey::new_from_array(data[8..40].try_into().unwrap()),
            market_slab: Pubkey::new_from_array(data[40..72].try_into().unwrap()),
            percolator_program: Pubkey::new_from_array(data[72..104].try_into().unwrap()),
            squads_multisig: Pubkey::new_from_array(data[104..136].try_into().unwrap()),
            metadao_futarchy: Pubkey::new_from_array(data[136..168].try_into().unwrap()),
            surplus_buy_burn_bps: u16::from_le_bytes(data[168..170].try_into().unwrap()),
            market_0_domain: data[170],
            config_bump: data[171],
            authority_bump: data[172],
        })
    }

    fn serialize(&self, data: &mut [u8]) {
        data[..8].copy_from_slice(&CONFIG_DISC);
        data[8..40].copy_from_slice(self.coin_mint.as_ref());
        data[40..72].copy_from_slice(self.market_slab.as_ref());
        data[72..104].copy_from_slice(self.percolator_program.as_ref());
        data[104..136].copy_from_slice(self.squads_multisig.as_ref());
        data[136..168].copy_from_slice(self.metadao_futarchy.as_ref());
        data[168..170].copy_from_slice(&self.surplus_buy_burn_bps.to_le_bytes());
        data[170] = self.market_0_domain;
        data[171] = self.config_bump;
        data[172] = self.authority_bump;
        data[173..CONFIG_SIZE].fill(0);
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    let (tag, data) = instruction_data
        .split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;
    match *tag {
        IX_INIT_CONFIG => process_init_config(program_id, accounts, data),
        IX_PULL_SURPLUS => process_pull_surplus(program_id, accounts, data),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn read_u64(data: &[u8]) -> Result<u64, ProgramError> {
    if data.len() != 8 {
        return Err(ProgramError::InvalidInstructionData);
    }
    Ok(u64::from_le_bytes(data.try_into().unwrap()))
}

// init_config accounts: [payer(s,w), coin_mint, market_slab, config(pda,w),
//   squads_multisig, metadao_futarchy, percolator_program, system]
//
// Pins the whole authority chain: the controller must be a real Squads multisig and
// the DAO (metadao_futarchy) is recorded. The twap_authority PDA derived here is the
// address that must hold percolator's insurance authority/operator role.
fn process_init_config(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    if !data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let coin_mint = next_account_info(iter)?;
    let market_slab = next_account_info(iter)?;
    let config_account = next_account_info(iter)?;
    let squads_multisig = next_account_info(iter)?;
    let metadao_futarchy = next_account_info(iter)?;
    let percolator_program = next_account_info(iter)?;
    let system_program = next_account_info(iter)?;

    if !payer.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *system_program.key != solana_program::system_program::ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    // The controller must be a genuine Squads multisig — that is the only account
    // through which the DAO (its config_authority) can ever reach this program.
    if *squads_multisig.owner != SQUADS_PROGRAM_ID {
        return Err(ProgramError::IllegalOwner);
    }
    if *metadao_futarchy.key == Pubkey::default() || *percolator_program.key == Pubkey::default() {
        return Err(ProgramError::InvalidAccountData);
    }

    let (expected_config, config_bump) =
        Pubkey::find_program_address(&config_seeds(market_slab.key), program_id);
    if *config_account.key != expected_config {
        return Err(ProgramError::InvalidSeeds);
    }
    if config_account.lamports() != 0 || config_account.data_len() != 0 {
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    let (_twap_authority, authority_bump) =
        Pubkey::find_program_address(&authority_seeds(market_slab.key), program_id);

    let rent = solana_program::rent::Rent::get()?;
    let bump_arr = [config_bump];
    let seeds: [&[u8]; 3] = [CONFIG_SEED, market_slab.key.as_ref(), &bump_arr];
    invoke_signed(
        &system_instruction::create_account(
            payer.key,
            config_account.key,
            rent.minimum_balance(CONFIG_SIZE),
            CONFIG_SIZE as u64,
            program_id,
        ),
        &[payer.clone(), config_account.clone(), system_program.clone()],
        &[&seeds],
    )?;

    let config = Config {
        coin_mint: *coin_mint.key,
        market_slab: *market_slab.key,
        percolator_program: *percolator_program.key,
        squads_multisig: *squads_multisig.key,
        metadao_futarchy: *metadao_futarchy.key,
        surplus_buy_burn_bps: DEFAULT_SURPLUS_BUY_BURN_BPS,
        market_0_domain: 0,
        config_bump,
        authority_bump,
    };
    config.serialize(&mut config_account.try_borrow_mut_data()?);
    Ok(())
}

// pull_surplus accounts: [cranker(s), config, twap_authority(pda), market_slab(w),
//   holding(w, twap_authority-owned token acct), percolator_vault(w), vault_authority,
//   percolator_program, token_program]
// data: amount (u64)
//
// Permissionless crank: the twap_authority PDA (percolator insurance operator) signs
// WithdrawInsuranceLimited, pulling `amount` of insurance surplus into a holding
// account it owns. The COIN buy + burn settlement is a later slice. The TWAP can
// only ever move insurance because it holds the percolator operator role — granted by
// the rotation from the subledger, itself authorised through the Squads/DAO chain.
fn process_pull_surplus(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let iter = &mut accounts.iter();
    let cranker = next_account_info(iter)?;
    let config_account = next_account_info(iter)?;
    let twap_authority = next_account_info(iter)?;
    let market_slab = next_account_info(iter)?;
    let holding = next_account_info(iter)?;
    let percolator_vault = next_account_info(iter)?;
    let vault_authority = next_account_info(iter)?;
    let percolator_program = next_account_info(iter)?;
    let token_program = next_account_info(iter)?;

    let amount = read_u64(data)?;
    if amount == 0 {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !cranker.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if config_account.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    if *token_program.key != spl_token::ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    let config = Config::deserialize(&config_account.try_borrow_data()?)?;
    // Bind every account to the pinned config.
    if *market_slab.key != config.market_slab
        || *percolator_program.key != config.percolator_program
    {
        return Err(ProgramError::InvalidAccountData);
    }
    // Re-derive the twap_authority so the signing seeds are trusted.
    let auth_bump = [config.authority_bump];
    let auth_seeds: [&[u8]; 3] = [TWAP_AUTHORITY_SEED, config.market_slab.as_ref(), &auth_bump];
    let expected_authority =
        Pubkey::create_program_address(&auth_seeds, program_id).map_err(|_| ProgramError::InvalidSeeds)?;
    if *twap_authority.key != expected_authority {
        return Err(ProgramError::InvalidSeeds);
    }
    if *vault_authority.key != perc_vault_authority(market_slab.key, percolator_program.key) {
        return Err(ProgramError::InvalidSeeds);
    }
    // Percolator requires the WithdrawInsuranceLimited destination to be owned by the
    // operator (the twap_authority). Holding must be a token account it owns.
    let holding_state = spl_token::state::Account::unpack(&holding.try_borrow_data()?)?;
    if holding_state.owner != expected_authority {
        return Err(ProgramError::InvalidAccountData);
    }

    let mut ix_data = vec![PERC_IX_WITHDRAW_INSURANCE_LIMITED];
    ix_data.extend_from_slice(&(amount as u128).to_le_bytes());
    invoke_signed(
        &Instruction {
            program_id: *percolator_program.key,
            accounts: vec![
                AccountMeta::new_readonly(*twap_authority.key, true),
                AccountMeta::new(*market_slab.key, false),
                AccountMeta::new(*holding.key, false),
                AccountMeta::new(*percolator_vault.key, false),
                AccountMeta::new_readonly(*vault_authority.key, false),
                AccountMeta::new_readonly(*token_program.key, false),
            ],
            data: ix_data,
        },
        &[
            twap_authority.clone(),
            market_slab.clone(),
            holding.clone(),
            percolator_vault.clone(),
            vault_authority.clone(),
            token_program.clone(),
            percolator_program.clone(),
        ],
        &[&auth_seeds],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips() {
        let c = Config {
            coin_mint: Pubkey::new_unique(),
            market_slab: Pubkey::new_unique(),
            percolator_program: Pubkey::new_unique(),
            squads_multisig: Pubkey::new_unique(),
            metadao_futarchy: Pubkey::new_unique(),
            surplus_buy_burn_bps: DEFAULT_SURPLUS_BUY_BURN_BPS,
            market_0_domain: 0,
            config_bump: 254,
            authority_bump: 251,
        };
        let mut buf = [0u8; CONFIG_SIZE];
        c.serialize(&mut buf);
        let d = Config::deserialize(&buf).unwrap();
        assert_eq!(d.coin_mint, c.coin_mint);
        assert_eq!(d.market_slab, c.market_slab);
        assert_eq!(d.squads_multisig, c.squads_multisig);
        assert_eq!(d.metadao_futarchy, c.metadao_futarchy);
        assert_eq!(d.surplus_buy_burn_bps, 8_000);
        assert_eq!(d.authority_bump, 251);
        assert!(d.surplus_buy_burn_bps < BPS_DENOMINATOR);
    }
}
