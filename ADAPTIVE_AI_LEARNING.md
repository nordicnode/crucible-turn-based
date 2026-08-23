# Adaptive Opponent Learning — Development Plan

> **Vision:** after every match, the AI learns from that match, looks back at all matches in general *and* against that specific person, then adapts. It should feel like it remembers how you play and adjusts to you match-over-match — while safe, deterministic at its core, and fair to beat.
>
> This document is the working spec we follow through development. It is **not** a monolith: it is a set of phases (P0→P3) each independently shippable and verifiable, with the deterministic core and committed baselines preserved throughout.

---

## 0. TL;DR

- Today **"VS AI" is static** (a hand-tuned `AdaptiveBot` that only scales a difficulty number picked by a client-side `localStorage` meter). The real learning is a **separate offline trainer** (`GenomeBot` + self-play evolution + ghosts) that never sees your games.
- We add a **personalization layer** so the AI genuinely adapts to each player: (1) an **opponent model** per player learned from their matches, (2) **counter-selection** that uses that model (and a global aggregate) to pick strategy + difficulty, and (3) **aggregate learning** that mines the replay DB to bias the trainer and calibrate difficulty.
- Design rule: **personalization lives entirely outside the deterministic sim.** It only decides *which policy/archetype/difficulty to serve*; it never mutates match logic. Goldens, balance baseline, bots acceptance, and replayability all stay intact.
- Honesty guardrail: this is **opponent modeling + adaptive difficulty**, not "neural net learns to beat you in one match." One match is too little signal to train weights; we turn each match into a *player tendency profile* instead — which is what delivers the felt experience and is robust.

---

## 1. Current State — what actually exists today (reality check)

| System | What it is | Learns? |
| --- | --- | --- |
| `AdaptiveBot` (`crucible-ai/src/scripted.rs`) | The **"VS AI"** commander. Deterministic, hand-tuned; picks a strategy archetype (defensive / infantry-pressure / expand) from a `difficulty` scalar. | **No.** Static. |
| Client difficulty meter (`client/src/main.ts`) | The **only** thing that changes after each match: stores last ~24 results in `localStorage`, sends `adaptive:<0..1>` so the server picks a harder/easier archetype for the *next* VS-AI game. | No learning — **selection only** (which archetype). |
| `GenomeBot` (`crucible-ai/src/network.rs` + `decision.rs`) | The learned neural commander (`Champion` / `Museum`). | **Yes — offline.** Self-play evolution handled by `trainer.rs`; a champion is crowned via `run_gauntlet` and stored as a frozen snapshot. |
| Ghosts (`crucible-evo/src/ghost.rs`) | Frozen opponents reconstructed from recorded matches' input logs; `ghost_fitness` blends them into training. | **Yes — offline.** Already the hook for "learn from human replays," currently used for self-play evals, not per-player serving. |
| `matches` table (`store.rs`) | Persists every match: `map_seed, p1_type, p2_type, result, duration_turns, replay(JSON), created_at`. | — (the raw memory; already exists). |

**The gap this project fills:** no **per-player** identity or memory, no *serving* that reads past matches, and no **global** mining feeding difficulty/training. The raw replay DB and the ghost machinery already exist — we bolt the personalization layer onto them.

---

## 2. Vision — decomposed into three learning layers

| # | Layer | Backs this part of the goal | Where it lives |
| --- | --- | --- | --- |
| L1 | **Per-player opponent model** | *"learns from the previous match + all matches against that specific person"* | server, new tables + a match-end hook |
| L2 | **Counter-selection (serving)** | *"the AI adapts to me"* | `resolve_opponent` chooses archetype/difficulty from the model |
| L3 | **Aggregate learning** | *"looks back at all matches in general, then learns"* | replay-DB mining → difficulty calibration + ghosts → trainer bias |

All three share one **decision boundary**: everything inside `crucible-sim` stays byte-identical. The layers choose **between** deterministic policies (archetypes / `AdaptiveBot` difficulties / a stored `GenomeBot`), never inside one.

---

## 3. Architecture

