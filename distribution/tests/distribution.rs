//! End-to-end litesvm tests: fixed COIN vault, proposal list, authority-gated
//! seal, permissionless per-recipient claim (pull, indexed), and burn-unclaimed
//! after the claim window.

use litesvm::LiteSVM;
use solana_sdk::{
    clock::Clock,
    instruction::{AccountMeta, Instruction},
    program_pack::Pack,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};

fn pid() -> Pubkey {
    distribution_program::id()
}
fn so_path() -> String {
    format!("{}/../target/deploy/distribution_program.so", env!("CARGO_MANIFEST_DIR"))
}
fn clone_kp(kp: &Keypair) -> Keypair {
    Keypair::from_bytes(&kp.to_bytes()).unwrap()
}

struct Env {
    svm: LiteSVM,
    payer: Keypair,
    coin_mint: Pubkey,
    mint_authority: Keypair,
    config: Pubkey,
    vault: Pubkey,
    authority: Keypair,
}

impl Env {
    /// Sets up a COIN mint, a config PDA + vault holding `supply` COIN, and runs
    /// InitConfig with the given claim window.
    fn new(supply: u64, claim_window: u64) -> Self {
        let mut svm = LiteSVM::new();
        svm.add_program_from_file(pid(), so_path()).unwrap();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
        let mint_authority = Keypair::new();
        let coin_mint = create_mint(&mut svm, &payer, &mint_authority.pubkey());

        let config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref()], &pid()).0;
        let vault = create_token_account(&mut svm, &payer, &coin_mint, &config);
        mint_to(&mut svm, &payer, &coin_mint, &mint_authority, &vault, supply);

        let authority = Keypair::new();
        let mut env = Env { svm, payer, coin_mint, mint_authority, config, vault, authority };

        let mut data = vec![0u8]; // IX_INIT_CONFIG
        data.extend_from_slice(&claim_window.to_le_bytes());
        data.extend_from_slice(&supply.to_le_bytes());
        let auth = env.authority.pubkey();
        let ix = Instruction {
            program_id: pid(),
            accounts: vec![
                AccountMeta::new(env.payer.pubkey(), true),
                AccountMeta::new_readonly(env.coin_mint, false),
                AccountMeta::new(env.config, false),
                AccountMeta::new_readonly(env.vault, false),
                AccountMeta::new_readonly(auth, false),
                AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
            ],
            data,
        };
        env.send(&[ix], &[]).expect("init config");
        env
    }

    fn send(&mut self, ixs: &[Instruction], extra: &[&Keypair]) -> Result<(), String> {
        self.svm.expire_blockhash();
        let bh = self.svm.latest_blockhash();
        let payer = clone_kp(&self.payer);
        let mut signers: Vec<&Keypair> = Vec::with_capacity(1 + extra.len());
        signers.push(&payer);
        signers.extend_from_slice(extra);
        let payer_pubkey = self.payer.pubkey();
        let tx = Transaction::new_signed_with_payer(ixs, Some(&payer_pubkey), &signers, bh);
        self.svm.send_transaction(tx).map(|_| ()).map_err(|e| format!("{:?}", e))
    }

    fn token_amount(&self, account: &Pubkey) -> u64 {
        spl_token::state::Account::unpack(&self.svm.get_account(account).unwrap().data).unwrap().amount
    }

    fn set_slot(&mut self, slot: u64) {
        self.svm.set_sysvar(&Clock { slot, ..Default::default() });
    }

    fn proposal_pda(&self, id: u64) -> Pubkey {
        Pubkey::find_program_address(
            &[b"dist_proposal", self.config.as_ref(), &id.to_le_bytes()],
            &pid(),
        )
        .0
    }

    fn create_proposal(&mut self, id: u64, capacity: u32) -> Pubkey {
        let proposal = self.proposal_pda(id);
        let mut data = vec![1u8]; // IX_CREATE_PROPOSAL
        data.extend_from_slice(&id.to_le_bytes());
        data.extend_from_slice(&capacity.to_le_bytes());
        let ix = Instruction {
            program_id: pid(),
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(self.config, false),
                AccountMeta::new(proposal, false),
                AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
            ],
            data,
        };
        self.send(&[ix], &[]).expect("create proposal");
        proposal
    }

    fn append(&mut self, proposal: &Pubkey, entries: &[(Pubkey, u64)]) -> Result<(), String> {
        let mut data = vec![2u8]; // IX_APPEND_ENTRIES
        data.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (pk, amt) in entries {
            data.extend_from_slice(pk.as_ref());
            data.extend_from_slice(&amt.to_le_bytes());
        }
        let ix = Instruction {
            program_id: pid(),
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(self.config, false),
                AccountMeta::new(*proposal, false),
            ],
            data,
        };
        self.send(&[ix], &[])
    }

    fn seal(&mut self, proposal: &Pubkey, signer: &Keypair) -> Result<(), String> {
        let ix = Instruction {
            program_id: pid(),
            accounts: vec![
                AccountMeta::new_readonly(signer.pubkey(), true),
                AccountMeta::new(self.config, false),
                AccountMeta::new(*proposal, false),
            ],
            data: vec![3u8], // IX_SEAL_WINNER
        };
        let s = clone_kp(signer);
        self.send(&[ix], &[&s])
    }

    fn claim(&mut self, proposal: &Pubkey, recipient: &Keypair, ata: &Pubkey, index: u32) -> Result<(), String> {
        let mut data = vec![4u8]; // IX_CLAIM
        data.extend_from_slice(&index.to_le_bytes());
        let ix = Instruction {
            program_id: pid(),
            accounts: vec![
                AccountMeta::new_readonly(recipient.pubkey(), true),
                AccountMeta::new_readonly(self.config, false),
                AccountMeta::new(*proposal, false),
                AccountMeta::new(self.vault, false),
                AccountMeta::new(*ata, false),
                AccountMeta::new_readonly(spl_token::ID, false),
            ],
            data,
        };
        let r = clone_kp(recipient);
        self.send(&[ix], &[&r])
    }

    fn burn_unclaimed(&mut self) -> Result<(), String> {
        let ix = Instruction {
            program_id: pid(),
            accounts: vec![
                AccountMeta::new_readonly(self.payer.pubkey(), true),
                AccountMeta::new_readonly(self.config, false),
                AccountMeta::new(self.vault, false),
                AccountMeta::new(self.coin_mint, false),
                AccountMeta::new_readonly(spl_token::ID, false),
            ],
            data: vec![5u8], // IX_BURN_UNCLAIMED
        };
        self.send(&[ix], &[])
    }

    fn new_recipient(&mut self) -> (Keypair, Pubkey) {
        let kp = Keypair::new();
        self.svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
        let payer = clone_kp(&self.payer);
        let mint = self.coin_mint;
        let ata = create_token_account(&mut self.svm, &payer, &mint, &kp.pubkey());
        (kp, ata)
    }
}

