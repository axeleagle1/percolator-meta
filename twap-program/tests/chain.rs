//! DAO -> Squads -> TWAP wiring, against the REAL Squads v4 binary.
//!
//! The TWAP config can only ever name a genuine Squads multisig as its controller,
//! and that multisig's `config_authority` is the DAO. So the DAO governs the TWAP
//! (and, through it, percolator insurance) exclusively via the timelocked Squads
//! path — there is no way to point the TWAP at an attacker-controlled "controller".

use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    clock::Clock,
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

fn twap_config_pda(
    market: &Pubkey,
    squads_multisig: &Pubkey,
    coin_mint: &Pubkey,
    percolator_program: &Pubkey,
) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"twap_config",
            market.as_ref(),
            squads_multisig.as_ref(),
            coin_mint.as_ref(),
            percolator_program.as_ref(),
        ],
        &twap_id(),
    )
    .0
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
            AccountMeta::new(twap_config_pda(market, squads_multisig, coin_mint, percolator_program), false),
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
    let cfg = svm.get_account(&twap_config_pda(&market, &multisig, &coin_mint, &percolator_program)).unwrap();
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

    // Squads -> TWAP gating: reconfigure is restricted to the multisig's default
    // vault PDA (the executor of a multisig vault-transaction, reachable only after a
    // DAO proposal clears the timelock). A random signer must be rejected.
    let cfg_pda = twap_config_pda(&market, &multisig, &coin_mint, &percolator_program);
    let squads_vault = Pubkey::find_program_address(
        &[b"multisig", multisig.as_ref(), b"vault", &[0u8]],
        &squads,
    )
    .0;
    let mut data = vec![2u8]; // IX_RECONFIGURE
    data.extend_from_slice(&5_000u16.to_le_bytes());
    let imposter = Keypair::new();
    let bad_reconfig = Instruction {
        program_id: twap_id(),
        accounts: vec![
            AccountMeta::new_readonly(imposter.pubkey(), true), // NOT the squads vault
            AccountMeta::new(cfg_pda, false),
        ],
        data: data.clone(),
    };
    let tx = Transaction::new_signed_with_payer(&[bad_reconfig], Some(&payer.pubkey()), &[&payer, &imposter], svm.latest_blockhash());
    assert!(svm.send_transaction(tx).is_err(), "only the squads vault may reconfigure the TWAP");

    // Even passing the correct vault address but NOT as a signer is rejected.
    let unsigned = Instruction {
        program_id: twap_id(),
        accounts: vec![
            AccountMeta::new_readonly(squads_vault, false), // correct key, not a signer
            AccountMeta::new(cfg_pda, false),
        ],
        data,
    };
    let tx = Transaction::new_signed_with_payer(&[unsigned], Some(&payer.pubkey()), &[&payer], svm.latest_blockhash());
    assert!(svm.send_transaction(tx).is_err(), "the squads vault must actually sign (via a vault-transaction execute)");
}

// --- Squads vault-transaction lifecycle (ported from program/tests/squads_handover) ---
const IX_VAULT_TRANSACTION_CREATE: [u8; 8] = [48, 250, 78, 168, 208, 226, 218, 211];
const IX_PROPOSAL_CREATE: [u8; 8] = [220, 60, 73, 224, 30, 108, 79, 159];
const IX_PROPOSAL_APPROVE: [u8; 8] = [144, 37, 164, 136, 188, 216, 42, 248];
const IX_VAULT_TRANSACTION_EXECUTE: [u8; 8] = [194, 8, 161, 87, 153, 164, 25, 171];
const SEED_VAULT: &[u8] = b"vault";
const SEED_TRANSACTION: &[u8] = b"transaction";
const SEED_PROPOSAL: &[u8] = b"proposal";

fn vault_pda(squads: &Pubkey, multisig: &Pubkey, index: u8) -> Pubkey {
    Pubkey::find_program_address(&[SEED_PREFIX, multisig.as_ref(), SEED_VAULT, &[index]], squads).0
}
fn transaction_pda(squads: &Pubkey, multisig: &Pubkey, index: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[SEED_PREFIX, multisig.as_ref(), SEED_TRANSACTION, &index.to_le_bytes()],
        squads,
    )
    .0
}
fn proposal_pda(squads: &Pubkey, multisig: &Pubkey, index: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[SEED_PREFIX, multisig.as_ref(), SEED_TRANSACTION, &index.to_le_bytes(), SEED_PROPOSAL],
        squads,
    )
    .0
}

// TransactionMessage carrying the twap IX_RECONFIGURE: account_keys
// [vault(readonly-signer), config(writable-non-signer), twap_program(readonly-non-signer)].
fn build_twap_reconfigure_message(vault: &Pubkey, config: &Pubkey, twap_program: &Pubkey, new_bps: u16) -> Vec<u8> {
    let mut m = Vec::new();
    m.push(1); // num_signers (vault)
    m.push(0); // num_writable_signers
    m.push(1); // num_writable_non_signers (config)
    m.push(3); // account_keys count
    m.extend_from_slice(vault.as_ref());
    m.extend_from_slice(config.as_ref());
    m.extend_from_slice(twap_program.as_ref());
    // instructions: 1
    m.push(1);
    m.push(2); // program_id_index -> twap_program
    m.push(2); // account_indexes: [vault=0, config=1]
    m.push(0);
    m.push(1);
    let mut data = vec![2u8]; // IX_RECONFIGURE
    data.extend_from_slice(&new_bps.to_le_bytes());
    m.extend_from_slice(&(data.len() as u16).to_le_bytes());
    m.extend_from_slice(&data);
    m.push(0); // address_table_lookups: empty
    m
}

fn vault_transaction_create_ix(squads: &Pubkey, multisig: &Pubkey, transaction: &Pubkey, creator: &Pubkey, message: &[u8]) -> Instruction {
    let mut data = Vec::new();
    data.extend_from_slice(&IX_VAULT_TRANSACTION_CREATE);
    data.push(0); // vault_index
    data.push(0); // ephemeral_signers
    data.extend_from_slice(&(message.len() as u32).to_le_bytes());
    data.extend_from_slice(message);
    data.push(0); // memo: None
    Instruction {
        program_id: *squads,
        accounts: vec![
            AccountMeta::new(*multisig, false),
            AccountMeta::new(*transaction, false),
            AccountMeta::new_readonly(*creator, true),
            AccountMeta::new(*creator, true),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data,
    }
}
fn proposal_create_ix(squads: &Pubkey, multisig: &Pubkey, proposal: &Pubkey, creator: &Pubkey, transaction_index: u64) -> Instruction {
    let mut data = Vec::new();
    data.extend_from_slice(&IX_PROPOSAL_CREATE);
    data.extend_from_slice(&transaction_index.to_le_bytes());
    data.push(0); // draft = false
    Instruction {
        program_id: *squads,
        accounts: vec![
            AccountMeta::new_readonly(*multisig, false),
            AccountMeta::new(*proposal, false),
            AccountMeta::new_readonly(*creator, true),
            AccountMeta::new(*creator, true),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data,
    }
}
fn proposal_approve_ix(squads: &Pubkey, multisig: &Pubkey, proposal: &Pubkey, member: &Pubkey) -> Instruction {
    let mut data = Vec::new();
    data.extend_from_slice(&IX_PROPOSAL_APPROVE);
    data.push(0);
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
fn vault_transaction_execute_ix(squads: &Pubkey, multisig: &Pubkey, proposal: &Pubkey, transaction: &Pubkey, member: &Pubkey, remaining: &[AccountMeta]) -> Instruction {
    let mut accounts = vec![
        AccountMeta::new_readonly(*multisig, false),
        AccountMeta::new(*proposal, false),
        AccountMeta::new_readonly(*transaction, false),
        AccountMeta::new_readonly(*member, true),
    ];
    accounts.extend_from_slice(remaining);
    Instruction { program_id: *squads, accounts, data: IX_VAULT_TRANSACTION_EXECUTE.to_vec() }
}

fn read_bps(svm: &LiteSVM, config: &Pubkey) -> u16 {
    let d = svm.get_account(config).unwrap().data;
    u16::from_le_bytes(d[168..170].try_into().unwrap())
}

// Finding P regression: init_config is permissionless, so before the PDA committed to
// the bindings an attacker could front-run the real DAO's deployment for a market by
// init'ing the per-market config first with their own throwaway Squads multisig —
// permanently squatting the (market-only) config PDA and bricking the legit deployment.
// Now the config PDA commits to (market, squads_multisig, coin_mint, percolator_program),
// so an attacker's own-multisig config lands at a DIFFERENT address and the real DAO's
// config PDA stays free. (And the only config that CAN exist at the legit address must
// carry the real multisig, which forces the real DAO via the config_authority check —
// covered by the mismatched-DAO negative in the binding test.)
#[test]
fn init_config_front_run_with_attacker_multisig_cannot_block_the_real_deployment() {
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

    // Helper: stand up a real Squads multisig whose config_authority is `dao`.
    let mut make_ms = |svm: &mut LiteSVM, dao: &Pubkey| -> Pubkey {
        let create_key = Keypair::new();
        let multisig = multisig_pda(&squads, &create_key.pubkey());
        let ix = multisig_create_v2_ix(
            &squads, &treasury, &multisig, &create_key.pubkey(), &payer.pubkey(),
            Some(dao), 1, &[(*dao, PERM_ALL)], TIMELOCK_1_WEEK_SECS,
        );
        let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer, &create_key], bh))
            .expect("create multisig");
        multisig
    };

    // The intended deployment bindings (all public).
    let coin_mint = Keypair::new().pubkey();
    let market = Keypair::new().pubkey();
    let percolator_program = Keypair::new().pubkey();

    // The real DAO + its multisig, and an attacker DAO + its own throwaway multisig.
    let real_dao = Keypair::new().pubkey();
    let real_ms = make_ms(&mut svm, &real_dao);
    let attacker_dao = Keypair::new().pubkey();
    let attacker_ms = make_ms(&mut svm, &attacker_dao);

    // ATTACKER FRONT-RUNS: init the config for the REAL market with their OWN multisig.
    // This passes the internal consistency check (their multisig IS config-controlled by
    // their DAO), so it succeeds — but lands at a PDA keyed on the attacker multisig.
    let squat = init_config_ix(&payer.pubkey(), &coin_mint, &market, &attacker_ms, &attacker_dao, &percolator_program);
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[squat], Some(&payer.pubkey()), &[&payer], bh))
        .expect("attacker can init their own config (permissionless) — but at their own PDA");
    let attacker_pda = twap_config_pda(&market, &attacker_ms, &coin_mint, &percolator_program);
    let real_pda = twap_config_pda(&market, &real_ms, &coin_mint, &percolator_program);
    assert_ne!(attacker_pda, real_pda, "the bindings are part of the PDA, so the addresses differ");
    assert!(svm.get_account(&attacker_pda).is_some_and(|a| !a.data.is_empty()), "attacker squatted only their own PDA");
    assert!(svm.get_account(&real_pda).map_or(true, |a| a.data.is_empty()), "the real config PDA is untouched");

    // THE REAL DEPLOYMENT STILL SUCCEEDS: the attacker's front-run did not block it.
    let real = init_config_ix(&payer.pubkey(), &coin_mint, &market, &real_ms, &real_dao, &percolator_program);
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[real], Some(&payer.pubkey()), &[&payer], bh))
        .expect("real DAO deployment is NOT bricked by the front-run (finding P fixed)");
    let cfg = svm.get_account(&real_pda).unwrap();
    let stored_squads = Pubkey::new_from_array(cfg.data[104..136].try_into().unwrap());
    let stored_dao = Pubkey::new_from_array(cfg.data[136..168].try_into().unwrap());
    assert_eq!(stored_squads, real_ms, "the live config is controlled by the REAL multisig");
    assert_eq!(stored_dao, real_dao, "and records the REAL DAO");
}

// KEYSTONE Squads -> TWAP: the surplus buy/burn share can be reconfigured ONLY by a
// DAO proposal that clears the 1-week Squads timelock and is executed by the multisig
// vault. Proven end-to-end against the real Squads v4 binary.
#[test]
fn reconfigure_only_via_squads_vault_execute_after_timelock() {
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

    // DAO is a signer (multisig config_authority + sole member with all perms).
    let dao = Keypair::new();
    svm.airdrop(&dao.pubkey(), 100_000_000_000).unwrap();
    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(
        &squads, &treasury, &multisig, &create_key.pubkey(), &payer.pubkey(),
        Some(&dao.pubkey()), 1, &[(dao.pubkey(), PERM_ALL)], TIMELOCK_1_WEEK_SECS,
    );
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create_ix], Some(&payer.pubkey()), &[&payer, &create_key], bh)).expect("create multisig");

    // Init the twap config controlled by that multisig.
    let coin_mint = Keypair::new().pubkey();
    let market = Keypair::new().pubkey();
    let percolator_program = Keypair::new().pubkey();
    let init = init_config_ix(&payer.pubkey(), &coin_mint, &market, &multisig, &dao.pubkey(), &percolator_program);
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[init], Some(&payer.pubkey()), &[&payer], bh)).expect("init twap config");
    let cfg_pda = twap_config_pda(&market, &multisig, &coin_mint, &percolator_program);
    assert_eq!(read_bps(&svm, &cfg_pda), 8_000, "default buy/burn share");

    // DAO proposes: the vault reconfigures the share to 5000.
    let vault = vault_pda(&squads, &multisig, 0);
    let new_bps = 5_000u16;
    let message = build_twap_reconfigure_message(&vault, &cfg_pda, &twap_id(), new_bps);
    let idx = 1u64;
    let transaction = transaction_pda(&squads, &multisig, idx);
    let proposal = proposal_pda(&squads, &multisig, idx);

    let mut send = |svm: &mut LiteSVM, ixs: &[Instruction], extra: &[&Keypair]| -> Result<(), String> {
        svm.expire_blockhash();
        let bh = svm.latest_blockhash();
        let mut signers: Vec<&Keypair> = vec![&payer];
        signers.extend_from_slice(extra);
        let tx = Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), &signers, bh);
        svm.send_transaction(tx).map(|_| ()).map_err(|e| format!("{:?}", e))
    };

    send(&mut svm, &[vault_transaction_create_ix(&squads, &multisig, &transaction, &dao.pubkey(), &message)], &[&dao]).expect("vault tx create");
    send(&mut svm, &[proposal_create_ix(&squads, &multisig, &proposal, &dao.pubkey(), idx)], &[&dao]).expect("proposal create");
    send(&mut svm, &[proposal_approve_ix(&squads, &multisig, &proposal, &dao.pubkey())], &[&dao]).expect("approve");

    let remaining = vec![
        AccountMeta::new_readonly(vault, false),
        AccountMeta::new(cfg_pda, false),
        AccountMeta::new_readonly(twap_id(), false),
    ];
    let exec = vault_transaction_execute_ix(&squads, &multisig, &proposal, &transaction, &dao.pubkey(), &remaining);

    // Before the timelock elapses: execution is rejected, config unchanged.
    assert!(send(&mut svm, &[exec.clone()], &[&dao]).is_err(), "execute blocked before the 1-week timelock");
    assert_eq!(read_bps(&svm, &cfg_pda), 8_000, "no reconfigure before the timelock");

    // Warp past the 1-week timelock.
    let mut clock = svm.get_sysvar::<Clock>();
    clock.unix_timestamp += i64::from(TIMELOCK_1_WEEK_SECS) + 1;
    svm.set_sysvar::<Clock>(&clock);

    // Now the DAO's reconfigure executes through the vault and CPIs the TWAP.
    send(&mut svm, &[exec], &[&dao]).expect("execute after timelock");
    assert_eq!(read_bps(&svm, &cfg_pda), new_bps, "DAO reconfigured the TWAP via Squads, only after the timelock");

    // The operator-handoff (IX_ACCEPT_OPERATOR) is gated the SAME way: a non-vault
    // signer cannot trigger the percolator insurance-operator rotation. (The positive
    // path — squads execute -> accept_operator -> percolator UpdateAssetAuthority on a
    // real market with asset_admin = the squads vault — is the next slice.)
    let imposter = Keypair::new();
    let twap_authority =
        Pubkey::find_program_address(&[b"market-0-twap", market.as_ref()], &twap_id()).0;
    let bad_accept = Instruction {
        program_id: twap_id(),
        accounts: vec![
            AccountMeta::new_readonly(imposter.pubkey(), true), // NOT the squads vault
            AccountMeta::new_readonly(cfg_pda, false),
            AccountMeta::new_readonly(twap_authority, false),
            AccountMeta::new(market, false),
            AccountMeta::new_readonly(percolator_program, false),
        ],
        data: vec![3u8], // IX_ACCEPT_OPERATOR
    };
    assert!(
        send(&mut svm, &[bad_accept], &[&imposter]).is_err(),
        "only the squads vault may rotate the insurance operator to the TWAP"
    );
}

// --- Percolator handoff e2e (slice 3): squads-execute -> accept_operator -> percolator ---
fn perc_id() -> Pubkey {
    percolator_prog::id()
}
fn perc_so() -> String {
    format!("{}/../../percolator-prog/target/deploy/percolator_prog.so", env!("CARGO_MANIFEST_DIR"))
}

fn make_live_market(slab: &Pubkey, mint: &Pubkey, marketauth: &Pubkey, init_slot: u64) -> Vec<u8> {
    let initial_price = 1_000_000u64;
    let mut wrapper = percolator_prog::state::WrapperConfigV16::default();
    wrapper.marketauth = marketauth.to_bytes();
    wrapper.collateral_mint = mint.to_bytes();
    wrapper.last_good_oracle_slot = init_slot;
    wrapper.insurance_withdraw_max_bps = 10_000;
    wrapper.insurance_withdraw_deposits_only = 1;
    wrapper.insurance_withdraw_cooldown_slots = 0;
    wrapper.permissionless_resolve_stale_slots = 2_000;
    wrapper.force_close_delay_slots = 100;
    wrapper.oracle_mode = percolator_prog::constants::ORACLE_MODE_MANUAL;
    wrapper.mark_ewma_e6 = initial_price;
    wrapper.mark_ewma_last_slot = init_slot;
    wrapper.mark_ewma_halflife_slots = percolator_prog::constants::DEFAULT_MARK_EWMA_HALFLIFE_SLOTS;
    wrapper.oracle_target_price_e6 = initial_price;
    let mut data = vec![0u8; percolator_prog::constants::MARKET_ACCOUNT_LEN];
    let mut cfg = percolator_prog::risk::V16Config::public_user_fund(1, 0, 10);
    cfg.min_nonzero_mm_req = 1;
    cfg.min_nonzero_im_req = 2;
    cfg.maintenance_margin_bps = 10_000;
    cfg.initial_margin_bps = 10_000;
    cfg.max_trading_fee_bps = 10_000;
    cfg.max_accrual_dt_slots = 1;
    cfg.min_funding_lifetime_slots = 1;
    cfg.max_price_move_bps_per_slot = 10_000;
    cfg.max_account_b_settlement_chunks = 1;
    cfg.max_bankrupt_close_chunks = 1;
    cfg.max_bankrupt_close_lifetime_slots = 1;
    cfg.public_b_chunk_atoms = 1;
    percolator_prog::state::init_market_account_zero_copy(&mut data, &wrapper, cfg, slab.to_bytes(), initial_price, init_slot)
        .expect("manual percolator market init");
    data
}

// TransactionMessage carrying the twap IX_ACCEPT_OPERATOR. account_keys (grouped:
// signer first, then writable non-signers, then readonly non-signers):
// [squads_vault(ro-signer), market_slab(w), config, twap_authority, percolator_program, twap_program].
fn build_accept_operator_message(
    squads_vault: &Pubkey, market_slab: &Pubkey, config: &Pubkey,
    twap_authority: &Pubkey, percolator_program: &Pubkey, twap_program: &Pubkey,
) -> Vec<u8> {
    let mut m = Vec::new();
    m.push(1); // num_signers
    m.push(0); // num_writable_signers
    m.push(1); // num_writable_non_signers (market_slab)
    m.push(6); // account_keys count
    m.extend_from_slice(squads_vault.as_ref());      // 0
    m.extend_from_slice(market_slab.as_ref());        // 1 (writable)
    m.extend_from_slice(config.as_ref());             // 2
    m.extend_from_slice(twap_authority.as_ref());     // 3
    m.extend_from_slice(percolator_program.as_ref()); // 4
    m.extend_from_slice(twap_program.as_ref());        // 5 (program id)
    m.push(1); // instructions count
    m.push(5); // program_id_index -> twap_program
    m.push(5); // account_indexes (accept_operator order: vault, config, twap_authority, market, perc)
    m.push(0);
    m.push(2);
    m.push(3);
    m.push(1);
    m.push(4);
    let data = [3u8]; // IX_ACCEPT_OPERATOR
    m.extend_from_slice(&(data.len() as u16).to_le_bytes());
    m.extend_from_slice(&data);
    m.push(0); // address_table_lookups
    m
}

// KEYSTONE slice-3: the asset-0 insurance operator rotates to the twap_authority ONLY
// through a DAO proposal that clears the 1-week Squads timelock and executes the twap
// accept_operator (which CPIs percolator UpdateAssetAuthority). All four real binaries.
#[test]
fn handoff_rotates_operator_to_twap_only_after_timelock() {
    // Percolator needs a larger heap; the nested squads->twap->percolator CPI runs it.
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000,
        heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(twap_id(), format!("{}/../target/deploy/twap_program.so", env!("CARGO_MANIFEST_DIR"))).unwrap();
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let squads = squads_id();
    let treasury = install_squads(&mut svm, &squads, &payer.pubkey());

    let dao = Keypair::new();
    svm.airdrop(&dao.pubkey(), 1_000_000_000_000).unwrap();
    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(
        &squads, &treasury, &multisig, &create_key.pubkey(), &payer.pubkey(),
        Some(&dao.pubkey()), 1, &[(dao.pubkey(), PERM_ALL)], TIMELOCK_1_WEEK_SECS,
    );
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create_ix], Some(&payer.pubkey()), &[&payer, &create_key], bh)).expect("create multisig");
    let squads_vault = vault_pda(&squads, &multisig, 0);

    // market-0 with marketauth = the squads vault (so the vault is the asset-0 asset_admin).
    let dummy_mint = Pubkey::new_unique();
    let slab = Pubkey::new_unique();
    let init_slot = 100u64;
    let slab_data = make_live_market(&slab, &dummy_mint, &squads_vault, init_slot);
    svm.set_account(slab, Account { lamports: 1_000_000_000, data: slab_data, owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
    svm.set_sysvar(&Clock { slot: init_slot, unix_timestamp: 100, ..Clock::default() });

    // twap config controlled by the multisig, for this market.
    let init = init_config_ix(&payer.pubkey(), &dummy_mint, &slab, &multisig, &dao.pubkey(), &perc_id());
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[init], Some(&payer.pubkey()), &[&payer], bh)).expect("twap init");
    let cfg = twap_config_pda(&slab, &multisig, &dummy_mint, &perc_id());
    let twap_authority = Pubkey::find_program_address(&[b"market-0-twap", slab.as_ref()], &twap_id()).0;

    // DAO proposes: accept_operator (rotate the operator to twap_authority).
    let message = build_accept_operator_message(&squads_vault, &slab, &cfg, &twap_authority, &perc_id(), &twap_id());
    let idx = 1u64;
    let transaction = transaction_pda(&squads, &multisig, idx);
    let proposal = proposal_pda(&squads, &multisig, idx);

    let mut send = |svm: &mut LiteSVM, ixs: &[Instruction], extra: &[&Keypair]| -> Result<(), String> {
        svm.expire_blockhash();
        let bh = svm.latest_blockhash();
        let mut signers: Vec<&Keypair> = vec![&payer];
        signers.extend_from_slice(extra);
        svm.send_transaction(Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), &signers, bh)).map(|_| ()).map_err(|e| format!("{:?}", e))
    };
    send(&mut svm, &[vault_transaction_create_ix(&squads, &multisig, &transaction, &dao.pubkey(), &message)], &[&dao]).expect("vault tx create");
    send(&mut svm, &[proposal_create_ix(&squads, &multisig, &proposal, &dao.pubkey(), idx)], &[&dao]).expect("proposal create");
    send(&mut svm, &[proposal_approve_ix(&squads, &multisig, &proposal, &dao.pubkey())], &[&dao]).expect("approve");

    let remaining = vec![
        AccountMeta::new_readonly(squads_vault, false),
        AccountMeta::new(slab, false),
        AccountMeta::new_readonly(cfg, false),
        AccountMeta::new_readonly(twap_authority, false),
        AccountMeta::new_readonly(perc_id(), false),
        AccountMeta::new_readonly(twap_id(), false),
    ];
    let exec = vault_transaction_execute_ix(&squads, &multisig, &proposal, &transaction, &dao.pubkey(), &remaining);

    // Before the timelock: the handoff is blocked.
    assert!(send(&mut svm, &[exec.clone()], &[&dao]).is_err(), "operator handoff blocked before the 1-week timelock");

    // Warp past the timelock and execute: operator rotates subledger/vault -> twap.
    let mut clock = svm.get_sysvar::<Clock>();
    clock.unix_timestamp += i64::from(TIMELOCK_1_WEEK_SECS) + 1;
    svm.set_sysvar::<Clock>(&clock);
    send(&mut svm, &[exec], &[&dao]).expect("handoff executes after timelock (operator -> twap)");
}

// TransactionMessage carrying percolator UpdateInsurancePolicy (tag 33). account_keys
// [squads_vault(ro-signer = marketauth), market_slab(w), percolator_program].
fn build_update_insurance_policy_message(
    squads_vault: &Pubkey, market_slab: &Pubkey, percolator_program: &Pubkey,
    max_bps: u16, deposits_only: u8, cooldown: u64,
) -> Vec<u8> {
    let mut m = Vec::new();
    m.push(1); // num_signers
    m.push(0); // num_writable_signers
    m.push(1); // num_writable_non_signers (market)
    m.push(3); // account_keys count
    m.extend_from_slice(squads_vault.as_ref());       // 0
    m.extend_from_slice(market_slab.as_ref());         // 1 (writable)
    m.extend_from_slice(percolator_program.as_ref());  // 2 (program)
    m.push(1); // instructions count
    m.push(2); // program_id_index -> percolator
    m.push(2); // account_indexes: [squads_vault=0, market=1]
    m.push(0);
    m.push(1);
    let mut data = vec![33u8]; // IX_UPDATE_INSURANCE_POLICY
    data.extend_from_slice(&max_bps.to_le_bytes());
    data.push(deposits_only);
    data.extend_from_slice(&cooldown.to_le_bytes());
    m.extend_from_slice(&(data.len() as u16).to_le_bytes());
    m.extend_from_slice(&data);
    m.push(0); // address_table_lookups
    m
}

