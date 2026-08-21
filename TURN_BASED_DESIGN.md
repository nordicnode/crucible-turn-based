# CRUCIBLE-Turn — Design Doc (Option B: Discrete Turn-Based)

Status: **approved direction** (user picked: alternating turns, passive income,
clean cutover). This document is the single source of truth for the conversion.
Every crate port implements *this*, not a reinterpretation.

## 0. What is deleted (clean cutover)

- Realtime tick loop: `tick.rs`, `economy.rs` (harvester loop), `movement.rs`
  (fix-unit stepping), `fixed.rs` (fix-point `Pos`), `Stance`, `UnitOrder`,
  `SetRally`, rally fields, `ApmBudget`, `COMMAND_TICK`, `TICKS_PER_SEC`,
  `MATCH_TIMEOUT_TICKS`, `DEPOSIT_PARK_TICKS`, `HARVEST_*`.
- `UnitType::Harvester`. Aircraft-over-building special-casing in formation
  (air units simply ignore terrain/buildings for MP).
- Client: display-position interpolation (`World.advance/display/headings`),
  realtime WS tick pump, C&C cursors tied to continuous hover-attack.
- Data: old DB rows (genomes/champions/replays/matches) dropped by migration;
  old replays unwatchable. Accepted.

## 1. Core model

- Grid: existing 64×64 maps, 8-directional, diagonal move costs 1 MP
  (matches `Map::neighbors`). No corner-cutting rule carried over (diagonals
  allowed between any two passable, unoccupied tiles).
- **No ticks. No wall clock. No RNG in-game.** Combat and economy are fully
  deterministic; the seeded `Rng` is used by map generation only. (The old sim
  already had no in-combat RNG; this is now structural.)
- Positions are tiles: `(u8, u8)`. All distances/ranges/vision are integer
  tiles, Euclidean on tile centers: `dx*dx + dy*dy <= r*r` (pure i64).
- Alternating turns: P0 acts (issues commands, executed immediately), sends
  `EndTurn`; engine resolves end-of-turn steps; P1's turn begins. `turn`
  starts at 1, increments on every EndTurn. `active: Player`.

### Turn lifecycle

`Game::end_turn()` (called via the `EndTurn` command, only by `active`):

1. **Turret auto-fire** (defender-free design: no reaction interrupts).
   Each of `active`'s turrets (TeslaCoil, Turret; ascending id) fires once at
   the lowest-id enemy in range. Damage applies immediately; deaths sweep
   after all turrets fired.
2. `sweep_dead()`.
3. `fog_phase()` (both players).
4. `check_win()`.
5. If not over: `active = active.enemy(); turn += 1;` then
   **start_of_turn(active)**:
   a. **Economy**: HQ trickle `+HQ_INCOME_PER_TURN` (10). Each refinery
      (ascending id): drains up to `REFINERY_ORE_PER_TURN` (60) from adjacent
      (8-dir) ore tiles, lowest tile-index first, banking what it took.
      Depleted-adjacent refinery earns 0 (dormant, shown in UI).
   b. **Production**: each building's queue advances 1 item-step; item with
      `progress >= build_time_turns` spawns on a free adjacent tile (existing
      `pick_spawn_tile` logic, tile-based). Low power ⇒ queues advance only
      on even turns (`turn % 2 == 0`), preserving the 50%-speed semantic.
   c. **Reset**: all of `active`'s units get `mp = max_mp`, `moved = false`,
      `acted = false`.
   d. `fog_phase()`.

`check_win` also runs after every applied command (an attack can kill an HQ
mid-turn). Commands are rejected with `CommandError::MatchOver` once over.

### Timeout

`GameConfig.timeout_turns` (default `MATCH_TIMEOUT_TURNS = 80`). Checked in
`check_win`: when `turn > timeout_turns`, winner by `remaining_value` (draw =
None). `<= 0` disables. Training/bootstrap configs override (see §7).

## 2. Entities

### Unit (rewritten)

```rust
pub struct Unit {
    pub id: EntityId,
    pub owner: Player,
    pub utype: UnitType,
    pub tile: (u8, u8),
    pub hp: i32,
    pub max_hp: i32,
    /// Movement points remaining this turn.
    pub mp: i32,
    /// Has moved this turn (move then attack allowed; not vice versa).
    pub moved: bool,
    /// Has attacked this turn (ends the unit's turn).
    pub acted: bool,
}
```

`unit_stats` gains `mp: i32`, `range_tiles: i32`, `min_range_tiles: i32`;
loses `speed`, `cooldown`, `vision(Fix)`→`vision_tiles`, `splash` (dropped),
`build_time`→`build_time_turns`. `air: bool` kept.

| Unit | Cost | HP | Dmg | Range | MinR | MP | BuildT | Air |
|---|---|---|---|---|---|---|---|---|
| Infantry | 50 | 90 | 55 | 1 | 0 | 3 | 1 | – |
| Artillery | 200 | 120 | 110 | 3 | 2 | 3 | 2 | – |
| MammothTank | 350 | 400 | 125 | 1 | 0 | 4 | 3 | – |
| Gunship | 250 | 140 | 90 | 2 | 0 | 7 | 2 | ✔ |
| Interceptor | 200 | 110 | 70 | 2 | 0 | 8 | 2 | ✔ |

