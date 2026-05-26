//! Squads v4 handover tests.
//!
//! Standalone harness (does not touch the genesis integration suite). Loads the
//! mainnet-dumped Squads v4 program and exercises the multisig lifecycle that
//! the genesis→DAO authority handover depends on.
//!
//! Fixtures (pulled from mainnet, committed under tests/fixtures):
//!   - squads_v4.so       : Squads v4 program (SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf)
//!
//! Design: the market is born under Squads — the vault PDA holds the percolator
//! market authorities and program upgrade keys from genesis. Control transfers to
//! the winning genesis DAO by *rotating the Squads config_authority*, never by
//! touching percolator's `UpdateAuthority` (so no incoming-authority consent is
//! needed and depositor custody is never re-pointed).
//!
//! Milestones (all against the real mainnet Squads binary in LiteSVM):
//!   M1 `test_create_1of1_multisig_with_48h_timelock`
//!        — create a controlled 1/1 multisig with a 48h timelock.
//!   M3 `test_rotate_config_authority_to_dao`
//!        — hand control to the DAO by rotating config_authority; the old
//!          genesis controller is locked out afterward.
//!   M4 `test_48h_timelock_blocks_then_allows_execution`
//!        — the 48h timelock is enforced: a vault transfer is rejected before
//!          48h and succeeds after.
//!   M5 `test_upgrade_authority_rotated_through_timelock`
//!        — the vault PDA holds a program's upgrade authority and the DAO rotates
//!          it through the timelocked multisig.

use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    bpf_loader_upgradeable,
    clock::Clock,
    compute_budget::ComputeBudgetInstruction,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_program,
    transaction::Transaction,
};
use std::path::PathBuf;
use std::str::FromStr;

// Squads v4 program id (mainnet).
fn squads_id() -> Pubkey {
    Pubkey::from_str("SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf").unwrap()
}

// Anchor discriminators (sha256("global:<ix>") / "account:*")[..8].
const IX_MULTISIG_CREATE_V2: [u8; 8] = [50, 221, 199, 93, 40, 245, 139, 233];
const IX_SET_CONFIG_AUTHORITY: [u8; 8] = [143, 93, 199, 143, 92, 169, 193, 232];
const ACCT_PROGRAM_CONFIG: [u8; 8] = [196, 210, 90, 231, 144, 149, 140, 63];
const ACCT_MULTISIG: [u8; 8] = [224, 116, 121, 186, 68, 161, 79, 236];

// Anchor discriminators for the vault-transaction lifecycle (M4).
const IX_VAULT_TRANSACTION_CREATE: [u8; 8] = [48, 250, 78, 168, 208, 226, 218, 211];
const IX_PROPOSAL_CREATE: [u8; 8] = [220, 60, 73, 224, 30, 108, 79, 159];
const IX_PROPOSAL_APPROVE: [u8; 8] = [144, 37, 164, 136, 188, 216, 42, 248];
const IX_VAULT_TRANSACTION_EXECUTE: [u8; 8] = [194, 8, 161, 87, 153, 164, 25, 171];

// Squads v4 PDA seeds.
const SEED_PREFIX: &[u8] = b"multisig";
const SEED_PROGRAM_CONFIG: &[u8] = b"program_config";
const SEED_MULTISIG: &[u8] = b"multisig";
const SEED_VAULT: &[u8] = b"vault";
const SEED_TRANSACTION: &[u8] = b"transaction";
const SEED_PROPOSAL: &[u8] = b"proposal";

// Permission bits: Initiate=1, Vote=2, Execute=4. All = 7.
const PERM_ALL: u8 = 7;

const TIMELOCK_48H_SECS: u32 = 48 * 60 * 60; // 172_800

fn squads_program_bytes() -> Vec<u8> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures/squads_v4.so");
    assert!(
        path.exists(),
        "Squads v4 binary missing at {:?}. Dump it: solana program dump -u m \
         SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf tests/fixtures/squads_v4.so",
        path
    );
    std::fs::read(path).unwrap()
}