// Slice 3 (policy half): the insurance policy can be rotated (principal-only ->
// surplus-only) ONLY through a DAO proposal that clears the 1-week Squads timelock.
// A policy change is dangerous (a wrong one could enable draining principal), so it
// must be timelock-gated. Proven end-to-end: squads-execute -> percolator
// UpdateInsurancePolicy, with the squads vault as the marketauth.
#[test]
fn handoff_rotates_insurance_policy_only_after_timelock() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000,
        heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let squads = squads_id();
    let treasury = install_squads(&mut svm, &squads, &payer.pubkey());

    let dao = Keypair::new();
    svm.airdrop(&dao.pubkey(), 1_000_000_000_000).unwrap();
    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(
        &squads, &treasury, &multisig, &create_key.pubkey(), &payer.pubkey(),
        Some(&dao.pubkey()), 1, &[(dao.pubkey(), PERM_ALL)], TIMELOCK_1_WEEK_SECS,
    );
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create_ix], Some(&payer.pubkey()), &[&payer, &create_key], bh)).expect("create multisig");
    let squads_vault = vault_pda(&squads, &multisig, 0);

    let dummy_mint = Pubkey::new_unique();
    let slab = Pubkey::new_unique();
    let init_slot = 100u64;
    let slab_data = make_live_market(&slab, &dummy_mint, &squads_vault, init_slot);
    svm.set_account(slab, Account { lamports: 1_000_000_000, data: slab_data, owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
    svm.set_sysvar(&Clock { slot: init_slot, unix_timestamp: 100, ..Clock::default() });

    // DAO proposes: rotate to a surplus-only policy (deposits_only=0, max_bps<1e4, cooldown!=0).
    let message = build_update_insurance_policy_message(&squads_vault, &slab, &perc_id(), 8_000, 0, 100);
    let idx = 1u64;
    let transaction = transaction_pda(&squads, &multisig, idx);
    let proposal = proposal_pda(&squads, &multisig, idx);
    let mut send = |svm: &mut LiteSVM, ixs: &[Instruction], extra: &[&Keypair]| -> Result<(), String> {
        svm.expire_blockhash();
        let bh = svm.latest_blockhash();
        let mut signers: Vec<&Keypair> = vec![&payer];
        signers.extend_from_slice(extra);
        svm.send_transaction(Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), &signers, bh)).map(|_| ()).map_err(|e| format!("{:?}", e))
    };
    send(&mut svm, &[vault_transaction_create_ix(&squads, &multisig, &transaction, &dao.pubkey(), &message)], &[&dao]).expect("vault tx create");
    send(&mut svm, &[proposal_create_ix(&squads, &multisig, &proposal, &dao.pubkey(), idx)], &[&dao]).expect("proposal create");
    send(&mut svm, &[proposal_approve_ix(&squads, &multisig, &proposal, &dao.pubkey())], &[&dao]).expect("approve");

    let remaining = vec![
        AccountMeta::new_readonly(squads_vault, false),
        AccountMeta::new(slab, false),
        AccountMeta::new_readonly(perc_id(), false),
    ];
    let exec = vault_transaction_execute_ix(&squads, &multisig, &proposal, &transaction, &dao.pubkey(), &remaining);

    assert!(send(&mut svm, &[exec.clone()], &[&dao]).is_err(), "policy rotation blocked before the 1-week timelock");
    let mut clock = svm.get_sysvar::<Clock>();
    clock.unix_timestamp += i64::from(TIMELOCK_1_WEEK_SECS) + 1;
    svm.set_sysvar::<Clock>(&clock);
    send(&mut svm, &[exec], &[&dao]).expect("policy rotates after the timelock");
}

// ===========================================================================
// Grand-unified E2E: subledger insurance + genesis votes + COIN distribution +
// the DAO->Squads handoff of the percolator insurance operator to the twap, then
// a real surplus pull. All six real binaries in ONE litesvm instance.
//
// Authority model (matches the intended design): the Squads vault is the asset-0
// asset_admin (the key holder). The DAO, via a timelock'd Squads execute, GRANTS the
// insurance operator+authority to the subledger pool for genesis (the pool only
// CONSENTS via accept_operator — it never rotates keys), and later rotates the operator
// onward to the twap. The subledger and twap are pure insurance fund-managers.
// ===========================================================================

fn sub_id() -> Pubkey {
    Pubkey::from_str("Sub1edger1111111111111111111111111111111111").unwrap()
}
fn so_deploy(name: &str) -> String {
    format!("{}/../target/deploy/{}.so", env!("CARGO_MANIFEST_DIR"), name)
}
const ATA_PROGRAM_ID: Pubkey = solana_sdk::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

// Raw SPL token account bytes (mint, owner, amount, Initialized), enough for transfers.
fn token_acct_bytes(mint: &Pubkey, owner: &Pubkey, amount: u64) -> Vec<u8> {
    let mut d = vec![0u8; 165]; // SPL token account length
    d[0..32].copy_from_slice(mint.as_ref());
    d[32..64].copy_from_slice(owner.as_ref());
    d[64..72].copy_from_slice(&amount.to_le_bytes());
    d[108] = 1; // AccountState::Initialized
    d
}
fn set_token(svm: &mut LiteSVM, key: &Pubkey, mint: &Pubkey, owner: &Pubkey, amount: u64) {
    svm.set_account(*key, Account {
        lamports: 2_000_000, data: token_acct_bytes(mint, owner, amount),
        owner: spl_token::ID, executable: false, rent_epoch: 0,
    }).unwrap();
}
fn token_amount(svm: &LiteSVM, key: &Pubkey) -> u64 {
    let a = svm.get_account(key).unwrap();
    u64::from_le_bytes(a.data[64..72].try_into().unwrap())
}

fn sub_pool_pda(collateral_mint: &Pubkey, asset_id: u64, slab: &Pubkey, perc: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"subledger_pool", collateral_mint.as_ref(), &asset_id.to_le_bytes(), slab.as_ref(), perc.as_ref()],
        &sub_id(),
    ).0
}
fn sub_position_pda(pool: &Pubkey, owner: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"subledger_position", pool.as_ref(), owner.as_ref()], &sub_id()).0
}
fn perc_vault_authority(slab: &Pubkey, perc: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"vault", slab.as_ref()], perc).0
}
fn canonical_insurance_vault(vault_authority: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[vault_authority.as_ref(), spl_token::ID.as_ref(), mint.as_ref()], &ATA_PROGRAM_ID).0
}

// Squads TransactionMessage wrapping subledger.accept_operator (the pool consents to
// receive the asset-0 insurance authority+operator from the Squads vault asset_admin).
// subledger.accept_operator accounts: [asset_admin(signer), pool, market_slab(w), perc].
fn build_subledger_accept_operator_message(
    squads_vault: &Pubkey, pool: &Pubkey, market_slab: &Pubkey, percolator_program: &Pubkey,
) -> Vec<u8> {
    let mut m = Vec::new();
    m.push(1); // num_signers
    m.push(0); // num_writable_signers
    m.push(1); // num_writable_non_signers (market_slab)
    m.push(5); // account_keys count
    m.extend_from_slice(squads_vault.as_ref());       // 0 signer (asset_admin)
    m.extend_from_slice(market_slab.as_ref());         // 1 writable
    m.extend_from_slice(pool.as_ref());                // 2
    m.extend_from_slice(percolator_program.as_ref());  // 3
    m.extend_from_slice(sub_id().as_ref());            // 4 program id
    m.push(1); // instructions count
    m.push(4); // program_id_index -> subledger
    m.push(4); // account_indexes count (accept_operator: asset_admin, pool, market, perc)
    m.push(0);
    m.push(2);
    m.push(1);
    m.push(3);
    let data = [7u8]; // IX_ACCEPT_OPERATOR
    m.extend_from_slice(&(data.len() as u16).to_le_bytes());
    m.extend_from_slice(&data);
    m.push(0); // address_table_lookups
    m
}

// Run a full Squads vault-transaction lifecycle (create, propose, approve, warp past the
// 1-week timelock, execute) for `message`. Advances only the unix clock (keeps the slot
// stable so the percolator oracle does not go stale).
#[allow(clippy::too_many_arguments)]
fn squads_execute(
    svm: &mut LiteSVM, squads: &Pubkey, multisig: &Pubkey, dao: &Keypair, payer: &Keypair,
    idx: u64, message: &[u8], remaining: &[AccountMeta],
) -> Result<(), String> {
    let transaction = transaction_pda(squads, multisig, idx);
    let proposal = proposal_pda(squads, multisig, idx);
    let mut send = |svm: &mut LiteSVM, ix: Instruction| -> Result<(), String> {
        svm.expire_blockhash();
        let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[payer, dao], bh))
            .map(|_| ()).map_err(|e| format!("{:?}", e))
    };
    send(svm, vault_transaction_create_ix(squads, multisig, &transaction, &dao.pubkey(), message))?;
    send(svm, proposal_create_ix(squads, multisig, &proposal, &dao.pubkey(), idx))?;
    send(svm, proposal_approve_ix(squads, multisig, &proposal, &dao.pubkey()))?;
    let mut clock = svm.get_sysvar::<Clock>();
    clock.unix_timestamp += i64::from(TIMELOCK_1_WEEK_SECS) + 1;
    svm.set_sysvar::<Clock>(&clock);
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(
        &[vault_transaction_execute_ix(squads, multisig, &proposal, &transaction, &dao.pubkey(), remaining)],
        Some(&payer.pubkey()), &[payer, dao], bh,
    )).map(|_| ()).map_err(|e| format!("{:?}", e))
}

// STAGE A: the DAO, via a timelock'd Squads execute, grants the asset-0 insurance
// authority+operator to the subledger pool (which only consents), and the subledger then
// tops up REAL percolator insurance. Proves the accept_operator bridge end-to-end.
#[test]
fn e2e_squads_grants_operator_to_subledger_then_real_deposit() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let squads = squads_id();
    let treasury = install_squads(&mut svm, &squads, &payer.pubkey());

    // DAO + its 1/1 Squads multisig (config_authority = DAO, 1-week timelock).
    let dao = Keypair::new();
    svm.airdrop(&dao.pubkey(), 1_000_000_000_000).unwrap();
    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(
        &squads, &treasury, &multisig, &create_key.pubkey(), &payer.pubkey(),
        Some(&dao.pubkey()), 1, &[(dao.pubkey(), PERM_ALL)], TIMELOCK_1_WEEK_SECS,
    );
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create_ix], Some(&payer.pubkey()), &[&payer, &create_key], bh)).expect("multisig");
    let squads_vault = vault_pda(&squads, &multisig, 0);

    // market-0 with marketauth = the Squads vault (the vault is the asset-0 asset_admin).
    let collateral_mint = Pubkey::new_unique();
    let slab = Pubkey::new_unique();
    let init_slot = 100u64;
    let slab_data = make_live_market(&slab, &collateral_mint, &squads_vault, init_slot);
    svm.set_account(slab, Account { lamports: 1_000_000_000, data: slab_data, owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
    svm.set_sysvar(&Clock { slot: init_slot, unix_timestamp: 100, ..Clock::default() });

    // The canonical percolator insurance vault + the subledger pool bound to this market.
    let vault_authority = perc_vault_authority(&slab, &perc_id());
    let perc_vault = canonical_insurance_vault(&vault_authority, &collateral_mint);
    set_token(&mut svm, &perc_vault, &collateral_mint, &vault_authority, 0);
    let pool = sub_pool_pda(&collateral_mint, 0, &slab, &perc_id());

    // init the subledger insurance pool (permissionless; vote_authority is a placeholder here).
    let vote_auth = Pubkey::new_unique();
    let mut d = vec![3u8]; // IX_INIT_INSURANCE_POOL
    d.extend_from_slice(&0u64.to_le_bytes()); // asset_id 0
    d.push(0); // POLICY_PRINCIPAL
    let init_pool = Instruction {
        program_id: sub_id(),
        accounts: vec![
            AccountMeta::new(payer.pubkey(), true),
            AccountMeta::new_readonly(collateral_mint, false),
            AccountMeta::new(pool, false),
            AccountMeta::new_readonly(perc_vault, false),
            AccountMeta::new_readonly(slab, false),
            AccountMeta::new_readonly(perc_id(), false),
            AccountMeta::new_readonly(system_program::ID, false),
            AccountMeta::new_readonly(vote_auth, false),
        ],
        data: d,
    };
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[init_pool], Some(&payer.pubkey()), &[&payer], bh)).expect("init insurance pool");

    // DAO -> Squads -> subledger.accept_operator: GRANT the insurance authority+operator
    // to the pool. The Squads vault (asset_admin) co-signs; the pool consents via CPI.
    let message = build_subledger_accept_operator_message(&squads_vault, &pool, &slab, &perc_id());
    let remaining = vec![
        AccountMeta::new_readonly(squads_vault, false),
        AccountMeta::new(slab, false),
        AccountMeta::new_readonly(pool, false),
        AccountMeta::new_readonly(perc_id(), false),
        AccountMeta::new_readonly(sub_id(), false),
    ];
    squads_execute(&mut svm, &squads, &multisig, &dao, &payer, 1, &message, &remaining).expect("squads grants operator to subledger pool");

    // Now the subledger pool is the asset-0 insurance authority: a depositor can top up
    // REAL percolator insurance through it.
    let alice = Keypair::new();
    svm.airdrop(&alice.pubkey(), 1_000_000_000).unwrap();
    let amount = 1_000_000u64;
    let alice_ata = Pubkey::new_unique();
    set_token(&mut svm, &alice_ata, &collateral_mint, &alice.pubkey(), amount);
    let holding = Pubkey::new_unique();
    set_token(&mut svm, &holding, &collateral_mint, &pool, 0);
    let position = sub_position_pda(&pool, &alice.pubkey());

    let mut dd = vec![4u8]; // IX_INSURANCE_DEPOSIT
    dd.extend_from_slice(&amount.to_le_bytes());
    let deposit = Instruction {
        program_id: sub_id(),
        accounts: vec![
            AccountMeta::new(alice.pubkey(), true),
            AccountMeta::new(pool, false),
            AccountMeta::new(position, false),
            AccountMeta::new(alice_ata, false),
            AccountMeta::new(holding, false),
            AccountMeta::new(slab, false),
            AccountMeta::new(perc_vault, false),
            AccountMeta::new_readonly(perc_id(), false),
            AccountMeta::new_readonly(spl_token::ID, false),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data: dd,
    };
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[deposit], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("insurance deposit into real percolator");

    assert_eq!(token_amount(&svm, &perc_vault), amount, "real percolator insurance funded via the granted subledger operator");
    assert_eq!(token_amount(&svm, &alice_ata), 0, "depositor collateral moved into insurance");
}

fn gv_config_pda_e2e(coin_mint: &Pubkey, pool: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"gv_config", coin_mint.as_ref(), pool.as_ref()], &gv_id_e2e()).0
}
fn gv_id_e2e() -> Pubkey { Pubkey::from_str("GenesisVote11111111111111111111111111111111").unwrap() }
fn dist_id_e2e() -> Pubkey { Pubkey::from_str("D1str1but1on11111111111111111111111111111111").unwrap() }
fn dist_config_pda_e2e(coin_mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref()], &dist_id_e2e()).0
}

fn create_real_mint(svm: &mut LiteSVM, payer: &Keypair, authority: &Pubkey) -> Pubkey {
    let mint = Keypair::new();
    let rent = svm.minimum_balance_for_rent_exemption(82);
    let ixs = [
        solana_sdk::system_instruction::create_account(&payer.pubkey(), &mint.pubkey(), rent, 82, &spl_token::ID),
        spl_token::instruction::initialize_mint(&spl_token::ID, &mint.pubkey(), authority, None, 6).unwrap(),
    ];
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[payer, &mint], bh)).unwrap();
    mint.pubkey()
}

// Squads message wrapping percolator TopUpInsurance (tag 9) — inject insurance SURPLUS
// while the Squads vault is still the insurance_authority (before granting to the pool).
fn build_topup_message(squads_vault: &Pubkey, market: &Pubkey, source: &Pubkey, vault: &Pubkey, perc: &Pubkey, amount: u128) -> Vec<u8> {
    let mut m = Vec::new();
    m.push(1); // num_signers
    m.push(0); // num_writable_signers
    m.push(3); // num_writable_non_signers (market, source, vault)
    m.push(6); // account_keys
    m.extend_from_slice(squads_vault.as_ref());  // 0 signer
    m.extend_from_slice(market.as_ref());         // 1 w
    m.extend_from_slice(source.as_ref());         // 2 w
    m.extend_from_slice(vault.as_ref());          // 3 w
    m.extend_from_slice(spl_token::ID.as_ref());  // 4 token program
    m.extend_from_slice(perc.as_ref());           // 5 program
    m.push(1); // instructions
    m.push(5); // program_id_index -> percolator
    m.push(5); // account_indexes: signer, market, source, vault, token_program
    m.push(0); m.push(1); m.push(2); m.push(3); m.push(4);
    let mut data = vec![9u8];
    data.extend_from_slice(&amount.to_le_bytes());
    m.extend_from_slice(&(data.len() as u16).to_le_bytes());
    m.extend_from_slice(&data);
    m.push(0);
    m
}

// Squads message wrapping twap.set_reserved_floor (tag 4) — the DAO sets the surplus floor
// (reserved depositor principal) via the timelock. Accounts: [squads_vault(signer), config(w)].
fn build_set_reserved_floor_message(squads_vault: &Pubkey, config: &Pubkey, floor: u128) -> Vec<u8> {
    let mut m = Vec::new();
    m.push(1); // num_signers
    m.push(0); // num_writable_signers
    m.push(1); // num_writable_non_signers (config)
    m.push(3); // account_keys
    m.extend_from_slice(squads_vault.as_ref()); // 0 signer
    m.extend_from_slice(config.as_ref());        // 1 w
    m.extend_from_slice(twap_id().as_ref());     // 2 program
    m.push(1); // instructions
    m.push(2); // program_id_index -> twap
    m.push(2); // account_indexes: squads_vault, config
    m.push(0); m.push(1);
    let mut data = vec![4u8]; // IX_SET_RESERVED_FLOOR
    data.extend_from_slice(&floor.to_le_bytes());
    m.extend_from_slice(&(data.len() as u16).to_le_bytes());
    m.extend_from_slice(&data);
    m.push(0);
    m
}

// ATTACK PROBE (authority bypass): the subledger.accept_operator grant must be
// unreachable except through the real asset_admin (the Squads vault, behind the 1-week
// timelock). An attacker who calls accept_operator DIRECTLY, signing as a forged
// asset_admin, must be rejected by percolator (the signer is not the asset-0 asset_admin),
// so the timelock cannot be sidestepped by calling the subledger straight.
#[test]
fn e2e_attacker_cannot_grant_operator_bypassing_squads() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let squads = squads_id();
    let treasury = install_squads(&mut svm, &squads, &payer.pubkey());
    let dao = Keypair::new();
    svm.airdrop(&dao.pubkey(), 1_000_000_000_000).unwrap();
    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(&squads, &treasury, &multisig, &create_key.pubkey(), &payer.pubkey(),
        Some(&dao.pubkey()), 1, &[(dao.pubkey(), PERM_ALL)], TIMELOCK_1_WEEK_SECS);
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create_ix], Some(&payer.pubkey()), &[&payer, &create_key], bh)).expect("multisig");
    let squads_vault = vault_pda(&squads, &multisig, 0);

    let collateral_mint = Pubkey::new_unique();
    let slab = Pubkey::new_unique();
    let init_slot = 100u64;
    let slab_data = make_live_market(&slab, &collateral_mint, &squads_vault, init_slot);
    svm.set_account(slab, Account { lamports: 1_000_000_000, data: slab_data, owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
    svm.set_sysvar(&Clock { slot: init_slot, unix_timestamp: 100, ..Clock::default() });
    let vault_authority = perc_vault_authority(&slab, &perc_id());
    let perc_vault = canonical_insurance_vault(&vault_authority, &collateral_mint);
    set_token(&mut svm, &perc_vault, &collateral_mint, &vault_authority, 0);
    let pool = sub_pool_pda(&collateral_mint, 0, &slab, &perc_id());
    let vote_auth = Pubkey::new_unique();
    let mut d = vec![3u8]; d.extend_from_slice(&0u64.to_le_bytes()); d.push(0);
    let init_pool = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true),
        AccountMeta::new_readonly(collateral_mint, false),
        AccountMeta::new(pool, false),
        AccountMeta::new_readonly(perc_vault, false),
        AccountMeta::new_readonly(slab, false),
        AccountMeta::new_readonly(perc_id(), false),
        AccountMeta::new_readonly(system_program::ID, false),
        AccountMeta::new_readonly(vote_auth, false),
    ], data: d };
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[init_pool], Some(&payer.pubkey()), &[&payer], bh)).expect("init pool");

    // ATTACK: call accept_operator DIRECTLY with the attacker as the "asset_admin" signer.
    // The pool consents (its PDA is hardcoded), but percolator's UpdateAssetAuthority
    // rejects because the signer is NOT the asset-0 asset_admin (the Squads vault).
    let attacker = Keypair::new();
    svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    let direct = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new_readonly(attacker.pubkey(), true), // forged asset_admin
        AccountMeta::new_readonly(pool, false),
        AccountMeta::new(slab, false),
        AccountMeta::new_readonly(perc_id(), false),
    ], data: vec![7u8] };
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    let r = svm.send_transaction(Transaction::new_signed_with_payer(&[direct], Some(&payer.pubkey()), &[&payer, &attacker], bh));
    assert!(r.is_err(), "a forged asset_admin must not be able to grant the operator outside the Squads timelock");

    // And the payer themselves (also not the asset_admin) cannot do it either.
    let direct2 = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new_readonly(payer.pubkey(), true),
        AccountMeta::new_readonly(pool, false),
        AccountMeta::new(slab, false),
        AccountMeta::new_readonly(perc_id(), false),
    ], data: vec![7u8] };
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    assert!(svm.send_transaction(Transaction::new_signed_with_payer(&[direct2], Some(&payer.pubkey()), &[&payer], bh)).is_err(),
        "only the real asset_admin (Squads vault, via timelock) can drive the grant");
}

// CANARY: the twap reads the asset-0 `insurance` u128 straight from the market slab at a
// hardcoded offset (twap src INSURANCE_OFFSET). Pin that offset against the REAL percolator
// binary two ways: (1) it must equal MARKET_GROUP_OFF + offset_of!(header, insurance) computed
// from the real percolator struct; (2) fund insurance with a unique value via a Squads TopUp,
// then bump the ADJACENT `vault` field to a different sentinel and assert the read still returns
// the insurance value — proving we read `insurance`, not the (larger) `vault` total. Reading
// `vault` is the finding-O failure class: trader capital would be pulled as "surplus".
#[test]
fn insurance_offset_matches_real_percolator_slab() {
    const INSURANCE_OFFSET: usize = 448 + 301; // must match twap src
    // (1) pin against the real percolator struct.
    use percolator::MarketGroupV16HeaderAccount as H;
    assert_eq!(INSURANCE_OFFSET, 448 + core::mem::offset_of!(H, insurance),
        "INSURANCE_OFFSET drifted from real percolator MarketGroupV16HeaderAccount::insurance");
    let vault_offset = 448 + core::mem::offset_of!(H, vault);
    assert_ne!(vault_offset, INSURANCE_OFFSET, "vault must not alias insurance");
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let squads = squads_id();
    let treasury = install_squads(&mut svm, &squads, &payer.pubkey());
    let dao = Keypair::new();
    svm.airdrop(&dao.pubkey(), 1_000_000_000_000).unwrap();
    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(&squads, &treasury, &multisig, &create_key.pubkey(), &payer.pubkey(),
        Some(&dao.pubkey()), 1, &[(dao.pubkey(), PERM_ALL)], TIMELOCK_1_WEEK_SECS);
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create_ix], Some(&payer.pubkey()), &[&payer, &create_key], bh)).expect("multisig");
    let squads_vault = vault_pda(&squads, &multisig, 0);

    let collateral_mint = Pubkey::new_unique();
    let slab = Pubkey::new_unique();
    let slab_data = make_live_market(&slab, &collateral_mint, &squads_vault, 100);
    svm.set_account(slab, Account { lamports: 1_000_000_000, data: slab_data, owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
    svm.set_sysvar(&Clock { slot: 100, unix_timestamp: 100, ..Clock::default() });
    let vault_authority = perc_vault_authority(&slab, &perc_id());
    let perc_vault = canonical_insurance_vault(&vault_authority, &collateral_mint);
    set_token(&mut svm, &perc_vault, &collateral_mint, &vault_authority, 0);

    // A distinctive insurance amount unlikely to collide elsewhere in the slab.
    let unique: u64 = 0x0000_0A1B_2C3D_4E5F;
    let src = Pubkey::new_unique();
    set_token(&mut svm, &src, &collateral_mint, &squads_vault, unique);
    let msg = build_topup_message(&squads_vault, &slab, &src, &perc_vault, &perc_id(), unique as u128);
    let remaining = vec![
        AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(slab, false), AccountMeta::new(src, false),
        AccountMeta::new(perc_vault, false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(perc_id(), false),
    ];
    squads_execute(&mut svm, &squads, &multisig, &dao, &payer, 1, &msg, &remaining).expect("topup insurance");

    // (2) Bump the adjacent `vault` field to a DISTINCT sentinel (modelling live trader capital
    // sitting in the vault on top of the insurance fund), then confirm the read still returns the
    // insurance value — not the vault total.
    let mut acct = svm.get_account(&slab).unwrap();
    let vault_sentinel: u128 = (unique as u128) + 0x7777_7777;
    acct.data[vault_offset..vault_offset + 16].copy_from_slice(&vault_sentinel.to_le_bytes());
    svm.set_account(slab, acct).unwrap();

    let data = svm.get_account(&slab).unwrap().data;
    let read = u128::from_le_bytes(data[INSURANCE_OFFSET..INSURANCE_OFFSET + 16].try_into().unwrap());
    assert_eq!(read, unique as u128,
        "insurance offset {} drifted — slab byte read ({}) does not match the funded insurance ({}); \
         if it matches the vault sentinel ({}) the offset is reading `vault`, not `insurance`",
        INSURANCE_OFFSET, read, unique, vault_sentinel);
}

// ATTACK PROBE (finding O fix integrity): the surplus floor (reserved_floor) is the only
// thing standing between a permissionless pull_surplus and depositor principal. It must be
// lowerable ONLY by the DAO through a timelock'd Squads execute. An attacker who calls
// set_reserved_floor DIRECTLY (to drop the floor to 0 and re-enable the drain) must be
// rejected — the signer is not the config's Squads vault.
#[test]
fn e2e_attacker_cannot_lower_surplus_floor_without_squads() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(twap_id(), so_deploy("twap_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let squads = squads_id();
    let treasury = install_squads(&mut svm, &squads, &payer.pubkey());
    let dao = Keypair::new();
    svm.airdrop(&dao.pubkey(), 1_000_000_000_000).unwrap();
    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(&squads, &treasury, &multisig, &create_key.pubkey(), &payer.pubkey(),
        Some(&dao.pubkey()), 1, &[(dao.pubkey(), PERM_ALL)], TIMELOCK_1_WEEK_SECS);
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create_ix], Some(&payer.pubkey()), &[&payer, &create_key], bh)).expect("multisig");
    let squads_vault = vault_pda(&squads, &multisig, 0);

    let collateral_mint = Pubkey::new_unique();
    let coin_mint = Pubkey::new_unique();
    let slab = Pubkey::new_unique();
    let slab_data = make_live_market(&slab, &collateral_mint, &squads_vault, 100);
    svm.set_account(slab, Account { lamports: 1_000_000_000, data: slab_data, owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
    svm.set_sysvar(&Clock { slot: 100, unix_timestamp: 100, ..Clock::default() });
    let twap_init = init_config_ix(&payer.pubkey(), &coin_mint, &slab, &multisig, &dao.pubkey(), &perc_id());
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[twap_init], Some(&payer.pubkey()), &[&payer], bh)).expect("twap init");
    let twap_cfg = twap_config_pda(&slab, &multisig, &coin_mint, &perc_id());

    // ATTACK 1: attacker signs as the "squads vault" with their own key -> key mismatch.
    let attacker = Keypair::new();
    svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    let mut d = vec![4u8]; d.extend_from_slice(&0u128.to_le_bytes());
    let direct = Instruction { program_id: twap_id(), accounts: vec![
        AccountMeta::new_readonly(attacker.pubkey(), true), AccountMeta::new(twap_cfg, false),
    ], data: d.clone() };
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    assert!(svm.send_transaction(Transaction::new_signed_with_payer(&[direct], Some(&payer.pubkey()), &[&payer, &attacker], bh)).is_err(),
        "an attacker key cannot lower the surplus floor");

    // ATTACK 2: pass the REAL squads vault but as a non-signer (an attacker has no private
    // key for the PDA, so it can never be a true signer outside a Squads execute).
    let spoof = Instruction { program_id: twap_id(), accounts: vec![
        AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(twap_cfg, false),
    ], data: d };
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    assert!(svm.send_transaction(Transaction::new_signed_with_payer(&[spoof], Some(&payer.pubkey()), &[&payer], bh)).is_err(),
        "the squads vault cannot be spoofed as a non-signer to lower the floor");

    // The floor is untouched (still the u128::MAX default).
    let floor = u128::from_le_bytes(svm.get_account(&twap_cfg).unwrap().data[173..189].try_into().unwrap());
    assert_eq!(floor, u128::MAX, "floor unchanged — only a timelock'd Squads execute can lower it");
}

