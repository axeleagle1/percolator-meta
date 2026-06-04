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

        // The config PDA binds the authority (anti front-run, finding AA), so derive it after.
        let authority = Keypair::new();
        let config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), authority.pubkey().as_ref()], &pid()).0;
        let vault = create_token_account(&mut svm, &payer, &coin_mint, &config);
        mint_to(&mut svm, &payer, &coin_mint, &mint_authority, &vault, supply);
        // Fixed supply: revoke the mint authority before init (the canonical
        // genesis-setup flow; distribution requires a non-mintable COIN).
        revoke_mint_authority(&mut svm, &payer, &coin_mint, &mint_authority);

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

    /// append_entries signed by an arbitrary keypair (not self.payer) — to prove the
    /// proposal's creator-binding refuses a foreign appender.
    fn append_as(&mut self, proposal: &Pubkey, signer: &Keypair, entries: &[(Pubkey, u64)]) -> Result<(), String> {
        let mut data = vec![2u8]; // IX_APPEND_ENTRIES
        data.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (pk, amt) in entries {
            data.extend_from_slice(pk.as_ref());
            data.extend_from_slice(&amt.to_le_bytes());
        }
        let ix = Instruction {
            program_id: pid(),
            accounts: vec![
                AccountMeta::new(signer.pubkey(), true), // creator slot = the attacker
                AccountMeta::new_readonly(self.config, false),
                AccountMeta::new(*proposal, false),
            ],
            data,
        };
        let s = clone_kp(signer);
        self.send(&[ix], &[&s])
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

fn revoke_mint_authority(svm: &mut LiteSVM, payer: &Keypair, mint: &Pubkey, authority: &Keypair) {
    let ix = spl_token::instruction::set_authority(
        &spl_token::ID,
        mint,
        None,
        spl_token::instruction::AuthorityType::MintTokens,
        &authority.pubkey(),
        &[],
    )
    .unwrap();
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[payer, authority], svm.latest_blockhash());
    svm.send_transaction(tx).unwrap();
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

// LOF boundary: competing proposals share ONE config and ONE funded vault (the genesis
// votes among several candidate COIN distributions, only one wins + is sealed). A losing
// proposal's recipients must NEVER be able to pull from that vault. claim binds
// `config.sealed_proposal == proposal.key` (lib.rs:518). Build the worst case: a self-dealing
// LOSING proposal that allocates the ENTIRE supply to the attacker, then prove its claim is
// refused after a different proposal wins — and the winner's vault is fully intact.
#[test]
fn a_losing_proposal_cannot_claim_the_winners_vault() {
    let mut env = Env::new(100, 1_000_000);

    // Winner: proposal 1, all 100 to alice.
    let winner = env.create_proposal(1, 4);
    let (alice, alice_ata) = env.new_recipient();
    env.append(&winner, &[(alice.pubkey(), 100)]).expect("seed winner");

    // Loser: proposal 2 under the same config/vault, a self-dealing full-supply grab.
    let loser = env.create_proposal(2, 4);
    let (mallory, mallory_ata) = env.new_recipient();
    env.append(&loser, &[(mallory.pubkey(), 100)]).expect("seed loser");

    // The vote/trigger seals the winner.
    let auth = clone_kp(&env.authority);
    env.seal(&winner, &auth).expect("seal winner");

    // Mallory's losing proposal cannot pay out of the shared vault.
    assert!(
        env.claim(&loser, &mallory, &mallory_ata, 0).is_err(),
        "a losing proposal must not claim the shared vault"
    );
    // Re-sealing the loser to redirect the vault is also refused (config already sealed).
    assert!(env.seal(&loser, &auth).is_err(), "cannot reseal to the loser");
    assert_eq!(env.token_amount(&mallory_ata), 0, "attacker got nothing");

    // The legitimate winner still claims the full, untouched vault.
    env.claim(&winner, &alice, &alice_ata, 0).expect("winner claim");
    assert_eq!(env.token_amount(&alice_ata), 100);
    assert_eq!(env.token_amount(&env.vault.clone()), 0, "vault went only to the winner");
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

// Anti-griefing: burn_unclaimed must be rejected while the claim window is still
// open. Otherwise anyone could permissionlessly burn the pool mid-window and
// destroy claimants' COIN before they get a chance to claim it (a DOS/LOF on every
// recipient who hasn't claimed yet).
#[test]
fn burn_unclaimed_is_rejected_during_the_claim_window() {
    let mut env = Env::new(100, 50); // window = 50 slots
    env.set_slot(10);
    let proposal = env.create_proposal(1, 4);
    let (alice, alice_ata) = env.new_recipient();
    let (bob, bob_ata) = env.new_recipient();
    env.append(&proposal, &[(alice.pubkey(), 60), (bob.pubkey(), 40)]).expect("append");
    let auth = clone_kp(&env.authority);
    env.seal(&proposal, &auth).expect("seal"); // seal_slot = 10, window ends at 60

    // A griefer tries to burn the unclaimed pool mid-window — must be rejected.
    env.set_slot(30);
    assert!(env.burn_unclaimed().is_err(), "cannot burn while the claim window is open");
    // Even one slot before close (window_end - 1) is still too early.
    env.set_slot(59);
    assert!(env.burn_unclaimed().is_err(), "still within the window at window_end - 1");

    // The funds were never touched: both recipients can still claim in full.
    env.claim(&proposal, &alice, &alice_ata, 0).expect("alice claims");
    env.claim(&proposal, &bob, &bob_ata, 1).expect("bob claims");
    assert_eq!(env.token_amount(&alice_ata), 60);
    assert_eq!(env.token_amount(&bob_ata), 40);
    assert_eq!(env.token_amount(&env.vault.clone()), 0, "fully claimed");

    // At/after window_end the burn is permitted (here a no-op: nothing unclaimed).
    env.set_slot(60);
    env.burn_unclaimed().expect("burn allowed once the window has closed");
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

// LOF boundary: append_entries binds to the proposal's recorded creator (lib.rs:417,
// `header.creator != *creator.key`). A proposal is a candidate COIN distribution list; if
// its winner is sealed, its entries become directly claimable. So a hostile actor who could
// append to *someone else's* in-flight proposal would inject a self-dealing entry into the
// unallocated headroom (below total_supply, so the cap check at :442 wouldn't catch it) and
// claim that COIN the moment the honest proposal won the vote. Confirm the creator-binding
// blocks a foreign appender, while the real creator can still extend its own proposal.
#[test]
fn append_entries_rejects_a_foreign_creator() {
    let mut env = Env::new(100, 1_000_000);
    // Honest creator = env.payer; proposal allocates 40 of 100, leaving 60 headroom.
    let proposal = env.create_proposal(1, 8);
    let (honest, _) = env.new_recipient();
    env.append(&proposal, &[(honest.pubkey(), 40)]).expect("creator seeds its own proposal");

    // Attacker signs, takes the creator slot, tries to inject a self-entry into the 60 headroom.
    let attacker = Keypair::new();
    env.svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    assert!(
        env.append_as(&proposal, &attacker, &[(attacker.pubkey(), 60)]).is_err(),
        "a non-creator must not be able to append entries to someone else's proposal"
    );

    // The genuine creator can still append into the same headroom — the binding is to the
    // creator, not a freeze.
    env.append(&proposal, &[(honest.pubkey(), 60)]).expect("the real creator extends its proposal");
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

    let authority = Keypair::new();
    let config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), authority.pubkey().as_ref()], &pid()).0;
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &config);
    // Fund the vault with only 60, but promise a total_supply of 100.
    mint_to(&mut svm, &payer, &coin_mint, &mint_authority, &vault, 60);
    revoke_mint_authority(&mut svm, &payer, &coin_mint, &mint_authority);

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

