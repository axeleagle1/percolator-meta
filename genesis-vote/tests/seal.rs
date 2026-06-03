//! Cross-program integration: the genesis-vote trigger seals a distribution
//! proposal by CPI (the genesis-vote config PDA is the distribution program's
//! seal authority). The Percolator-backed deposit/vote path is exercised in the
//! chain integration; here we inject a winning tally directly and prove the seal.

use litesvm::LiteSVM;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    program_pack::Pack,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};

fn gv_id() -> Pubkey {
    genesis_vote_program::id()
}
fn dist_id() -> Pubkey {
    distribution_program::id()
}
fn so(name: &str) -> String {
    format!("{}/../target/deploy/{}.so", env!("CARGO_MANIFEST_DIR"), name)
}
fn clone_kp(kp: &Keypair) -> Keypair {
    Keypair::from_bytes(&kp.to_bytes()).unwrap()
}

struct Env {
    svm: LiteSVM,
    payer: Keypair,
    coin_mint: Pubkey,
    mint_auth: Keypair,
    gv_config: Pubkey,
    dist_config: Pubkey,
    vault: Pubkey,
    sub_pid: Pubkey,
    sub_pool: Pubkey,
}

impl Env {
    fn new() -> Self {
        let mut svm = LiteSVM::new();
        svm.add_program_from_file(gv_id(), so("genesis_vote_program")).unwrap();
        svm.add_program_from_file(dist_id(), so("distribution_program")).unwrap();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
        let mint_auth = Keypair::new();
        let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());

