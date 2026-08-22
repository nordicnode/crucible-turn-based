# CONTRACT — crucible-ai

The AI commander: fog-legal feature extraction, the hand-rolled MLP, the
deterministic decision layer, and scripted baseline bots. Depends **only** on
`crucible-sim`. **Status: implemented (M2 scripted bots + M4 learned commander).**

## 1. Purity boundary

`crucible-ai` MUST NOT perform IO, spawn threads, read the clock, or draw
entropy. It is pure computation over a `crucible_sim::Game` / `FogView`. All
scheduling, storage, and match execution are injected by `crucible-server`
(headless) or `crucible-evo`.

## 2. The commander, not the soldier

- The network is **small** (~120 inputs → 2×48 tanh → output heads; ~10–12k
  params, flat `Vec<f32>`). It decides strategy on the **command tick** (every
  20 sim ticks = 2 s), never unit micro.
- Between command ticks the world runs on the sim's scripted unit rules. The
  AI may only issue group orders (`MoveGroup`, `TrainUnit`, `PlaceBuilding`,
  `StartResearch`, `Sell`), exactly the human action space.
- No unit-level learned micro, no neural pathfinding, no per-unit networks.

## 3. Fairness by construction

- The AI observes **only** a `FogView` (see `crucible-sim/CONTRACT.md` §5) —
  never the full `Game`. `features.rs` must take `FogView`, not `Game`, so a
  hidden entity cannot leak into the input vector by type error.
- The AI is subject to the same `validate_command` + APM budget as a human,
  enforced **inside** the sim. It cannot exceed a human-plausible command rate
  (default 120/min).
- Feature legality is test-enforced: hidden entities must produce **zero**
  delta in the feature vector (fuzz test).

## 4. Network & genome contract

- Feed-forward MLP on flat arrays; forward pass only. **No backprop, no
  autograd, no GPU.** Activation: `tanh` (a single deterministic table or a
  pinned implementation — document any platform-sensitive functions here).
- Genome = flat `Vec<f32>` of weights/biases, versioned schema. Mutation is
  Gaussian noise + occasional macromutation (owned by `crucible-evo`, but the
  encoding/decoding lives here and is versioned).
- Given the same genome and the same `FogView`/history, the output is
  **deterministic**. The history embedding (carried hidden state across command
  ticks) is part of the deterministic state.
- Outputs are *scores*; `decision.rs` maps them to concrete `Command`s via
  masked argmax + thresholds. Illegal candidates are masked from the state, so
  the network cannot choose them; a validator failure is a bug, not a runtime
  possibility.

## 5. Scripted baselines

`scripted.rs` provides deterministic, rule-based opponents used for the
bootstrap curriculum, gauntlet baselines, and regression tests: easy (passive
turtle), medium (periodic attack waves), hard (expand-and-push). Their pacing
was tuned in M8 so match length lands in the 5–10 min band: easy's turrets are
finite (never rebuilt, so sustained waves eventually break through) and hard
commits to a heavier push instead of stalling behind static defense.

Baseline bots are **oracle baselines**: they may read the full `Game`
(including hidden state) via the `Bot::decide(&Game, ...)` signature. This
makes them strong, reproducible opponents and keeps the champion honest — the
learned commander (§3) is fog-limited, so it must beat a strictly *stronger*
information set. Baselines are deterministic given a map seed and never exceed
the sim's APM cap; they go through the same `validate_command` path as
everyone else.

## 6. Guarantees to dependents

- `policy_commands(genome, fog_view, history, tick) -> Vec<Command>` (or
  equivalent) is pure and deterministic. The `history` is the previous
  `HISTORY_TICKS - 1` command ticks' feature vectors (oldest first), owned by
  the caller (e.g. `GenomeBot`) and zero-padded at the start of a match; it is
  fog-legal because it is derived from previous fog-legal observations.
- Every promoted champion must beat the hard scripted bot ≥ 90% at bootstrap
  (plan §5.7/M4; the CI curriculum pins a seed that clears the full
  "all three ≥ 90%" bar). The trainer runs a periodic self-play floor check
  (plan §5.8) that raises a `regression_alarm` event if the reigning champion
  dips below 70% vs hard.