```
 ┌────────────────────────── CLIENT (browser) ──────────────────────────┐
 │  game canvas        lobby ("VS AI", Champion, Museum)               │
 │  localStorage: playerId (stable UUID), adaptive difficulty meter    │
 └───────────────────────────────┬──────────────────────────────────────┘
                                 │  joinMatch { opponent: "vsai:<playerId>", playerId }
                                 ▼
 ┌────────────────────────── SERVER (axum/tokio) ───────────────────────┐
 │  resolve_opponent(store, opponent, playerId)                         │
 │     └─ L2 counter-selection:  archetype + difficulty  ──────────────┐│
 │        chosen from L1 model + L3 global calibration                 ││
 │                                                                     ││
 │  match loop (deterministic sim)      ◄─────────────── proxy/policy   ││
 │     └─ on finish → save_replay + save_live (exists)                 ││
 │                                                                     ▼│
 │  MATCH-END HOOK (new)  ──►  L1: update player model                 ││
 │                    (per player)        ┌────────────────────────────┼
 │                                        │  DB                        ││
 │  AGGREGATE JOB (periodic, new) ────────┤  matches (exists)          ││
 │     mine DB → difficulty calibration    │  players (new)            ││
 │     + feed ghost pool (exists)          │  player_profiles (new)    ││
 │     + trainer bias (echoes into evo)    │  ai_stats (new)           ││
 └──────────────────────────────────────┬─┘                            │
                                         ▼                              │
 ┌────────────────────────── TRAINER (offline, exists) ─────────────────┼┐
 │  self-play ES generations + ghost_fitness + gauntlet ─────────────────┘│
 │  (Champion crown: beats hard ≥90%, gauntlet vs ghost pool)             │
 └────────────────────────────────────────────────────────────────────────┘
```

---

## 4. Data model (all in existing SQLite via `store.rs`)

### 4.0 Reused as-is

- `matches` — source of every historical match (already has `replay` JSON and `result`).

### 4.1 New: `players`

```sql
CREATE TABLE IF NOT EXISTS players (
    id TEXT PRIMARY KEY,        -- stable client-stored UUID (pseudo-anonymous)
    first_seen_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    matches INTEGER NOT NULL DEFAULT 0,
    latest_skill f32 NOT NULL DEFAULT 1200   -- running estimate (see §6.4)
);
```

### 4.2 New: `player_profiles` (L1 per-player model, stored as a compact JSON snapshot)

```sql
CREATE TABLE IF NOT EXISTS player_profiles (
    player_id TEXT PRIMARY KEY REFERENCES players(id),
    -- compact tendency fingerprints, updated after each match (§5.4)
    opening_mix          TEXT NOT NULL DEFAULT '{}',  -- {build_order_key: weight}
    unit_mix             TEXT NOT NULL DEFAULT '{}',  -- {unit_type: share_0to1}
    rush_timing_offset   TEXT NOT NULL DEFAULT '{}',  -- {archetype: earliest_turn}
    tempo                REAL NOT NULL DEFAULT 0.5,   -- combat strength by T30
    tech_bias            REAL NOT NULL DEFAULT 0.5,   -- 0 none .. 1 tree-heavy
    expansion_bias       REAL NOT NULL DEFAULT 0.5,   -- refinery aggression
    vs_archetype         TEXT NOT NULL DEFAULT '{}',  -- {archetype: {wins,loses}}
    recent_form          TEXT NOT NULL DEFAULT '[]',  -- last N (result, arch, diff)
    recency_weight       REAL NOT NULL DEFAULT 0.7,   -- decay toward recent matches
    updated_at           INTEGER NOT NULL
);
```

Rules: **no raw replay stored here** (that lives in `matches.replay`); the profile is a bounded append/decay summary so it cannot grow without bound. All arithmetic is deterministic (no RNG) so the model is reproducible from the same inputs.

### 4.3 New: `ai_stats` (L3 aggregate, produced by the periodic job)

```sql
CREATE TABLE IF NOT EXISTS ai_stats (
    key TEXT PRIMARY KEY,           -- e.g. 'difficulty_calibration', 'global_tendencies'
    value TEXT NOT NULL,            -- JSON
    at INTEGER NOT NULL
);
```

Holds: (a) a **difficulty calibration map** (`skill → difficulty that predicts ~50%`) and (b) **global strategy tendencies** (frequency of opens/compositions) used to seed L2 when a player has no profile yet and to bias the trainer's ghost pool.

---

