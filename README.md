# CRUCIBLE

A minimalist real-time strategy game with one twist: **the opponent is a neural
network that learns while you play.**

Every match you fight, win or lose, becomes training data. The AI plays
against itself around the clock, a champion only gets replaced when a
challenger can prove it's better, and the strategy you used to win last night
gets countered before you sit down again.

## What you can do

- **Fight the AI.** Challenge scripted bots (easy / medium / hard), the reigning
  champion, or any former champion from the Museum.
- **Watch it improve.** The dashboard charts the champion's Elo over
  generations, its lineage, and every dethroned champion.
- **Replay any match.** Every game is saved as a tiny input log; watch it back
  step-by-step in the browser, unfogged, with play / pause / speed / scrub.
- **Feed it ghosts.** Your matches are replayed during training as frozen
  "ghost" opponents, so the strategy that beat you becomes tomorrow's training
  data — adopted into the pool live, no server restart needed. Matches where
  you beat the reigning champion get priority retraining weight.
- **Auto-battle ancestors.** Pit the current champion against any past champion
  and spectate the result.

## Quick start

You need a recent Rust toolchain, Node 20+, and a browser.

```bash
# 1. One-time setup: wasm target, bindings tool, client dependencies
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.127 --locked   # must match Cargo.lock
(cd client && npm install)

# 2. Build the client (includes the wasm replay shim)
(cd client && npm run build)

# 3. Run the server and play
cargo run -p crucible-server
# open http://127.0.0.1:8787
```

That gives you the full game against scripted bots. For a real, evolving
champion, start the trainer too (see *Training the AI* below).

## How to play

- **Left-click** to select; **drag a box** to select several (hold **Shift** to add).
- **Right-click** to move / attack-move; **right-click an enemy unit or
  building to focus-fire it** (the selected combat units lock onto that one
  target and ignore everything else, C&C-style); right-click on one of your
  own production buildings sets its rally point.
- **Middle-drag** pans the camera, **mouse wheel** zooms, **Esc** cancels placement.
- Select a building to train units, research upgrades (Tech Lab), or sell it.
  With nothing selected, the build panel offers PowerPlant, Refinery,
  Barracks, Factory, Tech Lab, Airfield, Radar, Tesla Coil, and Turret.
  Tech Lab and Airfield require a Factory; **Radar and Tesla Coil are the
  second tier and require the Tech Lab itself**. The Tech Lab also unlocks
  three research tracks — High-Explosive (+15% damage), Reinforced Armor
  (+15% HP), and Extended Range (+20% range) — one per player, chosen from
  the lab's command card. The Airfield trains Gunships and Interceptors from
  the aircraft tab — aircraft fly **over buildings** (though not over map
  terrain), so they can strike straight through a base's walls and turret
  line, and everything on the ground can still shoot them down.
- C&C-style cursors: hovering an enemy with attack-capable units selected
  shows a red targeting reticle; the Sell tool shows a **$** over sellable
  buildings and Repair shows a **wrench** over damaged ones.
- Buildings must be placed within a few tiles of an existing one, so your base
  grows as a connected clump (the placement ghost turns green when the spot is
  valid, red when it isn't).
- You see only your own fog-of-war view and are capped at a human-plausible
  120 actions per minute.

You start with a Harvester; it mines the gold crystals and banks ore at a
Refinery (watch the `workers` counter and the `+N/s` income readout in the
top bar — income comes only from harvester deposits; refineries give no
passive trickle).

Watch the power readout too: the HQ and PowerPlants produce power, while
Refineries, Barracks, Factories, Tech Labs, Airfields, and Turrets drain it.
If consumption ever exceeds production, your production lines slow to half
speed — build a PowerPlant to lift the cap, the same way the AI does.

Destroy the enemy HQ to win. Matches have no time limit — the game ends only
when an HQ falls (training matches keep an internal cap so a degenerate
self-play game can't run forever).

## The game

One resource (ore), ten buildings (HQ, PowerPlant, Refinery, Barracks,
Factory, Tech Lab, Airfield, Radar, Tesla Coil, and Turret), seven units
(Harvester, Infantry, Tank, Artillery, Mammoth Tank — the three vehicles and
the mammoth need a Tech Lab — plus Gunship and Interceptor, which need an
Airfield), on procedurally generated 64×64 maps. Radar dishes reveal a huge
swath of the battlefield passively; Tesla Coils are long-range arc turrets.
Maps are point-symmetric (spawn fairness is a theorem) with rich ore-field
cores, occasional mid-field rocks for cover, and varied expansion-site sizes.
The simulation runs at a fixed 10 ticks per second and is fully
deterministic and server-authoritative. Harvesters must dock at a Refinery to
unload ore — they cannot transfer it from a distance.

## Training the AI

The trainer runs inside the server process, in the background. Turn it on with
environment variables:

```bash
# Bootstrap a competent champion from a cold start, then evolve 24/7:
CRUCIBLE_TRAINER=1 CRUCIBLE_TRAINER_BOOTSTRAP=1 cargo run -p crucible-server

# Or a quick, bounded run with a small population:
CRUCIBLE_TRAINER=1 CRUCIBLE_TRAINER_SMALL=1 CRUCIBLE_TRAINER_GENERATIONS=5 cargo run -p crucible-server
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

Watch it learn at `http://127.0.0.1:8787/api/status` (generation, matches run)
and `http://127.0.0.1:8787/api/champion` (current champion + Elo).

## How the AI works

- **The commander, not the soldier.** The evolvable brain is a small neural
  network (~12k weights) that makes strategic decisions on a 2-second tick:
  build, train, expand, attack. Individual units run scripted micro. It plays
  with the same fog of war and APM limit you do. Your own commands are applied
  immediately (within one 100 ms tick), so the game stays responsive even
  though the opponent deliberates slowly.
- **Evolution strategy.** A population of genomes competes in headless
  self-play; the strongest are kept and mutated. No backpropagation; selection
  pressure does the learning.
- **The gauntlet.** A challenger only becomes champion by beating the incumbent
  (and sampled former champions) in a reproducible match series. Elo tracks the
  outcome.
- **Ghosts.** Your replays are replayed as frozen opponents during training, so
  a strategy that beats the champion gets countered within a training cycle.
- **Determinism.** One seeded PRNG, a fixed timestep, integer math, and
  input-log replays mean any match or promotion can be reproduced
  byte-for-byte, natively and in the browser's wasm shim.

## For developers

The workspace is pure-Rust simulation/AI crates plus a no-framework
TypeScript client:

```
crates/
  crucible-sim/          deterministic sim core (no IO)
  crucible-ai/           neural commander + scripted bots
  crucible-evo/          evolution, gauntlet, ghosts, Elo
  crucible-server/       HTTP/WS + trainer + SQLite (the only impure crate)
  crucible-client-wasm/  wasm-bindgen shim for replay spectate
client/                  TypeScript + Vite + Canvas 2D
```

Only `crucible-server` touches the network, filesystem, clock, or threads; the
client never implements game rules.

```bash
# Full local check (the same sequence CI runs)
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p crucible-sim -p crucible-ai -p crucible-evo -p crucible-client-wasm --target wasm32-unknown-unknown
cargo test -p crucible-client-wasm --target wasm32-unknown-unknown   # native/wasm golden parity
(cd client && npm test)
```

CI (`.github/workflows/ci.yml`) enforces all of the above on every push and PR,
including the native/wasm determinism golden-parity tests. Each crate keeps a
`CONTRACT.md` documenting its invariants.
