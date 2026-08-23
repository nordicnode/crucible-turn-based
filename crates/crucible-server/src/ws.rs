//! WebSocket live-match protocol. Server-authoritative: the sim runs here,
//! human commands are validated identically to the bot's, and the client only
//! receives the human player's fogged view.
//!
//! The sim uses alternating activations, while the live protocol exposes a
//! player-facing round boundary. P0 is the human: command batches apply
//! immediately, and an `EndTurn` synchronously drives exactly one P1 bot
//! activation before the server publishes the next P0-ready diff. There is no
//! intermediate bot-state broadcast and no wall-clock tick.

use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;

use crucible_ai::{adaptive, drive_bot_turn, easy, hard, medium, Bot, GenomeBot};
use crucible_sim::{
    entity::{BuildingType, ResourceBundle, ResourceType},
    map::{tile_index, Terrain, MAP_SIZE, MAP_TILES},
    tiles::within_range,
    unit_stats, Command, Game, GameConfig, Map, Player, Replay, ReplayResult, UnitType,
};

use crate::store::Store;

const MAX_WS_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_COMMANDS_PER_MESSAGE: usize = 8;
const MAX_MOVE_GROUP_UNITS: usize = 32;
const MAX_MOVE_GROUP_UNITS_PER_BATCH: usize = 64;
const COMMAND_CHANNEL_CAPACITY: usize = 4;
/// How long an in-match connection may stay idle (no command or inspection)
/// before the server abandons it. A connection that joins but never plays
/// would otherwise squat on a `MAX_LIVE_MATCHES` slot and a socket forever.
/// Overridable for smoke tests via `CRUCIBLE_IDLE_TIMEOUT_SECS`.
const MATCH_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Inputs are kept on one bounded channel so inspection requests cannot create
/// an unbounded side queue while the bot is taking its turn.
enum ClientInput {
    Commands(Vec<Command>),
    InspectTile { x: u8, y: u8 },
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ClientMsg {
    JoinMatch {
        opponent: String,
        player_id: Option<String>,
    },
    Commands {
        cmds: Vec<Command>,
    },
    /// Request the authoritative, fog-filtered facts for one map tile.
    InspectTile {
        x: u8,
        y: u8,
    },
    /// End the active player's turn (only valid from `game.active`; the sim
    /// runs the full lifecycle: turret fire → income → production → opponent).
    EndTurn,
    /// Client keepalive heartbeat. Parsed so the server logs it as a normal
    /// control frame rather than an unparseable message; no reply is needed
    /// (the client's liveness detection rides on any server traffic + TCP
    /// close).
    Ping,
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ServerMsg {
    MatchStart(MatchStartMsg),
    StateDiff(StateDiffMsg),
    TileInspection(TileInspectionMsg),
    CommandRejected(CommandRejectedMsg),
    MatchEnd(MatchEndMsg),
    /// The server is at its concurrent-match capacity; the client should
    /// return to the lobby rather than wait on a dead connection.
    ServerBusy,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct MatchStartMsg {
    map_seed: u64,
    player: u8,
    passable: Vec<bool>,
    /// Serde terrain names per tile ("Plains", "Forest", …) so the client
    /// renders the same typed terrain the sim moves/defends on.
    terrain: Vec<String>,
    /// Authoritative presentation metadata for the tile inspector. The
    /// per-tile `terrain` array remains compact; this table contains one entry
    /// per terrain kind.
    terrain_rules: Vec<TerrainRuleMsg>,
    /// Coarse climate fields explain biome placement to replays/tools and let
    /// clients add texture without inventing map facts.
    elevation: Vec<u8>,
    moisture: Vec<u8>,
    temperature: Vec<u8>,
    hq: [(u8, u8); 2],
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct TerrainRuleMsg {
    kind: String,
    label: String,
    passable: bool,
    move_multiplier: i32,
    defense_reduction: i32,
    tactical_tag: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct StateDiffMsg {
    /// Legacy activation counter (one P0 or P1 activation).
    turn: i32,
    /// Player-facing round containing the current activation pair.
    round: i32,
    /// Index of the player whose activation is current. Human matches publish
    /// only after the bot has resolved, so this is normally P0 on the wire.
    active_player: u8,
    /// Legacy scalar fields retained for older clients.
    ore: i32,
    crystal: i32,
    steel: i32,
    coal: i32,
    /// Authoritative four-resource wallet for the current player.
    resources: ResourceBundle,
    /// Estimated next-turn income, including the HQ and active refineries.
    income: ResourceBundle,
    power_produced: i32,
    power_consumed: i32,
    research: ResearchMsg,
    entities: Vec<DiffEntity>,
    /// Generic visible/known resource deposits. Deposits are infinite; the
    /// legacy `amount` marker remains only for replay/client compatibility.
    resource_tiles: Vec<ResourceTile>,
    /// Legacy split fields retained during the wire transition.
    ore_tiles: Vec<OreTile>,
    crystal_tiles: Vec<CrystalTile>,
    visible: Vec<u16>,
    events: Vec<DiffEvent>,
    /// Actions spent by the active player this turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    actions_spent: Option<i32>,
    /// Max actions per turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    actions_cap: Option<i32>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct MatchEndMsg {
    winner: Option<u8>,
    reason: Option<crucible_sim::WinReason>,
    /// Legacy activation duration.
    duration_turns: i32,
    /// Player-facing round duration.
    duration_rounds: i32,
    replay_id: Option<i64>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct CommandRejectedMsg {
    index: usize,
    reason: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct DiffEntity {
    id: u32,
    kind: String,
    owner: u8,
    /// Tile-center coordinates (tile + 0.5), matching the replay spectate
    /// frames so live and replay rendering share one convention.
    x: f32,
    y: f32,
    hp: i32,
    max_hp: i32,
    /// Turns since this enemy was last seen (own entities are never stale).
    #[serde(skip_serializing_if = "Option::is_none")]
    stale: Option<i32>,
    /// Current and maximum movement points for own units.
    #[serde(skip_serializing_if = "Option::is_none")]
    mp: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_mp: Option<i32>,
    /// Durable movement destination for own units.
    #[serde(skip_serializing_if = "Option::is_none")]
    move_target: Option<(u8, u8)>,
    /// Deterministic route preview for own units.
    #[serde(skip_serializing_if = "Option::is_none")]
    movement_path: Option<Vec<(u8, u8)>>,
    /// Own-building production queue (unit kind names, oldest first).
    #[serde(skip_serializing_if = "Option::is_none")]
    queue: Option<Vec<String>>,
    /// Progress of the current queue head, in turns.
    #[serde(skip_serializing_if = "Option::is_none")]
    progress: Option<i32>,
    /// Build time of the current queue head, in turns.
    #[serde(skip_serializing_if = "Option::is_none")]
    build_time: Option<i32>,
    /// Construction progress of an owned building, in turns.
    #[serde(skip_serializing_if = "Option::is_none")]
    construction_progress: Option<i32>,
    /// Construction duration of an owned building, in turns.
    #[serde(skip_serializing_if = "Option::is_none")]
    construction_time: Option<i32>,
    /// Production rally point (own production buildings only): newly-trained
    /// units auto-march here.
    #[serde(skip_serializing_if = "Option::is_none")]
    rally: Option<(u8, u8)>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct ResourceTile {
    x: u8,
    y: u8,
    resource: ResourceType,
    /// Legacy static marker. It is never a remaining reserve.
    amount: i32,
    richness: u8,
    /// Explicit economy contract: this deposit never depletes.
    infinite: bool,
    /// Base/tech-adjusted extraction estimate for a refinery on this tile.
    yield_per_turn: i32,
    /// Owner of the refinery claiming this tile, if any.
    refinery_owner: Option<u8>,
}

#[derive(Serialize, Clone, Debug)]
struct OreTile {
    x: u8,
    y: u8,
    amount: i32,
}

#[derive(Serialize, Clone, Debug)]
struct CrystalTile {
    x: u8,
    y: u8,
    amount: i32,
}

/// Authoritative tile facts returned after a client selects a tile. Dynamic
/// details are filtered through the same fog view used by state diffs.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct TileInspectionMsg {
    x: u8,
    y: u8,
    index: u16,
    visibility: String,
    terrain: Option<TerrainRuleMsg>,
    /// Climate facts are revealed only for explored tiles, never from hidden
    /// map state.
    elevation: Option<u8>,
    moisture: Option<u8>,
    temperature: Option<u8>,
    resource: Option<ResourceTile>,
    occupants: Vec<DiffEntity>,
    movement: Vec<TileMovementMsg>,
    route_targets: Vec<RouteTargetMsg>,
    placement: PlacementFactsMsg,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct TileMovementMsg {
    unit_id: u32,
    unit_kind: String,
    move_points: i32,
    terrain_cost: i32,
    can_enter: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct RouteTargetMsg {
    unit_id: u32,
    target: (u8, u8),
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct PlacementFactsMsg {
    known: bool,
    passable: Option<bool>,
    occupied_by_building: bool,
    occupied_by_unit: bool,
    resource: Option<ResourceType>,
    within_base_radius: bool,
    structure_site_available: bool,
    refinery_site_available: bool,
}

/// The player's research dashboard: the accruing point pool, the tech being
/// worked on (serde name, if any), and the completed technologies.
#[derive(Serialize, Clone, Debug)]
struct ResearchMsg {
    points: i32,
    researching: Option<String>,
    researched: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
struct DiffEvent {
    turn: i32,
    round: i32,
    kind: String,
    /// Amount for a `mined` / `sold` / `attacked` event (null otherwise) —
    /// lets the client show per-turn income and authoritative damage numbers.
    #[serde(skip_serializing_if = "Option::is_none")]
    amount: Option<i32>,
    /// Attacker entity id for `attacked` events, so the client can animate the
    /// projectile from the real shooter (not a nearest-enemy guess).
    #[serde(skip_serializing_if = "Option::is_none")]
    attacker: Option<u32>,
    /// Target entity id for `attacked` events.
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<u32>,
    /// Index of the player associated with this event (0 = P0, 1 = P1).
    #[serde(skip_serializing_if = "Option::is_none")]
    player: Option<u8>,
}

pub async fn handler(
    ws: WebSocketUpgrade,
    State(state): State<crate::AppState>,
) -> impl IntoResponse {
    ws.max_message_size(MAX_WS_MESSAGE_BYTES)
        .on_upgrade(move |socket| handle(socket, state))
}

async fn handle(socket: WebSocket, state: crate::AppState) {
    if let Err(e) = run(socket, state).await {
        tracing::warn!("ws session ended with error: {e}");
    }
}

async fn run(
    socket: WebSocket,
    state: crate::AppState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut sender, mut receiver) = socket.split();

    // Wait for JoinMatch, with a deadline: a connection that never greets
    // would otherwise squat on its socket (and file descriptor) forever.
    let (mut opponent, player_id) = match tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match receiver.next().await {
                Some(Ok(Message::Text(t))) => {
                    if let Ok(ClientMsg::JoinMatch {
                        opponent,
                        player_id,
                    }) = serde_json::from_str(&t)
                    {
                        return Some((opponent, player_id));
                    }
                }
                Some(Ok(Message::Close(_))) | None => return None,
                _ => continue,
            }
        }
    })
    .await
    {
        Ok(Some((opponent, player_id))) => (opponent, player_id),
        Ok(None) => return Ok(()), // client closed before joining
        Err(_) => {
            tracing::debug!("ws session timed out waiting for JoinMatch");
            let _ = sender.send(Message::Close(None)).await;
            return Ok(());
        }
    };

    // Cap concurrent live matches: every connection runs a full sim, so
    // unbounded sessions would exhaust CPU if the server were ever exposed
    // beyond localhost. A busy server tells the client instead of hanging.
    let _match_permit = match state.live_matches.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            sender
                .send(Message::Text(
                    serde_json::to_string(&ServerMsg::ServerBusy)?.into(),
                ))
                .await?;
            return Ok(());
        }
    };

    // F2 save/resume: `opponent == "saved"` restores the most recent live
    // match snapshot from the store (single-use) and continues it.
    let store = state.store.clone();
    let mut resume: Option<(String, Game, Replay)> = None;
    if opponent == "saved" {
        let store2 = store.clone();
        resume = tokio::task::spawn_blocking(move || {
            let save = store2.latest_save().ok().flatten()?;
            let game: Game = serde_json::from_str(&save.game_json)
                .ok()
                .filter(|g: &Game| g.validate())?;
            let mut replay = Replay::from_json(&save.replay_json).ok()?;
            replay.result = None; // the resumed match re-records its outcome
            let _ = store2.delete_save(&save.key); // saves are single-use
            Some((save.opponent, game, replay))
        })
        .await
        .ok()
        .flatten();
    }

    // Resolve the opponent off the async runtime: it reads the store (SQLite
    // behind a mutex), which can stall live match handling if contended.
    let opponent_name = resume
        .as_ref()
        .map_or_else(|| opponent.clone(), |(opp, _, _)| opp.clone());
    let store = state.store.clone();
    let mut bot: Box<dyn Bot> =
        match tokio::task::spawn_blocking(move || resolve_opponent(&store, &opponent_name)).await {
            Ok(bot) => bot,
            Err(e) => {
                tracing::error!("opponent resolution failed: {e}");
                Box::new(hard())
            }
        };

    // Seed from the wall clock (server is the one place this is allowed); the
    // seed is recorded in the replay so the match stays reproducible. A
    // resumed match keeps its original seed and command log.
    let (seed, mut game, mut replay) = match resume {
        Some((opp, game, replay)) => {
            opponent = opp;
            (game.map.seed, game, replay)
        }
        None => {
            let seed = seed_now();
            let config = timeout_override(GameConfig::default());
            let game = Game::new(Map::generate(seed), config.clone());
            let replay = Replay::new(seed, config);
            (seed, game, replay)
        }
    };

    let passable = game.map.passable.clone();
    let terrain: Vec<String> = game.map.terrain.iter().map(|t| format!("{t:?}")).collect();
    let terrain_rules = terrain_rules();
    let hq = [game.map.hq_tiles[0], game.map.hq_tiles[1]];

    sender
        .send(Message::Text(
            serde_json::to_string(&ServerMsg::MatchStart(MatchStartMsg {
                map_seed: seed,
                player: Player::P0.index() as u8,
                passable,
                terrain,
                terrain_rules,
                elevation: game.map.elevation.clone(),
                moisture: game.map.moisture.clone(),
                temperature: game.map.temperature.clone(),
                hq,
            }))?
            .into(),
        ))
        .await?;

    // Incoming commands and tile-inspection requests are buffered on a
    // bounded channel by a reader task. If a client floods faster than the
    // match loop drains, some inputs are dropped; the counter lets the loop
    // surface that to the client instead of dropping silently (raw sink writes
    // from the reader task would race the match loop, so the reader only
    // counts and the loop sends the rejection).
    let (tx, mut rx) = mpsc::channel::<ClientInput>(COMMAND_CHANNEL_CAPACITY);
    let dropped: Arc<std::sync::atomic::AtomicUsize> = Arc::new(Default::default());
    let dropped_reader = dropped.clone();
    tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Text(t) = msg {
                match serde_json::from_str::<ClientMsg>(&t) {
                    Ok(ClientMsg::Commands { cmds }) => {
                        if !command_batch_is_bounded(&cmds) {
                            tracing::warn!("dropping oversized command batch");
                            continue;
                        }
                        if tx.try_send(ClientInput::Commands(cmds)).is_err() {
                            dropped_reader.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            tracing::warn!("dropping command batch: pending input limit reached");
                        }
                    }
                    Ok(ClientMsg::EndTurn) => {
                        // EndTurn is free (costs no action budget) and carries
                        // no payload; forward it as its own batch.
                        if tx
                            .try_send(ClientInput::Commands(vec![Command::EndTurn {
                                player: Player::P0,
                            }]))
                            .is_err()
                        {
                            dropped_reader.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            tracing::warn!("dropping EndTurn: pending input limit reached");
                        }
                    }
                    Ok(ClientMsg::InspectTile { x, y }) => {
                        if tx.try_send(ClientInput::InspectTile { x, y }).is_err() {
                            dropped_reader.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            tracing::warn!("dropping tile inspection: pending input limit reached");
                        }
                    }
                    Ok(ClientMsg::JoinMatch { .. }) => {}
                    Ok(ClientMsg::Ping) => {}
                    // Never drop a malformed command silently: a wire-format
                    // drift (e.g. player as index vs "P0") otherwise looks
                    // like the game ignoring the player.
                    Err(e) => tracing::warn!("dropping unparseable client message: {e}: {t}"),
                }
            }
        }
    });

    let mut last_event_turn = -1i32;

    // Broadcast the player's starting position immediately, before waiting
    // for any input: the client needs the base, its terrain, and its resources
    // from the very first frame (there is no realtime tick pump to surface
    // them otherwise — without this the player stares at empty fog until they
    // happen to issue a command).
    let initial_diff = build_diff(&game, &mut last_event_turn);
    sender
        .send(Message::Text(serde_json::to_string(&initial_diff)?.into()))
        .await?;

    // Run the match. If the client disconnects (or a send fails) mid-match,
    // the error surfaces here with the replay state intact, so the match is
    // still persisted as a partial replay instead of being lost entirely.
    let result = match (async {
        loop {
            // Surface any inputs the reader had to drop because the client
            // out-paced the match loop, so a dropped command never looks like
            // the game silently ignoring the player. CommandRejected is only
            // ever sent from here (the reader owns the sink-free channel side).
            if dropped.swap(0, std::sync::atomic::Ordering::Relaxed) > 0 {
                sender
                    .send(Message::Text(
                        serde_json::to_string(&ServerMsg::CommandRejected(CommandRejectedMsg {
                            index: 0,
                            reason: "one or more commands dropped: pending input limit reached"
                                .into(),
                        }))?
                        .into(),
                    ))
                    .await?;
            }
            // P0's activation: wait for their next batch with an idle deadline.
            // P0's activation: wait for their next batch with an idle deadline.
            // Nothing happens between batches — the sim has no wall clock. A
            // silent connection is reaped (abandoned + saved) rather than
            // squatting on a live-match slot indefinitely.
            let human_cmds = match tokio::time::timeout(MATCH_IDLE_TIMEOUT, rx.recv()).await {
                Err(_) => {
                    tracing::debug!("live match idled out; abandoning");
                    return Err("client idle: abandoned by server".into());
                }
                Ok(None) => return Err("client closed the connection".into()),
                Ok(Some(ClientInput::InspectTile { x, y })) => {
                    if let Some(inspection) = build_tile_inspection(&game, x, y) {
                        sender
                            .send(Message::Text(
                                serde_json::to_string(&ServerMsg::TileInspection(inspection))?
                                    .into(),
                            ))
                            .await?;
                    }
                    continue;
                }
                Ok(Some(ClientInput::Commands(cmds))) => cmds,
            };
            if human_cmds.is_empty() {
                continue;
            }
            let human_turn = game.turn;
            let human_round = game.round;
            for c in &human_cmds {
                replay.record_at(human_round, human_turn, Player::P0, c.clone());
            }
            for (index, result) in game
                .apply_commands(Player::P0, &human_cmds)
                .into_iter()
                .enumerate()
            {
                if let Err(reason) = result {
                    sender
                        .send(Message::Text(
                            serde_json::to_string(&ServerMsg::CommandRejected(
                                CommandRejectedMsg {
                                    index,
                                    reason: reason.to_string(),
                                },
                            ))?
                            .into(),
                        ))
                        .await?;
                }
            }
            if game.is_over() {
                return Ok(ReplayResult {
                    winner: game.winner,
                    reason: game.win_reason,
                    duration_turns: game.turn,
                    duration_rounds: game.round,
                });
            }

            // EndTurn is a round boundary for the live protocol. Resolve the
            // bot's complete activation before publishing a diff, so the
            // client never observes a second, half-turn state.
            if game.active == Player::P1 && !game.is_over() {
                let bot_turn = game.turn;
                let bot_round = game.round;
                let bot_cmds = drive_bot_turn(&mut game, Player::P1, bot.as_mut());
                for c in bot_cmds {
                    replay.record_at(bot_round, bot_turn, Player::P1, c);
                }
            }
            if game.is_over() {
                return Ok(ReplayResult {
                    winner: game.winner,
                    reason: game.win_reason,
                    duration_turns: game.turn,
                    duration_rounds: game.round,
                });
            }

            // Publish exactly one post-activation state. For a normal human
            // match this is P0-ready and starts the next player-facing round.
            let diff = build_diff(&game, &mut last_event_turn);
            sender
                .send(Message::Text(serde_json::to_string(&diff)?.into()))
                .await?;
        }
    })
    .await
    {
        Ok(result) => result,
        Err(e) => {
            // Client disconnected (or a send failed) mid-match: persist a
            // partial replay so the match isn't lost, then propagate. If the
            // match is still live, snapshot it too so the player can resume.
            if !game.is_over() {
                save_live(&state, seed, &opponent, &game, &replay).await;
            }
            save_replay(&state, seed, &opponent, &game, &replay, None).await;
            if let Some(pid) = player_id.as_deref() {
                crate::personalize::record_match(&state.store, pid, &replay, None, &opponent);
            }
            return Err(e);
        }
    };

    // Match ended normally: record the result, persist the replay, and tell
    // the client. A failing final send (client already gone) is not fatal.
    replay.result = Some(result.clone());
    let replay_id = save_replay(&state, seed, &opponent, &game, &replay, Some(&result)).await;
    if let Some(pid) = player_id.as_deref() {
        crate::personalize::record_match(&state.store, pid, &replay, result.winner, &opponent);
    }
    let _ = sender
        .send(Message::Text(
            serde_json::to_string(&ServerMsg::MatchEnd(MatchEndMsg {
                winner: game.winner.map(|p| p.index() as u8),
                reason: game.win_reason,
                duration_turns: game.turn,
                duration_rounds: game.round,
                replay_id,
            }))?
            .into(),
        ))
        .await;
    Ok(())
}

/// F2: snapshot an in-progress match so the player can resume it later. The
/// game state plus the partial replay (seed + command log) are stored so a
/// resumed match continues recording seamlessly.
async fn save_live(
    state: &crate::AppState,
    seed: u64,
    opponent: &str,
    game: &Game,
    replay: &Replay,
) {
    let game_json = crucible_sim::serialize::snapshot_json(game);
    let replay_json = replay.to_json();
    let store = state.store.clone();
    let key = format!("save:{seed}");
    let opp = opponent.to_string();
    match tokio::task::spawn_blocking(move || store.save_game(&key, &opp, &game_json, &replay_json))
        .await
    {
        Ok(Ok(())) => tracing::info!("saved live match for resume (seed {seed})"),
        Ok(Err(e)) => tracing::error!("failed to save live match: {e}"),
        Err(e) => tracing::error!("live save task failed: {e}"),
    }
}

/// Persist a match's replay (finished or aborted by a disconnect) off the
/// async runtime so the store mutex never stalls the match loop. Failures are
/// logged, never fatal.
async fn save_replay(
    state: &crate::AppState,
    seed: u64,
    opponent: &str,
    game: &Game,
    replay: &Replay,
    result: Option<&ReplayResult>,
) -> Option<i64> {
    let mut final_replay = replay.clone();
    final_replay.result = result.cloned();
    let store = state.store.clone();
    let p1_type = format!("bot:{opponent}");
    // Canonical result label ("P0"/"P1"/"draw"; "abandoned" when the client
    // disconnected mid-match). The ghost pool keys off these exact strings.
    let result_str = match result {
        Some(r) => crate::store::result_label(r.winner),
        None => "abandoned".to_string(),
    };
    let duration_turns = game.turn;
    let json = final_replay.to_json();
    match tokio::task::spawn_blocking(move || {
        store.save_match(seed, "human", &p1_type, &result_str, duration_turns, &json)
    })
    .await
    {
        Ok(Ok(id)) => Some(id),
        Ok(Err(e)) => {
            tracing::error!("failed to save match replay: {e}");
            None
        }
        Err(e) => {
            tracing::error!("replay save task failed: {e}");
            None
        }
    }
}

fn build_tile_inspection(game: &Game, x: u8, y: u8) -> Option<TileInspectionMsg> {
    if x >= crucible_sim::map::MAP_SIZE as u8 || y >= crucible_sim::map::MAP_SIZE as u8 {
        return None;
    }

    let view = game.fog_view(Player::P0);
    let idx = tile_index(x, y);
    let visible = view.visible[idx];
    let explored = view.explored.get(idx).copied().unwrap_or(false);
    let known = visible || explored;
    let visibility = if visible {
        "visible"
    } else if explored {
        "explored"
    } else {
        "unexplored"
    }
    .to_string();

    let terrain = known.then(|| terrain_rule(game.map.terrain_at(x, y)));
    let resource_kind = if known {
        game.map
            .resource_at(x, y)
            .filter(|resource| visible || resource_is_known(&view, idx, *resource))
    } else {
        None
    };
    let resource = resource_kind.and_then(|resource| resource_tile(game, x, y, resource));

    let mut occupants = Vec::new();
    if known {
        for unit in game
            .units
            .iter()
            .filter(|unit| unit.is_alive() && unit.tile == (x, y))
        {
            if unit.owner == Player::P0 {
                occupants.push(diff_unit(game, unit, None));
            }
        }
        for building in game
            .buildings
            .iter()
            .filter(|building| building.is_alive() && building.tile == (x, y))
        {
            if building.owner == Player::P0 {
                occupants.push(diff_building(game, building));
            }
        }
        // Enemy entities come from fog memory, never from the hidden live
        // game. A stale occupant is useful context but has no secret HP or
        // queue details.
        for remembered in &view.units {
            if remembered.tile == (x, y) {
                occupants.push(DiffEntity {
                    id: remembered.id,
                    kind: unit_kind(remembered.utype),
                    owner: 1,
                    x: x as f32 + 0.5,
                    y: y as f32 + 0.5,
                    hp: 0,
                    max_hp: 0,
                    stale: Some(game.turn - remembered.last_seen),
                    mp: None,
                    max_mp: None,
                    move_target: None,
                    movement_path: None,
                    queue: None,
                    progress: None,
                    build_time: None,
                    construction_progress: None,
                    construction_time: None,
                    rally: None,
                });
            }
        }
        for remembered in &view.buildings {
            if remembered.tile == (x, y) {
                occupants.push(DiffEntity {
                    id: remembered.id,
                    kind: building_kind(remembered.btype),
                    owner: 1,
                    x: x as f32 + 0.5,
                    y: y as f32 + 0.5,
                    hp: 0,
                    max_hp: 0,
                    stale: Some(game.turn - remembered.last_seen),
                    mp: None,
                    max_mp: None,
                    move_target: None,
                    movement_path: None,
                    queue: None,
                    progress: None,
                    build_time: None,
                    construction_progress: None,
                    construction_time: None,
                    rally: None,
                });
            }
        }
    }
    occupants.sort_by_key(|entity| entity.id);

    let mut movement = Vec::new();
    let mut route_targets = Vec::new();
    if known {
        let tile_occupied_by_building = game
            .buildings
            .iter()
            .any(|building| building.is_alive() && building.tile == (x, y));
        let tile_occupied_by_unit = game
            .units
            .iter()
            .any(|unit| unit.is_alive() && unit.tile == (x, y));
        let tile_terrain = game.map.terrain_at(x, y);
        for unit in game
            .units
            .iter()
            .filter(|unit| unit.owner == Player::P0 && unit.is_alive())
        {
            let dx = (unit.tile.0 as i32 - x as i32).abs();
            let dy = (unit.tile.1 as i32 - y as i32).abs();
            let adjacent = dx <= 1 && dy <= 1;
            let base_cost = if dx == 0 && dy == 0 {
                0
            } else if adjacent && dx != 0 && dy != 0 {
                2
            } else if adjacent {
                1
            } else {
                tile_terrain.move_mult()
            };
            let terrain_cost = if adjacent {
                game.map
                    .move_cost(unit.tile, (x, y), unit_stats(unit.utype).air)
            } else {
                base_cost
                    * if unit_stats(unit.utype).air {
                        1
                    } else {
                        tile_terrain.move_mult()
                    }
            };
            let can_enter = tile_terrain.is_passable()
                && (!tile_occupied_by_building || unit_stats(unit.utype).air)
                && (!tile_occupied_by_unit || unit.tile == (x, y))
                && (terrain_cost == 0 || terrain_cost <= unit.mp);
            movement.push(TileMovementMsg {
                unit_id: unit.id,
                unit_kind: unit_kind(unit.utype),
                move_points: unit.mp,
                terrain_cost,
                can_enter,
            });
            if let Some(target) = unit.move_target {
                route_targets.push(RouteTargetMsg {
                    unit_id: unit.id,
                    target,
                });
            }
        }
    }

    let occupied_by_building = game
        .buildings
        .iter()
        .any(|building| building.is_alive() && building.tile == (x, y));
    let occupied_by_unit = game
        .units
        .iter()
        .any(|unit| unit.is_alive() && unit.tile == (x, y));
    let within_base_radius = known
        && game.buildings.iter().any(|building| {
            building.owner == Player::P0
                && building.is_alive()
                && within_range(building.tile.0, building.tile.1, x, y, 5)
        });
    let placement = if known {
        let passable = game.map.is_passable(x, y);
        let no_resource = resource_kind.is_none();
        PlacementFactsMsg {
            known: true,
            passable: Some(passable),
            occupied_by_building,
            occupied_by_unit,
            resource: resource_kind,
            within_base_radius,
            structure_site_available: passable
                && !occupied_by_building
                && !occupied_by_unit
                && no_resource
                && within_base_radius,
            refinery_site_available: passable
                && !occupied_by_building
                && !occupied_by_unit
                && resource.as_ref().is_some_and(|tile| tile.amount > 0)
                && game.can_afford(
                    Player::P0,
                    crucible_sim::building_stats(BuildingType::Refinery).resource_cost,
                ),
        }
    } else {
        PlacementFactsMsg {
            known: false,
            passable: None,
            occupied_by_building: false,
            occupied_by_unit: false,
            resource: None,
            within_base_radius: false,
            structure_site_available: false,
            refinery_site_available: false,
        }
    };

    Some(TileInspectionMsg {
        x,
        y,
        index: idx as u16,
        visibility,
        terrain,
        elevation: known.then(|| game.map.elevation.get(idx).copied().unwrap_or(0)),
        moisture: known.then(|| game.map.moisture.get(idx).copied().unwrap_or(0)),
        temperature: known.then(|| game.map.temperature.get(idx).copied().unwrap_or(0)),
        resource,
        occupants,
        movement,
        route_targets,
        placement,
    })
}

fn resource_is_known(
    view: &crucible_sim::fog::FogView,
    idx: usize,
    resource: ResourceType,
) -> bool {
    match resource {
        ResourceType::Ore => view.known_ore[idx],
        ResourceType::Steel => view.known_steel[idx],
        ResourceType::Coal => view.known_coal[idx],
        ResourceType::Crystal => view.known_crystal[idx],
    }
}

fn resource_tile(game: &Game, x: u8, y: u8, resource: ResourceType) -> Option<ResourceTile> {
    let amount = game.map.resource_amount_at(x, y);
    let richness = game.map.resource_richness_at(x, y);
    let refinery_owner = game
        .buildings
        .iter()
        .find(|building| {
            building.is_alive() && building.btype.is_refinery() && building.tile == (x, y)
        })
        .map(|building| building.owner.index() as u8);
    let yield_per_turn = refinery_owner
        .map(|owner| {
            game.refinery_yield(resource, richness)
                * game
                    .tech_effects(if owner == 0 { Player::P0 } else { Player::P1 })
                    .yield_num
                / 100
        })
        .unwrap_or_else(|| game.refinery_yield(resource, richness));
    Some(ResourceTile {
        x,
        y,
        resource,
        amount,
        richness,
        infinite: true,
        yield_per_turn,
        refinery_owner,
    })
}

fn diff_unit(game: &Game, unit: &crucible_sim::Unit, stale: Option<i32>) -> DiffEntity {
    DiffEntity {
        id: unit.id,
        kind: unit_kind(unit.utype),
        owner: unit.owner.index() as u8,
        x: unit.tile.0 as f32 + 0.5,
        y: unit.tile.1 as f32 + 0.5,
        hp: if stale.is_some() { 0 } else { unit.hp },
        max_hp: if stale.is_some() { 0 } else { unit.max_hp },
        stale,
        mp: if stale.is_some() { None } else { Some(unit.mp) },
        max_mp: if stale.is_some() {
            None
        } else {
            Some(crucible_sim::unit_stats(unit.utype).mp)
        },
        move_target: if stale.is_some() {
            None
        } else {
            unit.move_target
        },
        movement_path: if stale.is_some() {
            None
        } else {
            game.movement_path(unit.id)
        },
        queue: None,
        progress: None,
        build_time: None,
        construction_progress: None,
        construction_time: None,
        rally: None,
    }
}

fn diff_building(_game: &Game, building: &crucible_sim::Building) -> DiffEntity {
    let (queue, progress, build_time) = if building.queue.is_empty() {
        (None, None, None)
    } else {
        let head = building.queue[0];
        (
            Some(
                building
                    .queue
                    .iter()
                    .map(|unit| format!("{unit:?}"))
                    .collect(),
            ),
            Some(building.progress),
            Some(crucible_sim::unit_stats(head).build_time_turns),
        )
    };
    DiffEntity {
        id: building.id,
        kind: building_kind(building.btype),
        owner: building.owner.index() as u8,
        x: building.tile.0 as f32 + 0.5,
        y: building.tile.1 as f32 + 0.5,
        hp: building.hp,
        max_hp: building.max_hp,
        stale: None,
        mp: None,
        max_mp: None,
        move_target: None,
        movement_path: None,
        queue,
        progress,
        build_time,
        construction_progress: Some(building.construction_progress),
        construction_time: Some(building.construction_time()),
        rally: building.rally,
    }
}

fn build_diff(game: &Game, last_event_turn: &mut i32) -> ServerMsg {
    let view = game.fog_view(Player::P0);
    let mut entities = Vec::new();

    for u in &game.units {
        if u.owner == Player::P0 {
            entities.push(DiffEntity {
                id: u.id,
                kind: unit_kind(u.utype),
                owner: 0,
                x: u.tile.0 as f32 + 0.5,
                y: u.tile.1 as f32 + 0.5,
                hp: u.hp,
                max_hp: u.max_hp,
                stale: None,
                mp: Some(u.mp),
                max_mp: Some(crucible_sim::unit_stats(u.utype).mp),
                move_target: u.move_target,
                movement_path: game.movement_path(u.id),
                queue: None,
                progress: None,
                build_time: None,
                construction_progress: None,
                construction_time: None,
                rally: None,
            });
        }
    }
    for b in &game.buildings {
        if b.owner == Player::P0 {
            let (queue, progress, build_time) = if !b.queue.is_empty() {
                let head = b.queue[0];
                (
                    Some(b.queue.iter().map(|u| format!("{u:?}")).collect::<Vec<_>>()),
                    Some(b.progress),
                    Some(crucible_sim::unit_stats(head).build_time_turns),
                )
            } else {
                (None, None, None)
            };
            entities.push(DiffEntity {
                id: b.id,
                kind: building_kind(b.btype),
                owner: 0,
                x: b.tile.0 as f32 + 0.5,
                y: b.tile.1 as f32 + 0.5,
                hp: b.hp,
                max_hp: b.max_hp,
                stale: None,
                mp: None,
                max_mp: None,
                move_target: None,
                movement_path: None,
                queue,
                progress,
                build_time,
                construction_progress: Some(b.construction_progress),
                construction_time: Some(b.construction_time()),
                rally: b.rally,
            });
        }
    }
    // Enemy: only what the fog view exposes (last-seen + currently visible).
    for m in &view.units {
        entities.push(DiffEntity {
            id: m.id,
            kind: unit_kind(m.utype),
            owner: 1,
            x: m.tile.0 as f32 + 0.5,
            y: m.tile.1 as f32 + 0.5,
            hp: 0,
            max_hp: 0,
            stale: Some(game.turn - m.last_seen),
            mp: None,
            max_mp: None,
            move_target: None,
            movement_path: None,
            queue: None,
            progress: None,
            build_time: None,
            construction_progress: None,
            construction_time: None,
            rally: None,
        });
    }
    for m in &view.buildings {
        entities.push(DiffEntity {
            id: m.id,
            kind: building_kind(m.btype),
            owner: 1,
            x: m.tile.0 as f32 + 0.5,
            y: m.tile.1 as f32 + 0.5,
            hp: 0,
            max_hp: 0,
            stale: Some(game.turn - m.last_seen),
            mp: None,
            max_mp: None,
            move_target: None,
            movement_path: None,
            queue: None,
            progress: None,
            build_time: None,
            construction_progress: None,
            construction_time: None,
            rally: None,
        });
    }
    let mut resource_tiles = Vec::new();
    for idx in 0..MAP_TILES {
        let x = (idx % MAP_SIZE) as u8;
        let y = (idx / MAP_SIZE) as u8;
        let Some(resource) = game.map.resource_at(x, y) else {
            continue;
        };
        let known = match resource {
            ResourceType::Ore => view.known_ore[idx],
            ResourceType::Steel => view.known_steel[idx],
            ResourceType::Coal => view.known_coal[idx],
            ResourceType::Crystal => view.known_crystal[idx],
        };
        let amount = game.map.resource_amount_at(x, y);
        if known && game.map.has_resource_at(x, y) {
            let richness = game.map.resource_richness_at(x, y);
            let refinery_owner = game
                .buildings
                .iter()
                .find(|building| {
                    building.is_alive() && building.btype.is_refinery() && building.tile == (x, y)
                })
                .map(|building| building.owner.index() as u8);
            let yield_per_turn = refinery_owner
                .map(|owner| {
                    game.refinery_yield(resource, richness)
                        * game
                            .tech_effects(if owner == 0 { Player::P0 } else { Player::P1 })
                            .yield_num
                        / 100
                })
                .unwrap_or_else(|| game.refinery_yield(resource, richness));
            resource_tiles.push(ResourceTile {
                x,
                y,
                resource,
                amount,
                richness,
                infinite: true,
                yield_per_turn,
                refinery_owner,
            });
        }
    }
    // Split legacy projections for older browser builds.
    let ore_tiles: Vec<OreTile> = resource_tiles
        .iter()
        .filter(|t| t.resource == ResourceType::Ore)
        .map(|t| OreTile {
            x: t.x,
            y: t.y,
            amount: t.amount,
        })
        .collect();
    let crystal_tiles: Vec<CrystalTile> = resource_tiles
        .iter()
        .filter(|t| t.resource == ResourceType::Crystal)
        .map(|t| CrystalTile {
            x: t.x,
            y: t.y,
            amount: t.amount,
        })
        .collect();

    let visible: Vec<u16> = view
        .visible
        .iter()
        .enumerate()
        .filter(|(_, v)| **v)
        .map(|(i, _)| i as u16)
        .collect();

    let events: Vec<DiffEvent> = game
        .events
        .iter()
        .filter(|e| e.turn > *last_event_turn && event_player(game, &e.kind) == Some(Player::P0))
        .map(|e| DiffEvent {
            turn: e.turn,
            round: (e.turn + 1).div_euclid(2).max(1),
            kind: event_kind(&e.kind),
            amount: match &e.kind {
                crucible_sim::EventKind::ResourceMined { amount, .. }
                | crucible_sim::EventKind::OreMined { amount, .. }
                | crucible_sim::EventKind::CrystalMined { amount, .. } => Some(*amount),
                crucible_sim::EventKind::Sold { refund, .. } => Some(refund.total_value()),
                crucible_sim::EventKind::Attacked { damage, .. } => Some(*damage),
                _ => None,
            },
            attacker: match &e.kind {
                crucible_sim::EventKind::Attacked { attacker, .. } => Some(*attacker),
                _ => None,
            },
            target: match &e.kind {
                crucible_sim::EventKind::Attacked { target, .. } => Some(*target),
                _ => None,
            },
            player: event_player(game, &e.kind).map(|player| player.index() as u8),
        })
        .collect();
    *last_event_turn = game.turn;
    let (power_produced, power_consumed) = game.power(crucible_sim::Player::P0);

    let research = &game.research[0];
    ServerMsg::StateDiff(StateDiffMsg {
        turn: game.turn,
        round: game.round,
        active_player: game.active.index() as u8,
        ore: game.ore[0],
        crystal: game.crystal[0],
        steel: game.steel[0],
        coal: game.coal[0],
        resources: game.resources(Player::P0),
        income: game.resource_income(Player::P0),
        power_produced,
        power_consumed,
        research: ResearchMsg {
            points: research.points,
            researching: research.researching.map(|t| format!("{t:?}")),
            researched: research
                .researched
                .iter()
                .map(|t| format!("{t:?}"))
                .collect(),
        },
        entities,
        resource_tiles,
        ore_tiles,
        crystal_tiles,
        visible,
        events,
        actions_spent: Some(game.budgets[0].spent()),
        actions_cap: Some(game.budgets[0].cap()),
    })
}

/// The player an event belongs to. Attacks are attributed to the *defender's*
/// owner (resolved against the live game state) so the client sees, and can
/// log, enemy strikes on its own units.
fn event_player(game: &Game, event: &crucible_sim::EventKind) -> Option<Player> {
    match event {
        crucible_sim::EventKind::BuildingPlaced { player, .. }
        | crucible_sim::EventKind::UnitTrained { player, .. }
        | crucible_sim::EventKind::ResourceMined { player, .. }
        | crucible_sim::EventKind::OreMined { player, .. }
        | crucible_sim::EventKind::CrystalMined { player, .. }
        | crucible_sim::EventKind::ResearchStarted { player, .. }
        | crucible_sim::EventKind::ResearchComplete { player, .. }
        | crucible_sim::EventKind::Sold { player, .. } => Some(*player),
        crucible_sim::EventKind::UnitDied { owner, .. }
        | crucible_sim::EventKind::BuildingDestroyed { owner, .. } => Some(*owner),
        crucible_sim::EventKind::Attacked {
            attacker,
            target,
            attacker_owner: stored_attacker_owner,
            target_owner: stored_target_owner,
            ..
        } => {
            // Prefer the owners captured at resolution time: a killing blow's
            // target is swept from the world before the diff is built, so the
            // live lookup below can't tell which side it was. Fall back for
            // events serialized before the owner fields existed.
            let attacker_owner = stored_attacker_owner.or_else(|| {
                game.any_unit(*attacker)
                    .map(|u| u.owner)
                    .or_else(|| game.any_building(*attacker).map(|b| b.owner))
            });
            let target_owner = stored_target_owner.or_else(|| {
                game.any_unit(*target)
                    .map(|u| u.owner)
                    .or_else(|| game.any_building(*target).map(|b| b.owner))
            });
            // Deliver a combat event to a player whenever *either* side is
            // theirs, so the client can animate both outgoing and incoming fire.
            match (attacker_owner, target_owner) {
                (Some(Player::P0), _) | (_, Some(Player::P0)) => Some(Player::P0),
                (a, t) => a.or(t),
            }
        }
    }
}

fn command_batch_is_bounded(cmds: &[Command]) -> bool {
    if cmds.len() > MAX_COMMANDS_PER_MESSAGE {
        return false;
    }
    let mut total_unit_ids = 0usize;
    for cmd in cmds {
        // `Attack` carries a unit group too — bound it identically so a
        // malformed batch can't smuggle an oversized id list past the cap.
        let unit_ids = match cmd {
            Command::MoveGroup { units, .. }
            | Command::ClearMove { units, .. }
            | Command::Attack { units, .. } => units,
            _ => continue,
        };
        if unit_ids.len() > MAX_MOVE_GROUP_UNITS {
            return false;
        }
        total_unit_ids += unit_ids.len();
        if total_unit_ids > MAX_MOVE_GROUP_UNITS_PER_BATCH {
            return false;
        }
    }
    true
}

fn terrain_rule(terrain: Terrain) -> TerrainRuleMsg {
    TerrainRuleMsg {
        kind: format!("{terrain:?}"),
        label: terrain.label().to_string(),
        passable: terrain.is_passable(),
        move_multiplier: terrain.move_mult(),
        defense_reduction: terrain.defense_reduction(),
        tactical_tag: terrain.tactical_tag().to_string(),
    }
}

fn terrain_rules() -> Vec<TerrainRuleMsg> {
    Terrain::ALL.into_iter().map(terrain_rule).collect()
}

fn unit_kind(u: UnitType) -> String {
    // Serde variant name, matching both the snapshot format and the client's
    // renderer/selection kind strings ("Infantry", "Tank", …).
    format!("{u:?}")
}

fn building_kind(b: BuildingType) -> String {
    format!("{b:?}")
}

fn event_kind(e: &crucible_sim::EventKind) -> String {
    match e {
        crucible_sim::EventKind::UnitTrained { utype, .. } => {
            format!("trained:{utype:?}").to_lowercase()
        }
        crucible_sim::EventKind::UnitDied { .. } => "unit_died".into(),
        crucible_sim::EventKind::BuildingDestroyed { .. } => "building_destroyed".into(),
        crucible_sim::EventKind::ResourceMined { resource, .. } => {
            format!("mined:{resource:?}").to_lowercase()
        }
        crucible_sim::EventKind::OreMined { .. } => "ore_mined".into(),
        crucible_sim::EventKind::CrystalMined { .. } => "crystal_mined".into(),
        crucible_sim::EventKind::BuildingPlaced { btype, .. } => {
            format!("built:{btype:?}").to_lowercase()
        }
        crucible_sim::EventKind::Sold { .. } => "sold".into(),
        crucible_sim::EventKind::ResearchStarted { tech, .. } => {
            format!("research:{tech:?}").to_lowercase()
        }
        crucible_sim::EventKind::ResearchComplete { tech, .. } => {
            format!("researched:{tech:?}").to_lowercase()
        }
        crucible_sim::EventKind::Attacked { .. } => "attacked".into(),
    }
}
fn seed_now() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    now ^ COUNTER
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_mul(0x9E37_79B9)
}

/// Resolve a lobby opponent string to a concrete bot. Scripted bots (`easy`,
/// `medium`, `hard`) are always available; `champion` plays the reigning
/// champion and `museum:{genome_id}` plays any stored genome. Falls back to the
/// hard bot when the requested genome is missing (e.g. a fresh DB with no
/// crowned champion yet).
fn resolve_opponent(store: &Store, opponent: &str) -> Box<dyn Bot> {
    match opponent {
        "easy" => return Box::new(easy()),
        "medium" => return Box::new(medium()),
        "hard" => return Box::new(hard()),
        "adaptive" => return Box::new(adaptive(0.55)),
        _ => {}
    }

    // The single adaptive commander at an explicit difficulty: `adaptive:0.73`.
    if let Some(scalar) = opponent.strip_prefix("adaptive:") {
        if let Ok(d) = scalar.parse::<f32>() {
            return Box::new(adaptive(d));
        }
    }

    let genome_id = if opponent == "champion" {
        store
            .get_reigning_champion()
            .ok()
            .flatten()
            .map(|c| c.genome_id)
    } else if let Some(id) = opponent.strip_prefix("museum:") {
        id.parse::<i64>().ok()
    } else {
        None
    };

    if let Some(id) = genome_id {
        if let Ok(Some(weights)) = store.get_genome_weights(id) {
            // Guard against stale genomes persisted under an older network
            // shape (e.g. before the build head grew): a wrong-length genome
            // would panic the forward pass, so fall back to the hard bot.
            if weights.len() == crucible_ai::GENOME_LEN {
                return Box::new(GenomeBot::new(weights));
            }
        }
    }

    tracing::warn!("no genome for opponent {opponent:?}; falling back to hard bot");
    Box::new(hard())
}

/// Live matches have **no time limit** (`timeout_turns: 0`). Set
/// `CRUCIBLE_TIMEOUT_TURNS` to re-introduce a cap for smoke tests and
/// automated play.
fn timeout_override(mut config: GameConfig) -> GameConfig {
    if let Ok(v) = std::env::var("CRUCIBLE_TIMEOUT_TURNS") {
        if let Ok(turns) = v.parse::<i32>() {
            config.timeout_turns = turns;
            return config;
        }
    }
    config.timeout_turns = 0; // unlimited
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use crucible_sim::{EventKind, GameEvent};

    #[test]
    fn command_batches_have_strict_size_limits() {
        let oversized_group = Command::MoveGroup {
            player: Player::P0,
            units: vec![1; MAX_MOVE_GROUP_UNITS + 1],
            waypoint: (1, 1),
        };
        assert!(!command_batch_is_bounded(&[oversized_group]));

        let small_move = Command::MoveGroup {
            player: Player::P0,
            units: vec![1, 2],
            waypoint: (1, 1),
        };
        assert!(command_batch_is_bounded(&[small_move]));

        // The Attack command carries a unit group too and must be bounded
        // identically (a malformed batch must not smuggle ids past the cap).
        let oversized_attack = Command::Attack {
            player: Player::P0,
            units: vec![1; MAX_MOVE_GROUP_UNITS + 1],
            target: 9,
        };
        assert!(!command_batch_is_bounded(&[oversized_attack]));
        let small_attack = Command::Attack {
            player: Player::P0,
            units: vec![1, 2],
            target: 9,
        };
        assert!(command_batch_is_bounded(&[small_attack]));
    }

    #[test]
    fn state_diff_excludes_enemy_events() {
        let mut game = Game::new(Map::generate(4), GameConfig::default());
        game.events = vec![
            GameEvent {
                turn: 1,
                kind: EventKind::UnitTrained {
                    player: Player::P0,
                    utype: UnitType::Infantry,
                    tile: (1, 1),
                },
            },
            GameEvent {
                turn: 1,
                kind: EventKind::ResearchStarted {
                    player: Player::P1,
                    tech: crucible_sim::tech::TechId::HighExplosive,
                },
            },
        ];

        let mut last_event_turn = -1;
        let ServerMsg::StateDiff(diff) = build_diff(&game, &mut last_event_turn) else {
            panic!("expected a state diff");
        };
        assert_eq!(diff.events.len(), 1);
        assert_eq!(diff.events[0].player, Some(0));
        assert_eq!(diff.events[0].kind, "trained:infantry");
        assert_eq!(diff.turn, 1);
        assert_eq!(diff.active_player, 0);
    }

    #[test]
    fn resolve_opponent_scripted_and_fallback() {
        let store = Store::in_memory().unwrap();
        assert_eq!(resolve_opponent(&store, "easy").name(), "easy");
        assert_eq!(resolve_opponent(&store, "medium").name(), "medium");
        assert_eq!(resolve_opponent(&store, "hard").name(), "hard");
        // Unknown strings and a missing champion both fall back to hard.
        assert_eq!(resolve_opponent(&store, "champion").name(), "hard");
        assert_eq!(resolve_opponent(&store, "bogus").name(), "hard");
    }

    #[test]
    fn resolve_opponent_champion_and_museum() {
        let store = Store::in_memory().unwrap();
        // A real genome (correct length for the current network shape).
        let weights = crucible_ai::init(&mut crucible_sim::Rng::from_seed(7));
        let id = store.save_genome(3, None, "init", &weights).unwrap();
        store.crown_champion(id, 3, None, None).unwrap();

        assert_eq!(resolve_opponent(&store, "champion").name(), "genome");
        assert_eq!(
            resolve_opponent(&store, &format!("museum:{id}")).name(),
            "genome"
        );
        // A museum id with no stored genome falls back to hard.
        assert_eq!(resolve_opponent(&store, "museum:9999").name(), "hard");
        // A stale genome under an older network shape also falls back to hard
        // instead of panicking the forward pass.
        let stale = store
            .save_genome(4, None, "init", &[0.1_f32, -0.2, 0.3])
            .unwrap();
        assert_eq!(
            resolve_opponent(&store, &format!("museum:{stale}")).name(),
            "hard"
        );
    }

    #[test]
    fn champion_opponent_plays_a_match() {
        let store = Store::in_memory().unwrap();
        let genome = crucible_ai::init(&mut crucible_sim::Rng::from_seed(7));
        let id = store.save_genome(0, None, "init", &genome).unwrap();
        store.crown_champion(id, 0, None, None).unwrap();

        let mut champ = resolve_opponent(&store, "champion");
        assert_eq!(champ.name(), "genome");

        // The learned commander must drive a full match through the same
        // decision layer the live WS loop uses, without panicking.
        let cfg = crucible_sim::GameConfig {
            timeout_turns: 60,
            ..crucible_sim::GameConfig::default()
        };
        let outcome = crucible_ai::run_match(11, &cfg, &mut *champ, &mut hard());
        assert!(outcome.duration_turns > 0, "champion match failed to run");
    }
}