## 5. L1 — Per-player opponent model

### 5.1 Player identity

- No account system (dev-plan non-goal). On first client boot, generate `playerId = crypto.randomUUID()` and persist in `localStorage`. Sent on `joinMatch` and on the shared-replay link (`?replay=N&player=…`) so replays tag the right player.
- Pseudo-anonymous (a random UUID, not a name). No PII.

### 5.2 Feature extraction from a finished match

We do **not** store the whole model as raw replay; we extract a compact fingerprint once at match end. Walk the replay JSON (turns + commands) and compute:

- `opening_mix`: the ordered (building-type → unit-type) chain of the first ~25 player commands.
- `unit_mix`: final-count fraction of each enemy unit type the player fielded.
- `rush_timing`: earliest turn a player combat unit got within a radius of the AI's HQ (per strategy archetype the AI used).
- `tempo`: player's combat-unit count at turn 30 (normalized).
- `tech_bias`: did they build a TechLab early / research anything.
- `expansion_bias`: number of non-ore resource refineries claimed.
- `result`: won/lost vs the archetype the AI served, and at what difficulty.

### 5.3 Match-end hook (code touch point)

In `ws.rs`, alongside the existing `save_replay` / `save_live` calls at the end of the match loop (currently ~lines 658–668), add a call to a new `personalize::record_match(store, player_id, outcome, archetype, difficulty, replay)`. Single synchronous SQLite write; must be best-effort (never fail the match report if the profile write errors). Add a `tracing` line on failure.

### 5.4 Update algorithm (deterministic, recency-weighted)

After each match, recompute the profile as a **blended** of prior profile `P_old` (decay `w`) and this match's fingerprint `F`:

- Temporal knobs (tempo, tech, expansion): `P_new = (1 − w)·P_old + w·F`.
- Categorical maps (opening, unit mix, vs-archetype): decay existing weights by `(1 − w)` and add `w` to this match's observed keys, then renormalize.
- `recent_form`: push the newest result; cap the list.
- Determinism: pure arithmetic over stored values — no wall clock, no RNG → the same match history yields the same profile.

### 5.5 Acceptance (P1)

- Playing N VS-AI matches as one UUID produces a `player_profiles` row whose values move as expected (e.g., spamming scouts raises `unit_mix["Scout"]`).
- Two players get independent profiles.
- Deterministic unit test: replay the same match set twice → identical serialized profile.

---

## 6. L2 — Counter-selection (serving: "the AI adapts to me")

### 6.1 Where

`resolve_opponent(store, opponent, player_id)` in `ws.rs`. Add an overload/thread the `playerId` from the join message through to resolution.

### 6.2 Selection logic (deterministic given the profile)

1. If the player has **no profile** (or <3 matches): use the **current** behavior — global calibration map + client-supplied difficulty → `AdaptiveBot::new(difficulty)`. (No regression for new players.)
2. If profile exists: read `vs_archetype` and `unit_mix`. Choose the difficulty that the calibration map predicts ≈ near the player's `latest_skill`. Choose the **archetype** that maximizes expected engagement subject to §6.3, inferring tendencies from the model (e.g., a player who rushes fast → serve the turtle/defense archetype; a turtle → serve the expand archetype; an air-heavy player → serve the AA/expand archetype that carries counters).
3. Where the player historically performs **far below** the served archetype+difficulty, step difficulty **down** one; if they consistently win → step **up** one. This is the per-player "it learned I needed easier/harder."
4. Still bounded by a deterministic mapping (no RNG) so the same state → same opponent (replays of the *selection* are reproducible).

### 6.3 Fairness bound (hard requirement)

- The chosen combination must stay within a **fairness window**: never drop below the "easy" archetype floor nor exceed the "hard" ceiling, and never pick a hard-counter that the player cannot plausibly beat at their skill. Introduce a small hysteresis (require ≥2 consecutive evidence matches before switching difficulty) to stop ping-ponging.
- Rationale (dev-plan pillar 2): the AI wins on *better strategy within reach*, not by reading the player's mind and hard-countering — that reads as unfair.

### 6.4 Skill estimate

- `latest_skill` in `players`: updated with a simple sliding estimate after each match (e.g., a K-factor Elo-ish update against the served difficulty, or a sliding win-probability). Keep it deterministic and single scalar.