// ATTACK PROBE (handoff sequencing / liveness lifecycle): the operator handoff to the twap
// closes the subledger exit path — insurance_withdraw signs as the pool, which is the insurance
// OPERATOR only until the handoff. A depositor who has NOT exited before the (1-week-timelock'd)
// handoff can no longer withdraw via the subledger: their principal is protected by the floor
// (the twap can't pull it) but locked. CRUCIALLY the lock is NOT permanent — the DAO can rotate
// the operator BACK to the pool and the depositor exits. This test pins the full lifecycle:
// exit works before the handoff, is blocked after, and is recoverable via a DAO re-grant — so
// a non-exiter's principal is never permanently lost.
#[test]
fn e2e_subledger_exit_blocked_after_operator_handoff() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(twap_id(), so_deploy("twap_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let squads = squads_id();
    let treasury = install_squads(&mut svm, &squads, &payer.pubkey());
    let dao = Keypair::new();
    svm.airdrop(&dao.pubkey(), 1_000_000_000_000).unwrap();
    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(&squads, &treasury, &multisig, &create_key.pubkey(), &payer.pubkey(),
        Some(&dao.pubkey()), 1, &[(dao.pubkey(), PERM_ALL)], TIMELOCK_1_WEEK_SECS);
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create_ix], Some(&payer.pubkey()), &[&payer, &create_key], bh)).expect("multisig");
    let squads_vault = vault_pda(&squads, &multisig, 0);

    let collateral_mint = Pubkey::new_unique();
    let coin_mint = Pubkey::new_unique();
    let slab = Pubkey::new_unique();
    let slab_data = make_live_market(&slab, &collateral_mint, &squads_vault, 100);
    svm.set_account(slab, Account { lamports: 1_000_000_000, data: slab_data, owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
    svm.set_sysvar(&Clock { slot: 100, unix_timestamp: 100, ..Clock::default() });
    let vault_authority = perc_vault_authority(&slab, &perc_id());
    let perc_vault = canonical_insurance_vault(&vault_authority, &collateral_mint);
    set_token(&mut svm, &perc_vault, &collateral_mint, &vault_authority, 0);
    let pool = sub_pool_pda(&collateral_mint, 0, &slab, &perc_id());
    let mut dpool = vec![3u8]; dpool.extend_from_slice(&0u64.to_le_bytes()); dpool.push(0);
    let init_pool = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(collateral_mint, false), AccountMeta::new(pool, false),
        AccountMeta::new_readonly(perc_vault, false), AccountMeta::new_readonly(slab, false), AccountMeta::new_readonly(perc_id(), false),
        AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(Pubkey::new_unique(), false),
    ], data: dpool };
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[init_pool], Some(&payer.pubkey()), &[&payer], bh)).expect("init pool");

    // Grant the operator to the pool, then a depositor funds insurance.
    let grant_msg = build_subledger_accept_operator_message(&squads_vault, &pool, &slab, &perc_id());
    let grant_remaining = vec![
        AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(slab, false), AccountMeta::new_readonly(pool, false),
        AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(sub_id(), false),
    ];
    squads_execute(&mut svm, &squads, &multisig, &dao, &payer, 1, &grant_msg, &grant_remaining).expect("grant operator to pool");
    let alice = Keypair::new();
    svm.airdrop(&alice.pubkey(), 1_000_000_000).unwrap();
    let amount = 1_000_000u64;
    let alice_ata = Pubkey::new_unique();
    set_token(&mut svm, &alice_ata, &collateral_mint, &alice.pubkey(), amount);
    let holding = Pubkey::new_unique();
    set_token(&mut svm, &holding, &collateral_mint, &pool, 0);
    let position = sub_position_pda(&pool, &alice.pubkey());
    let mut dep = vec![4u8]; dep.extend_from_slice(&amount.to_le_bytes());
    let deposit = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new(pool, false), AccountMeta::new(position, false),
        AccountMeta::new(alice_ata, false), AccountMeta::new(holding, false), AccountMeta::new(slab, false),
        AccountMeta::new(perc_vault, false), AccountMeta::new_readonly(perc_id(), false),
        AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(system_program::ID, false),
    ], data: dep };
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[deposit], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("deposit");

    // Sanity: BEFORE the handoff, alice can withdraw (the pool is the operator).
    let withdraw = |amt: u64| Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new(pool, false), AccountMeta::new(position, false),
        AccountMeta::new(alice_ata, false), AccountMeta::new(holding, false), AccountMeta::new(slab, false),
        AccountMeta::new(perc_vault, false), AccountMeta::new_readonly(vault_authority, false),
        AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false),
    ], data: { let mut d = vec![5u8]; d.extend_from_slice(&amt.to_le_bytes()); d } };
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[withdraw(1)], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("pre-handoff exit works");

    // Handoff: rotate the operator to the twap.
    let twap_init = init_config_ix(&payer.pubkey(), &coin_mint, &slab, &multisig, &dao.pubkey(), &perc_id());
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[twap_init], Some(&payer.pubkey()), &[&payer], bh)).expect("twap init");
    let twap_cfg = twap_config_pda(&slab, &multisig, &coin_mint, &perc_id());
    let twap_authority = Pubkey::find_program_address(&[b"market-0-twap", slab.as_ref()], &twap_id()).0;
    let op_msg = build_accept_operator_message(&squads_vault, &slab, &twap_cfg, &twap_authority, &perc_id(), &twap_id());
    let op_remaining = vec![
        AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(slab, false), AccountMeta::new_readonly(twap_cfg, false),
        AccountMeta::new_readonly(twap_authority, false), AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(twap_id(), false),
    ];
    squads_execute(&mut svm, &squads, &multisig, &dao, &payer, 2, &op_msg, &op_remaining).expect("operator -> twap");

    // AFTER the handoff: alice's subledger exit is now rejected — the pool is no longer the
    // insurance operator, so percolator refuses the pool-signed WithdrawInsuranceLimited.
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    assert!(svm.send_transaction(Transaction::new_signed_with_payer(&[withdraw(100)], Some(&payer.pubkey()), &[&payer, &alice], bh)).is_err(),
        "post-handoff the subledger exit path is closed — depositors must exit during the timelock window");

    // RECOVERY: the lock is NOT permanent. The DAO, via a timelock'd Squads execute, rotates
    // the insurance operator+authority BACK to the subledger pool (subledger.accept_operator,
    // which the pool consents to), and alice can then exit her principal. So a non-exiter's
    // principal is never permanently lost — at worst it is locked until the DAO acts.
    let regrant = build_subledger_accept_operator_message(&squads_vault, &pool, &slab, &perc_id());
    let regrant_remaining = vec![
        AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(slab, false), AccountMeta::new_readonly(pool, false),
        AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(sub_id(), false),
    ];
    squads_execute(&mut svm, &squads, &multisig, &dao, &payer, 3, &regrant, &regrant_remaining).expect("DAO re-grants the operator to the pool");
    let before = token_amount(&svm, &alice_ata);
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[withdraw(100)], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("after the DAO re-grant, the depositor can exit again");
    assert_eq!(token_amount(&svm, &alice_ata) - before, 100, "the previously-locked principal is recovered");
}

// ATTACK PROBE (finding S, fixed): the handoff used to rotate only the asset-0 insurance
// OPERATOR (kind 2) to the twap, leaving the pool as the insurance AUTHORITY (kind 1) — so
// subledger insurance_deposit (TopUp) still worked AFTER the handoff. With a STATIC surplus
// floor, such a post-handoff deposit raised insurance above the floor and a cranker drained
// the new principal as "surplus" (LOF). Fix: accept_operator now atomically rotates kind 1 to
// the Squads vault too, so post-handoff deposits are rejected and no unprotected principal can
// enter. This pins that the deposit is blocked after the handoff.
#[test]
fn e2e_post_handoff_deposit_blocked_by_authority_revoke() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(twap_id(), so_deploy("twap_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let squads = squads_id();
    let treasury = install_squads(&mut svm, &squads, &payer.pubkey());
    let dao = Keypair::new();
    svm.airdrop(&dao.pubkey(), 1_000_000_000_000).unwrap();
    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(&squads, &treasury, &multisig, &create_key.pubkey(), &payer.pubkey(),
        Some(&dao.pubkey()), 1, &[(dao.pubkey(), PERM_ALL)], TIMELOCK_1_WEEK_SECS);
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create_ix], Some(&payer.pubkey()), &[&payer, &create_key], bh)).expect("multisig");
    let squads_vault = vault_pda(&squads, &multisig, 0);

    let collateral_mint = Pubkey::new_unique();
    let coin_mint = Pubkey::new_unique();
    let slab = Pubkey::new_unique();
    let slab_data = make_live_market(&slab, &collateral_mint, &squads_vault, 100);
    svm.set_account(slab, Account { lamports: 1_000_000_000, data: slab_data, owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
    svm.set_sysvar(&Clock { slot: 100, unix_timestamp: 100, ..Clock::default() });
    let vault_authority = perc_vault_authority(&slab, &perc_id());
    let perc_vault = canonical_insurance_vault(&vault_authority, &collateral_mint);
    set_token(&mut svm, &perc_vault, &collateral_mint, &vault_authority, 0);
    let pool = sub_pool_pda(&collateral_mint, 0, &slab, &perc_id());
    let mut dpool = vec![3u8]; dpool.extend_from_slice(&0u64.to_le_bytes()); dpool.push(0);
    let init_pool = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(collateral_mint, false), AccountMeta::new(pool, false),
        AccountMeta::new_readonly(perc_vault, false), AccountMeta::new_readonly(slab, false), AccountMeta::new_readonly(perc_id(), false),
        AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(Pubkey::new_unique(), false),
    ], data: dpool };
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[init_pool], Some(&payer.pubkey()), &[&payer], bh)).expect("init pool");
    let grant = build_subledger_accept_operator_message(&squads_vault, &pool, &slab, &perc_id());
    let gr = vec![
        AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(slab, false), AccountMeta::new_readonly(pool, false),
        AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(sub_id(), false),
    ];
    squads_execute(&mut svm, &squads, &multisig, &dao, &payer, 1, &grant, &gr).expect("grant operator to pool");

    // Genesis deposit P = 1,000,000.
    let principal = 1_000_000u64;
    let alice = Keypair::new(); svm.airdrop(&alice.pubkey(), 1_000_000_000).unwrap();
    let alice_ata = Pubkey::new_unique(); set_token(&mut svm, &alice_ata, &collateral_mint, &alice.pubkey(), principal);
    let holding = Pubkey::new_unique(); set_token(&mut svm, &holding, &collateral_mint, &pool, 0);
    let position = sub_position_pda(&pool, &alice.pubkey());
    let deposit = |who: &Pubkey, ata: &Pubkey, pos: &Pubkey, amt: u64| Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(*who, true), AccountMeta::new(pool, false), AccountMeta::new(*pos, false), AccountMeta::new(*ata, false),
        AccountMeta::new(holding, false), AccountMeta::new(slab, false), AccountMeta::new(perc_vault, false),
        AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(system_program::ID, false),
    ], data: { let mut d = vec![4u8]; d.extend_from_slice(&amt.to_le_bytes()); d } };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[deposit(&alice.pubkey(), &alice_ata, &position, principal)], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("genesis deposit");

    // Handoff: policy -> surplus, operator -> twap, floor = the genesis principal.
    let twap_init = init_config_ix(&payer.pubkey(), &coin_mint, &slab, &multisig, &dao.pubkey(), &perc_id());
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[twap_init], Some(&payer.pubkey()), &[&payer], bh)).expect("twap init");
    let twap_cfg = twap_config_pda(&slab, &multisig, &coin_mint, &perc_id());
    let twap_authority = Pubkey::find_program_address(&[b"market-0-twap", slab.as_ref()], &twap_id()).0;
    let pol = build_update_insurance_policy_message(&squads_vault, &slab, &perc_id(), 9_000, 0, 10);
    let pr = vec![AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(slab, false), AccountMeta::new_readonly(perc_id(), false)];
    squads_execute(&mut svm, &squads, &multisig, &dao, &payer, 2, &pol, &pr).expect("policy");
    let op = build_accept_operator_message(&squads_vault, &slab, &twap_cfg, &twap_authority, &perc_id(), &twap_id());
    let or = vec![
        AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(slab, false), AccountMeta::new_readonly(twap_cfg, false),
        AccountMeta::new_readonly(twap_authority, false), AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(twap_id(), false),
    ];
    squads_execute(&mut svm, &squads, &multisig, &dao, &payer, 3, &op, &or).expect("operator -> twap");
    let fm = build_set_reserved_floor_message(&squads_vault, &twap_cfg, principal as u128);
    let fr = vec![AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(twap_cfg, false), AccountMeta::new_readonly(twap_id(), false)];
    squads_execute(&mut svm, &squads, &multisig, &dao, &payer, 4, &fm, &fr).expect("set floor = principal");

    // POST-HANDOFF: a depositor tops up MORE principal (the pool is still the kind-1 authority).
    let new_p = 500_000u64;
    let bob = Keypair::new(); svm.airdrop(&bob.pubkey(), 1_000_000_000).unwrap();
    let bob_ata = Pubkey::new_unique(); set_token(&mut svm, &bob_ata, &collateral_mint, &bob.pubkey(), new_p);
    let bob_pos = sub_position_pda(&pool, &bob.pubkey());
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    let dep_res = svm.send_transaction(Transaction::new_signed_with_payer(&[deposit(&bob.pubkey(), &bob_ata, &bob_pos, new_p)], Some(&payer.pubkey()), &[&payer, &bob], bh));

    // A cranker pulls the "surplus" = insurance - floor = the new deposit's principal.
    let twap_holding = Pubkey::new_unique(); set_token(&mut svm, &twap_holding, &collateral_mint, &twap_authority, 0);
    let mut pd = vec![1u8]; pd.extend_from_slice(&new_p.to_le_bytes());
    let pull = Instruction { program_id: twap_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(twap_cfg, false), AccountMeta::new_readonly(twap_authority, false),
        AccountMeta::new(slab, false), AccountMeta::new(twap_holding, false), AccountMeta::new(perc_vault, false),
        AccountMeta::new_readonly(vault_authority, false), AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false),
    ], data: pd };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    let pull_res = svm.send_transaction(Transaction::new_signed_with_payer(&[pull], Some(&payer.pubkey()), &[&payer], bh));

    // Finding S FIXED: accept_operator atomically rotated the insurance AUTHORITY (kind 1)
    // to the Squads vault, so the post-handoff subledger deposit is REJECTED — no new
    // (unprotected) principal can enter, so there is nothing for a cranker to drain.
    assert!(dep_res.is_err(), "post-handoff deposit must be rejected (insurance authority revoked at handoff)");
    let _ = pull_res; // the pull is moot — no principal entered
    assert_eq!(token_amount(&svm, &twap_holding), 0, "no principal drained");
    assert_eq!(token_amount(&svm, &perc_vault), principal, "insurance is exactly the genesis principal — nothing added, nothing drained");
}

// ATTACK PROBE (flash-deposit vote): vote weight = floor(log2(age)) * principal, so a
// freshly-deposited position (age < 2) has ZERO weight and the gv `vote` must reject it.
// Otherwise a voter could flash-deposit, vote with full principal weight, and exit — buying
// governance influence with no time-at-risk. Pinned end-to-end: alice deposits and votes in
// the SAME slot (rejected), then after holding a few slots her vote succeeds.
#[test]
fn e2e_fresh_position_has_no_vote_weight() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(gv_id_e2e(), so_deploy("genesis_vote_program")).unwrap();
    svm.add_program_from_file(dist_id_e2e(), so_deploy("distribution_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let squads = squads_id();
    let treasury = install_squads(&mut svm, &squads, &payer.pubkey());
    let mint_auth = Keypair::new(); svm.airdrop(&mint_auth.pubkey(), 1_000_000_000).unwrap();
    let dao = Keypair::new(); svm.airdrop(&dao.pubkey(), 1_000_000_000_000).unwrap();
    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(&squads, &treasury, &multisig, &create_key.pubkey(), &payer.pubkey(),
        Some(&dao.pubkey()), 1, &[(dao.pubkey(), PERM_ALL)], TIMELOCK_1_WEEK_SECS);
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create_ix], Some(&payer.pubkey()), &[&payer, &create_key], bh)).expect("multisig");
    let squads_vault = vault_pda(&squads, &multisig, 0);

    let collateral_mint = Pubkey::new_unique();
    let coin_mint = create_real_mint(&mut svm, &payer, &mint_auth.pubkey());
    let slab = Pubkey::new_unique();
    let init_slot = 1000u64;
    let slab_data = make_live_market(&slab, &collateral_mint, &squads_vault, init_slot);
    svm.set_account(slab, Account { lamports: 1_000_000_000, data: slab_data, owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
    svm.set_sysvar(&Clock { slot: init_slot, unix_timestamp: 1000, ..Clock::default() });
    let vault_authority = perc_vault_authority(&slab, &perc_id());
    let perc_vault = canonical_insurance_vault(&vault_authority, &collateral_mint);
    set_token(&mut svm, &perc_vault, &collateral_mint, &vault_authority, 0);
    let pool = sub_pool_pda(&collateral_mint, 0, &slab, &perc_id());
    let gv_config = gv_config_pda_e2e(&coin_mint, &pool);
    let dist_config = dist_config_pda_e2e(&coin_mint);

    let mut dp = vec![3u8]; dp.extend_from_slice(&0u64.to_le_bytes()); dp.push(0);
    let init_pool = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(collateral_mint, false), AccountMeta::new(pool, false),
        AccountMeta::new_readonly(perc_vault, false), AccountMeta::new_readonly(slab, false), AccountMeta::new_readonly(perc_id(), false),
        AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(gv_config, false),
    ], data: dp };
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[init_pool], Some(&payer.pubkey()), &[&payer], bh)).expect("init pool");
    let grant = build_subledger_accept_operator_message(&squads_vault, &pool, &slab, &perc_id());
    let gr = vec![
        AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(slab, false), AccountMeta::new_readonly(pool, false),
        AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(sub_id(), false),
    ];
    squads_execute(&mut svm, &squads, &multisig, &dao, &payer, 1, &grant, &gr).expect("grant operator");

    // distribution: fund a fixed-supply COIN, init dist (authority = gv config) + gv config.
    let total = 100u64;
    let dist_vault = Pubkey::new_unique(); set_token(&mut svm, &dist_vault, &coin_mint, &dist_config, 0);
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[spl_token::instruction::mint_to(&spl_token::ID, &coin_mint, &dist_vault, &mint_auth.pubkey(), &[], total).unwrap()], Some(&payer.pubkey()), &[&payer, &mint_auth], bh)).unwrap();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[spl_token::instruction::set_authority(&spl_token::ID, &coin_mint, None, spl_token::instruction::AuthorityType::MintTokens, &mint_auth.pubkey(), &[]).unwrap()], Some(&payer.pubkey()), &[&payer, &mint_auth], bh)).unwrap();
    let mut di = vec![0u8]; di.extend_from_slice(&1_000_000u64.to_le_bytes()); di.extend_from_slice(&total.to_le_bytes());
    let dist_init = Instruction { program_id: dist_id_e2e(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new(dist_config, false),
        AccountMeta::new_readonly(dist_vault, false), AccountMeta::new_readonly(gv_config, false), AccountMeta::new_readonly(system_program::ID, false),
    ], data: di };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[dist_init], Some(&payer.pubkey()), &[&payer], bh)).expect("dist init");
    let gv_init = Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new(gv_config, false),
        AccountMeta::new_readonly(dist_id_e2e(), false), AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(sub_id(), false),
        AccountMeta::new_readonly(pool, false), AccountMeta::new_readonly(Pubkey::default(), false), AccountMeta::new_readonly(system_program::ID, false),
    ], data: vec![0u8] };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[gv_init], Some(&payer.pubkey()), &[&payer], bh)).expect("gv init");

    // register a proposal.
    let recipient = Pubkey::new_unique();
    let id = 1u64;
    let dist_proposal = Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &id.to_le_bytes()], &dist_id_e2e()).0;
    let mut cd = vec![1u8]; cd.extend_from_slice(&id.to_le_bytes()); cd.extend_from_slice(&4u32.to_le_bytes());
    let create = Instruction { program_id: dist_id_e2e(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(dist_proposal, false), AccountMeta::new_readonly(system_program::ID, false)], data: cd };
    let mut ad = vec![2u8]; ad.extend_from_slice(&1u32.to_le_bytes()); ad.extend_from_slice(recipient.as_ref()); ad.extend_from_slice(&total.to_le_bytes());
    let append = Instruction { program_id: dist_id_e2e(), accounts: vec![AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(dist_proposal, false)], data: ad };
    let gv_proposal = Pubkey::find_program_address(&[b"gv_proposal", gv_config.as_ref(), dist_proposal.as_ref()], &gv_id_e2e()).0;
    let reg = Instruction { program_id: gv_id_e2e(), accounts: vec![AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(gv_config, false), AccountMeta::new(gv_proposal, false), AccountMeta::new_readonly(dist_proposal, false), AccountMeta::new_readonly(system_program::ID, false)], data: vec![2u8] };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create, append, reg], Some(&payer.pubkey()), &[&payer], bh)).expect("create+register");

    // alice deposits — her position.start_slot = the CURRENT slot.
    let alice = Keypair::new(); svm.airdrop(&alice.pubkey(), 1_000_000_000).unwrap();
    let amount = 1_000_000u64;
    let alice_ata = Pubkey::new_unique(); set_token(&mut svm, &alice_ata, &collateral_mint, &alice.pubkey(), amount);
    let holding = Pubkey::new_unique(); set_token(&mut svm, &holding, &collateral_mint, &pool, 0);
    let position = sub_position_pda(&pool, &alice.pubkey());
    let mut dep = vec![4u8]; dep.extend_from_slice(&amount.to_le_bytes());
    let deposit = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new(pool, false), AccountMeta::new(position, false), AccountMeta::new(alice_ata, false),
        AccountMeta::new(holding, false), AccountMeta::new(slab, false), AccountMeta::new(perc_vault, false),
        AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(system_program::ID, false)], data: dep };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[deposit], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("deposit");

    let gv_ballot = Pubkey::find_program_address(&[b"gv_ballot", gv_config.as_ref(), alice.pubkey().as_ref()], &gv_id_e2e()).0;
    let vote = Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new(gv_config, false), AccountMeta::new(gv_ballot, false), AccountMeta::new(gv_proposal, false),
        AccountMeta::new(position, false), AccountMeta::new_readonly(pool, false), AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(sub_id(), false)], data: vec![3u8, 1u8] };

    // SAME-SLOT vote: age = 0 -> weight 0 -> rejected.
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    assert!(svm.send_transaction(Transaction::new_signed_with_payer(&[vote.clone()], Some(&payer.pubkey()), &[&payer, &alice], bh)).is_err(),
        "a freshly-deposited position (age 0) has zero weight and must not be able to vote");

    // After holding a few slots, the vote succeeds.
    let mut c = svm.get_sysvar::<Clock>(); c.slot += 8; svm.set_sysvar::<Clock>(&c);
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[vote], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("vote succeeds once the position has held");
}

