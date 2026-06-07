# Security analysis log (adversarial LOF/DOS sweep)

Running note so the 5-min loop doesn't repeat vectors. Format: vector → verdict.

## Checkpoint (latest)
Reachable six-binary surface is exhausted: ~79 vectors recorded (A–BX). Real CRITICAL bugs found + fixed by
this loop: AD signer-seed-binding, AI lamport-prefund init-DOS, AQ parasite-config insurance drain, plus 1
correctness fix AS self-loop buyback sink. NEW mutation-sharp PINS added in the BB+ run (each caught a
genuinely uncovered boundary): BB trigger-time sibling-distribution-proposal substitution (CRITICAL whole-
supply redirect — register-side + bait-and-switch tests did NOT cover it), BL re-deposit into a retired
insurance position (stuck funds + systemic quorum-denominator drag), BO filtered (below-reserve) bid
recovery (settle walks ALL occupied slots, not just eligible — else a cheap bid wedges the book). STRUCTURAL
invariants proven across all 4 programs: BJ/BK binding-identity immutability (no setter can mutate a
binding field), BR vault-mover enumeration (the distribution vault holding the whole COIN supply has only
2 validated movers — drain-proof), BS execute budget-conservation (total_usd <= holding always; zero-coin
marginal unpaid). Full regression GREEN: 168 tests (subledger insurance 39 + own-vault 6 + lib 6 = 51;
genesis-vote seal 14 + lib 3 = 17; distribution 19 + lib 4 = 23; twap chain 73 + lib 4 = 77; = 168), full
suite green, all four programs build-sbf clean.
ATTESTATION (every program x every attacker class is pinned mutation-sharp unless noted):
  TWAP auction - bidder: double-claim, settled-cancel double-spend, claim redirect (usd+coin), settled-book
    re-execute freeze, claim reopen-scan, zero-coin-marginal no-overpay; cranker: pull/spent-usd/buyback
    redirect, foreign vault/market; DAO: reserve-lower (timelock), bps>10000 over-pull, self-loop sink (both
    init+set doors), cross-config reserve/sink/fee, shutdown-escrow, reserve_den==0, round_length==0.
  SUBLEDGER - owner: cross-pool drain, fully-impaired exit, top-up-while-voted; non-owner: insurance-theft,
    own-vault non-owner; front-run: bad-policy, cross-instruction squat, lamport-prefund; vote_authority:
    hostile-lock. (over-withdraw cap doubly-defended by percolator EngineLock - documented.)
  GENESIS-VOTE - voter: re-vote + cross-proposal double-count, phantom-capital, too-recent, ballot-dust;
    trigger: strict majority/quorum (tie), snapshot bait-and-switch, live-outstanding, winner-take-all;
    register: creator + foreign-config + empty-proposal bindings; config: reinit + lamport-prefund + G/H.
  DISTRIBUTION - recipient: double-claim, wrong-recipient; burn-cranker: premature (pre-seal) torch, window;
    creator: append-after-seal, malformed entries, supply-cap, foreign-creator; config: zero-window,
    mintable/freezable/hoarding, authority-bound, underfunded, reinit (runtime-backstopped, documented).
  Copenhagen classes: sysvar-spoof N/A (syscall only); arbitrary-CPI pinned-but-litesvm-untestable +
    doubly-defended by percolator operator-dest; gv _reserved + twap market_0_domain = vestigial (never read).
  Residual is OFF-HARNESS — the on-chain surface is saturated. The BB+ run synthesized the FULL task-#6
    (orchestration tool) setup-validation requirement set, each derived from on-chain analysis and none an
    on-chain LOF: (1) deposit-deadline/kickstart timing [BH], (2) SANE claim_window_slots far below
    u64::MAX [BU — on-chain checked_add is overflow-safe], (3) UNUSED proposal-id selection [BX —
    create_proposal is reinit-guarded], (4) handover bound to the winner [BB pins the on-chain seal
    binding; orchestrator wires the winning COIN to the Squads handoff], (5) durable 1-week timelock
    [enforced on-chain at twap init_config]. The programs safely handle every trusted-setup input
    (reinit guards, checked arithmetic, PDA bindings); task #6 must supply sane inputs / unused PDAs.
On-chain FIXES this run: twap init_config enforces the bound Squads multisig time_lock >= 1 week; twap
cancel_bid no longer lets a no-op roll unlock the anti-spoof cooldown early (external issue #28).
Missing-signer guards pinned across the stack: twap reconfigure, subledger set_vote_lock, distribution
seal_winner (each verified that a privileged KEY match without a SIGNATURE is rejected).
Config-mutator auth fully covered: set_reserved_floor / set_reserve / set_coin_sink / shutdown / reconfigure
all have a direct non-Squads (or non-signing) rejection test.
All four permissionless-init PDAs (subledger pool, twap book, gv config, distribution config) now have a
finding-AI lamport-prefund-DOS regression test. The eviction refund-redirect guard is pinned inside
`e2e_full_book_evicts_only_for_a_strictly_better_bid` (extended, mutation-verified).
The percolator dep is pinned to committed revs (percolator-prog c050578, percolator 76d0e75), so a
sibling mid-edit no longer breaks the build. Recent ticks are confirmations, not new findings; the
remaining surface is runtime-guaranteed (e.g. AU SPL-authority), DAO-footgun hardening, or OFF this
harness (the `rewards-program` monolith with its own suite; the unbuilt local proposal-generation tool,
whose bugs are the realistic trigger for program-level footguns like AS). Recommend redirecting the loop
to one of those, or pausing it.

## Analyzed

### [REAL BUG FIXED (type-cosplay init squat / permissionless-init DOS) — distribution::init_config missing SPL-owner check] GX.
A reported finding (GitHub issue #29 / PR #30 from an UNTRUSTED external submitter) was independently
reproduced and fixed via LOCAL CLEAN-ROOM TDD — no remote code pulled in (hard rule: never trust remote
submitters; reproduce + fix every reported bug with our own test + our own patch). VECTOR: `distribution::
init_config` is permissionless and the config PDA is canonical per (coin_mint, authority). It unpacked the
caller-supplied `coin_mint` and `vault` via `Pack::unpack`, which does NOT verify the owning program. So a
front-runner could hand init_config a NON-SPL-owned account with token-shaped bytes (mint == COIN, owner ==
config PDA, amount >= total_supply) that clears every structural check (mint/owner/amount). Since the PDA
can't be re-initialized (AccountAlreadyInitialized), this squats the one real distribution config with a
vault no SPL Token CPI (claim/burn) can ever drive -> permanent distribution DOS (config-squat, not theft).
PROVED non-tautological: with the fix reverted, the new test FAILS (fake vault accepted, config squatted);
patched, it passes. FIX (distribution/src/lib.rs init_config): add `coin_mint.owner != &spl_token::ID ->
IllegalOwner` and `vault.owner != &spl_token::ID -> IllegalOwner` before the respective unpacks — matching the
existing `token_balance` guard at :234. Consistent with the system being exclusively classic SPL Token (claim
:536 / burn :608 both require token_program == spl_token::ID), so no Token-2022 vault is locked out. TEST
(distribution/tests/distribution.rs init_config_rejects_a_non_spl_owned_token_shaped_vault): plants a
system-owned account whose bytes round-trip through spl_token's OWN packer as an initialized token account
(mint=COIN, owner=config PDA, amount=supply) and asserts init is rejected + the config PDA stays
uninitialized. VERIFY: build-sbf green; distribution suite 21/21. NOTE: PR #30 is NOT merged — the upstream
fix is byte-identical in spirit but we never land untrusted commits; close the PR/issue referencing this
clean-room landing.

### [GR-FIXED — integration suite reconciled to the rebuilt percolator; 40-test target met (39 green, 1 obsolete merged)] GW.
Completed the GR migration deferred by GT/GV. The percolator/squads rebuild surfaced FIVE independent test-side
drifts (none new vectors — all "test asserts the OLD ABI/feature"); each reconciled to the real binaries:
(1) **canonical vault ATA** — the rebuilt percolator pins every vault to the canonical associated-token-account
(`[vault_authority, token_program, mint]` under the ATA program), rejecting random/custom vaults with
Custom(12) InvalidVaultAccount. Added `canonical_vault_ata()` helper; repointed the 4 vault sites
(make_percolator_market_data, deposit/topup, percolator_vault_for_slab). (2) **account rent growth** —
percolator portfolio/user accounts grew; init_user under-funded -> InsufficientFundsForRent; bumped the
funding lamports. (3) **UpdateAuthority encoding** — tag 32 now decodes ONLY a 32-byte new_pubkey (no kind
byte); fixed encode_update_admin. (4) **1-week timelock** — the genesis Squads multisig is created with a
1-week timelock (TIMELOCK_1_WEEK_SECS=604800), not 48h; renamed/revalued the stale SQUADS_TIMELOCK constant
and its assert messages. (5) **marketauth-burn-to-zero REMOVED** — the rebuilt percolator REJECTS zeroing
marketauth (v16_program.rs:2911 "Burning marketauth to zero is rejected"); renunciation is now via the Squads
config-authority handoff, not a zero-burn. The 4 admin-burn-dependent tests were reconciled: rewrote
`test_admin_burn_disables_all_admin_instructions` -> `test_non_admin_cannot_run_admin_instructions` (asserts
zero-burn is rejected + the still-valid property that every admin instruction is authority-gated so a non-admin
signer runs none + the real admin survives the rejected burn); merged the now-moot `test_admin_burn_is_irreversible`
into it (deleted — feature gone); restructured `test_dao_cannot_steal_via_admin_instructions` to assert a
non-market-admin DAO key can drive no fund-moving admin op (drops the burn step); trimmed the burn tail from
`test_insurance_topup_authority_gated_withdraw_restricted`, keeping the non-authority-topup rejection +
withdraw-restricted-before-resolution pins. VERIFY: build-sbf (governance + program) green; program lib 6/6,
integration 39/39, squads_handover 4/4. The GR coverage regression (admin-proxy finalize-lock pin was dark
because the suite didn't compile) is CLOSED — the suite compiles and the genesis governance surface is pinned
again. Net: no LOF/DOS introduced; GT (the only real bug in this cluster) was fixed in GV and remains fixed.

### [GT-FIXED (option-b, now VERIFIED safe — reverses GU's option-a) — genesis-DOS authority-offset drift] GV.
Fixed GT on master. Re-examined the market-substitution question GU left open and VERIFIED percolator's
answer: `marketauth` rotation REQUIRES the new key to CO-SIGN (v16_program.rs UpdateAuthority doc + handler:
"the current `marketauth` must sign; the non-zero replacement must co-sign"). So an attacker CANNOT point a
market's `marketauth` at this program's market-admin PDA (only this program can make that PDA co-sign), which
makes the marketauth=PDA-but-per-asset=attacker substitution IMPOSSIBLE. Combined with percolator deriving every
per-asset authority FROM `marketauth` at InitMarket (rotatable thereafter only by the PDA), `marketauth ==
market_admin PDA` already proves full exclusive control. So the three per-asset checks were REDUNDANT, not
load-bearing — GU's option-a (fragile dynamic-offset re-point) is unnecessary; option-b (drop them) is safe.
FIX (program/src/lib.rs): `read_market_config` drops the stale insurance_authority/operator/backing reads
(@+192/+224/+256 — they moved out of WrapperConfigV16 into a per-asset profile), fixes WRAPPER_CONFIG_LEN
624->432, and keeps `admin`(= marketauth @0). The 3 control checks (kickstart :2035, withdraw :2631, recovery
:3033) now gate on `marketauth == market_admin PDA` alone. build-sbf green; program lib 6/6; the integration
suite now COMPILES (was non-compiling) and 31/40 pass — and the kickstart now passes the AUTHORITY check
(previously reverted on garbage), confirming GT is fixed.
RESIDUAL (finding GR's broader scope — NOT GT, separate percolator/squads-rebuild drift surfaced by the now-
compiling suite; 9 integration tests): (1) kickstart TopUpBackingBucket -> Custom(12) InvalidVaultAccount — the
rebuilt percolator expects a SEPARATE backing-bucket vault; the meta passes the insurance vault for both (a real
program drift to reconcile); (2) init_user -> InsufficientFundsForRent (percolator account sizes grew); (3)
admin-burn -> InvalidInstructionData (a percolator admin-instruction encoding changed); (4) genesis Squads
multisig timelock is 604800 (1 week) but the test asserts SQUADS_TIMELOCK_48H=172800 (stale constant / the
program now uses a 1-week timelock). These need a dedicated GR migration to bring the 40-test suite fully green.

### [GT-CORRECTION (REVERSES last tick) — do NOT drop the per-asset checks; they are the market-substitution defense] GU.
Probed the market-substitution angle behind GT's proposed option-b fix and found option-b is UNSAFE — RETRACTING
the GT-UPDATE recommendation to "drop the 3 per-asset reads and gate on marketauth@0 alone." Why: kickstart
(lib.rs:2587) and genesis_withdraw (:2007 else-branch) take `market_slab` as a caller-supplied account and bind
its identity SOLELY by `collateral_mint == base_mint` (load_percolator_market_config) + the 4-way authority check
`admin && insurance_authority && insurance_operator && backing_bucket_authority == market_admin PDA`
(:2629-2632 / :2030-2033). There is NO `market_slab.key == genesis_config.market_slab` binding — GenesisConfig
stores no market key. So the 4-way check IS the market-identity guard. Attack if reduced to marketauth-only:
an attacker InitMarkets a percolator market with collateral=base_mint and marketauth=attacker (so the derived
per-asset authorities = attacker), then — IF percolator's market-level UpdateAuthority does not require new-key
consent — rotates marketauth -> the (deterministic) market_admin PDA, yielding marketauth==PDA but
insurance_operator==attacker. Under option-b kickstart would ACCEPT it and TopUpInsurance the depositors' base
capital into the attacker's market, which the attacker drains via their insurance_operator
(WithdrawInsuranceLimited) -> CRITICAL depositor-capital LOF. The CURRENT 4-way check blocks this
(insurance_operator != PDA -> reject). Whether the marketauth-rotation-consent path is actually open is a
percolator-internal question I did NOT fully verify — but the conservative, correct conclusion stands regardless:
the per-asset checks are load-bearing (or at minimum the intended market-control verification) and the GT fix
MUST PRESERVE the 4-way semantics. CORRECTED FIX = option (a): keep reading all four authorities but re-point the
3 per-asset reads at the NEW location — asset-0's AssetOracleProfileV16, which starts at a FIXED absolute offset
`MARKET_GROUP_OFF(=HEADER_LEN+432=448) + dynamic_asset_slot_offset::<AssetOracleStorageV16>(0)` (fixed for
index 0 = the market-group header size), with in-profile field offsets insurance_authority@+24, insurance_operator@+56,
backing_bucket_authority@+88 (struct: 4+4+2+2+2+2+2 + 6 pad = 24, then +32 each). PIN the absolute offset with an
`offset_of!` test against the real percolator struct in the integration suite (which CAN dev-dep percolator-prog),
exactly like twap's finding-T insurance@749 pin — never hardcode unpinned. Still a deliberate task-#11 change
(needs the offset_of! pin + the trivial integration.rs marketauth helper migration + the non-GT timelock/encoding
test-data fixes for a fully-green 40-test suite). Net: GT remains a real genesis DOS; the fix is option-a, NOT
option-b.

### [REAL BUG (genesis DOS) — meta read_market_config reads 3 authorities at STALE percolator offsets after the v16 rebuild] GT.
While restoring the GR-dark integration suite I PROVED the test-side migration is trivial (the percolator v16
rebuild collapsed admin/asset_authority/base_unit_authority into one market-level `marketauth` at wrapper
offset 0, and moved insurance_authority/insurance_operator/backing_bucket_authority/oracle_authority/asset_admin
into a per-asset AssetOracleProfileV16 that `init_market_account_zero_copy` derives from `marketauth`, so the
test's make_percolator_market_data helper just sets `wrapper.marketauth = admin`). That fix makes integration.rs
COMPILE (31/40 pass) and exposed a REAL meta-program bug:
program/src/lib.rs `read_market_config` (:46-117) parses the percolator slab by RAW OFFSET: admin@HEADER_LEN+0,
collateral@+32, secondary@+64, but insurance_authority@+192, insurance_operator@+224, backing_bucket_authority@+256.
In the REBUILT percolator those three authority fields NO LONGER live in WrapperConfigV16 — they moved to
AssetOracleProfileV16 (v16_program.rs:724-756, a different slab region) — so offsets 192/224/256 now read
UNRELATED wrapper bytes (oracle/fee/leg fields). admin@0 still reads correctly (marketauth IS at offset 0). The
meta program USES all four in its full-control checks at kickstart (:2030-2033), genesis_withdraw-after-kickstart
(:2629-2632), and recover_genesis_market (:3034-3037): `admin/insurance_authority/insurance_operator/
backing_bucket_authority != market_admin PDA -> revert`. Since the 3 moved reads now return garbage != the PDA,
ALL THREE paths REVERT against the rebuilt percolator -> the genesis lifecycle is BRICKED: a kickstarted market
cannot be withdrawn from and recovery is impossible -> depositor principal STRANDED (DOS, effective LOF). This is
a hardcoded-offset drift, not stale test data — the integration tests test_full_genesis_to_dao_lifecycle_*,
test_genesis_bootstrap_kickstarts_market_50_50, *_withdraw_after_kickstart, *_exit_rejects_overpull all fail on
exactly this. SEVERITY: high IF the deployed percolator is the new v16 layout (the sibling was deliberately
rebuilt 2026-06-05). FIX (NOT done this tick — needs a careful finding-T-style offset migration on the meta
SOURCE, too risky to rush): point the 3 authority reads at the asset-0 AssetOracleProfileV16 slab location, or
(simpler + safer) drop them and rely on `marketauth@0` alone PLUS percolator's own per-asset authority
enforcement — since at init all four are derived from marketauth and only percolator can rotate the per-asset
ones (which the handoff already drives). Other integration failures triaged as TEST-DATA (48h vs 1-week timelock
expectation @3274) or percolator instruction-ENCODING drift (admin-burn tag @2925) — separate, lower-stakes.
Test edits reverted this tick to keep the committed suite state clean (no red committed); finding recorded for a
dedicated migration. Flagged to the user as the headline item.
GT-UPDATE (scope + fix pinned down): the drift is CONTAINED, not systemic — the meta program's ONLY raw slab
reads are in read_market_config (lib.rs:45-117). admin@HEADER_LEN+0, collateral@+32, secondary@+64 are STILL
correct (marketauth/collateral_mint/secondary_collateral_mint remain the first three WrapperConfigV16 fields at
0/32/64); ONLY the 3 per-asset authorities @+192/+224/+256 are wrong. The meta program does NOT read
insurance/vault balances by offset (it manages insurance via CPIs and computes the 50/50 kickstart from the
deposited amount), so finding-T-style balance offsets are not affected — twap already owns those at the new
MARKET_GROUP_OFF=448 (percolator WRAPPER_CONFIG_LEN is now 432, vs the meta's stale 624 — harmless to the >=640
length check, fatal only to the 3 authority reads). The 3 fields moved to a per-asset AssetOracleProfileV16 whose
slab location is a DYNAMIC capacity-dependent offset (percolator dynamic_slot_offset), NOT a fixed constant — so
replicating it in the meta's minimal raw parser (which deliberately does NOT depend on percolator_abi) is fragile.
RECOMMENDED FIX (option b, robust + secure): DROP the 3 per-asset reads/checks and gate kickstart/withdraw/
recovery on `admin`(marketauth)@0 == market_admin PDA alone. Security argument: at InitMarket percolator derives
all per-asset authorities (insurance_authority/operator/backing/asset_admin) FROM marketauth
(asset_oracle_profile_from_config), and rotating any of them later requires the CURRENT per-asset authority or
asset_admin (both = the PDA pre-handoff) to sign percolator's UpdateAssetAuthority — which only the meta program
can do. So for a meta-created market, marketauth==PDA STRICTLY IMPLIES the per-asset authorities are still ==PDA;
the 3 checks are redundant defense-in-depth, not load-bearing. (Option a — replicate dynamic_slot_offset to read
the new location — preserves the literal 4-way check but is fragile to the next percolator slot-layout change.)
This is a security-relevant simplification of the meta validation surface + needs the full integration suite
green to verify (incl. the non-GT timelock/encoding test fixes), so it stays a deliberate task-#11 change, not a
probe-tick edit. Also swept this tick + confirmed exhaustively pinned: distribution burn_unclaimed (window-gated
checked-add, burns only the post-window vault remainder = unclaimed, idempotent, pre-seal torch blocked by
is_sealed) — pinned by unclaimed_is_burned_after_window, burn_unclaimed_is_rejected_during_the_claim_window
(boundary-1 sharp), burn_unclaimed_before_the_genesis_seals_cannot_torch_the_vault.

### [VERIFIED BLOCKED (cross-program backstop traced into the real binary) — twap execute unvalidated percolator_vault] GS.
HOSTILE vector (cross-vault drain via a substituted source vault): twap `execute` forwards the `percolator_vault`
account straight into the WithdrawInsuranceLimited CPI (lib.rs:1394) WITHOUT validating it against the market —
note the ASYMMETRY: subledger's insurance_withdraw DOES validate `percolator_vault.key == pool.vault`
(subledger:1023). If percolator accepted any vault_authority-owned account, an attacker could point the pull at a
DIFFERENT same-authority vault (e.g. a trading/backing sub-account) and drain the wrong bucket into the holding.
Traced the contract into the REAL percolator binary: handle_withdraw_insurance_limited (v16_program.rs:8500)
derives the vault_authority from the market (`derive_vault_authority(program_id, market_ai.key)`, :8536, then
`expect_key`), checks the operator is asset-0's live insurance_operator (:8532), and calls
`verify_withdrawable_token_accounts` (:8538) which PINS the vault to the SINGLE CANONICAL address —
`*vault_token_ai.key != canonical_vault_address(expected_vault_owner, &vault.mint)` -> InvalidVaultAccount
(:12010, the explicit "F-VAULT-FRAG" guard: "Without this, ANY vault_authority-owned account is accepted,
enabling liquidity fragmentation"). The dest (holding) must also be owned by the operator (twap_authority) and
share the collateral mint (:11995-11998). So a substituted percolator_vault is rejected by percolator regardless
of twap's lighter check. The asymmetry is by design: subledger stores `pool.vault` (cheap to assert); twap does
NOT store the vault and correctly delegates vault-custody validation to percolator, the custodian. Verdict:
BLOCKED (percolator canonical-vault pin backstops it). No code change (a twap-side check would re-test
percolator's guard, not a twap boundary — redundant). No new test (a substituted-vault e2e would assert
PERCOLATOR's F-VAULT-FRAG, already its own concern). Also swept this tick + confirmed exhaustively guarded: all
twap Squads setters (set_reserve/set_coin_sink/set_bid_fee) gate on require_squads_vault (canonical vault signer
of the config's bound multisig); the timelock ROOT (init_config :410 `time_lock < MIN_TIMELOCK_SECS`) is
boundary-sharp-pinned at EXACTLY 1 week by `twap_config_rejects_a_multisig_below_the_one_week_timelock`
(chain.rs:292, tests 604_799 reject + 1-week accept); accept_operator (:537) is Squads-gated + market-bound +
derives twap_authority + does the finding-S insurance-authority rotation.

### [COVERAGE REGRESSION (upstream ABI drift) — program/integration.rs no longer compiles; admin-proxy finalize-lock pin is dark] GR.
VECTOR PROBED: meta-program percolator ADMIN PROXY (IX 20) privilege escalation / pre-finalization grief. Read
the handler (program/src/lib.rs:1435 process_percolator_admin) — it is correctly triple-gated: (a) tag allow-list
`percolator_admin_tag_allowed` (:759) is lifecycle/config-scoped and EXCLUDES UpdateAuthority/custody moves, so
the proxy can never rotate market authorities; (b) the signer must equal `coin_cfg.authority` via
`validate_governance_authority` + explicit equality (:1462-1467) — NOT permissionless; (c) the whole proxy is
LOCKED until `genesis_cfg.is_finalized()` (:1477) so a dishonest governance majority cannot forward
RESOLVE_MARKET / CLOSE_SLAB / UPDATE_INSURANCE_POLICY / fee changes that would brick per-depositor principal
recovery while capital is at risk. Logic is sound by inspection. The pins for all three live in
program/tests/integration.rs::test_genesis_governance_surface_is_fixed_and_controller_gated (:2496 funding-tag
reject, :2501 custody-move reject, :2512 pre-finalization RESOLVE reject).
HOWEVER — `program/tests/integration.rs` NO LONGER COMPILES. The read-only sibling `../percolator-prog`
(workspace dev-dep) was rebuilt 2026-06-05 (.so + source) AFTER integration.rs was last touched (2026-05-31),
and its authority model was RESTRUCTURED: seven authority fields (`admin`, `base_unit_authority`,
`insurance_authority`, `insurance_operator`, `backing_bucket_authority`, `asset_authority`, `mark_authority`)
were moved OFF `WrapperConfigV16` (which now exposes `marketauth` + a per-asset/oracle sub-struct holding
`insurance_authority`/`insurance_operator`/`backing_bucket_authority` at v16_program.rs:736+). The meta program
`src/lib.rs` STILL builds (build-sbf green — it reads via `read_market_config`, not the moved fields) and the
PRIMARY six-binary e2e harness (twap-program/tests/chain.rs, 75) + subledger (44+6) + distribution (20+4) +
genesis-vote are ALL green against the rebuilt .so. Only the 40-test integration suite (genesis bootstrap,
admin-proxy scoping, governed-mint lock, builder registry, genesis principal recovery) is dark — a one-shot
test-only setup helper (`build...wrapper`, integration.rs:~162-172, 8 field-sets) writes the now-moved fields.
VERDICT: admin-proxy guards are sound by inspection + structurally unchanged in src; this is a TEST-COMPILE
coverage regression from a read-only upstream refactor, NOT a program bug. FIX deferred to a dedicated migration
task (reconcile the integration test market-setup to the new percolator authority layout) — out of the per-tick
security-probe scope, and rushing it risks a test that compiles but mis-stages the market (false confidence). No
src change this tick. Flagged to the user.

### [NEW PIN (backstop-explained) — co-depositor drain via same-pool over-withdraw was UNTESTED] GQ.
HOSTILE vector (one depositor drains co-depositors): insurance_withdraw caps the requested `amount` by BOTH
`amount > position.principal` AND `amount > pool.outstanding_principal` (subledger lib.rs:1054). Since
`outstanding == Σ position.principal`, the per-position clause is ALWAYS the tighter bound — drop it and a
depositor in a multi-party pool can request up to the WHOLE pool (amount up to outstanding): payout returns
that full amount, the percolator WithdrawInsuranceLimited CPI pays it out, and the thief drains co-depositors
while `position.principal -= amount` (line 1128) underflows their own principal to ~u64::MAX (a perpetual
infinite-withdrawal position). MUTATION AUDIT: dropping `amount > position.principal` passed the ENTIRE 49-test
subledger suite -> the critical anti-drain bound was UNPINNED. ROOT-CAUSE of the mutation-blindness: the
decrement at 1128 is itself guarded by `overflow-checks = true` (workspace [profile.release]) — under the
mutation the over-withdraw still REVERTS because the underflow PANICS atomically (rolling back the CPI). So the
guard is BACKSTOPPED defense-in-depth, NOT a live exploitable hole. CRUCIALLY confirmed the percolator CPI does
NOT independently bound the withdraw to the position's share: with the guard removed AND the decrement softened
to `saturating_sub` (a realistic "stop the panic" refactor), the 2M over-withdraw SUCCEEDED and drained the
co-depositor — i.e. subledger ALONE owns this bound; percolator will pay whatever the operator asks. FIX/ACTION:
no code change (guard is present + correct; overflow-checks is a real second layer). Added regression test
`a_depositor_cannot_withdraw_more_than_their_own_principal_and_drain_a_co_depositor` (insurance_percolator.rs):
healthy 2-party pool, Alice requests the whole 2M (> her 1M, == outstanding), asserts REJECTED + vault still 2M +
outstanding unchanged + Alice's principal intact (no underflow) + the honest exact-1M exit still works. Verified
it FAILS the moment the bound is removed and the decrement can't panic (guard+backstop combo broken) and PASSES
on real code — a genuine pin of the co-depositor-safety invariant + the subledger-owns-the-bound fact. Verdict:
BLOCKED (guard live, overflow-checks backstop, now test-pinned).

### [VERIFIED SHARP — zero-COIN marginal cannot extract free USD (execute clearing-math LOF guard)] GP.
HOSTILE vector (free-money via a dust marginal bid): the uniform-price clear pays each filled bid `usd_i`
USD for `coin_i = floor(usd_i * cm/um)` COIN (cm/um = marginal price). With a low reserve, the MARGINAL bid
can be filled for a tiny residual `usd_i > 0` whose `coin_i` floors to ZERO — if the protocol still booked
that fill it would move `usd_i` USD into settlement_usd (claimable by the bidder) AND refund the bidder's
FULL escrow (coin_refund = c - 0 = c): the bidder pockets USD for delivering nothing, a direct LOF drained
from the buy/burn budget. The defense is execute lib.rs:1497 `if usd_i > 0 && coin_i > 0` — the `coin_i > 0`
clause routes a zero-COIN fill into the else-branch (usd_owed=0, full refund, treated as unfilled), so the
protocol NEVER pays USD for zero COIN. MUTATION AUDIT: dropped the `&& coin_i > 0` clause (widened to
`if usd_i > 0`), rebuilt, ran the chain suite -> `e2e_settle_with_a_zero_coin_marginal_pays_no_usd_for_zero_coin`
(chain.rs:5154) FAILED (protocol spent bob's residual 400_000 vs the correct 399_600 = alice-only). Guard is
SHARP, not mask-blind — the dust-marginal boundary is already pinned by a non-tautological e2e test that builds
a real low-rate marginal and asserts only the whole-COIN winner's USD is spent. Verdict: BLOCKED (guard live +
test-pinned). No code/test change. Also confirmed this tick: claim binds both payout destinations to the
recorded canonical ATAs (:1617), cancel_bid rejects SETTLED slots (:1696, no double-dip vs burned escrow), and
init_book is the SOLE book creator — PDA-bound `["twap_book",config]` (:941-945), reinit-guarded (:946),
Squads-gated (:891) — so claim/execute's lighter `book.config==config` checks are correctly backstopped (no
forged-book path). The twap auction account-identity + clearing-math surface is exhaustively pinned.

### [VERIFIED SAFE (on-chain side of finding-BU) — claim_window_slots overflow is checked, not silent] GO.
HOSTILE vector (claim-window overflow -> window-bypass or stuck COIN): the claim window is `window_end =
seal_slot + claim_window_slots`; claim allows `clock < window_end`, burn_unclaimed requires `clock >= window_end`.
A huge claim_window_slots (near u64::MAX) overflows the sum. Two outcomes to distinguish: (a) SILENT WRAP ->
window_end becomes tiny -> claim refused instantly + burn torches the vault immediately (recipient LOF), or the
inverse (claim forever); vs (b) CHECKED REVERT -> a clean DOS (COIN stuck, can't claim or burn). Checked the
code: BOTH sites use `seal_slot.checked_add(claim_window_slots).ok_or(ArithmeticOverflow)` (distribution
lib.rs:525-527 claim, and burn_unclaimed) -> outcome (b), the SAFE one: overflow REVERTS, NO silent window
corruption/bypass. Init bounds `claim_window_slots == 0 -> reject` (:276) but has NO upper bound -> that upper
bound is the off-harness finding-BU (the proposal-gen tool must set claim_window far below u64::MAX). Crucially
an ATTACKER cannot weaponize this: to bind a malicious-claim-window distribution config, EZ's solvency check
requires the config's vault to already hold the FULL COIN supply (which only the legit orchestration mints/holds),
so a forged config is rejected at init. So the only reachability is an ORCHESTRATION bug (BU), and even then the
on-chain reverts cleanly — no silent LOF/bypass, just a recoverable-only-by-redeploy DOS. DEFENSE-IN-DEPTH
option (not implemented, design/DAO call): init_config could reject claim_window_slots above a sane cap to
fail-fast at init instead of bricking at seal. Verdict: BLOCKED (corruption-safe; upper bound is BU's
off-harness job). No code/test change.

### [VERIFIED SHARP — withdrawn-flag set ONLY on full exit (premature-retire stuck-remainder LOF)] GN.
Mirror of GM (the withdraw side of the withdrawn-flag lifecycle). HOSTILE vector (premature retirement strands
the remainder): insurance_withdraw decrements `position.principal -= amount` then retires the position
`if position.principal == 0 { position.withdrawn = true }` (subledger lib.rs:1133-1134). If withdrawn were set
on ANY withdraw (even a PARTIAL exit leaving principal > 0), the position would become `withdrawn=true,
principal>0` and the NEXT withdraw of the remainder would hit the hard reject `if position.withdrawn -> Err`
(:1042) -> the leftover principal is permanently STUCK in percolator insurance = LOF. Mutated the condition to
`if true` (retire on any withdraw) -> TWO tests FAIL: `principal_only_owner_exit_returns_funds_and_guards`
(full exit must succeed and retire cleanly) and `splitting_an_impaired_exit_cannot_beat_the_pro_rata_or_drain_a_codepositor`
(split/partial exits must each still let the remainder be withdrawn). Mutation-SHARP. So a partial exit keeps
the position ALIVE (remainder withdrawable) and only a full exit (principal==0) retires it terminally. This
closes the withdrawn-flag state machine: set only on full exit (GN), blocks re-deposit (GM), blocks re-withdraw
(:1042 + CZ cap) — partial exits never strand capital, full exits are terminal. Verdict: BLOCKED, no gap. No
code/test change.

### [VERIFIED SHARP — re-deposit into a retired position blocked (stuck-capital LOF / inconsistent state)] GM.
HOSTILE vector (revive a retired position into a stuck state -> LOF): the insurance position PDA is
deterministic per (pool, owner), and a full exit sets `withdrawn=true, principal=0` (terminal). If
insurance_deposit allowed re-depositing into that SAME retired position, it would become `withdrawn=true,
principal>0` — an inconsistent state. Then insurance_withdraw HARD-REJECTS any `position.withdrawn` (subledger
lib.rs:1042 `return InvalidAccountData`), so the re-deposited capital could NEVER be withdrawn -> permanently
stuck in the percolator insurance vault = LOF (and a revived position could also vote with the re-deposited
principal, finding-AR phantom territory). Guard: on re-deposit into an existing position, insurance_deposit
rejects `p.owner != owner || p.pool != pool || p.withdrawn` (:897). Anti-mask: the test re-deposits into the
depositor's OWN retired position (owner+pool CORRECT), so only the `p.withdrawn` clause decides. Mutated that
clause to `false` -> `cannot_redeposit_into_a_retired_position` FAILS = mutation-SHARP. So a fully-exited
position is terminally retired (no revive); to re-participate a user needs a fresh owner key (distinct PDA) —
the deliberate one-shot retirement consistent with the Sybil model and the withdrawn-flag terminality (EO/CZ
gate re-withdraw via the principal cap; GM gates re-deposit via the withdrawn flag). Verdict: BLOCKED, no gap.
No code/test change.

### [VERIFIED DEAD STATE — gv config outstanding_principal cache is never read for a decision] GL.
GK dead-field lens on the gv `Config.outstanding_principal` cache (refreshed each vote from the live pool,
genesis-vote lib.rs:581). Risk: if the quorum decision read this CACHE, a stale/deflated value (cf. FU's
vote-time fake-pool attempt) could fake quorum. Traced: it is WRITTEN at vote (:581) and read NOWHERE for a
decision — the trigger computes quorum against `live_outstanding = read_sub_pool_outstanding(sub_pool)` re-read
LIVE from the config-bound pool (DX, mutation-sharp), NOT config.outstanding_principal. Confirmed by mutation:
corrupted the cached write to `read_sub_pool_outstanding(..)?.wrapping_add(999_999)` -> seal 14 + insurance 43
PASS = the cache is DEAD STATE (no test, hence no on-chain decision, depends on it). So the cache's
value/staleness/deflation has ZERO security impact: the quorum is always measured against the live pool. This
fully closes the FU vote-time cache vector (the cache it could affect is unread) and matches GK (a recorded-but-
unused field). Pure hygiene (a wasted write), no fix needed. Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED — SL_PLACE_ROUND_END is diagnostic-only (no written-but-misused-field seam)] GK.
Probed for a "written-but-misused field" bug seam: place_bid records `SL_PLACE_ROUND_END` = book.round_end at
placement (twap lib.rs:1269), a per-slot field. Risk: if a guard READ it (e.g., a cooldown computed against the
PLACED round_end), a round-timing change could shift the gate. Traced all uses: it is WRITTEN once (:1269) and
read NOWHERE except the layout-packing unit test (:1807-1808). The source comment (:670) is explicit —
"Recorded for layout/diagnostics" — it is not a gate. The cancel cooldown correctly anchors on `SL_PLACE_SLOT`
(`now >= place_slot + 2*round_length`, EP mutation-sharp) and the cancel code DELIBERATELY ignores any
round_end delta (its comment: "we deliberately do NOT shortcut on a round_end delta ... Gate on aging alone")
precisely to stop a permissionless no-op-roll from advancing round_end and unlocking an early cancel (the
anti-spoof commitment). round_length is fixed at init_book (no setter), so the place_slot-anchored cooldown is
stable. So the diagnostic field has zero security impact and the real cooldown gate is the mutation-sharp,
round_end-independent SL_PLACE_SLOT. Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP (post-GG regression check) — DA bait-and-switch snapshot guard + GG offset-shift wiring] GJ.
FX-discipline check after the GG layout change: the GG fix shifted gv ProposalVote fields, incl. the DA
bait-and-switch snapshot `snapshot_entry_count` (@89->@97) and `snapshot_total_amount` (@93->@101). Verified the
shift did NOT silently break the DA guard (trigger refuses to seal a proposal whose distribution
(entry_count,total_amount) changed since registration -> stops a creator appending self-allocations AFTER
voters back it). The guard is `pd[84..88] != pv.snapshot_entry_count || pd[88..96] != pv.snapshot_total_amount`
(genesis-vote lib.rs:729-730) — pd[..] is the DISTRIBUTION proposal (offsets unchanged); pv.snapshot_* is the
gv struct read via the updated deserialize. Mutating EITHER clause alone -> `proposal_changed_after_registration_cannot_be_sealed`
PASSES = mutually-MASKED (an append changes BOTH entry_count +1 AND total_amount, so the other clause catches
it; an attacker cannot change only one via the public append -> FA-class redundancy, not a gap). Mutating BOTH
-> the test FAILS = the COMBINED guard is mutation-SHARP. Crucially this also proves the GG offset-shift is
correctly wired: a wrong snapshot offset would mismatch on EVERY trigger (legit seals would fail) — but the
full-genesis seal e2e passes AND the bait-and-switch is caught, so @97/@101 are right. No regression from the
GG fix. Verdict: BLOCKED, no gap; GG layout change verified non-breaking downstream. No code/test change.

### [VERIFIED SAFE — reserve-rate comparison cmp_rate is overflow-safe (continued fractions)] GI.
Continuing the GG arithmetic-overflow sweep onto the auction's rate comparisons. The reserve filter
`cmp_rate(c, u, reserve_num, reserve_den)` (twap lib.rs) drops bids whose rate c/u is below the DAO-set reserve
reserve_num/reserve_den. reserve_num/den are u128 (set by set_reserve, :876-877), so a NAIVE cross-multiply
`c·reserve_den` vs `reserve_num·u` could overflow u128 (u128·u64) -> wrong comparison -> a sub-reserve bid
accepted (protocol OVERPAYS for COIN = LOF) or an above-reserve bid wrongly dropped. Checked the code: cmp_rate
does NOT cross-multiply — it is a CONTINUED-FRACTION comparator (repeated quotient `n/d` + remainder `n%d`,
Euclidean-style, swapping into the remainder), which compares any two u128 fractions WITHOUT ever multiplying
the numerators/denominators together -> overflow-safe by construction for arbitrary u128 operands. Lib test
`cmp_rate_orders_by_coin_per_usd` pins it incl. `cmp_rate(u128::MAX, 3, u128::MAX, 4) == Greater` (max operands,
no overflow). The COMPANION ranking comparator cmp_bid (bid-vs-bid) DOES cross-multiply (coin_a·usdc_b) but is
safe because the legs are u64-bounded (finding AC: as_u64 rejects > u64::MAX, so u64·u64 < 2^128) — DU pinned
that. So both auction comparators are overflow-safe: cmp_rate (u128 reserve) via continued fractions, cmp_bid
(u64 legs) via bounded cross-multiply. No GG-class overflow in the rate/ranking math. Verdict: BLOCKED, no gap.
No code/test change.

### [VERIFIED SAFE — GG arithmetic-overflow class sweep (no other amplified-quantity overflow)] GH.
After fixing GG (u64 weight-tally overflow), swept the whole codebase for the SAME class — an AMPLIFIED/derived
quantity (a product, not a raw token amount) accumulated or compared in a width too small to hold it. Findings:
(1) subledger pro-rata haircut `mul_div_floor(balance, principal, outstanding)` does the multiply in u128
(`a as u128 * b as u128 / denom as u128`) so balance·principal (up to u64::MAX² ~ 2^128) cannot overflow; the
`as u64` cast cannot truncate because payout enforces `principal <= outstanding` => pro_rata = balance·principal/
outstanding <= balance <= u64::MAX. (2) twap execute clearing totals `total_coin`/`total_usd` are u128 (lib.rs:1420-21,
checked_add). (3) gv quorum `(total_voted_principal as u128) * 2 <= live_outstanding as u128` and the post-GG
majority `support_weight(u128) * 2 <= total_cast_weight(u128)` are full-width u128 (support_weight <= ~30·u64::MAX
~ 2^69, *2 = 2^70 << u128::MAX). (4) The PRINCIPAL sums (total_voted_principal, support_principal,
pool.outstanding_principal, distribution total_amount) are u64 but TOKEN-BOUNDED — Σ Pᵢ <= the mint supply <=
u64::MAX — so they cannot overflow (a raw token amount, not an amplified product). (5) twap bid legs are
u64-bounded and cmp_bid cross-multiplies in u128 (finding AC/DU). CONCLUSION: GG (the multiplier·principal
weight) was the UNIQUE amplified-quantity that was summed in u64; every other accumulator is u128 or sums
token-bounded principals. No other GG-class overflow exists. Verdict: BLOCKED, no gap. No code/test change.

### [FIXED — u64 weight-tally overflow -> vote-freeze DOS / minority-seize] GG.
*** First confirmed on-chain BUG (not a coverage gap), now FIXED (u128 weight tallies). Reproduced end-to-end,
fixed, and a regression added; all 6 suites green (insurance 43, seal 14, gv-lib 3, sub-lib 6, dist 20, chain 75). ***
FIX LANDED: `vote_weight` now returns u128 `(age.ilog2() as u128) * (principal as u128)` (no saturation; the
product is < u128::MAX), and `Config.total_cast_weight` (@208..224), `ProposalVote.support_weight` (@72..88),
`Ballot.voted_weight` (@72..88) are u128 — so the summed log-weights cannot overflow and `checked_add` never
rejects an honest vote, while the majority `support*2 > total_cast` is computed in full-width u128 (no minority-
seize from a capped denominator). Sizes grew: CONFIG_SIZE 232->240, PROPOSAL_SIZE 104->112, BALLOT_SIZE 112->120;
later fields shifted (Config outstanding@216->224 / bump->232; ProposalVote support_principal@80->88 /
executed@88->96 / snapshots->97,101; Ballot voted_principal@80->88). Test offset sites updated (seal.rs
inject_tally, insurance gv_proposal_support/read_cast/executed-read). Regression
`a_high_cast_weight_tally_does_not_overflow_and_block_honest_votes` injects total_cast_weight = u64::MAX (the old
overflow boundary) and asserts a real vote still lands + the tally grows PAST u64::MAX. (NOTE: the original
huge-deposit repro hit a percolator market deposit limit at ~2e18, so the regression uses tally-injection
instead — same boundary, no impossible deposit.)
MECHANISM: gv vote weight = `floor(log2(hold)) * principal` via `saturating_mul` (genesis-vote lib.rs vote_weight),
and the tallies `Config.total_cast_weight` / `ProposalVote.support_weight` are u64, accumulated with
`checked_add(weight)` (:642-645). The WEIGHTED sum can legitimately exceed u64::MAX: Σ(mᵢ·Pᵢ) <= ~30·Σ(Pᵢ) =
~30·outstanding, and outstanding can approach u64::MAX (token supply). Once `total_cast_weight` reaches u64::MAX
(one saturating whale, or just high participation on a large-supply collateral), EVERY subsequent vote's
`checked_add` OVERFLOWS -> ArithmeticOverflow -> the vote is REJECTED. So a SUB-MAJORITY whale (principal big
enough to saturate but < ½ outstanding) freezes the tally and blocks ALL honest voters -> no other proposal can
accumulate weight -> genesis bricked (and naive `saturating_add` would be WORSE: a capped denominator lets a
minority pass the `support*2 > total_cast` majority). REPRO: `a_saturating_whale_vote_does_not_freeze_the_tally...`
(insurance_percolator.rs, now #[ignore]) — Alice 2e18 (minority, m=10 -> weight 2e19 saturates), Bob 3e18
(larger honest stake); Alice votes first, Bob's vote is REJECTED. REACHABILITY: only when Σ(mᵢ·Pᵢ) > u64::MAX
i.e. collateral supply > ~6e17 atoms -> UNREACHABLE for USDC (3.5e16) / SOL (5.8e17) and most majors; reachable
for a high-supply/high-decimal collateral. SEVERITY: griefing DOS / liveness break (and minority-seize if
mis-fixed); low likelihood (collateral-bounded) but a real correctness break of the "every funded voter can
vote" + majority invariants. FIX PLAN: widen to u128 — vote_weight returns u128 (no saturation; m<=64, P<=u64::MAX
=> m·P < u128::MAX), and `Config.total_cast_weight` (@208), `ProposalVote.support_weight` (@72),
`Ballot.voted_weight` (@72) become u128 (shifts later fields + CONFIG_SIZE/PROPOSAL_SIZE/BALLOT_SIZE +8 each;
update serialize/deserialize, the accumulation/backout/majority, and test sites: seal.rs inject_tally @208/@72,
insurance gv_proposal_support @72/@80, the executed@88 raw reads). Verdict: REAL BUG, fix PENDING (u128 tallies);
repro preserved + ignored; suite green.

### [VERIFIED SHARP — append zero-entry guard (both clauses; underpins EE OOB-claim backstop)] GF.
Anti-mask of the distribution append zero-entry guard `if amount == 0 || pk == Pubkey::default() { reject }`
(lib.rs:428) — the invariant that EE's out-of-bounds-claim defense rests on (an OOB claim reads a ZERO slab
entry; if no valid entry can be zero, that read can never match a real signing recipient/amount). Risk: a test
passing an entry zero in BOTH amount and pk would let either clause mask the other. Checked: NOT masked.
`append_rejects_a_zero_amount_or_default_pubkey_entry` violates each clause SEPARATELY — `(alice, 0)` (zero
amount, valid pk -> pins the amount clause) and `(default, 50)` (zero pk, valid amount -> pins the pk clause).
Mutated the pk==default clause to `false` (keeping amount==0) -> that test FAILS = mutation-SHARP for the
zero-pubkey clause specifically. So no valid entry has a default recipient or zero amount, which is exactly
what makes EE sound: claim's OOB-index defense (CL pk==recipient + DM amount!=0 + slice-panic) can never be
satisfied by an unfilled zero slab entry, because a real recipient is a live-keypair signer (never the default
key) with a nonzero allocation. (Pairs: EF append creator-binding sharp; EE OOB index doubly-defended; GF the
zero-entry invariant under both.) Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED BACKSTOPPED — finding-AR own-vault withdraw on an insurance position (vault-ownership)] GE.
HOSTILE vector (phantom-capital Sybil vote via the WRONG withdraw instruction): the own-vault withdraw (IX 2,
process_withdraw) sets `withdrawn=true` and pays out WITHOUT decrementing `principal` (own-vault positions are
fully retired). If it ran on a genesis INSURANCE position, a voter could "exit" (get paid) yet leave principal
INTACT -> gv read_sub_position reads the stale principal -> re-vote with phantom capital while the live-outstanding
quorum denominator shrank = a free denominator-shrinking Sybil vote (finding AR). Guarded 3 ways; primary is
`if pool.is_insurance() -> InvalidAccountData` (subledger lib.rs:586). Mutated :586 to disable it -> 42 insurance
+ 6 lib PASS = MUTATION-BLIND. Analyzed: NOT a gap — backstopped STRUCTURALLY by (b) vault-ownership: the
own-vault withdraw transfers from `pool.vault` signed by the POOL PDA, but for an insurance pool `pool.vault`
is the percolator insurance vault OWNED BY THE MARKET vault_authority (not the pool PDA), so the SPL transfer
is refused (pool is not the source's authority) -> revert; and (c) the position is mutated only AFTER the
payout, so the revert leaves it intact (no phantom withdrawn state). So an own-vault exit on an insurance
position moves no funds AND corrupts no state even with :586 gone -> no phantom capital. :586 is fail-fast
hygiene. (The legitimate insurance exit, IX 5, DOES decrement principal — EO — so a real exit zeroes vote
weight; cannot_vote_with_a_withdrawn_position pins that.) The `own_vault_withdraw vs insurance` test confirms
the end-to-end boundary (IX 2 on the genesis insurance position refused, position fully intact) — KEPT as the
e2e isolation pin. Verdict: BLOCKED (defense-in-depth; vault-ownership + post-payout mutation). No code/test
change.

### [VERIFIED BACKSTOPPED — finding-Q init_pool PDA squat (structural disjoint-namespace + signed-seed)] GD.
HOSTILE vector (cross-instruction PDA squat / seed-collision): own-vault `init_pool` (tag 0) and
`init_insurance_pool` (tag 3) share `pool_seeds(mint, asset_id, market_slab, percolator_program)`. Point
init_pool's pool_account at the genesis insurance PDA (mint,0,REAL_market,REAL_program) to seize it with a
BACKING-domain own-vault pool -> legit insurance init then fails AccountAlreadyInitialized -> genesis bricked
(gv needs is_insurance()). finding-Q defense: init_pool HARDCODES the market/program seed parts to
`Pubkey::default()` (subledger lib.rs:393-394) and checks `*pool_account.key != expected_pool -> InvalidSeeds`
(:400). Mutated :400 (drop the explicit key check) -> 42 insurance PASS = MUTATION-BLIND. Analyzed: NOT a gap
— backstopped STRUCTURALLY: create_pda_robust does invoke_signed(allocate(pool_account), seeds = default-market
pool_seeds); the runtime REQUIRES the signing seeds to DERIVE the account address, but default-market seeds
derive own_vault_pda != the real-market insurance PDA, so the allocate is refused (seed/address mismatch) even
with :400 gone. Own-vault (default,default) and insurance (real,real) PDAs are provably DISJOINT namespaces, so
init_pool can never create at the insurance PDA; :400 is fail-fast hygiene. `own_vault_init_pool_cannot_squat_the_genesis_insurance_pda`
confirms the end-to-end boundary (init_pool at env.pool refused, PDA untouched, real init then succeeds at
INSURANCE domain bound to the real market) — KEPT as an e2e no-squat test (GA/FL class). Verdict: BLOCKED
(defense-in-depth; structural namespace disjointness + PDA-signing). No code/test change.

### [VERIFIED SHARP — init_insurance_pool policy range check blocks bad-policy pool-brick DOS] GC.
HOSTILE vector (front-run the genesis pool init with a garbage policy to brick it): init_insurance_pool is
permissionless and writes the pool's `policy` byte. An out-of-range value (POLICY_WITH_SURPLUS+1 = 2) would be
stored, then `Pool::deserialize` — which rejects `policy > POLICY_WITH_SURPLUS` on EVERY read — would make the
pool unreadable forever: all deposits/withdraws/votes/CPIs revert, and the legit re-init fails
AccountAlreadyInitialized (the deterministic PDA is squatted) -> the genesis is permanently BRICKED before it
starts. Guard: init validates `policy > POLICY_WITH_SURPLUS -> InvalidInstructionData` (subledger lib.rs:732)
BEFORE create/write. Mutated :732 to disable the policy clause -> `front_running_the_genesis_pool_with_a_bad_policy_is_rejected`
FAILS = mutation-SHARP. That test inits the REAL genesis pool PDA with policy=2 (only the policy wrong), asserts
init refuses + the PDA is UNTOUCHED (not bricked), then inits normally and round-trips a deposit+full exit.
(Pairs with finding-Q: the pool PDA seed includes market_slab+percolator_program so the disabled own-vault
init_pool can't squat the insurance PDA either.) Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — twap reads of the Squads Multisig (config_authority/time_lock offsets)] GB.
Extends the cross-program offset sweep to the OTHER sibling binary (Squads v4, after percolator slab EX/FX/FV/FW).
twap init_config reads the bound multisig's fields at hardcoded offsets into the Squads `Multisig` struct:
disc ms[..8] (DV), config_authority ms[40..72] (the DAO binding, DV/FA), time_lock ms[74..78] (the 1-week floor,
FJ). A drift here binds the wrong DAO (lose governance) or mis-reads the timelock (collapse the depositor exit
window). Mutated the time_lock offset 74->70 -> 10+ chain tests FAIL (every handoff-based test:
`e2e_execute_pulls_only_burn_share_and_ratchets_principal`, `e2e_ratchet_pulls_fresh_surplus...`,
`e2e_completed_squads_execute_cannot_be_replayed`, ...). Strongly mutation-SHARP — every test stands up a REAL
Squads v4 multisig (real binary) with a 1-week timelock, and init_config reads ms[74..78] to validate it >=
MIN_TIMELOCK; a wrong offset reads a different field -> the floor check breaks -> setup_handoff fails -> the
whole suite collapses. NOT masked (FX flavor absent): the real multisig has DISTINCT field values
(create_key, config_authority, threshold, time_lock), so reading the wrong one breaks the binding (no
value-equality to hide behind). The config_authority offset (:40) is likewise pinned by the DAO-binding tests
(DV/FA). So both sibling-program read seams are offset-verified: percolator slab (EX/FX + canaries) and Squads
multisig (GB, functional). Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED BACKSTOPPED — insurance_withdraw foreign-slab bindings (percolator operator check)] GA.
HOSTILE vector (substitute a foreign HEALTHY slab to inflate the pro-rata haircut basis): pass a different
market's slab (2M insurance) as market_slab so payout() reads its inflated insurance instead of the impaired
real market's (500k) -> over-compute owed -> over-pay / drain co-depositors. insurance_withdraw binds
`market_slab.key != pool.market_slab` (:1022) and `vault_authority.key != perc_vault_authority(market_slab,
perc)` (:1029). Mutated EACH to `false` separately -> 42 insurance PASS each = MUTATION-BLIND. Analyzed: NOT a
gap — percolator-backstopped (FL/DS class). The actual fund move is WithdrawInsuranceLimited, invoke_signed by
the POOL PDA against market_slab; percolator requires the signer to be that market's asset-0 OPERATOR. The pool
is the operator of its OWN market only, so a foreign-slab pull is rejected by percolator regardless of the
subledger-side key checks; `foreign_market_slab_cannot_inflate_the_haircut` confirms the attack fails
end-to-end (the inflated read is computed but the CPI reverts -> atomic, no over-pay). So :1022/:1029 are
fail-fast HYGIENE (reject before the wasted CPI). CRITICAL CONTRAST with FX: FX was a REAL gap because reading
the wrong OFFSET of the RIGHT (real, pool-owned) slab yields a wrong value percolator cannot catch (it is the
pool's own market) — no backstop, so the offset MUST be pinned. Here the foreign SLAB is caught by percolator's
operator check, so the binding is redundant. Per KEEP/DELETE: no test for the hygiene bindings (the e2e
foreign-slab test already pins the end-to-end boundary). Verdict: BLOCKED (defense-in-depth). No code/test
change.

### [VERIFIED SAFE — slab insurance u128->u64 width conversion (saturating, not truncating)] FY.
Follow-on to FX (the slab insurance read): percolator's `insurance` is a u128 field, but the subledger reads it
into a u64 for its u64 pro-rata haircut math. A naive `as u64` cast would TRUNCATE (wrap a >u64::MAX value to a
small one -> falsely "impaired" -> under-pay the haircut, LOF). Checked the code: `read_asset0_insurance`
(subledger lib.rs:306) does `u64::try_from(v).unwrap_or(u64::MAX)` — SATURATING, the safe direction: an
insurance above u64::MAX means the pool is super-healthy (insurance >> outstanding), and the saturated u64::MAX
is also >= outstanding, so payout = full principal (no haircut) — identical to the true-value outcome.
Truncation would have done the opposite (wrap -> false impairment). Moreover insurance <= u64::MAX in PRACTICE
(funded by u64 SPL token transfers; the COIN/collateral mint supply is u64), so the saturation never triggers.
twap reads the same field as u128 (no narrowing) because its surplus math is u128 end-to-end (surplus =
insurance - reserved_floor). So both cross-program width seams are safe: subledger saturates (safe for u64
haircut), twap stays u128. Verdict: BLOCKED, no gap. No code/test change.

### [GAP FIXED — subledger slab insurance offset was MASKED (canary pinned offset_of! but not the src const)] FX.
HOSTILE vector (finding-T for the SUBLEDGER side; layout-drift -> over-pay/DOS): the impaired-exit pro-rata
haircut reads asset-0 insurance straight from the percolator slab at `PERC_INSURANCE_OFFSET = 448 + 301 = 749`
(subledger lib.rs:300). The adjacent `vault` field @733 holds TOTAL tokens (insurance + trader capital + pnl);
reading it as "insurance" inflates the haircut basis -> over-computed `owed` (over-pay a withdrawer / drain
co-depositors, or owed>insurance -> WithdrawInsuranceLimited reverts -> exit DOS). Applied the FV/FW offset
lens: mutated :300 749->733 (vault) -> 42 insurance PASS = MUTATION-BLIND. ROOT CAUSE (mask): the impaired
tests' `impair_market` helper sets `off_vault == off_ins == new_insurance` (a consistent loss keeps
percolator's validate_shape happy), so reading vault or insurance yields the SAME value — the functional tests
cannot distinguish the offsets. And the existing canary only asserts `offset_of!(H, insurance) == 301` (pins
the percolator STRUCT layout) — it never referenced the subledger's shipped `PERC_INSURANCE_OFFSET`, so a src
regression of that const passed both the canary and the masked tests. (Contrast twap/EX: its real-market e2e
tests have vault != insurance, so they catch a src drift functionally.) FIX: made `PERC_INSURANCE_OFFSET` pub
and added to the canary `assert_eq!(subledger_program::PERC_INSURANCE_OFFSET, off_ins)` — pinning the SHIPPED
const against the real `offset_of!(insurance)`. Now mutating :300 -> the canary FAILS ("PERC_INSURANCE_OFFSET
drifted ... would read vault as insurance"). insurance 42 + lib 6 green. This is the FOURTH masked gap (cf. EM,
EZ, FN) — here a value-equality (vault==insurance) in the test fixture masked the offset. Verdict: BLOCKED;
src-offset COVERAGE GAP closed.

### [VERIFIED SHARP — gv->subledger position field offsets (principal, start_slot = weight inputs)] FW.
Completes the cross-program byte-offset-integrity sweep (EX twap->percolator slab insurance; FV gv->subledger
pool outstanding; FW gv->subledger position). `read_sub_position` reads the two inputs to vote weight =
floor(log2(now - start_slot)) * principal at hardcoded offsets `principal = data[72..80]` (genesis-vote
lib.rs:218) and `start_slot = data[89..97]` (:219) into the subledger Position struct. Drift here silently
mis-reads governance power: a wrong principal offset fabricates/destroys weight + quorum principal; a wrong
start_slot offset fabricates hold-time (log2 multiplier) or zeroes it. Mutated each: principal 72->80 -> 10+
insurance tests FAIL (incl. `genesis_vote_reads_subledger_position_and_weights` (the weight==10*amount
assertion), `re_voting...double_count`, `topping_up...does_not_inflate`, `trigger_with_a_substituted_low_outstanding_pool...`);
start_slot 89->81 -> 10+ FAIL (incl. the weight test, `a_too_recent_position_cannot_vote`, `cannot_vote_with_a_withdrawn_position`).
Strongly mutation-SHARP both — the e2e suite drives REAL subledger positions (real binary) with known
principal/start_slot through deposit->vote->weight->quorum->seal, so a wrong offset breaks broadly. NET
cross-program offset sweep: every raw byte-offset read of a sibling program's account (twap@insurance EX, gv@pool
FV, gv@position FW) is mutation-pinned to the correct field; EX additionally has an offset_of! canary vs the
external percolator binary, while FV/FW read in-repo subledger structs (coordinated layout) + a disc check.
Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — gv->subledger pool `outstanding` read offset (cross-program quorum denominator)] FV.
HOSTILE vector (cross-program layout drift -> wrong quorum denominator; the finding-T analogue for gv reading
the subledger pool): trigger's quorum is `total_voted_principal*2 > outstanding`, where `outstanding` is read
by `read_sub_pool_outstanding` at a HARDCODED byte offset `data[80..88]` (genesis-vote lib.rs:229) into the
subledger Pool struct. If that offset drifts from the real `outstanding_principal` field (e.g. reads an
adjacent field), the denominator is wrong -> quorum mis-evaluates: too-low reads let a minority finalize (mass
LOF — seize the supply), too-high reads brick finalize (DOS). Mutated the offset 80->72 (adjacent field) ->
FIVE tests FAIL across suites: `trigger_uses_live_pool_outstanding_not_stale_cache`,
`trigger_requires_a_strict_majority_and_quorum_not_a_tie` (seal), `those_who_stay_decide_after_a_nonvoting_majority_forfeits_by_exiting`,
`winning_voter_can_retract_and_exit_after_finalize`, `full_lifecycle_deposit_vote_seal_then_recipient_claims_coin`
(insurance). Strongly mutation-SHARP — the e2e tests drive REAL subledger pools (real binary) with known
outstanding through the genesis lifecycle, so a wrong offset breaks the quorum decision broadly. (Unlike
finding-T/EX which reads the EXTERNAL percolator binary's slab and needs an offset_of! canary, here both gv and
subledger are in THIS repo and a layout change is coordinated; the disc check `data[..8] == SUB_POOL_DISC`
guards type, the e2e suite guards the field offset.) Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED BACKSTOPPED — gv vote pool binding (cache deflation; position-read + trigger-live backstop)] FU.
Anti-mask probe of the gv VOTE path's pool binding (vs DX which pinned TRIGGER's pool binding). Vector: a voter
passes a FAKE low-outstanding sub_pool to vote, deflating the refreshed `config.outstanding_principal` cache so
a later quorum check passes for a minority. Guard: `*sub_pool.key != config.subledger_pool` (genesis-vote
lib.rs:562). Mutated the sub_pool.key clause to `false` -> seal 14 + insurance 42 PASS = MUTATION-BLIND.
Analyzed: NOT a gap — doubly backstopped: (1) POSITION-READ CONSISTENCY — `read_sub_position(data, sub_pool.key,
voter.key)` requires `position.pool == sub_pool.key`; the voter's REAL position carries the REAL pool, so a fake
sub_pool (!= real pool) makes the position read REJECT -> a fake pool cannot even be used at vote time. (2)
TRIGGER LIVE READ — the quorum decision is made at trigger against the LIVE outstanding re-read from the
config-bound pool (DX, mutation-SHARP, :738), NOT the vote-time cache; so even a deflated cache is ignored (the
cache is explicitly a refresh-only hint, per the DX comment "quorum is measured against the LIVE pool, not the
cached value"). So :562 is fail-fast hygiene, backstopped by the position-read field check + the authoritative
trigger live read. The security-relevant pool binding (trigger :738) IS sharp (DX). Per KEEP/DELETE: no test.
Verdict: BLOCKED (defense-in-depth). No code/test change.

### [VERIFIED BACKSTOPPED — insurance_deposit holding binding (DZ-parallel, TopUp SPL authority)] FT.
Completes the holding-substitution map (DZ covered the WITHDRAW holding; this is the DEPOSIT side). Vector:
pass an attacker-owned `holding` to insurance_deposit to capture/misroute a depositor's funds. Flow: user
transfers `amount` from their ATA -> holding (user signs), then TopUpInsurance moves holding -> percolator vault
signed by the POOL PDA. Guard: `hs.mint != pool.mint || hs.owner != *pool_account.key` (subledger lib.rs:863).
Mutated the owner clause to `false` -> 42 insurance PASS = MUTATION-BLIND. NOT a gap — backstopped (the src
comment at :51 says so outright): the TopUpInsurance CPI does spl_transfer(holding -> vault) with the pool PDA
as authority; SPL requires the authority to OWN the source, so an attacker-owned holding makes TopUp REVERT ->
the whole deposit (incl. the user's first transfer) reverts atomically -> no funds move. Plus SELF-SCOPED: the
depositor signs their own deposit and controls the holding arg, so a substituted holding can only fail their
own deposit, never a cross-user capture. Symmetric to DZ (withdraw holding, backstopped by the second
transfer's SPL authority). So both holding bindings are fail-fast HYGIENE, backstopped by SPL-transfer authority
on a pool-PDA-signed CPI; the SECURITY-relevant pool-PDA derivation (expected_pool) is mutation-sharp elsewhere.
Per KEEP/DELETE: no test. Verdict: BLOCKED (defense-in-depth). No code/test change.

### [VERIFIED SHARP — claim sealed-proposal binding blocks loser-drains-winner vault (anti-mask)] FS.
HOSTILE vector (cross-proposal payout redirect / LOF): after the winner seals, a recipient named in a LOSER
proposal claims from the SHARED distribution vault via that loser proposal -> drains COIN owed to the winner's
recipients (the vault holds the single fixed supply; multiple proposals competed for it). claim gates on
`!config.is_sealed() || config.sealed_proposal != *proposal_account.key` (distribution lib.rs:518) — pay only
from the proposal the config actually sealed to. Anti-mask check (could the is_sealed clause shadow the
sealed_proposal bind?): mutated ONLY the `sealed_proposal != proposal` clause to `false` ->
`a_losing_proposal_cannot_claim_the_winners_vault` FAILS = mutation-SHARP. That test seals the WINNER first
(so config.is_sealed() is TRUE and the first clause does NOT fire), then claims via the LOSER proposal -> the
sealed_proposal clause is the SOLE decider. So the cross-proposal redirect is pinned, not masked. (Pairs with
ES seal-irreversibility: ES stops a loser from RE-sealing to redirect the vault; FS stops a loser from CLAIMING
against the real seal. Together: the supply can only flow to the one sealed winner's named recipients.) The
`!is_sealed()` clause (claim-before-seal) is itself redundant with the sealed_proposal bind (sealed_proposal ==
default pre-seal != any real proposal), defense-in-depth. Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED BACKSTOPPED — Position discriminator (pool-as-position type cosplay)] FR.
HOSTILE vector (type cosplay, follow-on from FQ): insurance_withdraw binds the position by DATA + subledger-
ownership (no position-PDA derivation, FQ), so the only thing distinguishing a position from a sibling
subledger-owned account (a POOL) is the discriminator. Attack: pass a POOL account as the "position" to read
attacker-favorable bytes. Position::deserialize enforces `data[..8] != POSITION_DISC ("SUBPOS01")` (subledger
lib.rs:230); POOL_DISC is "SUBPOOL1" (distinct). Mutated :230 to drop the disc clause -> 42 insurance PASS =
MUTATION-BLIND. Analyzed: NOT a gap — backstopped, no type confusion possible: (1) FIELD-BINDING — even reading
a pool as a Position, line :1039 requires `position.owner == owner.key && position.pool == pool_account.key`;
a pool's bytes at the position offsets are `position.owner = pool[40..72]` (asset_id||vault — cannot equal the
attacker's key) and `position.pool = pool[8..40]` (the pool's mint — cannot equal the pool's own address), so
:1039 rejects. (2) WRITE-MONOPOLY — only the subledger program writes subledger-owned accounts, and it writes
positions with POSITION_DISC / pools with POOL_DISC; an attacker cannot forge a subledger-owned account with
attacker-controlled position fields under a wrong disc. So :230 is type-safety hygiene, backstopped by the
field check (:1039, mutation-SHARP per FQ) + the write monopoly. Per KEEP/DELETE: no test for the
hygiene/disc check. Verdict: BLOCKED (defense-in-depth). No code/test change.

### [VERIFIED SHARP — insurance_withdraw owner-key binding is the SOLE anti-theft guard (not masked)] FQ.
Anti-mask probe of insurance_withdraw's cross-user theft guard. Hypothesis: the owner-key data check
`position.owner != *owner.key` (subledger lib.rs:1039) might be MASKED by a position-PDA derivation (as the
attacker-grant test signs as `owner` and passes the victim's position, a `position_account.key != derive(pool,
owner)` check would reject FIRST). Checked the code: insurance_withdraw derives ONLY the pool PDA (expected_pool)
and does NOT derive/key-check the position — the position is bound SOLELY by the line-1039 DATA check
(position.owner == signing owner AND position.pool == pool) plus position_account.owner == program_id
(subledger-owned, so the data is trustworthy). So :1039 is the SOLE anti-theft guard, not a redundant sibling.
Confirmed sharp: `a_non_owner_cannot_withdraw_a_victims_insurance_principal` has the attacker SIGN as account-0
`owner` (is_signer passes) while passing the VICTIM's position -> only `position.owner(victim) != owner.key(attacker)`
rejects. Mutated :1039 owner clause to `false` -> that test AND `principal_only_owner_exit_returns_funds_and_guards`
FAIL = mutation-SHARP. So the cross-user insurance-theft boundary is the sole-decider case and properly pinned;
the FN failure mode (sibling masks the key bind) does NOT occur here because there is no position-PDA sibling.
(Net signer/key+owner anti-mask sweep: FN set_vote_lock masked->fixed; FO require_squads_vault, FP seal_winner,
FQ insurance_withdraw owner — all per-clause/sole-decider sharp.) Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — distribution seal_winner authority pair NOT masked (anti-mask sweep)] FP.
Third signer+key pair in the FN/FO anti-mask sweep: distribution `seal_winner` gates the seal on BOTH
`!authority.is_signer` (lib.rs:460) and `*authority.key != config.authority` (:467). Risk (the FN failure
mode): a test naming the real authority WITHOUT its signature trips is_signer (:460), masking the key bind
(:467) — which would let a non-authority seal a loser proposal and claim the whole COIN supply. Checked: NOT
masked. The tests split the two attacks cleanly — `seal_rejects_naming_the_authority_without_its_signature`
names the REAL authority NOT signing (pins :460), while the imposter-seal in `seal_then_recipients_claim_their_entries`
(env.seal by an IMPOSTER keypair that DOES sign) makes is_signer pass so ONLY :467 rejects. Mutated :467 to
`if false && ...` -> `seal_then_recipients_claim_their_entries` FAILS = mutation-SHARP (the signing imposter is
the sole-decider case for the key bind). seal_winner is reachable only via gv trigger (the gv config PDA signs
via invoke_signed), so the direct-call attacker can neither sign as the gv PDA (fails :460 if named) nor match
config.authority with their own key (fails :467 if signing) — both covered. NET of the signer+key anti-mask
sweep: set_vote_lock MASKED -> fixed (FN); require_squads_vault (FO) + seal_winner (FP) per-clause sharp. The
masked case was the lone outlier; the chain.rs/distribution tests otherwise use single-violation tests per
clause. Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — require_squads_vault key binding NOT masked (anti-mask of the keystone Squads gate)] FO.
After FN (a signer+key-binding pair where the test passed a NON-signing key and masked the key check), audited
the keystone gate for the SAME failure mode: `require_squads_vault` (twap lib.rs:840) — used by reconfigure /
set_reserve / set_reserved_floor / set_coin_sink / set_bid_fee / shutdown — does `if !squads_vault.is_signer {
reject }` (:842) then `if *squads_vault.key != squads_default_vault(config.squads_multisig) { reject }` (:844).
Risk: a test naming the vault WITHOUT its signature is rejected by is_signer (:842), masking the key bind
(:844). Checked: NOT masked. The tests deliberately split the two — `e2e_reconfigure_rejects_a_non_signing_or_forged_vault`
has ATTACK 1 (real vault, NOT signing -> pins :842) and ATTACK 2 (attacker SIGNS as their own key -> is_signer
passes, ONLY :844 rejects). Mutated :844 to `if false && ...` -> THREE tests FAIL
(`e2e_attacker_cannot_lower_the_reserve_without_squads`, `e2e_shutdown_sweeps_holding_only_via_squads`,
`e2e_dao_flips_burn_to_buyback_only_via_squads`), each passing a signing-but-wrong-key vault so :844 is the
sole decider. Mutation-SHARP, per-clause covered. So the keystone DAO->TWAP gate is well-constructed (single-
violation tests per clause), in contrast to FN's set_vote_lock where the only self-unlock test tripped
is_signer first. (Net of the FN/FO sweep of signer+key pairs: subledger set_vote_lock was masked -> fixed FN;
twap require_squads_vault is per-clause sharp -> FO.) Verdict: BLOCKED, no gap. No code/test change.

### [GAP FIXED — set_vote_lock vote_authority binding was MASKED (owner self-unlock -> ballot outlives capital)] FN.
HOSTILE vector (owner self-unlocks a live-voted position to exit while the ballot still counts = finding B,
ballot outlives capital -> a free, capital-less ballot inflates quorum/majority): set_vote_lock (subledger IX 6)
toggles a position's vote-lock and is reachable directly (public ix). It requires vote_authority.is_signer
(lib.rs:16/CP), owner.is_signer (:26), AND `pool.vote_authority != *vote_authority.key -> IllegalOwner` (:1186)
— the last being the SOLE guard stopping the OWNER from naming THEMSELVES as the vote_authority and unlocking
their own position. Applying the EZ anti-mask lens: mutated :1186 -> 42 insurance PASS = MUTATION-BLIND, despite
`owner_cannot_self_unlock_a_live_vote_to_exit_capital`. ROOT CAUSE (mask): that test's attack names the gv
config as vote_authority but WITHOUT its signature, so it is rejected by the is_signer check (:16) — :1186 is
never the decider, and dropping it changes nothing. The key binding was effectively untested. FIX: augmented
the test with ATTACK 2 — alice names HERSELF as the vote_authority AND signs, so is_signer PASSES and ONLY
:1186 (pool.vote_authority(gv PDA) != alice) stands between her and a self-unlock; asserts it is refused, the
position stays locked, and the capital still cannot exit. Mutation-sharp: PASSES with :1186, FAILS without.
insurance 42 (augmented, not +1). This is the THIRD masked-by-a-sibling-guard gap (cf. EM cancel-replay, EZ
solvency) — each time a stricter sibling (is_signer here, supply-equality in EZ) shadowed the real guard because
the test's input tripped the sibling first. Verdict: BLOCKED; self-unlock COVERAGE GAP closed.

### [VERIFIED SHARP — register_proposal non-empty check blocks empty-proposal finalize-brick DOS] FM.
HOSTILE vector (finalize-brick DOS via an empty winning proposal): register an EMPTY distribution proposal
(entry_count == 0, created but never appended) for voting. If it could be backed and won the strict majority,
trigger's seal_winner rejects `entry_count == 0` (distribution lib.rs) -> the WINNING proposal can never be
sealed -> genesis finalize permanently BRICKED (a strict-majority winner blocks any other proposal from
winning, and this one is unsealable). Secondary harm: registering empty freezes a (0,0) snapshot, and the
creator's later append makes the live proposal mismatch the snapshot forever (DA bait-and-switch brick).
Defense: register_proposal refuses `entry_count == 0` (genesis-vote lib.rs:476) so only a FULLY-built proposal
becomes votable. Mutated :476 to `if false && ...` -> `register_rejects_an_empty_proposal` FAILS =
mutation-SHARP (the test registers a created-but-never-appended proposal and asserts refusal + no gv
proposal-vote account created). Pairs with: seal_winner's entry_count==0 backstop, the foreign-config register
reject (:460), creator-only registration (DD), snapshot anti-bait-and-switch (DA), seal-finality (ES) — the
register/seal lifecycle rejects every malformed/foreign/empty/mutated proposal before it can brick finalize.
Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED BACKSTOPPED — twap accept_operator handoff (percolator two-signature gate)] FL.
HOSTILE vector (non-DAO/non-timelock signer hijacks the insurance operator grant): drive twap accept_operator
(IX 3) directly to grant the percolator asset-0 insurance OPERATOR to an attacker-chosen authority, bypassing
the Squads timelock. accept_operator binds `squads_vault == squads_default_vault(config.squads_multisig)`
(lib.rs:555) and `twap_authority == config-derived PDA` (:569), then CPIs percolator UpdateAssetAuthority(new =
twap_authority). Mutated BOTH bindings to `false` separately -> 75 chain PASS each = MUTATION-BLIND. Analyzed:
percolator-backstopped, no hijack possible: the UpdateAssetAuthority CPI requires TWO signatures — (1) the
CURRENT asset_admin (the canonical Squads vault) must sign, which is only obtainable via a timelock'd Squads
vault_transaction_execute (an attacker cannot make a Squads PDA sign); and (2) the NEW authority must sign,
which the program provides via invoke_signed over seeds derived from config_account (:28) — so the grantee is
ALWAYS the config-derived twap_authority regardless of the passed key, and a mismatched key is left unsigned ->
percolator rejects. So :555/:569 are fail-fast HYGIENE; the real gate is percolator's two-signature requirement
+ the Squads-vault-must-sign timelock. Coverage: `e2e_attacker_cannot_grant_operator_bypassing_squads` drives a
FORGED asset_admin against the REAL percolator binary and asserts the grant is refused (the genuine end-to-end
boundary). The same shape holds for the subledger accept_operator (its own e2e). This is the DS/CB class
(percolator-backstopped; twap-side key checks are hygiene, untestable-sharp without a forged percolator). Per
KEEP/DELETE: no test for the hygiene bindings. Verdict: BLOCKED (defense-in-depth; two-sig CPI + timelock). No
code/test change.

### [VERIFIED SHARP — fully-impaired exit retires without a zero-amount CPI (unretireable-position DOS)] FK.
HOSTILE vector (stuck/unretireable position DOS + lingering quorum phantom): when a depositor's insurance is
FULLY impaired (pro-rata `owed == 0`, the market drew insurance to ~0), their exit pays nothing but MUST still
retire the position (principal -> 0, leaves `outstanding`). percolator's WithdrawInsuranceLimited REJECTS a
zero-amount withdraw, so insurance_withdraw guards the CPI with `if owed > 0 { invoke_signed(WithdrawInsuranceLimited) }`
(subledger lib.rs:1081) and does the accounting/retire regardless. If that skip were removed (CPI always
called), a fully-impaired exit would hit percolator's zero-amount rejection -> the whole withdraw REVERTS ->
the position can NEVER be retired: it stays in `outstanding_principal` forever as a phantom, permanently
inflating the gv quorum denominator (`total_voted*2 > outstanding`) so honest voters can never reach quorum =
a genesis-finalize DOS, and the depositor's dead position is frozen. Mutated :1081 `if owed > 0` -> `if true`
-> `a_fully_impaired_exit_still_retires_the_position_without_a_zero_amount_cpi` FAILS = mutation-SHARP. (Pairs
with the pro-rata haircut ER: ER bounds the payout under partial impairment, FK handles the total-impairment
edge so the position always clears.) Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — 1-week Squads timelock depositor-protection window (enforce + floor)] FJ.
HOSTILE vector (skip/collapse the 1-week timelock -> remove the depositor exit window): the whole
DAO->Squads->TWAP->insurance model depends on a 1-week delay on every privileged action, giving depositors time
to exit before a hostile reconfigure/floor-lower lands. Two ways to defeat it: (a) EXECUTE a vault transaction
BEFORE the week; (b) BIND a config to a multisig whose on-chain time_lock is 0/short so there is no delay.
Verified BOTH are blocked + tested end-to-end vs the REAL Squads v4 binary: (a) ENFORCEMENT —
`reconfigure_only_via_squads_vault_execute_after_timelock` creates a real 1-week multisig, proposes+approves a
reconfigure, asserts `vault_transaction_execute` is REJECTED before the week (bps unchanged), warps
clock.unix_timestamp past TIMELOCK_1_WEEK_SECS+1, then asserts it SUCCEEDS (bps changes) — both directions
proven; the operator handoff (accept_operator) is noted gated the same way. (b) FLOOR — init_config reads the
bound multisig's `time_lock` (u32 @ ms[74..78]) and refuses `< MIN_TIMELOCK_SECS` (twap lib.rs:410); mutated
:410 to `if false && ...` -> `twap_config_rejects_a_multisig_below_the_one_week_timelock` (a multisig ONE SECOND
under a week, else correct) FAILS = mutation-SHARP. So the window cannot be skipped (Squads enforces the delay)
NOR configured away (the floor rejects short timelocks); pairs with the require_squads_vault gate (every
privileged ix routes through the timelock'd vault). Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP + DOUBLY-DEFENDED — reconfigure bps over-pull (floor-breach principal drain)] FI.
HOSTILE vector (DAO sets bps>100% -> over-pull breaches the floor into depositor principal): execute computes
`burnable = surplus * surplus_buy_burn_bps / 10_000` (twap lib.rs:1376) and pulls that from insurance. If
buy_burn_bps > 10_000 (>100%), burnable > surplus -> the WithdrawInsuranceLimited reaches BELOW reserved_floor
into protected depositor principal = a slow LOF, armed by a single reconfigure. Guard A (set-time, sharp):
process_reconfigure rejects `new_bps > BPS_DENOMINATOR (10_000)` (:473) — the DAO can set 0..=100% only.
Mutated :473 to `if false && ...` -> `reconfigure_rejects_a_bps_above_the_denominator_that_would_overpull_the_floor`
FAILS = mutation-SHARP (its test directly asserts the reconfigure is refused, so :473 is the sole decider).
Guard B (execute-time backstop): even if a bad bps were stored, execute computes `retained =
surplus.checked_sub(burnable)` (:1379); bps>10_000 makes burnable>surplus -> checked_sub UNDERFLOWS ->
ArithmeticOverflow -> execute REVERTS before any pull. So the floor-breach is doubly-blocked (set-time reject +
execute underflow-revert), and pairs with finding-O (single-pull floor) + EU (cross-round ratchet) to fully
bound the insurance pull. (Reconfigure is itself Squads-timelock'd — a third layer.) Verdict: BLOCKED, no gap.
No code/test change.

### [VERIFIED CLOSED — finding-AI prefund-DOS surface across ALL on-demand PDAs] FH.
Closed the finding-AI (lamport-prefund/dust DOS on a deterministic PDA) class across EVERY on-demand account
creation, incl. the candidate-suppression vector: an attacker dusts a RIVAL's gv_proposal PDA
["gv_proposal", gv_config, dist_proposal] before register_proposal to prevent it becoming votable (suppress a
competing proposal in the winner-take-all vote), or dusts a dist_proposal PDA before create_proposal. Verified
every creation uses a prefund-ROBUST helper (top up only the deficit + allocate/assign, NOT create_account):
genesis-vote `create_pda` (:306) used by config(:404), PROPOSAL(:492), ballot(:593); distribution
`create_pda_robust` (:225) used by config(:324), PROPOSAL(:381); subledger `create_pda_robust` (:674) used by
pool + position(:75). Dev comments at gv:300 / dist:220 explicitly cite "create_account aborts with
AccountAlreadyInUse on ANY pre-existing lamports". HELPER-LEVEL coverage is proven by mutation: FF gated gv
create_pda's top-up -> ONLY the ballot-dust test failed; FG gated subledger create_pda_robust's top-up ->
position-dust + pool-init tests failed; config-dust is exercised by `seal.rs:684` (dusted init) +
`distribution.rs:1010` (proposal registers under a dusted-then-inited config). Since the PROPOSAL call sites
share these helpers, the candidate-suppression dust is blocked + helper-covered; a 3rd/4th near-identical
proposal-dust test would be REDUNDANT (same helper, same mutation) -> per KEEP/DELETE, none added. Net: the
prefund-DOS surface is robust-by-construction at all 7 call sites and mutation-covered at the helper level for
all 3 helpers. Verdict: BLOCKED, no gap. No code/test change.

### [COVERAGE ADDED — position-PDA lamport-prefund cross-user EXCLUSION DOS (deposit call site)] FG.
HOSTILE vector (targeted depositor exclusion — higher-stakes sibling of FF): the subledger position PDA
["subledger_position", pool, owner] is deterministic, so an attacker can DUST a VICTIM's position with lamports
BEFORE they ever deposit. A naive create_account fails on a prefunded account -> the victim can NEVER open a
position -> totally excluded from the genesis (no capital at risk => no vote, no weight, no claim). This is the
FIRST gate, before voting even matters, and the attack is CROSS-USER (dust the victim's PDA, not your own).
insurance_deposit creates the position via create_pda_robust (subledger lib.rs:75), which is prefund-robust
(top up only the deficit `if current < required` + allocate/assign, NOT create_account). COVERAGE GAP at THIS
call site: the helper's robustness was pinned only via the POOL-init path (`lamport_prefund_cannot_brick_insurance_pool_init`)
and the BALLOT path (FF) — the deposit/position call site + the cross-user depositor-exclusion threat had no
test. FIX: added `dusting_a_depositors_position_pda_cannot_block_their_deposit` — dusts alice's position PDA
with 1 lamport before she deposits, asserts the deposit lands, the position is subledger-owned, and her full
principal is recorded (not excluded). Mutation-sharp: gating the top-up to `if current == 0` fails THIS test
(and the pool-init one), passing the other 40. KEPT: pins a distinct call site + a distinct (cross-user
targeted-exclusion) threat the pool-init/ballot tests don't cover. insurance 41->42. Verdict: BLOCKED; deposit
-path exclusion-DOS coverage added.

### [VERIFIED SHARP — ballot-PDA lamport-prefund (finding-AI) disenfranchisement DOS resistance] FF.
HOSTILE vector (permissionless disenfranchisement DOS): the gv ballot PDA `["gv_ballot", config, voter]` is
deterministic, so an attacker can DUST it (send lamports) before a victim ever votes. A naive
`system_instruction::create_account` FAILS on an account that already holds lamports -> the victim could NEVER
create their ballot -> permanently unable to vote (targeted disenfranchisement; at scale, suppress a faction to
swing the genesis). Defense (finding-AI): gv `create_pda` (lib.rs:306) is prefund-robust — it tops up only the
DEFICIT `if current < required { transfer(required - current) }` (the conditional also guards an over-dust
underflow of `required - current`) and then `allocate` + `assign` via invoke_signed (which succeed on a
prefunded system-owned, zero-data account), NOT create_account. vote only creates when `data_len() == 0`
(:587). Mutated the top-up condition `if current < required` -> `if current == 0` (fund only empty accounts) ->
EXACTLY ONE test fails: `dusting_a_voters_ballot_pda_cannot_block_their_vote` (the 1-lamport-dusted ballot is no
longer topped to rent-exemption -> allocate on a non-exempt account -> tx fails), while all 40 other
(fresh-ballot) vote tests PASS. So the mutation precisely isolates the dust path: mutation-SHARP, and the test
genuinely pins the robust top-up (not masked by the fresh-ballot path). Verdict: BLOCKED, no gap. No code/test
change.

### [VERIFIED BACKSTOPPED — burn_unclaimed vault/mint binding (SPL-burn-authority, no LOF)] FE.
Anti-mask probe of burn_unclaimed's `*vault.key != config.vault || *coin_mint.key != config.coin_mint` bundle
(distribution lib.rs:593). Mutated the VAULT clause to `false` -> 20 dist PASS = MUTATION-BLIND. Analyzed: NOT
a gap — backstopped hygiene (EW class). burn_unclaimed torches the vault's remaining COIN after the claim
window; the burn CPI is invoke_signed by the config PDA (seeds = config.coin_mint+authority). SPL burn requires
the AUTHORITY to own the burned account, and the config PDA is the authority of ONLY config.vault. So a
substituted vault is either (a) not config-PDA-owned -> burn fails on authority mismatch (incl. another
distribution config's vault, whose authority is a DIFFERENT PDA), or (b) the attacker's OWN config-PDA-... which
they cannot create. Moreover burning is DEFLATION by design (unclaimed COIN is meant to burn) -> even a
successful substitute burn moves no funds to anyone = no LOF, and `remaining = token_balance(vault)` of an
empty substitute burns 0. The mint clause is also SPL-backstopped (burn checks vault.mint == mint, EW). The
SECURITY-relevant guards on this path ARE pinned: the claim-window gate (DK, :601 — can't burn before the
window) and is_sealed. Per KEEP/DELETE: no test for the SPL-backstopped binding. Verdict: BLOCKED
(defense-in-depth; SPL burn authority + deflation-not-transfer). No code/test change.

### [VERIFIED SHARP — gv weight==0 guard blocks zero-weight quorum pumping] FD.
HOSTILE vector (quorum manipulation bypassing time-weighting Sybil resistance): gv BACK computes weight =
floor(log2(hold))*principal; a JUST-deposited position (hold < 2, last-write-time) or a withdrawn one
(principal 0) yields weight 0. The vote then does support_weight += 0 (no majority effect) BUT
total_voted_principal += principal — pumping the QUORUM NUMERATOR (`total_voted_principal*2 > outstanding`) with
capital that has NO hold time. So an attacker could deposit a large principal and IMMEDIATELY vote to push
quorum without earning time-weight, defeating the Sybil-resistance premise. Guard: `if weight == 0 { reject }`
(genesis-vote lib.rs:638) refuses the vote entirely before any tally update. Mutated :638 to `if false && ...`
(allow zero-weight votes) -> TWO tests FAIL: `a_too_recent_position_cannot_vote_or_pump_the_quorum` (the exact
quorum-pump — a fresh deposit cannot vote/pump) AND `cannot_vote_with_a_withdrawn_position` (principal 0 ->
weight 0 -> a retired position cannot re-vote). Mutation-SHARP. (Pairs with EQ last-write-time start_slot reset:
EQ stops accruing hold via top-up; FD stops a zero-hold position from voting at all — together the time-weight
cannot be shortcut.) Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — claim SETTLED-slot guard blocks destroying a victim's unsettled bid (anti-mask)] FC.
HOSTILE vector (permissionless claim destroys a victim's bid — cross-user LOF/grief): twap `claim` is
permissionless (any cranker). An UNSETTLED slot (OPEN book) has usd_owed=0, coin_refund=0 (set at place_bid).
If claim allowed an unsettled slot, a cranker could claim a VICTIM's live bid -> pay 0 but ZERO the slot
(DN clearing) -> the victim's escrowed COIN stays in the shared coin_escrow with NO slot tracking it ->
unrecoverable (no slot to claim or cancel): a direct cross-user LOF + book grief. Guard: claim requires
`d[o+SL_OCCUPIED] != 1 || d[o+SL_SETTLED] != 1 -> reject` (lib.rs:1607); the SETTLED!=1 clause blocks claiming
a live unsettled bid. Anti-mask check (could the OCCUPIED clause shadow it?): mutated ONLY the SETTLED!=1
clause to `false` -> THREE tests FAIL (`e2e_roll_with_a_marginal_zero_coin_fill_leaves_no_phantom_claim`,
`e2e_bid_cannot_be_cancelled_only_evicted_by_a_better_bid`, `e2e_roll_with_committed_bid_settles_correctly_next_round`),
each claiming an OCCUPIED-but-unsettled slot so SETTLED!=1 is the SOLE decider (not masked by OCCUPIED!=1).
Mutation-SHARP. (Companion: cancel handles the unsettled exit path with its cooldown; claim handles only
settled slots — the two are disjoint by the SETTLED flag.) Verdict: BLOCKED, no gap. No code/test change.

### [SPLIT VERDICT — zero-param init guard: claim_window clause SHARP, total_supply==0 clause backstopped-hygiene] FB.
Anti-mask sweep of the bundled `if total_supply == 0 || claim_window_slots == 0 { reject }` (distribution
lib.rs:276). Mutated EACH clause separately. (1) claim_window==0 clause -> `init_config_rejects_a_zero_claim_window`
FAILS = mutation-SHARP (real recipient-LOF DOS: window_end = seal_slot+0 = seal_slot -> claim refused the
instant the winner seals, burn_unclaimed then torches the WHOLE vault; the test uses build(window=0,supply=100)
so it violates ONLY this clause -> sole decider). (2) total_supply==0 clause -> 20 dist PASS = MUTATION-BLIND.
Analyzed: NOT a gap — backstopped hygiene. A total_supply=0 config requires a 0-supply MINT (the
`mint.supply != total_supply` tie :304), so it can never be the genesis COIN (fixed supply > 0); and such a
config is USELESS — append rejects zero-amount entries (so no entry can ever be added under a 0 cap), and
seal_winner rejects `entry_count == 0` (empty proposal unsealable). So a 0-supply config moves no funds and can
brick only itself (the creator wastes rent) — no LOF, no cross-user DOS. This is the EW class (fail-fast
hygiene, downstream-backstopped), DISTINCT from EZ where solvency was the SOLE guard against a real
underfunded-vault LOF. Per KEEP/DELETE: no test added for the hygiene clause; the LOF clause (claim_window) is
already pinned. Verdict: BLOCKED (claim_window sharp; total_supply==0 backstopped). No code/test change.

### [VERIFIED SHARP — init_config validation bundles are per-clause sharp (anti-mask sweep post-EZ)] FA.
After EZ (a guard masked by a sibling `if` triggering first), swept the multi-clause init_config validation
bundles for the same failure mode — a bundled `if A || B || C` where existing tests violate several clauses at
once, so dropping any one is masked by another. (1) gv init_config distribution_config bundle (genesis-vote
lib.rs:377-381 owner||len||disc||coin_mint||authority): EY pinned the authority clause (:380); mutated the
COIN_MINT clause (:379) -> `init_config_rejects_a_distribution_not_authority_bound_to_this_config` FAILS =
sharp. That test carries SEPARATE sub-assertions (one foreign config wrong only in coin_mint, another only in
authority), so each clause is independently the sole decider — well-constructed, NOT masked. (2) twap
init_config multisig bundle (owner :385 DV / disc :401 / config_authority :404 / timelock :410): each has its
own single-violation test — mutated the config_authority DAO-binding (:404) ->
`twap_config_binds_only_to_a_real_squads_multisig_controlled_by_the_dao` FAILS = sharp (the test's multisig has
the correct owner/disc/timelock but a non-DAO config_authority, so :404 is the sole decider); timelock pinned
by `twap_config_rejects_a_multisig_below_the_one_week_timelock`, owner by the DV cosplay test. Conclusion: the
init bundles use single-violation tests per clause, so they are genuinely per-clause sharp — EZ's cross-`if`
supply-vs-solvency mask was a one-off (two separate `if`s with overlapping test input), not a systemic bundle
pattern. Verdict: BLOCKED, no gap. No code/test change.

### [GAP FIXED — distribution solvency check was MASKED (underfunded-vault claim-race LOF)] EZ.
HOSTILE vector (underfunded vault -> claim-race LOF): distribution init_config promises `total_supply` COIN;
seal only enforces `total_amount <= total_supply`, so if the vault holds LESS than total_supply, the entries
can sum above the vault balance -> early claimants drain it, honest late claimants stranded. Guards:
supply-equality `mint.supply != total_supply` (lib.rs:304) AND solvency `vault_state.amount < total_supply`
(:318). Applying the EM anti-mask lens: mutated :318 (drop solvency) -> 19 dist PASS = MUTATION-BLIND, despite
a dedicated test `init_config_rejects_an_underfunded_vault`. ROOT CAUSE (mask): that test mints only 60 and
promises 100, so its rejection actually comes from the SUPPLY-EQUALITY check (:304, 60 != 100) — :318 is never
the deciding guard, so dropping it changes nothing. The solvency check was effectively untested. FIX: added
`init_config_rejects_a_vault_underfunded_below_a_fully_minted_supply` — mints the FULL 100 supply (mint.supply
== total_supply == 100) but seeds only 60 into the vault (40 to a decoy held outside), so :304 PASSES and ONLY
:318 stands between the underfunded vault and the LOF. Asserts init is rejected + no config PDA created.
Mutation-sharp: PASSES with :318, FAILS without. dist 19->20. (Companion sharp guard :304 caught by
`init_config_rejects_a_mintable_coin`; together they prove every COIN that exists is in this vault AND the
vault holds the full promised supply.) Verdict: BLOCKED; solvency COVERAGE GAP closed. This is the SECOND
masked-by-a-sibling-guard gap (cf. EM cancel-double-refund) — the anti-mask lens remains productive.

### [VERIFIED SHARP — gv init_config dependency back-bindings (anti-squat / anti-poison genesis wiring)] EY.
HOSTILE vector (front-run squat / genesis poisoning): the gv config PDA seed is ["gv_config", coin_mint,
subledger_pool] (finding R) — distribution_config is NOT in the seed but a stored field. An attacker front-runs
init_config (right coin_mint+pool) wiring their OWN distribution_config (so votes seal THEIR distribution and
they control the COIN payout), or a poisoned subledger_pool whose vote_authority isn't this config (bricking
votes, finding G/H). Defense: init_config binds every dependency BACK to this config PDA (genesis-vote
lib.rs:374-398): distribution_config must be distribution-owned + DIST_CONFIG_DISC + its coin_mint(dc[8..40]) ==
this coin + its seal authority(dc[72..104]) == `expected` (the gv config PDA); subledger_pool must be
subledger-owned + SUB_POOL_DISC + its vote_authority(sp[160..192]) == `expected`. So a foreign distribution
(authority != this PDA) or foreign pool can never be wired in. Mutated the distribution authority binding
(:380) -> `init_config_rejects_a_distribution_not_authority_bound_to_this_config` +
`gv_config_cannot_be_bound_to_a_substituted_pool` FAIL; mutated the pool vote_authority binding (:395) ->
`init_config_rejects_pool_not_bound_to_this_config` FAIL. Both mutation-SHARP. Combined with finding-R (pool in
the PDA seed) and the AccountAlreadyInitialized reinit guard (:360), the genesis wiring is tamper-proof: every
dependency must point back to the gv config PDA. Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP (doubly-pinned) — finding-T insurance slab offset] EX.
HOSTILE vector (finding-T / finding-O failure class — drain trader+depositor capital as "surplus"): execute
reads the market's asset-0 insurance straight from slab bytes at `INSURANCE_OFFSET = 448 + 301 = 749` (twap
lib.rs:257) to compute `surplus = insurance - reserved_floor`. The ADJACENT `vault` field at slab 733
(448+285) holds TOTAL tokens (insurance + trader capital + pnl); if the offset pointed there, the surplus pull
would treat live trader/depositor capital as withdrawable surplus -> mass LOF. Mutated the src offset to 448 +
285 (vault) -> FOUR tests FAIL: `e2e_execute_pulls_nothing_when_insurance_below_floor` (finding-O floor — vault
reads higher than insurance, breaking the below-floor no-pull), `e2e_roll_with_committed_bid_settles_correctly_next_round`,
`e2e_roll_does_not_unlock_cancel_before_aging`, `e2e_roll_with_a_marginal_zero_coin_fill...`. Mutation-SHARP.
DOUBLY-PINNED: (1) the static canary `insurance_offset_matches_real_percolator_slab` (chain.rs:1390) asserts
the value == `448 + offset_of!(MarketGroupV16HeaderAccount, insurance)` against the REAL percolator struct AND
that it differs from `offset_of!(.., vault)` — catches a percolator LAYOUT drift; (2) the e2e execute/roll tests
catch a SRC constant change (wrong offset -> wrong surplus -> wrong pull/floor behavior). Verdict: BLOCKED, no
gap (finding-T offset pinned both structurally and functionally). No code/test change.

### [VERIFIED DOUBLY-DEFENDED — set_coin_sink wrong-mint sink (fail-fast hygiene, SPL-backstopped)] EW.
HOSTILE vector (misconfig DOS): set a SEND-mode coin_sink that is NOT a coin-mint account -> execute's SEND
transfer can never succeed -> the book is bricked in SEND mode (bidders' COIN stuck). set_coin_sink validates
`s.mint != book.coin_mint -> reject` at set-time (twap lib.rs:1044) AND init_book has the same check. Mutated
:1044 to `if false && ...` -> 75 chain PASS: MUTATION-BLIND (no wrong-mint-sink test). BUT doubly-defended,
low-severity, NO LOF: (1) DAO-GATED — set_coin_sink/init_book are Squads-vault timelock'd, so only a DAO
misconfiguration can reach it (not permissionless). (2) SPL-BACKSTOPPED at execute — a wrong-mint sink makes
`spl_transfer(coin_escrow -> sink, total_coin)` fail on SPL's source.mint==dest.mint requirement -> execute
REVERTS, no funds move, COIN stays escrowed (claimable/cancellable). (3) RECOVERABLE — the DAO re-sets a valid
sink and the book settles. So :1044 is fail-fast HYGIENE (catch at set, not at execute), not the fund-safety
guard. KEY CONTRAST that justifies NOT testing it: the SIBLING self-loop check (`coin_sink == coin_escrow`,
:1040, finding AS) IS the sole guard and IS tested (`e2e_send_sink_cannot_be_the_coin_escrow`) precisely
because a sink==escrow transfer is SAME-MINT -> SPL SUCCEEDS as a no-op -> COIN silently STRANDED forever (SPL
does NOT backstop it). The wrong-mint case is SPL-backstopped, the self-loop is not — so the coverage asymmetry
is correct. A wrong-mint test would pin only fail-fast timing (marginal). Per KEEP/DELETE: no test added.
Verdict: BLOCKED (defense-in-depth; SPL mint-match + DAO-gate + recoverable). No code/test change.

### [VERIFIED SHARP — insurance_deposit outstanding increment (quorum denominator integrity)] EV.
HOSTILE vector (deflated quorum denominator -> minority seizes supply): gv quorum is
`total_voted_principal*2 > pool.outstanding_principal`. If insurance_deposit did NOT increment
`outstanding_principal` on each deposit, the denominator would stay artificially low while positions (and thus
votable principal) grow — letting a MINORITY clear quorum against a deflated outstanding and trigger
winner-take-all. Verified the deposit increments BOTH `pool.outstanding_principal += amount` (subledger
lib.rs:951-954) and `position.principal += amount` (:955-958), via checked_add. Mutated the outstanding
increment to `.checked_add(0)` -> 10+ tests FAIL across the suite (`cannot_over_withdraw_to_drain_a_codepositor`,
`impaired_insurance_exit_is_pro_rata`, `vote_locked_principal_cannot_exit_until_retracted`,
`insurance_pool_cannot_be_reinitialized_after_funding`, `a_non_owner_cannot_withdraw_a_victims_insurance_principal`,
the full deposit/withdraw/vote lifecycle). Strongly mutation-SHARP. This completes the subledger accounting
integrity that feeds the gv quorum: deposit outstanding increment (EV) + withdraw outstanding decrement (DR) +
principal decrement (EO) + over-withdraw cap (CZ) — every counter both directions is pinned. Verdict: BLOCKED,
no gap. No code/test change.

### [VERIFIED SHARP — execute reserved_floor ratchet prevents slow re-pull drain of principal] EU.
HOSTILE vector (slow multi-round drain of depositor principal): execute splits surplus 80/20 — pulls the
burnable share into the holding and RATCHETS the retained share into the principal counter `reserved_floor +=
retained` (lib.rs:1413-1416), so the retained 20% stays in percolator insurance AND is reclassified as
protected principal (next round's surplus = insurance - the NEW higher floor = only newly-accrued insurance).
If the ratchet were skipped, `reserved_floor` would stay flat while the retained share remained classified as
"surplus" (`surplus = insurance - reserved_floor`), so the SAME retained dollars become re-pullable every
subsequent execute — repeated cranks would ratchet nothing and progressively pull the entire insurance,
INCLUDING depositor principal below the intended floor: a slow LOF. Mutated the ratchet to `.checked_add(0)`
(no-op) -> FOUR tests FAIL: `e2e_ratchet_pulls_fresh_surplus_across_rounds` (the multi-round test that pins
"only NEWLY-accrued surplus is pulled next round, not the retained buffer again"),
`e2e_execute_pulls_only_burn_share_and_ratchets_principal`, `e2e_buy_burn_uniform_price_dutch_auction`,
`e2e_full_genesis_to_buy_burn`. Strongly mutation-SHARP across single- and multi-round paths. (Pairs with
finding-O floor: the floor bounds a single pull; the ratchet bounds re-pulls across rounds.) Verdict: BLOCKED,
no gap. No code/test change.

### [VERIFIED SHARP — execute coin_i>0 guard: never pay USD for zero COIN (dust-fill LOF)] ET.
HOSTILE vector (rounding-edge LOF): in a real settle (total_coin>0) the MARGINAL bid can receive a residual
budget so small that `coin_i = floor(usd_i * cm/um) == 0`. The clearing credits a filled bid via `if usd_i > 0
&& coin_i > 0 { total_usd += usd_i; total_coin += coin_i; refund = c - coin_i }` (lib.rs:1497); the `coin_i > 0`
clause routes a zero-COIN fill to the ELSE branch (usd_owed -> 0, FULL coin refund = treated as unfilled).
Without it, a marginal bidder whose fill rounds to 0 COIN would be credited `usd_owed = residual` (free USD from
the holding) AND `coin_refund = c - 0 = c` (full COIN back) — i.e. receive protocol USD while handing over ZERO
COIN and keeping all of it: a direct LOF that also lets total_usd > 0 with total_coin contribution 0 (paying
USD for nothing). Mutated :1497 to drop the `coin_i > 0` clause (`if usd_i > 0`) ->
`e2e_settle_with_a_zero_coin_marginal_pays_no_usd_for_zero_coin` FAILS = mutation-SHARP. (Companion edge: a
settle where EVERY fill rounds to 0 COIN is a roll, total_coin==0 -> not settled; finding AE restores the slot
state, pinned by `e2e_roll_with_a_marginal_zero_coin_fill`.) Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — cross-proposal seal irreversibility (winner-take-all is final)] ES.
HOSTILE vector (state-overwrite redirect of the whole distribution): once a proposal wins (gv trigger ->
seal_winner sets config.sealed_proposal), a SECOND proposal that also reaches the tally could re-run trigger ->
seal_winner and OVERWRITE config.sealed_proposal, redirecting the entire COIN distribution from the first
winner to the attacker's proposal. claim binds payout to config.sealed_proposal (DL), so a reseal makes the
real winner's claims fail and routes the supply to the attacker. Guard: seal_winner rejects `if
config.is_sealed()` (distribution lib.rs:470) — the seal is one-shot. Mutated :470 to `if false && ...` (allow
reseal) -> TWO tests FAIL across suites: `a_second_proposal_cannot_reseal_after_a_winner_is_sealed`
(genesis-vote seal.rs — B's trigger -> seal CPI must revert once A is sealed) AND
`a_losing_proposal_cannot_claim_the_winners_vault` (distribution — a loser cannot reseal to redirect the
vault). Strongly mutation-SHARP, doubly-covered (gv-side trigger path + distribution-side direct reseal). This
pins finding f8f688e (cross-proposal winner-take-all irreversibility). Verdict: BLOCKED, no gap. No code/test
change.

### [VERIFIED SHARP — impaired-exit pro-rata haircut + floor rounding is split-resistant] ER.
HOSTILE vector (rounding direction / split-withdrawal drain): under impairment the insurance exit pays a
pro-rata haircut `owed = payout(policy, insurance, outstanding, amount)`. Two ways to over-extract and drain
co-depositors: (a) NO haircut (pay full principal first-come, stranding late exiters), or (b) a CEIL/round-up
that lets an attacker split one exit into many tiny pieces, each capturing a rounding gain. Verified `payout`
(subledger lib.rs) uses `mul_div_floor(balance, principal, outstanding)` — FLOOR, the protocol-favoring
direction; since floor(a)+floor(b) <= floor(a+b), splitting can only LOSE dust, never beat a single exit
(order-independent, no first-come race). Mutated the impaired branch `Ok(pro_rata)` -> `Ok(principal)` (no
haircut) -> FOUR tests FAIL: `splitting_an_impaired_exit_cannot_beat_the_pro_rata_or_drain_a_codepositor` (the
anti-split/anti-drain test — a co-depositor's capital is present, so over-paying one exit drains it),
`impaired_insurance_exit_is_pro_rata`, `a_fully_impaired_exit_still_retires_the_position...`,
`foreign_market_slab_cannot_inflate_the_haircut` (also pins reading insurance from the bound slab, cf. finding
T/DS). Strongly mutation-SHARP. Verdict: BLOCKED, no gap (finding-L pro-rata haircut + floor rounding fully
pins the split/drain vector). No code/test change.

### [VERIFIED SHARP — insurance_deposit last-write-time start_slot reset blocks hold-time weight inflation] EQ.
HOSTILE vector (governance weight inflation via stale timestamp): gv weight = `floor(log2(hold)) * principal`,
hold = now - position.start_slot. If a top-up did NOT reset start_slot, an attacker deposits 1 atom EARLY
(start_slot = slot 0), waits to accrue a large `hold`, then TOPS UP a huge principal that inherits the ancient
start_slot -> a high log2(hold) multiplier is applied to the big late principal -> fabricated vote weight (and
quorum principal) far beyond what fresh capital should earn. Defense: insurance_deposit applies last-write-time
— `position.start_slot = Clock::get()?.slot` on EVERY deposit incl. top-ups (subledger lib.rs:960), so a
late-added principal always restarts its own hold clock. Mutated :960 to only set start_slot on the FIRST
deposit (stale on top-up) -> 10+ tests FAIL, critically `topping_up_a_voted_position_does_not_inflate_or_unlock_the_vote`
(the exact anti-inflation test for the top-up case) and `a_too_recent_position_cannot_vote_or_pump_the_quorum`
(plus the core deposit/vote/weight lifecycle). Strongly mutation-SHARP. (Companion: vote_weight treats
start_slot==0 / hold<2 as zero weight, so a just-deposited position cannot vote — `a_too_recent_position...`.)
Verdict: BLOCKED, no gap. The weight model (last-write-time EQ, floor(log2) cap, one-vote DO/CR, vote-lock ED)
is fully anti-inflation-verified. No code/test change.

### [VERIFIED SHARP — place_bid slot reuse has no stale-state inheritance; place_slot cooldown anchor sharp] EP.
HOSTILE vector (stale-state inheritance on slot reuse/eviction): when place_bid takes a slot — either a FREED
slot (zeroed by claim/cancel) or an EVICTED slot (overwritten over the weakest live bid) — any field NOT
rewritten would carry the prior occupant's value. Most dangerous: SL_PLACE_SLOT (the cancel-cooldown anchor):
if a new bid inherited an EARLIER place_slot, its cooldown `now >= place_slot + 2*round_length` would already be
satisfied, letting it cancel immediately — re-opening the last-second-cancel manipulation the cooldown exists to
stop. Also SL_SETTLED / SL_USD_OWED / SL_COIN_REFUND inheritance could let a fresh bid claim a phantom payout.
Verified place_bid writes the ENTIRE slot fresh on every placement (lib.rs:161-181): SL_OCCUPIED=1, SL_SETTLED=0,
SL_BIDDER, SL_USD_DEST/SL_COIN_ATA (canonical ATAs), SL_COIN/SL_USDC, SL_USD_OWED=0, SL_COIN_REFUND=0,
SL_PLACE_SLOT=now, SL_PLACE_ROUND_END — nothing is left at the evicted/freed occupant's value. Mutated the
place_slot write (:1268) to a stale `0u64` -> TWO tests FAIL: `e2e_bid_cancellable_after_cooldown_keeps_fee`
(cancel-before-cooldown must be rejected) and `e2e_roll_does_not_unlock_cancel_before_aging`. Mutation-SHARP —
the anti-spoof cooldown cannot be bypassed via a stale place_slot, and (since OPEN-book slots never carry
SETTLED/owed/refund) there is no phantom-payout inheritance. Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — insurance_withdraw principal decrement blocks repeated-withdraw drain] EO.
Continuing the EM anti-mask replay hunt on the SHARED percolator insurance vault. Candidate: `position.principal
-= amount` (subledger lib.rs:1128) — the per-withdraw decrement (distinct from CZ's amount-cap and DR's
outstanding decrement). If removed, a depositor with principal P could withdraw P REPEATEDLY (each time amount=P
<= principal=P, since principal never decreases), draining co-depositors' funds from the shared vault — and the
CZ cap (`amount > principal`) would NOT catch it (amount always == principal). Mutated :1128 to `-= 0` -> FIVE
tests FAIL: `splitting_an_impaired_exit_cannot_beat_the_pro_rata_or_drain_a_codepositor` (the explicit anti-mask
— a co-depositor's capital is present in the shared vault, so a repeated withdraw WOULD drain it absent the
decrement), `cannot_redeposit_into_a_retired_position`, `a_fully_impaired_exit_still_retires_the_position...`,
`principal_only_owner_exit_returns_funds_and_guards`, `cannot_vote_with_a_withdrawn_position`. Strongly
mutation-SHARP and genuinely anti-masked (the `..._drain_a_codepositor` test funds another depositor whose
principal exceeds the replay). Verdict: BLOCKED, no gap. The subledger withdraw accounting (principal :1128 EO,
outstanding :1127 DR, over-withdraw cap CZ) is fully anti-mask-verified. No code/test change.

### [VERIFIED SHARP — replay re-audit (claim slot-zeroing; retract ballot-clearing) post-EM] EN.
After EM (a transfer-insufficiency-MASKED replay guard slipped through), re-audited the sibling replay/double-spend
guards with the anti-mask lens — could a "replay rejected" test be masked by the shared pool being drained below
the replay amount? (1) TWAP claim slot-zeroing (:1629, DN): re-mutated to a no-op -> FIVE tests FAIL incl.
`e2e_claim_cannot_be_replayed_to_drain_other_winners` -> strongly mutation-SHARP, NOT masked (that test funds
multiple winners so the shared settlement_usd/coin_escrow exceed a single replay). (2) genesis-vote RETRACT
double-subtract: a voter retracting twice could corrupt the GLOBAL total_cast_weight (masked: other voters'
weight keeps checked_sub from underflowing) -> but the back-out (:618-622) subtracts the BALLOT's OWN
voted_weight, which retract ZEROES (:629-630), so a second retract subtracts 0 = harmless; additionally
`voted_proposal = default` (:628) makes has_live_ballot() false so retract #2 is rejected outright (:623) AND
the subledger vote-lock is released (lock_val, :658). Mutated :628 (drop the voted_proposal clear) -> FOUR
insurance tests FAIL (`winning_voter_can_retract_and_exit_after_finalize`,
`vote_locked_principal_cannot_exit_until_retracted`, ...) — :628 is load-bearing for lock-release-on-retract
(without it the retractor's capital stays frozen = self-DOS) and mutation-SHARP. Verdict: BLOCKED, no gap (both
replay guards genuinely sharp; EM remains the lone masked gap found by this lens). No code/test change.

### [GAP FIXED — cancel_bid double-refund drains the shared escrow] EM.
HOSTILE vector (replay / double-spend — cross-user LOF): cancel_bid refunds a bid's escrowed `coin_atoms`
from the SHARED coin_escrow (which pools EVERY bidder's COIN), then ZEROES the slot (lib.rs:1728). The
slot-zeroing is the SOLE guard — there is no separate "cancelled" flag. An aged bidder who cancels their OWN
slot TWICE would have the second refund paid out of OTHER bidders' escrowed COIN: a direct pool drain. MISSING
COVERAGE: mutated the cancel slot-zeroing loop `for b in d[o..o+SLOT_SIZE]{*b=0}` -> `d[o..o]` (no-op) ->
75->... 74 chain PASS: MUTATION-BLIND (no double-cancel test; the sibling slot-zeroing in claim (:1629) is DN,
but cancel's was never pinned). FIX: added `e2e_cancel_cannot_be_replayed_to_drain_the_shared_escrow` —
alice(10k, slot0) + bob(20k, slot1) escrow into the shared pool; after the cooldown alice cancels slot0 once
(refund 10k, pool=20k), then REPLAYS the cancel on her zeroed slot. Asserts the replay is refused, alice got
NO second refund, and bob's 20k is untouched + fully reclaimable. Mutation-sharp via a DELIBERATE anti-mask:
bob's leg (20k) EXCEEDS alice's refund (10k), so the replayed transfer would SUCCEED on balance absent the
slot-zeroing (avoids the DM-style transfer-insufficiency mask that made the first draft mutation-blind). PASSES
with :1728 present, FAILS (replay succeeds -> bob drained) with it removed. chain 74->75. Verdict: BLOCKED;
double-refund COVERAGE GAP closed. (This is the cancel-side analogue of DM's claim double-claim.)

### [VERIFIED SHARP — execute holding key binding (canonical budget routing)] EL.
HOSTILE vector (account substitution on the permissionless execute crank — last unverified execute fund
account): execute pulls `burnable` insurance into `holding` and then transfers `total_usd` from holding ->
settlement_usd. A cranker substitutes a NON-canonical holding to misroute the pulled budget. execute binds it
three ways `*holding.key != book.holding || h.owner != expected_auth || h.mint != book.collateral_mint`
(lib.rs:1352). The owner clause forces twap_authority ownership (an attacker cannot drain such an account
directly — only the DAO via shutdown can), but the KEY clause is what guarantees the rolled-over budget
accumulates in the ONE canonical holding future executes read; without it a cranker routes the budget into a
different twap_authority-owned account, starving future rounds (griefing/DOS of budget accounting). Mutated the
key clause to `if false || ...` -> `e2e_execute_pulls_only_burn_share_and_ratchets_principal` FAILS =
mutation-SHARP. This completes the execute fund-account surface: holding (EL), settlement_usd (CS), coin_escrow
(CU), claim source (CV), coin_sink SEND (EK/DB), market_slab+authority (DS/EH) — ALL mutation-verified or
structurally bound. Restored -> 74 chain green. Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — execute SEND-mode coin_sink binding blocks buyback redirect] EK.
HOSTILE vector (account substitution on a permissionless crank): in SEND (buyback) mode, execute transfers the
bought COIN (total_coin) to `coin_sink` instead of burning. execute is PERMISSIONLESS (any cranker), and the
coin_sink is a PASSED account — so a cranker could substitute their OWN account to redirect the DAO's entire
buyback to themselves (LOF). Defense: execute reads the sink account ONLY in SEND mode and binds it
`*coin_sink.key != book.coin_sink -> reject` (lib.rs:1540); book.coin_sink was pinned by the DAO via the
Squads-gated set_coin_sink/init_book (and != coin_escrow, finding AS). Mutated :1540 to `if false && ...` ->
TWO tests FAIL: `e2e_execute_send_cranker_cannot_redirect_the_buyback` (a rogue cranker-owned sink is refused)
AND `e2e_send_mode_routes_bought_coin_to_treasury_not_attacker` (bought COIN routes to the DAO treasury, not
the attacker). Mutation-SHARP, doubly-covered. (BURN mode has no sink account — total_coin is burned from
coin_escrow via spl_burn_signed, nothing to redirect.) Restored -> 74 chain green. Verdict: BLOCKED, no gap.
No code/test change.

### [VERIFIED — auction liveness: a winner refusing to claim cannot DOS the book; cranker cannot redirect] EJ.
HOSTILE vector (liveness DOS + permissionless-claim safety): after execute SETTLES the book, place_bid is
rejected until every slot drains via claim; the book reopens only when the last slot frees. If claim required
the BIDDER to sign, a malicious winner could simply refuse to claim -> book stuck SETTLED forever -> no new
rounds -> permanent auction DOS, with other bidders' committed COIN frozen too. Verified the design defeats
this: claim's first account is a `cranker` (any signer, :1562/1579), NOT the bidder — it is PERMISSIONLESS, so
anyone can crank a stranded winner's claim. The payout is force-bound to the slot's RECORDED canonical ATAs
(`*usd_dest.key != dest_key || *coin_ata.key != coin_key -> reject`, :1617; the dests were pinned to the
bidder's canonical ATA at place_bid), so a third-party cranker pays the winner's OWN ATA and cannot redirect;
the slot is then zeroed and the book flips to OPEN when empty (:1638). Coverage: tests crank claims with a
cranker Keypair DISTINCT from the bidders (chain.rs:3193-3195), `e2e_execute_on_a_settled_book_is_frozen_until_claims_drain_it`
pins the SETTLED-freeze, and the drain reopens the book to OPEN (:3325). Mutated the dest binding :1617 to
`if false && ...` -> `e2e_claim_cannot_redirect_a_winners_payout` AND `e2e_claim_cannot_redirect_a_losers_coin_refund`
(the CV pins) BOTH FAIL = mutation-SHARP. Combined with the canonical-ATA refund-brick fix
(`e2e_closing_refund_ata_cannot_permanently_brick_the_book`, anyone can recreate a closed ATA), there is no
refuse-to-claim DOS and no cranker theft. Restored -> 74 chain green. Verdict: BLOCKED, no gap. No code/test
change.

### [VERIFIED SHARP — shutdown cannot strand/steal bidder funds; Squads gate sharp] EI.
HOSTILE vector (DAO wind-down stranding/stealing user funds): the Squads-gated `shutdown` sweeps the TWAP's
accumulated USD. Could it (a) be run by a non-DAO to steal the holding, or (b) drain bidders' escrowed COIN
(coin_escrow) / winners' owed USD (settlement_usd), or strand them by bricking the book? Analysis: shutdown's
ACCOUNT SET is `[squads_vault, config, twap_authority, holding, dest, token_program]` — it unpacks only
holding+dest and does ONE transfer holding->dest. It never RECEIVES coin_escrow / settlement_usd / the book, so
it categorically cannot sweep bidder funds or write book state (those stay claimable via the book_escrow PDA).
(a) is gated by `require_squads_vault` (lib.rs:1759) + the twap_authority binding (:1768) + holding owner ==
expected_auth (:1772). (b) is additionally blocked because passing coin_escrow/settlement_usd AS the holding
fails the holding owner check (they are book_escrow-owned, not twap_authority-owned) — backstopped by the
SPL-transfer authority too (DC). Coverage: `e2e_shutdown_sweeps_holding_only_via_squads` (non-DAO rogue
shutdown rejected; holding swept only via real Squads execute) and `e2e_shutdown_cannot_drain_escrow_or_settlement`
(shutdown with coin_escrow / settlement_usd substituted as the holding is refused, bidder funds intact).
Mutated the Squads gate (commented out `require_squads_vault` in shutdown, :1759) -> `e2e_shutdown_sweeps_holding_only_via_squads`
FAILS = mutation-SHARP. Restored -> 74 chain green. Verdict: BLOCKED, no gap (shutdown blast-radius is the
holding only, by account-set construction + owner binding; Squads gate sharp). No code/test change.

### [VERIFIED COVERED — parasite config on the victim's market cannot drain insurance (finding AQ)] EH.
HOSTILE vector (PDA-binding / same-market isolation — the single highest-stakes drain): the twap operator PDA
is named `["market-0-twap", ...]` but `authority_seeds(config) = [SEED, config.as_ref()]` is CONFIG-scoped
(finding AQ, hardening the original market-scoped AD). Attack: stand up a SECOND twap config-A on the VICTIM's
market (env.slab), set config-A's OWN reserved_floor to 0 (bypassing the victim's 1M principal floor), and
permissionlessly crank execute(config-A) to pull the victim's ENTIRE insurance (principal included) into a
parasite holding. Defense: execute's WithdrawInsuranceLimited CPI is invoke_signed with config-A's seeds ->
produces config-A's DISTINCT authority `auth_a` (not the victim's operator PDA) -> the real percolator does NOT
recognize auth_a as the slab's asset-0 operator -> rejects the withdraw. No twap-side guard LINE to mutate; the
protection is the seed STRUCTURE (config-bound) + percolator's external operator check. Coverage:
`e2e_parasite_config_on_same_market_cannot_drain_insurance` stands up a real parasite config-A under an attacker
DAO+multisig on the victim's slab, asserts `auth_a != env.twap_authority` (pins the config-binding structurally
-- a market-scoped regression would make them equal and FAIL this), runs execute(config-A) end-to-end against
the REAL percolator binary, and asserts it is REJECTED + `perc_vault == insurance_before` (victim fully intact)
+ parasite holding == 0. Confirmed PASS. Verdict: BLOCKED, no gap (config-scoped authority seed + percolator
operator rejection; pinned by the assert_ne structural check AND the end-to-end drain-fails assertion). No
code/test change.

### [VERIFIED SHARP — cross-tenant set_* isolation (config-A cannot mutate config-B's book)] EG.
HOSTILE vector (account confusion / cross-tenant isolation): multiple twap configs coexist (one per market).
config-A's own Squads DAO authorizes a `set_*` on config-B's BOOK while passing config-A (which A controls) as
the config_account. Three escalating doors: set_reserve (drain B's surplus at bad prices), set_coin_sink (flip
B's book to SEND mode with an A-OWNED sink -> every COIN B's execute buys is redirected to A = cross-tenant
THEFT), set_bid_fee (jack B's per-bid fee to u64::MAX -> B's place_bid always unpayable -> auction bricked =
grief DOS). Each set_* re-reads the book and pins `book.config != *config_account.key -> reject` (set_reserve
lib.rs:1001, set_coin_sink :1031, set_bid_fee :1075) — three DISTINCT checks. All covered by ONE end-to-end
test `e2e_config_a_cannot_mutate_config_bs_book` (stands up a real second config-A under an attacker DAO+multisig,
drives each attack via the real Squads execute, asserts each is refused + config-B's book field unchanged, with
a positive control that config-B's OWN Squads CAN set its reserve). Mutated the highest-stakes pin
(set_coin_sink :1031, the buyback-theft door) to `if false && ...` -> the test FAILS on the exact SECOND DOOR
assertion ("config-A must NOT flip config-B's book sink"). Mutation-SHARP. Restored -> 74 chain green. Verdict:
BLOCKED, no gap (cross-tenant isolation pinned across all three set_* doors). No code/test change.

### [VERIFIED SHARP — append_entries creator-gating blocks proposal poisoning] EF.
HOSTILE vector (missing-signer/authority + griefing): a non-creator appends entries to someone's UNSEALED
distribution proposal — inserting a self-allocation, padding entry_count, or inflating total_amount to grief
the genesis or redirect funds. Defense: append_entries requires `creator.is_signer` (lib.rs:405) AND
`header.creator != *creator.key -> reject` (:417), plus `header.sealed` blocks appends after seal (:420),
capacity + `total_amount <= total_supply` bound the list (:431,:439). Mutated the creator clause to `|| false`
-> `append_entries_rejects_a_foreign_creator` FAILS = mutation-SHARP (a foreign appender is refused while the
real creator can still extend its own proposal). BONUS — closes the EE backstop: the SAME instruction's
zero-entry rejection (`append_rejects_a_zero_amount_or_default_pubkey_entry` pins amount==0 || pk==default ->
reject) GUARANTEES no valid entry is zero, so an out-of-bounds claim reading an unfilled (zero) slab entry can
never match a real recipient/amount — the EE defense-in-depth rests on a mutation-sharp invariant. Verdict:
BLOCKED, no gap. No code/test change.

### [VERIFIED DOUBLY-DEFENDED — distribution claim out-of-bounds index] EE.
HOSTILE vector (rounding/bounds + reading uninitialized data): claim takes a u32 `index` and pays
`entry[index].amount` to `entry[index].recipient`. Pass an index past the filled entries to read garbage as a
valid (recipient, amount) and mint a payout. claim bound-checks `index >= header.entry_count -> reject`
(lib.rs:535). Mutated :535 to `if false && ...` -> 19 dist green: MUTATION-BLIND. BUT doubly-defended, no LOF:
the proposal slab is sized to `capacity * ENTRY_SIZE` (capacity creator-chosen <= MAX_ENTRIES) and ZERO-init at
create_proposal. (a) In-allocation reads (index in [entry_count, capacity)) hit zeros -> `pk = default` != the
real recipient signer -> IllegalOwner (the CL pk-binding), AND `amount == 0` -> reject (the DM guard); an
attacker cannot satisfy pk==recipient with a zero entry (recipient is a real-keypair signer, never the zero
key). (b) Beyond-allocation reads (index >= capacity) -> `pd[eo..eo+32]` slices past data_len -> Rust panic ->
clean tx abort; Solana account data is isolated so there is NO cross-account OOB read, only a self-inflicted
fail. => :535 is a clean-error guard masked by CL (pk) + DM (amount==0) + slice-bounds-panic. A test would be
mutation-blind to :535 -> per KEEP/DELETE, no test added. Verdict: BLOCKED (defense-in-depth). No code/test
change.

### [VERIFIED SHARP — vote-lock blocks withdrawing capital out from under a live ballot] ED.
HOSTILE vector (governance integrity / quorum inflation): vote (gv counts your principal as weight AND in the
quorum numerator), THEN withdraw that principal, leaving a capital-less ballot that still inflates quorum/weight
-> a minority could clear quorum with capital no longer at risk and seize the COIN supply. Defense: on vote, gv
CPIs subledger set_vote_lock to pledge the position; insurance_withdraw HARD-BLOCKS a locked exit
`if position.vote_locked { return InvalidAccountData }` (lib.rs:1049) — the owner must retract first (which
clears the lock), keeping the vote's principal snapshot backed by still-at-risk capital. set_vote_lock requires
BOTH the gv config PDA (vote_authority) AND the owner to sign (:1142), so an owner cannot self-unlock to exit.
Mutated :1049 to `if false && ...` -> FOUR tests FAIL: `vote_locked_principal_cannot_exit_until_retracted`,
`owner_cannot_self_unlock_a_live_vote_to_exit_capital`, `winning_voter_can_retract_and_exit_after_finalize`,
`topping_up_a_voted_position_does_not_inflate_or_unlock_the_vote`. Strongly mutation-SHARP — the
ballot-backed-by-live-capital invariant is pinned from the exit-block, self-unlock, post-finalize, and top-up
angles. Restored -> 41 insurance green. Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED — place_bid eviction smuggling closed by lazy read; reserve filter sharp] EC.
HOSTILE vector (Copenhagen duplicate-accounts / remaining-account smuggling): place_bid takes a trailing
eviction account; smuggle an extra/duplicate account when the book has FREE SPACE (no eviction) to confuse the
fund flow. Analyzed: the eviction account is read LAZILY — `next_account_info(iter)?` is called ONLY inside the
`if let Some(evicted)` branch (lib.rs:122-123), so when a free slot exists NO trailing account is consumed or
trusted; surplus accounts are simply ignored by the iterator (Solana passes them, the program never reads
them). On the eviction path it is bound to the evictee's recorded ATA (`evict_acct.key != evicted_ata`, :124 —
the DG guard). => no smuggling/duplicate vector exists. SECOND PROBE (rounding/economic guard, fund-relevant):
the uniform-price clearing's RESERVE filter `cmp_rate(c, u, reserve_num, reserve_den) == Ordering::Less ->
drop` (:1438) enforces the DAO's max USD-per-COIN; dropping it would let sub-reserve (too-expensive) bids fill
and the protocol OVERPAY for COIN (LOF to the DAO/holders). Boundary is correct (rate == reserve -> kept; rate
< reserve -> dropped). Mutated to `if false && ...` (accept sub-reserve) -> `e2e_reserve_blocks_expensive_bid_from_draining_surplus`
FAILS = mutation-SHARP. Verdict: BLOCKED (smuggling impossible by lazy read; reserve economic guard sharp).
No code/test change.

### [VERIFIED SAFE BY CONSTRUCTION — sysvar (clock) spoofing categorically impossible] EB.
HOSTILE vector (Copenhagen sysvar spoofing): forge the slot to bypass a time-gate — cancel a committed bid
before its cooldown, settle/execute before round_end, claim after the window, or burn-unclaimed before it.
Audited EVERY time-read across all four programs: ALL use the `Clock::get()?` SYSCALL (reads the real Clock
sysvar directly, unspoofable), and NO instruction accepts a clock/sysvar as a passed AccountInfo (no
`Clock::from_account_info`, no clock in any account list) -> there is literally nothing to substitute. Reads:
twap execute round_end (:1367) + next_end (:1530) + cancel cooldown (:1712) + place_bid round_end (:953,1267);
subledger start_slot last-write (:546, :960); genesis-vote weight age (:632); distribution seal_slot (:482),
claim window (:524-529), burn_unclaimed window (:596-601). Rent likewise via `Rent::get()` syscall in
create_pda_robust. The class is closed by CONSTRUCTION (absence of the vulnerable pattern), so a test would be
tautological — none added. BONUS: mutation-verified the claim window-closed gate `clock.slot >= window_end ->
reject` (distribution :529, complementary to DK's burn gate) is SHARP — `unclaimed_is_burned_after_window`
FAILS when dropped (claim-after-window is blocked + covered). Verdict: BLOCKED (no spoofable time source
exists; the gates the syscall feeds are mutation-sharp — cf. DJ round_end, DK burn window, CW cancel cooldown).
No code/test change.

### [VERIFIED DOUBLY-DEFENDED — init_book reinit wipes a live book / strands COIN] EA.
HOSTILE vector (Copenhagen reinit): re-run twap `init_book` on a LIVE book to zero all 32 bid slots ->
every bidder's escrowed COIN stranded (LOF/DOS). Guard: `book_account.data_len() != 0 -> AccountAlreadyInitialized`
(lib.rs:946). Mutated :946 to `if false && ...` -> 74 chain green: MUTATION-BLIND (no reinit test). BUT
triply-defended, the wipe cannot occur: (1) SQUADS-GATED — init_book calls `require_squads_vault` first, so
only a timelock'd DAO action can even attempt it (not permissionless). (2) SYSTEM-ALLOCATE OWNERSHIP — after
init the book PDA is owned by the twap PROGRAM (assign), and `create_pda_robust` (:946+) calls
`system_instruction::allocate`, which REQUIRES system ownership + zero data; on a program-owned, already-sized
account allocate fails, so the reinit aborts even with :946 dropped. (3) the :946 data_len guard itself. A
reinit test would be mutation-blind to :946 (masked by allocate) and requires a DAO signature -> per KEEP/DELETE,
no test added (cf. init_config's analogous data_len guard). BONUS confirmation this tick: register_proposal's
finalize-brick DOS guard `dist_proposal_config != config.distribution_config` (:460) — registering a foreign
distribution proposal that, if it won, trigger could never seal (header.config mismatch) -> genesis bricked —
is mutation-SHARP (`register_rejects_a_proposal_from_a_foreign_distribution_config` FAILS when dropped; also
covered by insurance `register_rejects_foreign_distribution_proposal`). Verdict: BLOCKED (reinit
defense-in-depth; register binding sharp). No code/test change.

### [VERIFIED DOUBLY-DEFENDED — insurance_withdraw substituted holding / vault] DZ.
HOSTILE vector (substitution lens onto the subledger handoff fund path): pass an attacker-owned `holding`
(or foreign `percolator_vault`) to `insurance_withdraw` to capture withdrawn percolator insurance. Guards:
`holding_state.owner != *pool_account.key` (lib.rs:1035) requires the withdraw landing account to be
pool-PDA-owned; `*percolator_vault.key != pool.vault` (:1033 region) binds the source. Mutated :1035 to drop
the owner clause -> 41 insurance green: MUTATION-BLIND. BUT doubly-defended, no theft possible: (1) SPL-TRANSFER
AUTHORITY — the second leg `invoke_signed([holding, owner_ata, pool_account], &[&seeds])` (:1120) moves
holding->owner_ata with the POOL PDA as authority; SPL requires the authority to OWN the source, so an
attacker-owned holding makes this leg fail -> ATOMIC REVERT of the whole withdraw (incl. the WithdrawInsuranceLimited
CPI) -> funds return to insurance. (2) percolator's WithdrawInsuranceLimited independently requires the dest to
be the operator (pool PDA). (3) SELF-SCOPED — the position owner signs (:62 owner==*owner.key) and withdraws
their OWN pro-rata principal to their OWN ata; substituting the holding can only DOS their own exit, never a
cross-user LOF. The percolator_vault binding is the CZ class (the WithdrawInsuranceLimited CPI re-validates the
vault against the market/operator). A substituted-holding test would be mutation-blind to :1035 (masked by the
:1120 transfer authority) AND self-scoped -> per KEEP/DELETE (cf. CW/CX/DC), no test added. Verdict: BLOCKED
(defense-in-depth, SPL-authority + percolator + self-scope). No code/test change.

### [VERIFIED DEFENSE-IN-DEPTH — trigger distribution_config binding / seal redirect] DY.
HOSTILE vector (continuing the DW/DX substitution lens): substitute a foreign distribution_config into gv
`trigger` so the winner-take-all seal lands on an attacker-controlled distribution (config.authority preset to
the gv config PDA), redirecting the COIN payout. trigger binds three accounts (lib.rs:713-716):
distribution_program, distribution_config (:715), distribution_proposal==pv.distribution_proposal (:716).
Mutation results: :716 (proposal) is SHARP — `trigger_cannot_redirect_to_a_sibling_distribution_proposal`
(the BB test) FAILS when dropped. :715 (config) is mutation-BLIND (seal 14 + insurance 41 green). Probed the
backstop: distribution `seal_winner` :475 requires `header.config == config_account.key` (the proposal must
belong to the sealed config) — but THAT is ALSO mutation-blind (dist 19 + seal 14 green when dropped). Analyzed
the COMBINATION: the two config bindings (:715 trigger-side, :475 seal-side) are MUTUALLY REDUNDANT, and :716
already forces the legit registered proposal whose header.config IS the legit config — so the seal cannot be
redirected to a foreign config by ANY single guard removal (each is backstopped by the others + the proposal
binding). seal_winner is reachable ONLY via trigger (its authority == config.authority == the gv config PDA,
which only trigger can invoke_signed), so there is no direct-call bypass. => triply-redundant defense-in-depth,
NOT a single-point gap. A config-redirect test would be (a) mutation-blind to every single removal (masked) and
(b) redundant with the BB proposal-redirect test (the meaningful, stronger version). Per KEEP/DELETE: no test
added. Verdict: BLOCKED (defense-in-depth). No code/test change.

### [GAP FIXED — substituted low-outstanding pool -> fake quorum -> COIN-supply theft] DX.
HOSTILE vector (sibling of DW; account substitution): `trigger` measures quorum as `total_voted_principal*2 >
live_outstanding`, reading `outstanding` LIVE from the subledger pool (lib.rs:740). Feed a pool reporting a
tiny outstanding -> a MINORITY voter clears quorum -> winner-take-all seizes 100% of the COIN supply. The pool
is bound by owner AND key to config.subledger_pool (:738). The KEY bind is the SOLE guard: the owner check
alone is insufficient because anyone can permissionlessly init their OWN empty subledger pool (also
subledger-owned). MISSING COVERAGE: every trigger test passed the canonical pool, so mutating the key clause
(`*sub_pool.key != config.subledger_pool` -> `false`) left BOTH seal (14) AND the real-trigger integration
suite (40) green. FIX: added `trigger_with_a_substituted_low_outstanding_pool_cannot_fake_quorum_to_steal_the_supply`
(insurance_percolator.rs): alice deposits 1 + votes, bob deposits 1000 + does NOT vote (so 2*1 <= 1001, quorum
legitimately fails on the real pool — asserted as sanity); then forges a subledger-OWNED pool byte-identical to
the real one but with outstanding=0 at a NON-canonical key, and triggers with it. Asserts the substituted-pool
trigger is refused and the proposal is NOT marked executed (offset 88 stays 0). Mutation-SHARP: PASSES with :738
present, FAILS with the key clause dropped (the empty pool -> 2*1 > 0 -> quorum faked -> seal proceeds). insurance
40->41, seal 14 green. Verdict: BLOCKED; the pool key-binding COVERAGE GAP closed.

### [GAP FIXED — forged subledger-position -> fabricated vote weight -> COIN-supply theft] DW.
HOSTILE vector (THE single highest-stakes attack): genesis-vote `vote` reads (principal, start_slot) from a
subledger position to compute weight = floor(log2(hold))*principal. Feed a FORGED position with u64::MAX
principal -> astronomical weight + principal -> single-handedly clear quorum + majority -> TRIGGER
winner-take-all -> seize 100% of the COIN supply. Two layered guards: (a) `sub_position.owner ==
config.subledger_program` (lib.rs:559); (b) PDA key bind `*sub_position.key == PDA(["subledger_position",
pool, voter], subledger)` (:569). MISSING COVERAGE: every existing vote test uses a real, correctly-owned
position, so NEITHER guard was ever exercised against a forgery — mutating :559 OR :569 left ALL suites
(seal 14, insurance 39, real-vote integration path) green. FIX: added
`a_forged_subledger_position_cannot_fabricate_vote_weight_to_steal_the_supply` (insurance_percolator.rs) with
TWO end-to-end forgeries on a real vote: (a) canonical PDA address but non-subledger owner + u64::MAX
principal; (b) genuinely subledger-owned account with u64::MAX principal at a WRONG (non-canonical) address
via a hand-built vote ix. Asserts both are refused, ZERO weight credited, and the follow-on trigger to mint
the supply FAILS. Mutation results: case (b)/:569 (key bind) is SHARP and the SOLE guard for the wrong-key
forgery (mutating :569 -> test FAILS = the real gap closed). case (a)/:559 (owner check) is mutation-BLIND
because it is DOUBLY-DEFENDED: the downstream SetVoteLock CPI itself requires a subledger-owned position, so
a forged-owner account is rejected at the pledge step even with :559 gone (defense-in-depth). Both cases KEPT
as end-to-end no-theft assertions (case (b) sharp; case (a) a doubly-defended end-to-end guard, like CZ).
chain unchanged; insurance 39->40, seal 14 green. Verdict: BLOCKED; the key-binding COVERAGE GAP closed.

### [GAP FIXED — init_config multisig type-cosplay (missing-owner-check coverage)] DV.
HOSTILE vector (Copenhagen type-cosplay / missing owner check): bind the twap config to a FORGED Squads
`Multisig` — an account the attacker owns, byte-for-byte carrying the real 8-byte discriminator,
config_authority[40..72] = the real DAO, and a full 1-week time_lock[74..78] — so all of init_config's
internal consistency checks (disc :400, config_authority==DAO :404, time_lock>=1wk :410) PASS on the forged
bytes. The SOLE guard is the owner check `*squads_multisig.owner != SQUADS_PROGRAM_ID` (lib.rs:385). Mutated
:385 to `if false && ...`, build-sbf -> 73 chain PASS: MUTATION-BLIND (the front-run test :474 uses REAL
Squads-owned multisigs, so it never exercised the owner check). The downstream attack is largely backstopped
(a fake multisig yields a different config PDA, seeded by the multisig key, and privileged actions still need
the real Squads vault PDA the attacker can't sign) — but :385 is a load-bearing, zero-coverage input-validation
guard squarely in the loop's named classes, and a regression dropping it would be caught by NOTHING. FIX:
added `twap_config_rejects_a_non_squads_owned_multisig_cosplay` (chain.rs) — forges the exact bytes on a
NON-Squads-owned account, asserts init_config rejects it (IllegalOwner) and no config PDA is created.
Mutation-sharp: PASSES with :385 present, FAILS with it removed (the forged bytes bind a config). chain 73->74.
Verdict: BLOCKED (guard correct); COVERAGE GAP closed. Test KEPT (pins the owner boundary; distinct from the
:474 real-multisig front-run test). No src change.

### [VERIFIED SHARP — clearing-math overflow / whole-book settle-DOS via usd*coin leg] DU.
HOSTILE vector: the uniform-price clearing computes `coin_i = mul_div_floor(usd_i, cm, um)` (lib.rs:1496),
which is `usd_i.checked_mul(cm).checked_div(um)`. `checked_mul` REVERTS on overflow (no silent wrap) — so if
a marginal bid's COIN leg `cm` and a filled bid's `usd_i` are both large enough that `usd_i*cm > u128::MAX`,
`execute` reverts EVERY time -> the whole book can never settle -> permanent buy/burn DOS with everyone's COIN
locked. Safe ONLY if both legs <= u64::MAX: `(2^64-1)^2 = 2^128 - 2^65 + 1 < u128::MAX`, no overflow. That
guarantee is the finding-AC u64 leg bound in place_bid: `as_u64(coin_atoms)?` (load-bearing for the transfer)
AND the BARE `let _ = as_u64(usdc_atoms)?` (:1115) whose SOLE purpose is overflow-safety. Suspected the bare
usdc bound might be uncovered. Mutated :1115 to `let _ = usdc_atoms` (drop the bound), build-sbf -> TWO tests
FAIL: `e2e_place_bid_rejects_a_leg_above_u64` (part (b), usd leg = 2^64 rejected pre-escrow) AND
`e2e_full_book_of_worst_case_rates_cannot_dos_execute` (the exact settle-DOS scenario). Mutation-SHARP,
doubly-covered. Also confirmed the floor rounding direction is conservative (coin_i rounded DOWN => refund =
c - coin_i >= 0, checked_sub never underflows; protocol never burns more COIN than the bidder escrowed; the
sub-atom dust favors the bidder, not a LOF). Restored -> 73 chain green. Verdict: BLOCKED, no gap. No
code/test change.

### [VERIFIED SHARP — place_bid book monopolization via duplicate bids (DOS)] DT.
HOSTILE vector: a single attacker fills all 32 book slots with their own cheap bids to lock honest sellers
out of the uniform-price auction; the strictly-better-eviction rule would then PROTECT the squat (an honest
seller's bid only evicts the weakest, and the attacker keeps re-taking slots). Plausibility hinges on whether
`place_bid` enforces one-active-bid-per-bidder. It DOES (lib.rs:1161-1165): before insertion it scans all
MAX_BIDS slots and `if d[o+SL_OCCUPIED]==1 && book_rd_key(SL_BIDDER)==*bidder.key { return InvalidArgument }`
("already has an active bid") — so a bidder can never hold 2 slots, let alone 32. Mutated the scan predicate
to `if false && ...` (disable the dup check), build-sbf -> `e2e_bid_cannot_be_cancelled_only_evicted_by_a_better_bid`
FAILS (it asserts a same-bidder better re-place is rejected, chain.rs:3308 "a bidder cannot stack a second
bid"). Mutation-SHARP. The single-second-bid rejection fully covers the monopolization vector — you cannot
reach 2 slots, so 32 is unreachable; no extra test needed. Restored -> 73 chain green. Verdict: BLOCKED, no
gap. No code/test change.

### [VERIFIED DOUBLY-DEFENDED — execute slab->config binding / fabricated-surplus] DS.
HOSTILE vector: substitute a fake `market_slab` into twap `execute` whose finding-T insurance offset
reports a huge value, fabricating a giant `surplus` to over-pull insurance and/or ratchet `reserved_floor`
to a bogus value. `execute` binds `*market_slab.key != config.market_slab` (lib.rs:1322) as the sole
config-binding of the slab. Mutated 1322 to `|| false` (drop the binding), build-sbf -> 73 chain PASS:
mutation-BLIND (no test substitutes a wrong slab). BUT analyzed the attack as DOUBLY-DEFENDED, funds safe
even with 1322 gone: (1) ATOMIC REVERT — the insurance read (1373), the `reserved_floor += retained` ratchet,
and the `WithdrawInsuranceLimited` CPI are one `execute` tx; if the CPI fails the whole tx (ratchet included)
reverts, so no fabricated surplus persists. (2) OPERATOR BINDING at the CPI — the pull requires
`twap_authority` (PDA derived from `config_account`, :48-52) to be the slab's percolator insurance OPERATOR;
it operates exactly one market (`config.market_slab`), so a fake slab — or even a second REAL percolator
market — has a different operator and percolator's `WithdrawInsuranceLimited` rejects the non-operator signer.
(3) :56 derives `vault_authority` from `market_slab.key`, which percolator validates against the slab.
=> A faithful end-to-end exploit CANNOT be constructed (no substituted slab survives the real percolator CPI),
so any "wrong-slab-rejected" test would be MASKED by :56 / the CPI rather than sharply pinning :1322 — the
CB/CC class (percolator-backstopped, untestable-sharp without a deployed evil market `twap_authority` operates).
Verdict: BLOCKED (defense-in-depth). No test added (would be masked/marginal per KEEP/DELETE), no code change.

### [VERIFIED SHARP — insurance_withdraw outstanding decrement (quorum denominator)] DR.
Mutation-audited subledger `process_insurance_withdraw`'s `pool.outstanding_principal -= amount` (lib.rs:1127)
— the cross-program integrity guard: genesis-vote's quorum bar `total_voted_principal*2 > outstanding` and
the live-outstanding recompute both read this counter, so a withdrawn depositor MUST leave `outstanding` or
they'd keep phantom weight / inflate the quorum denominator (a withdrawn non-voting majority could no longer
be out-decided by those who stay, and a fully-impaired exit wouldn't retire). Mutated to a no-op
(`-= 0`), build-sbf -> EIGHT tests FAIL (incl. `a_fully_impaired_exit_still_retires_the_position...`,
`those_who_stay_decide_after_a_nonvoting_majority_forfeits_by_exiting`, `cannot_vote_with_a_withdrawn_position`,
`cannot_redeposit_into_a_retired_position`, the pro-rata + over-withdraw guards). Strongly mutation-sharp —
the outstanding counter is pinned by both the retire-on-exit lifecycle and the cross-program quorum tests.
Restored -> 39 insurance green. Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — place_bid anti-spam fee burn] DQ.
Mutation-audited place_bid's flat anti-spam fee burn `if book.bid_fee > 0 { burn book.bid_fee from the
bidder's COIN }` (twap lib.rs:1227) — a non-refundable per-bid cost (DAO-set) that makes flooding the
32-slot book expensive on top of the escrow. Skipped the burn (`if false`), build-sbf -> TWO tests FAIL:
`e2e_bid_fee_is_charged_and_burned` (asserts mint supply DROPS by the fee at place) +
`e2e_bid_cancellable_after_cooldown_keeps_fee` (the fee stays burned through cancel). So it is mutation-
sharp, doubly-tested. (Hypothesized a DM-style relative-comparison mask, but a dedicated charge-and-burn
test asserts the absolute supply drop.) Restored -> 73 chain green. Verdict: BLOCKED, no gap. No code/test
change.

### [VERIFIED SHARP — gv trigger quorum + majority strict checks] DP.
Mutation-audited the two winner-determination strict inequalities in trigger: quorum
`total_voted_principal*2 <= live_outstanding -> reject` (genesis-vote lib.rs:743) and majority
`support_weight*2 <= total_cast_weight -> reject` (:748). Each mutated to `if false` separately, build-sbf:
 - drop quorum -> `trigger_requires_a_strict_majority_and_quorum_not_a_tie` FAILS at "exactly-half principal
   is NOT a quorum" (a minority-capital proposal would seal -> minority capture / the whole COIN supply).
 - drop majority -> same test FAILS at "exactly-half cast weight is NOT a majority" (a tied proposal seals).
Both mutation-sharp, each isolated by the tie test (it injects exactly-50% for one while satisfying the
other). Restored -> 14 seal green. Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — gv vote back-out subtract (anti-inflation)] DO.
Mutation-audited the gv vote back-out `pv.support_weight = pv.support_weight.checked_sub(ballot.voted_weight)`
(genesis-vote lib.rs:619) — before (re-)recording a vote, vote() subtracts the ballot's PRIOR live weight
from the proposal + global tallies, so re-backing the SAME proposal REPLACES (not ACCUMULATES) the weight.
Mutated to `checked_sub(0)` (no-op back-out), build-sbf -> `e2e_retract_reback_cannot_inflate_vote_weight`
FAILS. So it is mutation-sharp. (I expected a possible DM-style mask — retract zeroes the ballot so a later
back-out is a no-op — but the test ALSO re-backs the LIVE ballot, exercising the real subtract: support
would accumulate w + w' without it = vote-weight inflation / quorum-majority manipulation.) Restored ->
test green. Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — twap claim slot-clearing (anti-replay)] DN.
Mutation-audited twap claim's slot-clearing `for b in d[o..o+SLOT_SIZE] { *b = 0 }` (twap lib.rs:1629) —
the anti-replay guard: after paying usd_owed + coin_refund, the bid slot is fully zeroed so a re-claim
reads SL_OCCUPIED!=1 / SL_SETTLED!=1 and is refused. Made it a no-op (`let _ = b`), build-sbf ->
`e2e_claim_cannot_be_replayed_to_drain_other_winners` FAILS (the same slot is claimed twice, the second
pay draining OTHER winners' settlement_usd). So it is mutation-sharp. NOTE: this is the TWAP analogue of
DM (distribution entry-zeroing), but UNLIKE DM the twap test is correctly designed — its very name
("drain_other_winners") means it replays WHILE settlement_usd still holds other winners' funds, isolating
the slot-clearing from the transfer-insufficiency that masked DM. Restored -> 73 chain green. Verdict:
BLOCKED, no gap. No code/test change.

### [COVERAGE GAP FIXED] DM. claim entry-zeroing (anti-replay) was mutation-BLIND (masked by vault insufficiency)
Mutation-audited claim's entry-zeroing `pd[eo+32..eo+40] = 0` (distribution lib.rs:564) — the LOAD-BEARING
anti-replay guard: after paying, the entry's amount is zeroed so a re-claim reads amount==0 and is refused.
Found: writing the amount BACK (no zeroing) left `seal_then_recipients_claim_their_entries`'s "no double
claim" assertion GREEN. Root cause (7th CL-class): that assertion fires AFTER both alice + bob claimed, so
the vault is EMPTY -> alice's re-claim reverts on transfer-insufficiency, not the zeroing. The masked LOF:
without the zeroing, a recipient with a SMALL entry re-claims while the vault is STILL FUNDED -> pays
themselves AGAIN out of OTHER recipients' unclaimed funds (cross-user double-spend). FIX: added
`double_claim_cannot_drain_other_recipients_while_the_vault_is_funded` — alice (entry 10) claims, then
re-claims BEFORE bob (vault still holds bob's 90); the zeroed entry must reject the replay and leave the
vault whole. Mutation proof: with the zeroing the re-claim is rejected; removing it -> the re-claim
succeeds (drains bob) -> test FAILS. Restored -> 19 distribution green, total ->168. KEEP. 7th mutation-audit
gap (CL/CR/CS/CU/CV/DF/DM); masking flavor = downstream transfer-insufficiency (vault drained before the
replay attempt), a TEST-ORDER artifact.

### [VERIFIED SHARP — claim winning-proposal binding] DL.
Mutation-audited distribution claim's winning-proposal binding `config.sealed_proposal != *proposal_account.key`
(lib.rs:518) — only the SEALED WINNER pays out, so a LOSING proposal (created under the same dist config,
attacker as recipient) cannot drain the shared vault (winner-take-all supply, BB-class). Dropped just the
sealed_proposal half (kept `!is_sealed()`), build-sbf -> `a_losing_proposal_cannot_claim_the_winners_vault`
FAILS (the loser claims). So the load-bearing half is mutation-sharp. The sibling `!is_sealed()` half is
DOUBLY-DEFENDED: before any seal, config.sealed_proposal == Pubkey::default(), which != any real proposal,
so claiming pre-seal is rejected by the sealed_proposal binding regardless — funds safe either way. Restored
-> 18 distribution green. Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — burn_unclaimed premature-burn window guard] DK.
Mutation-audited distribution burn_unclaimed's window guard `clock.slot < window_end -> reject` (lib.rs:601)
— the CROSS-USER anti-griefing guard: burn_unclaimed is PERMISSIONLESS, so without it a griefer could torch
the funded vault MID-WINDOW, destroying every recipient's UNCLAIMED COIN before they get to claim (LOF for
all unclaimed recipients). Dropped it (`if false`), build-sbf ->
`burn_unclaimed_is_rejected_during_the_claim_window` FAILS (a mid-window burn succeeds). So it is
mutation-sharp. (The complementary side — claim rejected AT/after window_end, burn allowed there — is the
clean cutoff pinned at slots 59/60, BF.) Restored -> 18 distribution green. Verdict: BLOCKED, no gap. No
code/test change.

### [VERIFIED SHARP — execute round gate] DJ.
Mutation-audited execute's round gate `clock_slot < book.round_end -> ERR_ROUND_ACTIVE` (twap lib.rs:1367)
— a round must run its full length before it can be executed, so a cranker can't PREMATURELY settle the
auction (early bids clearing at a worse marginal price before later bids arrive = manipulation). Dropped
it (`if false`), build-sbf -> `e2e_buy_burn_uniform_price_dutch_auction` FAILS at chain.rs:3129 ("execute
before the round expires must fail") — the headline test has an EXPLICIT pre-round_end execute-rejection
assertion (a dedicated `fn` grep missed it; it lives inside the multi-round headline test). So the round
gate is mutation-sharp. Restored -> 73 chain green. Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — cancel_bid owner check] DI.
Mutation-audited cancel_bid's owner check `book_rd_key(SL_BIDDER) != *bidder.key -> IllegalOwner` (twap
lib.rs:1699) — only the bid's OWNER may cancel it. Without it a non-owner could force-cancel (evict) a
VICTIM's bid from the book: the refund still goes to the victim's pinned coin_ata (so no theft), but the
victim loses their book position and must re-bid — a cross-user griefing / book-manipulation. Dropped it
(`if false`), build-sbf -> `e2e_bid_cancellable_after_cooldown_keeps_fee` FAILS at the mallory assertion
(a non-owner cancels alice's bid). So it is mutation-sharp. (The coin_ata binding passes in that assertion
since mallory supplies alice's recorded ATA, so ONLY the owner check rejects — correctly isolated.)
Restored -> 73 chain green. Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — cancel_bid anti-spoof cooldown] DH.
Mutation-audited cancel_bid's anti-spoof cooldown `aged = now >= place_slot + 2*round_length; if !aged ->
ERR_ROUND_ACTIVE` (twap lib.rs:1714) — the issue-#28 guard: a placed bid is committed until aged (or
settled), so a spoofer can't post a book-shaping bid, let others react, then yank it before execute.
Dropped it (`if false`), build-sbf -> TWO tests FAIL: `e2e_roll_does_not_unlock_cancel_before_aging` (the
no-op-roll cannot unlock cancel early) + `e2e_bid_cancellable_after_cooldown_keeps_fee` (cancel-before-
cooldown rejected). So it is mutation-sharp, doubly-tested. (round_length immutability — BJ — keeps the
cooldown stable; the settled-slot guard, which routes a settled bid to claim not cancel, is separate.)
Restored -> 73 chain green. Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — eviction refund-redirect guard] DG.
Mutation-audited place_bid's eviction refund-redirect guard `*evict_acct.key != evicted_ata -> reject`
(twap lib.rs:1212) — when a strictly-better bid evicts the weakest, its full COIN is refunded to the
EVICTEE's recorded canonical ATA, and the evictor cannot redirect it to an account they control. Dropped
it (`if false`), build-sbf -> `e2e_full_book_evicts_only_for_a_strictly_better_bid` FAILS at the thief
assertion (the evictor steals the evictee's escrowed COIN). So this guard IS genuinely mutation-sharp —
notably, the SAME test's strictly-better guard was mutation-BLIND (DF, just fixed), but the refund-redirect
half is sharp (the test supplies a real `thief` evict account, so the redirect path is fully reachable and
only this binding rejects). Restored -> 73 chain green. Verdict: BLOCKED, no gap. No code/test change.

### [COVERAGE GAP FIXED] DF. place_bid strictly-better eviction guard was mutation-BLIND (masked by absent evict_acct)
Mutation-audited the full-book eviction guard `cmp_bid(incoming, weakest) != Ordering::Greater -> reject`
(twap lib.rs:1199) — the anti-spam invariant that a FULL book only ever IMPROVES (a new bid can displace
the weakest ONLY if STRICTLY better; rate-equal/worse bids are refused). Found: dropping it left
`e2e_full_book_evicts_only_for_a_strictly_better_bid` GREEN. Root cause (6th CL-class): the test's
rate-equal spam bid passed `evict = None`, so with the guard removed the eviction path tried to read the
(absent) evict_acct and reverted with NotEnoughAccountKeys — masking the guard. The masked behavior: with
the guard gone AND a valid evict target supplied, a rate-EQUAL bid evicts the weakest -> a spammer churns
the book (evicting legitimate equal-rate bids, who are refunded but forced to re-bid; the book stops
monotonically improving). FIX: added an assertion where the same rate-equal bid SUPPLIES bid 0's canonical
ATA as the evict target, so only the strictly-better guard can reject it. Mutation proof: with the guard
the rate-equal bid is rejected; dropping :1199 -> it evicts -> assertion FAILS. Restored -> 73 chain green.
Strengthened existing test (no count change). KEEP. 6th mutation-blind gap (CL/CR/CS/CU/CV/DF) — the
absent-optional-account (eviction needs a trailing account) masked the guard, a new flavor of the masking.

### [VERIFIED SHARP — distribution fixed-supply invariant, both halves] DE.
Mutation-audited distribution init_config's fixed-supply guard `mint.mint_authority.is_some() ||
mint.freeze_authority.is_some() -> reject` (distribution lib.rs:295). It's a combined `if`, so each half
was mutated separately (CN-style): dropping the mint_authority half -> `init_config_rejects_a_mintable_coin`
FAILS (a mint with a live mint authority is accepted -> the holder could dilute COIN past the fixed pool,
diluting every recipient's governance/value); dropping the freeze_authority half ->
`init_config_rejects_a_freezable_coin` FAILS (a freezable mint is accepted -> the freeze authority could
freeze the vault or recipients' ATAs, DOSing all claims). Both halves mutation-sharp, each with its own
test. (Plus supply==total_supply + vault solvency, separately pinned.) Restored -> 18 distribution green.
Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — gv register creator binding] DD.
Mutation-audited gv register's creator binding `creator (pd[48..80]) != *payer.key -> reject` (genesis-vote
lib.rs:471) — stops an attacker FRONT-RUNNING registration of someone else's distribution proposal (which
would freeze the snapshot at a stale/partial (entry_count, total_amount), so the creator's later append
makes it permanently unsealable -> genesis stall). Dropped it (`if false`), build-sbf ->
`register_rejects_a_non_creator_front_runner` FAILS (a non-creator registers). So it is mutation-sharp.
(The foreign-distribution-config binding + empty-proposal guard are separately pinned; the trigger-side
bait-and-switch snapshot is DA.) Restored -> 14 seal green. Verdict: BLOCKED, no gap. No code/test change.

### [DOUBLY-DEFENDED — shutdown holding owner check, no new test] DC.
Mutation-audited shutdown's holding owner check `h.owner != expected_auth (twap_authority) -> reject`
(twap lib.rs:1772) — meant to stop the DAO's holding-sweep from draining the bidders' escrow/settlement.
Dropping it left `e2e_shutdown_cannot_drain_escrow_or_settlement` GREEN -> mutation-blind, BUT doubly-
defended (like CZ): the sweep transfer is `spl_transfer(holding -> dest, authority=twap_authority,
auth_seeds)`. The escrow + settlement_usd are BOOK_ESCROW-owned, so when passed as `holding` the SPL
transfer's authority check rejects (twap_authority is not their owner -> cannot sign the move) regardless
of the owner check. So a shutdown can NEVER move a non-twap_authority-owned account; funds safe, not a gap.
The existing test pins the end-to-end no-drain guarantee (passes whether the owner check OR the SPL
authority rejects), so it would catch a regression of BOTH the owner check AND a (hypothetical) signer
change. Verdict: BLOCKED (doubly-defended). No code/test change.

### [VERIFIED SHARP — execute SEND-mode coin_sink binding] DB.
Mutation-audited execute's SEND-mode buyback redirect guard `coin_sink.key != book.coin_sink` (twap
lib.rs:1540) — in SINK_SEND mode the bought COIN is transferred to coin_sink instead of burned, so this
key binding is the permissionless-cranker anti-theft (the cranker can't route the buyback to its own
account). Dropped it (`if false`), build-sbf -> TWO tests FAIL:
`e2e_execute_send_cranker_cannot_redirect_the_buyback` (5565) + `e2e_send_mode_routes_bought_coin_to_treasury_not_attacker`
(3898). So it is mutation-sharp, doubly-tested. (The self-loop guard coin_sink != coin_escrow is separately
pinned at both init+set doors, finding AS.) Restored -> 73 chain green. Verdict: BLOCKED, no gap. No
code/test change.

### [VERIFIED SHARP — gv trigger bait-and-switch snapshot check] DA.
Mutation-audited the trigger snapshot check (`pd[84..88] != pv.snapshot_entry_count || pd[88..96] !=
pv.snapshot_total_amount -> reject`, genesis-vote lib.rs:727-728) — stops a creator appending self-
allocations AFTER voters backed a proposal (then sealing the inflated version they never approved).
Dropped both snapshot clauses, build-sbf -> `trigger_refuses_a_distribution_inflated_after_registration`
FAILS (the inflated proposal seals). So it is mutation-sharp, and NOT masked by the seal-time
`total_amount > total_supply` cap — the test inflates WITHIN total_supply (adds entries that fit the
funded pool), isolating the snapshot check as the sole rejector. Sibling guards: the proposal KEY binding
(BB, substituted sibling) and config binding are separately pinned. Restored -> 14 seal green. Verdict:
BLOCKED, no gap. No code/test change.

### [DOUBLY-DEFENDED confirmed + new end-to-end test KEPT] CZ. over-withdraw cap (drain a co-depositor)
Mutation-audited insurance_withdraw's over-withdraw cap `amount > position.principal` (lib.rs:1054). The
existing sole-depositor test `cannot_withdraw_more_than_your_own_recorded_principal` is mutation-BLIND for
this half: with one depositor principal==outstanding, so its over-withdraw amount also trips the sibling
`amount > outstanding` cap. So I built a SHARP-shaped 2-depositor test: alice (principal 1M of outstanding
2M) withdraws 2M (== outstanding, > her 1M) — the sibling cap PASSES (2M !> 2M), so ONLY the
amount>principal cap can reject it (and without it she'd drain bob's 1M). RESULT: even with the cap REMOVED,
the over-withdraw is still BLOCKED — confirming the checkpoint's "doubly-defended by percolator EngineLock":
the WithdrawInsuranceLimited CPI can't pull the whole insurance (the EngineLock keeps insurance >= the
market's domain-budget-remaining), so the over-pull reverts before any drain. So the subledger cap is
mutation-blind but FUNDS ARE SAFE (not a gap — unlike CL/CV which had no backstop). KEPT the new test
`cannot_over_withdraw_to_drain_a_codepositor`: it pins the END-TO-END multi-depositor no-drain guarantee
(passes clean, exercises a real over-withdraw attack blocked by the combined cap+EngineLock) — a scenario
no prior test covered; it would catch a regression of BOTH layers. subledger insurance 38->39, total ->167.
Verdict: BLOCKED (doubly-defended). KEEP.

### [VERIFIED SHARP — distribution append running-sum cap] CY.
Extended the mutation-audit to ACCOUNTING guards. Audited the append per-entry running-sum cap `header.
total_amount > config.total_supply -> reject` (distribution lib.rs:442) — the over-allocation / drain-DOS
guard (a proposal can't promise more than the funded supply, or early claimers drain it and strand late
ones). Mutated to `if false`, build-sbf -> `append_cannot_exceed_total_supply` FAILS (the 60+50=110 > 100
append now succeeds). So it is mutation-sharp; the test pins it at APPEND time (not masked by the seal-time
cap at :478, since the test asserts the append itself is rejected without sealing). The per-append cap +
the checked_add (:43, overflow-safe) + the seal-time cap (:478, defense-in-depth) together bound total_amount
<= total_supply <= vault (init solvency). Restored -> 18 distribution green. Verdict: BLOCKED, no gap. No
code/test change.

### [UNPINNED but SELF-HARM-ONLY -> documented, not pinned] CX. distribution claim vault binding + key-binding audit closeout
Mutation-audited distribution claim's vault binding `vault.key != config.vault` (lib.rs:27): UNPINNED
(dropping it left the suite green). BUT distribution claim is RECIPIENT-GATED — `recipient.is_signer` (:14)
+ `pk != recipient.key` (:47), so only the NAMED recipient claims their own entry. A substitute vault
(config-PDA-owned, recipient-funded) only lets the recipient strand THEIR OWN claim (paid from the decoy,
canonical share stranded) — SELF-HARM, no cross-user LOF. Per CW's lesson, documented not pinned.
KEY-BINDING AUDIT CLOSEOUT (the CL..CX campaign): classified every account key/source binding by reachability:
 - PERMISSIONLESS / cranker-callable (pinned, real cross-user gaps): twap execute holding (CT, was sharp) +
   settlement_usd (CS, fixed) + coin_escrow (CU, fixed); twap claim dest (CN, sharp) + settlement_usd source
   + coin_escrow source (CV, fixed). All now mutation-sharp.
 - OWNER/RECIPIENT-gated (self-harm-only, documented not pinned): cancel_bid coin_escrow source (CW),
   distribution claim vault (CX). A substituted source/vault only strands the caller's own funds.
 - BACKSTOPPED by percolator (untestable-sharp, documented): execute percolator_vault / market_slab /
   percolator_program -> the percolator operator+dest+vault-authority checks reject a substitution (CB);
   a sharp test needs a malicious deployed program (CC, disproportionate).
The campaign found 5 real cross-user mutation-blind gaps (CL recipient-binding, CR one-vote, CS/CU/CV key
bindings) + confirmed 6 guards sharp + correctly declined 2 self-harm + 1 backstopped. Cross-user key-binding
surface is now exhaustively pinned. Verdict: BLOCKED. No code/test change this tick.

### [UNPINNED but SELF-HARM-ONLY -> deliberately not pinned] CW. cancel_bid coin_escrow source binding
Mutation-audited cancel_bid's coin_escrow SOURCE binding `coin_escrow.key != book.coin_escrow` (lib.rs:1682).
Found UNPINNED (dropping it left the chain suite green). BUT unlike the claim source bindings (CV), this is
NOT a cross-user boundary: cancel_bid is OWNER-GATED — it requires `SL_BIDDER == bidder.key` (:1727, "only
the bidder may cancel their own bid") + bidder.is_signer, and refunds to the bidder's pinned canonical ATA.
So a substituted coin_escrow source can only ever strand the CANCELLER'S OWN coin (they fund a decoy, get
refunded from it, lose their real escrow) — pure SELF-HARM, no attacker-vs-victim LOF/DOS. Per KEEP/DELETE
this does not pin a real cross-user boundary, so deliberately NOT given a sharp test (contrast CV's claim
source bindings, exploitable via PERMISSIONLESS claim -> strand a winner's/protocol's funds). The DEST
binding of cancel (coin_ata == recorded canonical ATA) IS the meaningful one and is pinned elsewhere
(eviction-refund + cancel tests). Verdict: BLOCKED (self-harm-only); documented, not pinned. No code/test
change. Lesson: when key-binding-auditing, gate the pin decision on whether the instruction is permissionless
(cranker-reachable -> pin, CV) or owner-gated (self-harm-only -> document, CW).

### [COVERAGE GAP FIXED] CV. claim SOURCE key bindings (settlement_usd + coin_escrow) were UNPINNED
Continued the key-binding audit on claim's SOURCE accounts: `settlement_usd.key != book.settlement_usd`
(lib.rs:1591) + `coin_escrow.key != book.coin_escrow` (:1592) — the accounts claim pays winners/losers FROM
(signed by book_escrow). Both UNPINNED: dropping either left the whole chain suite green (the claim-redirect
tests only substituted the DEST usd_dest/coin_ata, not the SOURCE). Masked danger: a cranker substitutes the
SOURCE with a FUNDED book_escrow-owned account (≠ canonical) -> the winner/loser is paid from the decoy and
the canonical's spent USD / escrowed COIN is STRANDED in the real account (book_escrow-owned, unrecoverable
since claims for that slot are now done) — a self-harm griefing that locks protocol/bidder funds. Empty
decoy reverts on the transfer (masking via insufficiency), so the pins FUND the substitute. FIX: added a
funded settlement_usd source-substitution to `e2e_claim_cannot_redirect_a_winners_payout` (1591) and a
funded coin_escrow source-substitution to `e2e_claim_cannot_redirect_a_losers_coin_refund` (1592), each with
the REAL dest (so only the source binding can reject). Mutation proof: dropping 1591+1592 -> both tests
FAIL; restored -> 73 chain green. Strengthened existing tests (no count change). KEEP. 5th mutation-audit
gap (CL/CR/CS/CU/CV) — the dual key+owner pattern hid BOTH source key bindings behind their (absent here)
sibling, and the existing tests only covered the dest. Source AND dest of every claim transfer now pinned.

### [COVERAGE GAP FIXED] CU. execute coin_escrow KEY binding was UNPINNED (no test caught its removal)
Continued the CS/CT dual-check audit on execute's `coin_escrow` (key binding `coin_escrow.key !=
book.coin_escrow` :1320 + owner check `ce.owner != expected_escrow` :1360). Dropping the KEY binding ->
the ENTIRE chain suite stayed green: it was unpinned (no substitution test existed for it). The owner
check only catches cranker-owned substitutes; a book_escrow-owned substitute (≠ canonical) passes it.
Masked danger: with 1320 gone, a griefer funds a book_escrow-owned coin account and passes it as
coin_escrow -> execute burns total_coin from THAT account, leaving the bidders' bought coin STRANDED
un-burned in the canonical escrow (the protocol paid total_usd but gets no deflation; the bought coin is
permanently stuck since claims only refund coin_refund). An EMPTY substitute just reverts on the burn
(masking via burn-insufficiency), so the pin must FUND the substitute (>= total_coin). FIX: added a
book_escrow-owned `coin_decoy` funded with 400k to `e2e_execute_cranker_cannot_redirect_the_spent_usd`,
asserting execute rejects it. Mutation proof: with 1320 it's rejected; dropping 1320 -> assertion FAILS.
Restored -> 73 chain green. Strengthened existing test (no count change). KEEP. 4th mutation-audit gap
(CL/CR/CS/CU); the dual key+owner pattern keeps hiding the anti-DOS key binding behind the anti-theft owner
check — every such pair needs an owner-PASSING substitute to isolate the key binding (CT did it right).

### [VERIFIED SHARP — execute holding key binding (the CS sibling, done RIGHT)] CT.
Applied the CS lens to execute's `holding` (same dual structure: key binding `holding.key != book.holding`
+ owner check `h.owner != expected_auth`, lib.rs:1352). Dropping the key binding -> ONE test FAILS:
`e2e_execute_pulls_only_burn_share_and_ratchets_principal` (chain.rs:3339, "execute must reject a holding
other than the book's pinned one"). Why it's SHARP (unlike CS): that test's `rogue_holding` is set
twap_authority-OWNED (:3337) — so the owner check PASSES and ONLY the key binding can reject it. That is
the correct way to isolate a key binding from its sibling owner check, exactly what CS's settlement_usd
test was NOT doing (cranker-owned substitute -> masked). So the holding anti-DOS key binding is genuinely
mutation-sharp; no gap. (Contrast: holding test = owner-passing substitute = sharp; settlement_usd test
was owner-failing = blind, now fixed in CS.) Verdict: BLOCKED, no gap. No code/test change.

### [COVERAGE GAP FIXED] CS. execute settlement_usd KEY binding was mutation-BLIND (masked by the owner check)
Mutation-audited execute's settlement_usd validation. It has TWO checks: the KEY binding `settlement_usd ==
book.settlement_usd` (lib.rs:1321) and the OWNER check `su.owner == expected_escrow` (book_escrow PDA,
:1356). Found: dropping the KEY binding left `e2e_execute_cranker_cannot_redirect_the_spent_usd` GREEN.
Root cause (3rd CL-class): the test substituted a CRANKER-owned account, which the OWNER check rejects —
masking the key binding. The two checks defend DIFFERENT things: owner-check = anti-THEFT (can't redirect
to a cranker-controlled account); key-binding = anti-DOS (the spent USD must land in the CANONICAL
settlement account, so winners can claim it). The masked danger: with the key binding gone, a griefer
points settlement_usd at a DIFFERENT book_escrow-owned account (owner check passes, cranker can't extract
it) -> execute parks the USD there -> winners claim from the empty real settlement_usd -> claims revert,
book never reopens, USD STRANDED unrecoverably (no twap ix moves a non-canonical book_escrow account). FIX:
added a book_escrow-owned `decoy` substitute (owner check passes) so ONLY the key binding rejects. Mutation
proof: with the binding the decoy is rejected; dropping :1321 -> the decoy assertion FAILS. Restored -> 73
chain green. Strengthened existing test (no count change). KEEP. 3rd mutation-blind find (sibling check
masking a guard that defends a DIFFERENT failure mode — owner-check masks the anti-DOS key binding).

### [COVERAGE GAP FIXED] CR. gv one-vote-one-proposal guard was mutation-BLIND (masked by a back-out underflow)
Mutation-audited the one-vote-one-proposal guard `ballot.has_live_ballot() && ballot.voted_proposal !=
*proposal_account.key -> reject` (lib.rs:612): it forces a voter to RETRACT before backing a different
proposal, so vote()'s back-out (subtracts the old ballot weight from the PASSED proposal) operates on the
SAME proposal the ballot is on. Found: removing 612, `e2e_voter_cannot_back_two_proposals_without_retracting`
STAYED GREEN. Root cause (CL-class): the test had only alice voting (on A), so B's support_weight/principal
were 0 — backing B made the back-out `checked_sub` UNDERFLOW (0 - alice's weight), and THAT rejected, not
guard 612. The real guard was untested. The masked danger: with 612 gone AND B holding support (another
backer), backing B does NOT underflow — the back-out wrongly subtracts alice's A-weight from B, her ballot
re-points to B, but her weight stays STRANDED on A => phantom weight on A (tally corruption / capture),
B's backer weight cancelled. FIX: inject B's support_weight@72 + support_principal@80 to >> alice's BEFORE
the back-B assertion, so no underflow can mask 612 — the guard is now the sole rejector. Mutation proof:
with 612 the test passes; removing 612 (rebuild gv) -> FAILS (alice backs B). Restored -> 73 chain green.
Strengthened an existing test (no count change). KEEP. This is the 2nd CL-class find: a checked_sub/state
backstop masking a guard — the recurring mutation-blind pattern.

### [VERIFIED SHARP — distribution seal_winner authority binding, both halves] CQ.
Mutation-audited the BB-class guard: seal_winner's authority binding (only the configured seal authority =
gv config PDA may seal -> mint the whole COIN supply). Both halves mutation-sharp, each with its own test:
 - drop `*authority.key != config.authority` (lib.rs:467) -> `seal_then_recipients_claim_their_entries`
   FAILS (its imposter-seal assertion: a signing non-authority seals an arbitrary proposal).
 - drop `!authority.is_signer` (:460) -> `seal_rejects_naming_the_authority_without_its_signature` FAILS
   (an attacker NAMES the real authority as a read-only account and seals with no signature).
Mutated by line. So the seal-authorization is doubly-pinned (key + signature); an unauthorized seal — the
upstream of BB's whole-supply mint — is caught. (BB itself pins the gv-side proposal substitution; this
pins the dist-side authority.) Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — set_vote_lock governance-capture guard] CP.
Mutation-audited the subledger `set_vote_lock` `vote_authority.is_signer` check (lib.rs:1165) — the guard
that stops a voter from SELF-unlocking to bypass retract (only the gv config PDA, via the gv program, can
sign as the vote_authority; a voter passing the config pubkey as a non-signer must be refused). Removing
it (mutate :1165 -> `if false`), build-sbf -> `owner_cannot_self_unlock_a_live_vote_to_exit_capital` (1848)
FAILS: the voter self-unlocks their live-vote position, which would let them withdraw principal while
keeping a capital-less ballot (governance capture / the vote-outlives-capital hole). So the guard is
mutation-sharp; the test correctly pins it. (The owner.is_signer half — hostile authority can't lock a
victim — is separately pinned by the hostile-lock test 1783; the pool.vote_authority binding by AZ.)
Mutated by line. Verdict: BLOCKED, no gap. No code/test change.

### [VERIFIED SHARP — finding-O floor, the principal-protection guard] CO.
Mutation-audited THE most critical guard: the surplus floor `surplus = insurance.saturating_sub(config.
reserved_floor)` (lib.rs:1374) that stops execute from pulling depositor PRINCIPAL as "surplus". Mutated it
to `surplus = insurance` (ignore the floor entirely), build-sbf -> 10+ chain tests FAIL
(e2e_execute_pulls_only_burn_share_and_ratchets_principal, e2e_buy_burn_uniform_price_dutch_auction,
e2e_ratchet/roll/marginal/shutdown/settle tests) because the pull over-reaches into principal and breaks
every burn-amount/ratchet/supply/spent-USD assertion downstream. So the floor is OVERWHELMINGLY
mutation-sharp — the principal-protection invariant is pinned across the whole auction suite, not one test.
Restored -> 73 chain green. (Earlier ticks separately verified the saturating_sub below-floor edge at 4658
and the >100% bps over-pull at 643 + the no-lower-without-Squads auth at 1416.) Verdict: BLOCKED, no gap.
No code/test change.

### [VERIFIED SHARP — twap claim payout-redirect guard, both halves] CN.
Mutation-audited the permissionless-cranker anti-theft guard in claim: `*usd_dest.key != dest_key ||
*coin_ata.key != coin_key -> reject` (lib.rs:1617), which pins both payout dests to the bid's recorded
canonical ATAs. Both halves are mutation-sharp, each with its OWN dedicated test:
 - drop the usd_dest half -> `e2e_claim_cannot_redirect_a_winners_payout` FAILS (cranker redirects the
   winner's settlement USD to itself).
 - drop the coin_ata half -> `e2e_claim_cannot_redirect_a_losers_coin_refund` FAILS (cranker redirects a
   loser's COIN refund to itself).
Mutated by LINE (1617) to avoid the CM parallel-function trap. No gap; both anti-theft halves correctly
pinned. Verdict: BLOCKED. No code/test change.

### [VERIFIED SHARP — insurance_withdraw owner binding + parallel-function mutation trap] CM.
Mutation-audited the insurance_withdraw owner binding `position.owner != *owner.key` (lib.rs:1039), the
core anti-theft (only the position owner exits). Result: VERIFIED mutation-sharp — removing line 1039,
`a_non_owner_cannot_withdraw_a_victims_insurance_principal` FAILS (the attacker drains the victim's
principal to their own ATA). So the test correctly pins it; NO gap (contrast CL, which WAS a gap).
METHODOLOGY TRAP (worth recording): the OWN-VAULT process_withdraw owner check (:604) and the
insurance_withdraw owner check (:1039) have BYTE-IDENTICAL text `if position.owner != *owner.key ||
position.pool != *pool_account.key`. A text-based `replace(old, new, 1)` mutation hits the FIRST (:604,
own-vault) — which the insurance test never exercises — yielding a FALSE "doubly-defended / test passed
under mutation" reading. Same trap as BL's first mutation. FIX: mutate by LINE NUMBER, not text, when a
guard's text is duplicated across parallel functions (own-vault IX 0/1/2 vs insurance IX 3/4/5 mirror each
other). Both owner bindings are real; the insurance one is now confirmed sharp. Verdict: BLOCKED, no gap.
No code/test change.

### [COVERAGE GAP FIXED] CL. distribution claim recipient-binding was mutation-BLIND (anti-theft guard)
Mutation-audited the claim recipient binding `pk != *recipient.key -> IllegalOwner` (lib.rs:541), the core
pull-model anti-theft (only the NAMED recipient claims their entry). Found: removing the guard, the suite
STAYED GREEN. Root cause: `seal_then_recipients_claim_their_entries` asserted "cannot claim bob's entry"
(index 1) only AFTER bob had already claimed index 1 — so that assertion fires on the `amount==0`
double-claim guard, NOT the recipient binding. The CRITICAL anti-theft check was therefore untested
(a regression would let anyone drain any UNCLAIMED entry to their own ATA). FIX: added a sharp assertion
asserting alice (a different valid recipient) cannot claim bob's index-1 entry WHILE IT IS STILL UNCLAIMED
(amount>0) — so only `pk==recipient` can reject — plus a balance check that alice's ATA is unchanged.
Mutation proof: with the guard the new assertion passes; removing `pk != *recipient.key` (rebuild) -> FAILED
("alice cannot claim bob's UNCLAIMED entry", she stole bob's 40); the OLD assertion did NOT catch it.
Restored -> 22 distribution green. Strengthened an existing test (no count change). KEEP. Methodology note:
an assertion can be present-but-mutation-blind when a SECOND guard backstops the same line in the test's
state — mutation-auditing existing critical-guard tests (not just adding new ones) is worth doing.

### [BLOCKED — cross-program COIN-flow independence, no new test] CK.
The genesis COIN is burned in TWO places; confirmed they're independent and non-interfering:
 - twap execute burns from `coin_escrow` == book.coin_escrow, book_escrow-PDA-owned, signed by the
   book_escrow seeds (twap lib.rs:263). It burns only the bidders' deposited COIN it custodies.
 - distribution burn_unclaimed burns from `vault` == config.vault, config-PDA-owned, signed by the
   dist_config seeds (distribution lib.rs:42). It burns only the unclaimed remainder of the distributed pool.
These are DIFFERENT token accounts owned by DIFFERENT PDAs in DIFFERENT programs — neither can sign for or
burn the other's account (the twap can't touch the distribution vault; the distribution can't touch the
auction escrow). Both reduce the LIVE supply of the same fixed-supply COIN (distribution init proved
mint_authority+freeze_authority revoked + supply==total_supply, so no re-mint), so the supply is
monotonically deflationary with no cross-account interference or double-burn. Verdict: BLOCKED. No code/test
change.

### [BLOCKED — balance-manipulation class closed across all programs, no new test] CJ.
Extends CH/CI: no program lets an attacker manipulate a token-account BALANCE to extract value.
 - twap: payouts use recorded book fields; only `budget=holding.amount` reads a balance (CH donor-subsidy);
   donations to escrows are stranded (CI).
 - distribution: claim pays the RECORDED entry `amount` (lib.rs:46/63, not the vault balance); the vault is
   solvency-guaranteed at init (`vault.amount >= total_supply`) + drain-proof (BR); a donation to the vault
   is simply BURNED by burn_unclaimed (`token_balance(vault)`, :37) -> extra deflation, donor self-harm.
 - subledger: insurance_withdraw DOES read live slab insurance for the pro-rata (`read_asset0_insurance`,
   :89), but insurance only changes via AUTHORITY-gated percolator TopUpInsurance (pool pre-handoff /
   squads_vault post-handoff), which records `outstanding` IN LOCKSTEP, so insurance/outstanding stays
   consistent — an external party can't inflate the ratio; and a deposit-then-withdraw into an impaired
   pool is self-harming (the floor haircut applies to the attacker's own deposit too; pinned by the
   splitting test 942). 
Across the stack, payouts track RECORDED state (or an authority-gated, lockstep-consistent live figure);
attacker-controlled balance donations are stranded, burned, or self-harm — never extractable. Verdict:
BLOCKED. No code/test change.

### [BLOCKED — recorded-field accounting principle (donation-stranding), no new test] CI.
Generalizes CH to ALL twap book accounts. The auction's payouts are computed from RECORDED book fields, NOT
live token balances: claim pays `usd_owed` (book_rd SL_USD_OWED, lib.rs:52) + `coin_refund` (SL_COIN_REFUND
:53); settle accumulates total_coin/total_usd from each bid's recorded `c_i`/`usd_i` (:218-219) and the
burn/transfer use those recorded totals (:261-265). The ONLY live-balance read in the whole auction is
`budget = holding.amount` (:137). Consequence: a DONATION to `coin_escrow` or `settlement_usd` is STRANDED —
it never increases anyone's recorded usd_owed/coin_refund, so no claim can extract it (conservation:
Σcoin_refund + total_coin == Σ recorded c_i; Σusd_owed == total_usd). The single balance-read (budget) is
the CH donor-subsidy (also no honest LOF). So balance-based donation manipulation is impossible across the
auction — payouts track the book, not the pot. Verdict: BLOCKED. No code/test change.

### [BLOCKED — donate-to-holding budget manipulation is a non-attack, no new test] CH.
The holding is a known twap_authority-owned ATA (pinned book.holding, execute lib.rs:70), and execute reads
`budget = holding.amount` (:137) — so anyone CAN transfer USD into it to inflate the buyback budget. Probed
for honest-party harm: NONE. A larger budget fills more bids and (since the marginal moves to a lower-rate
bid) LOWERS the clearing price P*, so bidders give LESS coin per USD and COIN holders get MORE deflation —
all funded by the DONOR's self-inflicted loss. A bidder-donor inflating the budget to clear their own
sub-marginal bid pays more USD donation than the USD they receive for their coin (circular, self-harming).
The budget is bounded by the actual balance (no over-spend), and surplus left unspent rolls over / is
shutdown-recoverable by the DAO (BP). Also: `percolator_vault == holding` aliasing is IMPOSSIBLE — percolator
requires percolator_vault to be the canonical vault_authority-owned insurance vault, while holding is
twap_authority-owned (owner mismatch). Verdict: BLOCKED (non-attack: donor self-harm, no honest LOF; no
aliasing). No code/test change.

### [BLOCKED — health check + trigger account-binding completion, no new test] CG.
Health check: 166 GREEN (subledger 50, gv 17, distribution 22, twap 77), all four build-sbf clean, no drift.
Completed the trigger account-binding picture (with CE/CF): all THREE distribution accounts trigger forwards
to seal_winner are exact-key-bound — distribution_program == config.distribution_program, distribution_config
== config.distribution_config, distribution_proposal == pv.distribution_proposal (lib.rs:25-27) — and the
seal_winner CPI INDEPENDENTLY owner-checks both config + proposal (`*.owner != program_id`). So the gv->dist
seal path is doubly-bound (gv-side exact key + dist-side owner) on every account; BB pins the proposal
substitution, 786/822 pin the init bindings. Also re-confirmed the WithdrawInsuranceLimited dest-must-equal-
operator invariant holds for BOTH callers (subledger dest=pool-owned holding, operator=pool; twap
dest=twap_authority-owned holding, operator=twap_authority) — the percolator-side guarantee that makes the
arbitrary-CPI doubly-defended (AU/CB). Verdict: BLOCKED; suite healthy, all cross-program seal/withdraw
account bindings complete. No code/test change.

### [BLOCKED — trigger distribution-read defense + the read-binding asymmetry rationale, no new test] CF.
Complements CE (subledger reads). The gv `trigger` reads the distribution proposal's snapshot (pd[84..88]
entry_count, pd[88..96] total_amount) WITHOUT a disc check — but it is sound via a DIFFERENT, stronger
binding: (a) exact key match `*distribution_proposal.key != pv.distribution_proposal -> reject` (lib.rs:27;
pv.distribution_proposal was fixed at register to the exact registered proposal's key, so the account is
uniquely identified — only a real registered distribution proposal could BE that key), (b) `pd.len() < 96
-> reject` (no OOB), (c) the seal_winner CPI independently checks `proposal_account.owner == distribution_
program`. RATIONALE for the asymmetry (so it isn't mis-flagged as a missing disc check): the subledger
position/pool are PDA-DERIVED bindings (f(pool,voter) / config.subledger_pool), so CE's disc+length+owner
re-validation adds type-safety ON TOP of the derivation; the distribution proposal is bound by an EXACT
STORED KEY (pv.distribution_proposal) — a strictly stronger identity than a derivation — so a disc check
would be redundant. Both cross-program raw reads are sound via the appropriate mechanism. Verdict: BLOCKED.
No code/test change.

### [BLOCKED — cross-program raw-offset reads are disc-guarded (type-cosplay), no new test] CE.
gv reads the subledger Position + Pool via raw byte offsets; verified BOTH readers reject type-cosplay, not
just rely on the PDA+owner binding:
 - `read_sub_position` (lib.rs): `data.len() < 97 || data[..8] != SUB_POSITION_DISC -> reject` (disc +
   length), THEN re-validates the stored pool@8 + owner@40 against expected (defense-in-depth ON TOP of the
   PDA derivation), then reads principal@72 / start_slot@89. So a non-Position account at the position PDA
   (or a mismatched pool/owner) is refused — a voter can't inflate weight by substituting a different
   subledger-owned account.
 - `read_sub_pool_outstanding`: `data.len() < 88 || data[..8] != SUB_POOL_DISC -> reject`, then reads
   outstanding@80 (the live quorum denominator). A non-Pool account is refused.
The disc constants (SUB_POSITION_DISC/SUB_POOL_DISC) + offsets must match the subledger's real
POSITION_DISC/POOL_DISC + layout; pinned implicitly by the real-binary E2E (a mismatch rejects ALL real
positions/pools -> vote/trigger break) and confirmed against the layouts in BD/BK. So the cross-program
raw reads are guarded by disc + length + binding — type-cosplay blocked. Verdict: BLOCKED. No code/test change.

### [BLOCKED — step-5 marginal-test audit, nothing to delete] CD.
Audited the 13 lib unit tests for tautology/redundancy (loop step 5). Verdict: ALL legitimate, none
deletable. subledger payout-policy tests (healthy/impaired/with-surplus/degenerate) pin the pro-rata math;
gv `weight_is_log_time_times_principal` pins the vote-weight formula; twap `cmp_rate_orders_by_coin_per_usd`
pins the rate comparator. The round-trip + offset-overlap tests (subledger/gv state_round_trips, distribution
entry_offsets_are_packed/config/proposal round-trips, twap book_layout_fields_dont_overlap/config) are NOT
tautological: they are the FOUNDATION of the cross-crate raw-offset safety (BD/BK) — gv reads subledger Pool
+ distribution Proposal/Config via hardcoded byte offsets, so a layout drift these catch would break those
bindings. They test at unit level (fast), complementary to (not redundant with) the integration tests
(e.g. lib payout vs the 942 end-to-end split). Nothing marginal/tautological in the suite. Verdict: BLOCKED;
no deletions. No code/test change.

### [BLOCKED — arbitrary-CPI sharp-test decision (CB addendum), no new test] CC.
Considered the ONLY mutation-sharp way to test the execute arbitrary-CPI guard (line 41): a custom DEPLOYED
"evil" program that, on being CPI'd, re-CPIs SPL-token transfer(holding -> attacker) using the propagated
twap_authority signature. With line 41 present, execute rejects the substituted percolator_program (evil
never called, holding intact); with line 41 removed, execute calls evil and the holding drains -> the test
would catch it. DECISION: NOT built. Rationale (proportionality): (a) it requires adding a whole deployable
program CRATE to the repo purely as a test fixture; (b) the exploit is REGRESSION-GATED (only reachable if
line 41 is first removed); (c) it is SURPLUS-ONLY — the holding stages the DAO's buyback budget, NOT
depositor principal (principal lives in the floor-protected percolator insurance vault, untouched by any
holding drain). A defense-in-depth, regression-gated, non-principal guard does not justify a new program
crate + build dependency. The guard is present (line 41) + downstream-CPI-failure-backstopped (CB) +
documented. This decision recorded so future ticks don't re-litigate it. Verdict: BLOCKED. No code/test change.

### [BLOCKED — arbitrary-CPI guard analysis (refines the checkpoint note), no new test] CB.
Every outbound CPI binds its program_id to a STORED value: twap execute/accept_operator percolator_program
== config.percolator_program (execute lib.rs:41); subledger insurance_withdraw/accept_operator
percolator_program == pool.percolator_program; gv trigger distribution_program == config.distribution_program;
gv vote uses config.subledger_program DIRECTLY (no passed account — strongest); distribution makes only
SPL/System CPIs (hardcoded IDs). So an attacker can't redirect a CPI to an arbitrary program.
WHY it's the SOLE guard + why it's litesvm-untestable-sharp (refines the checkpoint's "arbitrary-CPI
pinned-but-untestable"): in execute, line 56 (vault_authority == perc_vault_authority(slab, percolator_program))
does NOT independently block a wrong program — an attacker passes a vault_authority DERIVED from the wrong
program, so line 56 passes; line 41 is the lone program binding. But removing line 41 doesn't yield an
observable exploit in litesvm: a substituted program is either non-deployed (CPI "program not found") or a
real program that rejects the WithdrawInsuranceLimited-shaped data — both revert execute, so a naive
"wrong-program -> rejected" test PASSES with or without the guard (the downstream CPI failure is the
backstop). The ONLY mutation-distinguishing exploit needs a malicious DEPLOYED program that accepts the
call and re-CPIs SPL-token with the propagated twap_authority signature to drain the holding — staging
that in litesvm is disproportionate. Verdict: BLOCKED (guard present + downstream-CPI-failure backstop);
a naive test would be non-sharp/marginal, so deliberately not added per KEEP/DELETE. No code/test change.

### [BLOCKED — rounding-direction sweep (Copenhagen class), no new test] CA.
Enumerated every division across the 4 programs; all are FLOOR and the direction is safe (protocol/principal-
favoring or negligible+non-amplifiable):
 - subledger payout `mul_div_floor(balance, principal, outstanding)` (lib.rs:278): floor favors the POOL
   (pays less under impairment) -> a splitter can only round DOWN, never beat pro-rata or drain a co-depositor.
   Pinned `splitting_an_impaired_exit_cannot_beat_the_pro_rata_or_drain_a_codepositor` (942).
 - twap burnable `surplus * bps / BPS_DENOMINATOR` (:1378): floor favors KEEPING surplus in insurance (pulls
   LESS) -> principal-protective, never over-pulls by rounding.
 - twap marginal `coin_i = mul_div_floor(usd_i, cm, um)` (:1496): floor favors the bidder by <1 atom/bid, but
   bounded by MAX_BIDS=32 atoms total AND non-amplifiable — splitting a bid to harvest more sub-atoms costs a
   bid_fee COIN burn per split, exceeding the gain (and the per-key one-bid rule + 32-slot cap bound N).
   Pinned by the partial-marginal (4783) + zero-coin-marginal (4990) tests.
 - twap `cmp_rate` continued-fraction (:753): a COMPARATOR (quotients compared, not used as a payout value),
   overflow-safe via the u64-leg bound (finding AC) — no rounding value, no LOF.
 - gv vote_weight (log2*principal) + distribution claim (exact stored amounts): no division.
Verdict: BLOCKED; no rounding favors an attacker in an amplifiable way. No code/test change.

### [BLOCKED — strict-boundary precision sweep, no new test] BZ.
Following BY (1-week timelock tightened to the exact boundary), swept every strict-inequality boundary to
check it's pinned at the exact threshold WHERE A LOOSE PIN WOULD MISS AN LOF. Result: all LOF-relevant
strict boundaries are now exact-pinned, and the rest are benign-regression boundaries needing no exact pin.
 - LOF-relevant, EXACT-pinned: 1-week timelock min (BY — 604_799 reject / 604_800 accept); bps over-pull
   cap (reconfigure test already drives exactly 10_001 -> reject, the principal-floor breach side); quorum
   + majority (trigger test drives exactly 50%: 5*2==10 and 4*2==8 rejected, one-past seals); distribution
   claim/burn window cutoff (slot 59 burn-rejected, slot 60 claim-rejected+burn-allowed, BF).
 - benign-regression boundaries (a `>=`<->`>` slip is MORE conservative, never an LOF -> exact pin not
   warranted): reserve filter `cmp_rate < reserve` (at exactly rate==reserve a bid is kept; a slip to
   drop-at-equal just accepts fewer bids — the protocol can't OVERpay, the direction is pinned by the
   fair-bid-clears assertion); cancel cooldown `now >= place_slot + 2*round_length` (a slip needs 1 extra
   slot to cancel — stricter anti-spoof, not looser); execute round gate `clock < round_end` (a slip runs
   execute 1 slot later — stricter). Verdict: BLOCKED; boundary-precision complete on the LOF-bearing set.
   No code/test change.

### [BLOCKED+PINNED — tightened to exact boundary] BY. The 1-week timelock minimum, pinned mutation-sharp
twap `init_config` refuses to bind a multisig whose on-chain `time_lock < MIN_TIMELOCK_SECS` (7*24*60*60 =
604_800, lib.rs:48/410) — the depositor exit-window guarantee. The existing test
`twap_config_rejects_a_multisig_below_the_one_week_timelock` used a 1-DAY negative, pinning the constant
only to the loose RANGE (1 day, 1 week]: a subtle regression (e.g. MIN dropped to 3 days) would slip
through uncaught. Tightened the negative case to exactly `TIMELOCK_1_WEEK_SECS - 1` (604_799), so the
constant is now pinned to EXACTLY 604_800: the 604_799 reject + the 604_800 accept bracket it on both
sides. Mutation proof: lowering MIN_TIMELOCK_SECS by 1 (to 604_799), build-sbf, ran test -> FAILED (the
604_799 multisig was then accepted) — the old 1-day negative would NOT have caught this. Restored -> 73
chain green. Tightened an existing test (no new test, no count change; 166). KEEP.

### [BLOCKED on-chain / task-#6 synthesis, no new test] BX. Proposal-id collision + the task-#6 requirement set
 - distribution create_proposal: PDA = f(config, proposal_id), proposal_id caller-provided, reinit-guarded
   (`data_len()!=0 -> AccountAlreadyInitialized`, lib.rs:33-34). An attacker squatting id=N only makes the
   orchestrator's create at N fail (use another id; u64 id-space). Registration is creator-bound
   (`register_rejects_a_non_creator_front_runner` 466), so an attacker's competing proposal lists THEIR
   own recipients and must win honest quorum+majority to seal — the open-candidate design, not an attack
   (depositors won't back a self-dealing proposal; BB pins that even a winning gv proposal can't redirect
   the seal to a sibling). On-chain SAFE: no LOF, no misdirection.
 - finding S (twap accept_operator's 2nd CPI, insurance authority -> squads_vault): works because percolator
   UpdateAssetAuthority is gated by the ASSET_ADMIN (squads_vault) + the new key co-sign, NOT the current
   authority — so the asset_admin can revoke the pool's deposit authority. percolator independently checks
   the asset_admin, so a wrong squads_vault reverts the CPI. Pinned `e2e_post_handoff_deposit_blocked...` (1605).
SYNTHESIS — the on-chain programs correctly + safely handle every TRUSTED-setup input (reinit guards,
checked arithmetic, PDA bindings, the Squads time_lock>=1wk check), so the residual risk is the ORCHESTRATOR
providing sane inputs / selecting unused PDAs. The task-#6 (off-harness) setup-validation requirements
surfaced by this loop: (1) deposit-deadline/kickstart timing [BH], (2) a SANE claim_window_slots far below
u64::MAX [BU], (3) UNUSED proposal-id selection [BX, this], (4) handover bound to the winner [on-chain
binding pinned by BB; the orchestrator must wire the winning COIN to the Squads handoff], (5) durable
1-week timelock [enforced on-chain at twap init_config]. These are the genuine open items; none is an
on-chain LOF. Verdict: BLOCKED. No code/test change.

### [BLOCKED — init-squat vote_authority + remaining-account smuggling, no new test] BW.
 - init_insurance_pool vote_authority squat: the pool's vote_authority is set from a caller-provided
   account with NO init-time validation (by design), reinit-guarded (data_len()!=0 -> AccountAlreadyInitialized,
   lib.rs:39). The defense is downstream: gv `init_config` REQUIRES pool.vote_authority == the gv PDA
   (finding G/H), so a front-run squat with a wrong authority can never be bound by the genesis. Pinned by
   the `poison_pool_vote_authority` cases (seal.rs:757-772 — attacker-key pool rejected, gv-PDA pool
   accepted) + `gv_config_cannot_be_bound_to_a_substituted_pool` (822) + the subledger squat tests
   (init_insurance_pool_cannot_be_squatted 2292, non-canonical-vault 2024). No LOF/misdirection; a squat
   only forces the orchestrator to a different sub_pool (off-harness, task #6).
 - remaining-account smuggling: every instruction reads a FIXED account list via next_account_info and
   validates each by key/owner/PDA, so a substituted account fails its check; the only conditionally-read
   trailing accounts (execute's coin_sink in SEND mode, place_bid's evict_acct on a full book) are each
   pinned to book.coin_sink / the evicted bid's recorded ATA. Extra accounts beyond the fixed list are
   ignored; a missing required trailing account reverts (NotEnoughAccountKeys). No smuggling path.
Verdict: BLOCKED. No code/test change.

### [BLOCKED — health check + reconfigure bps bounds, no new test] BV.
Full-suite health check: 166 GREEN (subledger 50, gv 17, distribution 22, twap 77), all four build-sbf
clean, no drift since BO. twap `reconfigure` (surplus_buy_burn_bps): `new_bps > BPS_DENOMINATOR` rejected
(lib.rs:473) so it can't be set above 100% to over-pull the floor — pinned `reconfigure_rejects_a_bps_above_the_denominator_that_would_overpull_the_floor`
(643) + auth pinned `e2e_reconfigure_rejects_a_non_signing_or_forged_vault` (4319). The 0% (no buyback)
and 100% (burn-all-surplus) extremes are valid DAO choices, both principal-protected by the finding-O
floor (only surplus above it is ever pulled) — no LOF, so not separately pinned. Verdict: BLOCKED; suite
healthy, reconfigure bounded + authorized. No code/test change.

### [SAFE on-chain / task-#6 setup note, no bug] BU. Unbounded claim_window_slots (absurd value bricks claim+burn)
Probed: distribution `init_config` rejects `claim_window_slots == 0` but sets NO upper bound (lib.rs:276).
`window_end = seal_slot + claim_window_slots` is computed with `checked_add` in BOTH claim (:527) and
burn_unclaimed (:599). So a near-u64::MAX window makes window_end OVERFLOW -> checked_add ERRORS (no wrap,
no LOF) -> both claim and burn revert -> the vault is stuck (no payout, no deflation). On-chain verdict:
SAFE — the arithmetic is checked (errors, never wraps/over/under-pays), so there is no fund-loss bug; the
"stuck vault" is purely a setter-created footgun. NOT externally reachable: init_config's params come from
the trusted genesis orchestrator, and gv only ever seals the distribution_config whose authority == the gv
PDA (gv-init binding dc[72..104], pinned 786) — a parasite config with an absurd window is never used.
TASK-#6 REQUIREMENT (off-harness, parallel to BH's deposit-deadline): the orchestrator must set a SANE
claim_window_slots (a bounded window, far below u64::MAX) so claims close and burn_unclaimed can run.
Deliberately NOT adding an on-chain upper bound — picking a "sane max" is a trusted-setup policy decision
for task #6, not an arbitrary constant to bake into the program (the program is already overflow-safe).
Verdict: BLOCKED (no on-chain LOF); recorded as a task-#6 input-validation item. No code/test change.

### [BLOCKED — cancel/claim settled split + eviction reset + holding intermediary, no new test] BT.
Three more sharp distinctions drilled, all pinned/sound:
 - cancel-vs-claim double-spend: a SETTLED bid must use `claim` (refunds only the UNFILLED portion), while
   `cancel_bid` refunds the FULL escrow. cancel_bid rejects a settled slot (`SL_SETTLED != 0`, lib.rs ~42),
   so a settled loser can't be over-refunded via cancel on top of the settle's burn/payout. Pinned
   mutation-sharp by `e2e_cancel_cannot_double_spend_a_settled_bid` (4686): rejected cancel leaves escrow
   untouched, claim then refunds exactly 7 COIN once.
 - eviction slot reset: place_bid overwrites an evicted slot with USD_OWED=0/COIN_REFUND=0/SETTLED=0, and
   eviction only happens in an OPEN book (bids unsettled), so no stale settled-state can leak into a reused
   slot. The evicted bidder gets a full refund to their canonical ATA (4594).
 - insurance holding intermediary: insurance_withdraw is self-contained (pull `owed` into the pool-owned
   holding, transfer exactly `owed` out), so a shared holding or a donated pre-balance cannot over-pay or
   cross-contaminate — only `owed` (the floor pro-rata) ever leaves to the owner. Holding must be pool-owned
   + right mint (pinned `insurance_deposit_rejects_a_non_pool_holding` 369).
Verdict: BLOCKED. No code/test change.

### [BLOCKED — execute budget-conservation invariant, no new test] BS.
Core auction-money safety: `total_usd` (moved holding->settlement on a settle) can NEVER exceed the holding
balance, so the settle transfer can't revert (DOS) and no bid is paid for COIN it doesn't deliver. Proven
by construction: budget = holding.amount (lib.rs:137); loop (c) sets `remaining = budget` and each fill is
`min(remaining, u)` with `remaining -= fill`, so Σ(usd_owed) <= budget; loop (d) adds to total_usd ONLY in
the `usd_i > 0 && coin_i > 0` branch (:219), so a zero-coin marginal (positive residual, floor(usd*cm/um)
== 0) is EXCLUDED — its usd_owed is reset to 0 (no USD for zero COIN). Therefore total_usd <= budget =
holding.amount and the transfer at :265 always succeeds; the unspent remainder rolls over (BP). Conservation
on the COIN side mirrors it (coin_i <= c_i since the bid's rate >= P*, so Σcoin_i <= escrow; burn/refund
drain it exactly). Pinned at ALL boundaries: full-budget-spend (e2e_reserve_blocks... BO/4017, total_usd
== budget), partial marginal residual (e2e_uniform_price_partial_marginal_fill 4783), and zero-coin marginal
not paid (e2e_settle_with_a_zero_coin_marginal_pays_no_usd_for_zero_coin 4990). Verdict: BLOCKED. No
code/test change.

### [BLOCKED — vault-mover enumeration (drain-proof), no new test] BR.
Enumerated every `invoke_signed` that signs with a vault-authority PDA, to prove no arbitrary-transfer
path exists out of the value-bearing accounts:
 - distribution VAULT (holds the ENTIRE genesis COIN supply, config-PDA-owned): the `[b"dist_config",
   coin_mint, authority]` seeds sign in exactly 3 spots — init_config (allocates the CONFIG account, not
   the vault), `claim` (:550, vault->recipient_ata), `burn_unclaimed` (:609, burn). So the vault has only
   TWO movers, both fully validated: claim pays the NAMED recipient (pk==signer) the entry's EXACT amount
   then zeroes it (no replay, no cross-recipient, no arbitrary amount/dest beyond the recipient's own
   choice), and burn only after the window (deflation, no dest). seal_winner/create_proposal/append never
   touch the vault. There is NO instruction that lets the config PDA sign a caller-specified transfer ->
   the supply is structurally drain-proof. (claim/burn validations pinned by the distribution suite.)
 - subledger insurance vault (percolator-owned): moved only via percolator WithdrawInsuranceLimited
   (pool-PDA operator-signed, bounded by `owed` = floor pro-rata) -> pool-owned holding -> owner_ata;
   doubly-bounded by the position/outstanding caps + percolator's own EngineLock. (finding L + AU.)
 - twap coin_escrow/settlement_usd (book_escrow-owned) + holding (twap_authority-owned): movers are
   place_bid/cancel (escrow in/out), execute (burn/send + spend), claim (payouts), shutdown (DAO sweep) —
   all with pinned destination/amount validation (AV/AW/AX, BO, shutdown tests).
Every value account's signing authority is exercised only in enumerable, validated movers; no PDA can be
coerced into an arbitrary transfer. Verdict: BLOCKED. No code/test change.

### [BLOCKED — vote-tally consistency + eviction + lifecycle-ordering drill (BO lens), no new test] BQ.
Applied BO's "which set does the loop/accounting touch" lens to three more spots; all sound + pinned:
 - gv tally invariant `Σ(proposal.support_weight) == config.total_cast_weight`: the vote back-out
   (lib.rs:618-622) subtracts ballot.voted_weight from BOTH the passed proposal's support AND the global
   total_cast, gated by line 612 (passed proposal must equal ballot.voted_proposal) so it can only ever
   touch the SAME proposal it added to. Pinned via `e2e_tied_weight_between_proposals_deadlocks_until_broken`
   (chain.rs:2580): a 50/50 tie (support_A=support_B=w, total_cast=2w) blocks BOTH triggers (w*2 !> 2w),
   and breaking it (a backer shifts, decrementing that proposal's support AND total_cast) lets A seal —
   behavior that only holds if the invariant is exact. Reinforced by retract/reback (2328) + back-two (1989).
 - eviction protects good bids: place_bid's full-book weakest-scan finds the global MIN-rate slot and
   evicts only for a STRICTLY-better incoming bid, so a sub-reserve/low bid can only ever displace an
   even-worse one — a high-rate bid is never evicted by a bad one. Evicted bidder gets a full refund to
   their canonical ATA. Pinned `e2e_full_book_evicts_only_for_a_strictly_better_bid` (4594).
 - lifecycle ordering: the handoff (accept_operator), seal (trigger), and pull (execute) are each gated —
   trigger needs quorum+majority; execute's WithdrawInsuranceLimited fails pre-handoff (twap not yet the
   operator); the handoff is Squads-timelock'd (DAO-sequenced, 1-week notice). Out-of-order attempts fail
   safely or are DAO governance choices, never an external LOF; principal stays floor-protected throughout.
Verdict: BLOCKED; tally accounting + eviction + ordering all sound. No code/test change.

### [BLOCKED — multi-round rollover budget conservation (doubly-defended), no new test] BP.
Probed: execute ALWAYS pulls `burnable` into the holding (step 2) BEFORE clearing the book (step 4), so
cranking execute on an EMPTY / all-sub-reserve book with surplus>0 pulls the budget then ROLLS (total_coin
==0), leaving the staged budget sitting in the holding to roll over to a later round. Traced conservation:
round 1 (surplus 500k, bps 80%) pulls 400k + ratchets floor 1M->1.1M; insurance becomes 1.1M == floor, so
round 2 `surplus = 1.1M - 1.1M = 0` -> pulls 0 MORE -> budget = the 400k rollover, spent when bids arrive.
No double-pull (ratchet zeroes the next surplus), and the finding-O floor INDEPENDENTLY caps any pull at
the reserved principal even if the ratchet regressed (doubly-defended). The staged 400k is twap_authority-
owned, so it is never lost — spent next round, or shutdown-swept by the DAO. A griefer cranking execute on
an empty book only PRE-STAGES the DAO's own surplus (bounded by the floor); not an LOF, not a drain.
Pinned machinery: roll-undo / committed-bid survival (`e2e_roll_with_committed_bid_settles_correctly_next_round`
4871, finding AE), ratchet across rounds (`e2e_ratchet_pulls_fresh_surplus_across_rounds`), below-floor
no-pull (4658). The empty-book-pull-then-rollover path itself is low-severity (no LOF, doubly-defended,
recoverable) so deliberately not given a dedicated test per KEEP/DELETE. Verdict: BLOCKED. No code/test change.

### [BLOCKED+PINNED] BO. Filtered (below-reserve) bid recovery in a mixed settle — settle-ALL-occupied vs eligible-only
Vector/analysis: execute's settle has two passes — (a) builds the ELIGIBLE set (occupied, positive,
rate>=reserve) and sorts it; (d) the payout loop. A below-reserve bid is EXCLUDED from the eligible set
in (a). The correctness hinge is that loop (d) walks ALL occupied slots (`for i in 0..MAX_BIDS`), so a
filtered bid still gets SETTLED with a FULL coin refund (usd_owed stayed 0 -> the else branch) — claimable
immediately. If (d) instead iterated only the eligible set, a filtered bid would stay OCCUPIED+unsettled
after a real settle: not claimable, and (worse) it keeps the book from ever reopening (the reopen scan
sees an OCCUPIED slot) — a cheap below-reserve bid would WEDGE the auction until a cooldown-cancel. The
roll path (total_coin==0) is separately safe (full restore, finding AE).
Verdict: BLOCKED. Pinned by EXTENDING `e2e_reserve_blocks_expensive_bid_from_draining_surplus`: after the
fair bid settles, the attacker's filtered sub-reserve bid is claimed (full 1-COIN refund), the winner
claims, and a fresh bid proves the book REOPENED. Mutation proof: changing loop (d) to iterate `idx[0..n]`
(eligible-only), build-sbf — MY extension FAILED (filtered bid unsettled -> claim InvalidAccountData) while
`e2e_closing_refund_ata_cannot_permanently_brick_the_book` (3830, an ELIGIBLE loser) still PASSED, proving
the boundary was genuinely uncovered (3830's loser is in the eligible set, settled even by the buggy loop).
Restored -> 73 chain green. Extended an existing test (no redundant new one); count unchanged (166). KEEP.

### [BLOCKED — health check + deposit duplicate-account/double-withdraw confirmations, no new test] BN.
Full-suite health check: 166 GREEN (subledger 50, genesis-vote 17, distribution 22, twap 77), all four
build-sbf clean — matches checkpoint, no drift since BL. Analyzed two more vectors, both safe:
 - duplicate-account in insurance_deposit: aliasing `owner_ata == holding` is impossible — the source
   transfer (owner_ata -> holding) is OWNER-signed while holding must be POOL-owned (validated), so a
   pool-owned owner_ata can't be owner-signed; `percolator_vault == holding` is blocked by distinct owners
   (vault = vault_authority-owned, holding = pool-owned). No CPI/role confusion.
 - own-vault double-withdraw (process_withdraw IX 2): triply-defended — after a full exit the position is
   `withdrawn=true` AND `principal==0` (line 607 rejects on either) AND the vault is drained (a second
   payout would be pro_rata of balance 0 = 0). No double-drain.
 - handoff re-run: re-calling accept_operator (either side) is Squads/asset_admin-gated and idempotent
   (re-sets the same authority/operator); a non-DAO actor cannot drive it (require signer). The DAO
   re-granting the operator back to the pool is the DOCUMENTED exit-recovery path, not an attack.
Verdict: BLOCKED; suite healthy, deposit aliasing + double-withdraw + handoff-rerun all safe. No code/test change.

### [BLOCKED — terminal-state/reinit layer completion, no new test] BM.
Following BL (insurance re-deposit into a retired position), swept the rest of the terminal-state/reinit
layer across all programs; remaining members are pinned, no fresh gap:
 - gv post-seal capital recovery (LOF): a WINNING voter is vote-locked; they must be able to RETRACT even
   after the winner is sealed (vote line 553 blocks only VOTE_BACK when pv.executed, NOT retract) to clear
   the subledger lock and withdraw — else their principal is frozen forever. Pinned by
   `winning_voter_can_retract_and_exit_after_finalize` (insurance_percolator.rs:1790).
 - distribution append-after-seal (LOF): once a proposal seals, `append_entries` refuses (header.sealed ||
   config.is_sealed(), lib.rs:420) so the creator cannot inject a self-dealing entry into the unallocated
   headroom (e.g. 40 of 100) and grab COIN that was meant to be burned as deflation. Pinned comprehensively
   by `append_to_a_sealed_winner_cannot_grab_the_unallocated_headroom` (distribution.rs:570): append
   rejected, no phantom entry, honest 60 paid, 40 burned not captured.
 - distribution config reinit: `a_sealed_config_cannot_be_reinitialized...` (311) + the runtime
   allocate-on-program-owned backstop (documented doubly-defended).
 - gv re-trigger of the SAME executed proposal blocked by pv.executed; a SECOND proposal resealing blocked
   by distribution is_sealed — `a_second_proposal_cannot_reseal_after_a_winner_is_sealed` (728).
 - position/book/proposal/config reinit all guarded by data_len()!=0 / entry_count==0 + lamport-prefund
   tolerance (finding AI); BL pinned the insurance position re-deposit.
DELIBERATELY NOT PINNED (marginal): the OWN-VAULT process_deposit has the SAME terminal `if p.withdrawn`
guard (lib.rs:517) as insurance, but own-vault pools are not voted on, so a re-deposit into a retired
own-vault position is pure SELF-HARM (stuck own funds, no systemic quorum drag — the systemic effect is
exactly what made BL worth pinning). Per the KEEP/DELETE rule it stays unpinned. Verdict: BLOCKED;
terminal-state/reinit layer saturated. No code/test change.

### [BLOCKED+PINNED] BL. Re-deposit into a RETIRED insurance position (stuck funds + systemic quorum drag)
Vector: a position PDA is f(pool, owner), so after a full exit (principal 0, withdrawn=true) the owner
keeps the SAME terminal PDA. `process_insurance_deposit`'s position-load guard rejects a re-deposit via
the `|| p.withdrawn` clause (lib.rs:897). Without it, a re-deposit records principal>0 while `withdrawn`
stays true (deposit never clears the flag) → the funds can NEVER be withdrawn (insurance_withdraw rejects
withdrawn positions, :607) = STUCK, AND `pool.outstanding_principal` is inflated by that stuck principal,
permanently dragging the genesis QUORUM DENOMINATOR (trigger reads outstanding live) for EVERY voter — so
although the action is owner-initiated, the quorum drag is SYSTEMIC, not pure self-harm.
Verdict: BLOCKED. Pinned by `cannot_redeposit_into_a_retired_position` (insurance_percolator.rs): deposit
→ full exit (retired) → re-deposit REFUSED, outstanding stays 0, funds untouched. Mutation proof: dropping
the `|| p.withdrawn` clause at :897, build-sbf, ran test → FAILED (re-deposit succeeded + outstanding
inflated); restored → 38 insurance tests green. NOTE/methodology: my first mutation hit the WRONG guard —
the own-vault `process_deposit`'s separate `if p.withdrawn` at :517 — and the test stayed green (14.5k CU,
no CPI), which correctly located that insurance_deposit has its OWN guard at :897; re-aimed there and the
mutation bit. Distinct from `cannot_vote_with_a_withdrawn_position` (2458), which pins only the VOTE side
of a retired position, not re-deposit. KEEP.

### [BLOCKED — binding-immutability invariant across all 4 programs, no new test] BK.
Completed the BJ immutability sweep on the other three programs by enumerating every post-init write to
each account's fields. The security-critical binding/identity fields are NEVER reassigned after init in
any program — only designed accumulators/lifecycle flags change:
 - subledger Pool: ONLY `outstanding_principal` mutates (deposit/withdraw/insurance accounting, lib.rs
   536/643/951). `vote_authority`, `vault`, `policy`, `mint`, `market_slab`, `percolator_program` are
   immutable (the only occurrence of vote_authority outside init is a `==` check at :1185). => the entire
   vote-lock cross-program contract (AZ) rests on an immutable authority; an attacker cannot repoint
   pool.vote_authority to lock/unlock victims or self-unlock to bypass retract.
 - distribution Config: ONLY `sealed_proposal` + `seal_slot` change post-init (one-time, at seal_winner,
   guarded by !is_sealed, :483-484). `authority`, `vault`, `total_supply`, `coin_mint` are immutable =>
   claim solvency (total_amount<=total_supply<=vault) is stable forever; the seal authority + vault can't
   be repointed to redirect or over-draw the pool.
 - genesis-vote Config: NO post-init write to ANY binding (coin_mint / distribution_program /
   distribution_config / subledger_program / subledger_pool) — grep empty; only the tallies
   (total_voted_principal / total_cast_weight / outstanding_principal) mutate in `vote`. => BB's
   trigger routing + the gv-init fail-fast bindings (786/822) rest on immutable wiring.
With BJ (twap book), all four programs are confirmed: cross-program guards read FROZEN bindings, so the
"mutate a binding field to break an isolation/cross-program guard" class is closed everywhere. (Immutable
by absence of a setter — not separately litesvm-testable, but every binding test exercises the frozen
value and the round-trip/layout tests pin the field positions.) Verdict: BLOCKED. No code/test change.

### [BLOCKED — book-identity immutability invariant, no new test] BJ.
Probed the "mutate a binding/identity field post-init to break an isolation guard" class on the twap book.
Enumerated every writer of the book account: the IDENTITY fields — BK_CONFIG, BK_COIN_MINT,
BK_COLLATERAL_MINT, BK_COIN_ESCROW, BK_SETTLEMENT_USD, BK_HOLDING, BK_BOOK_BUMP, BK_ESCROW_BUMP — and
BK_ROUND_LENGTH are written ONLY at init_book (lib.rs:960-975). The DAO setters touch strictly their own
economic params and NOTHING else: set_reserve → BK_RESERVE_NUM/DEN, set_coin_sink → BK_SINK_MODE/BK_COIN_SINK,
set_bid_fee → BK_BID_FEE; reconfigure + set_reserved_floor mutate the CONFIG (bps / reserved_floor), not
the book. Consequences:
 - Every book-binding guard (execute/claim/place_bid/cancel checking book.coin_escrow / settlement_usd /
   holding / config / mints) reads IMMUTABLE values — so a "substitute then mutate the book to match" or
   "repoint an escrow" attack is structurally impossible; identity is frozen at init (which is itself
   Squads-gated + escrow-owner-validated, see BG/finding P/AS).
 - round_length immutability makes the anti-spoof cancel cooldown (place_slot + round_length*2, read at
   cancel time) STABLE — a bid placed under a long round can't be made early-cancellable by shrinking the
   round, because no instruction can shrink it. (Defends the issue-#28 cooldown from a config-mutation
   bypass; the no-op-roll bypass is separately pinned by e2e_roll_does_not_unlock_cancel_before_aging.)
 - The only MUTABLE book fields are the economic params (reserve/sink/fee, DAO-gated) + lifecycle state
   (state, round_end, slots) written by execute/claim/place_bid/cancel. `book_layout_fields_dont_overlap`
   (twap lib test) pins the offsets don't collide, so a setter cannot clobber an identity field via overlap.
Verdict: BLOCKED; book identity is immutable post-init, closing the mutate-binding-field class. No code/test change.

### [BLOCKED — economic/Sybil/lifecycle layer, no new test] BI.
Swept the incentive-security layer (auction + vote game theory); all properties pinned, no fresh gap:
 - Vote Sybil-resistance: weight = floor(log2(hold))*principal is LINEAR in principal, so splitting one
   position into N (each needing its own owner key + deposit) yields the same total weight — pinned
   `e2e_sybil_splitting_gives_no_vote_advantage` (2458). Positions are NON-TRANSFERABLE (no transfer
   instruction; position.owner is set only at create, lines 502/885), so aged (high-hold) weight cannot
   be bought/acquired; top-up resets start_slot (BF/BH). Capital outweighs hold-time so an early dust
   squatter cannot out-weight later real capital — pinned `e2e_capital_outweighs_hold_time_no_early_squatter_capture`
   (2252). Splitting an impaired exit cannot beat pro-rata or drain a co-depositor — `splitting_an_impaired_exit...`
   (942).
 - Auction pricing fairness: uniform marginal price P* with every filled bid paying the SAME P* (better
   bidders refunded the surplus COIN); the reserve rate is the DAO floor on USD-per-COIN so the protocol
   never overpays below it (bids cheaper than reserve are dropped) — pinned `e2e_reserve_blocks_expensive_bid_from_draining_surplus`.
   A bidder cannot extract more than P*; wash/self-dealing just sells COIN into the buyback at the clearing
   price (the intended deflation mechanism). Depositor PRINCIPAL is never spent (only surplus above the
   finding-O floor), so even a malicious post-genesis DAO setting a high reserve spends only its OWN
   surplus, not principal.
 - Lifecycle: there is NO on-chain `kickstart`/`finalize` instruction in percolator-meta (subledger tags
   are 0-7, gv 0/2/3/4); "kickstart" (50/50 deploy, deposit-deadline) and "finalize" are ORCHESTRATION
   steps (task #6, off-harness). The trigger IS the on-chain finalization (BB/seal tests). The
   permissionless winner-take-all trigger has no front-running benefit (it only seals the legitimate
   majority winner; the cranker gains nothing).
Verdict: BLOCKED; incentive-security layer saturated. No code/test change.

### [BY DESIGN — documented liveness trade-off, no bug] BH. Mid-vote deposit raises the quorum denominator (stall, not LOF)
Probed: `insurance_deposit` has NO phase/lifecycle gate — it stays open through the vote phase (until the
operator handoff revokes the pool's insurance authority, finding S). So a hostile actor CAN deposit
mid-vote to grow the LIVE pool `outstanding`, which `trigger` re-reads at seal time, pushing the quorum
bar (`total_voted_principal*2 > live_outstanding`) above the existing voters and blocking the winner from
sealing. Analysis: this is the INTENDED anti-minority-capture mechanism, not a bug — trigger deliberately
reads live outstanding so a small group that voted early (when the pool was tiny) cannot capture the
distribution after honest capital later grows the pool without a re-vote. That GOOD direction is pinned by
`trigger_uses_live_pool_outstanding_not_stale_cache` (seal.rs:692): a 6-principal early voter is rejected
once the live pool is 1006, and trigger proceeds only when a real quorum re-forms. The stall flip-side is:
 - NOT an LOF — no funds move; voters' principal is untouched and exitable; the griefer's own deposit is
   at market risk and (since they did NOT vote) is NOT vote-locked, so they can withdraw it any time,
   which immediately drops the bar back.
 - NOT permanent — maintaining the stall requires the griefer to keep capital parked at risk; honest
   participants counter by voting more principal (raising the numerator) or waiting them out; the griefer
   voting their own stake instead RESTORES quorum and gets outvoted.
 - Cheap only against a marginal quorum (bar just over 50%); a strong quorum needs the griefer to add a
   large fraction of the pool.
Surfaces a DESIGN-INTENT question for task #6 (off-harness): the money-map says "deposit open until
kickstart", but on-chain deposits are open until the HANDOFF — if the orchestration tool intends a hard
deposit deadline at kickstart (before voting), enforcing it would also remove this stall surface. That is
an orchestration-policy decision, not an on-chain LOF. Verdict: BLOCKED (anti-minority-capture pinned);
the stall is an accepted, documented liveness property. No code/test change.

### [BLOCKED — own-vault path + CPI-vault + PDA-collision sweep, no new test] BG.
Examined the least-documented surface this tick — the subledger OWN-VAULT path (IX 0/1/2, the
non-insurance pool) — plus two adjacent classes; all blocked, no fresh gap:
 - own-vault `process_withdraw` (IX 2): reuses the SAME `payout` pro-rata function as insurance, but the
   own-vault is never externally impaired — its vault is pool-PDA-owned and ONLY the subledger withdraw
   moves funds out, so balance == outstanding always → payout returns principal 1:1 (the pro-rata branch
   is effectively dead for own-vault, present for symmetry). A donation to the vault self-harms the donor
   (surplus stranded under POLICY_PRINCIPAL, or fairly redistributed under POLICY_WITH_SURPLUS) — no
   theft. The `is_insurance()` guard fail-fasts type confusion (own-vault withdraw on an insurance pool
   would try to sign as the pool for percolator's vault and fail anyway). Owner-bound, withdrawn-once,
   principal<=outstanding cap, pool PDA re-derived for trusted signing seeds. (Pinned by the 6 own-vault
   tests incl. `cannot_drain_a_foreign_pool_with_a_position_from_another_pool`.)
 - execute → WithdrawInsuranceLimited vault substitution: the DESTINATION (`holding`) is pinned to
   book.holding + twap_authority-owned, so the withdrawn insurance can only land in the twap's holding;
   the SOURCE (`percolator_vault`) need not be twap-validated because percolator itself requires it to be
   the canonical insurance vault for the slab — doubly-defended, no redirect.
 - PDA seed collisions: all program PDAs use distinct seed prefixes (twap config/book/`twap_book_escrow`/
   `twap_authority`; gv `gv_config`/`gv_ballot`/`gv_proposal`; dist `dist_config`/`dist_proposal`;
   sub `subledger_pool`/position). Distinct first-seed → distinct hash; find_program_address also binds
   program_id. No intra- or cross-program collision.
Verdict: BLOCKED; own-vault + CPI-vault + PDA-namespace layers saturated. No code/test change this tick.

### [BLOCKED — boundary/arith confirmations, no new test] BF.
Three more boundaries verified safe + pinned this tick:
 - vote_weight overflow: `vote_weight(principal, age) = age.ilog2().saturating_mul(principal)`
   (genesis-vote lib.rs:109) uses SATURATING mul, so log2(age)*principal can never wrap; the quorum +
   majority checks cast to u128 before `*2` (no overflow); total_cast_weight accumulates via checked_add
   (an overflowing add reverts the whale's OWN vote, not an exploit). Saturation only caps an absurd
   weight at u64::MAX = legitimate dominance, never a manipulation. (age<2 / principal==0 → 0 weight,
   the too-recent/unfunded guard, pinned by a_too_recent_position_cannot_vote_or_pump_the_quorum + co.)
 - distribution claim/burn window EXACT boundary: claim rejects `slot >= window_end`, burn_unclaimed
   rejects `slot < window_end` — complementary strict thresholds at the same window_end, so the cutoff is
   a clean partition (no slot where both run → no claim-vs-burn race that could burn a late claimant's
   funds; no slot where neither runs). The exact equality IS pinned: `unclaimed_is_burned_after_window`
   sets slot==window_end(60) and asserts claim REJECTED + burn ALLOWED there; `burn_unclaimed_is_rejected_during_the_claim_window`
   asserts burn REJECTED at window_end-1(59). A regression of claim's `>=` to `>` (overlap at the cutoff)
   would be caught.
 - phantom-capital: insurance_deposit unconditionally resets `start_slot = Clock::slot` on every top-up
   (subledger lib.rs:153), so fresh capital can't inherit an old hold-time; pinned by
   `top_up_resets_the_position_start_slot` (2532) + the vote-weight-timing suite.
Verdict: BLOCKED; weight-arith + window-cutoff + deposit-timing layers saturated, no fresh gap.

### [BLOCKED — health check + finding-T/recovery confirmations, no new test] BE.
Full-suite health check: 165 tests GREEN (subledger 49, genesis-vote 17, distribution 22, twap 77), all
four programs build-sbf clean — matches the checkpoint, no drift. Deep-verified this tick:
 - Finding-T offset canaries are NON-tautological: twap `insurance_offset_matches_real_percolator_slab`
   pins INSURANCE_OFFSET via `core::mem::offset_of!(percolator::MarketGroupV16HeaderAccount, insurance)`
   against the REAL struct, asserts vault≠insurance, AND functionally proves the read returns insurance
   (not the adjacent vault) by funding insurance + bumping the vault field to a distinct sentinel. The
   subledger has its own `offset_of!` canary. A percolator layout drift fails LOUDLY.
 - place_bid eviction with a CLOSED weakest-bidder canonical ATA is recoverable (recreate the ATA →
   eviction refunds), pinned by `e2e_closed_weakest_ata_cannot_permanently_block_eviction` (5154) —
   the eviction sibling of the claim-path `e2e_closing_refund_ata...` recovery.
 - Copenhagen residue: sysvar-spoof N/A (all clock reads are the `Clock::get()` syscall, no passed
   sysvar account to forge); rent/exempt covered by create_pda_robust + the lamport-prefund DOS tests.
Verdict: BLOCKED, no fresh gap; suite healthy.

### [BLOCKED — layer sweep, no new test] BD. Mint-trust, cross-crate raw-offset contracts, and proposal sizing
Swept three more layers this tick; all blocked + pinned (explicitly or sharply-implicitly), no fresh gap:
 - SPL-mint trust (distribution init_config): the COIN mint must have BOTH mint_authority AND
   freeze_authority revoked (lib.rs:295) — else the freeze authority could freeze the vault or a
   recipient's ATA (DOS all claims) or the mint authority could dilute past the fixed pool. Pinned by
   `init_config_rejects_a_mintable_coin` (710) and `init_config_rejects_a_freezable_coin` (766), plus
   supply==total + vault-solvency (304/318). The twap COIN is the SAME distribution-validated fixed COIN;
   the twap trusts its DAO-configured coin_mint (an external attacker cannot change config.coin_mint).
 - Cross-crate raw-offset contracts (finding-T class, but in-repo): gv reads the subledger Pool via
   hardcoded offsets (vote_authority sp[160..192], outstanding sp[80..88]) and the distribution proposal
   via pd[84..88]=entry_count / pd[88..96]=total_amount, with NO shared type across the crates. Verified
   the offsets MATCH the real serializers today (subledger Pool::serialize writes vote_authority@160,
   outstanding@80; distribution ProposalHeader entry_count@84, total_amount@88). They are SHARPLY pinned
   by the real-binary harnesses: a subledger-pool drift breaks gv `init_config`'s vote_authority binding
   (hard-fails the full chain.rs genesis E2E at init); a distribution-proposal drift breaks gv `trigger`'s
   bait-and-switch snapshot AND the BB redirect test (both use REAL distribution proposals), and the
   total_amount offset is independently asserted by `append_cannot_exceed_total_supply` (distribution.rs).
   An explicit canary would be redundant with this real-binary coverage.
 - distribution `create_proposal` sizing: account size = PROPOSAL_HEADER + capacity*ENTRY_SIZE (derived
   from the bounded capacity, `0 < capacity <= MAX_ENTRIES`), and `append` rejects entry_count>=capacity,
   so every entry write is in-bounds; a hypothetical mismatch would panic (DOS) on the safe slice index,
   never corrupt. Reinit-guarded (data_len!=0). No OOB.
Verdict: BLOCKED; mint-trust + cross-crate-offset + sizing layers saturated. No code/test change this tick.

### [BLOCKED — class sweep, no new test] BC. Cross-program-binding layer swept (the class BB belonged to)
After BB exposed a real gap in the higher-order CPI bindings, swept every permissionless instruction that
CPIs another program with a caller-provided "target" account, applying the BB lens (substitute a VALID
sibling whose single binding check is the only thing standing between the caller and a mis-routed
mint/transfer). All remaining members are bound AND pinned; BB was the lone gap:
 - gv `trigger` → distribution `SealWinner`: distribution_proposal (BB, now pinned), distribution_config
   + distribution_program (`config.*`), sub_pool (`config.subledger_pool`, owner-checked, read LIVE), and
   the pv.executed re-trigger guard. The seal authority is the gv config PDA via invoke_signed.
 - twap `claim`: settlement_usd + coin_escrow bound to `book.*`, book_escrow re-derived from config, payout
   dests pinned to the bid's recorded canonical ATAs (AW/AX). No cross-book drain.
 - twap `execute` → percolator WithdrawInsuranceLimited: vault/market/operator all bound; budget is READ
   from the holding post-CPI (not trusted from the requested amount).
 - twap `accept_operator` → percolator UpdateAssetAuthority: finding S (BA), market/program/twap_authority
   bound, Squads-vault co-sign.
 - subledger `accept_operator` → percolator UpdateAssetAuthority (the GRANT side): market_slab + program
   bound to `pool.*`, pool PDA re-derived for trusted seeds, and percolator requires the current
   asset_admin to co-sign — so the pool can only ever receive the operator role for ITS OWN recorded
   market, and only from that market's current admin. (asset_admin-gated; no permissionless reach.)
 - gv `init_config` fail-fast bindings: distribution config seal-authority == gv PDA + coin match (pinned
   `init_config_rejects_a_distribution_not_authority_bound_to_this_config`), subledger pool vote_authority
   == gv PDA (pinned `gv_config_cannot_be_bound_to_a_substituted_pool`).
 - distribution `claim`/`burn_unclaimed`: vault + proposal/sealed_proposal bound; burn is deflation (no
   theft target).
Also re-derived the execute marginal-fill conservation (coin_i = floor(usd_i*Cm/Um) <= C_i for every
filled bid since r_i >= P*, so Σcoin_i <= escrow and refunds drain it to 0 exactly — no over-burn /
over-refund; pinned by the partial-fill + zero-coin-marginal tests). Verdict: BLOCKED, the cross-program
binding class is saturated post-BB; no code/test change this tick.

### [BLOCKED+PINNED] BB. trigger redirects the COIN mint to a sibling distribution proposal (whole-supply LOF)
Vector: gv `trigger` is permissionless and CPIs distribution `SealWinner` with whatever
`distribution_proposal` account the caller passes. The seal authority is the gv config PDA, so whatever
proposal reaches SealWinner gets the ENTIRE genesis COIN supply minted to its recipients. The only guard
binding the seal to the proposal voters backed is `*distribution_proposal.key != pv.distribution_proposal`
(lib.rs:716; `pv.distribution_proposal` is fixed at register). Attack: register the legit proposal P
(bound to winner G), drive G to quorum+majority, then `trigger(G, Q)` where Q is the attacker's SIBLING —
created under the SAME distribution config, with the SAME (entry_count=1, total_amount=100) = P's
registered snapshot, but the attacker as sole recipient. The matching snapshot is the sharp part: the
anti bait-and-switch guard (`pd[84..88]/pd[88..96] == pv.snapshot_*`) compares SHAPE, not identity, so it
does NOT catch Q; and Q.config == dist_config + total <= total_supply means SealWinner would accept Q.
Only the key-binding check stops the redirect.
Verdict: BLOCKED. Pinned by `trigger_cannot_redirect_to_a_sibling_distribution_proposal` (seal.rs): the
redirect trigger(G,Q) is refused, nothing is sealed, and the honest trigger(G,P) still seals P. Mutation
proof: deleting the `|| *distribution_proposal.key != pv.distribution_proposal` clause (leaving the
program+config checks), build-sbf, ran test → FAILED (Q sailed through the snapshot guard and SEALED —
the attacker would mint the whole supply); restored + rebuilt → 14 seal tests green. This is distinct
from `register_rejects_a_proposal_from_a_foreign_distribution_config` (register-side, foreign CONFIG) and
`trigger_refuses_a_distribution_inflated_after_registration` (same-proposal MUTATION) — it pins the
trigger-time SUBSTITUTION of a valid sibling, which neither covered. KEEP.

### [BLOCKED — re-audit, no new test] BA. Per-instruction sweep completed (accept_operator/finding S was the last un-drilled handler)
Closed the per-instruction attestation by drilling the four boundaries not yet re-confirmed this session;
all are already pinned mutation-sharp, no fresh external-LOF vector:
 - twap `accept_operator` (the handoff endpoint, IX 3): gated by `require_squads_vault` (vault is_signer +
   canonical index-0 vault of the bound multisig) + slab/program/twap_authority bindings. It atomically
   (1) rotates the asset-0 insurance OPERATOR to twap_authority and (2) FINDING S — rotates the insurance
   AUTHORITY (kind 1, which gates TopUpInsurance/deposits) to the Squads vault, so NO deposit can enter
   market-0 insurance post-handoff. Without (2), a post-handoff deposit would raise insurance above the
   static floor and a permissionless cranker would drain that fresh principal as "surplus" (LOF). Pinned
   end-to-end by `e2e_post_handoff_deposit_blocked_by_authority_revoke` (chain.rs:1605) — the post-handoff
   subledger deposit is rejected.
 - twap `execute` surplus underflow: `surplus = insurance.saturating_sub(reserved_floor)` — when the market
   draws insurance BELOW the ratcheted floor, surplus clamps to 0 (no pull, floor intact) rather than
   wrapping to ~u128::MAX and draining the whole vault. Pinned mutation-sharp by
   `e2e_execute_pulls_nothing_when_insurance_below_floor` (4658): insurance set to 800k under a 1M floor,
   execute `.expect()`s SUCCESS with 0 pulled + vault untouched — a wrapping/checked regression makes
   execute error and fails the test.
 - the shared `require_squads_vault` primitive (used by every set_*/shutdown/init_book/accept_operator):
   checks BOTH `is_signer` AND key == canonical vault. Pinned in BOTH directions by
   `e2e_attacker_cannot_lower_surplus_floor_without_squads` (1413) — ATTACK 1 wrong-key signer
   (IllegalOwner), ATTACK 2 the REAL vault pubkey as a NON-signer (MissingRequiredSignature). One pin
   covers the primitive for all callers.
 - twap `init_book`: Squads-gated AND hard-validates escrows — coin_escrow/settlement_usd must be owned by
   the program's book_escrow PDA (mint-matched, amount 0), holding owned by twap_authority, `data_len()!=0`
   reinit guard, finding-AS self-loop guard. An attacker cannot create the book; even the DAO cannot wire
   attacker-owned escrows (bidder COIN only ever lands in a program-controlled PDA escrow).
ATTESTATION CLOSURE: every instruction across all four programs — twap (13: init_config, reconfigure,
accept_operator, set_reserved_floor, init_book, set_reserve, place_bid, execute, claim, set_coin_sink,
shutdown, set_bid_fee, cancel_bid), subledger (8: init_pool, deposit, withdraw, init_insurance_pool,
insurance_deposit, insurance_withdraw, set_vote_lock, accept_operator), genesis-vote (4: init_config,
register_proposal, vote, trigger), distribution (6: init_config, create_proposal, append_entries,
seal_winner, claim, burn_unclaimed) — now has its external-attacker boundaries pinned mutation-sharp or
documented (DAO-footgun / runtime-backstopped / vestigial). Verdict: BLOCKED, on-chain surface saturated;
no code/test change this tick.

### [BLOCKED — re-audit, no new test] AZ. Bidirectional vote-lock cross-program boundary (gv retract/back ↔ subledger set_vote_lock)
Deep re-audit this tick of the "a vote can never outlive the capital backing it" invariant, which spans
TWO programs and is the load-bearing Sybil guard for the whole governance bootstrap. Findings — all
already pinned, no fresh honest-user LOF:
 - gv `vote` (back/retract, lib.rs:519): proposal bound to config via `pv.config == config_account`
   (:545); ballot bound to voter (:605); one-vote-one-proposal forces the passed proposal to match the
   live ballot before any tally mutation (:612); back-out uses `checked_sub` on all four tallies
   (:619-622) so a retract can only remove the SAME weight it added to the SAME proposal; the lock is
   toggled by CPI carrying BOTH the config-PDA (vote_authority) signature AND the voter signature
   (:662). Quorum denominator is re-read live from the pool (:579), never the cached field.
 - subledger `set_vote_lock` (the other half): requires `vote_authority.is_signer` AND `owner.is_signer`
   AND `pool.vote_authority == vote_authority.key`. Doubly-gated by design — owner-sig stops a hostile
   authority freezing a victim (pinned by `set_vote_lock_requires_owner_sig`/hostile-lock test,
   insurance_percolator.rs:1783); authority-sig stops the owner SELF-UNLOCKING to bypass retract and
   exit capital while keeping a live ballot (pinned by `owner_cannot_self_unlock_a_live_vote_to_exit_capital`
   :1848 — alice naming the gv config as authority WITHOUT its signature is refused, position stays
   locked). Withdraw-while-locked is refused and retract clears the lock (`vote_locked_principal_cannot_exit_until_retracted`
   :1578). insurance_withdraw rejects `position.vote_locked` (subledger lib.rs ~:?), and a top-up of a
   voted position neither inflates nor unlocks the vote (`topping_up_a_voted_position...`).
 - The pro-rata insurance haircut (finding L) was re-checked for a rounding/order LOF: `payout` uses
   `mul_div_floor(balance, principal, outstanding)` — floor ALWAYS favors the pool, splitting a withdraw
   into sub-atom amounts where `owed` rounds to 0 is SELF-HARM (principal decremented, 0 paid), and
   rounding-down only raises the remaining depositors' insurance/outstanding ratio. Last-exiter dust is
   ≤1 atom. No co-depositor LOF. Already covered by the finding-L pro-rata test.
MARGINAL (recorded so it isn't re-derived): the gv vote-side `pv.config == config_account` check (:545)
has no DIRECT test, but a direct test would be marginal — a foreign-config proposal belongs to a foreign
election with a foreign COIN+distribution; backing it with config-A capital pushes only the FOREIGN
election (which the attacker themselves created and whose COIN they receive), leaving honest config-A's
tally, pool, and COIN entirely untouched. No honest-party LOF; the check is isolation hygiene against a
self-created parasite config, not a fund-loss boundary. Per the loop's KEEP/DELETE rule, not pinned.
Verdict: BLOCKED. The bidirectional vote-lock boundary is saturated; no code/test change this tick.

### [BLOCKED+PINNED] AY. place_bid oversized leg (> u64) → cmp_bid cross-multiply overflow + phantom escrow (LOF)
Vector: twap `place_bid` parses both bid legs (`coin_atoms`, `usdc_atoms`) as u128 from the instruction
data, but the ranking comparator `cmp_bid(a,b) = (coin_a*usdc_b).cmp(coin_b*usdc_a)` is a DIRECT
cross-multiply that is only overflow-safe because both legs are bounded to u64 (u64*u64 < 2^128). The
binding guard is the two `as_u64(coin_atoms)?` / `as_u64(usdc_atoms)?` calls (lib.rs:1114-1115), which
reject any leg ≥ 2^64 up front — before the signer/escrow/balance logic. A hostile bidder submitting a
leg of exactly 2^64 is rejected, nothing is escrowed, funds intact.
Why it's load-bearing (mutation-sharp): if the guard regressed to a truncating `coin_atoms as u64`, a
bid with `coin_atoms = 2^64` would truncate to 0 COIN ESCROWED while the book still records the full
2^64 (via `book_wr_u128(SL_COIN, coin_atoms)`). That bid then (a) overflows `cmp_bid` against other
bids (corrupting the rank/eviction order) and (b) claims 2^64 COIN it never paid for — at execute it
wins the entire USD budget for ~zero real COIN: a direct depositor-surplus LOF. The continued-fraction
`cmp_rate` (reserve filter) is separately overflow-safe against the unbounded-u128 DAO reserve and is
division-by-zero-safe (each recursion's new denominators are nonzero remainders); the two comparators
are consistent and deliberate (cmp_bid = cheap, both-bid u64 path; cmp_rate = reserve path).
Verdict: BLOCKED. Pinned by `e2e_place_bid_rejects_a_leg_above_u64` (rejects both the coin-leg and
usd-leg overshoot, asserts zero escrowed + bidder COIN intact, then a legal bid still escrows). Mutation
proof: replacing `as_u64(coin_atoms)?` with `coin_atoms as u64`, build-sbf, ran test → FAILED ("a coin
leg of 2^64 must be rejected"); restored + rebuilt → 73 chain tests green. This was the one finding-AC
boundary (the u64 bound that keeps cmp_bid safe) that was asserted in a code comment but not yet pinned
by a test. KEEP.

### [BLOCKED+PINNED] append rejects malformed entries (zero amount / zero-address recipient)
Vector: append_entries rejects amount == 0 || pk == Pubkey::default() per entry (lib.rs:428). A zero-amount
entry is permanently unclaimable and soaks a slot; a default-pubkey (zero address) entry allocates a chunk
of the FIXED supply to a key nobody can ever sign for — locking that COIN out of every real recipient (it
sits unclaimable and burns). The guard keeps the sealed distribution list well-formed. Single-guard for the
APPEND path (the claim-side amount==0 / pk-match checks gate CLAIMING, not the write, so they don't backstop
the malformed APPEND).
Verified BLOCKED + mutation-SHARP: a zero-amount entry, a default-pubkey entry, and a mixed chunk with one
bad entry are each rejected; the rejects write NOTHING (a following clean append is the FIRST entry,
entry_count 1 / total 60), confirming the whole-chunk atomicity. Neutering `amount == 0 || pk ==
Pubkey::default()` at :428 -> `if false` + rebuilding the .so lets the malformed entries append (test fails).
Completes append's guard coverage: creator-binding + supply-cap + sealed-freeze + bait-and-switch snapshot +
now per-entry well-formedness.
Test KEPT: append_rejects_a_zero_amount_or_default_pubkey_entry (distribution integration 18).

### [BLOCKED+PINNED] Register refuses an empty proposal (entry_count == 0)
Vector: register requires entry_count > 0 (lib.rs:476) so only a FULLY-built distribution becomes votable.
An empty proposal would freeze a (0,0) snapshot: (a) if it WON, the sealed distribution names no recipients
and the entire funded vault burns unclaimed (a total recipient LOF); or (b) registering empty then appending
makes the live proposal mismatch the (0,0) snapshot forever, so trigger could never seal it — a
permanently-unwinnable, vote-soaking proposal that can stall the genesis if it draws support. Single-guard.
Verified BLOCKED + mutation-SHARP: a proposal created but never appended (entry_count 0) is refused at
register (no gv proposal-vote account created); a 1-entry proposal by the SAME creator registers fine (the
gate is emptiness, not the creator). Removing the `entry_count == 0` reject at :476 + rebuilding the gv .so
lets the empty proposal register (test fails). Complements the creator-binding + foreign-config-binding +
snapshot bait-and-switch register pins — register now requires a complete, own, this-genesis proposal.
Test KEPT: register_rejects_an_empty_proposal (gv seal 13).

### [HARDENING/DOUBLY-DEFENDED] Reinit of a sealed distribution config (vault-redirect) — runtime backstop
Vector: re-initializing a LIVE, sealed distribution config would reset config.sealed_proposal + seal_slot,
un-sealing it so an attacker could re-seal to THEIR proposal and redirect the whole COIN vault (or re-open
the claim window). Built the e2e attack (seal a winner, then re-init the config) -> BLOCKED, the seal is
intact and the original winner still claims her full 100. But the mutation check is the finding: neutering
the explicit `data_len != 0` reject (lib.rs:285 -> `if false`) leaves the test GREEN — the reinit STILL
fails because create_pda_robust's System allocate only runs on a system-owned, data-empty account, and a
live config is distribution-OWNED (assigned at first init). So the data_len check is defense-in-depth over
that runtime backstop, not the sole guard. (Same doubly-defended shape as the over-withdraw cap, whose
backstop is the percolator EngineLock — a mutation that stays green locates the true guard.)
Test KEPT as end-to-end safety, completing the reinit coverage across all three configs (subledger-pool / gv
single-guard-pinned earlier; this one documents the distribution config's runtime-backstopped variant):
a_sealed_config_cannot_be_reinitialized_to_redirect_the_vault (distribution integration 17).

### [BLOCKED+PINNED] Premature burn_unclaimed before the seal would torch the funded vault (permissionless DOS)
Vector: burn_unclaimed is PERMISSIONLESS and refuses to run until config.is_sealed() (lib.rs:590). Before the
genesis vote seals a winner the distribution vault is FUNDED with the full supply but undistributed, so a
premature burn would destroy the ENTIRE supply and NO recipient could ever be paid — a catastrophic
attacker-reachable DOS/LOF. The is_sealed() check is the SOLE guard here: before any seal config.seal_slot ==
0, so window_end = seal_slot + claim_window == claim_window; once the genesis runs PAST claim_window slots
the window-gate (clock < window_end) no longer blocks a burn, leaving only is_sealed() between an attacker
and the vault. (The during-window burn test pins the window gate; this pins the seal gate, isolated by
warping past claim_window.)
Verified BLOCKED + mutation-SHARP: config funded 100, NOT sealed, warp to slot 60 (> window 50); a
permissionless burn_unclaimed is refused, vault stays 100. Neutering `if !config.is_sealed()` at :590 ->
`if false` + rebuilding the .so lets the premature burn pass the (already-elapsed) window gate and torch the
vault (test fails).
Test KEPT: burn_unclaimed_before_the_genesis_seals_cannot_torch_the_vault (distribution integration 16).

### [BLOCKED+PINNED] Distribution init_config rejects a zero claim window (recipient-LOF DOS)
Vector: init_config rejects claim_window_slots == 0 || total_supply == 0 (lib.rs:276). A ZERO claim window
is catastrophic — window_end = seal_slot + 0 = seal_slot, so claim (clock < window_end) is refused the
instant the winner seals, and burn_unclaimed (clock >= window_end) immediately torches the WHOLE vault:
every recipient loses their entire COIN allocation with no chance to claim. The guard blocks the config at
creation. (Orchestrator footgun reach — the config is authority-bound — but the consequence is a total
recipient LOF, so worth pinning.)
Verified BLOCKED + mutation-SHARP on the claim-window clause: build init with window 0 (fully-funded supply
100) -> rejected; a window-50 + funded-100 config -> accepted. Dropping the `claim_window_slots == 0` clause
from :276 + rebuilding the .so lets the zero-window config init (test fails). NOTE: the sibling
`total_supply == 0` clause is DOUBLY-defended (the anti-hoarding mint.supply == total_supply check rejects a
0 supply when the mint holds >0, already pinned by init_config_rejects_a_mintable_coin) — so it is not
single-guard; I dropped that assertion rather than ship a non-sharp one (the mutation that left it green was
the tell).
Test KEPT: init_config_rejects_a_zero_claim_window (distribution integration 15).

### [BLOCKED+PINNED] Fully-impaired exit must retire without a zero-amount CPI (no quorum-DOS)
Vector: under a TOTAL loss insurance is wiped to 0, so a depositor's owed = floor(0*amount/outstanding) = 0.
percolator REJECTS a zero-amount WithdrawInsuranceLimited (amount==0 -> InvalidInstruction), so
insurance_withdraw guards the CPI behind `if owed > 0` (lib.rs:1081) and still retires the position (state
update only). Without that guard a wiped depositor could NEVER retire — their lost principal would stay in
pool.outstanding forever, permanently inflating the genesis quorum denominator (the trigger reads
pool.outstanding LIVE), bricking finalize. A correctness/DOS boundary distinct from the pro-rata haircut
(owed > 0, finding L).
Verified BLOCKED + mutation-SHARP: impair the slab+vault to insurance 0; alice exits her worthless position
-> succeeds, 0 payout, position retired (withdrawn), pool.outstanding -> 0. Changing `if owed > 0` to
`if true` + rebuilding the .so makes the exit issue WithdrawInsuranceLimited(0), which percolator rejects ->
the fully-impaired depositor is stuck (test fails).
Test KEPT: a_fully_impaired_exit_still_retires_the_position_without_a_zero_amount_cpi (subledger insurance 37).

### [BLOCKED+PINNED] Trigger requires a STRICT majority/quorum (a tie is not enough)
Vector: trigger seals the winner-take-all distribution only if total_voted_principal*2 > live_outstanding
AND support_weight*2 > total_cast_weight (lib.rs:743,748, strict `* 2 <= ... -> reject`). The existing tests
use clearly-below (4/10) and clearly-above (10/10); the EXACT-50% boundary was untested. A `>` -> `>=`
regression (i.e. `<=` -> `<` in the reject conditions) would let a MINORITY holding exactly half the
principal, or a proposal with exactly half the cast weight, capture 100% of the COIN supply on a TIE.
Verified BLOCKED + mutation-SHARP on BOTH inequalities (via inject_tally + the real distribution seal CPI):
exactly-50% principal (voted 5/10, 5*2==10) is refused; exactly-50% cast weight (support 4/8, 4*2==8) is
refused; one unit past both ties (6/10 and 5/8) seals. Mutating the QUORUM `<=`->`<` makes the 5/10 case
seal (test fails); mutating the MAJORITY `<=`->`<` makes the 4/8 case seal (test fails) — each sub-assertion
independently pins its own strict inequality.
Test KEPT: trigger_requires_a_strict_majority_and_quorum_not_a_tie (gv seal 12).

### [BLOCKED+PINNED] Settle with a zero-COIN marginal: never pay USD for 0 COIN (overpay LOF)
Vector: in a real SETTLE (total_coin > 0) the marginal bid can get a residual budget so small that
coin_i = floor(usd_i * cm/um) == 0. execute treats any coin_i == 0 fill as UNFILLED (usd_owed -> 0, full
COIN refund). Without the `coin_i > 0` half of the fill guard, that marginal bidder is credited
usd_owed = residual (free USD) AND a full COIN refund — receiving USD while handing over 0 COIN and keeping
all of it. (Generalizes: with a low P* any 0-rounding winner would be paid for nothing.)
e2e_roll_with_a_marginal_zero_coin_fill pins the all-zero ROLL case (total_coin==0 -> roll); this pins the
zero-coin marginal INSIDE a real settle, which was untested.
Verified BLOCKED + mutation-SHARP: alice fills 399_600 of the 400k budget; bob (marginal, rate 1/500) gets
the 400-USD residual -> coin_i = floor(400/500) = 0 -> unfilled. After the settle, settlement_usd == 399_600
(NOT 400_000 — bob's residual not spent), holding == 400 (rolled over), and bob's claim pays 0 USD + refunds
his full 1 COIN. Dropping `&& coin_i > 0` from the fill condition + rebuilding the .so credits bob the 400
residual USD for 0 COIN (test fails).
Test KEPT: e2e_settle_with_a_zero_coin_marginal_pays_no_usd_for_zero_coin (chain 72).

### [BLOCKED+PINNED] Claim reopen-scan: a partial claim must keep the book SETTLED (no mid-drain corruption)
Vector: claim flips BK_STATE back to OPEN only when NO slot remains occupied (lib.rs:1639, the `if !any`
scan after zeroing the claimed slot). If a PARTIAL claim reopened the book prematurely, a fresh place_bid
(or execute) would run against a half-settled book — landing a new bid amid still-SETTLED slots carrying
usd_owed/coin_refund, which the next execute would re-process (double-settle the pending winner). Single-
guard, pure-state (the scan).
Verified BLOCKED + mutation-SHARP: two equal winners settle; after alice claims slot 0 (bob's slot 1 still
parked), a new place_bid is REFUSED (book still SETTLED) and escrows nothing; once bob claims slot 1 (last
slot drained) the scan reopens the book and the same place_bid is ACCEPTED. Changing the scan to `if true`
(always reopen) + rebuilding the .so lets the mid-drain bid land and the test fails. Extended in place into
e2e_claim_cannot_be_replayed_to_drain_other_winners (chain 71) — the two-bidder settle setup was already
there; only the reopen-state assertions were missing.

### [BLOCKED+PINNED] Deposit != vote: top-up while voted doesn't inflate the tally nor unlock the pledge
Probed the deposit x vote-lock INTERACTION (untested): insurance_deposit checks p.withdrawn but NOT
vote_locked and never touches the gv tallies (gv-owned state). So a voter may top up while a ballot is live.
Two properties matter: (a) the top-up must NOT silently raise counted weight/principal — else it would inject
vote power bypassing vote's weight/age/backout path (a Sybil-relevant inflation); (b) it must NOT unlock the
at-risk capital. The only way to count fresh capital is a re-vote, which resets the age clock
(top_up_resets... pins the reset; this pins the no-inflation interaction).
Verified: alice votes (weight 10*P, locked) -> tops up P (allowed, no DOS) -> tally STAYS at (10*P, P)
(extra capital uncounted), position grew to 2P but stays vote-locked, and a withdraw of the topped-up
position is refused until retract. The unlock half is mutation-covered by the existing vote_locked guard
(vote_locked_principal_cannot_exit); the NO-INFLATION half is a behavioral/regression guard (no single line
to mutate — deposit simply doesn't touch gv state), catching any future change that made deposit a vote.
Test KEPT: topping_up_a_voted_position_does_not_inflate_or_unlock_the_vote (subledger insurance 36).
Also confirmed covered this tick: place_bid on a SETTLED book is rejected (book.state==OPEN, pinned by the
refund-ATA brick tests at chain.rs:3807/4433); one-bid-per-bidder pinned (e2e_buy_burn:3275).

### [BLOCKED+PINNED] Cross-proposal phantom inflation (one-vote-one-proposal guard, single-guard)
Vector: a voter with a live ballot on proposal A must RETRACT before backing proposal B (lib.rs:612). Subtler
than same-proposal double-count: the re-vote backout subtracts ballot.voted_weight from the PASSED proposal,
so backing a DIFFERENT proposal B subtracts A's weight from B (corrupting B / underflowing if B is empty)
while leaving A's tally UNTOUCHED — a PHANTOM weight stranded on A that no live ballot backs, inflating A's
weighted-majority share. line 612 is the clean guard. Single-guard, no backstop (pure gv tallies).
Verified BLOCKED + mutation-SHARP: bob backs B (so B has support == alice's, no underflow on the mutation
path), alice backs A; alice (live on A) backing B WITHOUT retracting is REFUSED, A and B tallies intact,
global cast == exactly two votes; then the LEGIT switch (retract A -> back B) moves alice cleanly (A->0,
B->both). Neutering line 612 (-> `if false`) + rebuilding the gv .so lets the cross-vote land and corrupt the
tallies (test fails). Companion to re_voting_the_same_proposal (same-proposal door); together they pin the
full one-position-one-live-contribution invariant.
Test KEPT: cannot_back_a_second_proposal_without_retracting_the_first (subledger insurance 35, real gv path).

### [BLOCKED+PINNED] Re-vote weight inflation (the backout is single-guard, no sibling backstop)
Vector: `vote` backs out the ballot's prior live contribution from BOTH the proposal's support_weight/
support_principal AND the global total_cast_weight/total_voted_principal BEFORE re-adding the fresh weight
(lib.rs:618-622). Without that backout a voter could call vote N times on the SAME proposal and have ONE
position's weight + principal counted N times — inflating that proposal's weighted-majority share AND the
quorum numerator with a single deposit. A Sybil-free governance-capture attack.
Single-guard, NO backstop: unlike the over-withdraw cap (percolator EngineLock backstops it), the gv tallies
are gv-OWNED state with no CPI, so the in-handler backout is the SOLE guard. Existing vote tests only
covered single votes / re-vote-after-withdraw (weight 0); the double-count path was untested.
Verified BLOCKED + mutation-SHARP: alice votes -> weight 10*principal, global cast 10*principal; she votes
the SAME proposal twice more -> tallies STAY at exactly one vote (not 2x/3x). Removing the 4 checked_sub
backout lines at :618-622 + rebuilding the gv .so makes the re-vote DOUBLE the weight (test fails).
Test KEPT: re_voting_the_same_proposal_does_not_double_count_weight (subledger insurance 34, real gv path).

### [HARDENING/DOUBLY-DEFENDED] Over-withdraw drain capped — percolator EngineLock backstops the subledger cap
Probed insurance_withdraw's amount cap (lib.rs:1054, `amount <= position.principal && amount <= outstanding`)
as a presumed single-guard per-depositor protection (drain co-depositors by withdrawing > your principal).
Built the e2e attack (alice 1M + bob 1M; alice withdraws 2M) — BLOCKED, funds safe. But the mutation check
surfaced a real finding: removing the `amount <= position.principal` half does NOT let the drain through
right after deposits, because percolator's OWN insurance >= domain-budget-remaining invariant (EngineLock)
rejects any WithdrawInsuranceLimited that would drop insurance below the funded budgets. So the over-withdraw
is DOUBLY-defended (subledger per-caller cap + percolator EngineLock). The subledger cap is the sole
load-bearing guard only once the market has SPENT its budgets so insurance > budget-remaining (a state not
constructed here); in the budget-tracking state percolator backstops it. The cap also prevents the
position.principal u64 underflow — moot when the CPI reverts, load-bearing if percolator ever allowed the
pull. Distinct from the owner-half theft (single-guard, pinned above): that has no percolator backstop
(percolator can't tell depositors apart), this one does.
Test KEPT as end-to-end hardening: cannot_withdraw_more_than_your_own_recorded_principal (insurance 33) —
verifies the principal-only invariant holds against the REAL binaries (no underflow, co-depositor safe),
comment corrected to state the doubly-defended nature (not "sole guard"). Methodology note: a mutation that
leaves the test green is itself a finding — it locates the TRUE backstop (here: a sibling invariant).

### [BLOCKED+PINNED] Non-owner insurance-principal theft (owner half, genesis-critical, was untested)
Vector: insurance_withdraw re-derives the POOL PDA but NOT the position PDA, so `position.owner == owner`
(lib.rs:1039) is the SOLE guard that only the depositor pulls their at-risk principal. An attacker who SIGNS
(account-0 owner = attacker) could pass the VICTIM's position and route the payout to their own ATA, stealing
the victim's insurance principal — the genesis path where the real money lives. The OWN-VAULT path had
non_owner_cannot_withdraw_another_position; the INSURANCE owner-half had no equivalent (the prior tick pinned
the POOL half of the own-vault check; this pins the OWNER half of the insurance check — different door).
Verified BLOCKED + mutation-SHARP: victim deposits 1M; attacker signs an insurance_withdraw whose position is
the victim's and whose dest is the attacker's ATA -> rejected, perc_vault still 1M, attacker gets 0, victim's
position intact, then the genuine owner exits for the full 1M. Removing the `position.owner != *owner.key`
half from :1039 + rebuilding the .so makes the theft SUCCEED (test fails). This insurance owner-check is
single-guard sharp (no position-PDA re-derivation backstop), unlike the vote path's foreign-position
rejection which is doubly-defended (sub_position == PDA(pool,voter) at :565 AND read_sub_position owner field
at :215 — neither single removal lets a whale-position vote through, so no clean mutation test there).
Test KEPT: a_non_owner_cannot_withdraw_a_victims_insurance_principal (subledger insurance 32).

### [BLOCKED+PINNED] Cross-pool drain on own-vault withdraw (the pool half of the owner/pool guard)
Vector: own-vault process_withdraw checks `position.owner == owner && position.pool == pool_account`
(lib.rs:604). non_owner_cannot_withdraw_another_position pins the OWNER half; the POOL half was untested.
Without `position.pool == pool_account`, an attacker who holds a real position in pool-A (their own deposit)
could pass that position alongside pool-B's pool + vault and withdraw against pool-B — using pool-A's
principal to drain a DIFFERENT own-vault pool's vault (another depositor's funds). A cross-pool LOF.
(Insurance pools are singleton asset-0 so not cross-constructible; the own-vault assets-1..N pools are the
realizable multi-pool case.)
Verified BLOCKED + mutation-SHARP: two own-vault pools (asset 1 / asset 2), attacker holds 1M in pool-A,
victim funds pool-B 1M; a hand-built withdraw passing pool-B + vault-B + the attacker's pool-A position is
rejected, vault-B untouched, attacker gains 0 (then exits pool-A for exactly their own 1M). Removing the
`|| position.pool != *pool_account.key` half from :604 + rebuilding the .so makes the drain SUCCEED (test
fails). Same two-halves-of-one-guard lens as gv init_config (pool + dist bindings) and the book.config doors.
Test KEPT: cannot_drain_a_foreign_pool_with_a_position_from_another_pool (subledger own-vault 6).

### [BLOCKED+PINNED] Cross-config set_coin_sink theft door (book.config pin, higher severity than set_reserve)
Continuing the both-doors lens on the book.config == config pin (multi-tenant isolation: config-A must not
mutate config-B's book). It is a DISTINCT check in each book-taking mutator (set_reserve:1001,
set_coin_sink:1031, set_bid_fee:1075, init_book, place_bid/execute). Only set_reserve's door was pinned
(e2e_config_a_cannot_mutate_config_bs_book_reserve). set_coin_sink's is HIGHER severity: cross-config sink
flip to SEND with an A-OWNED coin account would redirect EVERY COIN config-B's execute buys to config-A's
treasury — cross-tenant buyback THEFT (not just the reserve-grief set_reserve enables). set_coin_sink's
mint check (coin_sink.mint == book.coin_mint) does NOT stop it (A makes a B-coin account); only book.config
== config-A does.
Verified BLOCKED + mutation-SHARP through the REAL Squads binary: extended the test (renamed
e2e_config_a_cannot_mutate_config_bs_book) so config-A's Squads also attempts set_coin_sink(SEND,
attacker_sink) on config-B's book -> rejected, B's book stays BURN (data[BK_SINK_MODE=249]==0). Neutering
ONLY set_coin_sink's book.config check (:1031 -> `if false`) + rebuilding the .so makes the hijack land and
the new sub-assertion fail, while set_reserve's check stays intact. Extended in place; chain 71.
Now ALSO pinned set_bid_fee's door (third sub-assertion, multisig-A idx 3): config-A jacking config-B's
per-bid fee to u64::MAX would brick B's auction (every place_bid needs an unpayable balance) — cross-tenant
grief DOS; rejected, B's fee unchanged. Added a build_set_bid_fee_message helper (mirrors set_reserve, tag
12). Mutation-verified per clause: neutering set_coin_sink's (:1031) OR set_bid_fee's (:1075) book.config
check alone fails the corresponding sub-assertion while the others stay green. The permissionless book.config
doors (place_bid:1127, execute:1315, claim:1590, cancel:1682) are NO-GAIN cross-config (book-scoped ops —
passing a foreign config redirects nothing) and/or doubly-defended (execute also pins holding.owner ==
config's twap_authority), so not separately pinned. init_book's book.config door is create-time Squads
footgun, low value. The three mutate doors that an external tenant could exploit (reserve grief, sink THEFT,
fee DOS) are all pinned.

### [BLOCKED+PINNED] Div-by-zero second door: init_book reserve_den == 0 (create-time, was unpinned)
Applying the "both doors" lens: the cmp_rate div-by-zero (reserve_num/0 panics every execute -> permanent
auction DOS) can be armed at TWO doors — set_reserve (mutate) and init_book (create). set_reserve's
reserve_den==0 was pinned; init_book's combined guard (reserve_den==0 || round_length==0 || sink_mode>
SINK_SEND, lib.rs:881) had its round_length clause pinned but NOT its reserve_den clause — removing ONLY the
init-book reserve_den check (keeping round_length) would let a DAO create a book that panics execute, and
neither the set_reserve test (different fn) nor the round_length test (different clause) would catch it.
Verified BLOCKED + mutation-SHARP: EXTENDED e2e_init_book_rejects_a_zero_round_length -> renamed
e2e_init_book_rejects_degenerate_params, adding a reserve_den=0 init attempt (real Squads execute) that
FAILS with the book never created. Removing ONLY the `reserve_den == 0` clause at :881 + rebuilding the .so
makes the reserve_den sub-assertion fail while round_length still passes — so the reserve_den door is now
independently pinned. Extended in place (no new test fn); chain stays 71.
Methodology note carried forward: a guard that exists at a create door AND a mutate door needs an
INDEPENDENT pin at each; a combined multi-clause guard needs a pin per security-relevant clause (each clause
mutation-checked alone). Applied to coin_sink self-loop (prev tick) and now div-by-zero.

### [BLOCKED+PINNED] Finding AS at the init_book door (self-loop SEND sink set at creation)
Vector: a book can be born in SEND mode with coin_sink already chosen. If init_book did not reject
coin_sink == coin_escrow (the same finding-AS self-loop set_coin_sink rejects), execute's SEND would
transfer the shared coin_escrow to itself (escrow -> escrow, a no-op) and STRAND every bought COIN in the
escrow forever (fixed supply) — nullifying the buyback from the very first round. AS was a real correctness
bug; this is its OTHER entry point.
Analysis: init_book DOES guard it (lib.rs:929, `*coin_sink.key == *coin_escrow.key` -> reject), a DISTINCT
check in a distinct function from set_coin_sink's (`coin_sink.key == book.coin_escrow`). Only the
set_coin_sink door was pinned (e2e_send_sink_cannot_be_the_coin_escrow); the init_book door was unpinned.
Verified BLOCKED + mutation-SHARP through the REAL Squads binary: a DAO init_book with sink_mode=SEND and
coin_sink := coin_escrow vault execute FAILS and the book is never created. Removing the :929 check +
rebuilding the .so makes the self-loop init land (test fails).
Test KEPT: e2e_init_book_send_sink_cannot_be_the_coin_escrow (chain 71). Pins the second of AS's two doors.

### [VERIFIED-COVERED] shutdown holding-substitution: owner check is load-bearing (not a book-pin)
Read process_shutdown (lib.rs:1740) closely. It is Squads-gated (require_squads_vault), derives+checks the
twap_authority PDA, requires holding.owner == twap_authority and dest.mint == holding.mint, then transfers
holding.amount -> dest signed by twap_authority. NOTE: `holding` is NOT pinned to book.holding — the ONLY
restriction is the owner check. That owner check is exactly the load-bearing guard: settlement_usd (winners'
parked USD) and coin_escrow (committed bids) are BOOK_ESCROW-owned, not twap_authority-owned, so neither can
be passed as `holding` to sweep them. The lack of a book-pin is harmless: the only twap_authority-owned
collateral account per config is book.holding (one book per config PDA); passing any other twap-owned account
just sweeps that account (DAO's own, Squads-approved dest in the message), never the escrows.
Already pinned end-to-end: e2e_shutdown_cannot_drain_escrow_or_settlement runs BOTH substitution attacks
(holding := coin_escrow rejected; holding := settlement_usd rejected, balances untouched) and
e2e_shutdown_sweeps_holding_only_via_squads pins the Squads gate. No gap, no new test; the book-pin would be
defense-in-depth only (and would need an extra account param on a Squads-gated op — not worth it).

### [VERIFIED-COVERED] Distribution proposal-id namespace, execute SEND account-count, withdraw atomicity
Three probes this tick; all contained, no new test (reasons below).
- DISTRIBUTION proposal_id SHARED namespace: proposal_seeds = [dist_proposal, config, id] (lib.rs:63) — NOT
  creator-scoped, so an attacker can create_proposal at any id with themselves as creator. CONTAINED + inert:
  (a) create at an occupied id fails AccountAlreadyInitialized (clean); (b) a squatted proposal is registerable
  ONLY by its own creator (register is creator-gated, pinned by register_rejects_a_non_creator_front_runner)
  and to win still needs a real quorum+weighted-majority an attacker can't fake; (c) the id space is u64, so
  the orchestrator just picks an unused id. No on-chain LOF/DOS. Minor task-#6 note: the proposal-generation
  tool should pick an unused proposal_id (or retry on collision) — trivial.
- EXECUTE SEND coin_sink account count: coin_sink is read via next_account_info ONLY inside the `if settled`
  block AND only when sink_mode == SINK_SEND, after the 14 fixed accounts; it is pinned to book.coin_sink (and
  != coin_escrow, finding AS). A rolled round never reads it; a settled SEND round that omits it fails cleanly
  (NotEnoughAccountKeys) and is re-crankable. Redirect already pinned (e2e_execute_send_cranker_cannot_redirect).
- INSURANCE_WITHDRAW two-step (percolator->holding->owner_ata): atomic — a bad/frozen owner_ata reverts the
  WHOLE tx (the position decrement + both transfers roll back), so no partial-state LOF; the owner retries with
  a good ATA. Atomicity is a Solana guarantee, so a test would be tautological.

### [VERIFIED-COVERED] twap Config field audit (metadao_futarchy binding + market_0_domain vestigial)
Probed the two twap Config fields I had not traced, for a hidden authority path or stale-snapshot misuse.
- metadao_futarchy: NOT vestigial and NOT a live authority. At init_config (lib.rs:388-404) it requires the
  bound Squads multisig's config_authority == metadao_futarchy, so the twap can only bind to a multisig the
  DAO actually controls (the tested twap_config_binds_only_to_a_real_squads_multisig... boundary). After init
  it is ONLY a stored snapshot — no mutator/CPI reads it; all post-init control flows through
  config.squads_multisig, re-validated at every Squads-gated entry (set_reserve/floor/coin_sink/shutdown/
  reconfigure/init_book/accept_operator via squads_default_vault(config.squads_multisig)). So the genesis->DAO
  config_authority rotation (squads_handover) making the init-time snapshot stale is harmless — nothing reads
  it. No vector.
- market_0_domain: serialized + deserialized but NEVER read in any logic path (grep-confirmed) — vestigial,
  hence unexploitable. Flagged for a future dead-field cleanup, not a security issue.
Health check this tick: all four programs build-sbf clean; 148 tests green (subledger 42, gv 14, dist 18,
twap 74). Checkpoint header refreshed from the stale 136.

### [BLOCKED+PINNED] init_book round_length == 0 re-opens place-then-yank spoofing (anti-spoof break)
Vector: init_book's combined guard rejects reserve_den==0 || round_length==0 || sink_mode>SINK_SEND
(lib.rs:881) before creating the book. The sharpest clause is round_length == 0: cancel_bid's cooldown is
`aged = now >= place_slot + 2*round_length`, so a zero round makes aged ALWAYS true — a bidder could place a
bid AND cancel it in the SAME slot, reconstructing the place-then-yank last-second spoof the cooldown exists
to stop (the issue-#28 anti-spoof commitment). reserve_den==0 (same guard) would also divide-by-zero-panic
execute (cf. set_reserve tick). Both are armed only at init, which is Squads-gated; the guard blocks them
even with a fully-approved, timelock'd execute.
Background: a systematic panic/arithmetic/type-cosplay sweep this tick found everything else guarded —
mul_div_floor checks denom==0; cmp_rate's continued-fraction denominators are bid-usdc (place_bid rejects
usdc_atoms==0) or reserve_den (guarded), and only reassign to non-zero remainders; vote_weight guards age<2
before ilog2 (ilog2(0) would panic) and is unit-tested; read_asset0_insurance (twap+subledger) uses
.get().ok_or() not raw slicing; gv read_sub_position/read_sub_pool_outstanding check BOTH len AND the disc
(SUB_POSITION_DISC/SUB_POOL_DISC), blocking short-account panics and type cosplay. The lone gap was init_book.
Verified BLOCKED + mutation-SHARP through the REAL Squads binary: a DAO init_book with round_length=0 vault
execute FAILS and the book is never created. Removing the `round_length == 0` clause at :881 + rebuilding the
.so makes the Squads init_book land (test's is_err fires).
Test KEPT: e2e_init_book_rejects_a_zero_round_length (chain 70).

### [BLOCKED+PINNED] reserve_den == 0 would div-by-zero-panic execute (permanent auction DOS)
Vector: the reserve is a fraction reserve_num/reserve_den; execute's eligibility filter calls
cmp_rate(c, u, reserve_num, reserve_den), which uses REAL division (an/ad, bn/bd) — not cross-multiply. A
stored reserve_den == 0 makes every execute panic (reserve_num / 0) on the first eligible bid, so no round
can EVER settle — a permanent buy/burn DOS (and bidders' COIN sits until aged-cancel). Bids cannot introduce
a zero denominator (place_bid rejects usdc_atoms == 0; coin_atoms == 0), and cmp_rate's continued-fraction
loop only reassigns denominators to NON-zero remainders, so the reserve is the SOLE path to a 0 denominator.
Analysis: set_reserve (lib.rs:992) and init_book (the combined reserve_den==0 || round_length==0 ||
sink_mode>SINK_SEND guard) both reject reserve_den == 0 BEFORE writing the book, so even a fully-approved,
timelock'd Squads set_reserve cannot arm the panic. Existing reserve tests all use valid denominators.
Verified BLOCKED + mutation-SHARP end-to-end through the REAL Squads binary: a DAO set_reserve(num 1, den 0)
vault execute FAILS, the book reserve is unchanged, and a subsequent execute still runs (book intact).
Removing the `reserve_den == 0` check at :992 + rebuilding the .so makes the Squads set_reserve land
(test's is_err assertion fires).
Test KEPT: e2e_set_reserve_rejects_a_zero_denominator_that_would_brick_execute (chain 69).

### [DOWNGRADED] Arbitrary-CPI insurance drain is DOUBLY-defended (percolator operator-dest invariant)
Resolves last tick's "load-bearing-but-litesvm-untestable" flag on the subledger percolator_program pin
(:853/:1024/:1241). Read the REAL percolator handle_withdraw_insurance_limited (../percolator-prog/src/
v16_program.rs:8470): it calls verify_withdrawable_token_accounts(dest_token, operator.key, ...), which
requires `dest.owner == operator` (the registered asset-0 insurance operator = the subledger pool PDA) AND
the vault to be the canonical insurance vault. So WithdrawInsuranceLimited can ONLY pay out to an account
owned by the operator.
Consequence: even in the hypothetical where the :853 pin were removed and a hostile program received the
pool-PDA signer via the redirected TopUp/Withdraw CPI, that program could at most move insurance into a
POOL-PDA-OWNED account (the holding) — it can NOT extract to an attacker account (percolator rejects a
non-operator-owned dest with InvalidTokenAccount). Pool-owned funds stay governed by the subledger's
owner-authorized, principal-only exit. So the arbitrary-CPI is doubly-defended (subledger program pin +
percolator operator-dest invariant), not a high-severity hole. The repo-side pin remains correct hygiene
(don't hand a PDA signer to an unpinned program); the percolator invariant is its own kani-tested backstop.
Net: no extractable LOF on this path; no new percolator-meta test warranted (the backstop lives in the
sibling's own suite). Closes the open question from the sysvar/arbitrary-CPI sweep below.

### [VERIFIED-COVERED] Sysvar-spoofing + arbitrary-CPI sweep (two Copenhagen classes)
Two classes swept across all four programs; both clean, no new test (reasons below).
SYSVAR SPOOFING — structurally absent: every time/rent read uses the Clock::get()/Rent::get() SYSCALL
(the `sysvar::Sysvar` import is just the trait), never a passed sysvar account. grep confirms 0
from_account_info / Clock::from / sysvar::clock::ID reads; the cancel cooldown, execute round gate, claim
window, and vote hold-time all read the syscall clock, so there is no account to substitute. Nothing to
test (the attack surface does not exist).
ARBITRARY CPI — pinned everywhere: every CPI program-id is bound before the call — subledger
insurance_deposit (lib.rs:853), insurance_withdraw (:1024), accept_operator (:1241) all require the passed
percolator_program == pool/config.percolator_program; gv vote->subledger / trigger->distribution pin
config.subledger_program / config.distribution_program; twap execute/accept_operator pin
config.percolator_program; all token CPIs pin spl_token::ID. The pool PDA re-derivation uses the STORED
program (not the passed one), so the pin is the load-bearing guard. NOTE: the deposit TopUp CPI carries no
vault_authority, so :853 is the SOLE guard there (withdraw's :1024 is backstopped by the :1029 vault_auth
derivation only if the attacker ALSO forges a matching authority). A mutation-sharp litesvm test is
infeasible: with the pin removed, redirecting the signed CPI to any NON-malicious program (a non-executable
key, or a real one like twap that rejects the percolator-shaped ix) still fails, so the test would pass
regardless of the guard (tautological). Only a purpose-built malicious program that re-routes the pool-PDA
signer to WithdrawInsuranceLimited would demonstrate the drain — out of scope for a tick. Recorded as
load-bearing-but-litesvm-untestable; a fuzz/redteam harness with a hostile program is the right tool.

### [BLOCKED+PINNED] Targeted voter disenfranchisement via ballot-PDA dusting (finding AI, vote path)
Vector: the gv ballot PDA is f(gv_config, voter) — fully deterministic from a PUBLIC voter key — and `vote`
lazily creates it on the first back. If that creation used System create_account (aborts AccountAlreadyInUse
on ANY pre-existing lamports), an attacker could transfer 1 lamport (no signature needed) to a TARGET
voter's ballot PDA and permanently block that specific voter from ever casting a ballot — silencing a large
holder to swing the genesis outcome (a precision DOS, distinct from the genesis-wide config brick).
Analysis: gv's create_pda is robust (top up the rent shortfall, then allocate + assign via invoke_signed,
which only need data-empty + system-owned), so a dusted ballot still gets created and the vote lands. The
existing prefund test (lamport_prefund_cannot_brick_gv_config_init) covers only the gv CONFIG account.
Verified BLOCKED + mutation-SHARP: dust alice's ballot PDA with 1 lamport, then her vote STILL lands (ballot
created + gv-owned, her weight/principal count). Replacing create_pda's robust body with System
create_account + rebuilding the gv .so makes the dusted vote fail (test fails). Same finding-AI robustness as
the four init PDAs, now pinned on the per-voter ballot path too.
Test KEPT: dusting_a_voters_ballot_pda_cannot_block_their_vote (subledger insurance 31, real gv+subledger+
percolator vote path).

### [BLOCKED+PINNED] Cross-instruction PDA squat: init_pool (own-vault) onto the genesis insurance PDA
Vector (PDA seed-collision / account-confusion): init_pool (own-vault, tag 0) and init_insurance_pool
(tag 3) BOTH derive their pool PDA from pool_seeds(mint, asset_id, market_slab, percolator_program). The
genesis insurance pool lives at (mint, 0, REAL_market, REAL_program). If init_pool let the caller supply
the market/program seed parts, an attacker could derive that exact address with a BACKING-domain own-vault
pool, seize the PDA (legit init then fails AccountAlreadyInitialized), and brick the genesis (genesis-vote
requires is_insurance()).
Analysis: init_pool HARDCODES the market/program seed components to Pubkey::default() (lib.rs:394-397,419-
420) — they are NOT caller-supplied — so own-vault pools are confined to the (mint, asset_id, default,
default) namespace, provably disjoint from any real-market insurance pool. (Symmetrically
init_insurance_pool requires percolator_program != default at :741, so an insurance pool can't drop into
the own-vault namespace either.) The existing squat tests use init_insurance_pool with a foreign market /
bad policy; the wrong-INSTRUCTION angle (init_pool) was untested.
Verified BLOCKED: own_vault PDA for (mint,0) != env.pool (structural disjointness asserted), and init_pool
pointed at env.pool is rejected (InvalidSeeds, before it touches the vault); env.pool stays empty and the
genuine insurance init then lands (domain byte 90 == INSURANCE, bound to the real market). NOTE: the guard
is STRUCTURAL (hardcoded default seeds), not a single-line check, so there is no clean one-line src-mutation;
the test is a regression guard — an init_pool refactored to take caller market/program would accept env.pool
and fail the is_err() assertion.
Test KEPT: own_vault_init_pool_cannot_squat_the_genesis_insurance_pda (subledger insurance 30).

### [BLOCKED+PINNED] reconfigure bps > 10000 would over-pull below the floor (principal drain LOF)
Vector: execute pulls burnable = surplus * buy_burn_bps / BPS_DENOMINATOR(10000). If buy_burn_bps could
exceed 10000, burnable would EXCEED surplus and the WithdrawInsuranceLimited reaches BELOW reserved_floor
into protected depositor principal — a LOF that bypasses the finding-O floor. The only mutable path to bps
is reconfigure (Squads-vault-gated, timelock'd); a malicious/mistaken DAO could try bps=10001.
Analysis: process_reconfigure rejects new_bps > BPS_DENOMINATOR (lib.rs:473) BEFORE writing config, so even
a fully-approved, post-timelock reconfigure cannot arm an over-pull; execute's burnable stays <= surplus and
retained = surplus - burnable stays >= 0 (checked_sub). Existing tests covered bps=5000 (happy) and bps=0
(auth) — neither pinned the upper bound.
Verified BLOCKED + mutation-SHARP end-to-end through the REAL Squads binary: propose+approve+warp-past-
timelock a reconfigure(10001) -> the vault execute fails (twap rejects), bps stays 8000. Removing the
`new_bps > BPS_DENOMINATOR` check at :473 + rebuilding the .so makes the 10001 reconfigure land (test fails).
Also confirmed init_config does not take bps as a free input (defaults 8000), so reconfigure is the only
mutation path. Adjacent surfaces reviewed + already covered (no new test): finding-S post-handoff deposit
lock (e2e_post_handoff_deposit_blocked_by_authority_revoke), post-handoff exit closure
(e2e_subledger_exit_blocked_after_operator_handoff), operator-grant Squads gating
(e2e_attacker_cannot_grant_operator_bypassing_squads), create_proposal capacity<=MAX_ENTRIES + exact-sized
allocation.
Test KEPT: reconfigure_rejects_a_bps_above_the_denominator_that_would_overpull_the_floor (chain 68).

### [BLOCKED+PINNED] Front-run the genesis insurance pool with an out-of-range policy (permanent exit DOS)
Vector: init_insurance_pool is permissionless and the genesis pool PDA is deterministic, so an attacker can
race the orchestrator to it. The market/vault bindings are part of the PDA seeds (covered by
init_insurance_pool_cannot_be_squatted_to_misdirect_the_genesis_pool), but `policy` is a free instruction
byte. If init didn't reject policy > POLICY_WITH_SURPLUS, an attacker could initialize the REAL genesis pool
PDA with a garbage policy: payout()'s `_ => Err` (and Pool::deserialize's policy guard) would make EVERY
insurance_deposit/withdraw revert, and the legit init is then refused (AccountAlreadyInitialized) — the
canonical pool is bricked and all depositor exits frozen forever (DOS/LOF).
Analysis: lib.rs:732 rejects policy > POLICY_WITH_SURPLUS up front (and domain is hardcoded DOMAIN_INSURANCE,
not caller-supplied). Distinct from the squat test (that pins the MARKET binding in the PDA seeds; this pins
the POLICY byte on the genesis PDA itself).
Verified BLOCKED + mutation-SHARP: a policy=2 init on the real genesis pool PDA (real mint/vault/slab, only
the policy wrong) is rejected and the PDA stays empty; the legit init then proceeds and a deposit + full
exit round-trips. Removing `policy > POLICY_WITH_SURPLUS` from :732 + rebuilding the .so makes the bad-policy
init land (test fails).
Test KEPT: front_running_the_genesis_pool_with_a_bad_policy_is_rejected (subledger insurance 29).

### [BLOCKED+PINNED] Squads 1-week timelock is ENFORCED, not just required (instant-rug delay)
Vector: the whole DAO->Squads->twap->percolator authority chain leans on the 1-week timelock so depositors/
voters get a week to react+exit before any DAO action lands. twap init_config only REQUIRES the bound
multisig's time_lock >= 1 week (pinned by twap_config_rejects_a_multisig_below_the_one_week_timelock); if
Squads v4 did not actually ENFORCE that delay the requirement would be cosmetic — a rushed/compromised
multisig could instantly flip the reserve to 0 (re-expose the whole surplus to whale draining), shutdown-
sweep the holding, or repoint coin_sink, with no reaction window.
Verified BLOCKED end-to-end against the REAL Squads binary: create+approve a set_reserve(7/3) vault tx, then
attempt execute (a) immediately and (b) 60s short of the week -> BOTH rejected, reserve unchanged; only after
warping past the full TIMELOCK_1_WEEK_SECS does the SAME approved action execute and apply 7/3. The post-
timelock success is the control that proves the premature failures were the timelock (not a malformed
msg/accounts), ruling out a false-positive. Squads is a read-only sibling so no src-mutation; the behavioral
premature-vs-post assertion is self-verifying.
Test KEPT: e2e_a_squads_action_cannot_execute_before_the_one_week_timelock (chain 67). Complements the
requirement test (that one pins the >=1wk floor at config bind; this pins enforcement at execute).

### [BLOCKED+PINNED] Foreign-distribution-config proposal registered -> votable-but-unsealable genesis stall
Vector: register binds a votable proposal to THIS genesis's distribution config (lib.rs:459, the proposal
header.config[8..40] must == config.distribution_config). Without it, a proposal created under a DIFFERENT
(attacker-owned) distribution config could be registered + voted on; if it won, trigger would
CPI SealWinner(our_dist_config, foreign_proposal) which the distribution rejects on header.config mismatch
-> the winner can never seal, and winner-take-all means no other proposal can seal either -> the genesis
stalls forever (DOS). Distinct from the creator-binding (register_rejects_a_non_creator_front_runner pins
the creator half; this pins the config half).
Verified BLOCKED + mutation-SHARP: plant a distribution-program-owned proposal valid in every respect
(disc DISTPRP1, creator==payer, entry_count 1, total 100) EXCEPT header.config = a foreign key; the genuine
creator's register is refused purely on the config binding and no gv proposal-vote account is created.
Removing the `dist_proposal_config != config.distribution_config` check at :459 + rebuilding the .so makes
register accept the foreign-config proposal (test fails).
Note: also reviewed init_book's Squads gate (shared require_squads_vault, already proven by
e2e_attacker_cannot_lower_the_reserve_without_squads) and the top-up start_slot reset (already pinned by
top_up_resets_the_position_start_slot) — both covered, no new test.
Test KEPT: register_rejects_a_proposal_from_a_foreign_distribution_config (seal 11).

### [BLOCKED+PINNED] Distribution bait-and-switch in the register->trigger window (LOF on voters)
Vector: voters back a gv proposal whose distribution they have read. The distribution-side append-freeze
(`header.sealed`, tested last tick) only engages at SEAL — but the seal happens INSIDE gv `trigger`, so
between `register` and `trigger` the distribution proposal is NOT sealed and its creator can still append.
A creator registers an honest "60 to alice, 40 burned", collects quorum+majority, then appends a self-
dealing "40 to mallory" into the burn-bound headroom (60+40 == total_supply, so distribution's own supply
cap never fires) and triggers — privatizing the 40 voters expected destroyed. The ONLY guard over this
exact window is gv trigger's snapshot check (lib.rs ~724): the live (entry_count, total_amount) must equal
the (snapshot_entry_count, snapshot_total_amount) frozen at register, else the seal is refused.
Analysis: append is append-only (no edit), entry_count + total_amount both monotonically grow, so ANY
post-register tamper changes the pair and trips the check; a content swap keeping the pair fixed is
impossible (no edit-entry ix). The tamper just bricks the creator's own proposal (self-DOS) — it can never
seal an inflated list. Voters get exactly what they approved, or nothing.
Verified BLOCKED + mutation-SHARP: register (alice,60) -> tally quorum+majority -> append (mallory,40)
[accepted at the distribution layer, pre-seal] -> trigger REFUSED, nothing sealed. Removing the snapshot
block at :724 + rebuilding the .so makes trigger seal the inflated distribution and the test fails.
Distinct from register_rejects_a_non_creator_front_runner (that pins the creator-BINDING on register; this
pins the snapshot-MISMATCH refusal at trigger over the creator's OWN post-registration tamper).
Test KEPT: trigger_refuses_a_distribution_inflated_after_registration (seal 10).

### [BLOCKED+PINNED] Split-withdraw rounding game on the impaired insurance haircut (LOF)
Vector: insurance_withdraw allows PARTIAL exits and the haircut is mul_div_floor(insurance, amount,
outstanding). A sophisticated exiter splits their exit into many small partial withdraws hoping the
per-chunk rounding accumulates in their favour — over-extracting beyond their pro-rata share and thereby
draining co-depositors (the impaired insurance is a shared, finite pot). Realistic because finding-L's
only test did single lump-sum exits.
Analysis: each chunk FLOORS, so a split can only round DOWN — the running total can never exceed the
single-shot share, and the withheld rounding dust stays in the insurance fund for whoever remains (never
extracted). Proof sketch: after a chunk, insurance-outstanding rises by at most 1 atom, so the deficit
never inverts to over-pay. Conservation: sum of all exits == impaired insurance exactly.
Verified BLOCKED + mutation-SHARP: with an ODD impaired insurance (1,000,001) and alice splitting
1,000,000 into 400k/300k/300k, she collects exactly 500,000 (her floor share), the leftover atom accrues
to bob (the depositor who stayed -> 500,001), and the vault ends at 0. Flipping mul_div_floor to round UP
+ rebuilding the .so makes the splitter pull 500,001 (> her share) and the test fails — so a rounding-
direction regression that would let a splitter drain a co-depositor is caught.
Test KEPT: splitting_an_impaired_exit_cannot_beat_the_pro_rata_or_drain_a_codepositor (subledger insurance
28). The lump-sum order-independence case (impaired_insurance_exit_is_pro_rata) is retained — it pins a
different property (two FULL exits, equal haircut); the new one pins the partial/split conservation.

### [GH-TRIAGE] No open PRs/issues; no external PR ever merged (DPRK lens)
Checked at user request (assume DPRK submitter): `gh pr list` / `gh issue list` both empty. Across ALL
history every contributor PR (#27,#25,#22,#21,#18,#15,#11,#10,#7,...) is CLOSED with mergedAt=null; the only
MERGED PR is #8 by the owner. So NO untrusted remote code ever entered the tree — clean-room discipline held
(hostile ideas verified locally + reimplemented from scratch, never pulled). Latest items already triaged:
#28 cancel-cooldown bypass (real, fixed by this loop), #27 bind-twap-auth (regression trap, rejected),
#24/#26 (superseded by F-VAULT + AQ), #20 minority-capture (real, accepted by "those who stay decide").

### [BLOCKED+PINNED] Append to a SEALED winner to grab the unallocated headroom (LOF)
Vector: a distribution proposal that allocates only PART of total_supply (e.g. 60 of 100) leaves 40 of
unallocated headroom destined to be burned as unclaimed -> deflation to ALL coin holders. The (real)
creator waits until the vote SEALS their proposal, then appends a self-dealing 40-entry into that headroom
(60+40 == total_supply, so the supply cap at append never fires) and claims it — privatizing protocol-wide
deflation AFTER voters can no longer react. The only thing standing in the way is the append-time freeze
`header.sealed || config.is_sealed()` (distribution/src/lib.rs:420). Existing append tests covered only the
supply cap + foreign-creator; the seal-freeze was unpinned.
Verified BLOCKED + mutation-SHARP: removing the guard at :420 + rebuilding the .so makes the post-seal append
land (test fails); restored -> green. The genuine creator (passes header.creator) would otherwise succeed —
so creator-binding does NOT cover this; the seal flag is the load-bearing guard.
Test KEPT: append_to_a_sealed_winner_cannot_grab_the_unallocated_headroom (append fails, no index-1 entry,
alice still gets exactly 60, the 40 remains and is later BURNED not captured, grabber gets 0). 14 dist tests.

### [VERIFIED-COVERED] cancel_bid redirect/owner + the whole distribution claim/seal/burn surface
Re-probed two surfaces for redirect/replay/substitution gaps; both already saturated, so NO new test
(adding one would be tautological).
- twap cancel_bid (lib.rs:1655): bidder.is_signer (1672) + SL_BIDDER==bidder.key (1699) so only the owner
  cancels; refund coin_ata pinned to the stored SL_COIN_ATA (1719); aged-only cooldown (1714, the #28
  fix); settled slots rejected -> claim path (1696). The coin_ata pin here is self-only (cancel needs the
  bidder's OWN signature, so they can only steer their own COIN) — not a cross-user surface. Cooldown +
  non-owner rejection already pinned by e2e_bid_cancellable_after_cooldown_keeps_fee (mallory case).
- distribution claim (lib.rs): recipient.is_signer + entry pk==recipient.key (named-pull) + sealed-winner
  + vault pin + window + zero-on-claim. recipient_ata is caller-chosen but SPL enforces mint-match and the
  recipient steers only their OWN entitlement. Double-claim + wrong-recipient pinned at distribution.rs:294/299.
- distribution seal_winner: authority key+sig (17/18) + re-seal refused via is_sealed (lib.rs:20) — pinned at
  distribution.rs:367 (cannot reseal to the loser). burn_unclaimed: vault+coin_mint pinned, sealed+window
  gated, BURNS (not transfers) the remainder — pinned both window directions.
Verdict: no LOF/DOS gap on either surface. distribution suite green (4+13).

### [BLOCKED+PINNED] Re-executing a SETTLED book (double-burn / double-spend)
Vector: after `execute` clears the auction (state -> SETTLED) but BEFORE any winner claims, a cranker
warps past the freshly-set `round_end` (so the ERR_ROUND_ACTIVE timer no longer blocks) and re-cranks
`execute`. If the OPEN-state precondition were absent, the second pass would re-walk the still-occupied
slots and re-run settlement: a SECOND burn of `total_coin` (destroying COIN owed back to bidders as
refunds) and a SECOND holding->settlement_usd transfer. A pure LOF.
Analysis: process_execute gates on `book.state != BOOK_STATE_OPEN` (lib.rs:1315) BEFORE any movement;
settle sets SETTLED, and only draining every slot via `claim` flips it back to OPEN. So the *state*, not
the round timer, freezes a settled-but-unclaimed book. (place_bid is likewise OPEN-gated at :1127.)
Verified BLOCKED + mutation-SHARP: dropping `|| book.state != BOOK_STATE_OPEN` from :1315, rebuilding the
.so, makes the second execute SUCCEED and the test's is_err() assertion fire (double-settle reproduced);
restored -> green. The round-active gate alone is insufficient here (it had elapsed) — the OPEN guard is
the load-bearing one. Previously only the *across-rounds* path (warp between executes) was tested; the
"settled book frozen until drained" boundary was unpinned.
Test KEPT: e2e_execute_on_a_settled_book_is_frozen_until_claims_drain_it (asserts the re-crank fails,
supply + settlement_usd unchanged, then a claim drains -> OPEN -> execute accepted again). 66 chain tests.

### [BLOCKED] Mid-auction config change cannot harm a committed bidder
Vector: a bidder places a committed bid; the DAO then changes the auction parameters (set_reserve /
reconfigure-bps / shutdown). Could a committed bidder lose their escrowed COIN or be forced into a worse
fill they can't escape? Analysis of every mutator's writes (twap-program/src/lib.rs):
- set_reserve writes ONLY BK_RESERVE_NUM/DEN; reconfigure writes ONLY config.surplus_buy_burn_bps; shutdown
  moves ONLY holding->dest. NONE touch coin_escrow (the committed COIN), the bid slots, or BK_STATE.
- Worst case for a committed bid: a raised reserve drops it below the bar -> at the next execute it settles
  as a loser with a FULL coin refund (claimable) or the round rolls and the bid stays committed -> reclaimable
  via the 2*round_length aged cancel. A swept holding (shutdown) just zeroes the budget -> next execute rolls
  -> aged-cancel. bps only changes the surplus split, never the bids.
So the escrowed COIN is never moved by a config change, and the bidder always has a refund/cancel path; the
bidder is never forced to sell at a worse price without an exit. Every such change is Squads-vault-gated and
timelock'd (the bidder observes it before it lands). BLOCKED.
Verdict: no LOF/DOS for committed bidders from mid-auction governance. Covered by the existing shutdown-escrow
test (e2e_shutdown_cannot_drain_escrow_or_settlement) + the aged-cancel test; no new test.

### [ATTESTATION] Coverage map — Copenhagen classes x repo boundaries (136 tests, build-sbf clean)
Consolidated reference after the multi-tick sweep. Every Copenhagen class and repo-specific boundary maps to
an enforcing test/finding (LOF/DOS verdict in parens):
COPENHAGEN CLASSES:
- account confusion / substitution: parasite-config drain (AQ), foreign market_slab/vault/vault_authority at
  execute + withdraw (AF, e2e_execute_rejects_*), canonical-vault F-VAULT (init_..._rejects_non_canonical),
  eviction-refund redirect, claim usd_dest/coin_ata pins (AW), deposit holding (HARDENING).
- missing owner/signer: reconfigure/set_vote_lock/seal_winner is_signer pins (mutation-verified), withdraw
  owner-binding (non_owner_cannot_withdraw), accept_operator bypass (e2e_attacker_cannot_grant_operator_*).
- PDA/seed collisions: config bindings P/Q/R/AA/AQ (gv config, dist config, twap config, subledger pool).
- type cosplay: owner+disc+offset checks; ALL four cross-program discs verified equal + offsets match (gv->
  subledger/distribution); two percolator readers offset_of!-canaried.
- arbitrary CPI: percolator_program/subledger_program/distribution_program pinned to config before every CPI.
- reinit: data_len gate + robust create, tested (insurance_pool / gv_config reinit, finding AJ).
- rounding: pro-rata haircut floor (finding L, order-independent), uniform-price floor + coin_i==0 unfilled
  (AE roll restore), ratchet burnable+retained==surplus (no residual). All conservative/bounded.
- missing rent/exempt (prefund DOS): robust create for ALL 4 init PDAs (subledger pool, twap book, gv config,
  distribution config) — finding AI, mutation-verified.
- sysvar spoofing: N/A — programs use Clock::get() syscall, never a passed sysvar account.
- duplicate accounts: pinned + mint-typed accounts cannot alias harmfully (different mints/owners/pinned keys).
- remaining-account smuggling: SEND coin_sink read only in SEND mode + pinned; BURN/claim/trigger fixed lists.
REPO BOUNDARIES: subledger insurance-authority-vs-operator (accept_operator hardcoded-to-pool + Squads), gv
weight/quorum (weight-0 flash-deposit, vote-lock self-unlock, live-outstanding by design), distribution
claim/seal (cross-proposal isolation, missing-signer seal, freeze/mint/supply, bait-and-switch snapshot), twap
auction (anti-spoof cancel #28 FIXED, roll restore, double-claim, book-squat init_book, reserve/sink/floor
Squads-gated), Squads 1-week timelock (enforced on-chain at init FIXED + before-expiry tests), finding-O floor
(execute pulls only surplus), finding-T offset (canaried both readers).
OFF-HARNESS (task #6 orchestration, on-chain-uncloseable): deposit-deadline/kickstart (bounds the exit-capture
#20-1 + the deposit-inflate-quorum griefing), durable timelock, handover-bound-to-vote-winner (#20-2).
Verdict: on-chain surface comprehensively covered; 2 real fixes (timelock-min, #28), 1 hardening (deposit
holding) this run; residual risk consolidated in the 3 off-harness orchestration requirements.

### [VERIFIED-COVERED] gv->distribution offsets verified — completes the finding-T-family cross-program audit
Closed the last raw-byte-offset cross-program read I had not explicitly checked: genesis-vote reads the
distribution proposal by hardcoded offsets in register_proposal (creator-binding + snapshot) and trigger
(bait-and-switch snapshot). Cross-checked ALL of them against the REAL distribution ProposalHeader
(distribution/src/lib.rs): disc `DISTPRP1` @ [0..8], config [8..40], creator [48..80], entry_count [84..88],
total_amount [88..96] — gv reads (lib.rs:456/459/470/474-475/727-728) MATCH exactly. e2e-validated: the
full-genesis chain/subledger tests create REAL distribution proposals via the real distribution program, so a
distribution layout drift would make register/trigger mis-read and fail (drift-catching — unlike the
hand-edited percolator slab, no separate canary is required).
This completes the finding-T-family audit of every cross-program raw-offset read:
  twap->percolator insurance@749     — own offset_of! canary (e2e hand-edits, canary required)
  subledger->percolator insurance@749 — own offset_of! canary (e2e hand-edits, canary required)
  gv->subledger Position/Pool         — e2e-validated with real positions (drift-catching)
  gv->distribution ProposalHeader     — e2e-validated with real proposals (drift-catching)
DISC VALUES also verified directly (not just inferred from the e2e): the four discriminators gv hardcodes
match the real source constants EXACTLY — gv SUB_POSITION_DISC `SUBPOS01` == subledger POSITION_DISC; gv
SUB_POOL_DISC `SUBPOOL1` == subledger POOL_DISC; gv DIST_PROPOSAL_DISC `DISTPRP1` == distribution PROPOSAL_DISC;
gv DIST_CONFIG_DISC `DISTCFG1` == distribution CONFIG_DISC. So the gv's type-cosplay defenses (owner + disc +
offset) are sound: a substituted account of the wrong type is rejected on the disc, and a real account is read
at the correct fields. No unit canary is added for the intra-repo couplings (gv->subledger, gv->distribution)
because the full-genesis e2e exercises them with REAL accounts and so catches any drift; the cross-repo
percolator reads keep their offset_of! canaries (the e2e there hand-edits and cannot).
Verdict: every foreign-struct raw-offset read is either canaried against the real struct or e2e-validated
with real data, and every cross-program discriminator matches; no silent-drift or type-cosplay gap. No code
change, no new test.

### [VERIFIED-COVERED] Deposit/withdraw validation-parity sweep complete — holding was the only asymmetry
Swept the paired subledger operations for validation asymmetries (the class that surfaced the deposit-holding
hardening below — the kind the external auditor flags):
- holding: was the one genuine gap (withdraw validated it, deposit didn't) — now symmetric (HARDENING entry
  below; doubly-defended either way).
- vault_authority: BOTH the withdraw and the twap execute pin `vault_authority == perc_vault_authority(slab,
  perc)` explicitly (subledger lib.rs:1019). The market-binding is covered by `foreign_market_slab_cannot_
  inflate_the_haircut` (AF) — it rejects a substituted slab at the EARLIER `market_slab != pool.market_slab`
  pin (lib.rs:851); the direct foreign-vault_authority-with-real-slab case is the same market-binding class,
  rejected at :1019 and doubly-defended by the percolator CPI (the vault is pinned == pool.vault and the
  pool, not vault_authority, signs). The deposit needs no vault_authority at all (TopUp is pool-signed).
- position: deposit re-derives the canonical PDA; withdraw + set_vote_lock use stored-field binding
  (position.owner==owner && position.pool==pool) — sufficient because positions only ever exist at the
  canonical PDA (one per owner per pool), so the stored fields uniquely identify it.
- owner_ata / usd_dest: not validated for ownership by design — the OWNER signs and directs their OWN funds
  (a wrong destination is self-inflicted, never cross-user theft, since a non-owner can't initiate).
Verdict: validation parity is consistent; the holding was the only real asymmetry and is closed. No code
change, no new test (the remaining differences are justified or covered).

### [HARDENING] subledger insurance_deposit — fail-fast holding validation (consistency with withdraw)
Vector probed: insurance_deposit routes funds user -> holding -> percolator insurance vault (TopUpInsurance,
pool-signed). The WITHDRAW validates `holding.owner == pool && holding.mint == pool.mint` up front
(lib.rs:1024), but the DEPOSIT did not — it relied on the pool-signed TopUp CPI to revert on a non-pool/
wrong-mint holding. Asymmetry of exactly the kind the external auditor flags.
Analysis + MUTATION result: NOT a security gap — the deposit was already safe. Mutation-verified: with the
new check REMOVED, `insurance_deposit_rejects_a_non_pool_holding` STILL passes, i.e. the TopUp CPI backstops a
non-pool holding (the pool cannot authorize a transfer from an account it does not own). So a wrong holding
was always rejected and the whole tx reverts (the user->holding leg is rolled back; no LOF). Added the
explicit up-front check anyway as DEFENSE-IN-DEPTH + consistency (a clear fail-fast InvalidAccountData instead
of a downstream CPI revert, and so a wrong holding can never even reach the user->holding transfer).
Added `insurance_deposit_rejects_a_non_pool_holding` pinning the boundary (attacker-owned holding -> rejected,
no credit, capital untouched). KEPT — it is the first test of the deposit-holding boundary (doubly-defended,
documented as such). No LOF was present; this is hardening, not a bug fix.

### [VERIFIED-COVERED] Auction state-machine sweep complete — cancel/execute/claim transitions all correctly guarded
Closing the auction state-machine class the external auditor mines (#28 was a cancel-transition bug). Verified
every OPEN<->SETTLED transition is correctly guarded:
- claim -> reopen: claim zeroes the ENTIRE slot (all SLOT_SIZE bytes, clearing OCCUPIED/SETTLED/amounts/place
  marks) then reopens (BK_STATE = OPEN) ONLY if a full scan finds NO occupied slot (`!any`). So the book stays
  SETTLED through partial claims; a new bid can never mix with settled-but-unclaimed state, and a re-bid into
  a freed slot starts byte-clean. (lib.rs process_claim tail.)
- execute: gated on state==OPEN + clock>=round_end; advances round_end once per round; settle marks all
  occupied slots SETTLED (claim-only), roll restores them unsettled byte-identical (finding AE). round_end
  advances ONLY here.
- cancel: gates on `aged` alone now (issue #28 fixed); early exits = settle->claim, strictly-better eviction,
  2*round_length aging. No round_end-delta shortcut remains.
- place_bid: rejected unless state==OPEN, so no bid enters a settling/claiming book.
Verdict: the full auction lifecycle (place -> execute(settle|roll) -> claim -> reopen) has no premature
transition or stale-state-mixing edge; the #28-class is closed. No new test (each transition is already pinned
by the buy-burn/roll/claim/cancel tests), no code change.

### [ORCHESTRATION] Handover not bound on-chain to the vote winner (#20 finding 2) — off-harness requirement
The #20 report's second finding (Low): `handover_genesis_squads` (DEPRECATED monolith program/src/lib.rs)
rotated the Squads config_authority to a caller-supplied key with no check it matches the vote winner. That
exact code is gone, but the concept maps to the current design: the twap config STORES `metadao_futarchy`
and verifies `config_authority(squads_multisig) == metadao_futarchy` (init_config), but has NO on-chain link
binding `metadao_futarchy` to the genesis-vote WINNER — confirmed: twap-program/src/lib.rs references neither
the genesis-vote config, the distribution, nor the sealed winning proposal. So the on-chain check guarantees
internal consistency (the named DAO really controls the multisig), but the orchestration is trusted to set
`metadao_futarchy` (and rotate the config_authority) to the COIN/futarchy the vote actually sealed.
Severity: defence-in-depth (no depositor principal at risk; the COIN supply is provably the genesis COIN via
the distribution invariants + the place_bid coin pin). Not closeable cleanly on-chain: the twap is a separate
program from genesis-vote/distribution and the "winner" is a COIN distribution to many recipients, not a
single key the twap can compare to.
NARROWING (the residual gap is small): the twap's MULTISIG identity is already on-chain-bound — the config
PDA seed folds in `squads_multisig` (finding AQ, lib.rs:416), and accept_operator only completes the operator
grant when `squads_default_vault(config.squads_multisig)` SIGNS (lib.rs accept_operator), so the bound multisig
MUST be the genesis multisig that performed the handoff. And init_config pins `config_authority(multisig) ==
metadao_futarchy`. So the ONLY orchestration-trusted step is rotating THAT genesis multisig's config_authority
to the winning DAO before/at the twap binding — the multisig identity and the authority==DAO consistency are
enforced on-chain. This is the THIRD OFF-HARNESS ORCHESTRATION REQUIREMENT for the unbuilt tool (task #6),
alongside the deposit-deadline/kickstart and the durable 1-week timelock: the orchestration MUST rotate the
genesis Squads config_authority to the futarchy of the COIN the genesis vote sealed.
Verdict: not a current-design on-chain bug (the deprecated handover code is gone; the on-chain consistency
check holds); recorded as an orchestration-tool requirement. No code change.

### [SCOPE] Deprecated workspace members are out of the six-binary probe scope; current handover is covered
Confirmed the probe scope. The workspace (Cargo.toml members) still contains the OLD monolith:
`program/` (genesis monolith + squads_handover.rs), `governance/` (governance adapter), `twap/` (the old
pay-as-bid library with the withdraw_bid spoof hole), `setup/`. These are SUPERSEDED — the live design is the
six binaries the loop targets: subledger, genesis-vote, distribution, twap-program (this repo) + percolator +
Squads v4 (siblings). Issues #1–#25 / PRs #6–#27 mostly audited the deprecated monolith; they do not apply to
the current programs. The deprecated members are NOT probed (not deployed in the current design).
The CURRENT Squads handover (DAO -> Squads multisig (1-week timelock) -> twap_authority operator ->
percolator insurance) is fully covered in the live suite: `handoff_rotates_operator_to_twap_only_after_
timelock`, `handoff_rotates_insurance_policy_only_after_timelock`, `e2e_attacker_cannot_grant_operator_
bypassing_squads`, `e2e_post_handoff_deposit_blocked_by_authority_revoke`, `e2e_subledger_exit_blocked_after_
operator_handoff`, and `twap_config_binds_only_to_a_real_squads_multisig_controlled_by_the_dao` (config_
authority == the DAO). The genesis->DAO config_authority rotation itself is an off-chain orchestration step;
the twap VERIFIES its result on-chain (config_authority == metadao_futarchy) at init_config.
Verdict: full current four-program suite green at 135; the deprecated monolith (program/governance/twap/setup)
is out of scope; the current handover chain is covered. No code change.

### [REVIEW] Full GitHub issue/PR audit (DPRK lens) — #20 verified real but ACCEPTED by design (voluntary exit)
Reviewed ALL issues + PRs (open + closed) adversarially. State: nothing open. Triage:
- #1–#19, #22–#25 + PRs #6–#18, #21–#27: audited the DEPRECATED `percolator-genesis` MONOLITH (mint_reward/
  reward_supply, percolator_admin/RESOLVE_MARKET, activate_live, handover_genesis_squads, transfer_mint_
  authority, init_market_rewards, pull_insurance, governance adapter) — that subsystem was DELETED; obsolete
  in the current four-program design. PR #27 was a regression trap (caught + closed). #26 (one TWAP signer
  across configs) superseded by the AQ config-binding; #24 (non-canonical vault) closed by F-VAULT pins.
- #28 (cancel-cooldown bypass): a real current-design bug — FIXED this run (see FIXED entry).
- #20 (minority captures when committed depositors exit during voting): VERIFIED still real in the CURRENT
  genesis-vote via TDD (`those_who_stay_decide_after_a_nonvoting_majority_forfeits_by_exiting`, subledger
  tests): the trigger reads LIVE pool outstanding, so when a non-voting majority exits, a 2% voter becomes the
  majority of the remainder and seals the whole distribution. BUT this is the INTENDED "those who stay decide"
  design and is ACCEPTED (owner decision, kept): the exit is OWNER-SIGNED (insurance_withdraw requires
  owner.is_signer + position.owner==owner; `principal_only_owner_exit_returns_funds_and_guards` pins that a
  non-owner cannot withdraw), so NO ONE can force the majority out — they leave VOLUNTARILY and with their
  FULL principal (no theft; only COIN governance follows participation). #20's proposed fix (anchor quorum to
  the committed pool) was declined: it trades this capture-resistance for low-turnout STALLS (a passive
  majority could freeze the genesis forever). The complementary deposit-DURING-voting griefing (inflate
  quorum to DOS the trigger) and the deposit-deadline/kickstart that would bound BOTH directions remain the
  documented OFF-HARNESS orchestration requirement (DESIGN-DOS entry below).
Verdict: no open issue is an unaddressed real bug. #28 fixed; #20 accepted-by-design (voluntary exit, no
theft) with a test documenting the intended behavior; the rest are deprecated-monolith or already covered.

### [VERIFIED-COVERED] #28-class sweep across state machines — no stale-cache read, no permissionless-advance bug
Generalized the #28 pattern ("a permissionless action advances some state, changing a TIME/STATE guard's
behavior") and swept every state machine + cached value for the same shape:
- genesis-vote quorum cache: `vote` writes `config.outstanding_principal` on every vote, but NO guard reads
  it — `trigger` deliberately RE-READS the live pool outstanding at seal time (lib.rs:737), never the stored
  field. So the cache is vestigial (off-chain visibility only); there is no stale-cache read bug. Using it
  WOULD reopen the stale-low minority-capture hole that `trigger_uses_live_pool_outstanding_not_stale_cache`
  exists to prevent. Clarified the misleading "Sync the quorum denominator" comment so an auditor/the active
  external filer does not mistake it for a stale read.
- twap reserved_floor / round_end / book.state, distribution sealed_proposal / seal_slot+claim_window,
  subledger pool.outstanding: every guard reads the CURRENT value at decision time (no cached snapshot drives
  a decision). round_end advances only via execute (already fixed for cancel, #28). seal_slot is set once by
  the trigger and the window is immutable; a cranker can only DELAY the seal (not shorten the claim window),
  and the trigger rejects until quorum holds.
- Other permissionless cranks (claim->reopen, burn_unclaimed, trigger->executed) advance state but no other
  guard keys off a delta exploitably (claim reopens only when fully drained; burn is terminal; trigger is
  one-shot).
Verdict: #28 was the only instance of the pattern; it is fixed, and no sibling exists in the other state
machines (all guards read live, no cache drives a decision). Comment-only change; suites green at 134.

### [VERIFIED-COVERED] Auction anti-spoof — proactive #28-sibling sweep (no other cooldown bypass)
After fixing #28 (no-op roll unlocking cancel), swept the auction state machine for SIBLING bypasses of the
"committed until settled/aged/evicted" anti-spoof — the class the external filer is mining:
- round_end advances ONLY via execute: the sole writes to BK_ROUND_END are init_book (initial) and the
  next-round advance in execute (lib.rs:1533). No instruction moves it otherwise — so post-fix nothing reads
  a round_end delta and nothing but the aging window opens cancel.
- execute is one-per-round + state-gated: requires `state == OPEN` (lib.rs:33) AND `clock_slot >= round_end`
  (:85); after it advances round_end, a second execute in the same slot fails the timing check, and once it
  marks SETTLED a re-execute fails the state check. So a cranker cannot spin executes to age out a bid.
- No eviction-based exit: a committed bid can only leave early by being evicted by a STRICTLY-better bid, and
  one-bid-per-bidder (lib.rs:1164) blocks an attacker from placing a second bid to self-evict their first —
  pinned by chain.rs:3159 ("a bidder cannot stack a second bid"). Cross-account self-eviction just commits an
  even-better bid (net commitment unchanged), so it is not a clean yank either.
Verdict: the only early exits remain settle->claim, eviction by a strictly-better bid, and 2*round_length
aging; no sibling to #28. No new test (the self-eviction block is already pinned), no code change.

### [VERIFIED-COVERED] issue-#28 fix completeness — no retroactive aging shrink, no sibling, shutdown-safe
Follow-up verification that the cancel-cooldown fix (FIXED entry below) is complete and side-effect-free:
- `round_length` is IMMUTABLE: written only by init_book (lib.rs:968), no set_round_length instruction, and
  init_book is reinit-guarded. So `aged = place_slot + 2*round_length` cannot be retroactively shrunk to
  re-open an early cancel (a DAO can't lower round_length to age out committed bids faster).
- No sibling round_end-delta vector: SL_PLACE_ROUND_END now has NO functional reader (the removed cancel use
  was the only one); it is written at place_bid + checked by the layout test only. Updated its stale comment.
  Nothing else in the program gates on a `round_end != place_round_end` style delta.
- Shutdown does not strand committed bids: process_shutdown only moves holding->dest; it never touches
  coin_escrow or BK_STATE, so a shutdown leaves committed bids reclaimable via the aged cancel (or eviction).
  `e2e_shutdown_cannot_drain_escrow_or_settlement` already pins that shutdown can't reach the escrow.
Verdict: fix is complete; the only early exits for a committed bid are now SETTLE (->claim), EVICTION by a
strictly-better bid, and the 2*round_length aging window. No code change beyond the stale-comment fix.

### [FIXED] twap cancel_bid — no-op roll unlocked the anti-spoof cooldown early (external issue #28)
Vector (external report, issue #28 by SrMessiSOL — same filer as the PR #27 regression trap, so reviewed
adversarially): process_cancel_bid gated cancel on `cleared || aged`, where `cleared = book.round_end !=
place_round_end` and `aged = now >= place_slot + 2*round_length`. The intent of `cleared` was "a settlement
cleared the book," but process_execute advances round_end on EVERY run — including a no-op ROLL (total_coin
== 0, routine at surplus 0) that leaves the bid OCCUPIED + unsettled. So a bidder could post a bid to shape
the book, crank a permissionless no-op roll (advancing round_end), and yank the bid well INSIDE the intended
2*round_length window — re-opening the last-second-cancel manipulation the cooldown was built (vs the old
library's withdraw_bid) to prevent. Net: a shape-the-book-then-yank seller-griefing (other sellers, reacting
to the fake supply, bid aggressively and clear at a worse price next round); the report confirms NO
third-party theft (only the attacker's own escrow moves), so it is a low-severity anti-spoof weakening, not
a LOF — but it is a genuine deviation from the design's explicit "committed until settled" guarantee.
Adversarial review of the report + its fix: the report is technically SOUND (verified: after a roll,
round_end moved so `cleared` is true while `aged` is false, and `!cleared && !aged` does not fire). Unlike
PR #27, the recommended fix is SAFE and not a trap: removing the `cleared` shortcut makes cancel STRICTER
(gate on `aged` alone) — it cannot open any hole (removing a cancel-escape only makes bids MORE committed),
cannot strand funds (`aged` always eventually fires), and `cleared` only ever mattered for rolled bids
anyway (settled bids are already rejected by SL_SETTLED). Cost: a rolled bid waits ~1 extra round to cancel.
FIX APPLIED (twap-program/src/lib.rs process_cancel_bid): dropped the round_end-delta `cleared` shortcut;
cancel now gates on `aged` (2*round_length) alone (eviction by a strictly-better bid remains the only other
early exit). Re-pointed the test: `e2e_roll_opens_the_cleared_cancel_path` -> `e2e_roll_does_not_unlock_
cancel_before_aging` — same setup, but now asserts the post-roll cancel is REJECTED and only succeeds after
the full aging window (escrow returned). Full chain suite 65/65 green; the aged-path + settled-bid + anti-
spoof cancel tests all still pass. INVARIANT: a bid is committed until SETTLED or 2*round_length aged (or
evicted by a strictly-better bid); a round_end change from a no-op roll must NOT count as a release.

### [VERIFIED-COVERED] Init-validation negative-half sweep — remaining untested clauses are marginal
Build health: re-ran `cargo build-sbf` for all four programs — clean. Then swept the init/validation guards
for more "negative half untested because the test helper can't make the bad input" gaps (the pattern that
caught the freeze-authority clause). The remaining untested negatives are all marginal and deliberately not
pinned:
- distribution init_config `total_supply == 0` / `claim_window_slots == 0` (one line): obvious input-sanity.
  A 0 window would make the whole distribution burn-only (claim window closes at seal); but it is a setup
  value the orchestration controls, fail-fast-rejected, and a single missed config line, not an exploit.
- subledger accept_operator on a NON-insurance (own-vault) pool: rejected by `is_insurance()` AND would fail
  the percolator CPI anyway (default market_slab) — doubly-defended, non-sharp.
- distribution claim `recipient.is_signer`: enforces the pull model, but removing it causes NO LOF (funds
  still go to the recipient's recorded ATA, never an attacker) — low value.
- seal_winner / register `entry_count == 0`: register already blocks an empty proposal up front, so the
  seal-side check is an unreachable backstop (doubly-defended).
Verdict: the init-validation surface's high-value negatives are now pinned (mintable + freezable + supply,
finding-AA authority bind, finding-AI prefund, finding-AJ reinit); the residue is sanity/double-defense. No
new test (would be marginal/tautological), no test deleted, build-sbf clean, full suite green at 134.

### [VERIFIED-COVERED] COIN-authority safety chain — enforced once (distribution), inherited everywhere
Follow-up to the freeze-authority pin below: audited whether ANY other program needs its own non-mintable/
non-freezable check on the genesis COIN. It does not — the safety is enforced at the single custody point and
inherited structurally:
1. distribution init_config is the enforcement point: rejects a live mint authority (pre-mint dilution) AND a
   live freeze authority (vault-freeze brick) AND `mint.supply != total_supply` (COIN held outside the pool).
   All three clauses are now tested (mint -> init_config_rejects_a_mintable_coin incl. the supply>total case;
   freeze -> init_config_rejects_a_freezable_coin, this run). So the genesis COIN is provably fixed-supply +
   unfreezable + fully in the vault.
2. The TWAP auction does NOT re-check the COIN's authorities — and does not need to: place_bid pins
   `bidder_src.mint == coin_mint == config.coin_mint` (lib.rs:42-61), so bidders MUST hold exactly the bound
   COIN; the auction's COIN therefore IS the genesis COIN and inherits (1). A parasite twap config bound to a
   different (mintable/freezable) COIN cannot drain real insurance — its config-derived twap_authority is not
   the percolator operator (finding AQ) — so it is inert.
3. The SUBLEDGER intentionally has no mint/freeze-authority check: its deposit asset is the collateral
   stablecoin (e.g. USDC), which is freezable BY DESIGN (the issuer can freeze) — a standard, accepted
   property, NOT the governance COIN. The COIN never touches the subledger.
Verdict: the non-mintable + non-freezable + full-supply invariants are enforced exactly once (distribution)
and inherited by the twap via the place_bid mint pin + finding AQ; no redundant twap/subledger-side check is
warranted. No code change, no test added.

### [BLOCKED] distribution init_config — a freezable COIN could brick every claim (freeze-authority clause)
Vector: the distributed COIN IS the MetaDAO. If a distribution were created against a COIN with a live FREEZE
authority, that authority (the deployer) could (a) FREEZE the distribution VAULT — the config PDA can then
never transfer out, so EVERY claim reverts and the whole genesis payout is permanently bricked (DOS), or
(b) freeze an individual recipient's account to block their claim. init_config rejects such a COIN.
Analysis (distribution/src/lib.rs init_config:40): `if mint.mint_authority.is_some() ||
mint.freeze_authority.is_some() { return Err }`. The mint-authority clause stops the pre-mint-dilution attack
(separately tested); the FREEZE clause stops the freeze-brick. BLOCKED.
Coverage gap closed: `init_config_rejects_a_mintable_coin` exercises only the MINT-authority clause — the test
helper `create_mint` initializes mints with `None` freeze authority, so the freeze clause was never hit. Added
`init_config_rejects_a_freezable_coin` (distribution/tests/distribution.rs): a COIN with the mint authority
REVOKED + supply == total_supply + vault funded but a LIVE freeze authority -> init rejected (the freeze
authority is the only thing left to fail on, so it isolates that clause). MUTATION-VERIFIED against the real
.so: deleting `|| mint.freeze_authority.is_some()` makes init ACCEPT the freezable COIN and the test FAILS
(sharp single-guard). KEPT. INVARIANT: init_config must reject BOTH a live mint authority AND a live freeze
authority — a fixed, non-freezable COIN is what makes the distribution unbrickable.

### [VERIFIED-COVERED] Cross-program byte-offset coupling — all readers pinned (finding-T family audit)
Audited every place a program reads a FOREIGN struct by raw byte offset (the finding-T risk class):
- twap reads asset-0 `insurance` from the percolator slab at INSURANCE_OFFSET=448+301=749. PINNED by its OWN
  canary `insurance_offset_matches_real_percolator_slab` (chain.rs:1279): `offset_of!(MarketGroupV16HeaderAccount,
  insurance)` against the REAL percolator binary + `assert_ne!(vault, insurance)`. This canary is REQUIRED
  because the e2e hand-edits slab[749] (edit + read both at 749 would agree even if the field moved); the
  canary catches a sibling-percolator layout drift the e2e cannot.
- subledger reads the same slab `insurance` — PINNED by its own canary (insurance_percolator.rs:352, impair_market):
  offset_of! for insurance/vault/budget-remaining. Independent of the twap canary (separate program).
- genesis-vote reads the subledger Position (pool[8..40], owner[40..72], principal[72..80], start_slot[89..97])
  and Pool (outstanding[80..88], vote_authority[160..192]) by hardcoded offsets. VERIFIED all six match the REAL
  subledger struct (subledger:239-245 Position, :179-204 Pool). No separate canary needed: the gv↔subledger e2e
  (chain.rs full genesis) drives REAL subledger deposits/positions, so an offset drift makes the weight/quorum
  wrong and the e2e fails — unlike the hand-edited percolator case, this coupling's e2e genuinely catches drift.
Verdict: every raw-offset foreign read is either canaried against the real struct (the two percolator readers,
where the e2e can't catch drift) or e2e-validated with real data (gv->subledger). No gap, no test added.

### [VERIFIED-COVERED] place_bid state guard / remaining-account smuggling / capped-pull conservation
Three probes this tick, all resolve to existing coverage or by-design safety:
- place_bid rejects when `book.state != BOOK_STATE_OPEN` (lib.rs:39) so a bid can't be slipped into a SETTLED
  (mid-claim) book and mix with already-settled slots. The NEGATIVE is explicitly pinned: chain.rs:3686
  asserts `place_bid(...).is_err()` while the book is SETTLED ("book is settled — placing is blocked until it
  drains"), then bidding succeeds only after claims reopen it.
- Remaining-account smuggling: execute reads the SEND coin_sink via next_account_info ONLY when
  sink_mode==SINK_SEND, and pins it to book.coin_sink; BURN mode reads no trailing account. claim/trigger have
  fixed account lists. So extra/trailing accounts are never interpreted.
- Capped percolator pull: if WithdrawInsuranceLimited delivers C < burnable, execute's budget = the actual
  holding balance (C), and the unpulled `burnable - C` simply becomes next round's surplus. The ratchet stays
  consistent: reserved_floor' = reserved_floor + retained <= insurance - C = insurance' (since C <= burnable),
  so the floor never exceeds the live insurance. No loss, no over-protection.
Verdict: no new gap, no test added/deleted. Reachable surface saturated; the only OPEN items remain the two
off-harness orchestration invariants — the deposit-deadline (DESIGN-DOS below) and durable 1-week timelock.

### [DESIGN-DOS / ORCHESTRATION] Quorum griefing — depositing during the vote inflates `outstanding` to block the trigger
Vector (genesis finalization DOS): the permissionless trigger requires `total_voted_principal*2 > live_
outstanding` (genesis-vote/src/lib.rs:739, read LIVE from the subledger pool). insurance_deposit RAISES
`pool.outstanding_principal` (subledger/src/lib.rs:941) and has NO deposit deadline / phase / "voting closed"
gate — and "kickstart" (the design's deposit-close step) exists ONLY in the design memory, NOWHERE in the four
on-chain programs. So an attacker can deposit a non-voting stake DURING the vote/trigger phase to inflate
`outstanding` and push the quorum out of reach, blocking the winning proposal from ever sealing.
COST (cheap for close votes): to block, the attacker needs `D >= 2V - O` (V = voted principal, O = prior
outstanding). For a BARE majority (V ≈ O/2, so 2V ≈ O) that is `D ≈ 0+` — a tiny, front-running deposit flips
a 50.1% quorum. For a strong majority (V = 0.8 O) it costs `D ≈ 0.6 O`. The deposit is capital-at-risk but
WITHDRAWABLE (the attacker never votes, so never vote-locks), so the griefing is sustained + near-free for the
attacker on a close vote; they front-run each trigger attempt.
WHY NOT a clean on-chain fix: the LIVE read is DELIBERATE — `trigger_uses_live_pool_outstanding_not_stale_cache`
proves a frozen/stale-LOW snapshot lets a minority that voted early CAPTURE the distribution after honest
deposits grow the pool. So you cannot just snapshot `outstanding` (that regresses minority-capture); and
min(snapshot,live) regresses it too. The only correct fix is a DEPOSIT DEADLINE: close deposits before the
vote settles, after which `outstanding` can only DROP via exits (which legitimately lowers the bar) — exactly
the "kickstart" the design names but the code never implements. That deadline cannot live in the subledger
(it is a generic, reusable pool the MetaDAO has no authority over and that knows nothing of gv phases), so it
must be enforced by the genesis ORCHESTRATION (task #6, unbuilt) revoking/closing the pool's deposit path
before voting — the same off-harness gap the on-chain checks (now incl. the timelock-minimum FIX) cannot
fully cover.
Verdict: REAL DOS surface, not closeable by the four on-chain programs alone; flagged as a hard requirement
for the orchestration tool (must close deposits before the vote settles). No code change (a naive on-chain
snapshot would reopen the worse minority-capture LOF); no test added (a test here would assert the griefing
SUCCEEDS — a weakness-pin — and would need rewriting once the orchestration deadline lands; the mechanics are
certain from lib.rs:739 + :941). Escalated to the user.
BOUNDING (follow-up): the grief surface is exactly the QUORUM numerator and exactly the window [vote-open,
trigger]:
- MAJORITY side is NOT a new hole: denying a proposal its `support_weight*2 > total_cast_weight` by voting a
  decoy is the deliberate, already-tested winner-take-all DEADLOCK (`e2e_tied_weight_between_proposals_
  deadlocks_until_broken`), resolved by voters consolidating — AND it costs more, because casting that decoy
  weight VOTE-LOCKS the attacker's capital (cannot withdraw until retract), unlike the quorum-grief whose
  deposit never votes and stays withdrawable.
- TOP-UP variant (vote a tiny principal, then top up huge to inflate outstanding) works — insurance_deposit's
  re-deposit branch only blocks `withdrawn`, not `vote_locked` (subledger:887) — but is the SAME mechanism and
  strictly more expensive than the no-vote fresh deposit; subsumed.
- WINDOW is pre-handoff only: deposits close when the operator handoff revokes the pool's authority
  (`e2e_post_handoff_deposit_blocked_by_authority_revoke`), so the twap/auction phase is immune; the grief is
  confined to the pre-trigger voting window. This is exactly why the orchestration's deposit-deadline (close
  deposits BEFORE the vote settles, not at handoff which is AFTER the trigger) is the fix.

### [VERIFIED-COVERED] twap init_book — instruction-level audit (book-squat / account-substitution surface closed)
Probed init_book as a book-squat vector: the AuctionBook (PDA ["twap_book", config]) holds the reserve,
round length, sink mode and coin_sink — squatting it with malicious params (reserve 0 -> whale drains the
surplus; hostile coin_sink -> bought COIN to the attacker) would be critical. Read every guard:
- Squads-gated: `require_squads_vault(squads_vault, &config)` at the TOP (before any token-account read), so a
  forged/unsigned vault is rejected before anything else. This is the SAME helper already pinned sharply by
  `e2e_attacker_cannot_lower_the_reserve_without_squads` (set_reserve) and the reconfigure missing-signer
  test — a dedicated init_book gating test would be redundant with those.
- coin_escrow + settlement_usd: each must be owned by the derived book-escrow PDA, correct mint, and
  amount == 0; holding must be owned by twap_authority + collateral mint. coin_mint == config.coin_mint.
- SEND coin_sink: rejected if == coin_escrow (finding AS, prevents the escrow->escrow self-loop) and must be
  coin-mint.
- book PDA derived from config; `data_len() != 0 -> AccountAlreadyInitialized` (reinit guard).
The amount==0 escrow checks are defense-in-depth only: the escrow/settlement/holding are FRESH accounts the
DAO creates (not PDAs an attacker can predict/pre-fund), so a pre-funded-escrow squat is not externally
reachable. Happy path is exercised by setup_auction in 60+ chain tests.
Verdict: no new gap, no new test (gate is pinned via the shared helper; the pre-fund check isn't externally
reachable), no test to delete.

### [BENIGN] Uniform-price clearing — does floor rounding let a seller extract value from the buyback? (no)
Vector: at clear, every filled bid sells `coin_i = floor(usd_i * cm/um)` COIN for `usd_i` USD, where cm/um is
the MARGINAL bid's rate P*. The floor rounds the COIN the seller delivers DOWN, so the protocol receives
slightly fewer COIN per USD than the exact P* — i.e. rounding favors the SELLER. Could a bidder farm this?
Analysis: the shortfall is at most 1 atom per filled bid (`floor` drops < 1 unit), and the auction fills at
most MAX_BIDS = 32 bids per round, so the protocol's worst-case "overpay" is ≤ 32 atoms of COIN per round —
for a 6-decimal COIN that is 0.000032 COIN, economically nil and not accumulable (one active bid per bidder,
one round per execute, the surplus budget caps total spend). The opposite direction (a bid too small to buy a
whole atom, coin_i == 0) is treated as UNFILLED with a full refund (the protocol never pays USD for 0 COIN —
see the finding-AE roll restore). So the floor is the SAFE rounding choice and the residual is bounded +
negligible. Covered by `e2e_uniform_price_partial_marginal_fill` (asserts the exact floored coin_i / refunds).
Also corrected a stale test-count in the checkpoint header (was 134, the breakdown sums to 133; full suite
re-run confirms 133). No new test, no code change.

### [VERIFIED-COVERED] twap accept_operator + permissionless cranks — doubly-defended, no redundant test needed
Probed the operator-handoff and permissionless-crank surface this tick; all resolve to existing coverage:
- `twap accept_operator` (rotate the asset-0 insurance operator to twap_authority) is DOUBLY-DEFENDED: the
  twap gate requires `squads_vault.is_signer` AND `squads_vault.key == squads_default_vault(config.squads_
  multisig)` (forged/unsigned vault rejected at the twap), and it CPIs percolator UpdateAssetAuthority which
  INDEPENDENTLY enforces the market asset_admin. A mutation removing the twap key/signer check would NOT make
  a bypass succeed (percolator backstops), so a twap-side direct-bypass test is non-sharp AND redundant with
  the already-present subledger mirror `e2e_attacker_cannot_grant_operator_bypassing_squads` + the handoff
  timelock tests. Do not add one.
- Permissionless cranks (trigger/execute/claim/burn_unclaimed) each require a signer only to pay, and pin
  ALL effects to recorded/derived accounts (claim -> recorded canonical ATAs; execute -> config-bound slab/
  vault/holding/escrow; trigger -> pinned distribution_config/proposal; burn -> config vault+mint). No
  privileged state is reachable by the choice of cranker.
- execute ratchet (`reserved_floor += retained`) cannot overflow: reserved_floor starts u128::MAX (surplus
  saturating_sub -> 0 -> retained 0, no pull) until the DAO sets a real (<= insurance, u64-bounded) floor,
  after which retained <= surplus <= insurance keeps it u64-scale.
Verdict: no new gap, no new test (would be redundant/non-sharp), no test to delete. Reachable six-binary
surface remains saturated; recent NEW finds were the untested negative half of an existing guard (missing-
signer, weight-0 quorum, eviction redirect) or the one missing on-chain enforcement (timelock minimum, now
FIXED). The realistic residual risk is OFF this harness: the unbuilt local proposal-generation/orchestration
tool (task #6), the sole guarantor of correct multisig members/threshold and the genesis wiring the on-chain
checks cannot fully cover.

### [SCOPE] Timelock guarantee — enforced at bind, NOT durable; bind surface is complete (follow-up to the FIXED entry)
Two follow-ups to the init_config time_lock fix below, to bound exactly what the 1-week guarantee covers:
1. BIND SURFACE COMPLETE: init_config is the ONLY place the twap reads/binds a Squads multisig (sole
   SQUADS_MULTISIG_DISC + MIN_TIMELOCK_SECS site; squads_multisig is then immutable — it is folded into the
   config PDA seed). So there is no second, unchecked path that could bind a short-timelock multisig. The
   subledger pool init has no multisig at all (its vote_authority is the gv config, validated gv-side); it
   instead pins the canonical percolator insurance vault (F-VAULT: vault.mint==mint && vault.owner==
   perc_vault_authority(slab)), which transitively validates the slab. No sibling gap.
2. DURABILITY IS A GOVERNANCE PROPERTY, NOT A CODE GUARANTEE: the multisig's `time_lock` is enforced at
   bind, but Squads lets the `config_authority` (= the DAO) change multisig config (incl. time_lock) via a
   CONFIG instruction, which is NOT itself gated by the vault timelock. So the DAO can shorten its own
   window after the fact; require_squads_vault only checks the vault PDA identity, never the CURRENT
   time_lock. This is intentional and not a fixable code vuln (the twap cannot police Squads' own config
   authority, and the elected futarchy IS the config_authority). The 1-week window therefore protects
   depositors against NON-DAO actors and against a SLOW DAO that doesn't actively reconfigure — NOT against
   the futarchy deliberately collapsing its own timelock. Documented so future readers/auditors don't assume
   the on-chain check makes the window tamper-proof against the DAO itself.
Verdict: no new test (the bind point is already pinned by `twap_config_rejects_a_multisig_below_the_one_week_
timelock`; the durability limitation is by-design and not a guard to pin). No code change.

### [FIXED] twap init_config — bound a sub-1-week-timelock Squads multisig, voiding the depositor exit window
Vector: the security model is DAO -> Squads (1-week timelock) -> TWAP -> percolator insurance. The 1-week
delay is the depositor-protection window: time to react/exit before any insurance-affecting DAO action lands.
init_config bound a multisig and verified its owner, disc, and config_authority == the DAO — but NEVER its
`time_lock`. The timelock lives in the MULTISIG account, not the TWAP config, so a config bound to a 0/short-
timelock multisig (config_authority still = the DAO) would silently void the window: the DAO (or whoever set
up the genesis) could act on insurance instantly, leaving depositors no time to exit. The premise was trusted
to the (unbuilt, off-harness) orchestration tool rather than enforced on-chain.
Severity/reachability: not externally exploitable in the correctly-orchestrated flow (the legit config binds
the genesis 1-week multisig, and a parasite config can't drive the real operator — finding AQ). But it is a
real defense-in-depth hole: a buggy or hostile genesis deployer could bind a short-timelock multisig and the
TWAP would accept it, defeating the system's headline guarantee with nothing on-chain catching it.
FIX (twap-program/src/lib.rs init_config): read the multisig's on-chain `time_lock` (u32 @ [74..78], after
config_authority [40..72] + threshold u16 [72..74]) and reject `time_lock < MIN_TIMELOCK_SECS` (604_800 =
7 days). The byte offset is validated against the REAL Squads binary by the whole existing chain suite (all
64 prior tests create 1-week multisigs and still pass — a wrong offset would reject them). Added
`twap_config_rejects_a_multisig_below_the_one_week_timelock`: a 1-day-timelock multisig (config_authority =
DAO, all other links valid) is REFUSED; the same wiring with a 1-week timelock is accepted. MUTATION-VERIFIED:
removing the time_lock check makes the short-timelock bind succeed and the test FAILS (sharp). INVARIANT:
init_config must enforce the 1-week minimum on the bound multisig's on-chain time_lock; the config_authority
check alone does not guarantee the protection window.

### [VERIFIED-COVERED] Auction comparator overflow + own-vault symmetry — probed, already closed
Creative probe this tick: the uniform-price auction ranks/clears bids with two comparators; an extreme
`usdc_atoms` could (a) overflow the RESERVE comparison to smuggle a below-reserve bid past the guard, or
(b) overflow the bid-vs-bid RANKING/eviction cross-multiply to mis-rank a garbage bid above legit ones
(evicting them / winning the budget for ~0 COIN). Traced to the actual code:
- `cmp_rate` (the reserve eligibility comparator) is the continued-fraction algorithm — pure div/mod, NO
  multiplication, overflow-safe even at u128::MAX (lib.rs unit test `cmp_rate(u128::MAX, 3, u128::MAX, 4)`).
  So the reserve cannot be bypassed by extreme magnitudes.
- `cmp_bid` (ranking/eviction) is a naive `coin_a * usdc_b` cross-multiply — but place_bid bounds BOTH
  coin_atoms AND usdc_atoms to u64 via `as_u64` (lib.rs ~26-27, finding AC: "subsumes the old
  coin_atoms*usdc_atoms overflow check"), so the product is u64*u64 < u128::MAX — no overflow. The u64
  bound is TESTED end-to-end: `e2e_*` (chain.rs:4069+, finding AC) rejects a `(u64::MAX as u128)+1` usdc bid.
- Own-vault pool path fully covered (subledger.rs: healthy/with-surplus/impaired-order-independent/
  non-owner-withdraw/vault-not-owned-by-pool); and BOTH deposit paths symmetrically reject re-deposit into a
  `withdrawn` position (insurance lib.rs:887, own-vault lib.rs:517) — no stranding asymmetry.
Verdict: no new gap. Reachable surface remains comprehensively covered; recent NEW pins have all been the
untested NEGATIVE half of an already-guarded boundary (missing-signer is_signer halves, weight-0 quorum,
eviction redirect). Recording the comparator-overflow analysis so it is not re-derived.

### [BLOCKED] genesis-vote vote — flash-deposit quorum pump via a weight-0 (too-recent) vote (Sybil timing)
Vector: vote weight = floor(log2(hold_age)) * principal, and a position with age < 2 has ZERO weight. The
vote handler rejects a weight-0 vote outright. That rejection is load-bearing because vote ADDS the position's
PRINCIPAL to config.total_voted_principal (the quorum numerator: total_voted_principal*2 > outstanding) right
AFTER the weight check. Hostile idea: deposit a large sum and vote in the SAME slot (age 0, weight 0). If the
weight-0 vote were accepted, the principal would still land in total_voted_principal — letting a last-second
flash deposit PUMP the quorum toward a premature trigger while contributing no time-weight at all.
Analysis (genesis-vote/src/lib.rs vote): `vote_weight(principal, age)` returns 0 for age < 2 (also avoids
ilog2(0)); then `if weight == 0 { return Err }` refuses the vote before any tally mutation. So a too-recent
position cannot vote and its principal never counts toward quorum. BLOCKED.
Coverage gap closed: the positive weight path (warp 1024 slots -> weight 10*principal) was exercised inside
the vote-lock tests, but the NEGATIVE (a too-recent position is refused AND credits no principal) had no test.
Added `a_too_recent_position_cannot_vote_or_pump_the_quorum` (subledger/tests/insurance_percolator.rs):
deposit then vote in the same slot -> rejected, proposal support stays (0,0); then warp to age 1024 and the
SAME position votes, crediting principal + weight 10*principal. MUTATION-VERIFIED against the real gv .so:
removing the `weight == 0` rejection lets the fresh vote succeed and credit its principal (support != 0) and
the test FAILS (sharp single-guard). KEPT. INVARIANT: vote must keep refusing weight-0 ballots BEFORE adding
principal to total_voted_principal — capital must sit at risk (age >= 2) before it can count toward quorum.

### [BLOCKED] distribution seal_winner — missing-signer seal of an attacker proposal (theft of the whole COIN supply)
Vector: seal_winner marks the WINNING proposal sealed; sealing is the gate to claiming the funded vault
(the entire fixed COIN supply). It gates on BOTH the authority's SIGNATURE and its key (== config.authority,
the gv config PDA in genesis). The canonical missing-signer risk: if it accepted a KEY match without a
signature, an attacker could NAME the real authority as a read-only account and seal an attacker-chosen
proposal with no authorization — then claim the whole supply. (The authority is a PDA in genesis, signing
ONLY via the gv trigger CPI, so is_signer is the line between "the vote authorized this seal" and "someone
merely named the vote".)
Analysis (distribution/src/lib.rs seal_winner): `!authority.is_signer -> MissingRequiredSignature` AND
`*authority.key != config.authority -> MissingRequiredSignature`. So a non-signing real authority and a
wrong signing key are both rejected. BLOCKED.
Coverage gap closed: the happy-path test's imposter case pins the KEY half (a wrong signer is rejected) but
NOT the is_signer half (the real authority named unsigned). Added
`seal_rejects_naming_the_authority_without_its_signature`: name the real authority as a read-only non-signer,
no one signs as it -> rejected, config.sealed_proposal stays default; then the genuine authority signing
still seals (guard is the signature, not a freeze). MUTATION-VERIFIED against the real .so: removing the
`!authority.is_signer` check lets the unsigned seal succeed and the test FAILS (sharp single-guard).
KEPT. INVARIANT: seal_winner must keep BOTH the authority is_signer and key checks — a config.authority KEY
match without its SIGNATURE is not authorization. (Same class as the reconfigure + set_vote_lock missing-
signer pins; this one guards the COIN-supply seal, the highest-stakes of the three.)

### [BLOCKED] subledger set_vote_lock — owner self-unlocking a live vote to exit capital (the core Sybil hole)
Vector: the whole bootstrap's Sybil resistance rests on "a vote can never outlive the capital backing it":
a live ballot vote-LOCKS the depositor's principal, and the lock is cleared ONLY by the gv vote-RETRACT CPI
(which also removes the ballot's weight/principal). set_vote_lock requires BOTH the owner AND the
vote_authority (gv config PDA) to sign. Hostile idea: the owner calls set_vote_lock(0) DIRECTLY on their own
position, NAMING the gv config as a read-only (unsigned) account — clearing the lock without retracting —
then withdraws their principal while the ballot stays live: a vote backed by capital no longer at risk.
Analysis (subledger/src/lib.rs process_set_vote_lock): `!vote_authority.is_signer -> MissingRequiredSignature`
(plus owner.is_signer, plus pool.vote_authority == vote_authority.key). The gv config PDA only signs via the
gv vote-retract CPI, so a bare owner cannot toggle the lock — the self-unlock is rejected. BLOCKED.
Coverage gap closed: `hostile_vote_authority_cannot_freeze_a_depositor` pins the OWNER-sig half (can't LOCK
a victim without their signature). `vote_locked_principal_cannot_exit_until_retracted` pins the withdraw-side
guard (locked -> can't exit). But the vote_authority-SIG half — owner can't SELF-UNLOCK — was untested. Added
`owner_cannot_self_unlock_a_live_vote_to_exit_capital`: alice votes (locks), then directly calls
set_vote_lock(0) naming the gv config UNSIGNED -> rejected, position stays locked, withdraw still refused,
capital still at risk. MUTATION-VERIFIED against the real .so: removing the `!vote_authority.is_signer` check
lets the self-unlock succeed and the test FAILS (sharp single-guard). KEPT. INVARIANT: set_vote_lock must keep
requiring BOTH the owner AND the vote_authority signatures — the lock toggles only inside the gv vote CPI,
never by the owner alone; a vote_authority KEY match without its SIGNATURE is not authorization.

### [BLOCKED] twap reconfigure — missing-signer bypass of the burn-share gate (Squads/timelock bypass DOS)
Vector: `reconfigure` (IX 2) changes the DAO's burn share (surplus_buy_burn_bps), Squads-vault-gated behind
the 1-week timelock. Unlike the other mutators it does NOT call require_squads_vault — it INLINES the gate.
The canonical Copenhagen "missing signer check" risk: if the inlined gate checked only the vault KEY and not
is_signer, an attacker could merely NAME the real Squads vault as a read-only (unsigned) account and
reconfigure the burn policy freely — bypassing the DAO AND the entire 1-week timelock (governance-capture
DOS: force bps to 0 to kill the buyback, or 100% to route all surplus to burn with no retention).
Analysis (twap-program/src/lib.rs process_reconfigure): it checks BOTH `!squads_vault.is_signer ->
MissingRequiredSignature` AND `*squads_vault.key != squads_default_vault(config.squads_multisig) ->
IllegalOwner`. So a non-signing real vault and a forged signing key are both rejected. BLOCKED.
Coverage gap closed: the existing `reconfigure_only_via_squads_vault_execute_after_timelock` covers only the
TIMELOCK negative (execute-before-elapsed), exercising the gate via a proper Squads execute — it never tries
a DIRECT reconfigure with a non-signing or forged vault. Added `e2e_reconfigure_rejects_a_non_signing_or_
forged_vault`: (1) reference the real vault as a NON-signer -> rejected (is_signer); (2) attacker signs as
their own key posing as the vault -> rejected (key); bps stays 8000 in both. MUTATION-VERIFIED against the
real .so: removing the `!squads_vault.is_signer` check lets attack (1) succeed (bps -> 0) and the test FAILS
(sharp single-guard on the missing-signer class). KEPT. INVARIANT: reconfigure's inlined gate must keep BOTH
the is_signer and canonical-vault-key checks; an authority KEY match without a SIGNATURE is not authorization.
This completes the config-mutator auth coverage: set_reserved_floor, set_reserve, set_coin_sink, shutdown,
reconfigure all now have a direct non-Squads rejection test.

### [BLOCKED] twap set_reserve — non-Squads caller lowering the reserve to drain the surplus (auth-bypass LOF)
Vector: the auction reserve rate is the DAO's guard against a whale's expensive (low-rate) bid dragging the
uniform clearing price down and making the protocol overpay (see e2e_reserve_blocks_expensive_bid_from_
draining_surplus). `set_reserve` (IX 6) is Squads-vault-gated. Hostile idea: a plain attacker calls set_reserve
directly — posing as the Squads vault — to LOWER a protective reserve to 0/1 (accept ANY bid), re-exposing
the whole surplus to be drained for ~1 COIN at a terrible clearing price.
Analysis (twap-program/src/lib.rs process_set_reserve): `require_squads_vault(squads_vault, &config)` demands
the signer (a) be a signer AND (b) equal `squads_default_vault(config.squads_multisig)` — the config's
canonical Squads vault, reachable only via a Squads execute. A plain attacker key fails (b) -> IllegalOwner.
BLOCKED.
Coverage gap closed: the cross-config test `e2e_config_a_cannot_mutate_config_bs_book_reserve` rejects via the
`book.config` pin (a foreign config can't touch this book) — it does NOT exercise the require_squads_vault
SIGNER gate (the foreign vault IS its own config's vault). The DIRECT non-Squads set_reserve had no test
(unlike set_reserved_floor, which has `e2e_attacker_cannot_lower_surplus_floor_without_squads`). Added
`e2e_attacker_cannot_lower_the_reserve_without_squads`: DAO sets a protective 2/1 reserve via Squads, then a
plain attacker posing as the vault tries to lower it to 0/1 -> rejected, reserve stays 2/1. MUTATION-VERIFIED
against the real .so: removing require_squads_vault from process_set_reserve lets the attacker's set succeed
(reserve -> 0/1) and the test FAILS. KEPT (sharp single-guard). INVARIANT: every twap config mutator
(set_reserve / set_reserved_floor / set_coin_sink / reconfigure) must keep require_squads_vault; the
book.config pin alone does NOT gate the signer.

### [BLOCKED] genesis-vote init_config — re-initializing a live config to wipe the vote tallies (reinit DOS)
Vector: init_config is permissionless. If an already-initialized gv config could be re-initialized, the
second init would RESET the global tallies (total_voted_principal / total_cast_weight / outstanding) to 0
while every voter's ballot PDA + subledger vote-lock persists — desyncing the genesis: it could never reach
quorum again (permanent DOS) and an in-flight winning vote would be silently wiped.
Analysis: BLOCKED by TWO independent layers — (1) the `data_len() != 0 -> AccountAlreadyInitialized` gate
at the top of init_config, and (2) the robust `create_pda` (allocate + assign via invoke_signed require a
SYSTEM-owned, data-empty account; after the first init the config is PROGRAM-owned, so allocate fails). So
re-init is doubly refused. Verified via mutation: even with the data_len gate removed, the create_pda
backstop still rejects the second init (the test stays green) — i.e. neither guard alone is a single point
of failure.
Coverage gap closed: the reinit-DOS was tested for the subledger pool (`insurance_pool_cannot_be_reinitialized_
after_funding`, finding AJ) but not for the gv governance config. Added `gv_config_cannot_be_reinitialized_to_
wipe_a_vote` (genesis-vote/tests/seal.rs): with a quorum+majority vote in progress, a second init_gv is
rejected and the vote then triggers + seals exactly as before (tally intact). KEPT — pins the outcome (a live
governance config can't be reset) end-to-end; defense-in-depth means a single-guard regression won't silently
reopen it. INVARIANT: init_config must keep the data_len re-init gate AND the system-owned-account robust
create; do not let either become the sole guard.

### [BLOCKED] twap place_bid eviction — redirecting the evicted bidder's escrowed COIN to the attacker (LOF)
Vector: when the book is full, a STRICTLY-better incoming bid evicts the weakest and must refund the
evictee's escrowed COIN. The incoming bidder passes the evict refund account as the trailing account.
Hostile idea: pass an attacker-controlled COIN account instead of the evictee's account, so the eviction
refund (the evictee's full escrow) lands in the attacker's hands — stealing a stranger's committed COIN
while taking their book slot.
Analysis (twap-program/src/lib.rs place_bid, ~1188-1207): the refund target is pinned to the weakest bid's
RECORDED `SL_COIN_ATA` (set to the evictee's CANONICAL COIN ATA at THEIR own place_bid, ~1243), and the
passed evict account must equal it exactly (`*evict_acct.key != evicted_ata -> InvalidAccountData`, ~1197).
So a mismatched/attacker account reverts the whole placement; the evictee's COIN can only ever go to the
evictee's canonical ATA (permissionlessly recreatable, finding V/AB). BLOCKED.
Coverage gap closed: the eviction HAPPY path (refund to the correct canonical ATA) and the not-better-bid
rejection were tested in `e2e_full_book_evicts_only_for_a_strictly_better_bid`, but the adversarial
REDIRECT was not. Extended that test (reusing its 32-bid full book — no duplicate setup): the better bidder
first tries to evict while redirecting the refund to a fresh attacker COIN account -> rejected, attacker
account stays 0, the evictee's COIN stays escrowed, the escrow total is untouched, and the attacker's own
bid COIN was NOT escrowed (tx reverted); then the HONEST eviction (correct canonical target) succeeds.
MUTATION-VERIFIED against the real .so: removing the `evict_acct.key != evicted_ata` pin makes the redirect
SUCCEED (attacker gets the COIN) and the test FAIL. KEPT (extended, no new test). INVARIANT: place_bid must
keep pinning the eviction refund to the weakest bid's recorded canonical SL_COIN_ATA; never refund to a
caller-supplied account.

### [BLOCKED] distribution init_config — lamport-prefund DOS on the config that custodies the COIN supply (finding AI)
Vector: the dist config PDA is deterministic (f(coin_mint, authority), both public) and init_config is
permissionless. System `create_account` aborts with AccountAlreadyInUse on ANY pre-existing lamports, so an
attacker can transfer 1 lamport to the config PDA (no signature needed) BEFORE the orchestrator inits it,
and the dust can never be swept from a system-owned PDA. With plain create_account this PERMANENTLY bricks
the distribution config that custodies the ENTIRE COIN supply (no config -> the funded vault can never be
sealed/claimed -> the genesis payout is frozen). Analysis: init_config's `create_pda_robust` (top-up +
allocate + assign via invoke_signed; re-init gated on data_len, not lamports) tolerates the dust. BLOCKED.
Coverage gap closed: the same prefund-DOS was tested for the subledger pool, twap book, and (prior tick) gv
config inits — this completes the set with the distribution config init, the account that holds the COIN
supply. Added `lamport_prefund_cannot_brick_config_init` (distribution/tests/distribution.rs): dust the PDA,
init STILL succeeds, the config is program-owned with data, and a proposal registers under it (valid state,
not a half-allocated husk). MUTATION-VERIFIED against the real .so: swapping create_pda_robust for plain
create_account makes the dusted init fail and the test fail. KEPT. All four permissionless-init PDAs across
the stack now have a prefund-DOS regression test; INVARIANT unchanged (robust create + data_len re-init gate).

### [BLOCKED] genesis-vote init_config — lamport-prefund DOS on the genesis governance config (finding AI)
Vector: the gv config PDA is deterministic (f(coin_mint, subledger_pool), both public) and init_config is
permissionless. System `create_account` aborts with AccountAlreadyInUse on ANY pre-existing lamports, so an
attacker can transfer 1 lamport to the gv config PDA (a transfer needs NO destination signature) BEFORE the
orchestrator inits it — and the dust can never be swept from a system-owned PDA. If init used plain
create_account this would PERMANENTLY brick the genesis GOVERNANCE config (no config -> no voting/trigger ->
the whole genesis stalls). Analysis: gv's `create_pda` is robust (finding AI) — top-up the rent shortfall
with a plain transfer, then allocate + assign via invoke_signed (both only require data-empty + system-owned,
true for a merely pre-funded address); callers gate re-init on `data_len()!=0`, not lamports. So it tolerates
the dust. BLOCKED.
Coverage gap closed: the same prefund-DOS was tested for the subledger pool init
(`lamport_prefund_cannot_brick_insurance_pool_init`) and the twap book init
(`e2e_lamport_prefund_cannot_brick_book_init`), but NOT for the gv config init (nor the distribution config
init — same class, still untested, candidate for a later tick). Added `lamport_prefund_cannot_brick_gv_config_init`
(genesis-vote/tests/seal.rs): dust the gv config PDA with 1 lamport, then init_gv STILL succeeds, the config is
owned-by-program with data, and a real proposal registers + seals (genesis proceeds normally).
MUTATION-VERIFIED against the real .so: swapping create_pda for plain create_account makes the dusted init
fail and the test fail. KEPT. INVARIANT: every permissionless genesis-path init must keep using the robust
create (top-up + allocate + assign) and gate re-init on data_len, never lamports.

### [BLOCKED] twap execute roll — phantom-claim after a marginal-zero-coin fill (finding-AE restore, anti-spoof bypass)
Vector: `execute` is a "roll" (nothing bought) when total_coin==0. There are TWO ways that happens:
(a) budget==0 (surplus below floor) — `marginal` is never set, the settle loop is skipped; (b) budget>0
but every fill rounds to coin_i==0 (a sub-atom fill at a low bid rate). In case (b) the settle loop ALREADY
ran and wrote SL_SETTLED=1 + SL_COIN_REFUND=full on the slot BEFORE total_coin==0 forces the roll. The
finding-AE restore (lib.rs ~1505) MUST reset SETTLED/COIN_REFUND/USD_OWED. If it didn't, the bid is left
phantom-SETTLED with a full refund → claim (which gates on the SLOT's SL_SETTLED, NOT the book state) would
pay the bidder their whole escrow back immediately — a FREE exit of a committed bid with no cancel cooldown
(anti-spoof bypass) + a drain of the shared coin_escrow. Analysis: the restore resets all three fields, so
the rolled bid is byte-identical to pre-execute and claim refuses it (SL_SETTLED==0). BLOCKED.
Coverage gap closed: the existing roll test (`e2e_roll_with_committed_bid_settles_correctly_next_round`)
triggers the roll via budget==0 — case (a), where `marginal` is never set, so the restore is a NO-OP and
the SETTLED=1→0 reset is never exercised. Added
`e2e_roll_with_a_marginal_zero_coin_fill_leaves_no_phantom_claim` (reserve 0/1; a 1-COIN-for-1000-USD bid;
surplus forced to 0 so NO percolator CPI; holding hand-seeded with 5 USD → marginal IS set, coin_i=floor(5/1000)=0
→ roll through the restore). Asserts: no burn, nothing parked, budget rolls over, escrow intact, the rolled
bid is NOT claimable (the guard), and a next round with a real 1000-USD budget settles + burns it correctly.
MUTATION-VERIFIED against the real .so: disabling the restore loop makes the phantom claim SUCCEED and the
test FAIL (so it pins case (b), which budget==0 cannot). KEPT. INVARIANT: a roll (total_coin==0) must reset
SL_USD_OWED/COIN_REFUND/SETTLED on every occupied slot; claim must keep gating on the slot's SL_SETTLED.

### [BLOCKED] twap claim — replaying a settled slot to drain other winners' parked USD (double-spend LOF)
Vector: after `execute`, every winner's `usd_owed` is parked together in ONE shared settlement-USD account
and pulled per-slot by the permissionless `claim`. Hostile idea: claim a settled slot, then call `claim`
on the SAME slot again — each replay pays `usd_owed` (+ `coin_refund`) once more out of the shared pool,
draining the OTHER winners' parked payouts (a direct LOF against co-winners).
Analysis (twap-program/src/lib.rs process_claim): claim requires `SL_OCCUPIED==1 && SL_SETTLED==1`
(line ~48), pays out, then ZEROES the slot (`*b = 0`, line ~71) clearing SL_OCCUPIED — so a second claim
sees OCCUPIED==0 and is refused. BLOCKED. (Cancel-vs-claim double-spend is separately blocked by
`e2e_cancel_cannot_double_spend_a_settled_bid`.)
Coverage gap closed: the happy-path `e2e_buy_burn_uniform_price_dutch_auction` claims each slot ONCE but
never replays — and there every winner claims, draining the pool to 0, so a replay would fail on pool
EXHAUSTION even if the slot-zero guard regressed (it didn't isolate the guard). Added
`e2e_claim_cannot_be_replayed_to_drain_other_winners`: two SYMMETRIC bidders (400k COIN/200k USD each)
clear at P*=2, settlement parks 400k = two equal 200k shares; alice claims slot 0 once, then a REPLAY of
slot 0 is refused while bob's 200k is still parked (so only the slot-zero guard can block it, not
exhaustion); alice never double-collects; bob then claims his intact share. KEPT — pins the claim
double-spend with the pool deliberately still funded. INVARIANT: process_claim must keep gating on
SL_OCCUPIED/SL_SETTLED and zeroing the slot after payout; never pay a slot whose OCCUPIED flag is clear.

### [VERIFIED-COVERED] Sweep tick — seven boundaries probed, each already pinned by a named test (no new test, no redundancy)
A breadth tick: picked seven plausible LOF/DOS vectors across the repo and traced each to the ACTUAL
code + the existing test that pins it. All BLOCKED and already covered — recording the vector→test map so
future iterations don't re-derive them.
1. Subledger haircut SEQUENTIAL conservation (finding L): two depositors racing to exit an impaired pool,
   rounding (mul_div_floor) must not let Σ payouts exceed the vault or strand the late exiter. Covered by
   `impaired_insurance_exit_is_pro_rata` (Alice+Bob both exit a 50%-impaired pool, order-independent, vault
   fully/fairly distributed → 0). floor is the safe direction (each payout ≤ exact share). Unit values in
   `principal_policy_impaired_is_pro_rata`.
2. Subledger accept_operator authority-bypass: accept_operator is permissionless but hardcodes the new
   authority to the pool's own PDA and relies on percolator UpdateAssetAuthority to require the real
   asset_admin (Squads vault) co-sign. Covered by `e2e_attacker_cannot_grant_operator_bypassing_squads`
   (forged asset_admin → percolator rejects) + `percolator_update_asset_authority_operator_encoding_is_accepted`
   (random key can't hijack the operator).
3. Finding-O floor zero-pull: execute pulls `saturating_sub(insurance, reserved_floor)` so insurance ≤ floor
   pulls nothing. Covered by `e2e_execute_pulls_nothing_when_insurance_below_floor` (the == boundary is the
   same saturating branch).
4. Distribution claim AFTER the window closes (`slot >= window_end`): a late claimer must not pull before a
   burn cranker runs. Covered by `unclaimed_is_burned_after_window` ("window closed" claim rejection +
   unclaimed burned). claim/burn share an exact `window_end` boundary (no race).
5. TWAP book-stuffing DOS: one identity filling all MAX_BIDS slots to crowd out bidders / skew the marginal
   clearing price. place_bid rejects a bidder who already occupies a slot (lib.rs:1149, no self-replace).
   Covered by the chain.rs auction test ("a bidder cannot stack a second bid").
6. TWAP shutdown scope: shutdown is Squads-gated and limited to sweeping the holding. Covered by
   `e2e_shutdown_sweeps_holding_only_via_squads` (non-DAO rejected) + `e2e_shutdown_cannot_drain_escrow_or_settlement`
   (even the DAO can't redirect it at the COIN escrow / settlement-USD).
7. GV quorum/tally desync: total_voted_principal counts only LIVE ballots (retract decrements it; exit needs
   retract first via the vote-lock), and trigger re-reads live outstanding. Structurally tight (checked
   arithmetic + `trigger_uses_live_pool_outstanding_not_stale_cache` + `vote_locked_principal_cannot_exit_until_retracted`).
Verdict: reachable six-binary surface remains comprehensively covered; the only untested-boundary finds this
session have been in the under-probed distribution/genesis-vote programs (3 added in prior ticks). Highest-value
remaining targets stay OFF this harness (the unbuilt local proposal-generation tool; the rewards-program monolith).

### [BLOCKED] genesis-vote register_proposal — non-creator front-run freezing a stale snapshot (griefing DOS)
Vector: `register_proposal` is permissionless and creates the UNIQUE gv_proposal PDA `f(config,
dist_proposal)`, freezing a `(entry_count, total_amount)` SNAPSHOT that `trigger` later requires to match
the live distribution proposal EXACTLY (the anti-bait-and-switch guard, lib.rs:720-729). Hostile idea: a
griefer front-runs an honest creator and registers the creator's PARTIALLY-built distribution proposal —
seizing the only gv_proposal PDA (the creator then can't re-register: data_len != 0 -> AccountAlready-
Initialized) AND freezing a stale snapshot. The creator's remaining appends make the live proposal's
(entry_count, total_amount) diverge from the frozen snapshot, so trigger rejects forever -> the victim's
distribution can NEVER be sealed/win. Pure DOS, no capital needed.
Analysis (genesis-vote/src/lib.rs register_proposal, line 470-473): register reads the distribution
proposal's `creator` (header [48..80]) and requires `creator == *payer.key`. So only the proposal's own
creator can register it — they do so once it is COMPLETE — and a front-runner is rejected with IllegalOwner.
BLOCKED.
Coverage gap closed: the APPEND creator-binding (distribution lib.rs:417) was tested
(`e2e_non_creator_cannot_append_to_a_proposal`, `append_entries_rejects_a_foreign_creator`) and the
append bait-and-switch snapshot was tested (`e2e_bait_and_switch_appended_entries_cannot_be_sealed`), but
the REGISTER creator-binding — a DISTINCT guard against the snapshot-freeze griefing DOS — had no test.
Added `register_rejects_a_non_creator_front_runner` (genesis-vote/tests/seal.rs): an attacker cannot
register the victim's proposal; the genuine creator then registers AND the proposal seals end-to-end
(proving the PDA was never seized). KEPT — pins a distinct DOS boundary. INVARIANT: register_proposal must
keep binding `dist_proposal.creator == payer`; never make register fully permissionless, or any in-flight
proposal can be snapshot-frozen by a front-runner and bricked.

### [BLOCKED] distribution claim — a LOSING proposal draining the winner's shared vault (cross-proposal isolation)
Vector: the genesis votes among SEVERAL candidate COIN distributions, all registered as proposals under
ONE distribution config that owns ONE funded vault (= full COIN supply). Only the winner is sealed.
Hostile idea: register a self-dealing LOSING proposal that allocates the ENTIRE supply to the attacker;
after an honest proposal wins, claim from the attacker's losing proposal against the shared vault — a
direct drain of the winner's funds.
Analysis (distribution/src/lib.rs claim, line 518): claim binds `config.sealed_proposal == *proposal.key`
(plus `is_sealed()`), so only the single sealed (winning) proposal can ever pay; a losing proposal's claim
is refused with InvalidAccountData. seal is also one-shot (`config.is_sealed()` guard at :470), so the
loser can't be re-sealed to redirect the vault. BLOCKED.
Coverage gap closed: the single-proposal claim guards (no-claim-before-seal, non-authority seal,
double-claim, wrong-recipient) were all tested, but every existing claim test used ONE proposal — the
cross-proposal vault isolation (the real multi-candidate genesis shape) had no test. Added
`a_losing_proposal_cannot_claim_the_winners_vault` (distribution/tests/distribution.rs): winner (100→alice)
+ a full-supply self-dealing loser (100→mallory) share one config/vault; seal the winner; mallory's claim
from the loser is refused, resealing the loser is refused, mallory gets 0, and alice claims the full,
untouched 100. KEPT — pins the cross-proposal isolation (distinct from the single-proposal guards).
INVARIANT: claim must keep binding `config.sealed_proposal == proposal` and seal must stay one-shot; never
let a non-winning proposal reach the vault.

### [BLOCKED] distribution append_entries — foreign creator injecting a self-dealing entry (theft of headroom)
Vector: a distribution proposal is a candidate COIN distribution list; once its winner is sealed, every
entry becomes directly claimable by the named recipient. `create_proposal`/`append_entries` are
permissionless (anyone can sign). Hostile idea: while an honest creator's proposal is still in-flight
(not sealed) and has unallocated headroom (`total_amount < total_supply`), front-run/append a self-entry
to it — then the moment that proposal wins the genesis vote and is sealed, CLAIM the injected COIN. The
total-supply cap (lib.rs:442) does NOT catch this because the injection fits in the headroom.
Analysis (distribution/src/lib.rs append_entries, line 417): the proposal records its `creator` at
create time, and append enforces `header.creator == *creator.key` (plus `header.sealed`/`config.is_sealed`
gates at :420). So only the original creator can extend a proposal; a foreign signer is rejected with
InvalidAccountData. BLOCKED.
Coverage gap closed: the cap negative (`append_cannot_exceed_total_supply`) and seal/claim/burn paths were
tested, but the append creator-binding had NO negative test. Added `append_entries_rejects_a_foreign_creator`
(distribution/tests/distribution.rs): honest creator seeds 40/100; an attacker (own signer, creator slot)
tries to inject a 60-COIN self-entry into the headroom → rejected; the genuine creator then appends into
the same headroom successfully (binding is to the creator, not a freeze). KEPT — pins a direct LOF guard.
INVARIANT: append_entries must keep enforcing `header.creator == signer` and the not-sealed gate; never
make append permissionless, or a winning proposal could be poisoned with attacker entries before seal.

### [BLOCKED] gv init_config — front-run squat binding a foreign/unsealable distribution config (finding H negative)
Vector: `genesis-vote::init_config` is permissionless (only the payer signs) and the gv config PDA seed
binds `[gv_config, coin_mint, subledger_pool]` (finding R) — but the `distribution_config` it wires is a
STORED field, NOT in the seed. Hostile idea: front-run the honest orchestrator's init_config and bind the
genesis to a DISTRIBUTION the attacker controls (or one for a different coin), so the winner-take-all
trigger seals the wrong distribution (hijack the COIN payout) or seals one whose authority ≠ this config
(trigger's seal CPI reverts on authority mismatch → finalize bricked → DOS).
Analysis (genesis-vote/src/lib.rs init_config, lines 39-44): the wired distribution_config is validated
HARD at bind time — `owner == distribution_program`, disc `DISTCFG1`, `dc[8..40] (coin) == coin_mint`,
and `dc[72..104] (seal authority) == expected` (THIS gv config PDA). Combined with the distribution's own
seed binding its authority (finding P/AA: `dist_config = f(coin, authority)`) and the funded-vault
requirement (finding E: vault ≥ total_supply of the fixed-supply COIN), the ONLY distribution that
satisfies `authority == gv PDA` is the real one whose vault already holds the COIN — which the attacker
cannot forge (can't obtain the COIN; mint revoked). So the squat is structurally BLOCKED: a foreign
distribution fails the authority/coin check at init; a "correct" one can't be funded.
Coverage gap closed: the parallel POOL-binding negative was tested (`init_config_rejects_pool_not_bound_*`,
`gv_config_cannot_be_bound_to_a_substituted_pool`) but the DISTRIBUTION-binding negative was NOT. Added
`init_config_rejects_a_distribution_not_authority_bound_to_this_config` (genesis-vote/tests/seal.rs):
plants a fully-valid-looking dist config (right owner/disc/vault) with (a) right coin but attacker seal
authority → rejected, (b) right authority (gv PDA) but a different coin → rejected, then accepts the real
authority+coin-bound distribution (boundary is EXACT, not a blanket reject). KEPT — pins finding H's
distribution-side binding, which had no negative test. INVARIANT: init_config must keep validating BOTH
`dist.coin == coin_mint` AND `dist.authority == this gv config PDA`; never drop either, else the genesis
could be wired to a distribution it cannot seal (DOS) or one paying a different coin (hijack).

### [REVIEW] External PR/issue adversarial review (DPRK lens) — regression-inducing "fix" caught + rejected
Reviewed the only open GitHub items under a nation-state-adversary lens (subtle backdoors, supply-chain
swaps, social-engineered "fixes" that weaken existing guards). Issue #26 (`SrMessiSOL`) was a genuine
external report of finding AD (one `twap_authority` shared across configs with different percolator
programs). PR #27 "Bind TWAP authority PDA to Percolator program" fixed it at the AD level —
`["market-0-twap", market, percolator_program]` — but master had ALREADY superseded that with the
config-binding (finding AQ, `["market-0-twap", config]`, commit 939078f). So PR #27 was a
REGRESSION TRAP, not an implant: merging it would DOWNGRADE the seed to a weaker (perc-only) binding,
reopening the CRITICAL parasite-config insurance drain (a parasite config on the victim's market, own
squads/coin, `reserved_floor = 0`, sharing the operator PDA). Confirmed empirically this tick: reverting
the 4 signer sites to PR #27's market_slab-bound seed makes `e2e_parasite_config_on_same_market_cannot_drain_insurance`
FAIL — so that test is the standing CI regression guard. Worse, PR #27's branch was stale (referenced
the removed `pull_surplus`) and its `chain.rs` predated the parasite test, so the merge would have
CLOBBERED the very guard. Scanned the full diff: NO Cargo/lock/CI/build.rs changes (no supply-chain
swap), no removed signer/owner/seed checks, no `unsafe`/fs/net — a stale, correct-but-superseded fix
whose only effect on master is a downgrade. VERDICT: closed PR #27 (superseded, regression warning) and
issue #26 (fixed by AQ) on GitHub. INVARIANT: the twap_authority seed MUST stay config-bound
(`["market-0-twap", config]`); never re-weaken it to `[market, percolator_program]` or `[market]`.

### [STATE] Canonical-ATA refund recoverability (finding-V class) — complete across all 3 refund paths
The auction returns escrowed COIN on three paths; a stuck (closed) refund target must never be a
permanent loss or book-brick. All three deliver to the bid's RECORDED destination, which `place_bid`
sets to the bidder's CANONICAL ATAs (`bidder_coin_ata(bidder, coin_mint)` / `(bidder, collateral_mint)`,
lines 1242/1244 — findings V/AB), so any closed target is permissionlessly recreatable:
 - `claim` (settled bid): pins `usd_dest == dest_key && coin_ata == coin_key` (line 1602). Closed-target
   recovery TESTED by finding V (`e2e_closing_refund_ata_...`, COIN) + AB (`e2e_closing_usd_dest_...`, USD).
 - place_bid EVICTION (full book, strictly-better bid refunds the evicted bidder): pins
   `evict_acct == evicted_ata` (line 1197). Closed-target recovery TESTED by AM
   (`e2e_closed_weakest_ata_cannot_permanently_block_eviction`).
 - `cancel_bid` (unsettled bid, bidder-signed, cooldown-gated): pins `coin_ata == coin_key` (line 1698).
   This is the LOWEST-severity path and the only untested one — a closed target here does NOT brick the
   book (an un-cancelled bid simply settles normally at the next execute) and is self-inflicted (the
   bidder closed their own ATA), recoverable by recreating it. No test added (marginal vs V/AM: same
   canonical-ATA invariant, no book-brick, no external attacker).
Verdict: the no-permanent-brick-via-closed-refund-target guarantee holds on every refund path.

### [STATE] Permissionless-fund-mover redirect class — fully analyzed across all six binaries
Completing AV/AW/AX: enumerated every PERMISSIONLESS instruction that touches funds, across all six
binaries, to confirm none lets the caller redirect value to themselves. All BLOCKED:
 - twap `execute` (crank): the 3 movable-balance destinations are pinned + tested — holding (the surplus
   pull; cranker-cannot-redirect-surplus), coin_sink (SEND buyback; AV), settlement_usd (spent USD; AX).
 - twap `claim` (crank): usd_dest pinned (winner test) + coin_ata pinned (loser refund; AW) — both ==
   the bid's recorded CANONICAL ATAs (V/AB).
 - twap `cancel_bid`: bidder-BOUND (`SL_BIDDER == bidder.key`, only the bidder cancels their own bid) +
   refund to the recorded canonical coin_ata; no cranker, no redirect.
 - distribution `burn_unclaimed` (crank): pins `vault == config.vault && coin_mint == config.coin_mint`
   and BURNS (no transfer destination) — only the config's own remainder, after the window.
 - distribution `claim`: recipient-SIGNED (`pk == recipient`), recipient directs their own payout.
 - genesis-vote `trigger` (crank): pins distribution program/config/proposal and only SEALS — moves no
   funds to the cranker.
 - subledger `deposit`/`withdraw`: owner-SIGNED; the owner directs their own funds (and the holding
   intermediate is SPL-authority-bound, finding AU).
No new code/test: the twap crank paths are the only ones with a redirectable destination, and they are
now all pinned + tested; the rest move no funds to the caller or are owner/recipient/bidder-signed.

### [BLOCKED] AX. Permissionless cranker redirects execute's spent USD via a substituted settlement_usd (external LOF)
Completes the AV/AW sweep of permissionless-cranker payout redirects. `execute` parks the budget it
spends this round (`total_usd`, moved from the holding) into `settlement_usd`, from which winners later
claim their `usd_owed`. If `settlement_usd` weren't pinned, a cranker would pass THEIR OWN collateral
account and (a) steal the spent USD and (b) brick winners' claims, since claim reads `book.settlement_usd`
which would be left empty. BLOCKED: execute pins `settlement_usd == book.settlement_usd` in its main
account-validation block, so a substituted account is rejected before any transfer and the whole execute
reverts. Pinned by `e2e_execute_cranker_cannot_redirect_the_spent_usd`. With AV (coin_sink, the SEND
buyback sink) and the pre-existing cranker-cannot-redirect-surplus (the holding, the pull destination),
all THREE movable-balance destinations a cranker controls in execute are now pinned + tested; the
remaining main-block accounts (coin_escrow/coin_mint/market_slab/book_escrow/twap_authority) are
accounting/auth pins, not direct-theft destinations.

### [BLOCKED] AW. Permissionless cranker redirects a loser's COIN refund via a substituted coin_ata (external LOF)
Sibling of AV on the claim path. `claim` is PERMISSIONLESS and pays a settled bid's `coin_refund`
(coin_escrow -> coin_ata); for a LOSER (eligible but unfilled because the budget ran out) the refund is
the FULL escrowed COIN. If `coin_ata` weren't pinned, a cranker would claim the loser's slot with THEIR
OWN COIN account and steal the refund. BLOCKED: claim requires `coin_ata == the bid's recorded canonical
COIN ATA` (and `usd_dest == the recorded canonical collateral ATA`) — findings V/AB — so a substituted
account is rejected and the refund stays claimable to the bidder. The pre-existing
`e2e_claim_cannot_redirect_a_winners_payout` only covered the USD side AND its winner sold all its COIN
(coin_refund == 0), so the COIN-refund redirect with a NON-ZERO refund was untested. Pinned by
`e2e_claim_cannot_redirect_a_losers_coin_refund`: alice wins and takes the 400k budget, bob loses
(rate 0.25, unfilled) and is owed a full 100k COIN refund; a cranker claiming bob's slot with their own
coin account is rejected (0 redirected), then the honest claim delivers bob's 100k to his canonical ATA.

### [BLOCKED] AV. Permissionless cranker redirects the SEND buyback via a substituted coin_sink (external LOF)
`execute` is PERMISSIONLESS (any cranker turns it). In SEND (buyback) mode it transfers the bought COIN
to a `coin_sink` passed as a TRAILING account. If the sink were not pinned, a hostile cranker would
pass THEIR OWN COIN account and steal the entire buyback — a direct external LOF (the bought COIN is
the protocol's/treasury's). BLOCKED: execute checks `*coin_sink.key == book.coin_sink` (the DAO-set,
Squads-gated sink recorded on the book) before the transfer, so a substituted sink is rejected and the
whole execute reverts (book unsettled, COIN safe in escrow). Pinned by
`e2e_execute_send_cranker_cannot_redirect_the_buyback`: a cranker passes their own COIN account as the
sink -> rejected, 0 redirected; the honest execute (book-recorded treasury sink) then routes the 400k
bought COIN to the treasury. Distinct from AS (set-time self-loop guard) and AH (happy-path
burn->send flip) — this is the execute-time redirect by an external cranker, which was untested.

### [BLOCKED] AU. insurance_deposit holding-intermediate substitution/duplicate — SPL authority is the boundary
`insurance_deposit` routes funds user_ata -> `holding` (user-signed) -> percolator insurance vault
(`TopUpInsurance`, signed by the pool PDA). The subledger does NOT explicitly check `holding.owner ==
pool`; it relies on SPL authority at the CPI. Probed every substitution/duplicate of `holding`:
 - Foreign (non-pool-owned) holding: step 1 lands the user's funds there, but step 2's
   `TopUpInsurance` is the pool moving funds OUT of the holding — only the holding's authority can
   authorize that, and the pool's signature only covers pool-owned accounts, so the SPL transfer
   fails and the whole instruction REVERTS (step 1 rolled back, user funds safe). The position is
   credited only AFTER both transfers (lines 941-953), so a failed deposit never credits.
 - Attacker-created pool-owned holding, optionally pre-funded: harmless — the user's `amount` still
   flows to the vault and the position is credited exactly `amount` (the `amount`-consistency: step1,
   step2, and the credit are all the same `amount`); any pre-funding is stranded in the holding
   (attacker self-loss), never an over-credit or inflation.
 - holding == owner_ata (duplicate): contradictory — step 1 needs owner-signable, step 2 needs
   pool-owned; no account is both, so it always fails.
 - holding == percolator_vault: step 1 funds the vault directly and step 2's vault->vault no-op still
   increments the insurance counter by the same `amount` — consistent (counter and balance both +amount;
   and a direct TopUpInsurance to inflate the counter without funding is impossible since only the
   subledger can sign as the pool operator).
No code change / no test: the boundary is the SPL token program's authority check (a runtime
invariant), so any negative test would be tautological. Recorded so future ticks don't re-derive it.

### [STATE] Audit sweep — input-validation, retract/re-back, clearing math, remaining-account smuggling
Iteration with no fresh reachable gap; re-confirmed (so future ticks skip them):
 - `set_reserve` input validation matches `init_book`: it rejects `reserve_den == 0` (twap
   src ~977), so the degenerate "infinite reserve filters every bid -> permanent auction DOS" can't be
   set even via Squads. (cmp_rate cross-multiplies, so no div-by-zero regardless.)
 - retract/re-back tally integrity: retract subtracts the BALLOT's recorded `voted_weight`/principal;
   re-back recomputes a FRESH weight from the current position. A top-up between (which resets
   start_slot, finding AT) just yields a fresh, correct weight — the tally always equals the sum of
   live ballots, no inflation.
 - clearing-math solvency: every filled bid prices at the marginal P* = cm/um and `coin_i =
   floor(usd_i*cm/um) <= C_i` because its rate >= P*; a bid AT P* fully filled sells exactly C_i
   (refund 0). So `total_coin <= escrow` and per-bid refunds sum to the post-burn escrow — no
   over-draw (pinned by the partial-marginal + happy-path tests).
 - remaining-account smuggling: handlers read fixed accounts via sequential next_account_info and
   IGNORE extras; the only conditional trailing reads (execute SEND coin_sink, place_bid eviction
   target, init_book SEND coin_sink) are each pinned (== book.coin_sink / == evicted bid's canonical
   ATA / != coin_escrow) so a smuggled account is rejected, not honored.
 - re-deposit after a full exit is blocked: full withdraw sets `withdrawn=true` and the deposit's
   existing-position branch rejects `p.withdrawn` (findings AR/AR-2), and the position PDA is unique
   per (pool, owner) so no fresh position can be opened either.

### [BLOCKED] AT. Early-squat-then-top-up to inflate vote hold-time (Sybil) — every deposit resets start_slot
Vote weight is `floor(log2(now - start_slot)) * principal`. Probe: a whale deposits 1 atom at genesis
start, lets the age compound for a long time, then tops up a HUGE principal right before voting — if
the top-up did NOT reset `start_slot`, ALL of that late capital would earn the early-join age =
inflated weight (gaming the hold-time multiplier with capital that was only just put at risk). BLOCKED:
`process_insurance_deposit` sets `position.start_slot = Clock::get()?.slot` on EVERY deposit (new or
top-up), so a late top-up's hold-time clock restarts at the top-up slot. (And the retire-then-redeposit
variant is also closed: a full withdraw sets `withdrawn=true`, and the deposit's existing-position
branch rejects `p.withdrawn` — finding AR / AR-2.) Pinned by `top_up_resets_the_position_start_slot`:
deposit 1 atom at slot 100, warp, top up ~2M at slot 1000 -> the position's start_slot reads back 1000
(reset), not 100, so the late capital earns no early-join age. (Aside, observed not a meta vuln: the
real percolator rejects an insurance deposit after a very large slot gap, Custom 0x1b — irrelevant to
the bounded genesis deposit window.) Distinct from the existing single-deposit start_slot check.

### [FIXED] AS. SEND (buyback) sink could be set to the coin_escrow — self-loop strands the buyback
Duplicate-account self-loop. `set_coin_sink` and `init_book` validated only `coin_sink.mint ==
coin_mint`, but the shared COIN escrow (`coin_escrow`) is ALSO a coin-mint account. If a SEND-mode sink
were set to it, `execute`'s SEND step does `spl_transfer(coin_escrow -> coin_sink, total_coin)` =
escrow -> escrow, a no-op: the bought COIN stays in the escrow and is STRANDED forever (a fixed-supply
COIN, and the escrow only ever pays bidders' recorded `coin_refund`), so the DAO's buyback is silently
nullified and the COIN is locked. Not an external-attacker LOF (the sink is Squads-vault-gated +
timelock'd), but a real correctness loss reachable via a buggy proposal-generation tool or a confused
DAO. FIX: both `set_coin_sink` and `init_book` now reject `coin_sink == coin_escrow` in SEND mode (the
other internal token accounts — settlement_usd, holding — are already excluded by the
`mint == coin_mint` check, being collateral-mint). Pinned by `e2e_send_sink_cannot_be_the_coin_escrow`:
flipping to SEND with the sink == coin_escrow is rejected via Squads, while a genuine external treasury
sink is accepted. 57 chain tests green.

### [STATE] Tick constrained by a broken read-only sibling — six-binary e2e unbuildable
The `percolator-prog` sibling (read-only to this repo) is mid-edit and currently fails to compile
(`error[E0308]` in `src/v16_program.rs`, uncommitted). The two e2e harnesses that link percolator's
LIB — `twap-program/tests/chain.rs` (the six-binary suite) and `subledger/tests/insurance_percolator.rs`
— therefore cannot build, so no new percolator-touching attack can be build-verified this tick (and the
deferred AR-2 own-vault-withdraw test waits on it). The STANDALONE real-binary harnesses do NOT link
percolator and were re-run GREEN this tick: `subledger/tests/subledger.rs` (own-vault, 5),
`genesis-vote/tests/seal.rs` (gv + distribution + Squads, 5), `distribution/tests/distribution.rs` (8).
A focused re-probe of those surfaces (gv vote/weight/quorum + live-outstanding trigger; distribution
claim/seal one-shot/burn-window; own-vault deposit/withdraw/pro-rata) found no fresh, non-redundant
vector — they remain exhaustively covered. All four percolator-meta PROGRAMS still `build-sbf` clean
(they don't link the sibling lib). Resume full six-binary probing + the AR-2 test once percolator-prog
compiles again.

### [BLOCKED] AR-2. Phantom-capital vote via the OWN-VAULT withdraw path (finding AR follow-up)
Follow-up to AR: the subledger has a SECOND exit, the own-vault withdraw (`process_withdraw`, IX 2),
which sets `withdrawn=true` and pays out WITHOUT decrementing `position.principal`. If it could run on
the genesis INSURANCE pool, a voter could "exit" via it, leave `principal` intact, and re-vote with
phantom capital (the AR attack, on a path where principal is NOT zeroed). BLOCKED three independent
ways: (a) `process_withdraw` rejects insurance pools up front — `if pool.is_insurance() { return Err }`
(subledger/src/lib.rs:~586, the withdraw twin of the existing
`own_vault_deposit_is_rejected_on_an_insurance_pool` guard); (b) an insurance pool's vault is the
percolator insurance vault, owned by the market `vault_authority`, NOT the pool PDA, so the pool can't
sign its `spl_transfer`; (c) the position is mutated only AFTER the payout transfer, so any failure
reverts the whole instruction (no partial `withdrawn=true`). So the only exit reachable for the genesis
pool remains `process_insurance_withdraw` (IX 5), which decrements `principal` (finding AR). No new test
this tick: the subledger integration tests link the percolator-prog LIB, and that sibling is currently
mid-edit / fails to compile (uncommitted changes, read-only to this repo), so a new e2e test cannot be
build-verified right now. Pinned by `own_vault_withdraw_is_rejected_on_an_insurance_pool`: IX 2 on the genesis insurance position
is rejected and the position is left fully intact (principal unchanged, not retired).

### [BLOCKED] AR. Phantom-capital vote with a withdrawn position (Sybil bypass) — withdraw decrements live principal
Probe: vote weight must reflect capital genuinely at risk. genesis-vote `read_sub_position` reads the
position's `principal` (and start_slot) but does NOT inspect a "withdrawn" flag — so IF a withdrawal
left `principal` intact, a voter could deposit P, back a proposal, retract, WITHDRAW P (capital
returned, pool.outstanding -= P), then back AGAIN and vote with the stale P. Because the quorum
denominator (`read_sub_pool_outstanding`, read live by `trigger`) had dropped, the phantom vote would
be FREE and would shrink the denominator — letting a tiny attacker cycle deposit->vote->withdraw to
capture the genesis with capital they no longer hold. BLOCKED: the live insurance-withdraw
(`process_insurance_withdraw`, IX 5) DECREMENTS `position.principal -= amount` (not just a flag), so a
full exit zeroes the live principal and the re-vote computes `vote_weight(0, age) == 0` -> rejected
("position has no vote weight"); a partial exit leaves only the remaining at-risk principal as weight.
Confirmed end-to-end against the real subledger+genesis-vote binaries by
`cannot_vote_with_a_withdrawn_position`: deposit -> vote -> retract -> full withdraw (principal read
back as 0) -> re-vote REJECTED. (Distinct from `vote_locked_principal_cannot_exit_until_retracted`,
which only pins that you cannot exit WHILE a vote is live; this pins that after a legitimate exit the
returned capital carries no vote weight.) NOTE: my initial read confused two withdraw functions — the
580-650 path only flips `withdrawn`, but it is NOT the live insurance-withdraw the genesis uses (IX 5
-> the principal-decrementing handler).

### [FIXED] AQ. Parasite config on the same market drains the victim's insurance (CRITICAL LOF) — twap_authority was market-scoped, not config-scoped
REAL CRITICAL bug, the deepest layer of finding AD. The `twap_authority` (the percolator insurance
OPERATOR granted by the handoff) was derived from `["market-0-twap", market, percolator_program]` —
NOT bound to the config's squads/coin. So TWO twap configs on the SAME (market, percolator_program),
differing only in squads_multisig/coin_mint, derived the SAME operator PDA. And `execute` computes the
pull as `insurance - config.reserved_floor` using the CALLING config's OWN floor. Attack: (1) the
attacker stands up a PARASITE config-A on the VICTIM's market (`init_config` does not validate the
market's admin, so any squads/coin works); (2) config-A's twap_authority == the victim's operator PDA;
(3) config-A sets ITS OWN `reserved_floor` to 0 via its own Squads; (4) anyone cranks `execute(config-A)`
— percolator only checks the signer is the (shared) operator, so it pays out `insurance * bps` into
config-A's holding. CONFIRMED end-to-end: the parasite drained the victim's insurance 1,500,000 ->
300,000 (BELOW the 1M depositor-principal floor — principal stolen), 1,200,000 landing in the
parasite's holding, which config-A then sweeps via shutdown.
FIX: bind the `twap_authority` seed to the CONFIG PDA — `["market-0-twap", config]` (the config PDA
already commits to market+squads+coin+perc via finding P). Now the operator is UNIQUE per config: only
the single config the handoff actually granted to derives the real operator; a parasite derives a
DISTINCT PDA that percolator does not recognize as the operator, so its `execute` (and any further CPI)
is rejected. Updated `authority_seeds`, `init_config`'s bump derivation, and all four signer sites
(accept_operator, execute, shutdown, init_book's holding-owner check) + every chain.rs derivation. The
legit handoff/execute/buy-burn E2E still passes (grant + execute derive the new seed consistently).
Pinned by `e2e_parasite_config_on_same_market_cannot_drain_insurance` (the confirming drain test,
flipped to assert BLOCKED: parasite authority != operator, execute rejected, victim insurance fully
intact, parasite pulls nothing) and `e2e_twap_authority_seed_binds_to_config_no_operator_reuse`
(anchored to the real grant: operator == config-bound seed, != the old (market,perc) seed, != any
other config's derivation). NOTE: the subledger pool operator is already pool-unique (its seed includes
mint+asset_id+market+perc, finding Q), so it has no analogous parasite.

### [BLOCKED/INVARIANT] AP. init_book escrow `amount == 0` is a latent finding-AI pre-fund surface — safe ONLY via random escrow addresses
`process_init_book` requires the shared COIN escrow and the settlement-USD account to be EMPTY at
init (`ce.amount != 0 -> reject`, `su.amount != 0 -> reject`) for a clean accounting start. Probe: a
transfer to a token account needs no destination signature (cf. finding AI's lamport pre-fund), so if
those escrow addresses were DETERMINISTIC (e.g. canonical ATAs of the `book_escrow` PDA), an attacker
could dust one with 1 token BEFORE the DAO's timelock'd init_book — making `amount != 0` reject it
forever (the balance can't be swept until the book_escrow PDA can sign, which only exists post-init):
a permanent book-init DOS. CURRENTLY BLOCKED: the escrows (and the holding) are RANDOM, DAO-chosen
token-account addresses (`Pubkey::new_unique()` in setup, never a PDA/ATA), so an attacker cannot
predict or pre-fund them. The `holding` has no amount check at all (a pre-funded holding is just extra
starting budget — harmless), so only the two escrows carry the empty-requirement. INVARIANT to
preserve: the escrow/settlement token accounts MUST stay at unpredictable addresses, OR — if a future
refactor makes them deterministic (ATAs) — `init_book` must tolerate a pre-funded balance the same way
`create_pda_robust` tolerates pre-funded lamports (finding AI), rather than reject on `amount != 0`.
No test: the attack is unreachable at random addresses, and asserting "init_book rejects a non-empty
escrow" only pins a DAO-side sanity check (not an attacker path).

### [BLOCKED] AO. Cross-config book mutation — `book.config` pin is LOAD-BEARING (not the squads gate)
Probe: the twap program is generic/reusable, so multiple (config, book) pairs for DIFFERENT
markets/DAOs can coexist. A malicious DAO that controls config-A's Squads tries to grief config-B's
auction by calling a Squads-gated book mutator (`set_reserve` / `set_bid_fee` / `set_coin_sink`) with
config-A (their squads SIGNS) + config-B's BOOK — setting a hostile reserve/fee or retargeting the
COIN sink on the victim's auction. Key insight: `require_squads_vault(config)` does NOT stop this — it
passes for the attacker's OWN config-A. The only thing that blocks it is the explicit
`book.config == config_account.key` check, which every book mutator performs right after loading the
header (`set_reserve` twap-program/src/lib.rs:~966, `set_bid_fee` ~1027, `set_coin_sink` ~1000); a
mismatched book is rejected. `shutdown` + `set_reserved_floor` take no book and are config-scoped (the
holding is owned by the config's twap_authority; `set_reserved_floor` writes only `config`). VERDICT:
BLOCKED — the `book.config` pin is load-bearing and uniformly present, so cross-config griefing is
impossible. NOW PINNED end-to-end by `e2e_config_a_cannot_mutate_config_bs_book_reserve`: it stands up
a SECOND independent twap config (config-A: its own Squads multisig + market + coin + init_config under
an attacker DAO), then has config-A's Squads vault authorize a hostile `set_reserve` (rate 999/1,
which would block every real bid) against config-B's BOOK. The inner set_reserve is rejected on
`book.config != config_account` (require_squads_vault PASSES for config-A's own vault — proving the
pin, not the squads gate, is the boundary), config-B's reserve is untouched, and the positive control
(config-B setting its OWN book reserve) still works. The `book.config` checks must never be removed as
"redundant" — they are the sole cross-config boundary.

### [COVERAGE] AN. cancel_bid CLEARED-path release (anti-permanent-lock liveness) — distinct branch pinned
The cancel_bid cooldown opens on `cleared` (an execute moved `round_end` since placement) OR `aged`
(2*round_length elapsed). The aged branch is covered (`e2e_bid_cancellable_after_cooldown_keeps_fee`);
the `cleared` branch — the PRIMARY "one round of twap clears the book" release — was untested. It is a
liveness/DOS boundary: a regression breaking `cleared` would lock a committed bidder's COIN after a
roll (an execute that buys nothing) until 2*round_length, yet still pass the aged-path test. Also a
spoof-relevant edge (the anti-spoof design relies on cancel being gated until the bid has been exposed
to at least one round). Pinned by `e2e_roll_opens_the_cleared_cancel_path`, which isolates the branch
rigorously: at the SAME slot E1 (= round_end) cancel is REJECTED before the roll (cleared=false +
aged=false) and ALLOWED immediately after the roll-execute (cleared=true, aged STILL false, since
2*round_length has not elapsed) — so the only thing that opened cancel is the roll advancing
round_end, and the bidder reclaims their full escrowed COIN. (A bid present at a REAL settlement is
marked SETTLED and uncancellable, so this release never lets a spoofer escape a purchase — see AE +
e2e_cancel_cannot_double_spend_a_settled_bid.)

### [BLOCKED] AM. Closed-ATA poison bid on the EVICTION path — uncensorability stays recoverable
Finding V pinned the canonical-ATA recoverability on the CLAIM path
(`e2e_closing_refund_ata_cannot_permanently_brick_the_book`), but the place_bid EVICTION path is a
distinct code path that was untested for a closed refund ATA. Probe: in a FULL 32-slot book the
weakest bidder closes their canonical coin ATA; a strictly-better bid must evict the weakest and
refund it there. The eviction's `spl_transfer` to the closed ATA fails, so the better bid is
temporarily blocked — a griefing attempt against the core uncensorable-bid guarantee. BLOCKED /
self-healing: the refund target is the weakest bid's CANONICAL ATA (finding V), which anyone can
recreate permissionlessly; after recreation the eviction succeeds and refunds. Not a permanent brick,
and the poison bid is exposed to consumption/clearing at the next execute regardless. Pinned by
`e2e_closed_weakest_ata_cannot_permanently_block_eviction`: eviction blocked while the ATA is closed
(better bid's COIN un-escrowed, funds intact), then succeeds + refunds once the ATA is recreated. A
regression pointing the eviction refund away from the canonical ATA would be caught here but NOT by
finding V's claim-path test.

### [STATE] Audit sweep — handoff gating, claim/burn window edge, execute arithmetic all confirmed covered
Iteration with no new vector (all probed surfaces BLOCKED + already pinned); recorded so future ticks
skip them:
 - `accept_operator` (the percolator insurance-operator handoff) is Squads-vault-gated
   (`squads_vault.is_signer` + `== squads_default_vault(config.squads_multisig)`), pins
   market/program/authority to the config, grants to the finding-AD-bound twap_authority, and rotates
   the deposit authority to the vault (finding S). A non-vault signer is rejected — already pinned by
   the negative case in `e2e_dao_reconfigures_twap_only_after_timelock` (imposter `IX_ACCEPT_OPERATOR`
   rejected) and the positive handoff e2e.
 - distribution claim/burn window boundary is race-free and FULLY pinned at the exact slot by
   `unclaimed_is_burned_after_window`: at `slot == window_end`, `claim` is rejected (`>=`) AND
   `burn_unclaimed` is allowed (`< window_end` false) — claims close exactly as burns open, so a
   boundary-slot claim can never be front-run by the burn (no LOF), and only UNCLAIMED amounts burn.
 - `execute` surplus/burnable/ratchet math is fully checked (`saturating_sub` + `checked_mul`/`_sub`/
   `_add`); the ratchet invariant `reserved_floor <= insurance` holds even when percolator caps the
   actual pull below `burnable` (the unpulled remainder rolls to next round; no principal loss).
 - finding S (post-handoff deposits drainable as surplus) is pinned end-to-end by
   `e2e_post_handoff_deposit_blocked_by_authority_revoke` + `e2e_subledger_exit_blocked_after_operator_handoff`.

### [BLOCKED] AL. Substituted percolator_vault token account at execute — relies on the percolator boundary
`execute` pins the vault_AUTHORITY (`vault_authority == perc_vault_authority(market_slab,
percolator_program)`, covered by `e2e_execute_rejects_foreign_market_vault_authority`) but hands the
`percolator_vault` TOKEN account straight into `WithdrawInsuranceLimited` WITHOUT pinning it to the
canonical address — it trusts the percolator CPI to validate the vault. Probe: a permissionless
cranker substitutes a DIFFERENT token account owned by the REAL vault_authority (a bait the attacker
funds) as `percolator_vault`, trying to redirect the pull (drain a wrong account / desync percolator's
insurance accounting against the real vault). BLOCKED: the real percolator binary validates the vault
is the market's canonical insurance vault and rejects the substitution; `execute` fails and BOTH the
real insurance vault and the bait are untouched, while the honest (canonical-vault) execute still
pulls the burn-share. Pinned by `e2e_execute_rejects_substituted_percolator_vault` — a distinct
account from the vault-authority test (the token account vs its authority); arithmetic in the
surplus/burnable/ratchet path is also fully checked (checked_mul/sub/add + saturating_sub, and
reserved_floor <= insurance holds). Verdict: the twap correctly leans on percolator's own vault check;
no twap-side pin needed.

### [BLOCKED] AK. Deposit with a foreign market routes capital away while crediting vote weight (Sybil)
The Sybil-resistance invariant is that vote weight must be backed by capital genuinely at risk in the
GENESIS market. `insurance_deposit` credits `position.principal` (which becomes vote weight) and CPIs
`TopUpInsurance` to move the capital. Probe: pass a FOREIGN `market_slab` (a market the attacker
controls / can reclaim from) while depositing to the genesis pool — getting a credited position
without leaving capital truly at risk = free governance power, defeating the whole bootstrap.
BLOCKED: deposit pins `market_slab == pool.market_slab`, `percolator_vault == pool.vault`,
`percolator_program == pool.percolator_program` (and re-derives the pool PDA, finding Q), so the
TopUp can only ever go to the pool's bound market; a foreign slab is rejected before any position is
created. (Voting is doubly bound: `genesis-vote::vote` also pins `sub_pool == config.subledger_pool`,
so even a fully-legit position in a DIFFERENT pool carries no genesis weight.) Pinned by
`deposit_with_foreign_market_slab_credits_no_position`: a foreign-slab deposit is rejected, no
position is created, outstanding stays 0, the attacker's capital is untouched. Distinct code path from
the withdraw foreign-slab pin (finding AF) — a regression dropping the deposit pin would not be caught
by AF. Previously untested.

### [BLOCKED] AJ. Re-init of a live account (regression guard for finding AI) — state-reset LOF
The finding-AI fix relaxed every init guard from `lamports() != 0 || data_len() != 0` to
`data_len() != 0` (so a dusted-but-empty PDA can still be created). Probe: does that weaken re-init
protection? If a SECOND init on an already-initialized PDA succeeded, an attacker could re-init a LIVE
subledger pool and reset `outstanding_principal` (the genesis quorum denominator) to 0 — instantly
collapsing the quorum threshold so a tiny minority captures the distribution — or re-point the pool's
vault/policy. BLOCKED: an initialized account always carries data, so `data_len() != 0` still rejects
the re-init (the relaxation only ignores lamports, which never indicate initialization). Confirmed by
`insurance_pool_cannot_be_reinitialized_after_funding`: a funded pool (1M outstanding) is re-init'd on
the same PDA → rejected, outstanding untouched. This re-init boundary was previously UNTESTED anywhere
in the stack; the test doubles as the regression guard for the finding-AI guard change.

### [FIXED] AI. Lamport pre-fund permanently bricks every init (whole stack) — cheap griefing DOS
REAL bug (HIGH), found via the PDA-creation discussion. Every init handler creates its PDA with the
System `create_account`, which aborts with `AccountAlreadyInUse` (Custom(0)) on ANY pre-existing
lamports — AND the handlers additionally guard `if lamports() != 0 -> AlreadyInitialized`. A PDA
address is deterministic and public; a plain `transfer` to it needs NO signature. So an attacker
sends 1 lamport to the address BEFORE the legit init and that init can NEVER succeed: the guard (and
then create_account) reject it forever, and the lamports can't be swept (no one can sign for a
system-owned PDA). Confirmed end-to-end against the real subledger binary
(`lamport_prefund_cannot_brick_insurance_pool_init`): dusting the pool PDA made init fail first with
the handler's own `AccountAlreadyInitialized`, and after relaxing that guard, with the System
program's "account ... already in use" — BOTH causes proven empirically. Impact is critical on the
genesis path: dusting the deterministic subledger POOL PDA bricks the whole bootstrap, and dusting a
depositor's deterministic POSITION PDA bricks that specific user's first deposit (targeted LOF/DOS).
FIX: `create_pda_robust` — reject re-init by `data_len() != 0` (NOT lamports), then top up the rent
shortfall with a plain `transfer` and `allocate` + `assign` via invoke_signed; allocate/assign only
require the account to be data-empty + system-owned, both true for a merely pre-funded address, so the
dust is absorbed instead of fatal. Applied STACK-WIDE to every PDA-creating init handler: subledger (insurance +
own-vault pool inits, both position inits), twap (config + book), genesis-vote (config + proposal +
ballot, via its shared `create_pda` helper made robust), distribution (config + proposal). Each gates
re-init on `data_len() != 0` and creates via the robust top-up + allocate + assign. The confirming
test `lamport_prefund_cannot_brick_insurance_pool_init` passes (init succeeds despite the dust); all
existing init tests across the four programs stay green (the robust create is behavior-compatible on a
zero-lamport account). Also fixed a pre-existing stale `dist_config` derivation in
genesis-vote/tests/seal.rs (finding-AA follow-up: the seal authority must be folded into the seed).
The twap BOOK path (distinct: Squads-gated, squads_vault as the robust-create payer) is pinned
end-to-end by `e2e_lamport_prefund_cannot_brick_book_init`: dusting the deterministic book PDA before
the timelock'd init_book no longer bricks it — the book is created and the auction clears + burns
normally (proving no corrupted half-init).

### [VERIFIED] AH. Buyback-vs-burn is DAO-controllable ONLY via Squads (twap) — not via permissionless init
Design confirmation prompted by the futarchy->Squads->twap authority arm: the buy/burn SINK MODE
(burn vs buyback-to-treasury) and its destination must be settable AND changeable by the futarchy
acting through Squads, but must NEVER ride on the permissionless `init_config` (a front-runner could
squat the config PDA and route bought COIN to an attacker account — the not-in-seed-field squat from
finding P). Verified the code already enforces exactly this: `init_book` and `set_coin_sink` both call
`require_squads_vault` (the Squads default vault must sign, behind the 1-week timelock), and
`set_coin_sink` flips `sink_mode` burn<->send (data[0]) + validates the SEND destination's mint (or
clears it on BURN). So the SEND destination is only ever DAO-set; a squatted `init_config` carries no
sink at all. The init-time SEND routing was pinned (`e2e_send_mode_routes_bought_coin_to_treasury_not_attacker`);
the CHANGE path was not. Added `e2e_dao_flips_burn_to_buyback_only_via_squads`: an auction starts in
BURN mode, a forged non-Squads `set_coin_sink` is rejected, the DAO flips BURN->SEND via a timelock'd
Squads execute, and the next `execute` routes the 400k bought COIN to the treasury (supply unchanged —
buyback does NOT burn). Pins that monetary policy is DAO-tunable through Squads and only through Squads.

### [BLOCKED] AG. Winner re-seal / winner-take-all override (distribution + genesis-vote)
Probe: two genesis proposals compete; A reaches a weighted majority and is sealed (COIN becomes
claimable to A's recipients). Later, vote shifts (retract from A, back B) push B over the majority.
Can B's permissionless `trigger` re-seal the distribution to B — changing the winner after A's COIN
is already claimable (double-distribution / theft of the COIN supply)? BLOCKED at two independent
points: (1) `distribution::seal_winner` is ONE-SHOT — `if config.is_sealed() return Err` (it records
`sealed_proposal` + `seal_slot` on first seal and refuses any later seal), and `claim` only pays
`config.sealed_proposal`; (2) `genesis-vote::trigger` sets `pv.executed = true` and refuses to act on
an already-executed proposal, and its seal CPI is signed by the gv config as the distribution's bound
authority. So the FIRST proposal to reach quorum+majority wins immutably; a later majority swing
cannot override it (B's trigger fails at the seal CPI). Note this is a deliberate
first-past-the-post: quorum + strict-majority can be met by at most one proposal at a time (they share
`total_cast_weight`), and the winner is locked the instant it seals. No new test — the one-shot guard
is explicit and the property is an extension of the existing winner-take-all/no-new-proposal coverage
(`only_the_winning_proposals_recipients_can_claim`, `no new proposal after finalize`).

### [BLOCKED] AF. Cross-market haircut-basis substitution (subledger) — pro-rata exit reads a pinned slab
Probe: a depositor in an IMPAIRED genesis pool tries to inflate their pro-rata exit by passing a
DIFFERENT, HEALTHY market's slab as `market_slab`. `withdraw`'s pro-rata haircut reads the live
insurance BASIS straight from `market_slab` (findings L + T, offset 749); if that read could be
pointed at an un-impaired market, `payout()` would see `insurance >= outstanding`, treat the exit as
healthy, and pay FULL principal while the actual `WithdrawInsuranceLimited` pull still drains the
real (impaired) market — stealing the loss-share owed to the depositors who stay. BLOCKED:
`process_*_withdraw` pins `market_slab.key == pool.market_slab` (and `percolator_vault == pool.vault`,
`percolator_program == pool.percolator_program`, `vault_authority == perc_vault_authority(market_slab,
perc)`) BEFORE reading insurance or signing the pull, so the haircut basis and the pull source are the
SAME pinned market. The defense existed but had no negative test on the subledger side (the twap had
the symmetric `e2e_execute_rejects_foreign_market_vault_authority`). Pinned by
`foreign_market_slab_cannot_inflate_the_haircut`: alice (1M) in a market impaired to 50% points
`market_slab` at a cloned HEALTHY slab (2M insurance) — rejected, position untouched, 0 extracted —
then her honest exit on the real slab pays exactly the 500k haircut, proving the foreign basis bought
no advantage.

Broad audit this tick (all BLOCKED, no new bug): distribution `claim` double-spend (guarded by
`amount==0 -> already claimed` + zero-after-transfer + pull-model recipient-only + claim-window) and
`burn_unclaimed` early-burn grief (window-gated, `clock < window_end` rejected); genesis-vote `vote`
fabricated-weight via a fake position (guarded by `sub_position.owner == config.subledger_program` +
canonical-PDA-for-(pool,voter) + disc, so neither an attacker-owned forgery nor someone else's real
position works); twap Config/AuctionBook type-cosplay (distinct discriminators TWAPCFG1/TWAPBOK1
checked at deserialize) and `place_bid` mint-confusion (coin_mint/collateral_mint/coin_escrow all
pinned to the book, src/dest mints re-checked). Discriminator hygiene is uniform across all four
programs (SUBPOOL1/SUBPOS01, GVCONFG1/GVBALOT1/GVPROPV1, DISTCFG1/DISTPRP1, TWAPCFG1/TWAPBOK1).

### [HARDENED] AE. Roll-undo left SL_COIN_REFUND stale (twap-program) — latent, non-exploitable state-restore gap
`execute` clears the book against the pulled budget. When NOTHING is bought (`total_coin == 0` — a
"roll": surplus below the floor so budget 0, OR a budget so small every marginal fill rounds to zero
COIN atoms) the round is NOT settled: bids stay committed and `round_end` advances. The roll-undo
loop restored each occupied slot's `SL_USD_OWED` and `SL_SETTLED` — but NOT `SL_COIN_REFUND`, which
the settlement loop may already have written (= full escrow) on every slot when a marginal bid was
selected yet all `coin_i` rounded to 0. So a rolled bid could carry a stale `SL_COIN_REFUND` into the
next round. ANALYSIS: not exploitable today — `SL_COIN_REFUND` is read only by `claim`, which requires
`SETTLED == 1` (reset to 0 by the roll); `cancel`/`evict` read `SL_COIN`, not `SL_COIN_REFUND`; and
the next REAL settlement overwrites `SL_COIN_REFUND` for every occupied slot before it is ever read.
But relying on "overwritten-before-read" is fragile and violates the invariant that a roll fully
restores each bid to its pre-execute bytes. HARDENED: the roll-undo now also zeroes `SL_COIN_REFUND`,
so a rolled bid is byte-identical to its pre-execute self for the subsequent cancel/evict/settle
paths. Pinned by `e2e_roll_with_committed_bid_settles_correctly_next_round`: a 400k/400k bid rides
through a budget-0 roll (insurance dropped below the floor — assert nothing pulled, no COIN burned,
the unsettled bid is NOT claimable) and then settles byte-exactly next round once surplus is restored
(full 400k COIN burned, 0 refund, full USD paid). The pre-existing below-floor roll test
(`e2e_execute_pulls_nothing_when_insurance_below_floor`) has NO bid in the book, so the
roll->survive->settle path was previously untested. (The exact stale-refund line needs a
marginal-set-but-zero-COIN budget, a contrived dust case left to the analysis above since it is
provably overwritten-before-read; the e2e pins the realistic roll->settle correctness boundary.)

### [FIXED] AD. twap_authority signer seed not bound to its caller-configurable CPI target (twap-program) — confused-deputy insurance drain
The twap_authority PDA is the percolator insurance OPERATOR (granted by the handoff) and signs
`WithdrawInsuranceLimited` (in `execute`) and the operator-accept (in `accept_operator`) into
`config.percolator_program`. But `percolator_program` is CALLER-CONFIGURABLE — every config carries
its own — while the signer seed was `["market-0-twap", market]`, COARSER than the config seed
`["twap_config", market, squads, coin_mint, percolator_program]`. So all configs for the same market
but different percolator programs shared ONE twap_authority = the real market's operator. `init_config`
does NOT bind `market_slab.owner == percolator_program` (the twap is deliberately program-agnostic), so
an attacker could: (1) init a SECOND config at its own PDA with `market_slab = the REAL market` but
`percolator_program = a program THEY deploy`; (2) `execute` it — the twap derives twap_authority from
`config.market_slab` only, getting the REAL operator PDA, and invoke_signed's into the attacker program;
(3) the attacker program receives the operator's signature and re-CPIs the REAL percolator's
`WithdrawInsuranceLimited` (signer privilege propagates through CPI), draining the real insurance to
itself. The per-call `percolator_program == config.percolator_program` check is USELESS here because the
malicious config legitimately stores the malicious program. This is the classic "a PDA that signs into a
caller-configurable program must commit to that program in its seeds" footgun.
FIX: fold percolator_program into the signer seed — `authority_seeds(market, percolator_program) =
["market-0-twap", market, percolator_program]`. Now each config derives a DISTINCT twap_authority; a
foreign-program config gets a powerless PDA the real percolator never granted operator, and `execute`
through it signs as a non-operator that percolator rejects. Same defense the subledger pool authority
already uses (finding Q folds percolator_program into `pool_seeds`). Updated the helper, `init_config`'s
bump derivation, and all four signer sites (accept_operator, execute, shutdown's holding sweep, +1);
the legit handoff/execute/buy-burn E2E still passes because the grant and `execute` derive the new seed
consistently. Pinned by `e2e_twap_authority_seed_binds_percolator_program_no_operator_reuse`, anchored
to the REAL on-chain grant: setup_handoff rotates the percolator operator to env.twap_authority, and the
test asserts that == the perc-bound seed, != the old unbound seed, and != any foreign-perc derivation.

STACK-WIDE AUDIT (does this footgun exist anywhere else? — answer: NO). Every other PDA that signs into
a program whose id comes from account/config data either pins the target to a compile-time CONST or
folds the caller-configurable program into its own signer seed:
 - subledger: signs into `pool.percolator_program`, but `pool_seeds` INCLUDES percolator_program
   (finding Q) AND each handler re-checks `== pool.percolator_program`. Bound. SAFE.
 - genesis-vote `vote`/`trigger`: sign into `config.subledger_program` / `config.distribution_program`,
   but the `gv_config` signer seed `["gv_config", coin_mint, subledger_pool]` IS the config seed (1:1,
   never shared across configs) AND `trigger` checks `== config.distribution_program`. SAFE.
 - distribution `claim`/`burn_unclaimed`: sign into the token program, validated `== spl_token::ID`
   (compile-time const). SAFE.
 - program/src: `verify_percolator_program` pins percolator to `percolator_abi::id()` (const). SAFE.
twap was the lone outlier solely because its signer was coarser than its config; with finding AD it now
matches the rest of the stack.

### [FIXED] V. Refund-ATA brick → permanent auction DOS (twap-program) — SOL-class: account-closure griefing
The auction's COIN refund (claim / cancel / eviction) was delivered to the bid's stored `coin_ata`,
which `place_bid` set to the bidder's *arbitrary* funding source. A losing bidder could place a bid
and then CLOSE that account — after which `claim` could never deliver the refund (`spl transfer` to a
closed account aborts), the slot could never free, and the book stayed `SETTLED` forever, blocking
all future `execute` and `place_bid` — a PERMANENT DOS of the whole buy/burn, at the cost of only the
attacker's own (forfeited) stake. An arbitrary account address, once closed, cannot be permissionlessly
recreated, so the brick was permanent.
FIX: pin the COIN refund target to the bidder's CANONICAL ATA (`bidder_coin_ata` =
ATA(bidder, coin_mint)) instead of the caller-supplied source. Anyone can recreate an ATA, so a stuck
claim is always recoverable — closing it is a temporary, self-healing nuisance, not a permanent brick.
Pinned: `e2e_closing_refund_ata_cannot_permanently_brick_the_book` drives the attack end-to-end
against the real binaries (loser closes the ATA → claim fails + book blocked → recreate the ATA →
claim succeeds + refund delivered + book reopens). Probe-loop iteration 1.

### [BLOCKED] W. Malicious-DAO can't drain escrow/settlement via `shutdown` (twap-program) — probe #3
`shutdown` is a Squads-gated privileged op that sweeps the twap's USD budget (the `holding`) to a
DAO-supplied address. Attack: the DAO substitutes the book-escrow-owned `coin_escrow` (bidders'
escrowed COIN) or `settlement_usd` (winners' settled USD) as the "holding" to drain user funds to
itself. BLOCKED: `shutdown` requires `holding.owner == twap_authority`; the escrow/settlement
accounts are owned by the `book_escrow` PDA, so the substitution is rejected before any transfer.
So even a hostile DAO can't repurpose `shutdown` to steal bidders'/winners' funds — `shutdown` is
scoped strictly to the twap's own (DAO-owned) budget. Pinned end-to-end against the real binaries by
`e2e_shutdown_cannot_drain_escrow_or_settlement` (both substitutions rejected, funds intact).

### [BLOCKED] X. Reserve price blocks surplus-drain via an "expensive" bid (twap-program) — probe #4
The auction sells surplus USD for COIN at a uniform marginal price. WITHOUT a reserve, a hostile
bidder can offer ~1 COIN for the WHOLE surplus (rate ~0), become the marginal/only fill, and drain
the insurance surplus for almost no COIN burned — a real economic LOF. GUARD: the DAO-set reserve
(`reserve_num/reserve_den` = min COIN-per-USD); `execute` filters every bid with rate < reserve
(`cmp_rate(c,u,reserve_num,reserve_den) == Less → skip`). The reserve was previously UNTESTED (every
test used (0,1) = accept-all). Now pinned by `e2e_reserve_blocks_expensive_bid_from_draining_surplus`
(+ new `build_set_reserve_message`): with reserve 1/1, a 1-COIN-for-400k-USD bid is filtered — no
COIN burned, no USD paid, surplus preserved — while a fair (>= reserve) bid still clears + burns.
HARDENING NOTE: `reserve = (0,1)` is accept-all (no protection); a real deployment's DAO MUST set a
meaningful reserve (like it must set `reserved_floor`). A 0 reserve is a footgun, not a code bug.

### [BLOCKED] Y. execute can't drain a FOREIGN market's insurance (twap-program) — probe #5
execute is the sole insurance puller. Attack: a cranker points it at a DIFFERENT percolator
market's vault/authority to drain that market's insurance into this twap. BLOCKED: execute pins
`market_slab == config.market_slab` AND `vault_authority == perc_vault_authority(market_slab)`, so a
foreign vault_authority is rejected (InvalidSeeds) before the CPI — and percolator independently
binds the vault to the market. (This boundary lost its test when the standalone pull_surplus tests
were removed; re-pinned on the execute path.) Pinned: `e2e_execute_rejects_foreign_market_vault_authority`
(foreign vault_authority rejected, insurance intact; honest execute pulls).
Also analyzed (probe #5, ACCEPTED-RISK, no test): distribution `init_config` stores an ARBITRARY
`authority` and the config PDA = ["dist_config", coin_mint] is not authority-bound (cf finding P).
A front-runner could squat it with `total_supply=0` (the solvency check `vault >= total_supply`
forbids promising COIN it doesn't hold, so NO theft of the genesis COIN is possible) — purely a
setup-time DOS that forces a fresh coin_mint. Mitigation: the deployer mints + inits the dist config
atomically. Not fixed (benign, pre-genesis, recoverable); recorded so it isn't re-investigated.

### [BLOCKED] Z. Distribution bait-and-switch — redirect COIN after voters approve (genesis-vote/distribution) — probe #6
A proposal CREATOR registers a community distribution, lets voters approve it, then APPENDS a new
entry redirecting COIN to themselves before `trigger`. Defense in depth, both confirmed:
(1) `append_entries` enforces `total_amount <= total_supply` (line 422) — can't over-promise beyond
the funded supply; (2) the gv proposal snapshots `(entry_count, total_amount)` at registration, and
`trigger` refuses to seal if the live distribution proposal no longer matches (the snapshot check) —
so even a WITHIN-supply redirect can't be sealed. The tampered proposal becomes permanently
un-sealable; the attacker can grief their OWN proposal (it dies) but can NEVER redirect funds — the
sealed distribution is exactly what voters approved. Both guards were untested; now pinned end-to-end
(all 4 genesis binaries) by `e2e_bait_and_switch_appended_entries_cannot_be_sealed` (community entry
50/100, redirect 50 appends within supply, trigger rejected, dist config left unsealed).

### [FIXED] AA. Distribution init front-run → THEFT of the entire COIN supply (distribution) — probe #7
HIGH severity. The dist config PDA was `["dist_config", coin_mint]` (deterministic, authority NOT in
the seed). The funded vault is owned by that deterministic PDA. So an attacker who front-runs
`init_config` AFTER the deployer funds the vault could init the config with **authority=themselves**
and pass the deployer's already-funded vault (all guards pass: vault owned by the PDA, mint.supply ==
total_supply, mint revoked, solvency) — then seal a self-dealing proposal (signing as authority) and
CLAIM the entire COIN supply. (My probe-#5 note that this was "DOS-only" was WRONG: using the funded
vault makes it a theft.) This is the finding-P/Q/R class (init front-run) but UNBOUND here and the
highest impact — the genesis COIN IS the MetaDAO.
FIX: bind `authority` into the config PDA seed — `["dist_config", coin_mint, authority]`. An
attacker's authority now derives a DIFFERENT PDA whose vault they must own + fund themselves
(impossible without the COIN), so the legit (authority = gv config PDA) config + funded vault are
untouchable. Updated all 4 program sites (config_seeds + the create_account + claim/burn_unclaimed
CPI signer seeds) and every PDA derivation in the tests. Pinned by
`init_config_authority_bound_blocks_funded_vault_hijack` (attacker's init over the legit funded vault
rejected, supply intact; legit init succeeds). All suites green (dist 4+8, chain 39 incl. the full
genesis→buy-burn E2E which now uses the bound derivation).

### [FIXED] AB. USD-side refund brick (twap-program) — finding-V extension, probe #8
Finding V pinned the COIN refund to the bidder's canonical ATA, but the WINNER's USD payout target
(`usd_dest`) was still stored from an ARBITRARY caller account. So a winner could close their
`usd_dest` after bidding → `claim`'s USD transfer aborts forever → the slot never frees → the book
stays SETTLED → permanent DOS (same class as V, missed on the USD side). FIX: `place_bid` now stores
BOTH payout targets as the bidder's CANONICAL ATAs (USD = `ATA(bidder, collateral_mint)`, COIN refund
= `ATA(bidder, coin_mint)`), so closing either is permissionlessly recoverable (recreate the ATA →
claim → reopen), not a brick. Pinned: `e2e_closing_usd_dest_cannot_permanently_brick_the_book`
(winner closes the USD ATA → claim fails + book blocked → recreate → claim succeeds + 400k USD
delivered + book reopens). All suites green (chain 40, lib 4).

### [FIXED] AC. Compute-budget DOS via O(N²) Euclidean bid ranking (twap-program) — probe #9
HIGH-value DOS, CONFIRMED. The auction ranks bids with an O(N²) insertion sort, and bid-vs-bid
comparison used `cmp_rate` — a continued-fraction (Euclidean) loop whose length grows with the
operands' continued-fraction expansion. `usdc_atoms` was also UNBOUNDED (parsed as u128). A hostile
bidder who fills all 32 slots with close, long-CF (Fibonacci-ratio) rates makes `execute` exceed the
1.4M compute budget — `ProgramFailedToComplete` — a PERMANENT buy/burn DOS (execute always fails, and
a committed book can't be cleared except by waiting out the cancel cooldown). Confirmed empirically:
a 32-Fibonacci-bid book made execute FAIL before the fix.
FIX: (1) bound BOTH legs to u64 at place_bid (`as_u64(usdc_atoms)`); (2) rank bid-vs-bid with a
CONSTANT-TIME cross-multiply `cmp_bid` = `(coin_a*usdc_b).cmp(coin_b*usdc_a)` (u64*u64 < 2^128, exact,
no loop) in both the eviction scan and the execute sort. `cmp_rate` (Euclidean) is kept only for the
O(N) bid-vs-reserve check (reserve may be a large u128). After the fix the same worst-case full book
clears in **278K CU** (was: budget-exhaustion failure). Pinned:
`e2e_full_book_of_worst_case_rates_cannot_dos_execute` (32 Fibonacci bids → execute succeeds at
< 500K CU; a u128-huge usdc_atoms bid is rejected).

### [DESIGN] U. Buy/burn uniform-price (Dutch) auction — invariants (twap-program)
The COIN buy/burn settlement is a permissionless, time-boxed uniform-price auction (twap-program
tags 5-11). Security properties, each pinned by a chain.rs e2e against the real binaries:
- **Anti-spoofing**: a placed bid CANNOT be cancelled — there is no withdraw instruction; the only
  way a bid leaves the book before `execute` is eviction by a STRICTLY better bid (which refunds the
  evictee). A bidder cannot yank a bid right before the auction runs, nor stack a second bid. This is
  the deliberate fix vs the `twap/` library's `withdraw_bid`/`close_bid_escrow` (left UNUSED on-chain).
  Pinned: `e2e_bid_cannot_be_cancelled_only_evicted_by_a_better_bid`.
- **Uniform marginal clearing**: bids ranked by COIN-per-USD (overflow-safe continued-fraction
  comparator), filled best-first until the budget is spent; EVERY filled bid clears at the marginal
  (lowest-accepted) rate P*, so better bidders give less COIN than offered (surplus refunded). A
  DAO-set **reserve rate** caps the price the protocol will pay, bounding the marginal-bid
  manipulation where a whale's huge expensive bid drags P* down. Pinned:
  `e2e_buy_burn_uniform_price_dutch_auction` (asserts the real COIN mint supply drops by the bought
  amount — an actual burn — and both winners pay the SAME P*).
- **`execute` is the SOLE insurance puller**: the standalone `pull_surplus` was removed. `execute`
  (permissionless, gated on round expiry) pulls only `surplus * buy_burn_bps` (default 80%) as the
  budget and **ratchets the retained share into the principal counter** (`reserved_floor +=`), so the
  retained 20% stays in insurance, is reclassified as protected principal, and compounds — a bare
  pull that skips the ratchet can no longer exist. Finding O's floor (slab read at offset 749, finding
  T) lives here now. Pinned: `e2e_execute_pulls_only_burn_share_and_ratchets_principal`.
- **Permissionless place/execute/claim**; **futarchy-configurable COIN sink** (burn default OR send to
  an account, Squads-gated); **DAO shutdown** sweeps the TWAP's accumulated USD to a supplied address
  (Squads-gated only). Pinned: `e2e_shutdown_sweeps_holding_only_via_squads`.
  - SEND-mode coin sink (probe #2): in SEND mode `execute` transfers the bought COIN to the
    DAO-configured `book.coin_sink` (not burned). The sink is PINNED — a cranker passing any other
    coin account is rejected (`*coin_sink.key != book.coin_sink`), so the bought COIN can't be
    redirected to an attacker (LOF blocked). The SEND branch was previously untested; now covered +
    the redirection attack pinned by `e2e_send_mode_routes_bought_coin_to_treasury_not_attacker`.
- COIN escrow is pooled in ONE book-escrow account so `execute` burns/pays in O(1) CPIs regardless of
  bid count; the book is a fixed 32-slot array with O(n) worst-bid eviction; the canonical USD
  `holding` is pinned in the book so the rolled-over budget can't be fragmented.
- **No cancel/claim double-spend** (probe #14): `cancel_bid` refunds the FULL escrowed COIN while
  `claim` pays the settled `usd_owed` + `coin_refund`; if a SETTLED bid could also be cancelled, the
  bidder would get the full COIN back AND their settled payout (escrow double-spend). `cancel` rejects
  any settled slot (`settled != 0`), early, before the cooldown. Pinned by
  `e2e_cancel_cannot_double_spend_a_settled_bid` (cancel of a settled loser slot rejected, escrow
  untouched; the loser's COIN is refunded exactly once via claim, escrow drains exactly).
- **Permissionless-claim anti-theft** (probe #13): `claim` is permissionless (any cranker turns it),
  so the only guard against a cranker redirecting a winner's USD/COIN to itself is that `usd_dest` /
  `coin_ata` must equal the bid's recorded canonical destinations. Pinned by
  `e2e_claim_cannot_redirect_a_winners_payout` (cranker claims a winner's slot with its OWN usd_dest →
  rejected, settlement_usd intact; honest claim pays the winner). Also verified the subledger insurance
  pool's `vault` is pinned to the CANONICAL percolator insurance vault at init (finding Q + F-VAULT-FRAG),
  so deposits can't be redirected.
- **Uncensorability / eviction** (probe #11): once the 32-slot book is full, a NOT-strictly-better
  bid is rejected (spam can't displace real bids), but a strictly-better bid always gets in — it
  evicts the weakest and refunds that bidder (to the canonical ATA, finding V). Previously untested;
  now pinned by `e2e_full_book_evicts_only_for_a_strictly_better_bid` (full book; equal-rate spam
  rejected + not escrowed; rate-50 bid evicts the rate-1 weakest, refunds 1 COIN, escrow swaps).
- **Anti-spam fee**: a DAO-set flat per-bid COIN fee (default 0.002 COIN) is BURNED on every
  place_bid — non-refundable even on eviction OR cancel, so flooding the 32-slot book has a real
  cost. Pinned: `e2e_bid_fee_is_charged_and_burned`.
- **Cancel with cooldown (no race)**: an unsettled bid is reclaimable by its owner only AFTER an
  execute has cleared the book once (round_end moved) OR 2*round_length slots elapsed — so a bid
  can't be yanked at the last second to manipulate the pending execute, yet funds aren't locked
  forever. Only the escrowed COIN is returned; the fee stays burned. A settled bid uses `claim`.
  Pinned: `e2e_bid_cancellable_after_cooldown_keeps_fee` (immediate cancel + non-owner cancel both
  rejected; owner cancel succeeds post-cooldown with the fee still burned).
- **Full lifecycle**: `e2e_full_genesis_to_buy_burn` runs deposit → vote → distribute → claim →
  DAO/Squads handoff → buy/burn auction across all six real binaries; the COIN winner sells COIN
  back into the surplus buy/burn and it is really burned (mint supply drops), closing the loop.
- **Principal protection under loss** (probe #12): if a market loss drops live insurance BELOW the
  reserved floor (principal counter), `execute`'s `surplus = insurance.saturating_sub(floor) = 0` →
  nothing is pulled and the subtraction can't underflow. (Lost coverage when the standalone pull tests
  were removed.) Pinned by `e2e_execute_pulls_nothing_when_insurance_below_floor` (slab insurance
  dropped to 800k < 1M floor → execute succeeds, holding stays 0, real vault untouched, floor unchanged).
- **Multi-round ratchet liveness** (probe #10): each `execute` pulls the burn-share of the CURRENT
  surplus and ratchets the retained share into the principal counter; as FRESH surplus accrues, later
  rounds pull it too (no permanent lockout) and the principal is never pulled. Pinned by
  `e2e_ratchet_pulls_fresh_surplus_across_rounds` (round 1 pulls 400k + floor→1.1M; inject 500k fresh
  surplus via a timelock'd Squads TopUp; round 2 pulls another 400k + floor→1.2M; holding 400k→800k;
  insurance always == the grown floor). Confirms the repeating buy/burn works over time; no bug.

### [FIXED] T. Insurance slab offset read `vault`, not `insurance` (subledger + twap) — finding-O class LOF
Both the subledger pro-rata haircut (`subledger/src/lib.rs PERC_INSURANCE_OFFSET`) and the twap
surplus pull (`twap-program/src/lib.rs INSURANCE_OFFSET`) read the asset-0 insurance fund straight
from the percolator market slab at a hardcoded byte offset. Both used `448 + 285`, derived from a
hand-counted struct layout that assumed `V16ConfigAccount` was 233 bytes. It is actually **249**,
so `+285` is the `vault` field; `insurance` is at **`448 + 301`** (slab offset 749, confirmed by
`core::mem::offset_of!(MarketGroupV16HeaderAccount, insurance)` against the real percolator crate,
and by scanning a live slab: vault@733, insurance@749, remaining_budget_total@893).
`vault` = total tokens in the market = `insurance + trader capital + pnl`, so it is `>= insurance`
and equal only when there is NO trading capital (exactly the case in every prior test, which is why
the old `insurance_offset_matches_real_percolator_slab` canary — funding via TopUp, which bumps BOTH
vault and insurance equally — could not tell them apart and the bug shipped).
Impact (both the finding-O failure class): with live trader capital in the market,
(a) the twap `pull_surplus` would treat trader/depositor capital as withdrawable "surplus" above the
reserved floor, and (b) the subledger pro-rata haircut would over-count the fund and under-charge the
haircut (paying early exiters too much, stranding late ones). FIX: both constants → `448 + 301`.
Pinned now by: (1) twap canary asserting `INSURANCE_OFFSET == 448 + offset_of!(.., insurance)` AND
bumping the adjacent `vault` field to a distinct sentinel to prove the read returns `insurance` not
`vault`; (2) subledger `impair_market` helper deriving every loss-coupled offset from the real struct
via `offset_of!`/value canaries. Tests: subledger `impaired_insurance_exit_is_pro_rata` (order-
independent 50% haircut against the real binary) + full twap chain (33) green.

### [FIXED] A. Stale quorum denominator → minority capture (genesis-vote trigger)
`trigger` checked quorum against `config.outstanding_principal`, a CACHE synced
only on `vote`. Attack: vote early when the subledger pool is tiny (cache=6), let
honest deposits grow the pool to 1006 without a re-vote, then trigger: 6*2 > 6
(stale cache) "passes" → a 6-principal minority captures the whole COIN
distribution. FIX: trigger now takes the subledger pool account and re-reads the
LIVE `outstanding_principal` for the quorum check. Regression:
`genesis-vote/tests/seal.rs::trigger_uses_live_pool_outstanding_not_stale_cache`.

### [FIXED] B. Vote outlives capital (genesis-vote support tallies are snapshots)
FIXED via a cross-program vote-lock (a boolean, simpler than the candidate
`locked_principal` since one position = one whole vote). The subledger Position
gained a `vote_locked` flag and the Pool a `vote_authority` (= the genesis-vote
config PDA); a new `IX_SET_VOTE_LOCK` (tag 6) lets ONLY that authority toggle it,
and `process_insurance_withdraw` refuses while set. genesis-vote `vote` now CPIs
SetVoteLock(1) on back / SetVoteLock(0) on retract (config PDA signs). A live
ballot pins its principal in the subledger; capital can leave only after the vote
is retracted — which the owner always controls, so no permanent-freeze risk. This
restores the "voters must retract before exit" invariant the single-program
genesis had. Regression (KEPT, real-percolator e2e):
insurance_percolator.rs::vote_locked_principal_cannot_exit_until_retracted
(vote → withdraw refused → retract → withdraw succeeds). Original analysis:

genesis-vote records `voted_principal`/`support_*` as a snapshot at vote time, but
the capital lives in the SUBLEDGER and the subledger `insurance_withdraw` does NOT
require the genesis-vote ballot to be retracted first. So a voter can vote
(support += P), then withdraw P from the subledger (capital returned), and the
ballot still counts toward quorum/majority with ZERO capital at risk — a free /
Sybil vote. WORSE after fix A: the live-outstanding denominator shrinks on exit
while the snapshot numerator stays, inflating quorum. The old single-program
genesis enforced retract-before-exit; the cross-program split broke it.
Candidate fix: genesis-vote vote/retract CPIs the subledger to set a
`locked_principal` on the position; subledger `insurance_withdraw` cannot reduce
principal below `locked_principal`. (Subledger exposes the lock to a registered
vote-authority only.) NEXT ITERATION.

### [FIXED] B2. Vote-lock freeze-after-finalize (regression from B's fix)
Adversarial self-review of B's fix: genesis-vote `vote` rejected ALL actions once
a proposal was sealed (`pv.executed`), so a WINNING voter could never retract to
clear the subledger vote-lock → principal frozen forever. FIX: post-seal forbid
only NEW backing (VOTE_BACK); always allow VOTE_RETRACT (clears the lock; seal is
immutable; post-seal tally writes are unread). Regression:
insurance_percolator.rs::winning_voter_can_retract_and_exit_after_finalize
(drives the real trigger to seal, then proves retract+exit works post-finalize).

### [FIXED] E. Distribution vault solvency not enforced at init (claim-race LOF)
`init_config` recorded `total_supply` from instruction data and validated the
vault's mint/owner, but never checked the vault was FUNDED to total_supply. The
seal only enforces `total_amount <= total_supply` (the claimed number), so a config
whose vault held less than promised would let early claimants drain it and STRAND
honest late claimants (first-come claim race). FIX: init now requires
`vault.amount >= total_supply` (InsufficientFunds otherwise), tying the promised
supply to real tokens — a config can never promise more than the vault holds, and
since seal caps total_amount at total_supply, every sealed proposal is fully
claimable. Test (KEPT): distribution.rs::init_config_rejects_an_underfunded_vault.

### [BLOCKED] Distribution claim/seal/append/burn — full adversarial read (tick 5)
Probed the entire distribution fund-exit surface; all well-defended:
- claim: pull model (`pk == recipient.key`), sealed-proposal pin, vault pin, window
  bound, index bound, double-claim zeroing (amount==0 reject). recipient_ata is
  unchecked but that is the recipient directing their OWN allocation — not theft.
- append_entries: rejected once `header.sealed || config.is_sealed()` (no post-seal
  drain), running `total_amount <= total_supply`, checked_add, capacity bound.
- seal_winner: authority==config.authority, not-already-sealed, `header.config ==
  config` (no foreign-proposal seal), `entry_count > 0`, total_amount <= supply.
- burn_unclaimed: sealed-only, vault+mint pinned, `clock.slot < window_end` blocks
  premature burns. No tests added (would only re-assert existing checks).

### [FIXED] F. Foreign distribution proposal → unsealable winner (finalize DOS)
genesis-vote `register_proposal` checked `distribution_proposal.owner ==
config.distribution_program` but NOT that the proposal belonged to
`config.distribution_config`. A proposal owned by the distribution program but
under a DIFFERENT config could be registered and voted on; `trigger` pins the
distribution_config to `config.distribution_config` and CPIs SealWinner against it,
which the distribution rejects on `header.config` mismatch. So a foreign-linked
proposal that WON could never be sealed → the genesis could never finalize (the
whole bootstrap bricks; the COIN/MetaDAO never forms). Voters can still retract+exit
(no fund freeze) but the protocol is dead. FIX: register_proposal now reads the
distribution proposal header (disc `DISTPRP1`, config at [8..40]) and requires it to
equal `config.distribution_config` — every votable proposal is guaranteed sealable.
Test (KEPT, real cross-program e2e with a second distribution config):
insurance_percolator.rs::register_rejects_foreign_distribution_proposal.
NOTE (low-risk, NOT fixed): a proposal under the correct config but with
entry_count==0 would also fail seal (seal requires entry_count>0); but entries can
be appended any time pre-seal and no rational voter backs an empty (zero-allocation)
proposal, so it is self-correcting, not a forced DOS.

### [FIXED] G. Hostile vote_authority → griefing freeze of depositors (LOF)
`init_insurance_pool` is permissionless and records `vote_authority` as-is (no
validation). An attacker could front-run creation of the genesis's (COIN, asset-0)
insurance pool with `vote_authority = attacker`. `set_vote_lock` required ONLY the
vote_authority to sign — so the attacker could then lock ANY depositor's position
(`set_vote_lock(victim, 1)`), and `insurance_withdraw` refuses while locked. The
real genesis-vote config PDA is NOT the authority, so it cannot unlock → victims'
principal FROZEN forever (griefing LOF). FIX: `set_vote_lock` now also requires the
position OWNER to sign (and `position.owner == owner`). A position can therefore
only be (un)locked in the context of its owner acting on their own vote — the only
legitimate case. The vote_authority gate stays so the owner cannot self-unlock and
bypass retract (which would re-open finding B). genesis-vote `vote` propagates the
voter's signature into the SetVoteLock CPI. Test (KEPT, real-percolator e2e):
insurance_percolator.rs::hostile_vote_authority_cannot_freeze_a_depositor.

### [FIXED] H. genesis-vote init_config didn't bind its wired dependencies
`init_config` stored `distribution_config` and `subledger_pool` as-is, with NO
check that they bind back to the config being created. An honest orchestrator could
thus wire the genesis to a poisoned/foreign pool or distribution config (e.g. the
front-run pool of finding G), silently bricking it: a subledger pool whose
vote_authority != this config PDA makes every vote's SetVoteLock CPI fail (no one
can vote), and a distribution config whose seal authority != this config PDA (or
for another mint) makes trigger's SealWinner always fail (finalize DOS). FIX: init
now requires, for this coin_mint, that (a) distribution_config is a real DISTCFG1
owned by distribution_program with coin_mint match and authority == config PDA, and
(b) subledger_pool is a real SUBPOOL1 owned by subledger_program with mint match and
vote_authority == config PDA. The config can only be created against dependencies
that recognize it — fail-fast at init instead of a mysterious brick at vote/trigger.
Complements finding G (G stops the freeze; H stops building on a poisoned pool).
Test (KEPT): seal.rs::init_config_rejects_pool_not_bound_to_this_config.

### [BLOCKED] Own-vault init_pool vault-substitution theft (now pinned by a test)
Probed: own-vault `deposit` (tag 1) transfers owner -> pool.vault and `withdraw`
(tag 2) pays from pool.vault signed by the pool PDA. If `init_pool` accepted a vault
owned by the attacker, they could stand up a pool, lure a deposit into their own
token account, and drain it directly via SPL (program withdraw would fail). Already
blocked: init_pool pins `vault_state.owner == pool PDA` (src line ~350). Added a
regression to lock this anti-theft invariant (it was previously untested):
subledger.rs::init_pool_rejects_a_vault_not_owned_by_the_pool.

### [BLOCKED] Distribution premature burn_unclaimed (DOS) — now pinned by a test
Probed: burn_unclaimed is permissionless. If it could run during the claim window
it would destroy claimants' COIN before they claim (DOS/LOF on every unclaimed
recipient). Already blocked: burn checks `clock.slot < window_end -> reject`
(src). The timing was only tested on the after-window side; added a regression for
the rejection side: distribution.rs::burn_unclaimed_is_rejected_during_the_claim_window
(burn at mid-window and at window_end-1 both rejected; recipients still claim in
full; burn permitted at/after window_end).

### [BLOCKED] TWAP on-chain attack surface — none yet
twap/ is a pure instruction-builder + math library (no entrypoint, no
process_instruction, no invoke). The buy/burn on-chain program (task #2) is not
built, so there is no live attack surface there yet. Re-examine once it lands.

### [FIXED] I. twap-program init didn't bind the controller multisig to the DAO
twap-program `init_config` validated the controller was OWNED by the Squads program
but never read its `config_authority`. So a TWAP could be wired to a real Squads
multisig whose config_authority is an attacker (not the DAO) — the config would
"look" DAO-governed while the DAO had no control (broken DAO->Squads link). FIX:
init now checks the controller is a Squads `Multisig` (disc) whose config_authority
(bytes [40..72]) == the named metadao_futarchy (DAO). Regression:
twap-program/tests/chain.rs (a multisig controlled by DAO but a different
metadao_futarchy passed -> rejected).

### [OPEN/NOTED] twap-program init permissionless + not genesis-bound
init_config is permissionless and per (market). It enforces controller-owned-by-
Squads + config_authority==DAO (finding I) but does NOT bind to the canonical
genesis: an attacker could still front-run with a self-consistent (their multisig,
their "DAO") config for the genesis market, bricking the legit TWAP setup or
controlling the future reconfigure path (same class as findings G/H). Proper fix
(follow-up): require squads_multisig == the deterministic genesis squads multisig
for coin_mint (derived via the rewards program's [b"genesis_squads", coin_mint]
create_key) and bind coin_mint/market to the genesis. Deferred until the
reconfigure/rotate instructions (which the controller actually gates) are built.

### [OPEN/NOTED] twap-program pull_surplus is permissionless + bps not enforced
IX_PULL_SURPLUS pulls a caller-specified `amount` bounded only by percolator's
WithdrawInsuranceLimited cap, NOT by the configured surplus_buy_burn_bps surplus
share. Pulled funds land in a twap_authority-owned holding (program-controlled, not
stealable), so no direct theft, but an over-crank could drain insurance beyond the
intended surplus share — relies on the post-mint insurance-policy rotation to
surplus-only mode. Bound the pull to the computed surplus share when the buy/burn
settlement slice lands (it needs to read market insurance-vs-backing state).

### [FIXED] J. Genesis Squads timelock was 48h, not the documented 1-week
The README documents a 1-week Squads timelock as THE user-exit backstop (Safety §3):
every genesis authority rotation runs DAO→Squads(1-week timelock)→..., leaving the
old authority in place for a week so users can exit before any rotation takes
effect. But `process_init_genesis_squads` created the multisig with
`TIMELOCK_48H_SECS` (48h), so the real exit window was ~2 days — a 3.5x shorter
safety window than promised, weakening the core backstop of the whole authority
chain. FIX: changed the on-chain genesis-squads timelock to one week
(TIMELOCK_1_WEEK_SECS = 7*24*60*60). squads_handover.rs updated so the enforcement
tests (M4/M5) prove execution is blocked until a FULL WEEK elapses (they advance the
clock by the renamed const). 4 squads tests green against real Squads v4.
NOTE: pinning init_genesis_squads's value end-to-end needs the drift-broken
integration harness; the timelock MECHANISM at 1-week is covered by M4/M5.

### [FIXED] K. Distribution didn't enforce the fixed-supply COIN (README §4)
README Safety §4 promises "the COIN mint has no mint authority ... no program can
mint COIN" — no inflation/dilution and no "mint to drain". But distribution
`init_config` validated the vault (mint/owner/funding, finding E) and never checked
the COIN mint's authority. So a distribution could be created against a
still-mintable COIN, and the mint-authority holder could mint unlimited COIN outside
the fixed pool, diluting every recipient's governance/value. FIX: init now unpacks
coin_mint and requires `mint_authority.is_none()` AND `freeze_authority.is_none()`
(a freeze authority could freeze the vault -> DOS all claims). The fixed pool is now
provably the entire COIN supply. Regression:
distribution.rs::init_config_rejects_a_mintable_coin (mintable -> rejected; after
revoking -> accepted). Cross-program tests updated to revoke the COIN authority
before dist init (matching the genesis-setup flow).

### [FIXED] H-overconstraint. genesis-vote init wrongly required subledger.mint == coin_mint
Finding H bound the subledger pool's mint to coin_mint. But the subledger holds the
at-risk COLLATERAL, a DIFFERENT mint from the distributed COIN (README money map);
finding K (fixed-supply COIN) made the conflict explicit — the COIN can't be the
mintable collateral. So H's mint check would REJECT a correct collateral != COIN
deployment (a self-inflicted DOS). FIX: dropped the `subledger_pool.mint ==
coin_mint` check from gv init_config; kept the security-critical binding
(vote_authority == this config PDA, findings G/H) which is what actually prevents a
poisoned/foreign pool. Cross-program tests refactored to use a separate fixed-supply
COIN (gv/distribution) vs the mintable collateral (subledger), as in the real design.

### [OPEN/DESIGN] L. Insurance exit is first-come under impairment, not pro-rata
Q: does the subledger correctly handle venue haircuts + surplus?
- SURPLUS: YES. percolator caps each WithdrawInsuranceLimited to
  `insurance*max_bps/1e4` then (deposits_only=1) `min(deposit_remaining)`, i.e. the
  deposited PRINCIPAL. Market profit/surplus is never withdrawable via the subledger
  exit. Correct ("never touch market profits", README §2).
- HAIRCUT: NO (first-come, not pro-rata). The cap tracks the LIVE insurance, and the
  withdraw also requires `amount <= vault` (percolator v16 ~line 8542/8555). The
  subledger requests the full `amount` (capped only to position.principal) and
  computes NO health-ratio haircut. So under an impairment (venue loss dropping
  insurance/vault below total deposited principal) the exit is FIRST-COME: an early
  depositor withdraws full principal and drains the impaired pool; a later one is
  stranded. This contradicts the documented "pro-rata under market loss / finalized
  withdraw haircuts by health ratio". Demonstrated against real percolator:
  insurance_percolator.rs::impaired_insurance_exit_is_first_come_not_pro_rata
  (alice exits whole + drains, bob stranded).
  NOT fixed — design decision needed: (a) accept first-come during the voting window
  with pro-rata only at a separate finalize path, or (b) make the subledger compute
  the haircut. (b) is non-trivial: it needs the LIVE asset-0 insurance figure
  (percolator's internal counter, not the vault token balance, which also holds
  backing), so the subledger would have to read the slab insurance accounting or
  percolator would expose a pro-rata withdraw. Flagged to the user.

### [FIXED] M. Proposal bait-and-switch (change distribution after voters back it)
distribution `append_entries` is allowed until seal, and genesis-vote
`register_proposal` did not freeze/snapshot the proposal. So a creator could register
a fair PARTIAL proposal, let voters back the gv_proposal, then append self-allocations
into the unallocated supply and trigger — sealing a distribution voters never
approved. Since the COIN IS the MetaDAO, that is a path to governance capture (bounded
only by the §3 timelock). FIX: register_proposal now snapshots the proposal's
(entry_count, total_amount) into the ProposalVote (and requires entry_count > 0 — only
a built proposal can be registered), and `trigger` re-reads the live proposal and
refuses to seal unless both match. So the sealed distribution is exactly the one
voters backed; any post-registration append breaks the snapshot and the seal is
refused. Regression (real cross-program e2e):
insurance_percolator.rs::proposal_changed_after_registration_cannot_be_sealed
(register partial -> vote -> creator appends -> trigger rejected).

### [BLOCKED] Percolator admin proxy (program/ IX_PERCOLATOR_ADMIN) — analyzed
Forwards percolator admin CPIs signed by the market_admin PDA. Well-guarded:
(a) tag ALLOWLIST (percolator_admin_tag_allowed) — only lifecycle-scoped tags;
(b) governance-authority gate (authority == coin_cfg.authority, signer);
(c) LOCKED ENTIRELY until genesis is finalized (the #16/#19 fix) — no pre-finalize
admin ops while depositor capital is at risk; kickstart/recovery use direct CPIs,
not this proxy. Post-finalize the allowlist is the MetaDAO's intended lifecycle
controls. Even the allowed UPDATE_INSURANCE_POLICY can't drain principal: the
subledger operator caps every withdraw to position.principal regardless of the
market policy. No gap. (Hard to e2e — needs the drift-broken genesis lifecycle.)

### [BLOCKED] Distribution seal authority — gated + already tested
seal_winner requires `authority.is_signer && authority.key == config.authority`
(the genesis-vote config PDA). A non-authority sealing would bypass the vote
entirely (governance capture) — already pinned:
distribution.rs::seal_then_recipients_claim_their_entries asserts an imposter
cannot seal and the real authority can.

### [BLOCKED] governance/ adapter forwarding (init/handover squads, percolator admin)
The adapter CPIs the rewards program signed by a PDA derived FROM the passed
rewards_program ([b"rewards_authority", rewards_program, coin_mint]). A malicious
rewards_program only ever yields a PDA bound to itself (no impersonation of the
legit authority), and the real validation (governance authority, finalized lock)
lives in the rewards program the adapter forwards to. Trusted-forwarder pattern,
PDA-bound. No gap.

### [COVERAGE] Full genesis lifecycle e2e (all real programs) — pinned
No test ran the COMPLETE chain to a COIN claim; a broken link (e.g. the cross-program
seal producing a non-claimable distribution) would brick the genesis. Added a full
e2e with percolator + subledger + genesis-vote + distribution all loaded:
collateral deposit -> vote -> permissionless trigger seals the winning distribution
by CPI -> winning recipient CLAIMS the fixed-supply COIN -> double-claim refused.
insurance_percolator.rs::full_lifecycle_deposit_vote_seal_then_recipient_claims_coin.
This pins that the genesis actually yields a claimable distribution and exercises the
separate collateral-vs-COIN mints (finding K) through the whole flow.

### [BLOCKED] vote_weight arithmetic overflow (genesis-vote)
`vote_weight = floor(log2(age)) * principal` uses `saturating_mul` (no wrap/panic)
and accumulation uses `checked_add` (graceful error). Saturating to u64::MAX needs
~2^58 real deposited tokens — self-bounded by capital, not attacker-reachable. No
fix/test needed.

### [BLOCKED] Subledger insurance_deposit holding-account substitution
`process_insurance_deposit` does not validate the `holding` account, but the
TopUpInsurance CPI's internal SPL transfer is authorized by the pool PDA, so
percolator requires `holding` to be pool-PDA-owned; a hostile holding makes the CPI
revert (whole tx reverts, user's step-1 transfer with it). The user pre-funds
`holding` with exactly `amount` and is credited `amount` — no path credits more
than entered or touches another user. Well defended; no test added.

### [BLOCKED] Subledger pool/position substitution in genesis-vote `vote`
`vote` pins `sub_pool == config.subledger_pool`, derives the position PDA from
that pool + voter, re-checks the stored pool/owner, and requires subledger-program
ownership. A foreign high-principal position cannot be substituted. Well defended;
no test added (would only re-assert existing checks).

### [FIXED] C. Pool type-confusion: own-vault path accepted an insurance pool
The subledger serves both own-vault pools (tags 1/2, funds in a pool-PDA-owned
vault) and percolator-insurance pools (tags 4/5, funds in the percolator
insurance vault). The insurance handlers already gated on `!pool.is_insurance()`,
but the own-vault `deposit`/`withdraw` had NO matching guard. Attack/footgun:
call own-vault deposit (tag 1) on an insurance pool — the SPL transfer pushes the
user's funds straight into the percolator insurance vault with NO TopUpInsurance
CPI (percolator never counts them) and records an own-vault position; the own-vault
withdraw can never sign those funds back out (the pool PDA is not the insurance
vault's token authority) → principal STRANDED (user LOF). FIX: added the symmetric
`if pool.is_insurance() { return Err }` guard to own-vault deposit AND withdraw.
Test (KEPT — pins a real stranded-funds boundary against the real percolator
binary): subledger/tests/insurance_percolator.rs::
own_vault_deposit_is_rejected_on_an_insurance_pool.

### [FIXED] D. Active-path canonical-vault pin (GH issue #24 / PR #25)
GH issue #24 + PR #25 (@SrMessiSOL) report that percolator-meta accepts a
non-canonical Percolator vault. ADVERSARIAL REVIEW of PR #25: the diff derives
the canonical ATA `[vault_authority, spl_token::ID, mint]` under the REAL ATA
program id and equality-checks it — byte-for-byte identical to percolator's own
`canonical_vault_address` (F-VAULT-FRAG). It is ADDITIVE (keeps the existing
mint/owner check) and can never reject a vault percolator would accept (percolator
enforces the same pin), so NO DOS and NOT a backdoor. The PR is legitimate but
targets the DEPRECATED custodial program/, whose integration suite no longer
compiles on master OR the PR branch due to percolator interface drift
(WrapperConfigV16 authority fields renamed) — unrelated to the PR.
The same gap existed on the ACTIVE path: subledger init_insurance_pool checked
`owner == vault_authority` + mint but did NOT pin the canonical ATA address. Not
exploitable (a non-canonical pool is simply inert — every deposit/withdraw CPI
reverts with InvalidVaultAccount), but a fail-fast pin is correct. FIX: added
`canonical_vault_address` to subledger and pinned it in init_insurance_pool.
Test (KEPT): insurance_percolator.rs::init_insurance_pool_rejects_non_canonical_vault.

### [FIXED] N. Fixed-supply COIN: pre-mint dilution (finding K gap)
Finding K required mint_authority==None (no FUTURE minting) but not that the mint's
CURRENT supply equals the distributed pool. An attacker could pre-mint extra COIN to
themselves BEFORE revoking, then fund the vault with only total_supply — holding
undistributed COIN that dominates governance (the COIN IS the MetaDAO). FIX: init
also requires `mint.supply == total_supply`; with the vault-funding check (E) this
proves every COIN that exists is in the distribution vault. Regressions:
distribution.rs::init_config_rejects_a_mintable_coin (now also pre-mint extra ->
rejected) + init_config_accepts_a_fully_in_vault_fixed_supply_coin.

### [COVERAGE] Squads -> TWAP reconfigure: timelock-gated, proven e2e
The twap IX_RECONFIGURE is gated on the config's Squads multisig default vault PDA.
Adversarial review: the multisig is pinned at init (finding I: config_authority ==
DAO), the vault is derived from it, new_bps is bounded (0,10000], and a foreign
config only exposes its own multisig's vault — secure. Pinned both ways against the
REAL Squads v4 binary:
- negative (chain.rs): a random signer / the correct vault address WITHOUT a
  signature are both rejected.
- keystone (chain.rs::reconfigure_only_via_squads_vault_execute_after_timelock):
  the DAO proposes a vault-transaction that CPIs reconfigure; executing it BEFORE the
  1-week timelock is rejected (config unchanged); after warping past the timelock the
  execute succeeds and the buy/burn share changes. This proves the on-chain
  DAO -> Squads(1-week) -> TWAP control link end-to-end and that the timelock cannot
  be bypassed for a TWAP reconfigure.

### [COVERAGE] Handoff bridge (twap IX_ACCEPT_OPERATOR) + dual-sign insight
Security insight: percolator UpdateAssetAuthority (tag 65) requires BOTH the current
authority (asset_admin) AND the new authority to SIGN — a safety feature preventing
rotation to a non-consenting/dead key. So the handoff to twap_authority cannot be a
plain squads-execute of UpdateAssetAuthority; it needs a twap-program bridge that
co-signs as twap_authority. Built IX_ACCEPT_OPERATOR: gated on the config's Squads
vault (the asset_admin, reachable only via a timelock'd execute), it CPIs percolator
UpdateAssetAuthority(asset 0, INSURANCE_OPERATOR, new=twap_authority), co-signing as
twap_authority via invoke_signed. The squads-vault gating is pinned (chain.rs: a
non-vault signer cannot trigger the rotation; same proven gate as the reconfigure
keystone). NEXT SLICE: the positive real-percolator e2e — a market with asset_admin
= the squads vault, squads execute -> accept_operator -> operator rotates
subledger->twap, and the subledger can no longer withdraw.

### [COVERAGE/OPS] Percolator interface coupling — drift caught + handoff CPI verified
The genesis is tightly coupled to percolator's exact per-instruction account lists.
A percolator .so rebuild mid-session silently broke EVERY subledger insurance CPI
with NotEnoughAccountKeys (the deployed binary briefly wanted more accounts); a later
rebuild restored it. Operational risk: a percolator upgrade can break the insurance
CPIs in lockstep — the subledger/twap CPIs must be re-synced on any percolator
interface change. Now that percolator is back in sync, pinned the TWAP handoff
bridge's percolator CPI encoding against the REAL binary:
insurance_percolator.rs::percolator_update_asset_authority_operator_encoding_is_accepted
(UpdateAssetAuthority tag 65, asset 0, kind INSURANCE_OPERATOR=2, [current(signer),
new(signer), market(w)] accepted). This both verifies accept_operator's CPI and acts
as an early-warning canary for future percolator drift on that instruction.

### [BLOCKED] Insurance-policy change is marketauth-gated (no attacker drain-policy)
The handoff rotates the insurance policy (principal-only -> surplus-only) via
percolator UpdateInsurancePolicy (tag 33), gated on the GLOBAL marketauth
(handle_update_insurance_policy: expect_live_authority(cfg.marketauth, admin)). The
risk: if anyone could change the policy, they could set deposits_only=0,
max_bps=10000 and enable withdrawing ALL insurance principal (drain). Pinned against
the real binary (insurance_percolator.rs::percolator_update_insurance_policy_is_marketauth_gated):
the marketauth can set the policy (encoding accepted) and a NON-marketauth is
rejected. In the handoff the marketauth is the squads vault, so policy changes are
1-week-timelock-gated. Also a drift canary for the policy-rotation encoding.

### [COVERAGE] Slice 3 handoff e2e — DAO->Squads(1wk)->TWAP->percolator, four real binaries
The keystone §3 test, proving the dangerous operation (rotating percolator's asset-0
insurance operator away from the constrained subledger) is timelock-gated end to end.
twap-program/tests/chain.rs::handoff_rotates_operator_to_twap_only_after_timelock:
market-0 with marketauth = the squads vault; DAO proposes a vault-transaction that
CPIs twap IX_ACCEPT_OPERATOR (which CPIs percolator UpdateAssetAuthority, co-signing
as twap_authority); executing BEFORE the 1-week timelock is rejected; after warping
past it the nested squads->twap->percolator CPI succeeds and the operator rotates to
twap_authority. All four real binaries (squads v4 + percolator + twap + the chain).
This is the §3 user-exit backstop in action: any authority rotation is delayed a full
week, in the clear, with the old constrained authority live the whole time.

### [COVERAGE] Slice 3 policy-rotation handoff e2e (timelock-gated, four binaries)
The handoff's other half: rotating the insurance policy (principal-only ->
surplus-only) is timelock-gated end-to-end. A bad policy (deposits_only=0,
max_bps=10000) could enable draining principal, so it must run through the timelock.
twap-program/tests/chain.rs::handoff_rotates_insurance_policy_only_after_timelock:
squads-execute -> percolator UpdateInsurancePolicy with the squads vault as the
marketauth; blocked before the 1-week timelock, succeeds after. Both handoff
operations (operator rotation + policy rotation) are now proven §3-timelock-gated
through the full DAO->Squads->percolator path with the real binaries.

### [OPEN/DEPENDENCY] O. Handoff before surplus-floor -> twap can pull PRINCIPAL (LOF)
After the operator handoff (twap IX_ACCEPT_OPERATOR) the twap_authority is the
percolator asset-0 insurance operator, and pull_surplus is PERMISSIONLESS. pull_surplus
pulls a caller-specified `amount` bounded ONLY by percolator's WithdrawInsuranceLimited
policy, with NO surplus-floor (reserved_principal + retained_surplus_floor, README §5).
Under the genesis principal-only policy (deposits_only=1) percolator caps to DEPOSITED
PRINCIPAL — so if the operator is handed to the twap while any depositor principal
remains, anyone could crank pull_surplus and pull PRINCIPAL (not just surplus) into the
twap holding -> LOF for non-exited depositors. §3 mitigates (depositors exit during the
1-week window before the rotation executes), but a non-exiter is exposed.
Real fix (buy/burn slice, slice 4): pull_surplus must enforce a floor — pull at most
(live asset-0 insurance - reserved principal). That needs the live insurance figure
from the slab (NOT the vault token balance, which also holds backing) and the reserved
principal (the subledger pool outstanding). Until then: do NOT perform the handoff, and
the handoff proposal should atomically set the policy to surplus-mode. Documented with
loud SAFETY comments in twap-program pull_surplus + accept_operator. NOT a live vuln
(nothing deployed; handoff naturally follows the buy/burn build) but a sequencing LOF
risk recorded so it isn't missed.

### [O-update] Surplus-floor implementation blocker (slice 4)
Attempted finding O's fix: have pull_surplus compute surplus = insurance - reserved
and pull at most that. percolator exposes the clean accessor
`percolator_prog::state::read_market(slab) -> (WrapperConfigV16, MarketGroupV16)`
with `wrapper.insurance_withdraw_deposit_remaining` (reserved principal) and
`group.header.insurance` (live insurance). BUT adding percolator-prog as a twap-program
LIB dep FAILS the SBF build — it pulls in `wincode-derive`, whose manifest doesn't
parse on the BPF toolchain (`failed to parse manifest ... wincode-derive-0.4.6`). So
the clean in-program read is blocked. Options for slice 4:
(a) percolator exposes a lightweight, wincode-free accessor for (insurance,
    deposit_remaining) the twap can link; or
(b) the twap reads raw slab offsets for those two u128s — but the slab layout drifts
    (proven this session), so this needs a pinned percolator + a layout canary.
Until resolved, finding O stands: do NOT perform the handoff, and the loud SAFETY
comments in pull_surplus/accept_operator remain. Reverted the read_market attempt;
twap-program stays green.

### [STATE] Audit coverage summary (live code exhaustively swept)
78 tests green across all crates. Findings A-O recorded. The live/built on-chain code
(subledger, genesis-vote, distribution, twap-program) has been swept across every
guidance category and found hardened:
- account substitution / PDA-seed confusion: pool/position/ballot/proposal/config all
  PDA-pinned + disc-checked; no cross-substitution (A,F,H, vote/trigger pins).
- missing signer/owner checks: owner-bound exits, set_vote_lock owner-signer (G),
  seal authority-gated, reconfigure/accept_operator squads-vault-gated.
- account reinit: every init checks lamports/data_len==0; no account is ever closed,
  so no close-then-reinit.
- arithmetic over/underflow: checked_add/sub on all tallies/balances; vote_weight
  saturating; u128 for quorum/majority.
- type confusion: every account carries + checks an 8-byte discriminator.
- unchecked CPI: token_program pinned to spl_token::ID; percolator CPIs go to the
  config-pinned program; squads vault gating; the percolator-side authority is
  enforced by percolator (adversarially pinned: operator + policy rotations).
- rent/closing, sysvar spoofing: no account closing; Clock via syscall (not passed).
- bait-and-switch, vote-outlives-capital, hostile-authority freeze, fixed-supply,
  drain-policy, minority-capture: all fixed (A,B,B2,G,K,M,N) + real-binary regressions.
Drift canaries (insurance_percolator.rs) pin every percolator CPI encoding the code
uses (TopUp/Withdraw/UpdateAssetAuthority/UpdateInsurancePolicy) against the live .so.
OPEN, build-gated: finding O (surplus floor, needs a wincode-free percolator accessor
for slice 4) and the twap genesis-binding front-run (needs slice 2's squads-creation
port). No live vuln remains in built code.

### [O-update 2] Surplus-floor: confirmed needs a percolator-side accessor (precise ask)
Investigated the raw-offset alternative for finding O's floor. Result: NOT cleanly
doable on-chain.
- `insurance_withdraw_deposit_remaining` lives in WrapperConfigV16 (#[repr(C)] +
  bytemuck::Pod) at slab offset HEADER_LEN(16) + offset_of!(...): computable + stable.
- `insurance` lives in MarketGroupV16, which is #[cfg(not(target_os="solana"))] with
  Vec fields — i.e. the HOST deserialization, not the on-chain layout. The slab stores
  the group zero-copy with the Vecs serialized inline, so `insurance`'s slab offset is
  NOT computable from the struct (this is exactly why read_market/wincode exists), and
  read_market can't link into a BPF program (wincode-derive manifest fails on SBF).
PRECISE ASK for the percolator side to unblock slice 4's floor:
  add a target_os="solana"-compatible, wincode-free getter, e.g.
    pub fn read_asset0_insurance_and_reserved(slab: &[u8]) -> Option<(u128, u128)>
  returning (market_insurance_remaining(asset 0), insurance_withdraw_deposit_remaining)
  by reading the fixed scalars directly from the slab bytes. Then twap pull_surplus
  enforces amount <= insurance - reserved. Until then finding O stands (handoff must
  not run; SAFETY comments in place).

### [FIXED] M2. Front-run griefing DOS on register_proposal (genesis-vote)
Finding M takes a snapshot of (entry_count, total_amount) at REGISTRATION time and
`trigger` rejects the proposal forever if the live distribution proposal no longer
matches that snapshot. register_proposal was permissionless (only payer.is_signer),
so an attacker could register a creator's PARTIALLY-built distribution proposal,
freezing a stale snapshot; the creator's very next `append` would then make the live
proposal mismatch the sealed snapshot, and `trigger` would reject it permanently — a
front-run that bricks a legitimate proposal (it can never be sealed -> finalize DOS),
costing the attacker only the gv_proposal rent.
Fix (genesis-vote/src/lib.rs register_proposal, in the snapshot block after the
config-binding check): bind registration to the distribution proposal's CREATOR
(header [48..80]) — `if creator != *payer.key { return Err(IllegalOwner) }`. The
creator registers only once they have finished building, so no third party can freeze
a premature snapshot. Regression: subledger/tests/insurance_percolator.rs
`only_the_proposal_creator_can_register_it` (a non-creator signer is rejected; the
creator succeeds) against the real binaries. Green.

### [BLOCKED] Distribution claim/seal/burn — account-substitution swept, no new vector
Adversarially swept the distribution claim path (the COIN payout surface) for LOF/DOS
this tick. Confirmed hardened; no test added (would be tautological):
- pull-model impersonation: claim pins `entry.pubkey == recipient.key` (only the named
  recipient is paid) AND `recipient.is_signer`. Covered by
  `seal_then_recipients_claim_their_entries` ("cannot claim bob's entry").
- double-claim: the entry amount is zeroed after the transfer; re-claim hits
  `amount == 0`. Covered (same test).
- cross-config substitution (the two non-obvious paths, now reasoned explicitly):
  (a) feeding a FOREIGN sealed proposal into the real config is blocked by
      `config.sealed_proposal == *proposal_account.key` (the config names exactly one
      payable proposal); (b) pairing the REAL vault with an attacker-sealed proposal is
      blocked because `vault.key == config.vault` and the sealed_proposal both derive
      from the SAME config account — there is no way to mix a real vault with a foreign
      proposal. The vault-authority PDA is seeded by `config.coin_mint`, so each config
      can only sign for its own vault.
- proposal griefing: create_proposal binds `header.creator`, append_entries requires
  `header.creator == creator.key` (third parties cannot append to / brick a creator's
  proposal — distribution-side analogue of finding M2, already enforced here).
- window race: claim requires window OPEN, burn requires window CLOSED (no overlap);
  burn-during-window DOS pinned by `burn_unclaimed_is_rejected_during_the_claim_window`.
- supply/strand: init pins vault.amount >= total_supply, seal pins total_amount <=
  total_supply, so claims can never strand a late recipient. Covered by
  `init_config_rejects_an_underfunded_vault` / `append_cannot_exceed_total_supply`.
- OOB: capacity <= MAX_ENTRIES (10k), index < entry_count <= capacity, so
  entry_offset stays in-bounds; size math can't overflow.
Distribution suite green (lib 4 + integration 7).

### [COVERAGE] Winner-take-all is irreversible across COMPETING proposals
Swept the genesis-vote vote/trigger tally surface this tick (checked_add/sub on every
tally, ballot PDA-pinned per (config,voter), sub_position canonical-PDA + owner pinned,
quorum read LIVE from the pool). All hardened. Found one DISTINCT boundary with no
direct coverage and pinned it: the existing re-trigger test only blocks the SAME
proposal (via pv.executed). Two COMPETING proposals share ONE distribution config, and
post-execution VOTE_RETRACT is allowed (so voters can leave an executed A and shift
weight onto B) — so the genesis-vote layer alone does not guarantee a single winner.
The true winner-take-all gate is the distribution `seal_winner` is_sealed() check:
B's trigger passes every gv check, sets pv_B.executed, then the seal CPI fails because
the config is already sealed, reverting B's trigger whole. New test
(genesis-vote/tests/seal.rs `a_second_proposal_cannot_reseal_after_a_winner_is_sealed`):
A seals; B is then made to look winning at the gv layer; trigger(B) is rejected and the
sealed winner stays A. KEPT — pins the cross-proposal irreversibility (not the same as
the same-proposal re-trigger block). genesis-vote suite green (lib 3 + seal 4).
Also noted (not a live risk): total_cast_weight is u64 and each voter adds <= 63*principal;
the sum could in principle overflow u64 only at absurd aggregate principal (> ~2.9e17
base units genuinely at risk), where checked_add fails the marginal vote (no corruption,
just a failed late vote). Not worth a saturating change; recorded for completeness.

### [BLOCKED] Subledger fund-movement (deposit/withdraw, both pools) — swept, no new vector
Swept the actual principal-movement surface this tick for LOF/dilution/rounding theft.
Confirmed sound; no test added (would be marginal — the boundaries are already pinned):
- NO phantom-principal dilution: own-vault deposit (subledger.rs path) does a real
  owner-signed SPL transfer of EXACTLY `amount` owner_ata -> vault BEFORE
  `outstanding_principal += amount`; if the transfer fails the tx reverts. So recorded
  principal is always backed by funds in the vault — an attacker cannot inflate
  outstanding without funding it (which would otherwise shrink honest depositors'
  pro_rata = balance*principal/outstanding on impairment). Insurance deposit likewise
  moves funds via TopUpInsurance before bumping outstanding.
- Rounding favors the POOL, never the attacker: mul_div_floor floors every pro_rata
  payout, so a withdrawer always gets <= fair share; dust strands in the vault (not
  stealable — only the pool PDA can move vault funds). No repeated-deposit rounding
  drain.
- No impaired-pool first-mover advantage (own-vault): proven order-independent — each
  exit removes balance and outstanding proportionally, leaving the remaining ratio
  unchanged. Pinned by `impaired_pool_is_pro_rata_and_order_independent` (alice-first,
  asserts bob gets the SAME 50% haircut, not worse). Donating to the vault only RAISES
  others' payout; an attacker cannot remove from the vault. No sandwich.
- Haircut/surplus coverage is complete: own-vault fair pro-rata + surplus
  (`with_surplus_policy_returns_yield_pro_rata`); insurance first-come
  (`impaired_insurance_exit_is_first_come_not_pro_rata`, finding L, documented);
  payout() pure-fn unit tests cover healthy/impaired/with-surplus/guards both ways.
- Owner/PDA pins: every deposit/withdraw re-derives the pool PDA + bump, pins
  vault==pool.vault, position==canonical PDA, owner-signer + position.owner==owner.
  Covered by `principal_only_owner_exit_returns_funds_and_guards` and
  `init_pool_rejects_a_vault_not_owned_by_the_pool`.
Subledger suites green (own-vault subledger.rs 5 + insurance_percolator 15).

### [FIXED] P. init_config front-run squat -> permanent deployment DOS (twap-program)
twap-program init_config is PERMISSIONLESS and its bindings (squads_multisig,
metadao_futarchy, coin_mint, percolator_program) were caller-supplied with only an
INTERNAL consistency check (squads_multisig owned by Squads + its config_authority ==
metadao_futarchy). The config PDA was keyed on `market_slab` ALONE. So an attacker could
stand up a throwaway Squads multisig (config_authority = an attacker key they also name
as metadao_futarchy — cheap, one multisig_create_v2), pass the consistency check, and
init the per-market config FIRST with their own bindings. The squatted config is inert
(accept_operator/reconfigure are gated on the squatted multisig's vault, which is NOT
the market's asset_admin, so it can never rotate the real operator) — but the per-market
config PDA is now TAKEN and cannot be re-initialized (AccountAlreadyInitialized), so the
real DAO's buy/burn deployment for that market is permanently bricked. A post-genesis,
no-fund-loss, permanent griefing DOS; the market_slab is public so the front-run is easy.
Fix: fold the caller-set bindings into the config PDA seed — now
[CONFIG_SEED, market_slab, squads_multisig, coin_mint, percolator_program]. The legit
config PDA = f(market, real_ms, real_coin, real_perc); to land an account THERE an
attacker must pass exactly those, which forces the real metadao_futarchy (via the
config_authority check) and yields the CORRECT config (no harm). Any attacker variation
lands at a different PDA the real deployment ignores. No percolator slab read needed (so
NOT blocked on finding O's accessor), no owner check (keeps fake-market unit tests valid).
reconfigure/accept_operator/pull_surplus never re-derive the config PDA (they trust
owner==program_id + bindings), so the seed change is safe for them. Regression:
twap-program/tests/chain.rs `init_config_front_run_with_attacker_multisig_cannot_block_the_real_deployment`
(attacker front-runs with their own multisig; lands at a different PDA; the real DAO's
init still succeeds and the live config is bound to the real multisig/DAO). Against the
real Squads v4 binary. twap-program suite green (lib 2 + chain 5).

### [FIXED] Q. init_insurance_pool front-run squat -> genesis deposits routed to attacker market (LOF)
Same class as finding P, worse impact. subledger init_insurance_pool is PERMISSIONLESS
and its market binding (market_slab, percolator_program, vault, vote_authority) was
caller-supplied; the pool PDA was keyed on (mint, asset_id) ALONE. The genesis pool PDA
= f(COIN_mint, 0) and the gv config PDA = f(COIN_mint) are BOTH predictable. So an
attacker could:
 1. stand up their own percolator market MKT_a (they are its marketauth) with the
    subledger pool PDA pre-set as MKT_a's asset-0 insurance authority/operator;
 2. front-run init_insurance_pool: pool=f(COIN_mint,0), market_slab=MKT_a, vault=MKT_a's
    canonical insurance vault, vote_authority = the predictable real gv config PDA,
    which PASSES init (vault is MKT_a's canonical vault) and later PASSES the gv
    init_config binding check (pool.vote_authority == gv config PDA).
Genesis then wires to a pool that routes every depositor's TopUpInsurance into MKT_a.
The attacker (MKT_a's marketauth) can then strand or bleed that insurance (hostile
policy / engineered market loss to their own trading account): depositor LOF, not just a
setup DOS. The real orchestrator's pool init also fails (PDA taken) = DOS even without
the LOF escalation.
Fix: fold market_slab + percolator_program into the pool PDA seed — now
[b"subledger_pool", mint, asset_id, market_slab, percolator_program]. The genesis pool
address can only ever hold a pool bound to the real market; an attacker's pool (any other
market) lands at a different PDA the genesis ignores. Own-vault pools pass Pubkey::default()
for both seed components (matching what they store), so the own-vault path is unchanged.
Threaded through every pool derive + invoke_signed seed set (init_pool, withdraw,
init_insurance_pool, insurance_deposit, insurance_withdraw). genesis-vote is unaffected
(it never derives the pool PDA; it trusts the stored config.subledger_pool key). No
percolator slab read needed (NOT blocked on finding O). Regression:
subledger/tests/insurance_percolator.rs `init_insurance_pool_cannot_be_squatted_to_misdirect_the_genesis_pool`
(attacker inits a pool bound to their own market against the real percolator binary; it
lands at a different PDA; the genesis pool still inits and binds the REAL market). On the
old seed the attacker pool PDA would equal the genesis pool PDA — the assert_ne + the
genesis re-init would both fail, so the test genuinely catches the regression. All suites
green: subledger lib 6 + insurance 16 + own-vault 5; genesis-vote 3+4; distribution 4+7.
NOTE: this is the same permissionless-init + caller-bindings + too-few-seeds pattern as
finding P (twap). Pattern to watch in any future init: bind the PDA to ALL trust-relevant
caller-supplied accounts, or land squats at harmless distinct addresses.

### [FIXED] R. gv init_config front-run squat -> genesis bound to attacker pool (LOF/DOS)
Third instance of the permissionless-init squat pattern (after P/Q). genesis-vote
init_config is permissionless and its config PDA was keyed on COIN_mint ALONE. It binds
a distribution_config and a subledger_pool, each required to point back at the (then
predictable) gv config PDA. The distribution_config is a UNIQUE PDA f(COIN) that can't be
forged (distribution init needs the funded fixed-supply COIN), so it can't be substituted
-- but the subledger_pool is NOT unique: an attacker could create their own valid pool
(vote_authority = the predictable gv PDA, bound to a market they control post-finding-Q)
and FRONT-RUN init_config to bind the genesis to THAT pool. Then every depositor's
principal routes into the attacker's pool/market (LOF), or the quorum is read from the
wrong pool (DOS), and the real orchestrator's init fails (PDA taken).
Fix: fold subledger_pool into the gv config PDA seed -> [b"gv_config", COIN_mint,
subledger_pool]. The legit gv address = f(COIN, real_pool); an attacker's pool yields a
different gv PDA the genesis ignores. And because the unique distribution_config's seal
authority is pinned to ONE gv PDA, post-fix only the pool that distribution commits to can
ever be bound -- a substituted pool makes `expected` mismatch the distribution authority
and is refused. distribution_config is NOT in the seed (it is already unique per COIN, so
unsubstitutable). Threaded through init derive + create_pda + the vote() and trigger()
gv-config signing seeds (config.subledger_pool). No percolator slab read (not blocked on
finding O). Tests: seal.rs Env derives gv config after the pool; new
`gv_config_cannot_be_bound_to_a_substituted_pool` (asserts the gv PDA now commits to the
pool -- fails pre-fix -- and that binding a substituted attacker pool is rejected by the
distribution-authority pin). All suites green: genesis-vote 3+5; subledger 6+16+5.
PATTERN now 3x (P twap, Q subledger, R gv): permissionless init + caller-supplied
bindings + PDA keyed on too few seeds. RULE: an init PDA must commit to every
trust-relevant caller-supplied account that is not already unique/unforgeable, so squats
land at harmless distinct addresses. Remaining permissionless init reviewed: distribution
init_config keys on COIN_mint and binds the COIN mint + vault (validated funded/fixed
supply) -- the COIN mint is the identity, the vault/authority are caller-set but a squat
there only produces a config for that same COIN (no misroute), and a re-init is blocked;
acceptable. twap init_config = finding P (fixed). subledger init_insurance_pool = finding
Q (fixed).

### [COVERAGE] Grand-unified E2E: genesis -> handoff -> twap surplus pull (ALL six binaries)
Built the full end-to-end lifecycle in ONE litesvm instance against every real binary
(subledger, genesis-vote, distribution, percolator, Squads v4, twap-program):
twap-program/tests/chain.rs `e2e_full_genesis_to_twap_surplus_pull`.
Flow: market-0 with marketauth = the Squads vault (asset_admin) -> DAO/Squads (1-week
timelock) injects insurance SURPLUS via percolator TopUpInsurance -> DAO/Squads grants the
asset-0 insurance authority+operator to the subledger pool (subledger.accept_operator, the
pool only CONSENTS) -> a depositor tops up REAL percolator insurance through the subledger
-> fixed-supply COIN distribution set up + the genesis vote/trigger seals the winning
distribution by CPI -> the winner CLAIMS the COIN -> DAO/Squads rotates the insurance
policy to surplus-mode AND the operator to the twap (both timelock-gated) -> the twap, now
the asset-0 insurance operator, pull_surplus pulls exactly the surplus, leaving depositor
principal in insurance. Asserts: insurance = surplus+principal after deposit; winner gets
the full COIN; twap holding receives the surplus; principal remains.
Design honored: the subledger NEVER rotates keys. Squads (driven by the DAO) is the
asset_admin and the only key-rotator; subledger + twap are pure insurance fund-managers
that only consent (accept_operator) to receive the operator role. The added subledger
accept_operator (tag 7) is the mirror of the twap's, required solely because percolator's
asset-0 UpdateAssetAuthority has no consent-free grant path (the incoming key must co-sign).
Stage-A (`e2e_squads_grants_operator_to_subledger_then_real_deposit`) pins the grant+deposit
half on its own. All suites green: twap lib 2 + chain 7; subledger 6+16+5; gv 3+5; dist 4+7.

### [BLOCKED] E2E probe: operator grant cannot bypass the Squads timelock
ATTACK: an attacker calls subledger.accept_operator DIRECTLY (not through a Squads
execute), signing as a forged asset_admin, to grant the asset-0 insurance operator to the
pool outside the 1-week timelock. The pool consents (its PDA is hardcoded in
accept_operator), but the inner percolator UpdateAssetAuthority requires the signer to be
the asset-0 asset_admin (the Squads vault) — an attacker key (and the plain payer) is not,
so percolator rejects. Confirms the grant/rotation is reachable ONLY through the real
asset_admin, i.e. a timelock'd Squads execute; calling the subledger straight cannot
sidestep it. accept_operator is powerless on its own — it only co-signs; percolator is the
gate. Test: twap-program/tests/chain.rs
`e2e_attacker_cannot_grant_operator_bypassing_squads` (real Squads v4 + percolator + the
deployed subledger). KEPT — pins the core authority boundary of the whole handoff.

### [OPEN/DEMONSTRATED] O-update 3. finding O proven end-to-end against real binaries (LOF)
ATTACK (finding O, now demonstrated): after the operator handoff to the twap with the
surplus-mode policy (deposits_only=0), a PERMISSIONLESS cranker calls pull_surplus and
drains DEPOSITOR PRINCIPAL even when there is ZERO surplus. Built end-to-end on the real
binaries: Squads funds insurance with 1,000,000 of PURE principal (no surplus), hands the
operator to the twap, then a cranker pulls 800,000 (80% = the surplus-mode max_bps cap) —
and it SUCCEEDS, moving principal into the twap holding. percolator surplus-mode caps to
max_bps of insurance and reserves nothing (it even zeroes insurance_withdraw_deposit_remaining),
so the floor MUST be enforced twap-side, which is not yet built. Test:
twap-program/tests/chain.rs `e2e_finding_o_cranker_drains_principal_no_floor` — it asserts
the drain currently SUCCEEDS (a KNOWN-OPEN marker); flip it to assert rejection once
pull_surplus enforces `amount <= insurance - reserved`.
Confirmed the fix is still blocked from this repo: the surplus floor needs the slab's
asset-0 `insurance` figure. The canonical insurance vault token balance is NOT a usable
proxy (it is the market's shared collateral vault, not insurance-only), so the reserved
amount cannot be derived from (vault_balance - subledger_outstanding) on-chain. Needs the
percolator-side wincode-free accessor read_asset0_insurance_and_reserved (findings O,
O-update 2). Until then: DO NOT perform the handoff in production with depositor principal
still present.

### [FIXED] O. Surplus floor — twap reads the slab insurance directly (no accessor needed)
Finding O is fixed. The earlier "blocked on a percolator wincode-free accessor" framing was
wrong: Solana account data is globally readable, so the twap reads the asset-0 `insurance`
u128 STRAIGHT FROM THE MARKET SLAB bytes. The slab's zero-copy MarketGroupV16 header is a
repr(C) Pod of [u8;N] newtypes (align 1, no padding) at MARKET_GROUP_OFF =
HEADER_LEN(16)+WRAPPER_CONFIG_LEN(432)=448; `insurance` sits at +285 within it (after
market_group_id 32 + V16ConfigAccount 233 + asset_slot_capacity 4 + vault 16). So
INSURANCE_OFFSET = 733, pinned by the `insurance_offset_matches_real_percolator_slab` canary
(funds insurance with a unique value via a real Squads TopUp and asserts the slab bytes match;
fails loudly on layout drift).
Fix: pull_surplus now enforces `amount <= insurance - reserved_floor`. `reserved_floor` is a
new twap Config field (the reserved depositor principal), initialized to u128::MAX (so a
freshly-configured twap pulls NOTHING) and lowered only by the DAO via a new Squads-vault-gated,
timelock'd `set_reserved_floor` (tag 4). A permissionless crank can therefore never reach
principal, regardless of percolator's policy mode. The loud SAFETY/ORDERING-DEPENDENCY comments
in pull_surplus/accept_operator are updated to reflect the bound.
Regression: `e2e_finding_o_floor_blocks_principal_drain` (was the known-open demonstration that
the drain SUCCEEDS) now sets the floor = principal and asserts the cranker's principal pull is
REJECTED with no funds moved. `e2e_full_genesis_to_twap_surplus_pull` sets the floor = principal
and pulls exactly the surplus (insurance - floor), leaving principal intact. All real binaries.
twap suite green: lib 2 + chain 10.

### [BLOCKED] E2E probe: the surplus floor cannot be lowered outside the Squads timelock
ATTACK (integrity of the finding-O fix): the reserved_floor is the only barrier between a
permissionless pull_surplus and depositor principal, so re-enabling the drain just means
lowering it. An attacker calls twap.set_reserved_floor DIRECTLY to drop the floor to 0:
(1) signing with their own key passed as the squads vault — rejected (key !=
squads_default_vault(config.squads_multisig)); (2) passing the REAL squads vault but as a
non-signer — rejected (it must be a signer, and the vault PDA can only sign inside a Squads
execute). The floor stays at its u128::MAX default. So the floor is lowerable ONLY through a
timelock'd DAO/Squads execute, exactly like reconfigure/accept_operator. Test:
twap-program/tests/chain.rs `e2e_attacker_cannot_lower_surplus_floor_without_squads`
(real Squads v4 + percolator + twap). KEPT — pins the integrity of the finding-O floor.

### [COVERAGE/SEQUENCING] E2E probe: the operator handoff closes the subledger exit path
The operator handoff to the twap is a POINT OF NO RETURN for the subledger exit. The
subledger insurance_withdraw signs as the pool, which is the asset-0 insurance OPERATOR only
until the handoff; afterwards the operator is the twap, so percolator rejects a pool-signed
WithdrawInsuranceLimited. Proven end-to-end: alice deposits, withdraws fine BEFORE the
handoff, then her withdraw is REJECTED AFTER the operator rotates to the twap. Implication: a
depositor who has not exited before the (1-week-timelock'd) handoff can no longer withdraw via
the subledger. Their principal is NOT stealable — the finding-O floor stops the twap pulling
it — but it is LOCKED until the DAO rotates the operator back to the subledger. This is a
liveness/DOS consideration, not a theft vector: it enforces the design's "exit during the
timelock window" requirement (README §3) and means a malicious DAO can at worst lock (not
steal) a non-exiter's principal, and only after a 1-week public warning. Test:
twap-program/tests/chain.rs `e2e_subledger_exit_blocked_after_operator_handoff`. KEPT — pins
the handoff sequencing boundary against the real binaries.

### [BLOCKED] E2E probe: looping pull_surplus cannot cumulatively cross the floor
ATTACK (finding O fix, cumulative drain): a cranker loops pull_surplus to drain principal in
pieces rather than one over-pull. Because pull_surplus re-reads LIVE asset-0 insurance from
the slab on EVERY call and caps to `insurance - reserved_floor`, successive pulls converge to
the floor and never cross it — even across the percolator withdraw cooldown. Proven
end-to-end: insurance = principal(1,000,000) + surplus(500,000), floor = principal; the cranker
pulls 250k, warps past the cooldown, pulls another 250k (draining exactly the surplus), then a
third pull of even 1 unit is REJECTED, with the full principal intact in insurance and exactly
the surplus in the twap holding. Pins that the floor is stateless/live (no cached insurance, no
cumulative over-pull). Test: twap-program/tests/chain.rs `e2e_floor_holds_across_repeated_pulls`.
KEPT — distinct from the single-pull finding-O test (this covers the looping/cumulative case).

### [BLOCKED] E2E probe: a cranker cannot redirect surplus to its own holding
pull_surplus is PERMISSIONLESS (anyone may crank it), so the destination must be locked to
the twap_authority — otherwise a cranker could pull the surplus into their own wallet (free
money). pull_surplus requires `holding.owner == twap_authority` (the percolator
WithdrawInsuranceLimited dest-owned-by-operator rule, re-checked twap-side). Proven
end-to-end: an attacker cranks pull_surplus with a holding token account THEY own; it is
rejected, no surplus reaches the attacker, and the insurance vault is untouched. So a
permissionless crank can only ever move surplus into a twap_authority-owned account (from
which only the twap program, via the buy/burn slice, can act). Test:
twap-program/tests/chain.rs `e2e_cranker_cannot_redirect_surplus_to_own_holding`. KEPT —
pins the destination boundary of the permissionless pull.

### [FIXED] S. Post-handoff deposits were drainable as "surplus" (LOF)
Found via the e2e probe loop. The handoff rotated only the asset-0 insurance OPERATOR (kind
2) to the twap, leaving the subledger pool as the insurance AUTHORITY (kind 1). So subledger
insurance_deposit (TopUp, gated on kind 1) STILL WORKED after the handoff. Because the twap's
reserved_floor (finding O) is a STATIC snapshot taken at handoff, a deposit made afterwards
raised the live asset-0 insurance ABOVE the floor — turning that new principal into pullable
"surplus". Demonstrated end-to-end against the real binaries: post-handoff a depositor topped
up 500,000, then a permissionless cranker pulled exactly 500,000 (insurance - floor) into the
twap holding — the depositor's entire principal drained.
Fix: twap.accept_operator now ATOMICALLY rotates the insurance authority (kind 1) to the
Squads vault in the same instruction it accepts the operator (kind 2). Both `current` and
`new` are the Squads vault (the asset_admin, propagated from the timelock'd execute), so it
needs no extra consent. After the handoff NOBODY can TopUp market-0 insurance, so no new
(unprotected) principal can enter and the static floor is sound. The subledger is fully
disconnected post-handoff (neither kind 1 nor kind 2) — consistent with "genesis is over".
Regression: twap-program/tests/chain.rs `e2e_post_handoff_deposit_blocked_by_authority_revoke`
(post-handoff deposit is now REJECTED; insurance stays exactly the genesis principal, nothing
drained). e2e_full_genesis_to_twap_surplus_pull still green (the revoke happens after all
genesis deposits). twap suite green: lib 2 + chain 15.

### [BLOCKED] E2E probe: a freshly-deposited position has no vote weight (anti flash-vote)
Vote weight = floor(log2(age)) * principal, where age = now_slot - position.start_slot
(last-write-time). A position deposited in the CURRENT slot has age < 2 → weight 0, and the
gv `vote` instruction rejects a zero-weight vote ("position has no vote weight"). So a voter
cannot flash-deposit, vote with full principal weight, and immediately exit — governance
influence costs real time-at-risk, not just momentary capital. Proven end-to-end against the
real subledger + genesis-vote binaries: alice deposits and votes in the SAME slot (REJECTED),
then after holding a few slots her vote SUCCEEDS. Complements (does not duplicate) the gv lib
unit test of vote_weight() — this exercises the full path: real subledger position ->
weight computation -> vote-instruction rejection. Test: twap-program/tests/chain.rs
`e2e_fresh_position_has_no_vote_weight`. KEPT — pins the time-at-risk requirement of the vote.

### [BLOCKED] E2E probe: pull_surplus is locked to the config's market vault (no cross-market drain)
pull_surplus moves funds out of the market insurance vault, so its SOURCE must be locked to
the config's market — otherwise a cranker could point the WithdrawInsuranceLimited at a
DIFFERENT market's vault/authority and drain another market's insurance. The twap pins
vault_authority == perc_vault_authority(config.market_slab) (and market_slab/percolator_program
== config). Proven end-to-end: a cranker passing a foreign market's vault_authority (derived
for a different slab) is rejected, and the insurance vault is untouched. So a permissionless
crank can only ever withdraw from THIS market's canonical insurance vault. Test:
twap-program/tests/chain.rs `e2e_pull_surplus_rejects_foreign_vault_authority`. KEPT — pins
cross-market source integrity. (Also added the `setup_handoff` harness helper to keep future
twap-side probes focused on the attack.)

### [BLOCKED] E2E probe: one vote, one proposal — cannot back two without retracting
A voter's weight = floor(log2(age)) * principal is their CAPITAL's say; backing more than one
proposal at once would split or double-count that capital across proposals (vote-splitting /
double influence). The gv `vote` enforces one-vote-one-proposal: while a ballot is LIVE on
proposal A, backing a DIFFERENT proposal B is rejected ("retract your existing vote before
backing another proposal") — the voter must retract A first, which frees the weight. Proven
end-to-end against the real subledger + genesis-vote binaries: alice backs A, is REJECTED
backing B, retracts A, then successfully backs B. This invariant was previously untested
anywhere. Test: twap-program/tests/chain.rs `e2e_voter_cannot_back_two_proposals_without_retracting`.
KEPT — pins one-vote-one-proposal. (Also added setup_genesis + register_proposal harness
helpers to keep future genesis-side probes focused.)

### [BLOCKED] E2E probe: no surplus pull before the floor is configured (fail-safe default)
The handoff is several timelock'd Squads executes and the surplus floor is set in its own
step. In the window AFTER the operator rotates to the twap but BEFORE set_reserved_floor — or
if the DAO never sets a floor — reserved_floor is its init default u128::MAX, so pull_surplus
computes surplus = insurance - MAX = 0 and a permissionless cranker can pull NOTHING. Proven
end-to-end: insurance is funded with genuine surplus, the policy + operator are handed to the
twap, the floor is left unset (verified == u128::MAX), and a cranker's pull is rejected with
the insurance untouched. So a handed-off-but-unconfigured twap is safe by default; the
multi-step handoff exposes no funds at any intermediate point. Test:
twap-program/tests/chain.rs `e2e_no_surplus_pull_before_floor_is_configured`. KEPT — pins the
fail-safe default of the floor.

### [COVERAGE/LIVENESS] E2E probe: post-handoff exit lock is RECOVERABLE (never permanent loss)
Extends the handoff-sequencing probe with the recovery half. After the operator rotates to the
twap, a non-exiter's subledger withdraw is blocked (the pool is no longer the operator) — but
the lock is NOT permanent: the DAO, via a timelock'd Squads execute, rotates the insurance
operator+authority BACK to the subledger pool (subledger.accept_operator, pool consents), and
the depositor then exits their principal. Proven end-to-end across the real binaries: exit
works BEFORE the handoff, is REJECTED after, and SUCCEEDS again after the DAO re-grant (the
previously-locked principal is recovered). Confirms the "principal is never permanently lost"
guarantee: the worst case for a non-exiter is a DAO-recoverable lock, never theft or burn. Test:
twap-program/tests/chain.rs `e2e_subledger_exit_blocked_after_operator_handoff` (now the full
exit lifecycle: works -> blocked -> recovered).

### [BLOCKED] E2E probe: minority turnout cannot capture the distribution (quorum guards turnout)
A minority-capital voter tries to seal their proposal by being the ONLY one to vote: they then
hold 100% of CAST weight (so the weighted-majority check passes trivially), but quorum is
total_voted_principal*2 > LIVE pool outstanding — measured against ALL deposited principal,
including non-voters. So a minority of live capital can never reach quorum. Proven with REAL
multi-party deposits against the real subledger + genesis-vote binaries: alice (400k of 1M
outstanding) votes and triggers -> REJECTED (no quorum), the distribution stays unsealed; only
once bob (600k) also votes does the trigger seal the winner. Distinct from the seal.rs
injected-tally quorum test — this exercises the full real path (two deposits -> outstanding ->
vote -> quorum) plus the positive case. Closes the "low-turnout capture" governance attack.
Test: twap-program/tests/chain.rs `e2e_minority_turnout_cannot_reach_quorum`. KEPT.

### [BLOCKED] E2E probe: a voter cannot vote with another voter's position (no vote-power theft)
Voting power is the voter's OWN capital. The gv `vote` derives the subledger position PDA from
the SIGNER (sub_position_seeds(pool, voter)) and pins the passed account to it, so a voter
cannot substitute someone else's (larger) position to vote with their weight. Proven end-to-end
against the real subledger + genesis-vote binaries: alice (100k) signs and passes BOB's (900k)
position account — REJECTED (the PDA derived from alice mismatches the passed account); alice
voting with her OWN position works. Closes the position-substitution / vote-power-theft vector.
Test: twap-program/tests/chain.rs `e2e_voter_cannot_vote_with_another_voters_position`. KEPT.

### [BLOCKED] E2E probe: only the winning proposal's recipients can claim (winner-take-all at claim)
Winner-take-all extends to the distribution claim: once proposal A is sealed as the winner, a
LOSING proposal's recipient gets NOTHING. The claim pins config.sealed_proposal (only the
winner pays) AND entry.pubkey == signer (pull model). Proven end-to-end with TWO real proposals
against the real binaries: a voter backs A to quorum+majority, the trigger seals A; A's named
recipient claims the full COIN supply; B's recipient (the loser) cannot claim from their own
never-sealed proposal B (config.sealed_proposal == A != B) NOR from the winner A (their pubkey
is not an entry there), and ends with zero. Distinct from the single-proposal distribution.rs
claim tests. Closes the "losing-proposal recipient claims anyway" vector. Test:
twap-program/tests/chain.rs `e2e_only_the_winning_proposal_can_be_claimed`. KEPT.

### [BLOCKED] E2E probe: capital dominates hold time — no early-squatter governance capture
Vote weight = floor(log2(age)) * principal: log-time is a SOFT, sub-linear (capped ~63)
multiplier while capital is LINEAR. So an early SMALL depositor cannot sit accumulating
time-weight to out-vote a later LARGE depositor and capture the COIN distribution cheaply.
Proven end-to-end with two competing proposals and real deposits: an early voter (100k held
~1500 slots, floor(log2)=10 -> weight ~1,000,000) backs proposal EARLY; a later voter
(1,000,000 = 10x capital, held ~16 slots, floor(log2)=4 -> weight ~4,000,000) backs proposal
LATE. The early-squatter proposal LACKS a weighted majority (1M*2 <= 5M cast) and cannot seal;
the larger-capital proposal IS the majority and seals. Confirms the Sybil-resistance balance —
capital (the at-risk cost) decides, with hold-time only a tie-tilting bonus. (Stayed inside the
percolator oracle-staleness window so deposits remain Live.) Test: twap-program/tests/chain.rs
`e2e_capital_outweighs_hold_time_no_early_squatter_capture`. KEPT.

### [BLOCKED] E2E probe: retract/re-back cannot inflate vote weight (tally integrity)
A voter cycles back -> retract -> re-back on the same proposal, trying to make their
support_weight accumulate beyond their single capital contribution (a weight-inflation /
double-count attack on governance influence). The gv `vote` subtracts EXACTLY the stored ballot
weight on retract (checked_sub on both the proposal support and the global total_cast_weight)
and re-adds a single fresh contribution on back — it never accumulates. Proven end-to-end:
after the first back, support_weight == total_cast_weight == W (one contribution); across 5
back/retract cycles (age fixed so weight is constant) retract returns both to 0 and re-back
restores exactly W — never 2W. Closes the cycling-inflation vector. Test:
twap-program/tests/chain.rs `e2e_retract_reback_cannot_inflate_vote_weight`. KEPT.

### [STATE] E2E attack-probe coverage map (genesis -> handoff -> twap, all six real binaries)
The probe loop has systematically swept the end-to-end chain. Full repo green (~98 tests):
subledger 6+16+5, genesis-vote 3+5, distribution 4+7, twap lib 24, twap-program lib 2 + chain 24,
setup 1+1 — no regression from this session's reseeds (P/Q/R), accept_operator, or finding O/S.
REAL bugs found + fixed by the loop:
  - O: pull_surplus had no surplus floor (LOF). Fixed by reading asset-0 insurance straight from
    the slab (offset 733, canary-pinned) and capping to insurance - reserved_floor (DAO-set,
    timelock'd, default u128::MAX).
  - S: post-handoff deposits were drainable (the pool kept kind-1 authority). Fixed by
    accept_operator atomically rotating kind 1 to the Squads vault, disabling deposits at handoff.
Boundaries pinned end-to-end (twap-program/tests/chain.rs), all BLOCKED:
  AUTHORITY/HANDOFF: operator grant cannot bypass the Squads timelock; reserved_floor cannot be
    lowered outside the timelock; no surplus pull before a floor is configured (fail-safe MAX
    default); foreign vault_authority rejected (no cross-market drain); cranker cannot redirect
    surplus to its own holding; repeated pulls cannot cumulatively cross the floor; finding-S
    deposit revoke; finding-O floor blocks principal drain.
  LIVENESS: post-handoff subledger exit is blocked but DAO-recoverable (principal never
    permanently lost).
  GOVERNANCE/VOTE: fresh (age<2) position has no weight (anti flash-vote); one vote, one
    proposal (no vote-splitting); minority turnout cannot reach quorum (no low-turnout capture);
    a voter cannot vote with another's position (no vote-power theft); capital dominates soft
    log-time weight (no early-squatter capture); retract/re-back cannot inflate weight.
  CLAIM: only the winning proposal's recipients can claim (winner-take-all at claim).
The high-value external-attack surface on the BUILT code is exhaustively covered. Remaining open
items are design/operational, not code bugs: finding L (impairment first-come vs pro-rata,
awaiting a design decision) and the unbuilt COIN buy/burn settlement slice (future probe target
once built). Future ticks: re-verify on any code change; target new instructions when added.

### [BLOCKED] E2E probe: a completed Squads execute cannot be replayed
The DAO->Squads->percolator handoff is a sequence of timelock'd vault transactions. A replay
attack: after a vault transaction executes (e.g., the asset-0 operator rotation to the twap),
re-run the SAME transaction to re-trigger the timelock'd action without a fresh
proposal/approval/timelock. Squads marks the proposal Executed, so a second execute of the same
transaction is rejected. Proven end-to-end on a fully handed-off market: the operator-handoff
vault transaction (idx 3) is re-executed and REJECTED. Confirms our multi-step handoff is
one-shot per step — a completed timelock'd action can never be replayed. (Our steps are also
idempotent/self-limiting, but this pins the squads replay protection in the integration.) Test:
twap-program/tests/chain.rs `e2e_completed_squads_execute_cannot_be_replayed`. KEPT.

### [BLOCKED/RECOVERY] E2E probe: twap tracks the live floor across impairment + recovery
pull_surplus reads LIVE asset-0 insurance every call, so the floor is enforced dynamically in
BOTH directions. Pinned end-to-end: pull the full surplus down to the floor (insurance ==
reserved_floor == principal); at the floor further pulls are BLOCKED (surplus 0); the DAO then
refills insurance above the floor (a TopUp via the Squads vault, which holds kind-1 post-handoff
— or, equivalently, market profits refilling it); the twap RESUMES pulling exactly the recovered
surplus and STILL cannot cross the floor; it ends back at the floor with the principal fully
intact and total pulled == original surplus + recovered surplus. This is the "recovers after
h<1, then healthy again" behaviour — the floor never lets a permissionless cranker touch
principal whether insurance is dropping toward it or recovering above it, and the twap pre-empts
nothing. Test: twap-program/tests/chain.rs `e2e_twap_resumes_pulling_after_insurance_recovers`.
KEPT.

### [BLOCKED] E2E probe: cannot vote without a subledger position (no free governance power)
Governance power is capital-at-risk: a voter must have a real subledger insurance position to
vote. The fresh-position probe covers a deposited-but-age<2 position (weight 0); this covers the
extreme — an account that NEVER deposited has no position account, so the gv `vote` cannot
read/own-check it (sub_position.owner != config.subledger_program for an uninitialized account)
and rejects. Proven end-to-end against the real subledger + genesis-vote binaries: an attacker
who deposited nothing tries to vote -> rejected; their position PDA is empty. So you cannot buy
governance influence without putting capital at risk. Distinct from e2e_fresh_position_has_no_
vote_weight (account-doesn't-exist vs weight-0 paths). Test: twap-program/tests/chain.rs
`e2e_cannot_vote_without_a_position`. KEPT.

### [BLOCKED] E2E probe: Sybil-splitting capital gives no vote advantage (core resistance property)
The whole premise is "Sybil-resistant governance": influence must scale ONLY with capital
at risk, not with the number of identities. Vote weight = floor(log2(age)) * principal is LINEAR
in principal, so splitting capital across many positions yields the SAME total as one large
position. Proven end-to-end against the real binaries: an attacker splits 1,000,000 into 4
identities of 250,000, all deposited at the same slot and voting the same proposal at the same
age (16, log2=4); the proposal's total support_weight == 4,000,000 == exactly what a single
1,000,000 position would produce, and the quorum denominator (total_voted_principal) == 1,000,000
(summed, not multiplied). So Sybiling neither inflates weight nor quorum — capital is the only
lever. This is the foundational property the design rests on; it was previously unpinned. Test:
twap-program/tests/chain.rs `e2e_sybil_splitting_gives_no_vote_advantage`. KEPT.

### [BLOCKED] E2E probe: exactly half the capital does not meet quorum (strict-inequality edge)
Quorum is total_voted_principal*2 > outstanding — a STRICT inequality. So a voter (or set) holding
EXACTLY half the live capital cannot seal: a 50/50 situation needs strictly MORE than half to have
voted. If this were >= a tie could capture the distribution. Pinned end-to-end against the real
binaries: two equal 500,000 depositors (outstanding 1,000,000); with only one voting (500k*2 ==
1,000,000, not >) the trigger is REJECTED for lack of quorum, and only once the second also votes
(1,000,000*2 > 1,000,000) does it seal. Guards the strict > (vs >=) so a half-and-half split can
never seal. Distinct from the 40%-minority probe (this is the exact 50% edge). Test:
twap-program/tests/chain.rs `e2e_exactly_half_capital_does_not_meet_quorum`. KEPT.

### [BLOCKED] E2E probe: a 50/50 weight tie between proposals deadlocks (majority strict edge)
The winner needs support_weight*2 > total_cast_weight — a STRICT inequality. So two proposals
each holding EXACTLY half of the cast weight TIE: NEITHER can seal. If this were >= both could
seal at 50% (double-seal / ambiguous winner). The tie simply deadlocks until additional weight
breaks it — preserving a single, unambiguous winner-take-all. Pinned end-to-end against the real
binaries: two equal voters (500k each, same age) back competing proposals A and B; triggering A
is rejected AND triggering B is rejected (each has exactly half the cast weight). A third voter
(100k) backs A, tipping it over half; A now has a strict weighted majority and seals as the sole
winner. Complements the quorum strict-edge probe (this is the MAJORITY/cast-weight edge). Test:
twap-program/tests/chain.rs `e2e_tied_weight_between_proposals_deadlocks_until_broken`. KEPT.

### [BLOCKED/DESIGN] E2E probe: a non-voter's exit recomputes quorum (those who stay decide)
Quorum is total_voted_principal*2 > LIVE pool outstanding, so a passive holder's capital counts
AGAINST quorum only while it stays in the pool. The anti-stall property: a large abstainer cannot
indefinitely block finalization — they must either vote or exit, and EXITING shrinks outstanding
and hands the decision to those who stay. Proven end-to-end with a real withdrawal that flips the
trigger: alice (400k = 40%) votes, bob (600k = 60%) abstains -> trigger REJECTED (no quorum); bob
(a non-voter, no vote-lock) withdraws his full 600k principal -> outstanding shrinks to 400k ->
alice is now 100% of the remainder -> trigger SEALS. Distinct from the static minority-turnout
probe (here the abstainer actually leaves). Confirms exits dynamically recompute quorum against
the live pool. Test: twap-program/tests/chain.rs `e2e_non_voter_exit_recomputes_quorum_stayers_decide`.
KEPT.

### [BLOCKED] E2E probe: a non-creator cannot append entries to another's proposal (no injection)
A distribution proposal is built by its CREATOR. append_entries pins header.creator == signer, so
an attacker cannot graft entries (e.g. a self-allocation) onto someone ELSE's proposal — only the
creator can append to it. Otherwise an attacker could inject a payout to themselves onto a popular
proposal before it is voted/sealed. The program-side gate (distribution/src/lib.rs:397) was only
exercised on the positive path (creator appends); the non-creator REJECTION was untested. Pinned
end-to-end: a creator creates a proposal, an attacker's append of a self-allocation is REJECTED,
and the creator's own append succeeds. Complements finding M2 (creator-gated register). Test:
twap-program/tests/chain.rs `e2e_non_creator_cannot_append_to_a_proposal`. KEPT.

### [BLOCKED] E2E probe: no new proposal after the genesis finalizes (one-shot)
The genesis is winner-take-all and one-shot. Once the winning distribution is sealed,
distribution create_proposal rejects (config.is_sealed()), so no NEW proposal can be created on
the finalized config — preventing post-decision clutter or attempts to re-contest a closed
genesis. Proven end-to-end against the real binaries: a voter backs a proposal to quorum +
majority and the trigger seals it; a subsequent create_proposal for a fresh id is REJECTED, and
the sealed winner is unchanged. Complements the seal-irreversibility (finding R / seal.rs) — that
blocks re-sealing, this blocks creating new contenders after the outcome is decided. Test:
twap-program/tests/chain.rs `e2e_no_new_proposal_after_genesis_finalizes`. KEPT.

### [STATE] Coverage map update — distinct attack surface exhausted (33 e2e chain tests)
Full repo green (~107 tests). The e2e harness (twap-program/tests/chain.rs, all six real binaries)
now has 33 tests; every distinct attack class from the probe brief is covered, most from several
angles. Since the last [STATE] entry (at 17 chain tests) the loop added:
 - finding O FIXED (surplus floor via direct slab read + canary) and finding S FIXED (post-handoff
   deposit revoke) — the two real LOFs.
 - handoff/floor: floor-cannot-be-lowered-out-of-band, no-pull-before-floor (fail-safe MAX),
   repeated-pull-cannot-cross-floor, foreign-vault-authority rejected (no cross-market drain),
   cranker-cannot-redirect-surplus, live-floor tracks impairment+recovery, completed-execute
   cannot-be-replayed.
 - liveness: post-handoff exit blocked but DAO-recoverable.
 - governance/economics: anti-flash-vote, no-vote-without-a-position, one-vote-one-proposal,
   minority-turnout / exact-50%-quorum (strict >), 50/50-majority-tie deadlock, non-voter-exit
   recomputes quorum, capital-dominates-log-time (no early squatter), Sybil-split gives no
   advantage, retract/re-back cannot inflate weight.
 - distribution: winner-take-all at claim, no-new-proposal after finalize, non-creator cannot
   append (no injection).
ASSESSMENT: the high-value external-attack surface on the BUILT code is exhaustively covered.
Further ad-hoc probes would be marginal/redundant (reinit guards, capacity bounds, claim-window
edges are already covered by the per-program suites). The right next targets are NOT more probes:
 (1) the unbuilt COIN buy/burn settlement slice (probe once built), and (2) finding L (impairment
 first-come vs pro-rata) which is an open DESIGN decision, not a code bug. Recommend pausing or
 redirecting the 5-minute loop until there is new code to attack.

### [ANALYZED/BLOCKED] deposits_only-policy handoff: the floor protects principal regardless of mode
Investigated a subtle handoff mis-orchestration: the policy rotation to surplus-mode is a SEPARATE
Squads step; what if it is skipped and the twap pulls while the policy is still the genesis
deposits_only (principal-recovery) mode? Read percolator handle_withdraw_insurance_limited
(v16_program.rs ~8542-8558): the withdraw cap is min(max_bps*insurance/1e4, deposit_remaining)
under deposits_only, then bounded by insurance/vault. CONCLUSION: no LOF. The twap's reserved_floor
is an INDEPENDENT, tighter guard applied twap-side BEFORE the CPI — pull_surplus caps `amount` to
insurance - reserved_floor. So the depositor principal sitting in the vault (>= reserved_floor) is
protected whether the policy is deposits_only or surplus-mode; a permissionless cranker can never
pull below the floor in either mode. The policy rotation to surplus-mode is therefore a FUNCTIONAL
requirement (deposits_only caps the operator to deposit_remaining, which is exhausted/zeroed as
depositors exit, so the twap could not pull genuine profits under it), NOT a safety requirement.
Worst case of skipping it: the twap simply cannot pull surplus (functional failure), or percolator's
deposit_remaining accounting drifts under deposits_only — but the actual principal is never lost
(floor-protected) and is recoverable via the DAO re-grant (see the exit-recovery probe). No code
change: the floor (finding O fix, already pinned by e2e_finding_o_floor_blocks_principal_drain and
e2e_twap_resumes_pulling_after_insurance_recovers) is the binding guard and is policy-mode-independent.

### [FIXED] IL. residual-distributor LP/trader cohorts must be scoped to the allow-listed Pyth market
ATTACK (wash-traded reward minting, residual-distributor only — secret branch): the LP cohort rewards
Δ `residual_received_atoms_total` and the trader cohort rewards Δ `residual_crystallized_loss_atoms_total`,
both per-portfolio percolator counters. These are PnL FLOWS — zero-sum and self-dealable. register_start
only checked `portfolio.owner == owner` and (comment) treated `market_group` as "informational". So an
attacker stands up their OWN percolator market (InitMarket as admin, an AUTH_MARK/EWMA/MANUAL oracle they
push), self-trades both sides (delta-neutral) and pushes the mark to manufacture an arbitrary
crystallized_loss on one account and the matching `received` on the other, then registers those portfolios
for the trader/LP cohorts — capturing up to 80% of the COIN supply (the 40/40 LP+trader cohorts) for the
price of trading fees, with their principal preserved (the "loss" is an internal transfer between the two
accounts they own). Manufacturing the loss is risk-free precisely because they control the oracle.
VERDICT: REAL. The user's call: do NOT drop the cohorts — ALLOW-LIST the acceptable (trusted-Pyth) markets
so the attacker can't move the oracle.
FIX (residual-distributor/src/lib.rs):
 - register_start LP/trader branch now enforces `portfolio.provenance.market_group == config.market_group`
   (OFF_PORTFOLIO_MARKET_GROUP = PERC_HEADER_LEN + 0, pinned in tests/offsets.rs vs the real
   PortfolioAccountV16Account.provenance_header.market_group_id). The counters only count when the portfolio
   belongs to the ONE market the orchestrator vetted as trusted-Pyth at init.
 - init now rejects market_group == default whenever the LP or trader cohort is active (lp_bps>0 or trader
   remainder>0), so those cohorts can never be left "any market".
RESIDUAL RISK (accepted): within an allow-listed Pyth market a delta-neutral self-trade still captures real
price drift for fees within the finalize window — the allow-list removes the *risk-free, oracle-controlled*
path (which was the whole-supply-for-fees attack), not 100% of farming. The orchestrator MUST point
config.market_group at a market whose oracle the public cannot control (no permissionless self-oracle'd
assets); insurance/backing cohorts are unaffected (share-value = real capital at risk).
Tests: residual-distributor/tests/e2e.rs `register_rejects_portfolio_from_a_foreign_market` (foreign-market
portfolio rejected for both cohorts even with fat counters + owner match; genesis-market portfolio accepted);
chain.rs e2e portfolio mocks now set market_group@16. lib 7 + e2e 6 + offsets 3 + chain 83 green.

### [FIXED] KM. residual-distributor: a share-value claim must be authorized by its own owner
ATTACK (targeted grief / value loss, residual-distributor only — secret branch): `claim` was fully
permissionless (any `cranker` signer). For the insurance/backing (share-value) cohorts it caps the payout by
the LIVE subledger Position shares read AT claim time, then sets the irreversible `stake.claimed` flag. A
subledger PARTIAL insurance withdraw leaves the Position `withdrawn=false` with `shares` reduced but non-zero
(subledger process_insurance_withdraw). So an attacker watches for a victim mid partial-withdraw (a normal
rebalance) and force-calls `claim` for the victim's stake at that transient low-share instant: the live cap
pays only the reduced amount, `claimed` locks, and the forfeited remainder is stranded in the vault (no
redistribution in the self-service path). Repeatable against any insurance/backing backer who ever
partial-withdraws post-freeze; attacker cost = tx fees. (LP/trader unaffected — they pay frozen points with
no live cap, so a forced claim just pays them correctly.) The COIN still goes to the bound recipient (GY), so
this is value-destruction/grief, not theft.
VERDICT: REAL (medium severity — targeted, needs the victim transiently low at the attacker's chosen slot).
FIX (residual-distributor/src/lib.rs claim): for COHORT_INSURANCE|COHORT_BACKING require `cranker == stake.owner`
(the depositor who controls the shares and bears the soft-veto timing). The soft-veto-on-exit semantics are
preserved (the owner still forfeits if THEY exit before claiming), but the claim slot is no longer
attacker-chosen. LP/trader stay permissionless. Tests: e2e.rs
`share_value_claim_cannot_be_forced_by_a_third_party_at_a_low_share_moment` (attacker forced claim rejected at
shares=30; victim re-deposits to 300 and claims full 100_000); claim helper now authorizes as the owner;
chain.rs insurance claim (alice) now owner-signed. lib 7 + e2e 7 + offsets 3 + chain 84 green.

### [FIXED] KN. twap-program reconfigure must hold the auction+savings <= 100% invariant (master/stack)
ROBUSTNESS (DAO footgun, twap-program — PUSHABLE STACK, not the distributor): `process_set_economics`
enforces `surplus_buy_burn_bps + base_unit_savings_bps <= 10_000` (the floor-protection invariant), but
`process_reconfigure` set `surplus_buy_burn_bps` alone and only checked `<= 10_000`. So a valid-looking,
timelock'd DAO reconfigure could raise the burn share above `10_000 - savings_bps`, after which every
permissionless `execute` underflows at `retained = surplus - burnable - savings` and reverts — permanently
bricking the surplus auction until a corrective reconfigure. Not externally exploitable (DAO-gated) and
principal is never at risk (execute reverts BEFORE any pull), but it is a real consistency gap that can wedge
the auction. NOTE: the agent audit otherwise found NO new external-attacker LOF/DOS in the pushable stack
(subledger/genesis-vote/distribution/twap-program) — it is heavily hardened (A–BX + H*/I* findings).
FIX (twap-program/src/lib.rs process_reconfigure): add the same
`new_bps + base_unit_savings_bps <= 10_000` check after the Squads gate, matching set_economics. Test:
chain.rs `reconfigure_must_hold_the_auction_plus_savings_invariant` (auction->4000, savings->5000, then
reconfigure->6000 rejected (sum 11000), boundary reconfigure->5000 accepted (sum 10000)), real Squads execute
vs the real twap .so. chain 84 green. SEPARABILITY: the fix is twap-program/src/lib.rs ONLY (identical on
master), committed alone so it cherry-picks to master cleanly without the distributor; the test + this log
entry stay on the secret branch.

### [FIXED] KO. residual-distributor: share-value crystallize must be owner-authorized (KM parity, one step earlier)
ATTACK (targeted grief, residual-distributor only — secret branch): KM owner-gated the share-value CLAIM, but
`crystallize` was left permissionless and is the stronger lever. crystallize OVERWRITES a share-value stake's
`points` from the LIVE Position shares read NOW (`*slot = slot - old + new; stake.points = new`), and `freeze`
then snapshots that as the frozen denominator term — which the claim-time min-cap (min(points, live)) can only
ever LOWER, never raise. So an attacker (any signer, tx fees only) calls crystallize on a VICTIM's
insurance/backing stake while the victim is at a transient low-share moment (mid partial insurance-withdraw:
withdrawn=false, shares reduced), force-setting the victim's points down, then front-runs the victim's recovery
with the permissionless `freeze` to lock it. The victim's COIN share is then permanently capped low even after
they restore shares; the forfeited remainder is stranded. Catchable on every insurance/backing backer that
ever reduces shares before freeze.
VERDICT: REAL (high confidence — same precondition/irreversibility as KM, applied to the write path).
FIX (residual-distributor/src/lib.rs crystallize, COHORT_INSURANCE|COHORT_BACKING branch): require
`cranker == stake.owner` (KM parity). LP/trader stay permissionless — their counters are monotonic, so a forced
crystallize can only RAISE the Δ, never grief. Tests: e2e.rs
`share_value_crystallize_cannot_be_forced_by_a_third_party_at_a_low_share_moment` (attacker forced crystallize at
shares=30 rejected; owner re-crystallizes at 300, freezes, claims full 100_000); crystallize helper now
authorizes as the owner; chain.rs insurance crystallize (alice) now owner-signed. lib 7 + e2e 8 + offsets 3 +
chain 84 green. NOTE: the paired stack audit this iteration found NO new external LOF/DOS (the
subledger/genesis-vote/distribution/twap-program surface remains hardened; closest seam examined — the
POLICY_WITH_SURPLUS partial-withdraw share math — was fuzzed conservation-safe).

### [STATE/CLEAN] Dual-loop tick — 3rd stack pass + 3rd distributor pass: nothing new exploitable
Both surfaces audited fresh this tick (read-only subagents + spot-checks); no new LOF/DOS found.

STACK (subledger/genesis-vote/distribution/twap-program): went deep on the untrodden arithmetic — the TWAP
uniform-price (Dutch) clearing is provably COIN-conservative (Σ refund + total_coin = Σ escrowed; coin_i =
usd_i*cm/um <= c for every bid, so no over-withdraw/underflow) and USD-bounded (total_usd <= budget <=
holding), with the settled/open roll-reset (AE) and canonical-ATA payout pins intact. Closest-but-blocked:
(1) execute savings pull does not assert savings_dest != holding — a Squads-timelocked captured DAO could fold
its own savings back into the auction budget; protocol-owned both sides, floor still subtracts first, no
depositor LOF (timelock foot-gun, not a vuln); (2) subledger insurance slab read u128->u64 unwrap_or(MAX) —
unreachable (balances are u64; a clamp only raises a depositor's own impaired payout, bounded by amount);
(3) distribution burn_unclaimed burns donated COIN too — post-window, donor self-harm, solvency invariant
guarantees legit recipients paid first.

DISTRIBUTOR (residual-distributor): Σ-claims <= supply holds globally (4 cohort bps sum to 10000; per-cohort
numerator <= frozen denom; share-value min-cap only lowers; single rd_config-PDA vault, only spend path is the
claim transfer). One-stake-per-(config,owner) blocks cross-cohort/same-portfolio double-count. The old seal
CPI seam is fully retired (no live invoke_signed but system-create + SPL claim transfer). Freeze vault binding
sound (no mint/freeze authority, supply==total_supply, config-owned full-supply vault — no fundable decoy).
Closest-but-blocked: (1) the BackingDomainLedger reward path (read_backing_counters / OFF_BACKING_* /
insurance_points / read_subledger_position / window_points / fee_supported_eligible) is ENTIRELY DEAD CODE —
referenced only in unit tests, never in a handler (the live backing cohort reads subledger Position.shares), so
the offset-confusion angle on it is UNREACHABLE; (2) attacker-scoped init keys = the atomic-init-squat that is
moot (one atomic genesis tx) + HC/HK binding; (3) soft-veto residue under-allocation is intended + conservative
(Σ < supply), and the forced-low-share grief is already closed by KO+KM owner-gates.

HYGIENE NOTE (not a vuln, candidate for a future dedicated tick): delete the dead BackingDomainLedger reward
reads + their OFF_BACKING_* consts and the now-misleading offset pins in tests/offsets.rs to shrink the audit
surface. Left in place this tick to keep it small/green.

### [STATE/CLEAN+HYGIENE] Dual-loop tick — 4th stack pass clean; distributor dead-code excised
STACK (4th pass, subledger/genesis-vote/distribution/twap-program): nothing new exploitable. Deep-dove four
fresh angles, all soundly closed: (1) gv vote-lock <-> subledger withdraw — the SetVoteLock CPI is the last
step under `?` (atomic with the ballot mutation), requires BOTH the gv-config vote_authority AND the position
owner to sign, and there is NO subledger close/dealloc ix, so a lock can never be stranded (retract always
clears it); (2) top-up-while-voted does NOT auto-update the stale ballot snapshot (tallies only move on an
explicit re-back that re-reads fresh principal) — no free weight; (3) distribution is a flat append-only list
(no top-N/sort/eviction), each index claimed independently, bounded by the seal's (entry_count,total_amount)
snapshot and total_amount<=total_supply; (4) twap place_bid/evict/cancel/settle slot reuse — place requires
BOOK_STATE_OPEN so eviction never touches a SETTLED slot, cancel rejects SETTLED slots, the roll fully resets
owed/refund/settled (AE), eviction always refunds the full u64-bounded escrow to the recorded canonical ATA.
No fix -> no master push.

DISTRIBUTOR (hygiene, secret branch): excised the retired old-insurance-model + BackingDomainLedger reward
dead code flagged last tick (unreachable — never called by any handler; the live insurance/backing cohorts read
subledger Position.shares via read_subledger_shares, LP/trader read PortfolioAccount via read_portfolio_residual).
Removed: floor_log2, fee_supported_eligible, window_points, read_backing_counters, insurance_points,
read_subledger_position; consts OFF_BACKING_MARKET_GROUP/OFF_BACKING_AUTHORITY/OFF_TOTAL_PRINCIPAL/
OFF_TOTAL_EARNINGS/OFF_CUMULATIVE_LOSS, SUB_POS_PRINCIPAL/SUB_POS_START_SLOT; their unit tests; and the
offsets.rs backing-ledger pin block (kept the DISTRIBUTION_PROGRAM_ID pin (HK), the PERC_HEADER_LEN==16 pin, the
portfolio pins, and the live subledger pool/owner/withdrawn/shares pins). Net -144 LOC. Pure dead-code removal —
the .so is functionally identical; no handler logic changed. lib 3 + e2e 8 + offsets 3 + chain 84 green.

### [STATE/CLEAN + COVERAGE] Dual-loop tick — stack clean; pinned the distributor freeze GX/EZ guards
STACK: nothing new (5th look). Spot-checked that the key exit boundary is already pinned by a real-binary
test — `subledger/tests/insurance_percolator.rs::vote_locked_principal_cannot_exit_until_retracted` (a
vote-locked position cannot withdraw until the voter retracts, which clears the lock via the gv->subledger
SetVoteLock CPI). No code change -> no master push.

DISTRIBUTOR (coverage, secret branch): the freeze GX/EZ guards (the COIN vault is bound at freeze and must be
rd_config-owned + hold the full fixed supply, with the coin_mint carrying NO mint authority and NO freeze
authority) previously had only the happy path tested — and the src comment even cited a
`set_authority_clears_delegate_no_vault_rug` test that NEVER EXISTED. Added the negatives as a real-binary
litesvm test `freeze_enforces_fixed_supply_and_vault_integrity` (each case its own mint so the global
mint.supply check isolates the guard): (1) live MINT authority rejected (could inflate supply under claimers),
then accepted after revoke; (2) live FREEZE authority rejected (could freeze/censor claimers' COIN), then
accepted after clearing it; (3) a fully-funded but NON-rd-owned vault rejected; (4) an rd-owned but
UNDER-funded vault rejected. Fixed the stale comment to point at the new test. No behavior change — pins the
existing guards (BLOCKED, as designed). lib 3 + e2e 9 + offsets 3 + chain 84 green.

### [COVERAGE] Dual-loop tick — distributor cross-cohort conservation property pin
DISTRIBUTOR (secret branch): added `cross_cohort_claims_never_exceed_cohort_or_total_supply` — a multi-stake,
all-four-cohort claim against the real rd .so with deliberately NON-even point splits (e.g. 3 equal insurance
shares over denom 3; lp denom 4007) so floor rounding leaves dust. Pins the core conservation invariant:
per-cohort Σ claims <= cohort_supply, total Σ <= fixed supply, and the vault is drained by EXACTLY the claimed
total (supply - total) — never over-drawn — with at least one cohort strictly under its supply (dust proof).
Complements the reasoning-level "Σ-claims<=supply" audit note with a real-binary regression. No code change
(BLOCKED-by-design conservation holds). e2e 10 green.
(Stack iteration this tick: a focused numeric/boundary audit of twap-program + distribution arithmetic — the
continued-fraction rate comparator, marginal-clearing rounding, eviction-at-MAX_BIDS, distribution claim index
math — is running; verdict recorded next.)

### [CLEAN] Dual-loop tick — stack numeric/boundary deep-pass (twap-program + distribution): nothing new
Narrow-and-deep audit of the two arithmetic-heavy stack files, below the GP/GH/GI high-level proofs. CLEAN.
 - twap `cmp_rate` continued-fraction comparator (lib.rs:854-879) is a consistent total order (Euclidean
   reciprocal-flip; all (ar==0,br==0) terminal quadrants + the `reversed` toggle verified by hand); its only
   caller passes c>0,u>0 (the c==0||u==0 skip) and reserve_den>0, so no div-by-zero; reserve_num==0 passes all.
 - twap marginal clearing (lib.rs:1641-1666): um>0,cm>0 (marginal is eligible); usd_i*cm < 2^128 (u64 legs) so
   no mul_div_floor overflow; refund = c - coin_i >= 0; budget==0 -> clean roll; COIN conserved (sold+refund ==
   escrow per bid), USD conserved (total_usd <= Σfills <= budget); as_u64(total_coin/total_usd) can't revert
   (bounded by real u64 SPL escrow/holding balances). The one reachable edge — a strictly-better fully-filled
   bid rounding to coin_i==0 (e.g. marginal 1/1000, bid 2/1) — is fund-safe: it takes the else branch, usd_owed
   reset to 0, full COIN refunded, its USD excluded from total_usd; no USD paid for zero COIN (documented GP).
 - place_bid eviction at MAX_BIDS rejects a bid merely EQUAL to the weakest (strictly-better only, no churn);
   distribution append (amount>0, pk!=default, entry_count<capacity, checked_add, total_amount<=total_supply) +
   claim (index<entry_count, pk==recipient, amount!=0 replay-guard, entry_offset always in-bounds) are sound.
No code change -> no master push. The pushable stack's numeric surface is saturated.

### [CLEAN + COVERAGE] Dual-loop tick — stack saturated; distributor self-service lifecycle guards pinned
STACK: nothing new. Confirmed the execute ratchet is already pinned (chain.rs
e2e_execute_pulls_only_burn_share_and_ratchets_principal) and the twap/distribution numeric surface was
deep-passed clean last tick. No code change -> no master push.

DISTRIBUTOR (coverage, secret branch): the self-service finalize lifecycle guards previously had only
claim-before-freeze tested. Added `self_service_lifecycle_guards_freeze_window_and_post_freeze_closure`
(real rd .so) pinning: (1) freeze BEFORE emission_end+finalize_window rejected (else a permissionless caller
freezes early and forfeits slow backers' un-crystallized points); (2) register CLOSED after freeze (else the
frozen denominator is diluted by a late stake); (3) crystallize CLOSED after freeze (else the denominator is
altered post-snapshot); (4) double-freeze rejected (snapshot + bound vault immutable). No behavior change —
BLOCKED-by-design guards now have regression coverage. e2e 11 green.

### [CLEAN + COVERAGE] Dual-loop tick — stack saturated; distributor register guards pinned
STACK: nothing new (8th look; numeric surface + execute ratchet + lifecycle already pinned). No master push.
DISTRIBUTOR (coverage, secret branch): pinned register_start guards distinct from the existing
foreign-owner/pool/market tests — `register_rejects_out_of_range_cohort_cross_program_and_double_register`
(real rd .so): (1) cohort > COHORT_TRADER rejected; (2) CROSS-PROGRAM type confusion rejected — an
insurance/backing cohort pointed at a percolator-owned account, or an LP/trader cohort at a subledger
position, is rejected by the owner-PROGRAM check (so the wrong struct is never read at the bound offsets,
concretely confirming the earlier "luck-blocked kind-confusion" reasoning is actually program-gated);
(3) double-register for the same owner (stake PDA already initialized) rejected. No behavior change. e2e 12 green.

### [CLEAN + COVERAGE] Dual-loop tick — stack saturated; distributor claim anti-redirect pinned
STACK: nothing new (9th look). No master push.
DISTRIBUTOR (coverage, secret branch): pinned the claim-layer anti-theft boundary (GY at claim). LP/trader
claim is PERMISSIONLESS, so the regression `claim_cannot_be_redirected_or_paid_from_a_decoy_vault` (real rd
.so) confirms a third-party cranker CANNOT (a) redirect the COIN to an ata it owns (ra.owner != stake.recipient
-> reject) nor (b) pay from a decoy vault (vault.key != config.vault -> reject); the control proves ANY cranker
may finalize the claim but ONLY into the bound recipient from the real vault. No behavior change. e2e 13 green.

### [CLEAN] Dual-loop tick — both surfaces saturated (verified, no new vector)
STACK: spot-checked the finding-II monotonic surplus-floor guard (twap set_reserved_floor:621 — once a REAL
floor is set, even a timelock'd Squads execute cannot LOWER it; the post-handoff captured-DAO principal-drain
backstop). It IS already pinned on master by `dao_cannot_lower_the_surplus_floor_to_drain_principal`
(master chain.rs:6868); the secret chain.rs carries the complementary `e2e_attacker_cannot_lower_surplus_
floor_without_squads`. No gap, no code change, no master push.
DISTRIBUTOR: no new vector. Over the last ticks the full lifecycle now has negative-test coverage — register
(foreign owner/pool/market, out-of-range cohort, cross-program type confusion, double-register), crystallize
(KO owner-gate), freeze (GX/EZ vault+mint integrity, window guard, post-freeze closure, double-freeze), claim
(KM owner-gate, anti-redirect, decoy-vault, double-claim, before-freeze), and cross-cohort conservation.
SATURATION: this is the 10th tick; 9 clean stack adversarial passes + the distributor hardened & fully pinned.
New findings are exhausted; further ticks yield marginal/tautological coverage. RECOMMEND throttling the 5-min
cadence or repointing the loop (e.g. a property/fuzz harness, or a specific subsystem) — per the loop's own
"don't invent a finding" guidance, ticks now record clean rather than add marginal tests.

### [CLEAN + HEALTH] Dual-loop tick — surfaces saturated; full-suite green health check
STACK: spot-checked the classic ERC4626 first-depositor inflation/donation vector — already blocked + pinned
(finding HU `first_depositor_inflation_attack_cannot_skim_a_later_depositor`, VIRTUAL_SHARES=1_000_000 bounds
the skim to ~amount/VIRTUAL_SHARES; dust-to-offset tests at subledger.rs:276-314). No gap. DISTRIBUTOR: no new
vector (lifecycle/access/conservation fully pinned).
FULL-SUITE HEALTH CHECK (no drift after 10+ ticks): all five .so build clean; tests all green —
subledger 62, genesis-vote 21, distribution 26, residual-distributor 19, twap-program 88 (= 216 total).
No code change, no master push. Saturation stands; recommend throttling/repointing the loop.

### [COVERAGE — keystone handoff binding] Dual-loop tick — accept_operator market/percolator binding pinned
STACK (twap-program; the most security-critical authority transfer): accept_operator rotates asset-0's
insurance operator to the twap_authority. The handler binds it to the config's OWN market + percolator program
(src ~line 663: market_slab.key == config.market_slab && percolator_program.key == config.percolator_program),
so even a fully-approved, timelock'd Squads execute cannot rotate the operator on a FOREIGN market or via a
fake percolator program. Master had only the happy-path handoff test + the non-vault-signer gate; this binding
was UNTESTED. Added `handoff_rejects_a_substituted_market_or_percolator_program` (real squads+twap+percolator):
(1) substituted percolator program rejected; (2) substituted market rejected; (3) correct accounts rotate the
operator (control). No code change (guard already exists). Lands on master (the guard is master code). secret
chain 85 green.
DISTRIBUTOR: no new vector this tick (lifecycle/access/conservation fully pinned).

### [STATE/ISSUE — master chain.rs is distributor-coupled (inherited, needs decision)] 
While porting the handoff-binding test to master this tick, discovered that origin/master's
twap-program/tests/chain.rs is ALREADY distributor-coupled: it contains
`e2e_market_genesis_traders_residual_decider_then_handoff_twap` and references the residual-distributor by
HARDCODED pubkey + .so path (`rd_id()` = a literal Res1dua1Distr1butor… pubkey, `rd_so()` =
so_deploy("residual_distributor")) — NOT via a crate `use`, so it compiles without the crate. This was pushed
in a PRIOR session (present at 60b02cd and f88623f), not this tick.
SAFETY: the residual-distributor CRATE/source is NOT on origin/master (Cargo.toml has no member; the crate
dir is absent) — the secret IP is not leaked. But the rd-decider TEST on master cannot run in a clean checkout
(no residual_distributor.so is built there), so master's chain suite is fragile/red for that one test unless a
stale rd .so happens to be present locally.
IMPACT: this contradicts the "stack/master carries no distributor coupling" intent. It is a pre-existing leak
of the test SHAPE (not the crate). NEEDS A USER DECISION — either (a) remove the rd-decider test (and the
rd_id/rd_so/rd_config helpers) from master's chain.rs so master is cleanly distributor-free, or (b) accept the
coupling as intentional. I did NOT autonomously rewrite master's chain.rs.
NO CHANGE pushed to master this tick (local master == origin/master == 44a1d5b, untouched). The
accept_operator market/percolator binding test (this tick's work) is pinned on the secret branch (6bfe594);
porting it to master is deferred pending the above decision.

### [COVERAGE] Dual-loop tick — distributor init-validation guards pinned (incl. IL init guard)
DISTRIBUTOR (secret branch): the init validators had no negative test. Added
`init_rejects_zero_supply_overallocation_and_unscoped_cohorts` (real rd .so): rejects zero supply; a cohort
bps sum > 100%; an active insurance/backing cohort with no pool scope; and (finding IL, init layer) an active
LP/trader cohort with a default market_group — the complement to the register-side IL test. Fully-scoped config
accepted (control). No behavior change. e2e 14 green.
STACK: no new vector. The keystone accept_operator binding was pinned on secret last tick (6bfe594); its master
port remains BLOCKED on the user's decision re: the inherited distributor-coupled master chain.rs (see prior
entry b8acc9b). No master push this tick (master == 44a1d5b, untouched).

### [VERIFIED — subledger accept_operator market/perc binding sound; rigorous test deferred] 
STACK (subledger): the pool-side accept_operator (the genesis grant: pool consents to receive asset-0
insurance authority+operator) binds to the pool's OWN refs — src line ~1394: `market_slab.key ==
pool.market_slab && percolator_program.key == pool.percolator_program`, plus a pool-PDA re-derivation
(1399-1405) so the signing seeds are trusted. Parallel to the twap accept_operator binding pinned last tick
(6bfe594). VERDICT: sound by code reading.
WHY NO NEW TEST: insurance_percolator.rs builds the market with marketauth = pool_pda, so asset-0's asset_admin
is the pool PDA (not an external signer). A foreign-market accept_operator call in that Env fails at the
percolator UpdateAssetAuthority CPI (the test signer isn't the real asset_admin) REGARDLESS of line 1394 — so a
negative test there would be TAUTOLOGICAL (passes whether or not the guard exists) and is intentionally NOT
added (loop guidance: delete marginal/tautological). A rigorous negative requires the chain.rs grand-unified
harness where the Squads VAULT is asset_admin (so the correct-binding grant actually succeeds and only the
substituted one fails) — that lives in the diverged chain.rs and is deferred with the master-coupling decision
(entry b8acc9b). The positive path is already covered by chain.rs e2e_squads_grants_operator_to_subledger_then_
real_deposit. No code change; master untouched (== 44a1d5b).
