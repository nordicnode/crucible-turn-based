# CRUCIBLE

A minimalist turn-based strategy game with one twist: **the opponent is a neural
network that learns while you play.**

Play alternates between you and the machine the way it does in Advance Wars or
Civilization — you issue orders on your turn, end it, and watch the AI take
its. Every match you fight, win or lose, becomes training data. The AI plays
against itself around the clock, a champion only gets replaced when a
challenger can prove it's better, and the strategy you used to win last night
gets countered before you sit down again.

## What you can do

- **Fight the AI.** Challenge scripted bots (easy / medium / hard), the reigning
  champion, or any former champion from the Museum.
- **Watch it improve.** The dashboard charts the champion's Elo over
  generations, its lineage, and every dethroned champion.
- **Replay any match.** Every game is saved as a tiny input log; watch it back
  frame-per-turn in the browser, unfogged, with play / pause / speed / scrub.
- **Feed it ghosts.** Your matches are replayed during training as frozen
  "ghost" opponents, so the strategy that beat you becomes tomorrow's training
  data — adopted into the pool live, no server restart needed. Matches where
  you beat the reigning champion get priority retraining weight.
- **Auto-battle ancestors.** Pit the current champion against any past champion
  and spectate the result.

## Quick start

You need Node.js 20+ and a browser.

```bash
# 1. Install dependencies
npm install

# 2. Build client and server
npm run build

# 3. Run development server (with hot reload)
npm run dev

# Or run the production build
npm start
# open http://localhost:3000
```

That gives you the full game against scripted bots. For a real, evolving
champion, start the trainer too (see *Training the AI* below).

## How to play