// Shared helper: build a fully handed-off market — Squads multisig, market-0 with
// asset_admin = the Squads vault, twap config, insurance funded with principal + surplus,
// policy rotated to surplus-mode, operator handed to the twap, and reserved_floor = principal.
// Returns the key accounts so a probe can focus purely on the attack.
#[allow(dead_code)]
struct HandoffEnv {
    squads: Pubkey, multisig: Pubkey, dao: Keypair, squads_vault: Pubkey,
    slab: Pubkey, collateral_mint: Pubkey, coin_mint: Pubkey, coin_mint_authority: Keypair,
    twap_cfg: Pubkey, twap_authority: Pubkey,
    perc_vault: Pubkey, vault_authority: Pubkey, principal: u64, surplus: u64,
}
fn setup_handoff(svm: &mut LiteSVM, payer: &Keypair) -> HandoffEnv {
    let squads = squads_id();
    let treasury = install_squads(svm, &squads, &payer.pubkey());
    let dao = Keypair::new(); svm.airdrop(&dao.pubkey(), 1_000_000_000_000).unwrap();
    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(&squads, &treasury, &multisig, &create_key.pubkey(), &payer.pubkey(),
        Some(&dao.pubkey()), 1, &[(dao.pubkey(), PERM_ALL)], TIMELOCK_1_WEEK_SECS);
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create_ix], Some(&payer.pubkey()), &[payer, &create_key], bh)).expect("multisig");
    let squads_vault = vault_pda(&squads, &multisig, 0);

    let collateral_mint = Pubkey::new_unique();
    let coin_mint_authority = Keypair::new();
    let coin_mint = create_real_mint(svm, payer, &coin_mint_authority.pubkey());
    let slab = Pubkey::new_unique();
    let slab_data = make_live_market(&slab, &collateral_mint, &squads_vault, 100);
    svm.set_account(slab, Account { lamports: 1_000_000_000, data: slab_data, owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
    svm.set_sysvar(&Clock { slot: 100, unix_timestamp: 100, ..Clock::default() });
    let vault_authority = perc_vault_authority(&slab, &perc_id());
    let perc_vault = canonical_insurance_vault(&vault_authority, &collateral_mint);
    set_token(svm, &perc_vault, &collateral_mint, &vault_authority, 0);
    let twap_init = init_config_ix(&payer.pubkey(), &coin_mint, &slab, &multisig, &dao.pubkey(), &perc_id());
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[twap_init], Some(&payer.pubkey()), &[payer], bh)).expect("twap init");
    let twap_cfg = twap_config_pda(&slab, &multisig, &coin_mint, &perc_id());
    let twap_authority = Pubkey::find_program_address(&[b"market-0-twap", slab.as_ref()], &twap_id()).0;

    let principal = 1_000_000u64;
    let surplus = 500_000u64;
    let src = Pubkey::new_unique();
    set_token(svm, &src, &collateral_mint, &squads_vault, principal + surplus);
    let topup = build_topup_message(&squads_vault, &slab, &src, &perc_vault, &perc_id(), (principal + surplus) as u128);
    let tr = vec![
        AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(slab, false), AccountMeta::new(src, false),
        AccountMeta::new(perc_vault, false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(perc_id(), false),
    ];
    squads_execute(svm, &squads, &multisig, &dao, payer, 1, &topup, &tr).expect("fund insurance");
    let pol = build_update_insurance_policy_message(&squads_vault, &slab, &perc_id(), 9_000, 0, 10);
    let pr = vec![AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(slab, false), AccountMeta::new_readonly(perc_id(), false)];
    squads_execute(svm, &squads, &multisig, &dao, payer, 2, &pol, &pr).expect("policy");
    let op = build_accept_operator_message(&squads_vault, &slab, &twap_cfg, &twap_authority, &perc_id(), &twap_id());
    let or = vec![
        AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(slab, false), AccountMeta::new_readonly(twap_cfg, false),
        AccountMeta::new_readonly(twap_authority, false), AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(twap_id(), false),
    ];
    squads_execute(svm, &squads, &multisig, &dao, payer, 3, &op, &or).expect("operator -> twap");
    let fm = build_set_reserved_floor_message(&squads_vault, &twap_cfg, principal as u128);
    let fr = vec![AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(twap_cfg, false), AccountMeta::new_readonly(twap_id(), false)];
    squads_execute(svm, &squads, &multisig, &dao, payer, 4, &fm, &fr).expect("set floor");

    HandoffEnv { squads, multisig, dao, squads_vault, slab, collateral_mint, coin_mint, coin_mint_authority, twap_cfg, twap_authority, perc_vault, vault_authority, principal, surplus }
}

// Shared helper: a genesis wired up to the point of voting — Squads market (asset_admin =
// vault), subledger insurance pool granted the operator, a fixed-supply COIN, and the
// distribution + genesis-vote configs initialized. Returns the accounts so a probe can focus
// on the vote/claim attack.
#[allow(dead_code)]
struct GenesisEnv {
    dao: Keypair, squads_vault: Pubkey, slab: Pubkey, collateral_mint: Pubkey, coin_mint: Pubkey,
    pool: Pubkey, gv_config: Pubkey, dist_config: Pubkey, dist_vault: Pubkey, perc_vault: Pubkey, mint_auth: Keypair,
}
fn setup_genesis(svm: &mut LiteSVM, payer: &Keypair) -> GenesisEnv {
    let squads = squads_id();
    let treasury = install_squads(svm, &squads, &payer.pubkey());
    let mint_auth = Keypair::new(); svm.airdrop(&mint_auth.pubkey(), 1_000_000_000).unwrap();
    let dao = Keypair::new(); svm.airdrop(&dao.pubkey(), 1_000_000_000_000).unwrap();
    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(&squads, &treasury, &multisig, &create_key.pubkey(), &payer.pubkey(),
        Some(&dao.pubkey()), 1, &[(dao.pubkey(), PERM_ALL)], TIMELOCK_1_WEEK_SECS);
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create_ix], Some(&payer.pubkey()), &[payer, &create_key], bh)).expect("multisig");
    let squads_vault = vault_pda(&squads, &multisig, 0);

    let collateral_mint = Pubkey::new_unique();
    let coin_mint = create_real_mint(svm, payer, &mint_auth.pubkey());
    let slab = Pubkey::new_unique();
    let slab_data = make_live_market(&slab, &collateral_mint, &squads_vault, 1000);
    svm.set_account(slab, Account { lamports: 1_000_000_000, data: slab_data, owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
    svm.set_sysvar(&Clock { slot: 1000, unix_timestamp: 1000, ..Clock::default() });
    let vault_authority = perc_vault_authority(&slab, &perc_id());
    let perc_vault = canonical_insurance_vault(&vault_authority, &collateral_mint);
    set_token(svm, &perc_vault, &collateral_mint, &vault_authority, 0);
    let pool = sub_pool_pda(&collateral_mint, 0, &slab, &perc_id());
    let gv_config = gv_config_pda_e2e(&coin_mint, &pool);
    let dist_config = dist_config_pda_e2e(&coin_mint);
    let mut dp = vec![3u8]; dp.extend_from_slice(&0u64.to_le_bytes()); dp.push(0);
    let init_pool = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(collateral_mint, false), AccountMeta::new(pool, false),
        AccountMeta::new_readonly(perc_vault, false), AccountMeta::new_readonly(slab, false), AccountMeta::new_readonly(perc_id(), false),
        AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(gv_config, false)], data: dp };
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[init_pool], Some(&payer.pubkey()), &[payer], bh)).expect("init pool");
    let grant = build_subledger_accept_operator_message(&squads_vault, &pool, &slab, &perc_id());
    let gr = vec![
        AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(slab, false), AccountMeta::new_readonly(pool, false),
        AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(sub_id(), false)];
    squads_execute(svm, &squads, &multisig, &dao, payer, 1, &grant, &gr).expect("grant operator");

    let total = 100u64;
    let dist_vault = Pubkey::new_unique(); set_token(svm, &dist_vault, &coin_mint, &dist_config, 0);
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[spl_token::instruction::mint_to(&spl_token::ID, &coin_mint, &dist_vault, &mint_auth.pubkey(), &[], total).unwrap()], Some(&payer.pubkey()), &[payer, &mint_auth], bh)).unwrap();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[spl_token::instruction::set_authority(&spl_token::ID, &coin_mint, None, spl_token::instruction::AuthorityType::MintTokens, &mint_auth.pubkey(), &[]).unwrap()], Some(&payer.pubkey()), &[payer, &mint_auth], bh)).unwrap();
    let mut di = vec![0u8]; di.extend_from_slice(&1_000_000u64.to_le_bytes()); di.extend_from_slice(&total.to_le_bytes());
    let dist_init = Instruction { program_id: dist_id_e2e(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new(dist_config, false),
        AccountMeta::new_readonly(dist_vault, false), AccountMeta::new_readonly(gv_config, false), AccountMeta::new_readonly(system_program::ID, false)], data: di };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[dist_init], Some(&payer.pubkey()), &[payer], bh)).expect("dist init");
    let gv_init = Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new(gv_config, false),
        AccountMeta::new_readonly(dist_id_e2e(), false), AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(sub_id(), false),
        AccountMeta::new_readonly(pool, false), AccountMeta::new_readonly(Pubkey::default(), false), AccountMeta::new_readonly(system_program::ID, false)], data: vec![0u8] };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[gv_init], Some(&payer.pubkey()), &[payer], bh)).expect("gv init");
    GenesisEnv { dao, squads_vault, slab, collateral_mint, coin_mint, pool, gv_config, dist_config, dist_vault, perc_vault, mint_auth }
}

// register a one-entry proposal allocating the whole supply to `dest`; returns (dist, gv) proposals.
fn register_proposal(svm: &mut LiteSVM, payer: &Keypair, env: &GenesisEnv, id: u64, dest: &Pubkey, amount: u64) -> (Pubkey, Pubkey) {
    let dist_proposal = Pubkey::find_program_address(&[b"dist_proposal", env.dist_config.as_ref(), &id.to_le_bytes()], &dist_id_e2e()).0;
    let mut cd = vec![1u8]; cd.extend_from_slice(&id.to_le_bytes()); cd.extend_from_slice(&4u32.to_le_bytes());
    let create = Instruction { program_id: dist_id_e2e(), accounts: vec![AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(env.dist_config, false), AccountMeta::new(dist_proposal, false), AccountMeta::new_readonly(system_program::ID, false)], data: cd };
    let mut ad = vec![2u8]; ad.extend_from_slice(&1u32.to_le_bytes()); ad.extend_from_slice(dest.as_ref()); ad.extend_from_slice(&amount.to_le_bytes());
    let append = Instruction { program_id: dist_id_e2e(), accounts: vec![AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(env.dist_config, false), AccountMeta::new(dist_proposal, false)], data: ad };
    let gv_proposal = Pubkey::find_program_address(&[b"gv_proposal", env.gv_config.as_ref(), dist_proposal.as_ref()], &gv_id_e2e()).0;
    let reg = Instruction { program_id: gv_id_e2e(), accounts: vec![AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(env.gv_config, false), AccountMeta::new(gv_proposal, false), AccountMeta::new_readonly(dist_proposal, false), AccountMeta::new_readonly(system_program::ID, false)], data: vec![2u8] };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create, append, reg], Some(&payer.pubkey()), &[payer], bh)).expect("create+register");
    (dist_proposal, gv_proposal)
}

// ATTACK PROBE (vote splitting / double influence): one voter, one proposal. A voter who has
// a LIVE ballot on proposal A must not be able to also back proposal B — that would split or
// double-count their capital weight across proposals. The gv `vote` rejects backing a
// different proposal while a ballot is live; the voter must retract A first.
#[test]
fn e2e_voter_cannot_back_two_proposals_without_retracting() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(gv_id_e2e(), so_deploy("genesis_vote_program")).unwrap();
    svm.add_program_from_file(dist_id_e2e(), so_deploy("distribution_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_genesis(&mut svm, &payer);

    let a_dest = Pubkey::new_unique();
    let b_dest = Pubkey::new_unique();
    let (_da, gv_a) = register_proposal(&mut svm, &payer, &env, 1, &a_dest, 100);
    let (_db, gv_b) = register_proposal(&mut svm, &payer, &env, 2, &b_dest, 100);

    // alice deposits, then holds so her position has weight.
    let alice = Keypair::new(); svm.airdrop(&alice.pubkey(), 1_000_000_000).unwrap();
    let amount = 1_000_000u64;
    let alice_ata = Pubkey::new_unique(); set_token(&mut svm, &alice_ata, &env.collateral_mint, &alice.pubkey(), amount);
    let holding = Pubkey::new_unique(); set_token(&mut svm, &holding, &env.collateral_mint, &env.pool, 0);
    let position = sub_position_pda(&env.pool, &alice.pubkey());
    let mut dep = vec![4u8]; dep.extend_from_slice(&amount.to_le_bytes());
    let deposit = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new(env.pool, false), AccountMeta::new(position, false), AccountMeta::new(alice_ata, false),
        AccountMeta::new(holding, false), AccountMeta::new(env.slab, false), AccountMeta::new(env.perc_vault, false),
        AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(system_program::ID, false)], data: dep };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[deposit], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("deposit");
    let mut c = svm.get_sysvar::<Clock>(); c.slot += 8; svm.set_sysvar::<Clock>(&c);

    let gv_ballot = Pubkey::find_program_address(&[b"gv_ballot", env.gv_config.as_ref(), alice.pubkey().as_ref()], &gv_id_e2e()).0;
    let vote = |gv_proposal: &Pubkey, action: u8| Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(gv_ballot, false), AccountMeta::new(*gv_proposal, false),
        AccountMeta::new(position, false), AccountMeta::new_readonly(env.pool, false), AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(sub_id(), false)], data: vec![3u8, action] };
    let send = |svm: &mut LiteSVM, ix: Instruction| { svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer, &alice], bh)) };

    // Back A.
    send(&mut svm, vote(&gv_a, 1)).expect("back A");
    // Backing B while the ballot is live on A is rejected.
    assert!(send(&mut svm, vote(&gv_b, 1)).is_err(), "cannot back a second proposal without retracting the first");
    // Retract A, then B can be backed.
    send(&mut svm, vote(&gv_a, 2)).expect("retract A");
    send(&mut svm, vote(&gv_b, 1)).expect("after retract, back B");
}

// ATTACK PROBE (low-turnout capture): a minority-capital voter tries to seal their proposal
// by being the ONLY one to vote — they then hold 100% of the CAST weight (majority trivially
// passes), but quorum is measured against the LIVE pool outstanding (including non-voters), so
// total_voted_principal*2 must exceed ALL deposited principal. A minority cannot reach it.
// Proven with REAL multi-party deposits: alice (400k of 1M outstanding) votes and triggers ->
// rejected (no quorum); only once bob (600k) also votes does the trigger succeed.
#[test]
fn e2e_minority_turnout_cannot_reach_quorum() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(gv_id_e2e(), so_deploy("genesis_vote_program")).unwrap();
    svm.add_program_from_file(dist_id_e2e(), so_deploy("distribution_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_genesis(&mut svm, &payer);
    let recipient = Pubkey::new_unique();
    let (dist_proposal, gv_proposal) = register_proposal(&mut svm, &payer, &env, 1, &recipient, 100);

    // Two depositors: alice 400k (minority), bob 600k (majority, abstains at first).
    let deposit = |svm: &mut LiteSVM, who: &Keypair, amt: u64| -> Pubkey {
        svm.airdrop(&who.pubkey(), 1_000_000_000).unwrap();
        let ata = Pubkey::new_unique(); set_token(svm, &ata, &env.collateral_mint, &who.pubkey(), amt);
        let holding = Pubkey::new_unique(); set_token(svm, &holding, &env.collateral_mint, &env.pool, 0);
        let position = sub_position_pda(&env.pool, &who.pubkey());
        let mut d = vec![4u8]; d.extend_from_slice(&amt.to_le_bytes());
        let ix = Instruction { program_id: sub_id(), accounts: vec![
            AccountMeta::new(who.pubkey(), true), AccountMeta::new(env.pool, false), AccountMeta::new(position, false), AccountMeta::new(ata, false),
            AccountMeta::new(holding, false), AccountMeta::new(env.slab, false), AccountMeta::new(env.perc_vault, false),
            AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(system_program::ID, false)], data: d };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer, who], bh)).expect("deposit");
        position
    };
    let alice = Keypair::new(); let alice_pos = deposit(&mut svm, &alice, 400_000);
    let bob = Keypair::new(); let bob_pos = deposit(&mut svm, &bob, 600_000);
    let mut c = svm.get_sysvar::<Clock>(); c.slot += 8; svm.set_sysvar::<Clock>(&c);

    let vote = |svm: &mut LiteSVM, who: &Keypair, pos: &Pubkey| {
        let ballot = Pubkey::find_program_address(&[b"gv_ballot", env.gv_config.as_ref(), who.pubkey().as_ref()], &gv_id_e2e()).0;
        let ix = Instruction { program_id: gv_id_e2e(), accounts: vec![
            AccountMeta::new(who.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(ballot, false), AccountMeta::new(gv_proposal, false),
            AccountMeta::new(*pos, false), AccountMeta::new_readonly(env.pool, false), AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(sub_id(), false)], data: vec![3u8, 1u8] };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer, who], bh)).expect("vote");
    };
    let trigger = |svm: &mut LiteSVM| -> Result<(), String> {
        let ix = Instruction { program_id: gv_id_e2e(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(gv_proposal, false),
            AccountMeta::new_readonly(dist_id_e2e(), false), AccountMeta::new(env.dist_config, false), AccountMeta::new(dist_proposal, false),
            AccountMeta::new_readonly(env.pool, false)], data: vec![4u8] };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], bh)).map(|_| ()).map_err(|e| format!("{:?}", e))
    };

    // Only the minority voted: 400k*2 = 800k <= 1,000,000 outstanding -> NO quorum.
    vote(&mut svm, &alice, &alice_pos);
    assert!(trigger(&mut svm).is_err(), "a minority of live capital cannot seal by being the only voter (quorum guards turnout)");
    // The dist config is not sealed.
    let dist_cfg = svm.get_account(&env.dist_config).unwrap();
    assert_eq!(Pubkey::new_from_array(dist_cfg.data[120..152].try_into().unwrap()), Pubkey::default(), "not sealed");

    // Once the majority also votes, quorum is met and the trigger succeeds.
    vote(&mut svm, &bob, &bob_pos);
    trigger(&mut svm).expect("with a real quorum the trigger seals the winner");
    let dist_cfg = svm.get_account(&env.dist_config).unwrap();
    assert_eq!(Pubkey::new_from_array(dist_cfg.data[120..152].try_into().unwrap()), dist_proposal, "sealed once quorum reached");
}

// ATTACK PROBE (position substitution / vote-power theft): voting power is the voter's OWN
// capital. The gv `vote` derives the subledger position PDA from the SIGNER (voter) and pins
// the passed account to it — so a voter cannot pass someone ELSE's (larger) position to vote
// with their weight. Proven end-to-end: alice (small) tries to vote with bob's (large)
// position account and is rejected; voting with her own position works.
#[test]
fn e2e_voter_cannot_vote_with_another_voters_position() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(gv_id_e2e(), so_deploy("genesis_vote_program")).unwrap();
    svm.add_program_from_file(dist_id_e2e(), so_deploy("distribution_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_genesis(&mut svm, &payer);
    let recipient = Pubkey::new_unique();
    let (_dp, gv_proposal) = register_proposal(&mut svm, &payer, &env, 1, &recipient, 100);

    let deposit = |svm: &mut LiteSVM, who: &Keypair, amt: u64| -> Pubkey {
        svm.airdrop(&who.pubkey(), 1_000_000_000).unwrap();
        let ata = Pubkey::new_unique(); set_token(svm, &ata, &env.collateral_mint, &who.pubkey(), amt);
        let holding = Pubkey::new_unique(); set_token(svm, &holding, &env.collateral_mint, &env.pool, 0);
        let position = sub_position_pda(&env.pool, &who.pubkey());
        let mut d = vec![4u8]; d.extend_from_slice(&amt.to_le_bytes());
        let ix = Instruction { program_id: sub_id(), accounts: vec![
            AccountMeta::new(who.pubkey(), true), AccountMeta::new(env.pool, false), AccountMeta::new(position, false), AccountMeta::new(ata, false),
            AccountMeta::new(holding, false), AccountMeta::new(env.slab, false), AccountMeta::new(env.perc_vault, false),
            AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(system_program::ID, false)], data: d };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer, who], bh)).expect("deposit");
        position
    };
    let alice = Keypair::new(); let alice_pos = deposit(&mut svm, &alice, 100_000);
    let bob = Keypair::new(); let bob_pos = deposit(&mut svm, &bob, 900_000);
    let mut c = svm.get_sysvar::<Clock>(); c.slot += 8; svm.set_sysvar::<Clock>(&c);

    // vote ix with an EXPLICIT position account (so we can try substituting bob's).
    let vote = |who: &Keypair, position: &Pubkey| Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(who.pubkey(), true),
        AccountMeta::new(env.gv_config, false),
        AccountMeta::new(Pubkey::find_program_address(&[b"gv_ballot", env.gv_config.as_ref(), who.pubkey().as_ref()], &gv_id_e2e()).0, false),
        AccountMeta::new(gv_proposal, false),
        AccountMeta::new(*position, false),
        AccountMeta::new_readonly(env.pool, false),
        AccountMeta::new_readonly(system_program::ID, false),
        AccountMeta::new_readonly(sub_id(), false),
    ], data: vec![3u8, 1u8] };

    // alice signs but passes BOB's position -> the derived PDA (from alice) mismatches -> rejected.
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    assert!(svm.send_transaction(Transaction::new_signed_with_payer(&[vote(&alice, &bob_pos)], Some(&payer.pubkey()), &[&payer, &alice], bh)).is_err(),
        "a voter must not be able to vote with another voter's position");

    // alice voting with HER own position works.
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[vote(&alice, &alice_pos)], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("vote with own position works");
}

// ATTACK PROBE (winner-take-all at the claim layer): once proposal A is sealed as the winner,
// a LOSING proposal's recipient must get nothing. The distribution claim pins
// config.sealed_proposal (only the winner pays) AND entry.pubkey == signer (pull model). So a
// loser can claim neither from their own (never-sealed) proposal nor from the winner (not their
// entry). Proven end-to-end with two real proposals.
#[test]
fn e2e_only_the_winning_proposal_can_be_claimed() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(gv_id_e2e(), so_deploy("genesis_vote_program")).unwrap();
    svm.add_program_from_file(dist_id_e2e(), so_deploy("distribution_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_genesis(&mut svm, &payer);

    let winner = Keypair::new();   // named in proposal A (the winner)
    let loser = Keypair::new();    // named in proposal B (the loser)
    let (prop_a, gv_a) = register_proposal(&mut svm, &payer, &env, 1, &winner.pubkey(), 100);
    let (prop_b, _gv_b) = register_proposal(&mut svm, &payer, &env, 2, &loser.pubkey(), 100);

    // alice deposits 100% of capital and backs proposal A to quorum + majority.
    let alice = Keypair::new(); svm.airdrop(&alice.pubkey(), 1_000_000_000).unwrap();
    let amount = 1_000_000u64;
    let alice_ata = Pubkey::new_unique(); set_token(&mut svm, &alice_ata, &env.collateral_mint, &alice.pubkey(), amount);
    let holding = Pubkey::new_unique(); set_token(&mut svm, &holding, &env.collateral_mint, &env.pool, 0);
    let position = sub_position_pda(&env.pool, &alice.pubkey());
    let mut dep = vec![4u8]; dep.extend_from_slice(&amount.to_le_bytes());
    let deposit = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new(env.pool, false), AccountMeta::new(position, false), AccountMeta::new(alice_ata, false),
        AccountMeta::new(holding, false), AccountMeta::new(env.slab, false), AccountMeta::new(env.perc_vault, false),
        AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(system_program::ID, false)], data: dep };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[deposit], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("deposit");
    let mut c = svm.get_sysvar::<Clock>(); c.slot += 8; svm.set_sysvar::<Clock>(&c);
    let ballot = Pubkey::find_program_address(&[b"gv_ballot", env.gv_config.as_ref(), alice.pubkey().as_ref()], &gv_id_e2e()).0;
    let vote = Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(ballot, false), AccountMeta::new(gv_a, false),
        AccountMeta::new(position, false), AccountMeta::new_readonly(env.pool, false), AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(sub_id(), false)], data: vec![3u8, 1u8] };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[vote], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("vote A");
    let trigger = Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(gv_a, false),
        AccountMeta::new_readonly(dist_id_e2e(), false), AccountMeta::new(env.dist_config, false), AccountMeta::new(prop_a, false),
        AccountMeta::new_readonly(env.pool, false)], data: vec![4u8] };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[trigger], Some(&payer.pubkey()), &[&payer], bh)).expect("seal A");

    let claim = |who: &Keypair, ata: &Pubkey, proposal: &Pubkey| Instruction { program_id: dist_id_e2e(), accounts: vec![
        AccountMeta::new_readonly(who.pubkey(), true), AccountMeta::new_readonly(env.dist_config, false), AccountMeta::new(*proposal, false),
        AccountMeta::new(env.dist_vault, false), AccountMeta::new(*ata, false), AccountMeta::new_readonly(spl_token::ID, false),
    ], data: { let mut d = vec![4u8]; d.extend_from_slice(&0u32.to_le_bytes()); d } };
    let winner_ata = Pubkey::new_unique(); set_token(&mut svm, &winner_ata, &env.coin_mint, &winner.pubkey(), 0);
    let loser_ata = Pubkey::new_unique(); set_token(&mut svm, &loser_ata, &env.coin_mint, &loser.pubkey(), 0);

    // The winner claims the full COIN supply from proposal A.
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[claim(&winner, &winner_ata, &prop_a)], Some(&payer.pubkey()), &[&payer, &winner], bh)).expect("winner claims A");
    assert_eq!(token_amount(&svm, &winner_ata), 100, "winner got the full supply");

    // The loser cannot claim from their own (never-sealed) proposal B...
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    assert!(svm.send_transaction(Transaction::new_signed_with_payer(&[claim(&loser, &loser_ata, &prop_b)], Some(&payer.pubkey()), &[&payer, &loser], bh)).is_err(),
        "a losing proposal's recipient cannot claim from the unsealed losing proposal");
    // ...nor from the winning proposal A (their pubkey is not an entry there).
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    assert!(svm.send_transaction(Transaction::new_signed_with_payer(&[claim(&loser, &loser_ata, &prop_a)], Some(&payer.pubkey()), &[&payer, &loser], bh)).is_err(),
        "the loser is not an entry in the winning proposal and cannot claim from it");
    assert_eq!(token_amount(&svm, &loser_ata), 0, "the loser receives nothing");
}