// Fixed-supply invariant (README Safety §4): a COIN whose mint authority is NOT
// revoked must be refused — otherwise the authority holder could mint unlimited COIN
// outside the fixed distribution pool and dilute every recipient (no "mint to drain").
#[test]
fn init_config_rejects_a_mintable_coin() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(pid(), so_path()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let mint_authority = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_authority.pubkey());
    let auth = Keypair::new().pubkey();
    let config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), auth.as_ref()], &pid()).0;
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_authority, &vault, 100);

    let build = || {
        let mut data = vec![0u8]; // IX_INIT_CONFIG
        data.extend_from_slice(&1_000_000u64.to_le_bytes());
        data.extend_from_slice(&100u64.to_le_bytes());
        Instruction {
            program_id: pid(),
            accounts: vec![
                AccountMeta::new(payer.pubkey(), true),
                AccountMeta::new_readonly(coin_mint, false),
                AccountMeta::new(config, false),
                AccountMeta::new_readonly(vault, false),
                AccountMeta::new_readonly(auth, false),
                AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
            ],
            data,
        }
    };
    let send = |svm: &mut LiteSVM, ix: Instruction| -> Result<(), String> {
        svm.expire_blockhash(); // distinct blockhash so the retried ix isn't a dup
        let bh = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], bh);
        svm.send_transaction(tx).map(|_| ()).map_err(|e| format!("{:?}", e))
    };

    // Mint authority still live: rejected.
    assert!(send(&mut svm, build()).is_err(), "must reject a still-mintable COIN");

    // Pre-mint extra COIN to the attacker (supply 150 > distributed pool 100), THEN
    // revoke. Must still be rejected: undistributed COIN outside the pool would
    // dominate governance.
    let attacker_ata = create_token_account(&mut svm, &payer, &coin_mint, &Pubkey::new_unique());
    mint_to(&mut svm, &payer, &coin_mint, &mint_authority, &attacker_ata, 50);
    revoke_mint_authority(&mut svm, &payer, &coin_mint, &mint_authority);
    assert!(send(&mut svm, build()).is_err(), "must reject a COIN with supply > the distributed pool");
}

