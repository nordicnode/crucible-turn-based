# CONTRACT — crucible-evo

Pure training logic: the (μ+λ) evolution strategy, fitness, the bootstrap
curriculum, lineage, ghosts, the champion gauntlet, Elo, change reports, and
the balance harness. Depends on `crucible-sim` and `crucible-ai` only.
**Status: M4 (population + fitness + curriculum) + M5 (gauntlet, lineage,
league/Elo, change reports) + M7 (ghosts, ghost pool, ghost fitness) + M8
(balance harness + baseline) implemented.**

## 1. Purity boundary

`crucible-evo` MUST NOT do IO, spawn threads, or read the clock. It computes:
given a population, an evaluation set, and match results, produce the next
population / fitness / lineage updates. The server injects storage, scheduling,
parallelism (rayon), and match execution.

## 2. Evolution strategy contract

- (μ+λ) ES, mutation-only in v1 (no crossover — keeps lineage trees clean):
  population 64, μ = top 16 retained, λ = 48 offspring via Gaussian mutation,
  σ annealed 0.02 → 0.005 by generation, 10% macromutation rate.
- Fitness per genome = mean over the evaluation set:
  win +1.0 / draw +0.1 / loss −1.0, plus margin shaping
  `0.25 × (own_remaining − enemy_remaining) / total`, minus anti-rush damping
  `0.2` if the match ends < 2 min. Exact weights live in `config.toml` and are
  injected, not hardcoded.
- Evaluation set per generation: 8 matches/genome — 4 self-play vs sampled
  population, 2 vs champion, 2 vs ghosts — with both spawn sides played.
- Every match run is seeded and **reproducible**; seeds + genome ids are logged
  per match (see §5).

## 2.5 Bootstrap curriculum

- `curriculum.rs` drives a cold-start population through staged shaping before
  the self-play league: economy (ore mined) → production (army value) → combat
  (vs idle) → scripted easy/medium/hard → **scripted gauntlet** (a final stage
  whose fitness is the mean shaped score vs easy+medium+hard, so the champion
  is not overfit to the last bot it trained on). Each stage runs a bounded
  number of generations and then advances; the whole schedule is a fixed,
  reproducible budget.
- The CI test `curriculum_converges_to_beating_hard` pins the budget
  (pop 16, μ 4, σ 0.05, 2 gens/stage, 2 seeds/gen, 2-min match cap) and
  asserts the final genome beats `hard` **≥ 90% over 32 held-out maps**
  (both spawn sides). Measured: 14 generations → easy 100% / medium 98.4% /
  hard 100% at the pinned master seed 20240818.
- **The bootstrap match cap is 2 minutes** (`bootstrap_match_timeout_ticks`),
  separate from the self-play/league cap. The curriculum only converges to
  beat `hard` at short caps; at the full 6-min league cap the same budget
  produces a rush specialist that loses to `hard` ~75% of the time. The
  full-length meta is the self-play/ghost league's job, not the bootstrap's.
- `crucible-server` runs this curriculum on a cold start when
  `CRUCIBLE_TRAINER_BOOTSTRAP=1`, and **refuses to crown** unless the
  bootstrapped champion beats `hard` ≥ 90% on 32 held-out maps (a structural
  gate in `bootstrap_cold`, not just a test).
- **Known gap:** the plan's stronger regression bar — *every* champion beats
  all three scripted bots ≥ 90% — is **not yet enforceable**. The bootstrap
  champion is a rush specialist and its easy/medium rates are seed-dependent
  (across 5 master seeds: easy 0–100%, medium 43–98% at the CI budget).
  Enforcing that bar needs a stronger curriculum (larger population / more
  medium-specific budget / a non-rush champion), not a one-line assertion.

## 3. Champion gating (the gauntlet)

- A generation winner is a *challenger*, not yet champion. Promotion requires:
  - ≥ 55% over 40 matches vs the reigning champion (20 seeds × both sides);
  - ≥ 50% aggregate over 20 matches vs 4 sampled historical champions.
- The champion genome is immutable until dethroned. On promotion the old
  champion moves to the Museum, lineage is updated, Elo recalculated, and a
  change report is generated.
- The gauntlet protocol is a pure function of (challenger, champion set, seeds,
  match executor); the result must be deterministic given those inputs.

## 4. Ghosts

- A ghost replays the *human side* of a recorded match: a frozen policy
  (deterministic function of `(tick, fog_view) -> commands`) reconstructed from
  the replay's command log. Same inputs ⇒ same commands (immutability).
- Ghost pool policy: keep last N=200 human matches + all matches that beat a
  champion + curator-pinned classics; recent ghosts weighted higher. Tunable,
  injected via config.

## 5. Reproducibility & lineage

- Lineage records ancestry (`parent_id`, generation, born_from) so any genome's
  descent is queryable.
- Elo (K=24, draws handled) applies to every genome with ≥ 10 league matches.
  Champion Elo history is the headline metric; a self-play floor check
  (≥ 70% vs the hard bot) raises a regression alarm if violated.
- **Every gauntlet/league match is logged with seeds and genome ids.** A
  promotion that cannot be reproduced from those logs is a determinism bug.

## 6. Balance harness (M8)

- `balance.rs` runs batch headless matchups and aggregates win rates
  (`counter_matrix`, `bot_tiers`, `balance_table`) plus match-length
  percentiles (`bot_tier_lengths`, `median`).
- The committed baseline (`tests/fixtures/balance_baseline.json`) pins the
  counter matrix and bot tiers over a fixed **32-seed** set (3.125% rate
  resolution); the CI test `balance_table_matches_baseline` fails if any
  sim/unit change moves a rate.
- Target band: no unit may win its counter matchup outside **50–85%** at equal
  cost, and each counter must still win a majority. The band used to be 35–65%
  "soft counters", but that was measured against the pre-movement-fix sim
  where every unit stacked on one tile (the counters were an artifact of that
  bug). Under positional combat (formation spread, separation, building
  collision) the counters are stronger but still non-trivial; the committed
  v1 tune gives tank>infantry 81%, artillery>tank 72%, infantry>artillery 72%.
- Match-length p50 targets 5–10 min. `match_length_p50_within_band` asserts
  both bot tiers land in the band (rush-vs-turtle ~5.0 min,
  hard-vs-medium ~8.3 min); the turtle's finite, never-rebuilt turrets are
  what let sustained waves break through instead of stalemating.

## 7. Guarantees to dependents

- `crucible-server` can call into this crate with its own match executor and
  storage callbacks; this crate never talks to SQLite or the network directly.
- Population/generation state is serializable so the server can checkpoint it
  atomically and resume a crashed generation cleanly.