// ATTACK PROBE (time-weighting balance / early-squatter capture): weight = floor(log2(age)) *
// principal. log-time is a SOFT (capped, sub-linear) multiplier while capital is LINEAR — so
// capital must dominate. Otherwise an early tiny depositor could sit and accumulate enough
// time-weight to out-vote a later, much larger depositor, capturing governance cheaply. This
// pins that a later 10x-capital voter out-weighs an early small voter despite less hold time:
// the two back COMPETING proposals and the big-but-late voter's proposal wins.
#[test]
fn e2e_capital_outweighs_hold_time_no_early_squatter_capture() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(gv_id_e2e(), so_deploy("genesis_vote_program")).unwrap();
    svm.add_program_from_file(dist_id_e2e(), so_deploy("distribution_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_genesis(&mut svm, &payer);
    let early_dest = Pubkey::new_unique();
    let late_dest = Pubkey::new_unique();
    let (prop_early, gv_early) = register_proposal(&mut svm, &payer, &env, 1, &early_dest, 100);
    let (prop_late, gv_late) = register_proposal(&mut svm, &payer, &env, 2, &late_dest, 100);

    let deposit = |svm: &mut LiteSVM, who: &Keypair, amt: u64| -> Pubkey {
        svm.airdrop(&who.pubkey(), 1_000_000_000).unwrap();
        let ata = Pubkey::new_unique(); set_token(svm, &ata, &env.collateral_mint, &who.pubkey(), amt);
        let holding = Pubkey::new_unique(); set_token(svm, &holding, &env.collateral_mint, &env.pool, 0);
        let position = sub_position_pda(&env.pool, &who.pubkey());
        let mut d = vec![4u8]; d.extend_from_slice(&amt.to_le_bytes());
        let ix = Instruction { program_id: sub_id(), accounts: vec![
            AccountMeta::new(who.pubkey(), true), AccountMeta::new(env.pool, false), AccountMeta::new(position, false), AccountMeta::new(ata, false),
            AccountMeta::new(holding, false), AccountMeta::new(env.slab, false), AccountMeta::new(env.perc_vault, false),
            AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(system_program::ID, false)], data: d };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer, who], bh)).expect("deposit");
        position
    };
    let vote = |svm: &mut LiteSVM, who: &Keypair, pos: &Pubkey, gv_prop: &Pubkey| {
        let ballot = Pubkey::find_program_address(&[b"gv_ballot", env.gv_config.as_ref(), who.pubkey().as_ref()], &gv_id_e2e()).0;
        let ix = Instruction { program_id: gv_id_e2e(), accounts: vec![
            AccountMeta::new(who.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(ballot, false), AccountMeta::new(*gv_prop, false),
            AccountMeta::new(*pos, false), AccountMeta::new_readonly(env.pool, false), AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(sub_id(), false)], data: vec![3u8, 1u8] };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer, who], bh)).expect("vote");
    };

    // Early small voter: 100k deposited at slot 1000, holds a LONG time (large age — but we
    // stay inside the percolator oracle-staleness window so the late deposit still lands).
    let early = Keypair::new(); let early_pos = deposit(&mut svm, &early, 100_000);
    let mut c = svm.get_sysvar::<Clock>(); c.slot += 1500; svm.set_sysvar::<Clock>(&c); // floor(log2(1500)) = 10
    vote(&mut svm, &early, &early_pos, &gv_early);

    // Later large voter: 1,000,000 (10x) deposited now, holds only a short time.
    let late = Keypair::new(); let late_pos = deposit(&mut svm, &late, 1_000_000);
    let mut c = svm.get_sysvar::<Clock>(); c.slot += 16; svm.set_sysvar::<Clock>(&c); // floor(log2(16)) = 4
    vote(&mut svm, &late, &late_pos, &gv_late);
    // early weight ~= 10 * 100k = 1,000,000 ; late weight ~= 4 * 1,000,000 = 4,000,000 (capital wins).

    let trigger = |svm: &mut LiteSVM, gv_prop: &Pubkey, dist_prop: &Pubkey| -> Result<(), String> {
        let ix = Instruction { program_id: gv_id_e2e(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(*gv_prop, false),
            AccountMeta::new_readonly(dist_id_e2e(), false), AccountMeta::new(env.dist_config, false), AccountMeta::new(*dist_prop, false),
            AccountMeta::new_readonly(env.pool, false)], data: vec![4u8] };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], bh)).map(|_| ()).map_err(|e| format!("{:?}", e))
    };
    // The early-squatter proposal lacks a weighted majority -> cannot seal.
    assert!(trigger(&mut svm, &gv_early, &prop_early).is_err(), "an early small-capital voter must NOT out-weigh a later large-capital voter");
    // The later large-capital proposal IS the weighted-majority winner -> seals.
    trigger(&mut svm, &gv_late, &prop_late).expect("the larger-capital proposal wins despite less hold time");
    let dist_cfg = svm.get_account(&env.dist_config).unwrap();
    assert_eq!(Pubkey::new_from_array(dist_cfg.data[120..152].try_into().unwrap()), prop_late, "capital dominates the soft log-time weight");
}

// ATTACK PROBE (weight inflation via retract/re-back cycling): a voter repeatedly backs and
// retracts the same proposal, trying to make their support_weight accumulate beyond their
// single capital contribution. The gv `vote` must subtract EXACTLY the stored ballot weight on
// retract and re-add a single fresh contribution on back — never accumulate. Proven end-to-end:
// across multiple back/retract cycles (no slots elapse, so weight is constant) the proposal's
// support_weight and the global total_cast_weight stay at exactly ONE contribution, and retract
// returns them to zero.
#[test]
fn e2e_retract_reback_cannot_inflate_vote_weight() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(gv_id_e2e(), so_deploy("genesis_vote_program")).unwrap();
    svm.add_program_from_file(dist_id_e2e(), so_deploy("distribution_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_genesis(&mut svm, &payer);
    let recipient = Pubkey::new_unique();
    let (_dp, gv_proposal) = register_proposal(&mut svm, &payer, &env, 1, &recipient, 100);

    let alice = Keypair::new(); svm.airdrop(&alice.pubkey(), 1_000_000_000).unwrap();
    let amount = 1_000_000u64;
    let alice_ata = Pubkey::new_unique(); set_token(&mut svm, &alice_ata, &env.collateral_mint, &alice.pubkey(), amount);
    let holding = Pubkey::new_unique(); set_token(&mut svm, &holding, &env.collateral_mint, &env.pool, 0);
    let position = sub_position_pda(&env.pool, &alice.pubkey());
    let mut dep = vec![4u8]; dep.extend_from_slice(&amount.to_le_bytes());
    let deposit = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new(env.pool, false), AccountMeta::new(position, false), AccountMeta::new(alice_ata, false),
        AccountMeta::new(holding, false), AccountMeta::new(env.slab, false), AccountMeta::new(env.perc_vault, false),
        AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(system_program::ID, false)], data: dep };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[deposit], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("deposit");
    let mut c = svm.get_sysvar::<Clock>(); c.slot += 16; svm.set_sysvar::<Clock>(&c); // fix the age so weight is constant

    let ballot = Pubkey::find_program_address(&[b"gv_ballot", env.gv_config.as_ref(), alice.pubkey().as_ref()], &gv_id_e2e()).0;
    let vote = |svm: &mut LiteSVM, action: u8| {
        let ix = Instruction { program_id: gv_id_e2e(), accounts: vec![
            AccountMeta::new(alice.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(ballot, false), AccountMeta::new(gv_proposal, false),
            AccountMeta::new(position, false), AccountMeta::new_readonly(env.pool, false), AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(sub_id(), false)], data: vec![3u8, action] };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("vote");
    };
    let support = |svm: &LiteSVM| u64::from_le_bytes(svm.get_account(&gv_proposal).unwrap().data[72..80].try_into().unwrap());
    let cast = |svm: &LiteSVM| u64::from_le_bytes(svm.get_account(&env.gv_config).unwrap().data[208..216].try_into().unwrap());

    // First back establishes the single contribution W.
    vote(&mut svm, 1);
    let w = support(&svm);
    assert!(w > 0, "backing records a positive weight");
    assert_eq!(cast(&svm), w, "global cast weight == this voter's single contribution");

    // Cycle back/retract several times: it must NEVER accumulate.
    for _ in 0..5 {
        vote(&mut svm, 2); // retract
        assert_eq!(support(&svm), 0, "retract zeroes the proposal support");
        assert_eq!(cast(&svm), 0, "retract zeroes the global cast weight");
        vote(&mut svm, 1); // re-back
        assert_eq!(support(&svm), w, "re-back is ONE contribution, never accumulated");
        assert_eq!(cast(&svm), w, "global cast weight stays a single contribution");
    }
}

// ATTACK PROBE (replay of a Squads execute): the handoff is a sequence of timelock'd vault
// transactions. Once a vault transaction has executed (e.g., the operator rotation), it must
// NOT be replayable — otherwise a completed timelock'd action could be re-triggered without a
// fresh proposal/approval/timelock. Squads marks the proposal Executed; a second execute of
// the same transaction is rejected. Pinned end-to-end on a fully handed-off market.
#[test]
fn e2e_completed_squads_execute_cannot_be_replayed() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(twap_id(), so_deploy("twap_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_handoff(&mut svm, &payer); // idx 1=topup, 2=policy, 3=operator, 4=floor

    // Reconstruct the operator-handoff vault transaction (idx 3) and try to execute it AGAIN.
    let idx = 3u64;
    let transaction = transaction_pda(&env.squads, &env.multisig, idx);
    let proposal = proposal_pda(&env.squads, &env.multisig, idx);
    let remaining = vec![
        AccountMeta::new_readonly(env.squads_vault, false), AccountMeta::new(env.slab, false), AccountMeta::new_readonly(env.twap_cfg, false),
        AccountMeta::new_readonly(env.twap_authority, false), AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(twap_id(), false),
    ];
    let exec = vault_transaction_execute_ix(&env.squads, &env.multisig, &proposal, &transaction, &env.dao.pubkey(), &remaining);
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    assert!(svm.send_transaction(Transaction::new_signed_with_payer(&[exec], Some(&payer.pubkey()), &[&payer, &env.dao], bh)).is_err(),
        "an already-executed Squads vault transaction must not be replayable");
}

// ATTACK PROBE (voting with NO capital at all): a voter must have a real subledger position
// (capital at risk) to vote. The fresh-position probe covers a deposited-but-too-recent
// position (weight 0); this covers the extreme — an account that NEVER deposited has no position
// account, so the gv `vote` cannot read/own-check it and rejects. So governance power requires
// actually putting capital at risk; you cannot vote for free.
#[test]
fn e2e_cannot_vote_without_a_position() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(gv_id_e2e(), so_deploy("genesis_vote_program")).unwrap();
    svm.add_program_from_file(dist_id_e2e(), so_deploy("distribution_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_genesis(&mut svm, &payer);
    let recipient = Pubkey::new_unique();
    let (_dp, gv_proposal) = register_proposal(&mut svm, &payer, &env, 1, &recipient, 100);

    // An attacker who has deposited NOTHING — their position PDA is an uninitialized account.
    let attacker = Keypair::new(); svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    let position = sub_position_pda(&env.pool, &attacker.pubkey());
    assert!(svm.get_account(&position).map_or(true, |a| a.data.is_empty()), "attacker has no position");
    let ballot = Pubkey::find_program_address(&[b"gv_ballot", env.gv_config.as_ref(), attacker.pubkey().as_ref()], &gv_id_e2e()).0;
    let vote = Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(attacker.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(ballot, false), AccountMeta::new(gv_proposal, false),
        AccountMeta::new(position, false), AccountMeta::new_readonly(env.pool, false), AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(sub_id(), false)], data: vec![3u8, 1u8] };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    assert!(svm.send_transaction(Transaction::new_signed_with_payer(&[vote], Some(&payer.pubkey()), &[&payer, &attacker], bh)).is_err(),
        "an account with no subledger position (no capital at risk) cannot vote");
}

// ATTACK PROBE (Sybil split — the core resistance property): vote weight = floor(log2(age)) *
// principal is LINEAR in principal, so splitting capital across many positions/identities must
// give NO weight advantage over depositing it all at once. Here an attacker splits 1,000,000
// into 4 identities of 250,000, all deposited at the same slot and voting the same proposal at
// the same age: the total support_weight equals exactly what a SINGLE 1,000,000 position would
// produce (floor(log2(age)) * 1,000,000). So you cannot multiply governance power by Sybiling.
#[test]
fn e2e_sybil_splitting_gives_no_vote_advantage() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(gv_id_e2e(), so_deploy("genesis_vote_program")).unwrap();
    svm.add_program_from_file(dist_id_e2e(), so_deploy("distribution_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_genesis(&mut svm, &payer);
    let recipient = Pubkey::new_unique();
    let (_dp, gv_proposal) = register_proposal(&mut svm, &payer, &env, 1, &recipient, 100);

    let split = 250_000u64;
    let n = 4u64; // total = 1,000,000
    let mut voters = Vec::new();
    for _ in 0..n {
        let who = Keypair::new(); svm.airdrop(&who.pubkey(), 1_000_000_000).unwrap();
        let ata = Pubkey::new_unique(); set_token(&mut svm, &ata, &env.collateral_mint, &who.pubkey(), split);
        let holding = Pubkey::new_unique(); set_token(&mut svm, &holding, &env.collateral_mint, &env.pool, 0);
        let position = sub_position_pda(&env.pool, &who.pubkey());
        let mut d = vec![4u8]; d.extend_from_slice(&split.to_le_bytes());
        let ix = Instruction { program_id: sub_id(), accounts: vec![
            AccountMeta::new(who.pubkey(), true), AccountMeta::new(env.pool, false), AccountMeta::new(position, false), AccountMeta::new(ata, false),
            AccountMeta::new(holding, false), AccountMeta::new(env.slab, false), AccountMeta::new(env.perc_vault, false),
            AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(system_program::ID, false)], data: d };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer, &who], bh)).expect("deposit");
        voters.push((who, position));
    }
    // All positions share the same start_slot; warp once so every vote is at age 16 (log2 = 4).
    let mut c = svm.get_sysvar::<Clock>(); c.slot += 16; svm.set_sysvar::<Clock>(&c);
    for (who, position) in &voters {
        let ballot = Pubkey::find_program_address(&[b"gv_ballot", env.gv_config.as_ref(), who.pubkey().as_ref()], &gv_id_e2e()).0;
        let ix = Instruction { program_id: gv_id_e2e(), accounts: vec![
            AccountMeta::new(who.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(ballot, false), AccountMeta::new(gv_proposal, false),
            AccountMeta::new(*position, false), AccountMeta::new_readonly(env.pool, false), AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(sub_id(), false)], data: vec![3u8, 1u8] };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer, who], bh)).expect("vote");
    }

    let support = u64::from_le_bytes(svm.get_account(&gv_proposal).unwrap().data[72..80].try_into().unwrap());
    // A single 1,000,000 position at age 16 would weigh floor(log2(16)) * 1,000,000 = 4,000,000.
    let single_position_weight = 4u64 * (split * n);
    assert_eq!(support, single_position_weight, "splitting capital across {} identities gives no weight advantage", n);
    // Quorum denominator (voted principal) is likewise just the summed capital, not multiplied.
    let voted = u64::from_le_bytes(svm.get_account(&env.gv_config).unwrap().data[200..208].try_into().unwrap());
    assert_eq!(voted, split * n, "voted principal is the summed capital — Sybiling does not inflate quorum either");
}

// ATTACK PROBE (quorum strict-inequality boundary): quorum is total_voted_principal*2 >
// outstanding (STRICT). So a voter holding EXACTLY half the live capital cannot seal — a 50/50
// situation needs strictly MORE than half to have voted. If this were >= a tie could capture
// the distribution. Pinned end-to-end: two equal 500k depositors; one voting (exactly 50%) fails
// to reach quorum, and only once the second also votes (now > 50%) does the trigger seal.
#[test]
fn e2e_exactly_half_capital_does_not_meet_quorum() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(gv_id_e2e(), so_deploy("genesis_vote_program")).unwrap();
    svm.add_program_from_file(dist_id_e2e(), so_deploy("distribution_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_genesis(&mut svm, &payer);
    let recipient = Pubkey::new_unique();
    let (dist_proposal, gv_proposal) = register_proposal(&mut svm, &payer, &env, 1, &recipient, 100);

    let deposit = |svm: &mut LiteSVM, who: &Keypair, amt: u64| -> Pubkey {
        svm.airdrop(&who.pubkey(), 1_000_000_000).unwrap();
        let ata = Pubkey::new_unique(); set_token(svm, &ata, &env.collateral_mint, &who.pubkey(), amt);
        let holding = Pubkey::new_unique(); set_token(svm, &holding, &env.collateral_mint, &env.pool, 0);
        let position = sub_position_pda(&env.pool, &who.pubkey());
        let mut d = vec![4u8]; d.extend_from_slice(&amt.to_le_bytes());
        let ix = Instruction { program_id: sub_id(), accounts: vec![
            AccountMeta::new(who.pubkey(), true), AccountMeta::new(env.pool, false), AccountMeta::new(position, false), AccountMeta::new(ata, false),
            AccountMeta::new(holding, false), AccountMeta::new(env.slab, false), AccountMeta::new(env.perc_vault, false),
            AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(system_program::ID, false)], data: d };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer, who], bh)).expect("deposit");
        position
    };
    let a = Keypair::new(); let a_pos = deposit(&mut svm, &a, 500_000);
    let b = Keypair::new(); let b_pos = deposit(&mut svm, &b, 500_000); // outstanding = 1,000,000
    let mut c = svm.get_sysvar::<Clock>(); c.slot += 8; svm.set_sysvar::<Clock>(&c);

    let vote = |svm: &mut LiteSVM, who: &Keypair, pos: &Pubkey| {
        let ballot = Pubkey::find_program_address(&[b"gv_ballot", env.gv_config.as_ref(), who.pubkey().as_ref()], &gv_id_e2e()).0;
        let ix = Instruction { program_id: gv_id_e2e(), accounts: vec![
            AccountMeta::new(who.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(ballot, false), AccountMeta::new(gv_proposal, false),
            AccountMeta::new(*pos, false), AccountMeta::new_readonly(env.pool, false), AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(sub_id(), false)], data: vec![3u8, 1u8] };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer, who], bh)).expect("vote");
    };
    let trigger = |svm: &mut LiteSVM| -> Result<(), String> {
        let ix = Instruction { program_id: gv_id_e2e(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(gv_proposal, false),
            AccountMeta::new_readonly(dist_id_e2e(), false), AccountMeta::new(env.dist_config, false), AccountMeta::new(dist_proposal, false),
            AccountMeta::new_readonly(env.pool, false)], data: vec![4u8] };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], bh)).map(|_| ()).map_err(|e| format!("{:?}", e))
    };

    // EXACTLY half (500k of 1,000,000): 500k*2 == 1,000,000, NOT > 1,000,000 -> no quorum.
    vote(&mut svm, &a, &a_pos);
    assert!(trigger(&mut svm).is_err(), "exactly 50% of capital must NOT meet quorum (strict >)");
    // Strictly more than half (both, 1,000,000): 1,000,000*2 > 1,000,000 -> quorum.
    vote(&mut svm, &b, &b_pos);
    trigger(&mut svm).expect("strictly more than half meets quorum");
}

// ATTACK PROBE (majority strict-inequality / tie deadlock): the winner needs support_weight*2 >
// total_cast_weight (STRICT). So two proposals each holding EXACTLY half the cast weight tie —
// NEITHER can seal. If this were >= both could seal at 50% (double-seal / ambiguous winner). The
// tie simply deadlocks until more weight breaks it. Pinned end-to-end: two equal-weight voters
// back competing proposals (neither triggers), then a third voter tips one over half (it seals).
#[test]
fn e2e_tied_weight_between_proposals_deadlocks_until_broken() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(gv_id_e2e(), so_deploy("genesis_vote_program")).unwrap();
    svm.add_program_from_file(dist_id_e2e(), so_deploy("distribution_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_genesis(&mut svm, &payer);
    let (prop_a, gv_a) = register_proposal(&mut svm, &payer, &env, 1, &Pubkey::new_unique(), 100);
    let (prop_b, gv_b) = register_proposal(&mut svm, &payer, &env, 2, &Pubkey::new_unique(), 100);

    let deposit = |svm: &mut LiteSVM, who: &Keypair, amt: u64| -> Pubkey {
        svm.airdrop(&who.pubkey(), 1_000_000_000).unwrap();
        let ata = Pubkey::new_unique(); set_token(svm, &ata, &env.collateral_mint, &who.pubkey(), amt);
        let holding = Pubkey::new_unique(); set_token(svm, &holding, &env.collateral_mint, &env.pool, 0);
        let position = sub_position_pda(&env.pool, &who.pubkey());
        let mut d = vec![4u8]; d.extend_from_slice(&amt.to_le_bytes());
        let ix = Instruction { program_id: sub_id(), accounts: vec![
            AccountMeta::new(who.pubkey(), true), AccountMeta::new(env.pool, false), AccountMeta::new(position, false), AccountMeta::new(ata, false),
            AccountMeta::new(holding, false), AccountMeta::new(env.slab, false), AccountMeta::new(env.perc_vault, false),
            AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(system_program::ID, false)], data: d };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer, who], bh)).expect("deposit");
        position
    };
    let vote = |svm: &mut LiteSVM, who: &Keypair, pos: &Pubkey, gv_prop: &Pubkey| {
        let ballot = Pubkey::find_program_address(&[b"gv_ballot", env.gv_config.as_ref(), who.pubkey().as_ref()], &gv_id_e2e()).0;
        let ix = Instruction { program_id: gv_id_e2e(), accounts: vec![
            AccountMeta::new(who.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(ballot, false), AccountMeta::new(*gv_prop, false),
            AccountMeta::new(*pos, false), AccountMeta::new_readonly(env.pool, false), AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(sub_id(), false)], data: vec![3u8, 1u8] };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer, who], bh)).expect("vote");
    };
    let trigger = |svm: &mut LiteSVM, gv_prop: &Pubkey, dist_prop: &Pubkey| -> Result<(), String> {
        let ix = Instruction { program_id: gv_id_e2e(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(*gv_prop, false),
            AccountMeta::new_readonly(dist_id_e2e(), false), AccountMeta::new(env.dist_config, false), AccountMeta::new(*dist_prop, false),
            AccountMeta::new_readonly(env.pool, false)], data: vec![4u8] };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], bh)).map(|_| ()).map_err(|e| format!("{:?}", e))
    };

    // Two equal voters back competing proposals at the same age -> exact weight tie.
    let a = Keypair::new(); let a_pos = deposit(&mut svm, &a, 500_000);
    let b = Keypair::new(); let b_pos = deposit(&mut svm, &b, 500_000);
    let mut c = svm.get_sysvar::<Clock>(); c.slot += 16; svm.set_sysvar::<Clock>(&c);
    vote(&mut svm, &a, &a_pos, &gv_a);
    vote(&mut svm, &b, &b_pos, &gv_b);
    assert!(trigger(&mut svm, &gv_a, &prop_a).is_err(), "a 50/50 weight tie cannot seal proposal A");
    assert!(trigger(&mut svm, &gv_b, &prop_b).is_err(), "a 50/50 weight tie cannot seal proposal B either");

    // A third voter tips A over half -> A now has a strict weighted majority and seals.
    let carol = Keypair::new(); let carol_pos = deposit(&mut svm, &carol, 100_000);
    let mut c = svm.get_sysvar::<Clock>(); c.slot += 16; svm.set_sysvar::<Clock>(&c);
    vote(&mut svm, &carol, &carol_pos, &gv_a);
    trigger(&mut svm, &gv_a, &prop_a).expect("the tie-broken majority seals A");
    let dist_cfg = svm.get_account(&env.dist_config).unwrap();
    assert_eq!(Pubkey::new_from_array(dist_cfg.data[120..152].try_into().unwrap()), prop_a, "A is the sealed winner once the tie breaks");
}

// ATTACK/DESIGN PROBE (exit recomputes quorum — "those who stay decide"): quorum is
// total_voted_principal*2 > LIVE pool outstanding. A non-voter's capital counts AGAINST quorum
// only while it stays in the pool; when they EXIT, outstanding shrinks and a voter who was below
// quorum can become the majority of who remains. This is the design's anti-stall property: a
// large passive holder cannot indefinitely block a finalize just by abstaining — they either
// vote or exit, and exiting hands the decision to those who stay. Pinned end-to-end with a real
// withdrawal that flips the trigger from rejected to sealed.
#[test]
fn e2e_non_voter_exit_recomputes_quorum_stayers_decide() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(gv_id_e2e(), so_deploy("genesis_vote_program")).unwrap();
    svm.add_program_from_file(dist_id_e2e(), so_deploy("distribution_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_genesis(&mut svm, &payer);
    let recipient = Pubkey::new_unique();
    let (dist_proposal, gv_proposal) = register_proposal(&mut svm, &payer, &env, 1, &recipient, 100);

    // Returns (position, ata, holding) so the depositor can later exit.
    let deposit = |svm: &mut LiteSVM, who: &Keypair, amt: u64| -> (Pubkey, Pubkey, Pubkey) {
        svm.airdrop(&who.pubkey(), 1_000_000_000).unwrap();
        let ata = Pubkey::new_unique(); set_token(svm, &ata, &env.collateral_mint, &who.pubkey(), amt);
        let holding = Pubkey::new_unique(); set_token(svm, &holding, &env.collateral_mint, &env.pool, 0);
        let position = sub_position_pda(&env.pool, &who.pubkey());
        let mut d = vec![4u8]; d.extend_from_slice(&amt.to_le_bytes());
        let ix = Instruction { program_id: sub_id(), accounts: vec![
            AccountMeta::new(who.pubkey(), true), AccountMeta::new(env.pool, false), AccountMeta::new(position, false), AccountMeta::new(ata, false),
            AccountMeta::new(holding, false), AccountMeta::new(env.slab, false), AccountMeta::new(env.perc_vault, false),
            AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(system_program::ID, false)], data: d };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer, who], bh)).expect("deposit");
        (position, ata, holding)
    };
    let trigger = |svm: &mut LiteSVM| -> Result<(), String> {
        let ix = Instruction { program_id: gv_id_e2e(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(gv_proposal, false),
            AccountMeta::new_readonly(dist_id_e2e(), false), AccountMeta::new(env.dist_config, false), AccountMeta::new(dist_proposal, false),
            AccountMeta::new_readonly(env.pool, false)], data: vec![4u8] };
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], bh)).map(|_| ()).map_err(|e| format!("{:?}", e))
    };

    let alice = Keypair::new(); let (a_pos, _a_ata, _a_h) = deposit(&mut svm, &alice, 400_000);
    let bob = Keypair::new(); let (b_pos, b_ata, b_h) = deposit(&mut svm, &bob, 600_000); // outstanding = 1,000,000
    let mut c = svm.get_sysvar::<Clock>(); c.slot += 8; svm.set_sysvar::<Clock>(&c);

    // alice (40%) votes; bob (60%) abstains -> no quorum.
    let ballot = Pubkey::find_program_address(&[b"gv_ballot", env.gv_config.as_ref(), alice.pubkey().as_ref()], &gv_id_e2e()).0;
    let vote = Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(ballot, false), AccountMeta::new(gv_proposal, false),
        AccountMeta::new(a_pos, false), AccountMeta::new_readonly(env.pool, false), AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(sub_id(), false)], data: vec![3u8, 1u8] };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[vote], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("alice votes");
    let _ = b_pos;
    assert!(trigger(&mut svm).is_err(), "40% of capital cannot reach quorum while the 60% holder stays");

    // bob (a non-voter, no vote-lock) EXITS his full principal -> outstanding shrinks to 400,000.
    let withdraw = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(bob.pubkey(), true), AccountMeta::new(env.pool, false), AccountMeta::new(b_pos, false), AccountMeta::new(b_ata, false),
        AccountMeta::new(b_h, false), AccountMeta::new(env.slab, false), AccountMeta::new(env.perc_vault, false), AccountMeta::new_readonly(perc_vault_authority(&env.slab, &perc_id()), false),
        AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false)],
        data: { let mut d = vec![5u8]; d.extend_from_slice(&600_000u64.to_le_bytes()); d } };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[withdraw], Some(&payer.pubkey()), &[&payer, &bob], bh)).expect("bob exits");

    // Now alice's 400k is 100% of the remaining outstanding -> quorum, and the trigger seals.
    trigger(&mut svm).expect("after the abstainer exits, the remaining voter has quorum and seals");
    let dist_cfg = svm.get_account(&env.dist_config).unwrap();
    assert_eq!(Pubkey::new_from_array(dist_cfg.data[120..152].try_into().unwrap()), dist_proposal, "those who stay decide");
}

