//! DAO -> Squads -> TWAP wiring, against the REAL Squads v4 binary.
//!
//! The TWAP config can only ever name a genuine Squads multisig as its controller,
//! and that multisig's `config_authority` is the DAO. So the DAO governs the TWAP
//! (and, through it, percolator insurance) exclusively via the timelocked Squads
//! path — there is no way to point the TWAP at an attacker-controlled "controller".

use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_program,
    transaction::Transaction,
};
use std::path::PathBuf;
use std::str::FromStr;

fn twap_id() -> Pubkey {
    twap_program::id()
}
fn squads_id() -> Pubkey {
    Pubkey::from_str("SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf").unwrap()
}

const IX_MULTISIG_CREATE_V2: [u8; 8] = [50, 221, 199, 93, 40, 245, 139, 233];
const ACCT_PROGRAM_CONFIG: [u8; 8] = [196, 210, 90, 231, 144, 149, 140, 63];
const SEED_PREFIX: &[u8] = b"multisig";
const SEED_PROGRAM_CONFIG: &[u8] = b"program_config";
const SEED_MULTISIG: &[u8] = b"multisig";
const PERM_ALL: u8 = 7;
const TIMELOCK_1_WEEK_SECS: u32 = 7 * 24 * 60 * 60;

fn squads_program_bytes() -> Vec<u8> {
    // Reuse the Squads v4 fixture dumped for the program/ handover tests.
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../program/tests/fixtures/squads_v4.so");
    assert!(path.exists(), "Squads v4 binary missing at {:?}", path);
    std::fs::read(path).unwrap()
}

fn program_config_pda(squads: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[SEED_PREFIX, SEED_PROGRAM_CONFIG], squads).0
}
fn multisig_pda(squads: &Pubkey, create_key: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[SEED_PREFIX, SEED_MULTISIG, create_key.as_ref()], squads).0
}

fn install_squads(svm: &mut LiteSVM, squads: &Pubkey, authority: &Pubkey) -> Pubkey {
    svm.add_program(*squads, &squads_program_bytes());
    let treasury = Keypair::new().pubkey();
    svm.set_account(
        treasury,
        Account { lamports: 1_000_000_000, data: vec![], owner: system_program::ID, executable: false, rent_epoch: 0 },
    )
    .unwrap();
    // ProgramConfig: disc(8) authority(32)@8 fee(u64)@40 treasury(32)@48 reserved[64]@80.
    let mut pc = vec![0u8; 144];
    pc[0..8].copy_from_slice(&ACCT_PROGRAM_CONFIG);
    pc[8..40].copy_from_slice(authority.as_ref());
    pc[48..80].copy_from_slice(treasury.as_ref());
    svm.set_account(
        program_config_pda(squads),
        Account { lamports: 10_000_000, data: pc, owner: *squads, executable: false, rent_epoch: 0 },
    )
    .unwrap();
    treasury
}

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
    data.push(0); // rentCollector: None
    data.push(0); // memo: None
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

fn twap_config_pda(market: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"twap_config", market.as_ref()], &twap_id()).0
}

#[allow(clippy::too_many_arguments)]
fn init_config_ix(
    payer: &Pubkey,
    coin_mint: &Pubkey,
    market: &Pubkey,
    squads_multisig: &Pubkey,
    dao: &Pubkey,
    percolator_program: &Pubkey,
) -> Instruction {
    Instruction {
        program_id: twap_id(),
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(*coin_mint, false),
            AccountMeta::new_readonly(*market, false),
            AccountMeta::new(twap_config_pda(market), false),
            AccountMeta::new_readonly(*squads_multisig, false),
            AccountMeta::new_readonly(*dao, false),
            AccountMeta::new_readonly(*percolator_program, false),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data: vec![0u8], // IX_INIT_CONFIG
    }
}

