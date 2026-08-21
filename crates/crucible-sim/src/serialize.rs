//! Snapshot and replay serialization.
//!
//! A snapshot is just the serialized [`Game`]; a replay is the *input log*
//! (map seed + ordered commands + result) so it is a few KB, not a state dump.
//! Formats are versioned from day one. v5 is the turn-based format: commands
//! are stamped `(turn, seq)` and execute immediately when applied; `EndTurn`
//! drives the turn lifecycle.

use serde::{Deserialize, Serialize};

use crate::entity::Player;
use crate::game::{Game, GameConfig, WinReason};
use crate::orders::Command;

pub const FORMAT_VERSION: u32 = 5;

/// A command stamped with the turn it was issued in and its sequence within
/// the whole match (issuance order). The pair reproduces application order
/// exactly.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct TimedCommand {
    pub turn: i32,
    pub seq: u32,
    pub player: Player,
    pub command: Command,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct ReplayResult {
    pub winner: Option<Player>,
    pub reason: Option<WinReason>,
    pub duration_turns: i32,
}

/// The replay (input log) format. Versioned so old replays stay re-runnable
/// *within a format generation*; v4 realtime logs are not loadable by the
/// turn-based engine (clean cutover).
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Replay {
    pub version: u32,
    pub map_seed: u64,
    pub config: GameConfig,
    pub commands: Vec<TimedCommand>,
    pub result: Option<ReplayResult>,
}

impl Replay {
    pub fn new(map_seed: u64, config: GameConfig) -> Self {
        Replay {
            version: FORMAT_VERSION,
            map_seed,
            config,
            commands: Vec::new(),
            result: None,
        }
    }

    /// Record one command at the game's current turn, sequenced after every
    /// command recorded so far.
    pub fn record(&mut self, turn: i32, player: Player, command: Command) {
        let seq = self.commands.len() as u32;
        self.commands.push(TimedCommand {
            turn,
            seq,
            player,
            command,
        });
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("replay serialization is infallible")
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

/// Serialize a full game state to a canonical JSON string.
///
/// Field order is stable (struct definition order), so this is byte-identical
/// for identical states on every target. Used by golden determinism tests.
pub fn snapshot_json(game: &Game) -> String {
    serde_json::to_string(game).expect("game serialization is infallible")
}

/// Serialize a full game state to canonical JSON bytes (for hashing).
pub fn snapshot_bytes(game: &Game) -> Vec<u8> {
    serde_json::to_vec(game).expect("game serialization is infallible")
}

/// Rebuild a fresh game from a replay's seed and re-apply its command log to
/// reproduce the match exactly. Commands execute immediately in log order;
/// `EndTurn` entries drive the turn lifecycle.
pub fn replay_to_game(replay: &Replay) -> Game {
    let mut game = Game::new(
        crate::map::Map::generate(replay.map_seed),
        replay.config.clone(),
    );
    for tc in &replay.commands {
        if game.is_over() {
            break;
        }
        game.apply_commands(tc.player, std::slice::from_ref(&tc.command));
    }
    game
}