// ATTACK PROBE (append injection): a distribution proposal is built by its CREATOR. append_entries
// is creator-gated (header.creator == signer), so an attacker cannot inject entries (e.g. a
// self-allocation) into someone ELSE's proposal — only the creator can append to it. Otherwise an
// attacker could graft a payout to themselves onto a popular proposal before it is voted/sealed.
#[test]
fn e2e_non_creator_cannot_append_to_a_proposal() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(gv_id_e2e(), so_deploy("genesis_vote_program")).unwrap();
    svm.add_program_from_file(dist_id_e2e(), so_deploy("distribution_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_genesis(&mut svm, &payer);

    // A creator creates an (empty) proposal.
    let creator = Keypair::new(); svm.airdrop(&creator.pubkey(), 1_000_000_000).unwrap();
    let id = 7u64;
    let dist_proposal = Pubkey::find_program_address(&[b"dist_proposal", env.dist_config.as_ref(), &id.to_le_bytes()], &dist_id_e2e()).0;
    let mut cd = vec![1u8]; cd.extend_from_slice(&id.to_le_bytes()); cd.extend_from_slice(&4u32.to_le_bytes());
    let create = Instruction { program_id: dist_id_e2e(), accounts: vec![
        AccountMeta::new(creator.pubkey(), true), AccountMeta::new_readonly(env.dist_config, false), AccountMeta::new(dist_proposal, false), AccountMeta::new_readonly(system_program::ID, false)], data: cd };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create], Some(&payer.pubkey()), &[&payer, &creator], bh)).expect("creator creates the proposal");

    let append = |signer: &Keypair, dest: &Pubkey, amt: u64| Instruction { program_id: dist_id_e2e(), accounts: vec![
        AccountMeta::new(signer.pubkey(), true), AccountMeta::new_readonly(env.dist_config, false), AccountMeta::new(dist_proposal, false)],
        data: { let mut a = vec![2u8]; a.extend_from_slice(&1u32.to_le_bytes()); a.extend_from_slice(dest.as_ref()); a.extend_from_slice(&amt.to_le_bytes()); a } };

    // ATTACK: an attacker tries to append a self-allocation to the creator's proposal -> rejected.
    let attacker = Keypair::new(); svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    assert!(svm.send_transaction(Transaction::new_signed_with_payer(&[append(&attacker, &attacker.pubkey(), 100)], Some(&payer.pubkey()), &[&payer, &attacker], bh)).is_err(),
        "a non-creator must not be able to inject entries into another's proposal");
    // The creator can append to their own proposal.
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[append(&creator, &Pubkey::new_unique(), 100)], Some(&payer.pubkey()), &[&payer, &creator], bh)).expect("the creator can append to their own proposal");
}

// ATTACK PROBE (post-finalization proposal injection): the genesis is winner-take-all and
// one-shot. Once the winning distribution is sealed, no NEW proposal may be created — distribution
// create_proposal rejects when config.is_sealed(). Otherwise an attacker could keep spawning
// proposals after the outcome is decided (clutter / confusion / attempts to re-contest a closed
// genesis). Pinned end-to-end: a winner is voted + sealed, then create_proposal for a fresh id is
// rejected, and the original sealed winner is unchanged.
#[test]
fn e2e_no_new_proposal_after_genesis_finalizes() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(gv_id_e2e(), so_deploy("genesis_vote_program")).unwrap();
    svm.add_program_from_file(dist_id_e2e(), so_deploy("distribution_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_genesis(&mut svm, &payer);
    let recipient = Pubkey::new_unique();
    let (dist_proposal, gv_proposal) = register_proposal(&mut svm, &payer, &env, 1, &recipient, 100);

    // A voter backs it to quorum + majority and seals.
    let alice = Keypair::new(); svm.airdrop(&alice.pubkey(), 1_000_000_000).unwrap();
    let amount = 1_000_000u64;
    let alice_ata = Pubkey::new_unique(); set_token(&mut svm, &alice_ata, &env.collateral_mint, &alice.pubkey(), amount);
    let holding = Pubkey::new_unique(); set_token(&mut svm, &holding, &env.collateral_mint, &env.pool, 0);
    let position = sub_position_pda(&env.pool, &alice.pubkey());
    let mut dep = vec![4u8]; dep.extend_from_slice(&amount.to_le_bytes());
    let deposit = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new(env.pool, false), AccountMeta::new(position, false), AccountMeta::new(alice_ata, false),
        AccountMeta::new(holding, false), AccountMeta::new(env.slab, false), AccountMeta::new(env.perc_vault, false),
        AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(system_program::ID, false)], data: dep };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[deposit], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("deposit");
    let mut c = svm.get_sysvar::<Clock>(); c.slot += 8; svm.set_sysvar::<Clock>(&c);
    let ballot = Pubkey::find_program_address(&[b"gv_ballot", env.gv_config.as_ref(), alice.pubkey().as_ref()], &gv_id_e2e()).0;
    let vote = Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(ballot, false), AccountMeta::new(gv_proposal, false),
        AccountMeta::new(position, false), AccountMeta::new_readonly(env.pool, false), AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(sub_id(), false)], data: vec![3u8, 1u8] };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[vote], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("vote");
    let trigger = Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(gv_proposal, false),
        AccountMeta::new_readonly(dist_id_e2e(), false), AccountMeta::new(env.dist_config, false), AccountMeta::new(dist_proposal, false),
        AccountMeta::new_readonly(env.pool, false)], data: vec![4u8] };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[trigger], Some(&payer.pubkey()), &[&payer], bh)).expect("seal the winner");

    // POST-SEAL: creating a fresh proposal on the now-sealed config is rejected.
    let id2 = 99u64;
    let dist_proposal2 = Pubkey::find_program_address(&[b"dist_proposal", env.dist_config.as_ref(), &id2.to_le_bytes()], &dist_id_e2e()).0;
    let mut cd = vec![1u8]; cd.extend_from_slice(&id2.to_le_bytes()); cd.extend_from_slice(&4u32.to_le_bytes());
    let create = Instruction { program_id: dist_id_e2e(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(env.dist_config, false), AccountMeta::new(dist_proposal2, false), AccountMeta::new_readonly(system_program::ID, false)], data: cd };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    assert!(svm.send_transaction(Transaction::new_signed_with_payer(&[create], Some(&payer.pubkey()), &[&payer], bh)).is_err(),
        "no new proposal can be created after the genesis distribution is sealed");
    // The sealed winner is unchanged.
    let dist_cfg = svm.get_account(&env.dist_config).unwrap();
    assert_eq!(Pubkey::new_from_array(dist_cfg.data[120..152].try_into().unwrap()), dist_proposal, "sealed winner unchanged");
}

// ===========================================================================
// Buy/burn uniform-price (Dutch) auction — end-to-end against the real binaries
// ===========================================================================

fn book_pda(cfg: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"twap_book", cfg.as_ref()], &twap_id()).0
}
fn book_escrow_pda(cfg: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"twap_book_escrow", cfg.as_ref()], &twap_id()).0
}
fn mint_supply(svm: &LiteSVM, mint: &Pubkey) -> u64 {
    let a = svm.get_account(mint).unwrap();
    u64::from_le_bytes(a.data[36..44].try_into().unwrap())
}
fn read_reserved_floor(svm: &LiteSVM, cfg: &Pubkey) -> u128 {
    let a = svm.get_account(cfg).unwrap();
    u128::from_le_bytes(a.data[173..189].try_into().unwrap())
}
fn warp_to(svm: &mut LiteSVM, slot: u64) {
    let mut c = svm.get_sysvar::<Clock>();
    c.slot = slot;
    svm.set_sysvar(&c);
}
fn mint_coin(svm: &mut LiteSVM, payer: &Keypair, mint: &Pubkey, authority: &Keypair, dest: &Pubkey, amount: u64) {
    let ix = spl_token::instruction::mint_to(&spl_token::ID, mint, dest, &authority.pubkey(), &[], amount).unwrap();
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[payer, authority], bh)).expect("mint coin");
}

// Squads vault-transaction message wrapping twap.init_book (tag 5). squads_vault is a WRITABLE
// signer (it pays the book account rent), book is a writable non-signer; the rest are read-only.
#[allow(clippy::too_many_arguments)]
fn build_init_book_message(
    squads_vault: &Pubkey, book: &Pubkey, config: &Pubkey, book_escrow: &Pubkey, coin_escrow: &Pubkey,
    settlement_usd: &Pubkey, holding: &Pubkey, coin_mint: &Pubkey, collateral_mint: &Pubkey,
    reserve_num: u128, reserve_den: u128, round_length: u64, sink_mode: u8, bid_fee: u64,
    coin_sink: Option<&Pubkey>, // included only in SEND mode (init_book reads it last)
) -> Vec<u8> {
    let mut m = Vec::new();
    let n_keys: u8 = if coin_sink.is_some() { 12 } else { 11 };
    let twap_idx: u8 = n_keys - 1; // twap program is the last key
    m.push(1); // num_signers
    m.push(1); // num_writable_signers (squads_vault pays rent)
    m.push(1); // num_writable_non_signers (book)
    m.push(n_keys); // account_keys
    m.extend_from_slice(squads_vault.as_ref());   // 0 writable signer
    m.extend_from_slice(book.as_ref());           // 1 writable non-signer
    m.extend_from_slice(config.as_ref());         // 2 ro
    m.extend_from_slice(book_escrow.as_ref());    // 3 ro
    m.extend_from_slice(coin_escrow.as_ref());    // 4 ro
    m.extend_from_slice(settlement_usd.as_ref()); // 5 ro
    m.extend_from_slice(coin_mint.as_ref());      // 6 ro
    m.extend_from_slice(collateral_mint.as_ref());// 7 ro
    m.extend_from_slice(system_program::ID.as_ref()); // 8 ro
    m.extend_from_slice(holding.as_ref());        // 9 ro
    if let Some(s) = coin_sink {
        m.extend_from_slice(s.as_ref());          // 10 ro coin sink (SEND mode)
    }
    m.extend_from_slice(twap_id().as_ref());      // program (last)
    m.push(1); // instructions
    m.push(twap_idx); // program_id_index -> twap
    // account_indexes — the order init_book reads: squads_vault, config, book, book_escrow,
    // coin_escrow, settlement_usd, holding, coin_mint, collateral_mint, system_program, [coin_sink].
    let mut idx = vec![0u8, 2, 1, 3, 4, 5, 9, 6, 7, 8];
    if coin_sink.is_some() { idx.push(10); }
    m.push(idx.len() as u8);
    for i in idx { m.push(i); }
    let mut data = vec![5u8];
    data.extend_from_slice(&reserve_num.to_le_bytes());
    data.extend_from_slice(&reserve_den.to_le_bytes());
    data.extend_from_slice(&round_length.to_le_bytes());
    data.push(sink_mode);
    data.extend_from_slice(&bid_fee.to_le_bytes());
    m.extend_from_slice(&(data.len() as u16).to_le_bytes());
    m.extend_from_slice(&data);
    m.push(0);
    m
}

// Squads message wrapping twap.shutdown (tag 11): sweep the holding USD to `dest`.
fn build_shutdown_message(squads_vault: &Pubkey, config: &Pubkey, twap_authority: &Pubkey, holding: &Pubkey, dest: &Pubkey) -> Vec<u8> {
    let mut m = Vec::new();
    m.push(1); m.push(0); m.push(2); m.push(7);
    m.extend_from_slice(squads_vault.as_ref());  // 0 ro signer
    m.extend_from_slice(holding.as_ref());        // 1 w
    m.extend_from_slice(dest.as_ref());           // 2 w
    m.extend_from_slice(config.as_ref());         // 3 ro
    m.extend_from_slice(twap_authority.as_ref()); // 4 ro
    m.extend_from_slice(spl_token::ID.as_ref());  // 5 ro token program
    m.extend_from_slice(twap_id().as_ref());      // 6 program
    m.push(1); m.push(6); m.push(6);
    for i in [0u8, 3, 4, 1, 2, 5] { m.push(i); }
    let data = [11u8];
    m.extend_from_slice(&(data.len() as u16).to_le_bytes());
    m.extend_from_slice(&data);
    m.push(0);
    m
}

#[allow(clippy::too_many_arguments)]
fn place_bid_ix(
    bidder: &Pubkey, config: &Pubkey, book: &Pubkey, book_escrow: &Pubkey, coin_escrow: &Pubkey,
    bidder_coin_src: &Pubkey, usd_dest: &Pubkey, coin_mint: &Pubkey, collateral_mint: &Pubkey,
    coin_atoms: u128, usdc_atoms: u128, evict: Option<Pubkey>,
) -> Instruction {
    let mut accounts = vec![
        AccountMeta::new(*bidder, true),
        AccountMeta::new_readonly(*config, false),
        AccountMeta::new(*book, false),
        AccountMeta::new_readonly(*book_escrow, false),
        AccountMeta::new(*coin_escrow, false),
        AccountMeta::new(*bidder_coin_src, false),
        AccountMeta::new_readonly(*usd_dest, false),
        AccountMeta::new(*coin_mint, false), // writable: place_bid burns the anti-spam fee from it
        AccountMeta::new_readonly(*collateral_mint, false),
        AccountMeta::new_readonly(spl_token::ID, false),
    ];
    if let Some(e) = evict { accounts.push(AccountMeta::new(e, false)); }
    let mut data = vec![7u8];
    data.extend_from_slice(&coin_atoms.to_le_bytes());
    data.extend_from_slice(&usdc_atoms.to_le_bytes());
    Instruction { program_id: twap_id(), accounts, data }
}

#[allow(clippy::too_many_arguments)]
fn execute_ix(
    cranker: &Pubkey, env: &HandoffEnv, book: &Pubkey, holding: &Pubkey, settlement_usd: &Pubkey,
    book_escrow: &Pubkey, coin_escrow: &Pubkey, coin_sink: Option<Pubkey>,
) -> Instruction {
    let mut accounts = vec![
        AccountMeta::new(*cranker, true),
        AccountMeta::new(env.twap_cfg, false),
        AccountMeta::new(*book, false),
        AccountMeta::new_readonly(env.twap_authority, false),
        AccountMeta::new(env.slab, false),
        AccountMeta::new(env.perc_vault, false),
        AccountMeta::new_readonly(env.vault_authority, false),
        AccountMeta::new_readonly(perc_id(), false),
        AccountMeta::new(*holding, false),
        AccountMeta::new(*settlement_usd, false),
        AccountMeta::new_readonly(*book_escrow, false),
        AccountMeta::new(*coin_escrow, false),
        AccountMeta::new(env.coin_mint, false),
        AccountMeta::new_readonly(spl_token::ID, false),
    ];
    if let Some(s) = coin_sink { accounts.push(AccountMeta::new(s, false)); }
    Instruction { program_id: twap_id(), accounts, data: vec![8u8] }
}

fn claim_ix(
    cranker: &Pubkey, config: &Pubkey, book: &Pubkey, book_escrow: &Pubkey, settlement_usd: &Pubkey,
    coin_escrow: &Pubkey, usd_dest: &Pubkey, coin_ata: &Pubkey, slot_index: u8,
) -> Instruction {
    Instruction {
        program_id: twap_id(),
        accounts: vec![
            AccountMeta::new(*cranker, true),
            AccountMeta::new_readonly(*config, false),
            AccountMeta::new(*book, false),
            AccountMeta::new_readonly(*book_escrow, false),
            AccountMeta::new(*settlement_usd, false),
            AccountMeta::new(*coin_escrow, false),
            AccountMeta::new(*usd_dest, false),
            AccountMeta::new(*coin_ata, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data: vec![9u8, slot_index],
    }
}

struct BookEnv {
    book: Pubkey,
    book_escrow: Pubkey,
    coin_escrow: Pubkey,
    settlement_usd: Pubkey,
    holding: Pubkey,
}

// setup_handoff + an initialised AuctionBook (reserve = accept-all, BURN sink) ready for bids.
#[allow(clippy::too_many_arguments)]
fn setup_auction(svm: &mut LiteSVM, payer: &Keypair, env: &HandoffEnv, round_length: u64, sink_mode: u8, coin_sink: Option<Pubkey>, bid_fee: u64) -> BookEnv {
    let book = book_pda(&env.twap_cfg);
    let book_escrow = book_escrow_pda(&env.twap_cfg);
    let coin_escrow = Pubkey::new_unique();
    let settlement_usd = Pubkey::new_unique();
    let holding = Pubkey::new_unique();
    set_token(svm, &coin_escrow, &env.coin_mint, &book_escrow, 0);
    set_token(svm, &settlement_usd, &env.collateral_mint, &book_escrow, 0);
    set_token(svm, &holding, &env.collateral_mint, &env.twap_authority, 0);
    svm.airdrop(&env.squads_vault, 1_000_000_000).unwrap();
    let msg = build_init_book_message(&env.squads_vault, &book, &env.twap_cfg, &book_escrow, &coin_escrow,
        &settlement_usd, &holding, &env.coin_mint, &env.collateral_mint, 0, 1, round_length, sink_mode, bid_fee, coin_sink.as_ref());
    let mut rem = vec![
        AccountMeta::new(env.squads_vault, false), AccountMeta::new(book, false),
        AccountMeta::new_readonly(env.twap_cfg, false), AccountMeta::new_readonly(book_escrow, false),
        AccountMeta::new_readonly(coin_escrow, false), AccountMeta::new_readonly(settlement_usd, false),
        AccountMeta::new_readonly(env.coin_mint, false), AccountMeta::new_readonly(env.collateral_mint, false),
        AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(holding, false),
    ];
    if let Some(s) = coin_sink { rem.push(AccountMeta::new_readonly(s, false)); }
    rem.push(AccountMeta::new_readonly(twap_id(), false));
    squads_execute(svm, &env.squads, &env.multisig, &env.dao, payer, 5, &msg, &rem).expect("init_book");
    BookEnv { book, book_escrow, coin_escrow, settlement_usd, holding }
}

// A bidder with a funded COIN source account and an empty collateral USD destination.
// A bidder's canonical COIN ATA — the auction's pinned refund target (matches the program's
// bidder_coin_ata). Reuses the ATA derivation (= associated-token-address).
fn coin_ata_of(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    canonical_insurance_vault(owner, mint)
}

fn new_bidder(svm: &mut LiteSVM, payer: &Keypair, env: &HandoffEnv, coin_amount: u64) -> (Keypair, Pubkey, Pubkey) {
    let bidder = Keypair::new();
    svm.airdrop(&bidder.pubkey(), 1_000_000_000).unwrap();
    // Fund + bid from the bidder's CANONICAL ATA, which is also the pinned COIN refund target.
    let coin_src = coin_ata_of(&bidder.pubkey(), &env.coin_mint);
    set_token(svm, &coin_src, &env.coin_mint, &bidder.pubkey(), 0);
    mint_coin(svm, payer, &env.coin_mint, &env.coin_mint_authority, &coin_src, coin_amount);
    let usd_dest = Pubkey::new_unique();
    set_token(svm, &usd_dest, &env.collateral_mint, &bidder.pubkey(), 0);
    (bidder, coin_src, usd_dest)
}

fn send(svm: &mut LiteSVM, signers: &[&Keypair], ix: Instruction) -> Result<(), String> {
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[ix], Some(&signers[0].pubkey()), signers, bh))
        .map(|_| ())
        .map_err(|e| format!("{:?}", e.err))
}

