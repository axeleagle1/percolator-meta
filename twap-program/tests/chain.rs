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

    // Squads -> TWAP gating: reconfigure is restricted to the multisig's default
    // vault PDA (the executor of a multisig vault-transaction, reachable only after a
    // DAO proposal clears the timelock). A random signer must be rejected.
    let cfg_pda = twap_config_pda(&market);
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
    let cfg_pda = twap_config_pda(&market);
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