### 6.5 Acceptance (P2)

- Given a scripted profile, `resolve_opponent` returns the expected archetype+difficulty; an empty profile returns today's behavior (unchanged).
- Determinism test: two calls with the same stored state → same bot.
- Fairness test: the selection never leaves the allowed difficulty band.

---

## 7. L3 — Aggregate learning ("looks back at all matches in general")

### 7.1 Periodic mint job (new, e.g. a tokio interval in the server, `CRUCIBLE_MINT_INTERVAL`-tunable)

Runs over all rows in `matches` (optionally since last run) and writes `ai_stats`:

- **Difficulty calibration:** bucket matches by p1/p2 difficulty, compute empirical win probability per skill band → a map `skill → difficulty ≈ 0.5 win rate`.
- **Global tendencies:** frequency of opening keys, unit-mix centroids, rush-timing distributions — used to seed L2 for cold-start players and to weight the ghost pool.

### 7.2 Feeding the trainer (echoes the dev-plan "every human game is selection pressure" pillar)

- Populate / refresh the **ghost pool** (existing ghost machinery in `ghost.rs`) from matches flagged as *informative* (decisive, short, recent, champion-beating) so `ghost_fitness` in the trainer evaluates the population against real human strategies.
- **Trainer bias:** optionally weight the curriculum's opponent mix so openings/compositions the replay DB shows win get nominally more ghost coverage. This is the honest form of "the AI got better because it studied human games" — an *offline* batch echo, not per-match.

### 7.3 Dashboard

- Add an `ai_stats` surface to the existing dashboards (`intel.ts` / `dashboard`): "Adapting to you," win streaks, difficulty moved, how many matches studied.

### 7.4 Acceptance (P3)

- Insert N seeded matches → mint job → `ai_stats` calibration/tendencies are deterministic and sensible.
- The trainer's ghost pool reflects human-mined matches (sampled deterministically).

---

## 8. Client changes (small)

- Generate + persist `playerId` (UUID in `localStorage`); send on every `joinMatch` and append to shared-replay links.
- Keep the existing difficulty meter as the *fallback* signal for players with no server profile (it stays as the cold-start input).
- Optional: a small lobby line "The AI is studying your games" when a profile exists.

---

## 9. Determinism, safety, and non-goals

### 9.1 Determinism & the committed baselines (non-negotiable)

- **No code inside `crucible-sim` changes.** The personalization layer is selection-only.
- All profile/calibration math is pure/deterministic (no RNG, no wall clock as a seed). Reproducible from stored inputs.
- Guard with existing gates — every milestone must keep green:
  - `cargo test -p crucible-ai` (lib + `bots.rs`: rush>turtle, hard>medium, economy-by-60)
  - `cargo test -p crucible-evo --test balance` (`balance_table_matches_baseline`, `is_deterministic`)
  - `cargo test -p crucible-server` (24 tests)
  - client `npx tsc --noEmit && npx vitest run`
  - any sim golden tests untouched (no sim edits).

### 9.2 Safety

- **Bounded personalization:** fairness window + hysteresis; never unboundedly-smarter-against-you.
- **Storage:** profiles are bounded summaries; no raw-replay duplication per player (reuse `matches.replay`).
- **Best-effort hook:** profile writes never break match reporting.
- **Privacy:** UUID identity, no PII, localhost-first.

### 9.3 Non-goals (do not build)

- No online gradient/backprop learning from individual matches (too noisy — §0 honesty guardrail).
- No account system, no matchmaking, no auth.
- No external ML services/LLMs (hard constraint).
- No per-player raw-replay storage.
- No AI that "cheats" by reading fog of war or hard-counting exposure.

---

## 10. Phases & milestones (follow in order)

| Phase | Deliverable | Gate |
| --- | --- | --- |
| **P0** | Player identity + `players` table + match-end hook (record match→player; no model yet) | integration test: a VS-AI match writes/updates a `players` row; all existing tests still green |
| **P1** | `player_profiles` + L1 feature extraction + update algorithm | unit tests: profile moves correctly; deterministic; two players independent |
| **P2** | L2 counter-selection in `resolve_opponent` (archetype+difficulty from model, fairness-bounded) | determinism + fairness tests; cold-start = today's behavior |
| **P3** | L3 mint job + calibration + ghost-pool feed + dashboard surface | mint deterministic; ghost pool reflects mined matches; full gate suite green |
| *(Future)* | Deploy to VPS / multi-player / per-player on-demand calibration | — |