fn create_mint(svm: &mut LiteSVM, payer: &Keypair, authority: &Pubkey) -> Pubkey {
    let mint = Keypair::new();
    let rent = svm.minimum_balance_for_rent_exemption(spl_token::state::Mint::LEN);
    let ixs = [
        system_instruction::create_account(&payer.pubkey(), &mint.pubkey(), rent, spl_token::state::Mint::LEN as u64, &spl_token::ID),
        spl_token::instruction::initialize_mint(&spl_token::ID, &mint.pubkey(), authority, None, 6).unwrap(),
    ];
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[payer, &mint], svm.latest_blockhash());
    svm.send_transaction(tx).unwrap();
    mint.pubkey()
}

fn create_token_account(svm: &mut LiteSVM, payer: &Keypair, mint: &Pubkey, owner: &Pubkey) -> Pubkey {
    let acc = Keypair::new();
    let rent = svm.minimum_balance_for_rent_exemption(spl_token::state::Account::LEN);
    let ixs = [
        system_instruction::create_account(&payer.pubkey(), &acc.pubkey(), rent, spl_token::state::Account::LEN as u64, &spl_token::ID),
        spl_token::instruction::initialize_account(&spl_token::ID, &acc.pubkey(), mint, owner).unwrap(),
    ];
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[payer, &acc], svm.latest_blockhash());
    svm.send_transaction(tx).unwrap();
    acc.pubkey()
}

fn mint_to(svm: &mut LiteSVM, payer: &Keypair, mint: &Pubkey, authority: &Keypair, dest: &Pubkey, amount: u64) {
    let ix = spl_token::instruction::mint_to(&spl_token::ID, mint, dest, &authority.pubkey(), &[], amount).unwrap();
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[payer, authority], svm.latest_blockhash());
    svm.send_transaction(tx).unwrap();
}

#[test]
fn seal_then_recipients_claim_their_entries() {
    let mut env = Env::new(100, 1_000_000);
    let proposal = env.create_proposal(1, 4);
    let (alice, alice_ata) = env.new_recipient();
    let (bob, bob_ata) = env.new_recipient();
    env.append(&proposal, &[(alice.pubkey(), 60), (bob.pubkey(), 40)]).expect("append");

    // Claims are blocked until the winner is sealed.
    assert!(env.claim(&proposal, &alice, &alice_ata, 0).is_err(), "no claim before seal");

    // Only the configured authority (the vote/trigger) can seal.
    let imposter = Keypair::new();
    env.svm.airdrop(&imposter.pubkey(), 1_000_000_000).unwrap();
    assert!(env.seal(&proposal, &imposter).is_err(), "non-authority cannot seal");
    let auth = clone_kp(&env.authority);
    env.seal(&proposal, &auth).expect("seal by authority");

    // Each recipient pulls their own entry by index.
    env.claim(&proposal, &alice, &alice_ata, 0).expect("alice claim");
    env.claim(&proposal, &bob, &bob_ata, 1).expect("bob claim");
    assert_eq!(env.token_amount(&alice_ata), 60);
    assert_eq!(env.token_amount(&bob_ata), 40);
    assert_eq!(env.token_amount(&env.vault.clone()), 0, "vault fully distributed");

    // Double-claim and wrong-recipient claim are rejected.
    assert!(env.claim(&proposal, &alice, &alice_ata, 0).is_err(), "no double claim");
    let attacker_ata = {
        let (_, ata) = env.new_recipient();
        ata
    };
    assert!(env.claim(&proposal, &alice, &attacker_ata, 1).is_err(), "cannot claim bob's entry");
}