A match is a sequence of strictly alternating turns. You act, press **END TURN**
in the sidebar, the opponent acts, and control returns to you. The top bar shows
the current turn and whose turn it is; while the opponent plays ("OPPONENT
TURN…") your inputs are ignored, and the server enforces that anyway.

**On your turn** you may issue up to 16 commands — build, train, move, attack —
in any order, then end your turn.

- **Select.** Left-click a unit or building to select it; drag a box to select a
  group (hold **Shift** to add). Middle-drag pans the camera, the mouse wheel
  zooms, and **Esc** cancels whatever you're placing.
- **Move & attack.** With combat units selected, **right-click open ground** to
  march them there — they advance up to their movement points (MP) on your turn.
  **Right-click an enemy** to focus-fire it: every selected unit in range
  attacks it together, and adjacent units move up. Moving and then attacking in
  one turn is allowed.
- **Build.** With nothing selected, the sidebar offers PowerPlant, Refinery,
  Barracks, Factory, TechLab, Airfield, Radar, TeslaCoil, and Turret.
  Buildings must be placed within a few tiles of an existing one, so your base
  grows as a connected clump (the placement ghost turns green when the spot is
  valid, red when it isn't). A **Refinery is built directly on a resource tile**
  — it extracts that tile's resource every turn, scaled by the deposit's
  richness (poor / standard / rich).
- **Train & research.** Select a production building to queue units (Barracks →
  Infantry / Scout / RocketTrooper; Factory → Tank / Artillery / MammothTank /
  SamLauncher; Airfield → Gunship / Interceptor). Select a TechLab to open the
  research tree (10 technologies across multiple tiers). Select any building
  to repair or sell it.
- **Power.** The HQ and PowerPlants produce power; Refineries, Barracks,
  Factories, TechLabs, Airfields, Radar, and Turrets consume it. If consumption
  ever exceeds production, your production queues only advance every other
  turn — build a PowerPlant to lift the cap, the same way the AI does.
- **Economy.** Four resources — **Ore, Steel, Coal, Crystal** — are extracted
  by refineries built directly on infinite deposit tiles. Each deposit has a
  richness tier (poor / standard / rich) that scales the per-turn yield. The HQ
  banks a small ore trickle each turn; every refinery adds its resource's
  richness-scaled yield. No refineries, no income beyond the HQ trickle.
- **Fog of war.** You see only tiles near your own units and buildings. Enemy
  sightings fade after 6 turns unseen, so scouting matters — especially the
  passive Radar dish, which reveals a huge swath of the battlefield.

**Win by destroying the enemy HQ.** Matches cap at 80 turns; if nobody falls by
then, the player with the higher remaining military value takes it (a draw if
equal).

## The game

Four resources (Ore, Steel, Coal, Crystal), twelve buildings (HQ, PowerPlant,
Refinery, Barracks, Factory, TechLab, Airfield, Radar, TeslaCoil, Turret,
AATurret, CrystalRefinery), and nine units on procedurally generated 64×64 maps
with typed terrain (plains, forest, hills, desert, swamp, rivers, lakes,
mountains):

| Unit | Cost | HP | Dmg | Range | MP | Build | Notes |
|---|---|---|---|---|---|---|---|
| Infantry | 50 | 90 | 55 | 1 | 3 | 1 t | Barracks; cheap line troops |
| Tank | 150 | 260 | 105 | 1 | 5 | 2 t | Factory; the workhorse |
| Artillery | 200 | 120 | 110 | 3 (min 2) | 3 | 2 t | Siege; can't fire point-blank |
| MammothTank | 350 | 550 | 170 | 1 | 4 | 3 t | TechLab; slow heavy armor |
| Gunship | 250 | 140 | 105 | 2 | 7 | 2 t | Airfield; flies over everything |
| Interceptor | 200 | 110 | 70 | 2 | 8 | 2 t | Airfield; fast strike aircraft |

TechLab (requires a Factory) unlocks the MammothTank and one global research
track per player: **High-Explosive (+25% damage), Reinforced Armor (+25% HP)**,
or **Extended Range (+1 tile)**. Radar and TeslaCoil are the second tier and
need the TechLab itself; the TeslaCoil is a long-range arc turret (range 4,
24 dmg), the plain Turret is cheaper and shorter-ranged (range 3, 12 dmg), and
both fire automatically once at the end of your turn.

Combat is deterministic Advance-Wars-style: damage scales with the attacker's
remaining HP, and a defender in range counterattacks once. There is no
randomness in a battle — the seeded PRNG exists only for map generation. Maps
are asymmetric but constraint-scored: the generator rejects candidates that
fail route-cost parity, resource roster, or terrain-variety gates. Terrain
affects movement cost, defense, and passability — forests give cover, hills
grant defense bonuses, rivers slow crossings, and mountains/lakes are
impassable. The engine runs on strictly alternating turns with no wall clock
and no in-game RNG, and is fully deterministic and server-authoritative.

The generator uses a full climate model: elevation, moisture, and a new
temperature field (latitude + elevation cooling + regional noise) drive the
biomes, so polar latitudes read as tundra, equatorial belts as jungle, and
mountain ridges as rock. Rivers follow the elevation field downhill (steepest
descent, pooling into lakes at local minima), mountain chains meander along
tectonic splines with fault branches, and spawn rings take their biome palette
from the local climate. Resource deposits correlate with terrain — steel in
hills, coal in deserts, crystal in forests.

Quality-of-life and production features: a right-side command rail, tile
inspector with climate readouts, build-tree tab in the research bureau,
movement-path preview with MP cost on hover, fog-of-war reveal fade,
stacking badges, floating damage numbers, keyboard-shortcut overlay,
touch support, adaptive AI difficulty (remembers your recent results),
replay sharing via `?replay=<id>` URL, and save/resume: a disconnect
mid-match snapshots the game, and the lobby's **Resume Saved Match** button
continues it server-side.

## Training the AI

The trainer runs inside the server process, in the background. Turn it on with
environment variables:

```bash
# Bootstrap a competent champion from a cold start, then evolve 24/7:
CRUCIBLE_TRAINER=1 CRUCIBLE_TRAINER_BOOTSTRAP=1 cargo run -p crucible-server -- start

# Or a quick, bounded run with a small population:
CRUCIBLE_TRAINER=1 CRUCIBLE_TRAINER_SMALL=1 CRUCIBLE_TRAINER_GENERATIONS=5 cargo run -p crucible-server -- start
```

| Variable | Effect |
| --- | --- |
| `CRUCIBLE_TRAINER=1` | enable the trainer loop |
| `CRUCIBLE_TRAINER_BOOTSTRAP=1` | run the staged curriculum on a cold start |
| `CRUCIBLE_TRAINER_GENERATIONS=N` | stop after N generations (fast-forward) |
| `CRUCIBLE_TRAINER_SMALL=1` | small, fast population for demos |
| `CRUCIBLE_DB=path` | SQLite file (default `data/crucible.db`) |

Progress (population, champion, Elo, replays) is checkpointed in SQLite and
resumes across restarts. Delete `data/crucible.db` for a fresh cold start.

For local development, the `start` argument gracefully replaces an existing
Crucible server launched from this checkout before binding the configured
address. Use `cargo run -p crucible-server -- start` (or `cargo run start` from
the server crate directory) instead of manually hunting down stale processes.

Watch it learn at `http://127.0.0.1:8787/api/status` (generation, matches run)
and `http://127.0.0.1:8787/api/champion` (current champion + Elo).

## How the AI works

- **The commander, not the soldier.** The evolvable brain is a small neural
  network (~17.5k weights) that makes strategic decisions once per turn: what
  to build, train, expand toward, and attack. Movement and attack execution for
  a decided army are scripted. It plays under the same fog of war and action
  budget you do. Your commands apply immediately as you send them; the opponent
  deliberates once its turn begins.
- **Evolution strategy.** A population of genomes competes in headless
  self-play; the strongest are kept and mutated. No backpropagation; selection
  pressure does the learning.
- **The gauntlet.** A challenger only becomes champion by beating the incumbent
  (and sampled former champions) in a reproducible match series. Elo tracks the
  outcome.
- **Ghosts.** Your replays are replayed as frozen opponents during training, so
  a strategy that beats the champion gets countered within a training cycle.
- **Determinism.** One seeded PRNG (map generation only), integer math, no
  in-game RNG, and input-log replays mean any match or promotion can be
  reproduced byte-for-byte, natively and in the browser's wasm shim.

## For developers

The workspace is a TypeScript fullstack application:

```
client/                  TypeScript + Vite + Canvas 2D + Vitest
server/                  Simulation logic, AI bot logic, and session store
server.ts                HTTP and WebSocket server
```

```bash
# Typecheck
npm run lint

# Run unit tests
npm test

# Build production bundle
npm run build
```

CI (`.github/workflows/ci.yml`) runs typechecking, unit tests, and production build verification on every push and PR.