### Building

Unchanged struct minus `rally`. **Placement remains instant** (cost charged,
full HP). New validator rule: `Refinery` requires an ore tile within 8-dir
adjacency of the placement tile (`CommandError::RefineryNeedsOre`) — this is
what makes ore fields matter under passive income. All other placement rules
unchanged (PLACE_RADIUS_TILES 5 clump rule, tech gates, ore-free tile).

Building stats: unchanged costs/HP/power. Turrets: TeslaCoil dmg 24 range 4;
Turret dmg 12 range 3 (fire once per own turn via auto-fire step).

### Upgrades (rebalanced for discrete space)

Damage **+25%**, Hp **+25%**, Range **+1 tile** (old +15/+15/+20% made no
sense at range 1). One per player, from TechLab, global. Unchanged otherwise.

## 3. Combat (Advance-Wars rules, deterministic, no luck)

`resolve_attack(attacker_unit_or_turret, defender)`:

```
deal(a→d)   = a.dmg * a.hp / a.max_hp          (integer, floor)
d.hp -= deal(a→d)
if d survives AND d is a direct unit (range>=1, not artillery-min-range case)
   AND a within d.range AND d hasn't already countered this exchange:
    deal(d→a) = d.dmg * d.hp_after / d.max_hp ; a.hp -= deal(d→a)
```

- Counters apply only against the *attacker* (never splash-adjacent — splash
  is gone). Turrets never counter (they auto-fire on their own turn).
- Air units: only air-capable or range≥2 units may target them? **No** — keep
  old rule: everything can shoot air (range permitting). Ground melee (range 1)
  cannot hit air at range 2; air strike from range 2 takes no counter from
  range-1 defenders (out of their range). Emergent, no special cases.
- Deaths: hp <= 0 marks dead; `sweep_dead` removes in ascending id order
  after each command resolution and at end-of-turn.

### Player commands (action space, v5)

```rust
pub enum Command {
    PlaceBuilding { player, btype, tile },
    TrainUnit { player, building, utype },
    MoveGroup { player, units: Vec<EntityId>, waypoint },   // stance gone
    Attack { player, units: Vec<EntityId>, target: EntityId },
    ChooseUpgrade { player, lab, upgrade },
    Sell { player, building },
    Repair { player, building },
    EndTurn { player },
}
```

- **MoveGroup**: per unit (ascending id): A* (`Map::find_path`, blocked =
  buildings + other units' tiles? NO — pathing ignores units, but the
  destination tile must be free of a living unit; if occupied, stop on the
  last free tile along the path). Spend min(mp, path_len) steps; leftover MP
  kept. Sets `moved = true` iff any step taken. Validator: group non-empty,
  all own living units, waypoint in bounds & passable terrain.
- **Attack**: every ordered unit (ascending id) with `!acted` and target in
  its range resolves `resolve_attack`; sets `acted = true` (even if out of
  range? NO — validator rejects if *no* unit is in range with
  `CommandError::OutOfRange`; units individually in range attack, others skip
  without spending `acted`). Harvester-gone: `NotACombatant` error removed.
- **EndTurn**: only by `active`; runs §1 lifecycle. Costs no budget.
- **Repair**: heals `max_hp * 30 / 100`, costs `max(cost * 20 / 100, 10)`
  ore, once per building per turn (`AlreadyActed`-style guard via new
  `building.repaired_this_turn: bool`, reset in start_of_turn).
- **Sell**: unchanged (50% refund).

### Action budget (replaces APM)

`ActionBudget { spent: i32, cap: i32 }` per player; reset each own turn.
Default cap 16 (`GameConfig.actions_per_turn`). Every command except EndTurn
costs 1. Over budget ⇒ `CommandError::RateLimited` (name kept).

## 4. Fog of war

Structures unchanged (`FogMemory`, `FogView`, remembered units/buildings,
known_ore, explored). Changes: `last_seen` is a **turn** number; memory drops
after `FOG_MEMORY_TURNS = 6` unseen (was 60 s). `compute_visible` walks all
own entities' vision_tiles with integer Euclidean radius. Recomputed: after
every command batch (cheap: ≤ few hundred entities), at end_turn, and at
start_of_turn. `FogView` gains `turn: i32`.

## 5. Serialization & wire

- `FORMAT_VERSION = 5`. Replay: `{version, map_seed, config, commands:
  Vec<TimedCommand>, result}` with `TimedCommand { turn, seq, player,
  command }` (`seq` = issuance order within the turn; global monotonic
  counter is fine). `ReplayResult { winner, reason, duration_turns }`.
- `replay_to_game`: fresh `Game::new`, apply commands in log order (commands
  execute immediately; `EndTurn` drives the lifecycle). Byte-stable serde
  field order preserved.