Each phase is independently shippable and verifiable; do not start P{n+1} until P{n} is green and landed.

---

## 11. File-by-file change map

| File | Change |
| --- | --- |
| `crates/crucible-server/src/store.rs` | New migrations: `players`, `player_profiles`, `ai_stats`; CRUD helpers (`upsert_player`, `get_player`, `get_profile`, `set_ai_stat`, …) |
| `crates/crucible-server/src/personalize.rs` **(new)** | L1 matching-end: feature extraction + recency-weighted update |
| `crates/crucible-server/src/mint.rs` **(new)** | L3 periodic job: calibration + tendencies + ghost-pool refresh |
| `crates/crucible-server/src/ws.rs` | Thread `playerId` through `joinMatch` → `resolve_opponent`; call `personalize::record_match` at match end |
| `crates/crucible-server/src/ws.rs` `resolve_opponent` | L2 counter-selection (deterministic, fairness-bounded, cold-start fallback) |
| `crates/crucible-server/src/main.rs` | Spawn mint job; route `playerId`; optional env knobs (`CRUCIBLE_MINT_INTERVAL`) |
| `crates/crucible-server/src/http.rs` | Dashboard stats for `ai_stats`; accept `player` on replay-share |
| `client/src/main.ts` | `playerId` gen/persist/send; keep difficulty meter as fallback |
| `client/src/net.ts` / shared types | `playerId` field on `joinMatch` |
| `client/src/intel.ts` / `dashboard.ts` | "Adapting to you" aggregate surface |
| `crates/crucible-evo/src/ghost.rs` | (reused) pick up mined informative matches for the pool |

---

## 12. Test plan

### Rust — `crucible-ai`

- bots acceptance unchanged (must stay green).

### Rust — `crucible-evo` (`--test balance`)

- Baseline + determinism unchanged (must stay green). Any rate change → documented fixture regen only.

### Rust — `crucible-server` (new tests in `tests/` or inline)

- `personalize`: extract features from a fixture replay; update moves the profile deterministically; two player ids independent; recency decay works.
- `resolve_opponent`: empty profile → current fallback; scripted profile → expected archetype/difficulty; never leaves fairness band; same state → same bot (determinism).
- `mint`: seeded `matches` → deterministic `ai_stats`.
- **End-to-end:** drive one full VS-AI match via the WS/turn loop → `players` row exists with `matches=1`; profile updated.

### Client — vitest

- `playerId` generation is stable across reload; sent on `joinMatch`; included in replay link.

### Full gate (run before every phase lands)

```bash
cargo test -p crucible-ai
cargo test -p crucible-evo --test balance
cargo test -p crucible-server
cd client && npx tsc --noEmit && npx vitest run
```

---

## 13. Risks & mitigations

| Risk | Mitigation |
| --- | --- |
| Personalization feels unfair / "reads my mind" | §6.3 fairness window + hysteresis; bounded to ±1 difficulty step |
| AI difficulty ping-pongs | Require ≥2 consecutive evidence matches before stepping |
| Profile grows unbounded | Bounded summary + decay, no raw-replay duplication |
| Nondeterminism leaks in (breaks replays/goldens) | Pure arithmetic only; sim untouched; per-milestone gate suite |
| Player-id spam / storage bloat | Cap players rows (e.g., LRU cleanup old ids), summary-only model |
| Ghost pool over-fits to cheese | Sample informative matches deterministically with diversity weighting |
| Match-end hook fails | Best-effort, logged, non-fatal |

---

## 14. Definition of done

The feature is done when:

1. A player plays VS-AI matches and the AI **demonstrably** changes what it serves across matches (archetype + difficulty) using that player's history — visible from the dashboard.
2. It generalizes from all matches (mint job + ghost pool + difficulty calibration).
3. **All** committed gates stay green; no sim changes; baseline untouched unless a documented fixture regen.
4. Fairness bounds hold.

---

*Started 2026-08-22. Follow phases P0→P3 in order; never start the next phase until the current one is green and landed.*
