//! WebSocket live-match protocol. Server-authoritative: the sim runs here,
//! human commands are validated identically to the bot's, and the client only
//! receives the human player's fogged view.
//!
//! The match is strictly alternating-turn. P0 is the human: their command
//! batches apply immediately as they arrive, and the game only advances when
//! an `EndTurn` flips the turn to P1. While `game.active == P1` the server
//! asks the bot for its turn's commands, applies them, and broadcasts; the
//! bot's own `EndTurn` hands the turn back. There is no wall-clock tick.

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
use tokio::sync::mpsc;

use crucible_ai::{easy, hard, medium, Bot, GenomeBot};
use crucible_sim::{
    entity::BuildingType, Command, Game, GameConfig, Map, Player, Replay, ReplayResult, UnitType,
};

use crate::store::Store;

const MAX_WS_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_COMMANDS_PER_MESSAGE: usize = 8;
const MAX_MOVE_GROUP_UNITS: usize = 32;
const MAX_MOVE_GROUP_UNITS_PER_BATCH: usize = 64;
const COMMAND_CHANNEL_CAPACITY: usize = 4;

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ClientMsg {
    JoinMatch {
        opponent: String,
    },
    Commands {
        cmds: Vec<Command>,
    },
    /// End the active player's turn (only valid from `game.active`; the sim
    /// runs the full lifecycle: turret fire → income → production → opponent).
    EndTurn,
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ServerMsg {
    MatchStart(MatchStartMsg),
    StateDiff(StateDiffMsg),
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
    hq: [(u8, u8); 2],
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct StateDiffMsg {
    turn: i32,
    /// Index of the player whose turn it is (0 = P0/human, 1 = P1/bot).
    active_player: u8,
    ore: i32,
    power_produced: i32,
    power_consumed: i32,
    /// The player's currently researched upgrade ("None" before any research).
    upgrade: String,
    entities: Vec<DiffEntity>,
    ore_tiles: Vec<OreTile>,
    visible: Vec<u16>,
    events: Vec<DiffEvent>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct MatchEndMsg {
    winner: Option<u8>,
    reason: Option<crucible_sim::WinReason>,
    duration_turns: i32,
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
    /// Own-building production queue (unit kind names, oldest first).
    #[serde(skip_serializing_if = "Option::is_none")]
    queue: Option<Vec<String>>,
    /// Progress of the current queue head, in turns.
    #[serde(skip_serializing_if = "Option::is_none")]
    progress: Option<i32>,
    /// Build time of the current queue head, in turns.
    #[serde(skip_serializing_if = "Option::is_none")]
    build_time: Option<i32>,
}

#[derive(Serialize, Clone, Debug)]
struct OreTile {
    x: u8,
    y: u8,
    amount: i32,
}

#[derive(Serialize, Clone, Debug)]
struct DiffEvent {
    turn: i32,
    kind: String,
    /// Amount for `ore_mined` / `sold` events (null otherwise) — lets the
    /// client show per-turn income, since refineries bank passively.
    #[serde(skip_serializing_if = "Option::is_none")]
    amount: Option<i32>,
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
    let opponent = match tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match receiver.next().await {
                Some(Ok(Message::Text(t))) => {
                    if let Ok(ClientMsg::JoinMatch { opponent }) = serde_json::from_str(&t) {
                        return Some(opponent);
                    }
                }
                Some(Ok(Message::Close(_))) | None => return None,
                _ => continue,
            }
        }
    })
    .await
    {
        Ok(Some(opponent)) => opponent,
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

    // Resolve the opponent off the async runtime: it reads the store (SQLite
    // behind a mutex), which can stall live match handling if contended.
    let store = state.store.clone();
    let opponent_name = opponent.clone();
    let mut bot: Box<dyn Bot> =
        match tokio::task::spawn_blocking(move || resolve_opponent(&store, &opponent_name)).await {
            Ok(bot) => bot,
            Err(e) => {
                tracing::error!("opponent resolution failed: {e}");
                Box::new(hard())
            }
        };

    // Seed from the wall clock (server is the one place this is allowed); the
    // seed is recorded in the replay so the match stays reproducible.
    let seed = seed_now();
    let config = timeout_override(GameConfig::default());
    let mut game = Game::new(Map::generate(seed), config.clone());
    let mut replay = Replay::new(seed, config);

    let passable = game.map.passable.clone();
    let hq = [game.map.hq_tiles[0], game.map.hq_tiles[1]];

    sender
        .send(Message::Text(
            serde_json::to_string(&ServerMsg::MatchStart(MatchStartMsg {
                map_seed: seed,
                player: Player::P0.index() as u8,
                passable,
                hq,
            }))?
            .into(),
        ))
        .await?;

    // Incoming commands are buffered on a channel by a reader task.
    let (tx, mut rx) = mpsc::channel::<Vec<Command>>(COMMAND_CHANNEL_CAPACITY);
    tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Text(t) = msg {
                match serde_json::from_str::<ClientMsg>(&t) {
                    Ok(ClientMsg::Commands { cmds }) => {
                        if !command_batch_is_bounded(&cmds) {
                            tracing::warn!("dropping oversized command batch");
                            continue;
                        }
                        if tx.try_send(cmds).is_err() {
                            tracing::warn!("dropping command batch: pending input limit reached");
                        }
                    }
                    Ok(ClientMsg::EndTurn) => {
                        // EndTurn is free (costs no action budget) and carries
                        // no payload; forward it as its own batch.
                        if tx
                            .try_send(vec![Command::EndTurn { player: Player::P0 }])
                            .is_err()
                        {
                            tracing::warn!("dropping EndTurn: pending input limit reached");
                        }
                    }
                    Ok(ClientMsg::JoinMatch { .. }) => {}
                    // Never drop a malformed command silently: a wire-format
                    // drift (e.g. player as index vs "P0") otherwise looks
                    // like the game ignoring the player.
                    Err(e) => tracing::warn!("dropping unparseable client message: {e}: {t}"),
                }
            }
        }
    });

    let mut last_event_turn = -1i32;

    // Run the match. If the client disconnects (or a send fails) mid-match,
    // the error surfaces here with the replay state intact, so the match is
    // still persisted as a partial replay instead of being lost entirely.
    let result = match (async {
        loop {
            // P0's turn: wait for their next batch. Nothing happens between
            // batches — the sim has no wall clock.
            let human_cmds = rx.recv().await.ok_or("client closed the connection")?;
            if human_cmds.is_empty() {
                continue;
            }
            for c in &human_cmds {
                replay.record(game.turn, Player::P0, c.clone());
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
                });
            }

            let diff = build_diff(&game, &mut last_event_turn);
            sender
                .send(Message::Text(serde_json::to_string(&diff)?.into()))
                .await?;

            // While it is the bot's turn, drive it: one `decide` per own turn
            // (its commands end with `EndTurn`, which hands the turn back).
            // Defensive guard: a bot that omits `EndTurn` must not hang the
            // connection forever, so force the lifecycle after 100 decides.
            let mut bot_stall_guard = 0;
            while game.active == Player::P1 && !game.is_over() {
                bot_stall_guard += 1;
                if bot_stall_guard > 100 {
                    tracing::warn!("bot stalled without EndTurn; forcing end of turn");
                    game.end_turn();
                } else {
                    let bot_cmds = bot.decide(&game, Player::P1);
                    for c in &bot_cmds {
                        replay.record(game.turn, Player::P1, c.clone());
                    }
                    game.apply_commands(Player::P1, &bot_cmds);
                }
                if game.is_over() {
                    return Ok(ReplayResult {
                        winner: game.winner,
                        reason: game.win_reason,
                        duration_turns: game.turn,
                    });
                }
                let diff = build_diff(&game, &mut last_event_turn);
                sender
                    .send(Message::Text(serde_json::to_string(&diff)?.into()))
                    .await?;
            }
        }
    })
    .await
    {
        Ok(result) => result,
        Err(e) => {
            // Client disconnected (or a send failed) mid-match: persist a
            // partial replay so the match isn't lost, then propagate.
            save_replay(&state, seed, &opponent, &game, &replay, None).await;
            return Err(e);
        }
    };

    // Match ended normally: record the result, persist the replay, and tell
    // the client. A failing final send (client already gone) is not fatal.
    replay.result = Some(result.clone());
    let replay_id = save_replay(&state, seed, &opponent, &game, &replay, Some(&result)).await;
    let _ = sender
        .send(Message::Text(
            serde_json::to_string(&ServerMsg::MatchEnd(MatchEndMsg {
                winner: game.winner.map(|p| p.index() as u8),
                reason: game.win_reason,
                duration_turns: game.turn,
                replay_id,
            }))?
            .into(),
        ))
        .await;
    Ok(())
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
                queue: None,
                progress: None,
                build_time: None,
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
                queue,
                progress,
                build_time,
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
            queue: None,
            progress: None,
            build_time: None,
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
            queue: None,
            progress: None,
            build_time: None,
        });
    }

    let mut ore_tiles = Vec::new();
    for idx in 0..(64 * 64) {
        if view.known_ore[idx] && game.map.ore[idx] > 0 {
            ore_tiles.push(OreTile {
                x: (idx % 64) as u8,
                y: (idx / 64) as u8,
                amount: game.map.ore[idx],
            });
        }
    }

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
            kind: event_kind(&e.kind),
            amount: match &e.kind {
                crucible_sim::EventKind::OreMined { amount, .. } => Some(*amount),
                crucible_sim::EventKind::Sold { refund, .. } => Some(*refund),
                _ => None,
            },
            player: event_player(game, &e.kind).map(|player| player.index() as u8),
        })
        .collect();
    *last_event_turn = game.turn;
    let (power_produced, power_consumed) = game.power(crucible_sim::Player::P0);

    ServerMsg::StateDiff(StateDiffMsg {
        turn: game.turn,
        active_player: game.active.index() as u8,
        ore: game.ore[0],
        power_produced,
        power_consumed,
        upgrade: format!("{:?}", game.upgrades[0]),
        entities,
        ore_tiles,
        visible,
        events,
    })
}

