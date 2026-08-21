//! # crucible-evo
//!
//! Pure training logic: the (μ+λ) evolution strategy, lineage records, ghost
//! replay opponents, the champion gauntlet, Elo, and change reports. No IO —
//! the server injects storage and scheduling.
//!
//! M4 shipped the population + fitness; M5 adds the gauntlet, lineage, Elo,
//! and behavioral change reports. Ghosts land in M7.

pub mod balance;
pub mod curriculum;
pub mod fitness;
pub mod gauntlet;
pub mod ghost;
pub mod league;
pub mod lineage;
pub mod population;
pub mod report;

pub use balance::{
    balance_table, bot_tier, bot_tier_lengths, bot_tiers, composition_cost, counter_matrix, median,
    micro_matchup, micro_matchup_rate, Composition, WinRate,
};
pub use curriculum::{Curriculum, CurriculumConfig, Stage};
pub use fitness::{
    army_value, evaluate_economy, evaluate_production, evaluate_vs, head_to_head, outcome_for,
    self_play_fitness, shaped_fitness, spent_value, Noop,
};
pub use gauntlet::{run_gauntlet, should_promote, GauntletConfig, GauntletResult};
pub use ghost::{ghost_fitness, Ghost, GhostEntry, GhostPool};
pub use league::{update, EloHistory, Outcome, K};
pub use lineage::{BornFrom, Lineage, LineageRecord};
pub use population::{EsParams, Population};
pub use report::{change_report, diff, era_name, fingerprint, ChangeReport, Fingerprint};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