#[test]
fn twap_config_binds_only_to_a_real_squads_multisig_controlled_by_the_dao() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(
        twap_id(),
        format!("{}/../target/deploy/twap_program.so", env!("CARGO_MANIFEST_DIR")),
    )
    .unwrap();

    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let squads = squads_id();
    let treasury = install_squads(&mut svm, &squads, &payer.pubkey());

    // The DAO (winning genesis futarchy authority).
    let dao = Keypair::new().pubkey();

    // DAO -> Squads: a 1/1 multisig whose config_authority is the DAO, 1-week timelock.
    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(
        &squads,
        &treasury,
        &multisig,
        &create_key.pubkey(),
        &payer.pubkey(),
        Some(&dao), // config_authority = DAO
        1,
        &[(dao, PERM_ALL)],
        TIMELOCK_1_WEEK_SECS,
    );
    let tx = Transaction::new_signed_with_payer(
        &[create_ix],
        Some(&payer.pubkey()),
        &[&payer, &create_key],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx).expect("create DAO-controlled multisig");

    // Sanity (DAO -> Squads): the multisig's config_authority is the DAO.
    // Multisig layout: create_key(32)@8, config_authority(32)@40.
    let ms = svm.get_account(&multisig).unwrap();
    assert_eq!(ms.owner, squads, "multisig owned by Squads");
    let cfg_auth = Pubkey::new_from_array(ms.data[40..72].try_into().unwrap());
    assert_eq!(cfg_auth, dao, "config_authority = DAO");

    let coin_mint = Keypair::new().pubkey();
    let market = Keypair::new().pubkey();
    let percolator_program = Keypair::new().pubkey();

    // NEGATIVE: a controller that is NOT a Squads multisig (a plain system account)
    // is rejected — the TWAP can't be pointed at an arbitrary "controller".
    let fake_controller = Keypair::new().pubkey();
    svm.set_account(
        fake_controller,
        Account { lamports: 1_000_000, data: vec![], owner: system_program::ID, executable: false, rent_epoch: 0 },
    )
    .unwrap();
    let bad = init_config_ix(&payer.pubkey(), &coin_mint, &market, &fake_controller, &dao, &percolator_program);
    let tx = Transaction::new_signed_with_payer(&[bad], Some(&payer.pubkey()), &[&payer], svm.latest_blockhash());
    assert!(svm.send_transaction(tx).is_err(), "controller must be a real Squads multisig");

    // POSITIVE: the genuine DAO-controlled multisig is accepted.
    let good = init_config_ix(&payer.pubkey(), &coin_mint, &market, &multisig, &dao, &percolator_program);
    let tx = Transaction::new_signed_with_payer(&[good], Some(&payer.pubkey()), &[&payer], svm.latest_blockhash());
    svm.send_transaction(tx).expect("genuine Squads controller accepted");

    // TWAP -> (Squads, DAO): the config pins the chain.
    let cfg = svm.get_account(&twap_config_pda(&market)).unwrap();
    assert_eq!(cfg.owner, twap_id());
    let stored_squads = Pubkey::new_from_array(cfg.data[104..136].try_into().unwrap());
    let stored_dao = Pubkey::new_from_array(cfg.data[136..168].try_into().unwrap());
    assert_eq!(stored_squads, multisig, "config controller = the Squads multisig");
    assert_eq!(stored_dao, dao, "config records the DAO");

    // NEGATIVE (DAO->Squads integrity): the multisig is config-controlled by `dao`,
    // so naming a DIFFERENT metadao_futarchy must be rejected — you cannot claim a
    // DAO governs the TWAP through a multisig that DAO does not actually control.
    let other_market = Keypair::new().pubkey();
    let not_the_dao = Keypair::new().pubkey();
    let mismatched =
        init_config_ix(&payer.pubkey(), &coin_mint, &other_market, &multisig, &not_the_dao, &percolator_program);
    let tx = Transaction::new_signed_with_payer(&[mismatched], Some(&payer.pubkey()), &[&payer], svm.latest_blockhash());
    assert!(
        svm.send_transaction(tx).is_err(),
        "controller multisig must be config-controlled by the named DAO"
    );
}