// HEADLINE: a full buy/burn — three bids at different rates clear at ONE marginal uniform price,
// the bought COIN is really burned (mint supply drops), winners' USD is parked and claimed, and
// surplus COIN is refunded. All against the real percolator + Squads + twap binaries.
#[test]
fn e2e_buy_burn_uniform_price_dutch_auction() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(twap_id(), so_deploy("twap_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_handoff(&mut svm, &payer);
    let bk = setup_auction(&mut svm, &payer, &env, 10, 0, None, 0);

    // surplus = insurance(1.5M) - floor(1M) = 500k; burn-share (80%) = 400k = the auction budget.
    // Bids (COIN offered for USD wanted): alice 600k/200k (rate 3), bob 400k/200k (rate 2),
    // carol 100k/200k (rate 0.5). Budget 400k fills alice + bob; carol is left out. Marginal = bob,
    // so the uniform clearing price P* = 2 COIN/USD applies to EVERY winner.
    let (alice, a_src, a_usd) = new_bidder(&mut svm, &payer, &env, 600_000);
    let (bob, b_src, b_usd) = new_bidder(&mut svm, &payer, &env, 400_000);
    let (carol, c_src, c_usd) = new_bidder(&mut svm, &payer, &env, 100_000);
    let supply_before = mint_supply(&svm, &env.coin_mint);
    assert_eq!(supply_before, 1_100_000, "all bid COIN minted");

    send(&mut svm, &[&alice], place_bid_ix(&alice.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &a_src, &a_usd, &env.coin_mint, &env.collateral_mint, 600_000, 200_000, None)).expect("alice bid");
    send(&mut svm, &[&bob], place_bid_ix(&bob.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &b_src, &b_usd, &env.coin_mint, &env.collateral_mint, 400_000, 200_000, None)).expect("bob bid");
    send(&mut svm, &[&carol], place_bid_ix(&carol.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &c_src, &c_usd, &env.coin_mint, &env.collateral_mint, 100_000, 200_000, None)).expect("carol bid");
    assert_eq!(token_amount(&svm, &bk.coin_escrow), 1_100_000, "all bid COIN escrowed");

    // Round still open -> execute must be rejected.
    let cranker = Keypair::new(); svm.airdrop(&cranker.pubkey(), 1_000_000_000).unwrap();
    assert!(send(&mut svm, &[&cranker], execute_ix(&cranker.pubkey(), &env, &bk.book, &bk.holding, &bk.settlement_usd, &bk.book_escrow, &bk.coin_escrow, None)).is_err(),
        "execute before the round expires must fail");

    warp_to(&mut svm, 111);
    send(&mut svm, &[&cranker], execute_ix(&cranker.pubkey(), &env, &bk.book, &bk.holding, &bk.settlement_usd, &bk.book_escrow, &bk.coin_escrow, None)).expect("execute");

    // 400k COIN (alice) + 400k COIN (bob) bought at P*=2 and BURNED; carol untouched.
    assert_eq!(mint_supply(&svm, &env.coin_mint), 1_100_000 - 800_000, "800k COIN really burned");
    assert_eq!(token_amount(&svm, &bk.settlement_usd), 400_000, "spent USD parked for winners");
    assert_eq!(token_amount(&svm, &bk.holding), 0, "all pulled USD was spent");
    // retained 20% (100k) ratcheted into the principal counter.
    assert_eq!(read_reserved_floor(&svm, &env.twap_cfg), 1_100_000, "principal counter ratcheted by the retained surplus");

    // Claims (permissionless). Slot order = placement order: alice 0, bob 1, carol 2.
    send(&mut svm, &[&cranker], claim_ix(&cranker.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.settlement_usd, &bk.coin_escrow, &a_usd, &a_src, 0)).expect("alice claim");
    send(&mut svm, &[&cranker], claim_ix(&cranker.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.settlement_usd, &bk.coin_escrow, &b_usd, &b_src, 1)).expect("bob claim");
    send(&mut svm, &[&cranker], claim_ix(&cranker.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.settlement_usd, &bk.coin_escrow, &c_usd, &c_src, 2)).expect("carol claim");

    assert_eq!(token_amount(&svm, &a_usd), 200_000, "alice paid USD at the uniform price");
    assert_eq!(token_amount(&svm, &a_src), 200_000, "alice's surplus COIN (600k offered - 400k sold) refunded");
    assert_eq!(token_amount(&svm, &b_usd), 200_000, "bob paid the SAME uniform price");
    assert_eq!(token_amount(&svm, &b_src), 0, "bob sold his full 400k at P*");
    assert_eq!(token_amount(&svm, &c_usd), 0, "carol won nothing");
    assert_eq!(token_amount(&svm, &c_src), 100_000, "carol's COIN fully refunded");
    assert_eq!(token_amount(&svm, &bk.settlement_usd), 0, "settlement fully claimed");
    assert_eq!(token_amount(&svm, &bk.coin_escrow), 0, "escrow drained");
    let _ = (&alice, &bob, &carol);
}

// ANTI-SPOOF: a placed bid cannot be cancelled. There is no withdraw instruction; the only way a
// bid leaves the book early is eviction by a STRICTLY better bid (which refunds the evictee), and
// a not-better bid against a full book is rejected.
#[test]
fn e2e_bid_cannot_be_cancelled_only_evicted_by_a_better_bid() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(twap_id(), so_deploy("twap_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_handoff(&mut svm, &payer);
    let bk = setup_auction(&mut svm, &payer, &env, 10, 0, None, 0);

    // There is no cancel/withdraw tag — sending the (removed) pull tag, or any unknown tag, fails,
    // and the only mutators of a placed bid are eviction (place_bid) and post-execute claim. A
    // bidder thus cannot reclaim COIN before the auction runs.
    let (alice, a_src, a_usd) = new_bidder(&mut svm, &payer, &env, 100_000);
    send(&mut svm, &[&alice], place_bid_ix(&alice.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &a_src, &a_usd, &env.coin_mint, &env.collateral_mint, 100_000, 100_000, None)).expect("alice bid");
    assert_eq!(token_amount(&svm, &a_src), 0, "alice's COIN is escrowed and committed");
    // A claim before execute is rejected (the slot is not settled).
    assert!(send(&mut svm, &[&alice], claim_ix(&alice.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.settlement_usd, &bk.coin_escrow, &a_usd, &a_src, 0)).is_err(),
        "cannot claim/withdraw a bid before the auction settles");
    assert_eq!(token_amount(&svm, &a_src), 0, "still committed — no early exit");

    // A better bid for the SAME bidder is rejected (one active bid per bidder, no self-replace).
    let a_src2 = Pubkey::new_unique();
    set_token(&mut svm, &a_src2, &env.coin_mint, &alice.pubkey(), 0);
    mint_coin(&mut svm, &payer, &env.coin_mint, &env.coin_mint_authority, &a_src2, 500_000);
    assert!(send(&mut svm, &[&alice], place_bid_ix(&alice.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &a_src2, &a_usd, &env.coin_mint, &env.collateral_mint, 500_000, 100_000, None)).is_err(),
        "a bidder cannot stack a second bid");
    let _ = alice;
}

// FINDING O (now enforced by execute, the sole puller): execute pulls only the burn-share of the
// surplus and ratchets the retained share into the principal counter — it can never reach
// principal. A second execute when the surplus is exhausted pulls nothing.
#[test]
fn e2e_execute_pulls_only_burn_share_and_ratchets_principal() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(twap_id(), so_deploy("twap_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_handoff(&mut svm, &payer);
    let bk = setup_auction(&mut svm, &payer, &env, 10, 0, None, 0);

    let cranker = Keypair::new(); svm.airdrop(&cranker.pubkey(), 1_000_000_000).unwrap();
    warp_to(&mut svm, 111);
    // A non-canonical (but twap_authority-owned) holding is rejected — the budget can't be routed
    // into a different account and fragmented.
    let rogue_holding = Pubkey::new_unique();
    set_token(&mut svm, &rogue_holding, &env.collateral_mint, &env.twap_authority, 0);
    assert!(send(&mut svm, &[&cranker], execute_ix(&cranker.pubkey(), &env, &bk.book, &rogue_holding, &bk.settlement_usd, &bk.book_escrow, &bk.coin_escrow, None)).is_err(),
        "execute must reject a holding other than the book's pinned one");
    assert_eq!(token_amount(&svm, &rogue_holding), 0, "rogue holding never funded");

    // No bids: execute still pulls the burn-share + ratchets, then rolls. surplus=500k, burn=400k.
    send(&mut svm, &[&cranker], execute_ix(&cranker.pubkey(), &env, &bk.book, &bk.holding, &bk.settlement_usd, &bk.book_escrow, &bk.coin_escrow, None)).expect("execute 1");
    assert_eq!(token_amount(&svm, &bk.holding), 400_000, "only the 80% burn-share left insurance");
    assert_eq!(token_amount(&svm, &env.perc_vault), 1_100_000, "20% retained stays in insurance");
    assert_eq!(read_reserved_floor(&svm, &env.twap_cfg), 1_100_000, "retained ratcheted into the principal counter");

    // Surplus is now exhausted (insurance == floor == 1.1M); a second execute pulls nothing.
    warp_to(&mut svm, 211);
    send(&mut svm, &[&cranker], execute_ix(&cranker.pubkey(), &env, &bk.book, &bk.holding, &bk.settlement_usd, &bk.book_escrow, &bk.coin_escrow, None)).expect("execute 2");
    assert_eq!(token_amount(&svm, &bk.holding), 400_000, "no further pull — principal is untouchable");
    assert_eq!(token_amount(&svm, &env.perc_vault), 1_100_000, "insurance never crosses the floor");
}

// SHUTDOWN: only the DAO (via a timelock'd Squads execute) can sweep the TWAP's accumulated USD to
// a supplied destination; a permissionless caller cannot.
#[test]
fn e2e_shutdown_sweeps_holding_only_via_squads() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(twap_id(), so_deploy("twap_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_handoff(&mut svm, &payer);
    let bk = setup_auction(&mut svm, &payer, &env, 10, 0, None, 0);

    // Accumulate USD in the holding (one no-bid execute pulls the 400k burn-share).
    let cranker = Keypair::new(); svm.airdrop(&cranker.pubkey(), 1_000_000_000).unwrap();
    warp_to(&mut svm, 111);
    send(&mut svm, &[&cranker], execute_ix(&cranker.pubkey(), &env, &bk.book, &bk.holding, &bk.settlement_usd, &bk.book_escrow, &bk.coin_escrow, None)).expect("execute");
    assert_eq!(token_amount(&svm, &bk.holding), 400_000);

    // A non-DAO caller cannot sweep: forge a shutdown ix signed by an attacker as the "squads vault".
    let attacker = Keypair::new(); svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    let dest = Pubkey::new_unique();
    set_token(&mut svm, &dest, &env.collateral_mint, &payer.pubkey(), 0);
    let rogue = Instruction { program_id: twap_id(), accounts: vec![
        AccountMeta::new(attacker.pubkey(), true), AccountMeta::new_readonly(env.twap_cfg, false),
        AccountMeta::new_readonly(env.twap_authority, false), AccountMeta::new(bk.holding, false),
        AccountMeta::new(dest, false), AccountMeta::new_readonly(spl_token::ID, false),
    ], data: vec![11u8] };
    assert!(send(&mut svm, &[&attacker], rogue).is_err(), "non-DAO shutdown must be rejected");
    assert_eq!(token_amount(&svm, &bk.holding), 400_000, "holding untouched by the attacker");

    // The DAO sweeps via a timelock'd Squads execute.
    let msg = build_shutdown_message(&env.squads_vault, &env.twap_cfg, &env.twap_authority, &bk.holding, &dest);
    let rem = vec![
        AccountMeta::new_readonly(env.squads_vault, false), AccountMeta::new(bk.holding, false), AccountMeta::new(dest, false),
        AccountMeta::new_readonly(env.twap_cfg, false), AccountMeta::new_readonly(env.twap_authority, false),
        AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(twap_id(), false),
    ];
    squads_execute(&mut svm, &env.squads, &env.multisig, &env.dao, &payer, 6, &msg, &rem).expect("dao shutdown");
    assert_eq!(token_amount(&svm, &bk.holding), 0, "DAO swept the holding");
    assert_eq!(token_amount(&svm, &dest), 400_000, "swept to the DAO-supplied address");
}

// FULL grand-unified E2E: subledger insurance deposits + genesis vote + COIN distribution
// + claim, then the DAO->Squads handoff of the insurance operator to the twap, then a real
// surplus pull. All six real binaries.
#[test]
fn e2e_full_genesis_to_buy_burn() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(gv_id_e2e(), so_deploy("genesis_vote_program")).unwrap();
    svm.add_program_from_file(dist_id_e2e(), so_deploy("distribution_program")).unwrap();
    svm.add_program_from_file(twap_id(), so_deploy("twap_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let squads = squads_id();
    let treasury = install_squads(&mut svm, &squads, &payer.pubkey());
    let mint_auth = Keypair::new();
    svm.airdrop(&mint_auth.pubkey(), 1_000_000_000).unwrap();

    // DAO + Squads multisig.
    let dao = Keypair::new();
    svm.airdrop(&dao.pubkey(), 1_000_000_000_000).unwrap();
    let create_key = Keypair::new();
    let multisig = multisig_pda(&squads, &create_key.pubkey());
    let create_ix = multisig_create_v2_ix(&squads, &treasury, &multisig, &create_key.pubkey(), &payer.pubkey(),
        Some(&dao.pubkey()), 1, &[(dao.pubkey(), PERM_ALL)], TIMELOCK_1_WEEK_SECS);
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create_ix], Some(&payer.pubkey()), &[&payer, &create_key], bh)).expect("multisig");
    let squads_vault = vault_pda(&squads, &multisig, 0);

    // market-0 with marketauth = squads vault.
    let collateral_mint = Pubkey::new_unique();
    let coin_mint = create_real_mint(&mut svm, &payer, &mint_auth.pubkey());
    let slab = Pubkey::new_unique();
    let init_slot = 100u64;
    let slab_data = make_live_market(&slab, &collateral_mint, &squads_vault, init_slot);
    svm.set_account(slab, Account { lamports: 1_000_000_000, data: slab_data, owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
    svm.set_sysvar(&Clock { slot: init_slot, unix_timestamp: 100, ..Clock::default() });
    let vault_authority = perc_vault_authority(&slab, &perc_id());
    let perc_vault = canonical_insurance_vault(&vault_authority, &collateral_mint);
    set_token(&mut svm, &perc_vault, &collateral_mint, &vault_authority, 0);

    let pool = sub_pool_pda(&collateral_mint, 0, &slab, &perc_id());
    let gv_config = gv_config_pda_e2e(&coin_mint, &pool);
    let dist_config = dist_config_pda_e2e(&coin_mint);

    // subledger insurance pool (vote_authority = gv config PDA, per finding R).
    let mut d = vec![3u8];
    d.extend_from_slice(&0u64.to_le_bytes());
    d.push(0);
    let init_pool = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true),
        AccountMeta::new_readonly(collateral_mint, false),
        AccountMeta::new(pool, false),
        AccountMeta::new_readonly(perc_vault, false),
        AccountMeta::new_readonly(slab, false),
        AccountMeta::new_readonly(perc_id(), false),
        AccountMeta::new_readonly(system_program::ID, false),
        AccountMeta::new_readonly(gv_config, false),
    ], data: d };
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[init_pool], Some(&payer.pubkey()), &[&payer], bh)).expect("init pool");

    // --- Inject insurance SURPLUS (squads is still the insurance_authority) ---
    let surplus = 500_000u64;
    let squads_src = Pubkey::new_unique();
    set_token(&mut svm, &squads_src, &collateral_mint, &squads_vault, surplus);
    let topup_msg = build_topup_message(&squads_vault, &slab, &squads_src, &perc_vault, &perc_id(), surplus as u128);
    let topup_remaining = vec![
        AccountMeta::new_readonly(squads_vault, false),
        AccountMeta::new(slab, false),
        AccountMeta::new(squads_src, false),
        AccountMeta::new(perc_vault, false),
        AccountMeta::new_readonly(spl_token::ID, false),
        AccountMeta::new_readonly(perc_id(), false),
    ];
    squads_execute(&mut svm, &squads, &multisig, &dao, &payer, 1, &topup_msg, &topup_remaining).expect("squads injects insurance surplus");
    assert_eq!(token_amount(&svm, &perc_vault), surplus, "surplus in insurance");

    // --- Grant operator+authority to the subledger pool ---
    let grant_msg = build_subledger_accept_operator_message(&squads_vault, &pool, &slab, &perc_id());
    let grant_remaining = vec![
        AccountMeta::new_readonly(squads_vault, false),
        AccountMeta::new(slab, false),
        AccountMeta::new_readonly(pool, false),
        AccountMeta::new_readonly(perc_id(), false),
        AccountMeta::new_readonly(sub_id(), false),
    ];
    squads_execute(&mut svm, &squads, &multisig, &dao, &payer, 2, &grant_msg, &grant_remaining).expect("grant operator to pool");

    // --- Genesis deposit (subledger TopUp as the granted authority) ---
    let alice = Keypair::new();
    svm.airdrop(&alice.pubkey(), 1_000_000_000).unwrap();
    let principal = 1_000_000u64;
    let alice_ata = Pubkey::new_unique();
    set_token(&mut svm, &alice_ata, &collateral_mint, &alice.pubkey(), principal);
    let holding = Pubkey::new_unique();
    set_token(&mut svm, &holding, &collateral_mint, &pool, 0);
    let position = sub_position_pda(&pool, &alice.pubkey());
    let mut dd = vec![4u8];
    dd.extend_from_slice(&principal.to_le_bytes());
    let deposit = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true),
        AccountMeta::new(pool, false),
        AccountMeta::new(position, false),
        AccountMeta::new(alice_ata, false),
        AccountMeta::new(holding, false),
        AccountMeta::new(slab, false),
        AccountMeta::new(perc_vault, false),
        AccountMeta::new_readonly(perc_id(), false),
        AccountMeta::new_readonly(spl_token::ID, false),
        AccountMeta::new_readonly(system_program::ID, false),
    ], data: dd };
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[deposit], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("genesis deposit");
    assert_eq!(token_amount(&svm, &perc_vault), surplus + principal, "insurance = surplus + principal");

    // --- Distribution setup: fund + freeze a fixed-supply COIN ---
    let total_supply = 100u64;
    let dist_vault = Pubkey::new_unique();
    set_token(&mut svm, &dist_vault, &coin_mint, &dist_config, 0);
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(
        &[spl_token::instruction::mint_to(&spl_token::ID, &coin_mint, &dist_vault, &mint_auth.pubkey(), &[], total_supply).unwrap()],
        Some(&payer.pubkey()), &[&payer, &mint_auth], bh)).expect("mint coin");
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(
        &[spl_token::instruction::set_authority(&spl_token::ID, &coin_mint, None, spl_token::instruction::AuthorityType::MintTokens, &mint_auth.pubkey(), &[]).unwrap()],
        Some(&payer.pubkey()), &[&payer, &mint_auth], bh)).expect("revoke mint auth");
    // distribution init_config (authority = gv config)
    let mut data = vec![0u8];
    data.extend_from_slice(&1_000_000u64.to_le_bytes()); // claim window
    data.extend_from_slice(&total_supply.to_le_bytes());
    let dist_init = Instruction { program_id: dist_id_e2e(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true),
        AccountMeta::new_readonly(coin_mint, false),
        AccountMeta::new(dist_config, false),
        AccountMeta::new_readonly(dist_vault, false),
        AccountMeta::new_readonly(gv_config, false),
        AccountMeta::new_readonly(system_program::ID, false),
    ], data };
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[dist_init], Some(&payer.pubkey()), &[&payer], bh)).expect("dist init");
    // gv init_config
    let gv_init = Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true),
        AccountMeta::new_readonly(coin_mint, false),
        AccountMeta::new(gv_config, false),
        AccountMeta::new_readonly(dist_id_e2e(), false),
        AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new_readonly(sub_id(), false),
        AccountMeta::new_readonly(pool, false),
        AccountMeta::new_readonly(Pubkey::default(), false),
        AccountMeta::new_readonly(system_program::ID, false),
    ], data: vec![0u8] };
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[gv_init], Some(&payer.pubkey()), &[&payer], bh)).expect("gv init");

    // --- Proposal: full COIN supply to a recipient; create + register ---
    let recipient = Keypair::new();
    // The winner's canonical COIN ATA (also the auction's pinned bid-refund target).
    let recipient_ata = coin_ata_of(&recipient.pubkey(), &coin_mint);
    set_token(&mut svm, &recipient_ata, &coin_mint, &recipient.pubkey(), 0);
    let id = 1u64;
    let dist_proposal = Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &id.to_le_bytes()], &dist_id_e2e()).0;
    let mut cd = vec![1u8]; cd.extend_from_slice(&id.to_le_bytes()); cd.extend_from_slice(&4u32.to_le_bytes());
    let create = Instruction { program_id: dist_id_e2e(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new(dist_proposal, false), AccountMeta::new_readonly(system_program::ID, false),
    ], data: cd };
    let mut ad = vec![2u8]; ad.extend_from_slice(&1u32.to_le_bytes()); ad.extend_from_slice(recipient.pubkey().as_ref()); ad.extend_from_slice(&total_supply.to_le_bytes());
    let append = Instruction { program_id: dist_id_e2e(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(dist_proposal, false),
    ], data: ad };
    let gv_proposal = Pubkey::find_program_address(&[b"gv_proposal", gv_config.as_ref(), dist_proposal.as_ref()], &gv_id_e2e()).0;
    let reg = Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(gv_config, false), AccountMeta::new(gv_proposal, false),
        AccountMeta::new_readonly(dist_proposal, false), AccountMeta::new_readonly(system_program::ID, false),
    ], data: vec![2u8] };
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[create, append, reg], Some(&payer.pubkey()), &[&payer], bh)).expect("create+append+register");

    // --- Vote + trigger (warp slot so the position has vote weight) ---
    let mut clock = svm.get_sysvar::<Clock>();
    clock.slot = 1124;
    svm.set_sysvar::<Clock>(&clock);
    let gv_ballot = Pubkey::find_program_address(&[b"gv_ballot", gv_config.as_ref(), alice.pubkey().as_ref()], &gv_id_e2e()).0;
    let vote = Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new(gv_config, false), AccountMeta::new(gv_ballot, false),
        AccountMeta::new(gv_proposal, false), AccountMeta::new(position, false), AccountMeta::new_readonly(pool, false),
        AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(sub_id(), false),
    ], data: vec![3u8, 1u8] };
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[vote], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("vote");
    let trigger = Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(gv_config, false), AccountMeta::new(gv_proposal, false),
        AccountMeta::new_readonly(dist_id_e2e(), false), AccountMeta::new(dist_config, false), AccountMeta::new(dist_proposal, false),
        AccountMeta::new_readonly(pool, false),
    ], data: vec![4u8] };
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[trigger], Some(&payer.pubkey()), &[&payer], bh)).expect("trigger seals distribution");

    // --- Recipient claims the COIN ---
    let mut cl = vec![4u8]; cl.extend_from_slice(&0u32.to_le_bytes());
    let claim = Instruction { program_id: dist_id_e2e(), accounts: vec![
        AccountMeta::new_readonly(recipient.pubkey(), true), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new(dist_proposal, false), AccountMeta::new(dist_vault, false), AccountMeta::new(recipient_ata, false),
        AccountMeta::new_readonly(spl_token::ID, false),
    ], data: cl };
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[claim], Some(&payer.pubkey()), &[&payer, &recipient], bh)).expect("claim COIN");
    assert_eq!(token_amount(&svm, &recipient_ata), total_supply, "winner claimed the full COIN supply");

    // --- Handoff: DAO rotates the insurance policy to surplus-mode, then the operator to the twap ---
    // twap config for this market.
    let twap_init = init_config_ix(&payer.pubkey(), &coin_mint, &slab, &multisig, &dao.pubkey(), &perc_id());
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[twap_init], Some(&payer.pubkey()), &[&payer], bh)).expect("twap init");
    let twap_cfg = twap_config_pda(&slab, &multisig, &coin_mint, &perc_id());
    let twap_authority = Pubkey::find_program_address(&[b"market-0-twap", slab.as_ref()], &twap_id()).0;

    // policy -> surplus mode (deposits_only = 0, max_bps < 1e4, cooldown != 0).
    let policy_msg = build_update_insurance_policy_message(&squads_vault, &slab, &perc_id(), 8_000, 0, 100);
    let policy_remaining = vec![
        AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(slab, false), AccountMeta::new_readonly(perc_id(), false),
    ];
    squads_execute(&mut svm, &squads, &multisig, &dao, &payer, 3, &policy_msg, &policy_remaining).expect("rotate policy to surplus-mode");

    // operator -> twap.
    let op_msg = build_accept_operator_message(&squads_vault, &slab, &twap_cfg, &twap_authority, &perc_id(), &twap_id());
    let op_remaining = vec![
        AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(slab, false), AccountMeta::new_readonly(twap_cfg, false),
        AccountMeta::new_readonly(twap_authority, false), AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(twap_id(), false),
    ];
    squads_execute(&mut svm, &squads, &multisig, &dao, &payer, 4, &op_msg, &op_remaining).expect("rotate operator to twap");

    // DAO sets the surplus floor = the reserved depositor principal (finding O fix). Until
    // this, the twap's reserved_floor is u128::MAX and pull_surplus pulls nothing.
    let floor_msg = build_set_reserved_floor_message(&squads_vault, &twap_cfg, principal as u128);
    let floor_remaining = vec![AccountMeta::new_readonly(squads_vault, false), AccountMeta::new(twap_cfg, false), AccountMeta::new_readonly(twap_id(), false)];
    squads_execute(&mut svm, &squads, &multisig, &dao, &payer, 5, &floor_msg, &floor_remaining).expect("set surplus floor = reserved principal");

    // --- The TWAP runs the buy/burn AUCTION (the final link). The COIN winner sells COIN back
    //     into the surplus buy/burn and it is BURNED — closing the genesis loop end to end. ---
    let book = book_pda(&twap_cfg);
    let book_escrow = book_escrow_pda(&twap_cfg);
    let coin_escrow = Pubkey::new_unique();
    let settlement_usd = Pubkey::new_unique();
    let holding = Pubkey::new_unique();
    set_token(&mut svm, &coin_escrow, &coin_mint, &book_escrow, 0);
    set_token(&mut svm, &settlement_usd, &collateral_mint, &book_escrow, 0);
    set_token(&mut svm, &holding, &collateral_mint, &twap_authority, 0);
    svm.airdrop(&squads_vault, 1_000_000_000).unwrap();
    let ib = build_init_book_message(&squads_vault, &book, &twap_cfg, &book_escrow, &coin_escrow,
        &settlement_usd, &holding, &coin_mint, &collateral_mint, 0, 1, 10, 0, 0, None);
    let ib_rem = vec![
        AccountMeta::new(squads_vault, false), AccountMeta::new(book, false),
        AccountMeta::new_readonly(twap_cfg, false), AccountMeta::new_readonly(book_escrow, false),
        AccountMeta::new_readonly(coin_escrow, false), AccountMeta::new_readonly(settlement_usd, false),
        AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(collateral_mint, false),
        AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(holding, false),
        AccountMeta::new_readonly(twap_id(), false),
    ];
    squads_execute(&mut svm, &squads, &multisig, &dao, &payer, 6, &ib, &ib_rem).expect("init auction book");

    // The COIN winner bids: offer 50 of the 100 claimed COIN for the surplus USD.
    svm.airdrop(&recipient.pubkey(), 1_000_000_000).unwrap();
    let r_usd = Pubkey::new_unique();
    set_token(&mut svm, &r_usd, &collateral_mint, &recipient.pubkey(), 0);
    let place = place_bid_ix(&recipient.pubkey(), &twap_cfg, &book, &book_escrow, &coin_escrow,
        &recipient_ata, &r_usd, &coin_mint, &collateral_mint, 50, 400_000, None);
    send(&mut svm, &[&recipient], place).expect("winner bids COIN into the buy/burn");

    // Round expires; anyone executes. It pulls 80% of the 500k surplus (=400k) as the budget,
    // ratchets the retained 100k into the principal counter, clears the bid at the uniform price,
    // and BURNS the bought COIN.
    let mut c = svm.get_sysvar::<Clock>(); c.slot = 1140; svm.set_sysvar(&c);
    let supply_before = mint_supply(&svm, &coin_mint);
    assert_eq!(supply_before, total_supply, "full COIN supply outstanding before the burn");
    let cranker = Keypair::new(); svm.airdrop(&cranker.pubkey(), 1_000_000_000).unwrap();
    let exec = Instruction { program_id: twap_id(), accounts: vec![
        AccountMeta::new(cranker.pubkey(), true), AccountMeta::new(twap_cfg, false), AccountMeta::new(book, false),
        AccountMeta::new_readonly(twap_authority, false), AccountMeta::new(slab, false), AccountMeta::new(perc_vault, false),
        AccountMeta::new_readonly(vault_authority, false), AccountMeta::new_readonly(perc_id(), false),
        AccountMeta::new(holding, false), AccountMeta::new(settlement_usd, false), AccountMeta::new_readonly(book_escrow, false),
        AccountMeta::new(coin_escrow, false), AccountMeta::new(coin_mint, false), AccountMeta::new_readonly(spl_token::ID, false),
    ], data: vec![8u8] };
    send(&mut svm, &[&cranker], exec).expect("twap executes the buy/burn");

    assert_eq!(mint_supply(&svm, &coin_mint), total_supply - 50, "the TWAP bought + BURNED 50 COIN");
    assert_eq!(token_amount(&svm, &settlement_usd), 400_000, "surplus USD parked for the winner");
    assert_eq!(token_amount(&svm, &perc_vault), principal + surplus - 400_000, "only the 80% burn-share left insurance");
    assert_eq!(read_reserved_floor(&svm, &twap_cfg), (principal + 100_000) as u128, "retained 20% ratcheted into the principal counter");

    // The winner permissionlessly claims their USD.
    send(&mut svm, &[&cranker], claim_ix(&cranker.pubkey(), &twap_cfg, &book, &book_escrow, &settlement_usd, &coin_escrow, &r_usd, &recipient_ata, 0)).expect("winner claims USD");
    assert_eq!(token_amount(&svm, &r_usd), 400_000, "winner received the surplus USD at the clearing price");
}

