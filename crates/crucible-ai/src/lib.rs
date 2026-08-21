//! # crucible-ai
//!
//! The AI commander: fog-legal feature extraction, the hand-rolled MLP, the
//! deterministic decision layer, and scripted baseline bots. Pure — depends
//! only on `crucible-sim`.
//!
//! M2 shipped the scripted bots + headless runner; M4 adds the learned
//! commander (`features`, `network`, `decision`).

pub mod bot;
pub mod commander;
pub mod decision;
pub mod features;
pub mod headless;
pub mod network;
pub mod scripted;

pub use bot::Bot;
pub use commander::GenomeBot;
pub use decision::decide;
pub use features::{extract, extract_single, FeatureInput, FEATURE_DIM};
pub use headless::{
    run_match, run_match_detailed, run_match_with_replay, series, DetailedOutcome, MatchOutcome,
    SeriesReport,
};
pub use network::{forward, init, mutate, GENOME_LEN, OUTPUT};
pub use scripted::{easy, hard, medium};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
