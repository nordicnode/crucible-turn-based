# CONTRACT — crucible-server

The orchestrator binary and **the only impure crate**. It owns the network,
filesystem, SQLite, wall clock, and threads, and injects them into the pure
crates. **Status: M3 (WS match + SQLite) + M5 (museum/Elo API) + M6 (self-play
trainer, dashboard API, auto-battle) + M7 (ghost pool + focused cycles) +
M9 (champion & museum playable in the live lobby) implemented.**

## 1. Purity split (must hold forever)

- All game rules live in `crucible-sim`. The server never reimplements them.
- Live matches are **server-authoritative**: the server runs the sim at the
  fixed timestep, validates client commands with the *same*
  `Game::validate_command` the AI uses, and broadcasts fogged state. The
  client never runs the authoritative sim.
- The server treats human, AI, and ghost commands identically; there is no
  "trusted client" or "trusted AI" path that skips validation.

## 2. Network boundary

- No outbound network calls at runtime. No telemetry, CDNs, package fetches, or
  external APIs. The server binds a localhost socket and serves the client.
- REST endpoints (v1): `/api/champion`, `/api/elo-history`,
  `/api/lineage`, `/api/museum`, `/api/replays`, `/api/replay/:id`,
  `/api/status`, `POST /api/report/:old/:new`, and
  `POST /api/autobattle/:a/:b`. Diagnostic POST endpoints are serialized,
  capped at a short diagnostic match duration, and run off the async runtime;
  auto-battle persistence is never a GET side effect. No auth in v1
  (localhost); an auth hook is reserved in the router config for a future VPS
  deploy.
- WebSocket live-match protocol (M3): client sends
  `JoinMatch { opponent }` then `Commands { cmds[] }`; server sends
  `MatchStart`, per-tick fogged `StateDiff`, and `MatchEnd { result, replay_id }`.
  Invalid commands receive `CommandRejected { index, reason }`. Incoming
  websocket messages, queued batches, command counts, and move-group sizes
  are bounded before reaching simulation/pathfinding.
- `StateDiff` entities for the human player's buildings carry `queue` (unit
  kind names, oldest first), `progress` (ticks into the current item), and
  `buildTime` (ticks for the current item) so the client can render the build
  queue. Enemy/unit entities omit these fields.
- `StateDiff.events` carries only the connected human player's events; enemy
  activity is represented solely through fog-legal entity sightings.
- Command wire format is pinned by serde derives: `player` is the variant
  name `"P0"`/`"P1"` (NOT an index), `btype`/`utype` are variant names.
  The client pins this contract in `client/src/types.test.ts`, and the
  canonical server-side dump lives in `crucible-sim/examples/wire_probe.rs`.
  A drift here drops commands silently; the server logs such drops as WARN.
- `opponent` is `easy` | `medium` | `hard` (scripted baselines),
  `champion` (the reigning champion genome), or `museum:{genome_id}` (any
  stored genome). A missing genome (e.g. no champion crowned yet) falls back
  to the hard bot rather than erroring.

## 3. Trainer contract

Priority order, always: (1) pending gauntlets, (2) ghost-league cycles,
(3) self-play generations. The trainer yields CPU to live matches instantly.
CPU budget (cores, duty cycle) is config, default all cores @ 60% duty when
idle, 0% during live matches on ≤ 4-core machines. Only the server schedules
rayon parallelism and injects results into `crucible-evo`.

## 4. Storage contract (SQLite)

- Single SQLite file under `data/`. Schema is **versioned**; `store.rs` runs
  migrations at boot. Tables (v1): `genomes`, `champions`, `matches`,
  `elo_history`, `training_stats`, `events`.
- Genomes and replays carry versioned envelopes; old ghosts and old champions
  must stay replayable across migrations.
- **Checkpoints are atomic.** The trainer writes population + lineage + stats
  in a transaction; generation N+1 is only persisted when complete. A crash
  mid-generation must resume cleanly with zero lost/corrupt state.

## 5. Determinism & reproducibility

- Every live, league, ghost, and gauntlet match stores its seed and both
  players' command logs (`matches.replay`), so any result can be re-run
  byte-identically by `crucible-sim` (native or wasm).
- The server may add wall-clock/timestamp metadata to events for the "while you
  were away" feed, but such metadata never affects sim state or outcomes.

## 6. Configuration over constants

CPU budget, APM cap, population size, gauntlet thresholds, and ghost-pool
parameters live in `config.toml` with sane defaults; tests override via a
builder, never by editing source constants.

## 7. Guarantees to operators

- One command runs the server, opens a browser, and plays vs the live champion
  on localhost. Restarts resume training with zero lost state (M3+).