/// The player an event belongs to. Attacks are attributed to the *defender's*
/// owner (resolved against the live game state) so the client sees, and can
/// log, enemy strikes on its own units.
fn event_player(game: &Game, event: &crucible_sim::EventKind) -> Option<Player> {
    match event {
        crucible_sim::EventKind::BuildingPlaced { player, .. }
        | crucible_sim::EventKind::UnitTrained { player, .. }
        | crucible_sim::EventKind::OreMined { player, .. }
        | crucible_sim::EventKind::Sold { player, .. }
        | crucible_sim::EventKind::UpgradeChosen { player, .. } => Some(*player),
        crucible_sim::EventKind::UnitDied { owner, .. }
        | crucible_sim::EventKind::BuildingDestroyed { owner, .. } => Some(*owner),
        crucible_sim::EventKind::Attacked { target, .. } => game
            .any_unit(*target)
            .map(|u| u.owner)
            .or_else(|| game.any_building(*target).map(|b| b.owner)),
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
            Command::MoveGroup { units, .. } | Command::Attack { units, .. } => units,
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
        crucible_sim::EventKind::OreMined { .. } => "ore_mined".into(),
        crucible_sim::EventKind::BuildingPlaced { btype, .. } => {
            format!("built:{btype:?}").to_lowercase()
        }
        crucible_sim::EventKind::Sold { .. } => "sold".into(),
        crucible_sim::EventKind::UpgradeChosen { upgrade, .. } => {
            format!("upgrade:{upgrade:?}").to_lowercase()
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
        _ => {}
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
                kind: EventKind::UpgradeChosen {
                    player: Player::P1,
                    upgrade: crucible_sim::Upgrade::Damage,
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
