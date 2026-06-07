# residual-distributor — design (branch `risidual_genesis_never_push_upstream`, DO NOT PUSH)

Deterministic, points-based COIN distribution decider. Replaces winner-take-all voting
(`genesis-vote`) behind the `distribution` program's pluggable-decider seam. Two cohorts of
the fixed COIN supply, each split pro-rata to Sybil/wash/JIT-resistant points:

- 20% insurance deposits      (capital-at-risk * time)
- 80% residual-backing capital (eligible-loss-absorbed, fee-capped, log-time weighted)

## Anti-capture stack (defence in depth, weakest-to-strongest)

1. **Weak log-time weight** `floor(log2(hold))` — a late whale is only ~1.15x behind an early
   backer. Necessary but not sufficient.
2. **Fee-support cap** `eligible = min(Δresidual, Δfee*10000/80bps)` — farming costs sunk fees;
   recycled-loss self-dealing is net-lossy. Conservation is enforced by percolator at the sink
   (residual_received = cumulative_loss_atoms can never exceed real losses).
3. **JIT damping** — points use the farm-side hold window, so a 1-slot sniper earns ~0.
4. **SOFT VETO (the teeth).** Insurance runs POLICY_WITH_SURPLUS, no lock: a depositor may exit
   ANY TIME taking principal + pro-rata fee surplus, FORFEITING their COIN share. So if an
   attacker farms points to capture the COIN (and thus the surplus), honest insurance need not
   out-farm him — they EXIT WITH THE SURPLUS and he captures COIN over an empty pool. Capture is
   a Pyrrhic win: the point math makes farming expensive; the soft veto makes a successful farm
   WORTHLESS. Governance (COIN) is decoupled from value (surplus); the value can always walk.

## What the soft veto requires of this program

- An exited insurance position MUST forfeit its points: the seal must not allocate COIN to a
  position that has withdrawn. Mechanism: exit (subledger withdraw) invalidates the PointStake,
  or the seal cross-checks the live position and skips/zeros withdrawn ones. Forfeited COIN is
  not minted (floor rounding / unallocated supply is burned by distribution's burn_unclaimed).
- Symmetric for residual-backers if they exit before crystallization (their delta simply never
  accrues — handled already: no live backing ⇒ no Δresidual ⇒ no points).

## Trust / determinism
- `IX_SEAL` re-derives every distribution entry from on-chain PointStakes and refuses to seal
  unless `(recipient, amount) == (stake.recipient, floor(total_supply*points/total_points))`.
  Nothing is trusted; a cranker can only seal the one deterministic distribution.
- percolator stays ledger-free: this program snapshot-deltas its monotonic counters
  (residual_received / total_earnings / total_principal). Offsets are PLACEHOLDERS — pin with
  offset_of! (finding-T) before mainnet; finding GT is the cautionary tale.

## Status
- Done + unit-tested: point math (log2 / fee cap / window / pro-rata); Config + Stake state;
  init / register_start / crystallize / verify-then-seal; distribution seal CPI.
- Done + e2e (real distribution binary): register -> crystallize (snapshot-delta of a mock
  percolator backing-ledger) -> cranker builds the deterministic proposal -> decider verify-then
  -seals via CPI -> recipient claims exactly its pro-rata share.
- Done + unit-tested: SOFT-VETO forfeiture. `insurance_points(seal_slot, principal, start_slot,
  withdrawn)` reads the LIVE subledger position; a withdrawn / zero-principal position yields 0,
  so a depositor that exited with the surplus forfeits its COIN (the share is never allocated and
  is burned as unclaimed by distribution::burn_unclaimed). `read_subledger_position` reads the
  stable Position offsets (principal@72 / withdrawn@88 / start_slot@89).
- Done: insurance-cohort seal path. Supply splits insurance_bps/residual (default 20/80); insurance
  points = capital*log-time crystallized from the LIVE subledger position into an authoritative
  insurance_total_points (subtract-old/add-new); seal verifies each insurance entry against it AND
  reads the live position to FORFEIT (amount must be 0) a withdrawn depositor — the forfeited share
  stays in the total and is burned as unclaimed, never redistributed. e2e: insurance_cohort_split_and_exit_forfeiture.
- Done: percolator BackingDomainLedger offsets pinned with offset_of! (tests/offsets.rs).
