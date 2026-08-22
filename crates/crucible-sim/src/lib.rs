//! # crucible-sim
//!
//! The pure, deterministic simulation core for CRUCIBLE. No IO, no threads,
//! no OS calls, no wall clock. Compiles identically for native and wasm.
//!
//! The game is **turn-based** (alternating turns on a 64×64 tile grid).
//! The determinism contract:
//! - All randomness flows through the injected seeded [`Rng`](rng::Rng) —
//!   used by map generation only; in-game resolution is fully deterministic.
//! - Only the `active` player may act; commands execute immediately.
//!   [`Game::end_turn`] runs the fixed lifecycle: turret fire → sweep → fog →
//!   win check → opponent start-of-turn (income → production → resets → fog).
//! - Entities are iterated in ascending id order everywhere.
//! - Integer math only; no platform-variable float functions in game-state
//!   math.
//! - [`Game`] is fully serializable via serde at any turn.

pub mod entity;
pub mod fog;
pub mod game;
/// The shared determinism-golden scenario (test support).
#[doc(hidden)]
pub mod golden;
pub mod map;
pub mod orders;
pub mod rng;
pub mod serialize;
pub mod tech;
pub mod tiles;

pub use entity::{
    building_produces, building_stats, unit_stats, Building, BuildingType, EntityId, Player, Unit,
    UnitType, FOG_MEMORY_TURNS, HQ_INCOME_PER_TURN, PLACE_RADIUS_TILES, REFINERY_ORE_PER_TURN,
};
pub use game::{EventKind, Game, GameConfig, GameEvent, WinReason};
pub use map::{open_test_map, Map};
pub use orders::{Command, CommandError};
pub use rng::Rng;
pub use serialize::{Replay, ReplayResult, TimedCommand, FORMAT_VERSION};

/// Match timeout in turns (default for live matches; training overrides).
pub const MATCH_TIMEOUT_TURNS: i32 = 80;

/// Library version string.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