        let gv_config = Pubkey::find_program_address(&[b"gv_config", coin_mint.as_ref()], &gv_id()).0;
        let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref()], &dist_id()).0;
        let vault = create_token_account(&mut svm, &payer, &coin_mint, &dist_config);
        mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &vault, 100);

        // Stand-in subledger program id + insurance pool; the genesis-vote config
        // pins these and the trigger re-reads the pool's outstanding live.
        let sub_pid = Pubkey::new_from_array([7u8; 32]);
        let sub_pool = Pubkey::new_from_array([8u8; 32]);

        let mut env = Env { svm, payer, coin_mint, mint_auth, gv_config, dist_config, vault, sub_pid, sub_pool };
        env.set_pool_outstanding(0);
        env.init_distribution();
        env.init_gv();
        env
    }

    /// Write a fake subledger insurance pool account (owned by the stand-in
    /// subledger program) with the given `outstanding_principal` at offset 80..88
    /// and the SUBPOOL1 discriminator. This is what the trigger reads live.
    fn set_pool_outstanding(&mut self, outstanding: u64) {
        let mut data = vec![0u8; 160];
        data[..8].copy_from_slice(b"SUBPOOL1");
        data[80..88].copy_from_slice(&outstanding.to_le_bytes());
        let acc = solana_sdk::account::Account {
            lamports: 1_000_000,
            data,
            owner: self.sub_pid,
            executable: false,
            rent_epoch: 0,
        };
        self.svm.set_account(self.sub_pool, acc).unwrap();
    }

    fn send(&mut self, ixs: &[Instruction], extra: &[&Keypair]) -> Result<(), String> {
        self.svm.expire_blockhash();
        let bh = self.svm.latest_blockhash();
        let payer = clone_kp(&self.payer);
        let mut signers: Vec<&Keypair> = vec![&payer];
        signers.extend_from_slice(extra);
        let pk = self.payer.pubkey();
        let tx = Transaction::new_signed_with_payer(ixs, Some(&pk), &signers, bh);
        self.svm.send_transaction(tx).map(|_| ()).map_err(|e| format!("{:?}", e))
    }

    // distribution InitConfig with authority = the genesis-vote config PDA.
    fn init_distribution(&mut self) {
        let mut data = vec![0u8];
        data.extend_from_slice(&1_000_000u64.to_le_bytes()); // claim window
        data.extend_from_slice(&100u64.to_le_bytes()); // total supply
        let ix = Instruction {
            program_id: dist_id(),
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(self.coin_mint, false),
                AccountMeta::new(self.dist_config, false),
                AccountMeta::new_readonly(self.vault, false),
                AccountMeta::new_readonly(self.gv_config, false), // authority = gv config PDA
                AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
            ],
            data,
        };
        self.send(&[ix], &[]).expect("dist init");
    }

    fn dist_proposal(&self, id: u64) -> Pubkey {
        Pubkey::find_program_address(&[b"dist_proposal", self.dist_config.as_ref(), &id.to_le_bytes()], &dist_id()).0
    }

    fn create_dist_proposal(&mut self, id: u64, entries: &[(Pubkey, u64)]) -> Pubkey {
        let proposal = self.dist_proposal(id);
        let mut data = vec![1u8];
        data.extend_from_slice(&id.to_le_bytes());
        data.extend_from_slice(&4u32.to_le_bytes()); // capacity
        let create = Instruction {
            program_id: dist_id(),
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(self.dist_config, false),
                AccountMeta::new(proposal, false),
                AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
            ],
            data,
        };
        self.send(&[create], &[]).expect("create proposal");
        let mut ad = vec![2u8];
        ad.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (pk, amt) in entries {
            ad.extend_from_slice(pk.as_ref());
            ad.extend_from_slice(&amt.to_le_bytes());
        }
        let append = Instruction {
            program_id: dist_id(),
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(self.dist_config, false),
                AccountMeta::new(proposal, false),
            ],
            data: ad,
        };
        self.send(&[append], &[]).expect("append");
        proposal
    }

    fn init_gv(&mut self) {
        let dummy = Pubkey::new_unique();
        let ix = Instruction {
            program_id: gv_id(),
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(self.coin_mint, false),
                AccountMeta::new(self.gv_config, false),
                AccountMeta::new_readonly(dist_id(), false),
                AccountMeta::new_readonly(self.dist_config, false),
                AccountMeta::new_readonly(self.sub_pid, false),  // subledger_program
                AccountMeta::new_readonly(self.sub_pool, false), // subledger_pool
                AccountMeta::new_readonly(dummy, false),         // _reserved
                AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
            ],
            data: vec![0u8],
        };
        self.send(&[ix], &[]).expect("gv init");
    }

    fn gv_proposal_pda(&self, dist_proposal: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[b"gv_proposal", self.gv_config.as_ref(), dist_proposal.as_ref()], &gv_id()).0
    }

    fn register(&mut self, dist_proposal: &Pubkey) -> Pubkey {
        let gv_proposal = self.gv_proposal_pda(dist_proposal);
        let ix = Instruction {
            program_id: gv_id(),
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(self.gv_config, false),
                AccountMeta::new(gv_proposal, false),
                AccountMeta::new_readonly(*dist_proposal, false),
                AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
            ],
            data: vec![2u8],
        };
        self.send(&[ix], &[]).expect("register");
        gv_proposal
    }

    /// Inject a winning tally directly (the Percolator-backed deposit/vote path is
    /// tested in the chain integration). Sets gv config global tallies and the
    /// gv proposal-vote support.
    fn inject_tally(&mut self, gv_proposal: &Pubkey, voted_principal: u64, cast_weight: u64, outstanding: u64, support_weight: u64, support_principal: u64) {
        let mut cfg = self.svm.get_account(&self.gv_config).unwrap();
        cfg.data[200..208].copy_from_slice(&voted_principal.to_le_bytes());
        cfg.data[208..216].copy_from_slice(&cast_weight.to_le_bytes());
        cfg.data[216..224].copy_from_slice(&outstanding.to_le_bytes());
        self.svm.set_account(self.gv_config, cfg).unwrap();

        let mut pv = self.svm.get_account(gv_proposal).unwrap();
        pv.data[72..80].copy_from_slice(&support_weight.to_le_bytes());
        pv.data[80..88].copy_from_slice(&support_principal.to_le_bytes());
        self.svm.set_account(*gv_proposal, pv).unwrap();
    }

    fn trigger(&mut self, gv_proposal: &Pubkey, dist_proposal: &Pubkey) -> Result<(), String> {
        let ix = Instruction {
            program_id: gv_id(),
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new(self.gv_config, false),
                AccountMeta::new(*gv_proposal, false),
                AccountMeta::new_readonly(dist_id(), false),
                AccountMeta::new(self.dist_config, false),
                AccountMeta::new(*dist_proposal, false),
                AccountMeta::new_readonly(self.sub_pool, false), // live quorum denominator
            ],
            data: vec![4u8],
        };
        self.send(&[ix], &[])
    }

    fn dist_sealed_proposal(&self) -> Pubkey {
        let cfg = self.svm.get_account(&self.dist_config).unwrap();
        Pubkey::new_from_array(cfg.data[120..152].try_into().unwrap())
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
fn trigger_seals_the_distribution_cross_program() {
    let mut env = Env::new();
    let alice = Pubkey::new_unique();
    let bob = Pubkey::new_unique();
    let dist_proposal = env.create_dist_proposal(1, &[(alice, 60), (bob, 40)]);
    let gv_proposal = env.register(&dist_proposal);
    env.set_pool_outstanding(10); // live quorum denominator

    // Below quorum/majority: trigger is rejected.
    env.inject_tally(&gv_proposal, 4, 8, 10, 3, 4); // voted 4 of 10 -> 4*2=8 !> 10
    assert!(env.trigger(&gv_proposal, &dist_proposal).is_err(), "no quorum");

    // Quorum + majority: total_voted 10 of 10 (20>10), support_weight 8 of 8 (16>8).
    env.inject_tally(&gv_proposal, 10, 8, 10, 8, 10);
    assert_eq!(env.dist_sealed_proposal(), Pubkey::default(), "not sealed yet");
    env.trigger(&gv_proposal, &dist_proposal).expect("trigger seals");

    // The distribution program now has this proposal sealed as the winner.
    assert_eq!(env.dist_sealed_proposal(), dist_proposal, "distribution sealed cross-program");

    // Re-trigger is rejected (gv proposal already executed; distribution already sealed).
    assert!(env.trigger(&gv_proposal, &dist_proposal).is_err(), "no double seal");
}

/// Regression: the quorum denominator is the LIVE subledger pool outstanding, not
/// the cached config value (synced only on votes). A minority that voted early
/// while the pool was small cannot capture the distribution after honest deposits
/// grow the pool without a re-vote.
#[test]
fn trigger_uses_live_pool_outstanding_not_stale_cache() {
    let mut env = Env::new();
    let alice = Pubkey::new_unique();
    let dist_proposal = env.create_dist_proposal(1, &[(alice, 100)]);
    let gv_proposal = env.register(&dist_proposal);

    // The attacker voted early with 6 when the pool was tiny: the CACHED config
    // outstanding is a stale 6 (6*2=12 > 6 would "pass" against the cache).
    env.inject_tally(&gv_proposal, 6, 8, 6, 8, 6);

    // ...but honest depositors have since grown the LIVE pool to 1006 without a
    // re-vote. The trigger reads the live pool, so 6*2=12 is NOT > 1006 -> rejected.
    env.set_pool_outstanding(1006);
    assert!(
        env.trigger(&gv_proposal, &dist_proposal).is_err(),
        "stale-cache minority capture must be blocked by the live-pool quorum read"
    );
    assert_eq!(env.dist_sealed_proposal(), Pubkey::default(), "not sealed");

    // Once a real quorum forms against the live pool (e.g. the pool shrinks back to
    // 10 via exits, or enough principal votes), the trigger proceeds.
    env.set_pool_outstanding(10);
    env.trigger(&gv_proposal, &dist_proposal).expect("trigger seals at real quorum");
    assert_eq!(env.dist_sealed_proposal(), dist_proposal);
}
