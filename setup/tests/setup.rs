//! The default genesis init must produce a FIXED-SUPPLY 42M COIN: the full supply
//! lands in the destination, the mint authority is revoked, and no further minting
//! is possible — verified end-to-end against the real SPL token program in litesvm.

use genesis_setup::{
    default_init_coin_ixs, default_raw_supply, DEFAULT_COIN_DECIMALS, DEFAULT_COIN_SUPPLY_COINS,
};
use litesvm::LiteSVM;
use solana_sdk::{
    program_pack::Pack,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::Transaction,
};

#[test]
fn default_init_creates_a_fixed_supply_42m_coin() {
    let mut svm = LiteSVM::new();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();

    let mint = Keypair::new();
    let authority = Keypair::new(); // transient mint authority, revoked in the same tx
    let destination = Keypair::new();
    let destination_owner = Pubkey::new_unique();

    let ixs = default_init_coin_ixs(
        &payer.pubkey(),
        &mint.pubkey(),
        &authority.pubkey(),
        &destination.pubkey(),
        &destination_owner,
    );
    let tx = Transaction::new_signed_with_payer(
        &ixs,
        Some(&payer.pubkey()),
        &[&payer, &mint, &destination, &authority],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx).expect("default init");

    // The mint holds exactly 42M coins (scaled), at the default decimals, and its
    // mint authority is revoked -> the supply can never grow.
    let mint_acc = svm.get_account(&mint.pubkey()).unwrap();
    let m = spl_token::state::Mint::unpack(&mint_acc.data).unwrap();
    assert_eq!(m.decimals, DEFAULT_COIN_DECIMALS);
    assert_eq!(m.supply, default_raw_supply());
    assert_eq!(m.supply, DEFAULT_COIN_SUPPLY_COINS * 1_000_000);
    assert!(m.mint_authority.is_none(), "fixed supply: mint authority revoked");
    assert!(m.freeze_authority.is_none(), "no freeze authority: holders never frozen");

    // The entire supply is in the destination.
    let dest_acc = svm.get_account(&destination.pubkey()).unwrap();
    let a = spl_token::state::Account::unpack(&dest_acc.data).unwrap();
    assert_eq!(a.amount, default_raw_supply());
    assert_eq!(a.owner, destination_owner);

    // And minting more is now impossible: the revoked authority cannot mint.
    let more = spl_token::instruction::mint_to(
        &spl_token::ID,
        &mint.pubkey(),
        &destination.pubkey(),
        &authority.pubkey(),
        &[],
        1,
    )
    .unwrap();
    let tx = Transaction::new_signed_with_payer(
        &[more],
        Some(&payer.pubkey()),
        &[&payer, &authority],
        svm.latest_blockhash(),
    );
    assert!(svm.send_transaction(tx).is_err(), "no party can mint after the supply is fixed");
}