// Once the entire COIN supply is the distribution pool (and mint authority revoked),
// the config is accepted — proving every COIN that exists is in this vault.
#[test]
fn init_config_accepts_a_fully_in_vault_fixed_supply_coin() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(pid(), so_path()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let mint_authority = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_authority.pubkey());
    let authority = Pubkey::new_unique();
    let config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), authority.as_ref()], &pid()).0;
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_authority, &vault, 100); // entire supply -> vault
    revoke_mint_authority(&mut svm, &payer, &coin_mint, &mint_authority);
    let mut data = vec![0u8];
    data.extend_from_slice(&1_000_000u64.to_le_bytes());
    data.extend_from_slice(&100u64.to_le_bytes());
    let ix = Instruction {
        program_id: pid(),
        accounts: vec![
            AccountMeta::new(payer.pubkey(), true),
            AccountMeta::new_readonly(coin_mint, false),
            AccountMeta::new(config, false),
            AccountMeta::new_readonly(vault, false),
            AccountMeta::new_readonly(authority, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data,
    };
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], svm.latest_blockhash());
    svm.send_transaction(tx).expect("entire-supply-in-vault COIN accepted");
}

// ADVERSARIAL (init front-run theft, finding AA): the config PDA binds the AUTHORITY, so an
// attacker cannot init the per-mint config with authority=themselves over the deployer's
// already-funded vault (owned by the LEGIT config PDA). Their authority derives a DIFFERENT PDA
// that does not own the vault, so init is rejected and the funded supply is untouchable.
#[test]
fn init_config_authority_bound_blocks_funded_vault_hijack() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(pid(), so_path()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let mint_authority = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_authority.pubkey());

    // Deployer's legit config + funded vault (vault owned by the LEGIT, authority-bound config PDA).
    let legit_authority = Pubkey::new_unique();
    let legit_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), legit_authority.as_ref()], &pid()).0;
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &legit_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_authority, &vault, 100);
    revoke_mint_authority(&mut svm, &payer, &coin_mint, &mint_authority);

    let init = |config: Pubkey, authority: Pubkey| {
        let mut data = vec![0u8];
        data.extend_from_slice(&1_000_000u64.to_le_bytes());
        data.extend_from_slice(&100u64.to_le_bytes());
        Instruction { program_id: pid(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new(config, false),
            AccountMeta::new_readonly(vault, false), AccountMeta::new_readonly(authority, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data }
    };

    // ATTACK: front-run init with authority=attacker pointed at the legit funded vault.
    let attacker = Keypair::new();
    let attacker_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), attacker.pubkey().as_ref()], &pid()).0;
    assert_ne!(attacker_config, legit_config);
    let bh = svm.latest_blockhash();
    assert!(svm.send_transaction(Transaction::new_signed_with_payer(&[init(attacker_config, attacker.pubkey())], Some(&payer.pubkey()), &[&payer], bh)).is_err(),
        "attacker's authority cannot init over the legit funded vault (different PDA, not the vault owner)");
    let v = svm.get_account(&vault).unwrap();
    assert_eq!(u64::from_le_bytes(v.data[64..72].try_into().unwrap()), 100, "funded supply untouched");

    // The legit deployer (authority over its OWN PDA's vault) inits fine.
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[init(legit_config, legit_authority)], Some(&payer.pubkey()), &[&payer], bh)).expect("legit init succeeds");
}
