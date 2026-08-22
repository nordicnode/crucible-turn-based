//! Snapshot and replay serialization.
//!
//! A snapshot is just the serialized [`Game`]; a replay is the *input log*
//! (map seed + ordered commands + result) so it is a few KB, not a state dump.
//! Formats are versioned from day one. v6 is the round-aware turn-based
//! format: commands are stamped `(round, turn, seq)` and execute immediately
//! when applied; `EndTurn` drives one activation lifecycle. The activation
//! `turn` remains in every record so v5 logs can be replayed deterministically.

use serde::{Deserialize, Serialize};

use crate::entity::Player;
use crate::game::{Game, GameConfig, WinReason};
use crate::orders::Command;

pub const FORMAT_VERSION: u32 = 6;

/// A command stamped with the turn it was issued in and its sequence within
/// the whole match (issuance order). The pair reproduces application order
/// exactly.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct TimedCommand {
    /// Activation number, retained for ordering and v5 compatibility.
    pub turn: i32,
    /// Player-facing P0/P1 round containing this activation.
    #[serde(default)]
    pub round: i32,
    pub seq: u32,
    pub player: Player,
    pub command: Command,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct ReplayResult {
    pub winner: Option<Player>,
    pub reason: Option<WinReason>,
    /// Legacy activation duration.
    pub duration_turns: i32,
    /// Player-facing round duration. Defaults to zero for v5 replays and is
    /// filled by replay consumers from the final game's round when needed.
    #[serde(default)]
    pub duration_rounds: i32,
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

    /// Record one command using the legacy activation-only API. The round is
    /// derived from the initial game's turn convention; live callers should
    /// use [`Replay::record_at`] so resumed/hand-built states retain it.
    pub fn record(&mut self, turn: i32, player: Player, command: Command) {
        self.record_at((turn + 1).div_euclid(2).max(1), turn, player, command);
    }

    /// Record one command with its exact player-facing round and activation.
    pub fn record_at(&mut self, round: i32, turn: i32, player: Player, command: Command) {
        let seq = self.commands.len() as u32;
        self.commands.push(TimedCommand {
            turn,
            round,
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
/// `EndTurn` entries drive the activation lifecycle. v5 records without a
/// round field remain valid because the sim derives the same round internally.
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