- Golden: rebuilt for the new model — `golden.rs` scenarios rewritten
  (bases incl. ore-adjacent refineries, train queues, move+attack playouts at
  fixed turns `[10, 30, 60]`), new committed hash constants; native+wasm
  parity tests consume the same constants (mechanism unchanged).
- WS protocol: `MatchStart` unchanged. `StateDiff` gains `turn`,
  `activePlayer`; entity `x/y` become integer tile coords; `queue/progress/
  buildTime` kept (progress now in turns); `stale` = turns since last seen.
  New `ClientMsg::EndTurn`. `MatchEnd.durationTurns`. `CommandRejected`
  unchanged. Bounds checks unchanged (message size, batch size, group size).

## 6. crucible-ai port

- `Bot::decide(&Game, Player)` signature kept; called once per own turn
  (headless runner + server call it when `game.active == player` at turn
  start; bots issue commands + `EndTurn`).
- Scripted bots: pacing constants converted (tick thresholds → turn
  thresholds ÷20, wave intervals 400/600 ticks → 20/30 turns); harvester
  training removed; refinery placement seeks ore adjacency (helper:
  nearest known/map ore tile, ring-search around it); `g.tick` → `g.turn`.
- Learned commander: `FeatureInput` rewritten for tiles/turns (own unit
  counts+HP fractions, building counts, fog-decayed enemy sightings by turn
  delta, income estimate = refineries×60 + 10, turn/timeout fraction,
  64-sector presence, unexplored fraction). `SINGLE_FEATURE_DIM` stays 112
  (layout re-documented), `HISTORY_TICKS = 2` kept. Heads: `TRAIN_OUT` 7→6
  (no Harvester); others unchanged ⇒ OUTPUT 91→90, GENOME_LEN recomputed.
  Army actions map: Attack/Defend/Scout emit MoveGroup(+Attack when legal);
  Snipe emits Attack on closest visible-this-turn enemy of winning type.
  Legality masks mirror the new validator (incl. refinery-needs-ore).
- Genome schema version bumped; all stored genomes void (clean cutover).

## 7. crucible-evo port

- Fitness shaping: win/draw/loss + margin unchanged; anti-rush threshold
  `< 2 min` → `duration_turns < 12`. Bootstrap cap 2 min → 20 turns; league
  cap 6 min → 60 turns; curriculum `shaping_ticks` → `shaping_turns` (30).
- Balance harness: counter matrix + bot tiers rerun over the same 32 seeds;
  match-length band becomes **p50 ∈ [40, 90] turns**. Baseline fixture
  regenerated after tuning; CI pin updated.
- Curriculum/gauntlet/Elo/ghosts/lineage: structure unchanged; ghosts replay
  `(turn, seq)` streams with the same creation-order id remapping (the
  mid-tick-command workaround dies — commands only exist at turn granularity
  now, but the ghost cursor still applies them in order).

## 8. crucible-server port

- Live matches: no interval pump. Flow: `JoinMatch` → `MatchStart` →
  human commands execute immediately (diff broadcast per batch);
  when `active == P1`, server asks the bot for commands, applies, broadcasts;
  `EndTurn` triggers lifecycle, broadcasts the resulting diff(s).
  Concurrency cap, store writes, replay recording (stamp `turn/seq`),
  rejection reporting: all preserved.
- Trainer: identical structure; `match_timeout_ticks` → `_turns` threading;
  rayon parallelism unchanged.
- SQLite: schema v2 migration — `duration_ticks` → `duration_turns`;
  old rows deleted (clean cutover). Envelope versions bumped.

## 9. Client port

- Render from tile coords; delete interpolation (`advance`, `display`,
  `headings`, `isMoving`). Keep camera pan/zoom, selection, minimap/radar.
- Turn UI: top bar shows `TURN n — YOUR TURN / CHAMPION'S TURN`; End Turn
  button (sidebar); during opponent turn inputs are ignored (server rejects
  anyway). Queued-command list optional, not required (commands execute
  immediately).
- FX: keep explosion/tracer/death effects triggered by `Attacked`/
  `BuildingDestroyed` events; projectiles animate over ~300 ms purely
  cosmetically. Mining-laser/harvester FX removed.
- Spectate/wasm shim: frame-per-turn; scrub/play in turns/s (≈1 turn/s base
  speed). `snapshot.ts` maps the new lean frame.
- types.ts: command builders mirror §3; `Stance` removed; tests updated.

## 10. Verification plan (acceptance)

1. `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
2. Native/wasm golden parity green (new constants).
3. Balance baseline regenerated + pinned; `balance_table_matches_baseline`
   green; p50 length inside [40, 90] turns.
4. Curriculum CI pin re-converged (beats hard ≥ 90% over 32 held-out maps).
5. Client `npm run build` + `npm test` green.
6. Manual smoke: play a match vs easy — move, attack, build, refinery income,
   turret auto-fire, EndTurn alternation, win by HQ destruction; spectate the
   resulting replay in the browser.

## 11. Non-goals (v1 of the turn version)

Per-unit-type damage multipliers (AW-style matrix), capture, reaction fire,
zone-of-control, unit transport, fog-driven hidden movement phases, multiplayer
hotseat beyond alternating local P0/P1.