// ANTI-SPAM: a DAO-set flat per-bid fee (default 0.002 COIN) is BURNED on every place_bid, even
// if the bid is later evicted — so flooding the book has a real, non-refundable cost.
#[test]
fn e2e_bid_fee_is_charged_and_burned() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(twap_id(), so_deploy("twap_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_handoff(&mut svm, &payer);
    let fee = 2_000u64; // 0.002 COIN at 6 decimals
    let bk = setup_auction(&mut svm, &payer, &env, 10, 0, None, fee);

    // A bidder funded with exactly coin_atoms (no fee) is rejected.
    let (poor, p_src, p_usd) = new_bidder(&mut svm, &payer, &env, 10_000);
    assert!(send(&mut svm, &[&poor], place_bid_ix(&poor.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &p_src, &p_usd, &env.coin_mint, &env.collateral_mint, 10_000, 5_000, None)).is_err(),
        "a bid that cannot cover coin_atoms + fee is rejected");

    // A funded bidder pays: the fee is burned, only coin_atoms reaches the escrow.
    let (alice, a_src, a_usd) = new_bidder(&mut svm, &payer, &env, 10_000 + fee);
    let supply_before = mint_supply(&svm, &env.coin_mint);
    send(&mut svm, &[&alice], place_bid_ix(&alice.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &a_src, &a_usd, &env.coin_mint, &env.collateral_mint, 10_000, 5_000, None)).expect("alice bid");
    assert_eq!(mint_supply(&svm, &env.coin_mint), supply_before - fee, "the bid fee was burned");
    assert_eq!(token_amount(&svm, &bk.coin_escrow), 10_000, "only coin_atoms (not the fee) is escrowed");
    assert_eq!(token_amount(&svm, &a_src), 0, "source drained of coin_atoms + fee");
    let _ = (alice, poor);
}

fn cancel_ix(bidder: &Pubkey, config: &Pubkey, book: &Pubkey, book_escrow: &Pubkey, coin_escrow: &Pubkey, coin_ata: &Pubkey, slot_index: u8) -> Instruction {
    Instruction {
        program_id: twap_id(),
        accounts: vec![
            AccountMeta::new(*bidder, true),
            AccountMeta::new_readonly(*config, false),
            AccountMeta::new(*book, false),
            AccountMeta::new_readonly(*book_escrow, false),
            AccountMeta::new(*coin_escrow, false),
            AccountMeta::new(*coin_ata, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data: vec![13u8, slot_index],
    }
}

// CANCEL: an unsettled bid is reclaimable by its owner only AFTER the cooldown (an execute clears
// the book, or 2*round_length slots pass) — so there is no last-second cancel that could
// manipulate a pending execute. The escrowed COIN is returned but the anti-spam fee stays burned.
#[test]
fn e2e_bid_cancellable_after_cooldown_keeps_fee() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(twap_id(), so_deploy("twap_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_handoff(&mut svm, &payer);
    let round_length = 10u64;
    let fee = 2_000u64;
    let bk = setup_auction(&mut svm, &payer, &env, round_length, 0, None, fee);

    let (alice, a_src, a_usd) = new_bidder(&mut svm, &payer, &env, 10_000 + fee);
    send(&mut svm, &[&alice], place_bid_ix(&alice.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &a_src, &a_usd, &env.coin_mint, &env.collateral_mint, 10_000, 5_000, None)).expect("alice bid");
    let supply_after_place = mint_supply(&svm, &env.coin_mint);
    assert_eq!(token_amount(&svm, &bk.coin_escrow), 10_000, "coin escrowed");

    // Cancelling immediately is rejected — the cooldown blocks a last-second (race) cancel.
    assert!(send(&mut svm, &[&alice], cancel_ix(&alice.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &a_src, 0)).is_err(),
        "cancel before the cooldown must be rejected");
    assert_eq!(token_amount(&svm, &bk.coin_escrow), 10_000, "still committed");

    // After 2*round_length slots (no execute cleared the book), the owner may cancel.
    warp_to(&mut svm, 100 + 2 * round_length + 1);
    // A non-owner still cannot cancel it.
    let mallory = Keypair::new(); svm.airdrop(&mallory.pubkey(), 1_000_000_000).unwrap();
    assert!(send(&mut svm, &[&mallory], cancel_ix(&mallory.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &a_src, 0)).is_err(),
        "only the bidder may cancel their own bid");
    send(&mut svm, &[&alice], cancel_ix(&alice.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &a_src, 0)).expect("alice cancels after cooldown");

    assert_eq!(token_amount(&svm, &a_src), 10_000, "escrowed COIN returned");
    assert_eq!(token_amount(&svm, &bk.coin_escrow), 0, "escrow drained");
    assert_eq!(mint_supply(&svm, &env.coin_mint), supply_after_place, "the anti-spam fee stays burned — cancelling still costs it");
    let _ = (alice, a_usd, mallory);
}

// ADVERSARIAL DOS (refund-ATA brick): a losing bidder closes their COIN refund account after
// bidding, so claim cannot deliver the refund and the slot can never free — bricking the whole
// book (it stays SETTLED, execute + place_bid blocked) forever. FIXED by pinning the refund to the
// bidder's CANONICAL ATA: anyone may recreate it, so a stuck claim is always recoverable, not a
// permanent DOS. This test drives the attack end-to-end against the real binaries and proves
// recovery.
#[test]
fn e2e_closing_refund_ata_cannot_permanently_brick_the_book() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(twap_id(), so_deploy("twap_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_handoff(&mut svm, &payer);
    let bk = setup_auction(&mut svm, &payer, &env, 10, 0, None, 0);

    // A winner (rate 1) takes the whole 400k budget; the attacker's tiny-rate bid loses and is
    // owed a full COIN refund.
    let (winner, w_src, w_usd) = new_bidder(&mut svm, &payer, &env, 400_000);
    let (attacker, a_src, a_usd) = new_bidder(&mut svm, &payer, &env, 10);
    send(&mut svm, &[&winner], place_bid_ix(&winner.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &w_src, &w_usd, &env.coin_mint, &env.collateral_mint, 400_000, 400_000, None)).expect("winner bid");
    send(&mut svm, &[&attacker], place_bid_ix(&attacker.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &a_src, &a_usd, &env.coin_mint, &env.collateral_mint, 10, 400_000, None)).expect("attacker bid");

    let cranker = Keypair::new(); svm.airdrop(&cranker.pubkey(), 1_000_000_000).unwrap();
    warp_to(&mut svm, 111);
    send(&mut svm, &[&cranker], execute_ix(&cranker.pubkey(), &env, &bk.book, &bk.holding, &bk.settlement_usd, &bk.book_escrow, &bk.coin_escrow, None)).expect("execute");

    // The winner claims fine.
    send(&mut svm, &[&cranker], claim_ix(&cranker.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.settlement_usd, &bk.coin_escrow, &w_usd, &w_src, 0)).expect("winner claim");

    // ATTACK: the loser CLOSES their refund ATA so claim cannot deliver the 10-COIN refund.
    let close = spl_token::instruction::close_account(&spl_token::ID, &a_src, &attacker.pubkey(), &attacker.pubkey(), &[]).unwrap();
    send(&mut svm, &[&attacker], close).expect("attacker closes refund ata");
    assert!(send(&mut svm, &[&cranker], claim_ix(&cranker.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.settlement_usd, &bk.coin_escrow, &a_usd, &a_src, 1)).is_err(),
        "claim cannot deliver to a closed account (slot temporarily stuck)");
    // While the slot is stuck the book is SETTLED — new bids are blocked.
    let (late, l_src, l_usd) = new_bidder(&mut svm, &payer, &env, 5_000);
    assert!(send(&mut svm, &[&late], place_bid_ix(&late.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &l_src, &l_usd, &env.coin_mint, &env.collateral_mint, 5_000, 5_000, None)).is_err(),
        "book is settled — placing is blocked until it drains");

    // RECOVERY (permissionless): anyone recreates the canonical ATA, then claim succeeds and the
    // book reopens. This is what the canonical-ATA pin buys — no permanent brick.
    set_token(&mut svm, &a_src, &env.coin_mint, &attacker.pubkey(), 0);
    send(&mut svm, &[&cranker], claim_ix(&cranker.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.settlement_usd, &bk.coin_escrow, &a_usd, &a_src, 1)).expect("claim recovers once the ATA exists again");
    assert_eq!(token_amount(&svm, &a_src), 10, "attacker's COIN refund delivered after recreating the ATA");
    // Book reopened: a new bid now works.
    send(&mut svm, &[&late], place_bid_ix(&late.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &l_src, &l_usd, &env.coin_mint, &env.collateral_mint, 5_000, 5_000, None)).expect("book reopened — bidding works again");
    let _ = (winner, attacker, late);
}

// ADVERSARIAL (coin-sink redirection) + COVERAGE (SEND branch was untested): in SEND mode the
// bought COIN is transferred to the DAO-configured treasury instead of burned. A cranker must not
// be able to redirect it to their own account — the sink is pinned to book.coin_sink.
#[test]
fn e2e_send_mode_routes_bought_coin_to_treasury_not_attacker() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(twap_id(), so_deploy("twap_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_handoff(&mut svm, &payer);

    // DAO treasury COIN account = the configured sink.
    let treasury = Pubkey::new_unique();
    set_token(&mut svm, &treasury, &env.coin_mint, &Pubkey::new_unique(), 0);
    let bk = setup_auction(&mut svm, &payer, &env, 10, 1 /* SINK_SEND */, Some(treasury), 0);

    // A bidder sells 400k COIN for the 400k surplus budget (rate 1, fully filled).
    let (alice, a_src, a_usd) = new_bidder(&mut svm, &payer, &env, 400_000);
    send(&mut svm, &[&alice], place_bid_ix(&alice.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &a_src, &a_usd, &env.coin_mint, &env.collateral_mint, 400_000, 400_000, None)).expect("alice bid");

    warp_to(&mut svm, 111);
    let supply_before = mint_supply(&svm, &env.coin_mint);
    let cranker = Keypair::new(); svm.airdrop(&cranker.pubkey(), 1_000_000_000).unwrap();

    // ATTACK: a cranker tries to redirect the bought COIN to their OWN account.
    let rogue_sink = Pubkey::new_unique();
    set_token(&mut svm, &rogue_sink, &env.coin_mint, &cranker.pubkey(), 0);
    assert!(send(&mut svm, &[&cranker], execute_ix(&cranker.pubkey(), &env, &bk.book, &bk.holding, &bk.settlement_usd, &bk.book_escrow, &bk.coin_escrow, Some(rogue_sink))).is_err(),
        "execute must reject a coin_sink other than the configured treasury");
    assert_eq!(token_amount(&svm, &rogue_sink), 0, "no COIN redirected to the attacker");
    assert_eq!(token_amount(&svm, &treasury), 0, "nothing moved yet (rogue execute fully reverted)");

    // Honest execute routes the bought COIN to the configured treasury — NOT burned.
    send(&mut svm, &[&cranker], execute_ix(&cranker.pubkey(), &env, &bk.book, &bk.holding, &bk.settlement_usd, &bk.book_escrow, &bk.coin_escrow, Some(treasury))).expect("execute (send mode)");
    assert_eq!(mint_supply(&svm, &env.coin_mint), supply_before, "SEND mode does not burn — supply unchanged");
    assert_eq!(token_amount(&svm, &treasury), 400_000, "bought COIN routed to the DAO treasury");
    assert_eq!(token_amount(&svm, &bk.settlement_usd), 400_000, "spent USD parked for the winner");
    let _ = (alice, a_usd);
}

// MALICIOUS-DAO SCOPE (shutdown can't drain user funds): shutdown is a privileged Squads-gated op
// that sweeps the twap's USD budget (the holding). It must NOT be repurposable — by substituting
// the book-escrow-owned coin_escrow or settlement_usd as the "holding" — to drain bidders' escrowed
// COIN or winners' settled USD. The holding.owner == twap_authority check scopes it.
#[test]
fn e2e_shutdown_cannot_drain_escrow_or_settlement() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(twap_id(), so_deploy("twap_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_handoff(&mut svm, &payer);
    let bk = setup_auction(&mut svm, &payer, &env, 10, 0, None, 0);

    // A winner takes the budget; a loser leaves a COIN refund in the escrow.
    let (winner, w_src, w_usd) = new_bidder(&mut svm, &payer, &env, 400_000);
    let (loser, l_src, l_usd) = new_bidder(&mut svm, &payer, &env, 10);
    send(&mut svm, &[&winner], place_bid_ix(&winner.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &w_src, &w_usd, &env.coin_mint, &env.collateral_mint, 400_000, 400_000, None)).expect("winner bid");
    send(&mut svm, &[&loser], place_bid_ix(&loser.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &l_src, &l_usd, &env.coin_mint, &env.collateral_mint, 10, 400_000, None)).expect("loser bid");
    let cranker = Keypair::new(); svm.airdrop(&cranker.pubkey(), 1_000_000_000).unwrap();
    warp_to(&mut svm, 111);
    send(&mut svm, &[&cranker], execute_ix(&cranker.pubkey(), &env, &bk.book, &bk.holding, &bk.settlement_usd, &bk.book_escrow, &bk.coin_escrow, None)).expect("execute");
    assert_eq!(token_amount(&svm, &bk.coin_escrow), 10, "loser's refund sits in escrow");
    assert_eq!(token_amount(&svm, &bk.settlement_usd), 400_000, "winner's USD sits in settlement");

    // ATTACK 1: the DAO tries to sweep the COIN escrow via shutdown (holding := coin_escrow).
    let thief_coin = Pubkey::new_unique(); set_token(&mut svm, &thief_coin, &env.coin_mint, &payer.pubkey(), 0);
    let m1 = build_shutdown_message(&env.squads_vault, &env.twap_cfg, &env.twap_authority, &bk.coin_escrow, &thief_coin);
    let r1 = vec![
        AccountMeta::new_readonly(env.squads_vault, false), AccountMeta::new(bk.coin_escrow, false), AccountMeta::new(thief_coin, false),
        AccountMeta::new_readonly(env.twap_cfg, false), AccountMeta::new_readonly(env.twap_authority, false),
        AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(twap_id(), false),
    ];
    assert!(squads_execute(&mut svm, &env.squads, &env.multisig, &env.dao, &payer, 6, &m1, &r1).is_err(),
        "shutdown must reject the book-escrow-owned coin_escrow as 'holding'");
    assert_eq!(token_amount(&svm, &bk.coin_escrow), 10, "bidders' escrowed COIN untouched");
    assert_eq!(token_amount(&svm, &thief_coin), 0, "no COIN stolen");

    // ATTACK 2: the DAO tries to sweep the settled USD via shutdown (holding := settlement_usd).
    let thief_usd = Pubkey::new_unique(); set_token(&mut svm, &thief_usd, &env.collateral_mint, &payer.pubkey(), 0);
    let m2 = build_shutdown_message(&env.squads_vault, &env.twap_cfg, &env.twap_authority, &bk.settlement_usd, &thief_usd);
    let r2 = vec![
        AccountMeta::new_readonly(env.squads_vault, false), AccountMeta::new(bk.settlement_usd, false), AccountMeta::new(thief_usd, false),
        AccountMeta::new_readonly(env.twap_cfg, false), AccountMeta::new_readonly(env.twap_authority, false),
        AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(twap_id(), false),
    ];
    assert!(squads_execute(&mut svm, &env.squads, &env.multisig, &env.dao, &payer, 7, &m2, &r2).is_err(),
        "shutdown must reject the book-escrow-owned settlement_usd as 'holding'");
    assert_eq!(token_amount(&svm, &bk.settlement_usd), 400_000, "winner's settled USD untouched");
    assert_eq!(token_amount(&svm, &thief_usd), 0, "no USD stolen");
    let _ = (winner, loser);
}

// Squads message wrapping twap.set_reserve (tag 6). Accounts: [squads_vault(signer), config, book(w)].
fn build_set_reserve_message(squads_vault: &Pubkey, config: &Pubkey, book: &Pubkey, reserve_num: u128, reserve_den: u128) -> Vec<u8> {
    let mut m = Vec::new();
    m.push(1); m.push(0); m.push(1); m.push(4);
    m.extend_from_slice(squads_vault.as_ref()); // 0 ro signer
    m.extend_from_slice(book.as_ref());          // 1 w
    m.extend_from_slice(config.as_ref());        // 2 ro
    m.extend_from_slice(twap_id().as_ref());     // 3 program
    m.push(1); m.push(3); m.push(3);
    for i in [0u8, 2, 1] { m.push(i); }          // squads_vault, config, book
    let mut data = vec![6u8];
    data.extend_from_slice(&reserve_num.to_le_bytes());
    data.extend_from_slice(&reserve_den.to_le_bytes());
    m.extend_from_slice(&(data.len() as u16).to_le_bytes());
    m.extend_from_slice(&data);
    m.push(0);
    m
}

// ADVERSARIAL (overpay / surplus drain) + COVERAGE (the reserve was never exercised): WITHOUT a
// reserve a hostile bidder can sell 1 COIN for the WHOLE surplus, draining insurance value for ~0
// COIN burned. The DAO-set reserve (min COIN-per-USD) is the guard: execute filters bids below it,
// so an "expensive" bid is never filled and the surplus is preserved — while fair bids still clear.
#[test]
fn e2e_reserve_blocks_expensive_bid_from_draining_surplus() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(twap_id(), so_deploy("twap_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_handoff(&mut svm, &payer);
    let bk = setup_auction(&mut svm, &payer, &env, 10, 0, None, 0);

    // DAO sets a reserve of 1 COIN per 1 USD (a bid must give at least 1 COIN per dollar).
    let rm = build_set_reserve_message(&env.squads_vault, &env.twap_cfg, &bk.book, 1, 1);
    let rr = vec![
        AccountMeta::new_readonly(env.squads_vault, false), AccountMeta::new(bk.book, false),
        AccountMeta::new_readonly(env.twap_cfg, false), AccountMeta::new_readonly(twap_id(), false),
    ];
    squads_execute(&mut svm, &env.squads, &env.multisig, &env.dao, &payer, 6, &rm, &rr).expect("set reserve");

    // ATTACK: a hostile bidder offers just 1 COIN for the entire 400k surplus (rate 1/400000 « 1).
    let (attacker, a_src, a_usd) = new_bidder(&mut svm, &payer, &env, 1);
    send(&mut svm, &[&attacker], place_bid_ix(&attacker.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &a_src, &a_usd, &env.coin_mint, &env.collateral_mint, 1, 400_000, None)).expect("attacker bid");
    let supply_before = mint_supply(&svm, &env.coin_mint);
    let cranker = Keypair::new(); svm.airdrop(&cranker.pubkey(), 1_000_000_000).unwrap();
    warp_to(&mut svm, 111);
    // Execute runs (pulls the surplus into the holding + ratchets), but the below-reserve bid is
    // filtered, so NOTHING is bought/burned and no USD is paid to the attacker — the surplus is
    // preserved, not drained for 1 COIN.
    send(&mut svm, &[&cranker], execute_ix(&cranker.pubkey(), &env, &bk.book, &bk.holding, &bk.settlement_usd, &bk.book_escrow, &bk.coin_escrow, None)).expect("execute (rolls — bid below reserve)");
    assert_eq!(mint_supply(&svm, &env.coin_mint), supply_before, "no COIN burned — the expensive bid was filtered");
    assert_eq!(token_amount(&svm, &bk.settlement_usd), 0, "no USD paid to the attacker — surplus not drained");
    assert_eq!(token_amount(&svm, &bk.coin_escrow), 1, "attacker's COIN still escrowed (bid not filled)");

    // A FAIR bid (>= reserve) still clears against the preserved budget — the reserve isn't over-restrictive.
    let (fair, f_src, f_usd) = new_bidder(&mut svm, &payer, &env, 400_000);
    send(&mut svm, &[&fair], place_bid_ix(&fair.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &f_src, &f_usd, &env.coin_mint, &env.collateral_mint, 400_000, 400_000, None)).expect("fair bid");
    warp_to(&mut svm, 122);
    send(&mut svm, &[&cranker], execute_ix(&cranker.pubkey(), &env, &bk.book, &bk.holding, &bk.settlement_usd, &bk.book_escrow, &bk.coin_escrow, None)).expect("execute (fair bid clears)");
    // Supply = attacker's 1 unsold COIN (still escrowed); the fair bidder's 400k was bought + burned.
    assert_eq!(mint_supply(&svm, &env.coin_mint), 1, "the fair bid's COIN is bought + burned, only the attacker's unsold COIN remains");
    assert_eq!(token_amount(&svm, &bk.settlement_usd), 400_000, "fair bidder paid at the clearing price");
    let _ = (attacker, fair, a_usd, f_usd);
}

// CROSS-MARKET DRAIN: execute is the sole insurance puller. It must be locked to the config's
// market — a cranker must not be able to point the pull at a DIFFERENT market's vault/authority to
// drain that market's insurance into this twap. execute pins market_slab == config.market_slab and
// vault_authority == perc_vault_authority(market_slab). (Lost coverage when the pull tests were
// removed; re-pinned on the execute path.)
#[test]
fn e2e_execute_rejects_foreign_market_vault_authority() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(twap_id(), so_deploy("twap_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_handoff(&mut svm, &payer);
    let bk = setup_auction(&mut svm, &payer, &env, 10, 0, None, 0);
    let (alice, a_src, a_usd) = new_bidder(&mut svm, &payer, &env, 400_000);
    send(&mut svm, &[&alice], place_bid_ix(&alice.pubkey(), &env.twap_cfg, &bk.book, &bk.book_escrow, &bk.coin_escrow, &a_src, &a_usd, &env.coin_mint, &env.collateral_mint, 400_000, 400_000, None)).expect("alice bid");

    warp_to(&mut svm, 111);
    let cranker = Keypair::new(); svm.airdrop(&cranker.pubkey(), 1_000_000_000).unwrap();
    let insurance_before = token_amount(&svm, &env.perc_vault);

    // ATTACK: a foreign market's vault_authority (derived for a DIFFERENT slab).
    let other_slab = Pubkey::new_unique();
    let foreign_vault_authority = perc_vault_authority(&other_slab, &perc_id());
    assert_ne!(foreign_vault_authority, env.vault_authority);
    let rogue = Instruction { program_id: twap_id(), accounts: vec![
        AccountMeta::new(cranker.pubkey(), true), AccountMeta::new(env.twap_cfg, false), AccountMeta::new(bk.book, false),
        AccountMeta::new_readonly(env.twap_authority, false), AccountMeta::new(env.slab, false), AccountMeta::new(env.perc_vault, false),
        AccountMeta::new_readonly(foreign_vault_authority, false), AccountMeta::new_readonly(perc_id(), false),
        AccountMeta::new(bk.holding, false), AccountMeta::new(bk.settlement_usd, false), AccountMeta::new_readonly(bk.book_escrow, false),
        AccountMeta::new(bk.coin_escrow, false), AccountMeta::new(env.coin_mint, false), AccountMeta::new_readonly(spl_token::ID, false),
    ], data: vec![8u8] };
    assert!(send(&mut svm, &[&cranker], rogue).is_err(), "execute must reject a vault_authority not derived from the config's market");
    assert_eq!(token_amount(&svm, &env.perc_vault), insurance_before, "no insurance moved");

    // The honest execute (correct, config-bound vault_authority) works.
    send(&mut svm, &[&cranker], execute_ix(&cranker.pubkey(), &env, &bk.book, &bk.holding, &bk.settlement_usd, &bk.book_escrow, &bk.coin_escrow, None)).expect("honest execute");
    assert!(token_amount(&svm, &env.perc_vault) < insurance_before, "honest execute pulled the burn-share");
    let _ = (alice, a_usd);
}

// BAIT-AND-SWITCH (distribution redirect after approval, LOF): a proposal CREATOR registers a
// distribution, lets voters approve it, then APPENDS a new entry redirecting COIN to themselves
// before trigger. The gv proposal snapshots (entry_count,total_amount) at registration, and trigger
// refuses to seal if the live distribution proposal no longer matches — so the sealed distribution
// is exactly what voters approved, and the redirect can never be sealed/claimed.
#[test]
fn e2e_bait_and_switch_appended_entries_cannot_be_sealed() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(sub_id(), so_deploy("subledger_program")).unwrap();
    svm.add_program_from_file(gv_id_e2e(), so_deploy("genesis_vote_program")).unwrap();
    svm.add_program_from_file(dist_id_e2e(), so_deploy("distribution_program")).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let env = setup_genesis(&mut svm, &payer);

    // The creator registers a community distribution (1 entry, total 100); voters snapshot it.
    let community = Pubkey::new_unique();
    let (dist_proposal, gv_proposal) = register_proposal(&mut svm, &payer, &env, 1, &community, 50); // 50 of 100 supply — leaves room

    // alice deposits + holds for weight + backs the proposal (meets quorum + majority alone).
    let alice = Keypair::new(); svm.airdrop(&alice.pubkey(), 1_000_000_000).unwrap();
    let amount = 1_000_000u64;
    let alice_ata = Pubkey::new_unique(); set_token(&mut svm, &alice_ata, &env.collateral_mint, &alice.pubkey(), amount);
    let holding = Pubkey::new_unique(); set_token(&mut svm, &holding, &env.collateral_mint, &env.pool, 0);
    let position = sub_position_pda(&env.pool, &alice.pubkey());
    let mut dep = vec![4u8]; dep.extend_from_slice(&amount.to_le_bytes());
    let deposit = Instruction { program_id: sub_id(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new(env.pool, false), AccountMeta::new(position, false), AccountMeta::new(alice_ata, false),
        AccountMeta::new(holding, false), AccountMeta::new(env.slab, false), AccountMeta::new(env.perc_vault, false),
        AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(system_program::ID, false)], data: dep };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[deposit], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("deposit");
    let mut c = svm.get_sysvar::<Clock>(); c.slot += 8; svm.set_sysvar::<Clock>(&c);
    let gv_ballot = Pubkey::find_program_address(&[b"gv_ballot", env.gv_config.as_ref(), alice.pubkey().as_ref()], &gv_id_e2e()).0;
    let vote = Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(gv_ballot, false), AccountMeta::new(gv_proposal, false),
        AccountMeta::new(position, false), AccountMeta::new_readonly(env.pool, false), AccountMeta::new_readonly(system_program::ID, false), AccountMeta::new_readonly(sub_id(), false)], data: vec![3u8, 1u8] };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[vote], Some(&payer.pubkey()), &[&payer, &alice], bh)).expect("vote");

    // ATTACK: the creator appends a redirect entry to the SAME proposal AFTER voters approved.
    let attacker_dest = Pubkey::new_unique();
    let mut ad = vec![2u8]; ad.extend_from_slice(&1u32.to_le_bytes()); ad.extend_from_slice(attacker_dest.as_ref()); ad.extend_from_slice(&50u64.to_le_bytes()); // within supply, so only the snapshot guard blocks it
    let append = Instruction { program_id: dist_id_e2e(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(env.dist_config, false), AccountMeta::new(dist_proposal, false)], data: ad };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[append], Some(&payer.pubkey()), &[&payer], bh)).expect("append is allowed pre-seal");

    // trigger now REJECTS — the live proposal no longer matches the approved snapshot.
    let trigger = Instruction { program_id: gv_id_e2e(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(env.gv_config, false), AccountMeta::new(gv_proposal, false),
        AccountMeta::new_readonly(dist_id_e2e(), false), AccountMeta::new(env.dist_config, false), AccountMeta::new(dist_proposal, false),
        AccountMeta::new_readonly(env.pool, false)], data: vec![4u8] };
    svm.expire_blockhash(); let bh = svm.latest_blockhash();
    assert!(svm.send_transaction(Transaction::new_signed_with_payer(&[trigger], Some(&payer.pubkey()), &[&payer], bh)).is_err(),
        "trigger must refuse a distribution proposal changed after voters approved it");
    // The distribution is NOT sealed — the redirect never takes effect.
    let dc = svm.get_account(&env.dist_config).unwrap();
    assert_eq!(Pubkey::new_from_array(dc.data[120..152].try_into().unwrap()), Pubkey::default(), "no winner sealed — bait-and-switch blocked");
    let _ = (community, attacker_dest, alice);
}