fn program_config_pda(squads: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[SEED_PREFIX, SEED_PROGRAM_CONFIG], squads).0
}

fn multisig_pda(squads: &Pubkey, create_key: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[SEED_PREFIX, SEED_MULTISIG, create_key.as_ref()], squads).0
}

/// Squads vault PDA (index 0) — the address that will receive the handed-over
/// market + upgrade authorities.
fn vault_pda(squads: &Pubkey, multisig: &Pubkey, index: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[SEED_PREFIX, multisig.as_ref(), SEED_VAULT, &[index]],
        squads,
    )
    .0
}

fn transaction_pda(squads: &Pubkey, multisig: &Pubkey, index: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[
            SEED_PREFIX,
            multisig.as_ref(),
            SEED_TRANSACTION,
            &index.to_le_bytes(),
        ],
        squads,
    )
    .0
}

fn proposal_pda(squads: &Pubkey, multisig: &Pubkey, index: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[
            SEED_PREFIX,
            multisig.as_ref(),
            SEED_TRANSACTION,
            &index.to_le_bytes(),
            SEED_PROPOSAL,
        ],
        squads,
    )
    .0
}

/// Install Squads + a fee-free ProgramConfig at the canonical PDA, returning the
/// treasury account key (must be passed to create and equal program_config.treasury).
fn install_squads(svm: &mut LiteSVM, squads: &Pubkey, authority: &Pubkey) -> Pubkey {
    svm.add_program(*squads, &squads_program_bytes());

    let treasury = Keypair::new().pubkey();
    svm.set_account(
        treasury,
        Account {
            lamports: 1_000_000_000,
            data: vec![],
            owner: system_program::ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();

    // ProgramConfig: disc(8) authority(32)@8 fee(u64)@40 treasury(32)@48 reserved[64]@80.
    let mut pc = vec![0u8; 144];
    pc[0..8].copy_from_slice(&ACCT_PROGRAM_CONFIG);
    pc[8..40].copy_from_slice(authority.as_ref());
    // fee @40 = 0
    pc[48..80].copy_from_slice(treasury.as_ref());
    svm.set_account(
        program_config_pda(squads),
        Account {
            lamports: 10_000_000,
            data: pc,
            owner: *squads,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();

    treasury
}

/// Build the `multisig_create_v2` instruction for a controlled 1/1 multisig.
#[allow(clippy::too_many_arguments)]
fn multisig_create_v2_ix(
    squads: &Pubkey,
    treasury: &Pubkey,
    multisig: &Pubkey,
    create_key: &Pubkey,
    creator: &Pubkey,
    config_authority: Option<&Pubkey>,
    threshold: u16,
    members: &[(Pubkey, u8)],
    time_lock: u32,
) -> Instruction {
    let mut data = Vec::with_capacity(128);
    data.extend_from_slice(&IX_MULTISIG_CREATE_V2);
    // args: MultisigCreateArgsV2
    match config_authority {
        Some(k) => {
            data.push(1);
            data.extend_from_slice(k.as_ref());
        }
        None => data.push(0),
    }
    data.extend_from_slice(&threshold.to_le_bytes());
    data.extend_from_slice(&(members.len() as u32).to_le_bytes());
    for (key, mask) in members {
        data.extend_from_slice(key.as_ref());
        data.push(*mask);
    }
    data.extend_from_slice(&time_lock.to_le_bytes());
    data.push(0); // rentCollector: Option = None
    data.push(0); // memo: Option<String> = None

    Instruction {
        program_id: *squads,
        accounts: vec![
            AccountMeta::new_readonly(program_config_pda(squads), false),
            AccountMeta::new(*treasury, false),
            AccountMeta::new(*multisig, false),
            AccountMeta::new_readonly(*create_key, true),
            AccountMeta::new(*creator, true),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data,
    }
}

/// Build `multisig_set_config_authority` — rotate the multisig's config_authority
/// to `new_authority`. Only the *current* config_authority signs; the new one is
/// just an argument (no consent required from the incoming authority).
///
/// Accounts (Squads `MultisigConfig`): multisig(mut), config_authority(signer),
/// rent_payer(Option<Signer>), system_program(Option). For a pure authority swap
/// no reallocation happens, so the two optionals are passed as None — Anchor
/// encodes a `None` optional account by placing the program's own id in the slot.
fn set_config_authority_ix(
    squads: &Pubkey,
    multisig: &Pubkey,
    current_authority: &Pubkey,
    new_authority: &Pubkey,
) -> Instruction {
    let mut data = Vec::with_capacity(48);
    data.extend_from_slice(&IX_SET_CONFIG_AUTHORITY);
    // args: MultisigSetConfigAuthorityArgs { config_authority: Pubkey, memo: Option<String> }
    data.extend_from_slice(new_authority.as_ref());
    data.push(0); // memo: None

    Instruction {
        program_id: *squads,
        accounts: vec![
            AccountMeta::new(*multisig, false),
            AccountMeta::new_readonly(*current_authority, true),
            AccountMeta::new_readonly(*squads, false), // rent_payer: None
            AccountMeta::new_readonly(*squads, false), // system_program: None
        ],
        data,
    }
}

/// Encode a Squads `TransactionMessage` (the compact `SmallVec` form the program
/// deserializes) carrying a single System transfer of `lamports` from the vault
/// PDA to `recipient`.
///
/// account_keys order is mandated by Squads: writable-signers, readonly-signers,
/// writable-non-signers, readonly-non-signers. Here: [vault(ws), recipient(wns),
/// system_program(rns)].
fn build_transfer_message(vault: &Pubkey, recipient: &Pubkey, lamports: u64) -> Vec<u8> {
    let mut m = Vec::new();
    m.push(1); // num_signers (vault)
    m.push(1); // num_writable_signers (vault)
    m.push(1); // num_writable_non_signers (recipient)

    // account_keys: SmallVec<u8, Pubkey>
    m.push(3);
    m.extend_from_slice(vault.as_ref());
    m.extend_from_slice(recipient.as_ref());
    m.extend_from_slice(system_program::ID.as_ref());

    // instructions: SmallVec<u8, CompiledInstruction>
    m.push(1);
    m.push(2); // program_id_index -> system_program
    // account_indexes: SmallVec<u8, u8> = [vault=0, recipient=1]
    m.push(2);
    m.push(0);
    m.push(1);
    // data: SmallVec<u16, u8> = System Transfer { lamports }
    let mut data = Vec::new();
    data.extend_from_slice(&2u32.to_le_bytes()); // SystemInstruction::Transfer
    data.extend_from_slice(&lamports.to_le_bytes());
    m.extend_from_slice(&(data.len() as u16).to_le_bytes());
    m.extend_from_slice(&data);

    // address_table_lookups: SmallVec<u8, _> = empty
    m.push(0);
    m
}

/// Encode a `TransactionMessage` carrying a single BPFLoaderUpgradeable
/// `SetAuthority`, rotating a program's upgrade authority from the vault PDA
/// (current) to `new_authority`. The vault PDA is the only signer.
///
/// account_keys: [vault(readonly-signer), program_data(writable-non-signer),
/// new_authority(readonly-non-signer), loader(readonly-non-signer)].
fn build_set_upgrade_authority_message(
    vault: &Pubkey,
    program_data: &Pubkey,
    new_authority: &Pubkey,
) -> Vec<u8> {
    let mut m = Vec::new();
    m.push(1); // num_signers (vault)
    m.push(0); // num_writable_signers
    m.push(1); // num_writable_non_signers (program_data)

    // account_keys: SmallVec<u8, Pubkey>
    m.push(4);
    m.extend_from_slice(vault.as_ref());
    m.extend_from_slice(program_data.as_ref());
    m.extend_from_slice(new_authority.as_ref());
    m.extend_from_slice(bpf_loader_upgradeable::ID.as_ref());

    // instructions: SmallVec<u8, CompiledInstruction>
    m.push(1);
    m.push(3); // program_id_index -> loader
    // account_indexes: [program_data=1, current_authority(vault)=0, new_authority=2]
    m.push(3);
    m.push(1);
    m.push(0);
    m.push(2);
    // data: SmallVec<u16, u8> = UpgradeableLoaderInstruction::SetAuthority (variant 4)
    let data = 4u32.to_le_bytes();
    m.extend_from_slice(&(data.len() as u16).to_le_bytes());
    m.extend_from_slice(&data);

    // address_table_lookups: empty
    m.push(0);
    m
}

fn vault_transaction_create_ix(
    squads: &Pubkey,
    multisig: &Pubkey,
    transaction: &Pubkey,
    creator: &Pubkey,
    message: &[u8],
) -> Instruction {
    let mut data = Vec::new();
    data.extend_from_slice(&IX_VAULT_TRANSACTION_CREATE);
    data.push(0); // vault_index
    data.push(0); // ephemeral_signers
    data.extend_from_slice(&(message.len() as u32).to_le_bytes()); // Vec<u8> (u32 len)
    data.extend_from_slice(message);
    data.push(0); // memo: None

    Instruction {
        program_id: *squads,
        accounts: vec![
            AccountMeta::new(*multisig, false),
            AccountMeta::new(*transaction, false),
            AccountMeta::new_readonly(*creator, true),
            AccountMeta::new(*creator, true), // rent_payer
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data,
    }
}

fn proposal_create_ix(
    squads: &Pubkey,
    multisig: &Pubkey,
    proposal: &Pubkey,
    creator: &Pubkey,
    transaction_index: u64,
) -> Instruction {
    let mut data = Vec::new();
    data.extend_from_slice(&IX_PROPOSAL_CREATE);
    data.extend_from_slice(&transaction_index.to_le_bytes());
    data.push(0); // draft = false -> Active immediately

    Instruction {
        program_id: *squads,
        accounts: vec![
            AccountMeta::new_readonly(*multisig, false),
            AccountMeta::new(*proposal, false),
            AccountMeta::new_readonly(*creator, true),
            AccountMeta::new(*creator, true), // rent_payer
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data,
    }
}

fn proposal_approve_ix(
    squads: &Pubkey,
    multisig: &Pubkey,
    proposal: &Pubkey,
    member: &Pubkey,
) -> Instruction {
    let mut data = Vec::new();
    data.extend_from_slice(&IX_PROPOSAL_APPROVE);
    data.push(0); // memo: None

    Instruction {
        program_id: *squads,
        accounts: vec![
            AccountMeta::new_readonly(*multisig, false),
            AccountMeta::new(*member, true),
            AccountMeta::new(*proposal, false),
        ],
        data,
    }
}

/// `remaining` must list every account in `message.account_keys` order. The vault
/// PDA appears as a non-signer here even when it signs the inner instruction —
/// Squads provides its signature via invoke_signed.
fn vault_transaction_execute_ix(
    squads: &Pubkey,
    multisig: &Pubkey,
    proposal: &Pubkey,
    transaction: &Pubkey,
    member: &Pubkey,
    remaining: &[AccountMeta],
) -> Instruction {
    let mut data = Vec::new();
    data.extend_from_slice(&IX_VAULT_TRANSACTION_EXECUTE);

    let mut accounts = vec![
        AccountMeta::new_readonly(*multisig, false),
        AccountMeta::new(*proposal, false),
        AccountMeta::new_readonly(*transaction, false),
        AccountMeta::new_readonly(*member, true),
    ];
    accounts.extend_from_slice(remaining);

    Instruction {
        program_id: *squads,
        accounts,
        data,
    }
}

#[test]
fn test_create_1of1_multisig_with_48h_timelock() {
    let mut svm = LiteSVM::new();
    let squads = squads_id();

    let creator = Keypair::new();
    svm.airdrop(&creator.pubkey(), 100_000_000_000).unwrap();

    // The winning genesis DAO key that will control the multisig config.
    let dao = Keypair::new().pubkey();
    let treasury = install_squads(&mut svm, &squads, &dao);

    // Ephemeral create-key seeds the multisig PDA; the single member is the DAO.
    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());

    let ix = multisig_create_v2_ix(
        &squads,
        &treasury,
        &multisig,
        &create_key.pubkey(),
        &creator.pubkey(),
        Some(&dao), // controlled multisig: config_authority = DAO
        1,          // threshold 1/1
        &[(dao, PERM_ALL)],
        TIMELOCK_48H_SECS,
    );

    let cu = ComputeBudgetInstruction::set_compute_unit_limit(400_000);
    let tx = Transaction::new_signed_with_payer(
        &[cu, ix],
        Some(&creator.pubkey()),
        &[&creator, &create_key],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx).expect("multisig_create_v2 failed");

    // Verify the created Multisig account.
    let ms = svm.get_account(&multisig).expect("multisig account exists");
    assert_eq!(ms.owner, squads, "multisig owned by Squads");
    assert_eq!(&ms.data[0..8], &ACCT_MULTISIG, "Multisig discriminator");

    // Layout: create_key(32)@8, config_authority(32)@40, threshold(u16)@72, time_lock(u32)@74.
    let config_authority = Pubkey::new_from_array(ms.data[40..72].try_into().unwrap());
    let threshold = u16::from_le_bytes(ms.data[72..74].try_into().unwrap());
    let time_lock = u32::from_le_bytes(ms.data[74..78].try_into().unwrap());

    assert_eq!(threshold, 1, "1/1 threshold");
    assert_eq!(time_lock, TIMELOCK_48H_SECS, "48h timelock");
    assert_eq!(config_authority, dao, "config authority = DAO (controlled multisig)");

    // The vault PDA (index 0) is derivable — this is the address that will
    // receive the percolator + upgrade authorities in M2.
    let _vault = vault_pda(&squads, &multisig, 0);
}

/// M3: the handover itself. The market is born under Squads (vault PDA already
/// holds the percolator/upgrade authorities), with `config_authority` held by a
/// genesis controller key. Handover to the winning DAO is a single config-authority
/// rotation — no percolator `UpdateAuthority` is ever touched, so no incoming
/// authority needs to consent. After rotation the DAO alone can reconfigure the
/// multisig (members/threshold), i.e. it fully controls the vault PDA's authority.
#[test]
fn test_rotate_config_authority_to_dao() {
    let mut svm = LiteSVM::new();
    let squads = squads_id();

    let creator = Keypair::new();
    svm.airdrop(&creator.pubkey(), 100_000_000_000).unwrap();

    // Genesis controller: holds the multisig config_authority from creation until
    // the genesis vote resolves. Must be able to sign the rotation.
    let genesis_controller = Keypair::new();
    svm.airdrop(&genesis_controller.pubkey(), 100_000_000_000)
        .unwrap();

    let treasury = install_squads(&mut svm, &squads, &genesis_controller.pubkey());

    // Create the controlled multisig under the genesis controller.
    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(
        &squads,
        &treasury,
        &multisig,
        &create_key.pubkey(),
        &creator.pubkey(),
        Some(&genesis_controller.pubkey()),
        1,
        &[(genesis_controller.pubkey(), PERM_ALL)],
        TIMELOCK_48H_SECS,
    );
    let cu = ComputeBudgetInstruction::set_compute_unit_limit(400_000);
    let tx = Transaction::new_signed_with_payer(
        &[cu.clone(), create_ix],
        Some(&creator.pubkey()),
        &[&creator, &create_key],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx).expect("multisig_create_v2 failed");

    // Sanity: config_authority starts as the genesis controller.
    let ms = svm.get_account(&multisig).unwrap();
    let before = Pubkey::new_from_array(ms.data[40..72].try_into().unwrap());
    assert_eq!(before, genesis_controller.pubkey(), "controller pre-rotation");

    // === Handover: rotate config_authority -> winning DAO ===
    let dao = Keypair::new().pubkey();

    // The incoming authority does NOT sign. Only the genesis controller does.
    let rotate_ix = set_config_authority_ix(&squads, &multisig, &genesis_controller.pubkey(), &dao);
    let tx = Transaction::new_signed_with_payer(
        &[cu.clone(), rotate_ix],
        Some(&creator.pubkey()),
        &[&creator, &genesis_controller],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx)
        .expect("set_config_authority rotation failed");

    // The DAO now owns the multisig config authority.
    let ms = svm.get_account(&multisig).unwrap();
    let after = Pubkey::new_from_array(ms.data[40..72].try_into().unwrap());
    assert_eq!(after, dao, "config authority handed to DAO");
    // Timelock and threshold are untouched by the rotation.
    let threshold = u16::from_le_bytes(ms.data[72..74].try_into().unwrap());
    let time_lock = u32::from_le_bytes(ms.data[74..78].try_into().unwrap());
    assert_eq!(threshold, 1, "threshold preserved across handover");
    assert_eq!(time_lock, TIMELOCK_48H_SECS, "48h timelock preserved");

    // The old controller can no longer rotate: its key is stale.
    let stale = set_config_authority_ix(
        &squads,
        &multisig,
        &genesis_controller.pubkey(),
        &Pubkey::new_unique(),
    );
    let tx = Transaction::new_signed_with_payer(
        &[cu, stale],
        Some(&creator.pubkey()),
        &[&creator, &genesis_controller],
        svm.latest_blockhash(),
    );
    assert!(
        svm.send_transaction(tx).is_err(),
        "stale controller must not be able to rotate after handover"
    );
}

/// M4: prove the 48h timelock is *enforced*, not merely stored. The DAO (sole
/// member) drives a vault transaction through its full lifecycle and finds that
/// execution is rejected until 48h have elapsed since approval, then succeeds.
#[test]
fn test_48h_timelock_blocks_then_allows_execution() {
    let mut svm = LiteSVM::new();
    let squads = squads_id();

    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    // The DAO is the single 1/1 member with full permissions.
    let dao = Keypair::new();
    svm.airdrop(&dao.pubkey(), 100_000_000_000).unwrap();

    let treasury = install_squads(&mut svm, &squads, &dao.pubkey());

    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(
        &squads,
        &treasury,
        &multisig,
        &create_key.pubkey(),
        &payer.pubkey(),
        Some(&dao.pubkey()),
        1,
        &[(dao.pubkey(), PERM_ALL)],
        TIMELOCK_48H_SECS,
    );
    let cu = ComputeBudgetInstruction::set_compute_unit_limit(400_000);
    let tx = Transaction::new_signed_with_payer(
        &[cu.clone(), create_ix],
        Some(&payer.pubkey()),
        &[&payer, &create_key],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx).expect("create failed");

    // Fund the vault PDA so the inner transfer has lamports to move.
    let vault = vault_pda(&squads, &multisig, 0);
    svm.airdrop(&vault, 5_000_000_000).unwrap();
    let recipient = Pubkey::new_unique();
    let transfer_amount = 1_000_000_000u64;

    // 1) Create the vault transaction (index 1) carrying a vault->recipient transfer.
    let tx_index = 1u64;
    let transaction = transaction_pda(&squads, &multisig, tx_index);
    let message = build_transfer_message(&vault, &recipient, transfer_amount);
    let vtc = vault_transaction_create_ix(&squads, &multisig, &transaction, &dao.pubkey(), &message);
    let tx = Transaction::new_signed_with_payer(
        &[cu.clone(), vtc],
        Some(&payer.pubkey()),
        &[&payer, &dao],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx).expect("vault_transaction_create failed");

    // 2) Create the proposal (Active) and 3) approve it (1/1 -> Approved).
    let proposal = proposal_pda(&squads, &multisig, tx_index);
    let pc = proposal_create_ix(&squads, &multisig, &proposal, &dao.pubkey(), tx_index);
    let pa = proposal_approve_ix(&squads, &multisig, &proposal, &dao.pubkey());
    let tx = Transaction::new_signed_with_payer(
        &[cu.clone(), pc, pa],
        Some(&payer.pubkey()),
        &[&payer, &dao],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx).expect("proposal create+approve failed");

    // 4a) Execute immediately -> must be rejected by the timelock.
    let exec_remaining = vec![
        AccountMeta::new(vault, false),
        AccountMeta::new(recipient, false),
        AccountMeta::new_readonly(system_program::ID, false),
    ];
    let exec = vault_transaction_execute_ix(
        &squads, &multisig, &proposal, &transaction, &dao.pubkey(), &exec_remaining,
    );
    let tx = Transaction::new_signed_with_payer(
        &[cu.clone(), exec.clone()],
        Some(&payer.pubkey()),
        &[&payer, &dao],
        svm.latest_blockhash(),
    );
    let early = svm.send_transaction(tx);
    assert!(
        early.is_err(),
        "execution before 48h must fail (TimeLockNotReleased)"
    );
    assert_eq!(
        svm.get_account(&recipient).map(|a| a.lamports).unwrap_or(0),
        0,
        "recipient must not have received funds before timelock"
    );

    // 5) Warp the clock past the 48h timelock.
    let mut clock = svm.get_sysvar::<Clock>();
    clock.unix_timestamp += i64::from(TIMELOCK_48H_SECS) + 1;
    svm.set_sysvar::<Clock>(&clock);
    // Fresh blockhash so the retry isn't rejected as a duplicate signature.
    svm.expire_blockhash();

    // 4b) Execute again -> now allowed; recipient receives the funds.
    let tx = Transaction::new_signed_with_payer(
        &[cu, exec],
        Some(&payer.pubkey()),
        &[&payer, &dao],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx)
        .expect("execution after 48h must succeed");
    assert_eq!(
        svm.get_account(&recipient).map(|a| a.lamports).unwrap_or(0),
        transfer_amount,
        "recipient receives funds only after the 48h timelock elapses"
    );
}

const PROGRAMDATA_HEADER_LEN: usize = 45; // u32 variant + u64 slot + Option<Pubkey>

/// Build a BPFLoaderUpgradeable ProgramData account whose upgrade authority is
/// `authority`. Layout: variant(u32=3) slot(u64) Some(1) authority(32).
fn programdata_account(authority: &Pubkey) -> Account {
    let mut data = vec![0u8; PROGRAMDATA_HEADER_LEN + 64];
    data[0..4].copy_from_slice(&3u32.to_le_bytes()); // ProgramData variant
    // slot @4..12 = 0
    data[12] = 1; // Option::Some
    data[13..45].copy_from_slice(authority.as_ref());
    Account {
        lamports: 10_000_000_000,
        data,
        owner: bpf_loader_upgradeable::ID,
        executable: false,
        rent_epoch: 0,
    }
}

fn programdata_authority(acct: &Account) -> Option<Pubkey> {
    if acct.data[12] == 1 {
        Some(Pubkey::new_from_array(acct.data[13..45].try_into().unwrap()))
    } else {
        None
    }
}

/// M5: the upgrade-key handover. A program's upgrade authority is held by the
/// Squads vault PDA (the design: the market is "initialized with Squads", and the
/// DAO controls upgrade keys *through* the multisig). The DAO drives a
/// `set_upgrade_authority` through the timelocked vault transaction; it is blocked
/// until 48h pass, then rotates the upgrade authority to a new key.
#[test]
fn test_upgrade_authority_rotated_through_timelock() {
    let mut svm = LiteSVM::new();
    let squads = squads_id();

    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let dao = Keypair::new();
    svm.airdrop(&dao.pubkey(), 100_000_000_000).unwrap();

    let treasury = install_squads(&mut svm, &squads, &dao.pubkey());

    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(
        &squads,
        &treasury,
        &multisig,
        &create_key.pubkey(),
        &payer.pubkey(),
        Some(&dao.pubkey()),
        1,
        &[(dao.pubkey(), PERM_ALL)],
        TIMELOCK_48H_SECS,
    );
    let cu = ComputeBudgetInstruction::set_compute_unit_limit(400_000);
    let tx = Transaction::new_signed_with_payer(
        &[cu.clone(), create_ix],
        Some(&payer.pubkey()),
        &[&payer, &create_key],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx).expect("create failed");

    // The program's upgrade authority is the vault PDA from the start.
    let vault = vault_pda(&squads, &multisig, 0);
    let program_data = Pubkey::new_unique();
    svm.set_account(program_data, programdata_account(&vault)).unwrap();
    assert_eq!(
        programdata_authority(&svm.get_account(&program_data).unwrap()),
        Some(vault),
        "upgrade authority starts at the vault PDA",
    );

    // Queue a set_upgrade_authority(vault -> new_upgrade_authority) vault transaction.
    let new_upgrade_authority = Pubkey::new_unique();
    let tx_index = 1u64;
    let transaction = transaction_pda(&squads, &multisig, tx_index);
    let message = build_set_upgrade_authority_message(&vault, &program_data, &new_upgrade_authority);
    let vtc = vault_transaction_create_ix(&squads, &multisig, &transaction, &dao.pubkey(), &message);
    let tx = Transaction::new_signed_with_payer(
        &[cu.clone(), vtc],
        Some(&payer.pubkey()),
        &[&payer, &dao],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx).expect("vault_transaction_create failed");

    let proposal = proposal_pda(&squads, &multisig, tx_index);
    let pc = proposal_create_ix(&squads, &multisig, &proposal, &dao.pubkey(), tx_index);
    let pa = proposal_approve_ix(&squads, &multisig, &proposal, &dao.pubkey());
    let tx = Transaction::new_signed_with_payer(
        &[cu.clone(), pc, pa],
        Some(&payer.pubkey()),
        &[&payer, &dao],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx).expect("proposal create+approve failed");

    let exec_remaining = vec![
        AccountMeta::new_readonly(vault, false),
        AccountMeta::new(program_data, false),
        AccountMeta::new_readonly(new_upgrade_authority, false),
        AccountMeta::new_readonly(bpf_loader_upgradeable::ID, false),
    ];
    let exec = vault_transaction_execute_ix(
        &squads, &multisig, &proposal, &transaction, &dao.pubkey(), &exec_remaining,
    );

    // Before the timelock: rejected, authority unchanged.
    let tx = Transaction::new_signed_with_payer(
        &[cu.clone(), exec.clone()],
        Some(&payer.pubkey()),
        &[&payer, &dao],
        svm.latest_blockhash(),
    );
    assert!(
        svm.send_transaction(tx).is_err(),
        "upgrade-authority rotation must be blocked before 48h"
    );
    assert_eq!(
        programdata_authority(&svm.get_account(&program_data).unwrap()),
        Some(vault),
        "authority unchanged before timelock",
    );

    // Warp past the timelock and re-execute.
    let mut clock = svm.get_sysvar::<Clock>();
    clock.unix_timestamp += i64::from(TIMELOCK_48H_SECS) + 1;
    svm.set_sysvar::<Clock>(&clock);
    svm.expire_blockhash();

    let tx = Transaction::new_signed_with_payer(
        &[cu, exec],
        Some(&payer.pubkey()),
        &[&payer, &dao],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx)
        .expect("upgrade-authority rotation must succeed after 48h");
    assert_eq!(
        programdata_authority(&svm.get_account(&program_data).unwrap()),
        Some(new_upgrade_authority),
        "upgrade authority handed over only after the 48h timelock",
    );
}