#[test]
fn unclaimed_is_burned_after_window() {
    let mut env = Env::new(100, 50); // window = 50 slots
    env.set_slot(10);
    let proposal = env.create_proposal(1, 4);
    let (alice, alice_ata) = env.new_recipient();
    let (bob, _bob_ata) = env.new_recipient();
    env.append(&proposal, &[(alice.pubkey(), 60), (bob.pubkey(), 40)]).expect("append");

    let auth = clone_kp(&env.authority);
    env.seal(&proposal, &auth).expect("seal"); // seal_slot = 10, window ends at 60

    // Alice claims; bob never does.
    env.claim(&proposal, &alice, &alice_ata, 0).expect("alice claim");
    assert_eq!(env.token_amount(&alice_ata), 60);

    // Past the window: claims rejected, unclaimed (bob's 40) is permissionlessly burned.
    env.set_slot(60);
    assert!(env.claim(&proposal, &bob, &alice_ata, 1).is_err(), "window closed");

    let mint_before = spl_token::state::Mint::unpack(&env.svm.get_account(&env.coin_mint).unwrap().data).unwrap().supply;
    env.burn_unclaimed().expect("burn unclaimed");
    assert_eq!(env.token_amount(&env.vault.clone()), 0, "vault emptied");
    let mint_after = spl_token::state::Mint::unpack(&env.svm.get_account(&env.coin_mint).unwrap().data).unwrap().supply;
    assert_eq!(mint_before - mint_after, 40, "unclaimed 40 burned from supply");
}

#[test]
fn append_cannot_exceed_total_supply() {
    let mut env = Env::new(100, 1_000_000);
    let proposal = env.create_proposal(1, 4);
    let (alice, _) = env.new_recipient();
    let (bob, _) = env.new_recipient();
    // 60 + 50 = 110 > total_supply 100 -> rejected.
    assert!(
        env.append(&proposal, &[(alice.pubkey(), 60), (bob.pubkey(), 50)]).is_err(),
        "cannot allocate more than the fixed supply"
    );
}

// Solvency invariant: InitConfig must reject a vault that holds less than the
// promised total_supply. Otherwise a config could promise 100 COIN while the vault
// holds only 60 — the seal (total_amount <= total_supply) would pass, then early
// claimants drain the 60 and honest late claimants are stranded (claim-race LOF).
// This pins that the promised supply is backed by real tokens at init time.
#[test]
fn init_config_rejects_an_underfunded_vault() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(pid(), so_path()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let mint_authority = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_authority.pubkey());

    let config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref()], &pid()).0;
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &config);
    // Fund the vault with only 60, but promise a total_supply of 100.
    mint_to(&mut svm, &payer, &coin_mint, &mint_authority, &vault, 60);

    let authority = Keypair::new();
    let build_init = |total_supply: u64| {
        let mut data = vec![0u8]; // IX_INIT_CONFIG
        data.extend_from_slice(&1_000_000u64.to_le_bytes()); // claim window
        data.extend_from_slice(&total_supply.to_le_bytes());
        Instruction {
            program_id: pid(),
            accounts: vec![
                AccountMeta::new(payer.pubkey(), true),
                AccountMeta::new_readonly(coin_mint, false),
                AccountMeta::new(config, false),
                AccountMeta::new_readonly(vault, false),
                AccountMeta::new_readonly(authority.pubkey(), false),
                AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
            ],
            data,
        }
    };
    let send = |svm: &mut LiteSVM, ix: Instruction| -> Result<(), String> {
        let bh = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], bh);
        svm.send_transaction(tx).map(|_| ()).map_err(|e| format!("{:?}", e))
    };

    // Underfunded (vault 60 < promised 100): rejected.
    assert!(send(&mut svm, build_init(100)).is_err(), "underfunded vault must be rejected");
    // Promising exactly what the vault holds (60) succeeds — supply is tied to real tokens.
    send(&mut svm, build_init(60)).expect("fully-backed supply is accepted");
}
