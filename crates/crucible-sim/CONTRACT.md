# CONTRACT — crucible-sim

The pure, deterministic simulation core. This crate is the single source of
truth for game rules; every other crate and the client treat its behavior as
law. **Status: implemented (M1).**

## 1. Purity boundary

`crucible-sim` MUST NOT:

- open files, sockets, or the wall clock;
- spawn threads or tasks;
- make OS calls or touch the environment;
- draw entropy from anywhere except the injected seeded PRNG (`rng::Rng`);
- depend on any crate that requires OS/JS bindings (it must compile for
  `wasm32-unknown-unknown` with no `getrandom`, no `std::time`, no `std::thread`).

The crate depends only on `serde`/`serde_json` (serialization) plus its own
modules. Any future system-resource need must be pushed to `crucible-server`
and injected.

## 2. Determinism contract (this is the whole point)

1. **Fixed timestep.** `Game::step()` advances exactly one tick = 100 ms.
   `TICKS_PER_SEC = 10`. There is no variable-duration step. The *bot
   deliberation* cadence is every 20 sim ticks (2 s): `COMMAND_TICK = 20`.
   Human commands are **not** gated on it: the server applies them the tick
   they arrive and records that tick in the replay, and the replay consumers
   (`serialize::replay_to_game`/`replay_at_tick`, the ghost runner) apply
   commands at arbitrary ticks, so command latency is one 100 ms tick while
   bot cadence stays a deliberate 2 s.
2. **One PRNG.** All randomness flows through `rng::Rng`, a self-contained
   xoshiro256\*\* seeded from a `u64`. No unseeded entropy exists in the sim.
   A pinned known-sequence test guards the exact stream.
3. **Integer math only.** Positions/quantities are fixed-point integers
   (`fixed.rs`, 1 tile = 256 fix units). No `f32`/`f64` in game-state math; no
   `sin`/`cos`/`powf`/`sqrt` (distance uses `isqrt` + squared comparisons).
   A `HashMap`/`HashSet` must never influence a sim outcome; entity storage is
   `Vec` and pathfinding uses a tie-broken `BinaryHeap`.
4. **Entity order is spec.** Entities are assigned ids in ascending creation
   order from a single allocator. Every phase (economy, production, combat,
   turret fire, fog) iterates entities in **ascending id order**. Death sweeps
   use `retain`, preserving relative order.
5. **Byte-identical cross-target.** Identical seed + command log ⇒ identical
   serialized state on `x86_64-unknown-linux-gnu` and `wasm32-unknown-unknown`.
   Golden tests hash `serialize::snapshot_bytes` at fixed ticks and fail on any
   byte change.

## 3. Fixed tick order

`Game::step()` runs, in this exact order:

1. `tick += 1`; APM budgets refill.
2. Cooldowns decrement (units, then buildings).
3. `start_of_turn` — HQ ore trickle + refinery extraction (each refinery extracts its tile's resource, scaled by richness tier; deposits are infinite and never deplete).
4. `production_phase` — queues progress; completed units spawn (id order).
5. `combat_phase` — per combat unit: acquire target, move, fire (id order).
6. `turret_phase` — turrets fire (id order).
7. `separation_phase` — overlapping units are pushed apart (later id moves).
8. `sweep_dead` — dead entities removed.
9. `fog_phase` — visibility recomputed; last-seen memory updated.
10. `check_win` — HQ destroyed, or timeout by remaining value.

Reordering these changes determinism and requires a golden-hash update.

## 3a. Movement contract

- Movement is turn-based: each unit has movement points (MP) per turn. A
  `MoveGroup` sets a durable destination; the sim resolves as many steps as
  MP allows immediately, and the destination is retained so later turns
  continue the march automatically (Civ-style multi-turn movement).
- Buildings are **blocking** for ground units: `find_path` takes a `blocked`
  overlay (building tiles), so units path around and never walk through
  buildings. `Game::blocked_grid()` builds the overlay. **Aircraft**
  (`unit_stats(utype).air`) fly over buildings — the overlay is skipped for
  them — but still respect map terrain passability.
- `ClearMove` cancels a durable destination without changing the unit's
  current position or MP.
- Units do not stack: movement stops before a tile occupied by another unit.
- Terrain affects movement: forests/hills cost ×2, swamps/rivers cost ×3,
  deserts/plains cost ×1. Mountains and lakes are impassable.

## 4. Command & validation contract

- The complete action space is `orders::Command`: `PlaceBuilding`,
  `TrainUnit`, `MoveGroup`, `ClearMove`, `Attack`, `StartResearch`, `Sell`,
  `Repair`, `EndTurn`.
- `Attack` is focus-fire: the ordered units lock onto the single target
  (unit or building) and ignore everything else. A surviving defender in
  range counterattacks once.
- **One validator.** `Game::validate_command` is the only validation path;
  `apply_commands` validates, charges the APM budget, then executes. Humans,
  the AI, ghosts, and tests all go through it. No bypass exists.
- The per-turn action budget (default 16 actions/turn; `EndTurn` is free) is
  enforced inside the sim via `ActionBudget`; over-budget commands return
  `RateLimited`.
- Economy rules: four resources (Ore, Steel, Coal, Crystal). Train/build
  costs are charged at issue time across all four stockpiles; sell refunds 50%
  of the resource cost. Building placement requires a passable,
  resource-free, unoccupied tile within `PLACE_RADIUS_TILES` (5) of the nearest
  own building — bases grow in connected clumps. **Refineries are exempt**:
  they must be placed directly on a live resource deposit tile and extract
  that resource every turn, scaled by the deposit's richness tier (1–3).
  Deposits are infinite and never deplete. Artillery and Mammoth Tank
  production require a Tech Lab; Tech Lab placement requires a Factory;
  Radar, TeslaCoil, and AATurret placement require a Tech Lab. Research is a
  10-tech tree (tiered, with prerequisites); each Tech Lab generates research
  points per turn and one tech may be researched at a time.
- A match may end as a draw (`winner = null`): simultaneous HQ destruction and
  equal remaining value at timeout are side-neutral terminal results.

## 5. Fog-of-war contract

- `fog::FogView` is the *only* observation object exposed to a player (and the
  only input the AI may read). It contains currently-visible tiles,
  remembered enemy units/buildings with `last_seen` ticks, and known ore tiles.
  It cannot contain a live hidden entity.
- `Game::fog_phase` runs each turn and maintains `FogMemory` in serialized
  state; remembered positions decay (dropped after 6 turns unseen). Hidden
  entity death is never consulted to prune memory; memory is removed only by
  expiry or by re-observing the remembered location.

## 6. Serialization & replay contract

- `Game` is `Serialize`/`Deserialize` and byte-stable at any turn (field order
  is definition order).
- A replay is an **input log**: `{version, map_seed, config, commands[],
  result?}` (`serialize::Replay`), not a state dump. `FORMAT_VERSION = 5`.
  Version envelopes exist from day one; old replays must stay re-runnable.

## 7. Guarantees to dependents

`crucible-ai`, `crucible-evo`, `crucible-server`, and `crucible-client-wasm`
may rely on: the determinism guarantees above; the public types re-exported
from `lib.rs`; and `Map::generate(seed)` producing a constraint-scored,
fully-connected 64×64 map with typed terrain (plains/forest/hills/desert/
swamp/river/lake/mountain), asymmetric but fair spawn envelopes, route-cost
parity, and every resource tile reachable from both HQs. Deposits are
infinite; `resource_kind` and `richness` are the authoritative static data.

The map exposes three climate fields — `elevation`, `moisture`, and
`temperature` (0–255, latitude + elevation cooling + regional noise) — as
presentation metadata that also drives the biome model: polar latitudes are
tundra, equatorial wet belts are jungle, and deserts only form in warm
latitudes. `Game` is fully `Serialize`/`Deserialize`, so the server can
snapshot a live match and resume it; `Map::generate` output is unchanged in
guarantees when the climate fields are added (they are `#[serde(default)]`
for old snapshots).
