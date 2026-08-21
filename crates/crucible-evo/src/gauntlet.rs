//! The champion gauntlet: a challenger genome is promoted only if it wins a
//! deterministic, logged series of headless matches against the reigning
//! champion **and** a sample of historical champions. Pure — given the same
//! genomes, seeds, and config, the result is identical.

use crucible_ai::{run_match_detailed, GenomeBot};
use crucible_sim::{GameConfig, Player};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct GauntletConfig {
    /// Map seeds used against the reigning champion (each played both sides).
    pub champion_seeds: u32,
    /// Win rate (fraction) required against the reigning champion.
    pub champion_win_rate: f32,
    /// Map seeds used against each historical champion (each played both sides).
    pub historical_seeds: u32,
    /// Aggregate win rate (fraction) required against historical champions.
    pub historical_win_rate: f32,
    /// How many historical champions to include (the caller pre-samples).
    pub historical_count: usize,
}

impl Default for GauntletConfig {
    fn default() -> Self {
        GauntletConfig {
            champion_seeds: 20,
            champion_win_rate: 0.55,
            historical_seeds: 3,
            historical_win_rate: 0.50,
            historical_count: 4,
        }
    }
}

/// The outcome of a gauntlet run.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GauntletResult {
    pub promoted: bool,
    pub champion_wins: u32,
    pub champion_total: u32,
    pub champion_win_rate: f32,
    pub historical_wins: u32,
    pub historical_total: u32,
    pub historical_win_rate: f32,
}

impl GauntletResult {
    /// Serialize for storage (kept compact; winner fields omitted).
    pub fn to_parts(&self) -> (u32, u32, u32, u32) {
        (
            self.champion_wins,
            self.champion_total,
            self.historical_wins,
            self.historical_total,
        )
    }
}

/// Pure gating decision. Promotion requires meeting the champion threshold AND
/// (when any historical champions were played) the historical threshold.
/// Comparisons use `>=` so a challenger exactly at the bar is promoted.
pub fn should_promote(
    champion_wins: u32,
    champion_total: u32,
    historical_wins: u32,
    historical_total: u32,
    gc: &GauntletConfig,
) -> bool {
    if champion_total == 0 {
        return false;
    }
    let champion_rate = champion_wins as f32 / champion_total as f32;
    if champion_rate < gc.champion_win_rate {
        return false;
    }
    if historical_total > 0 {
        let historical_rate = historical_wins as f32 / historical_total as f32;
        if historical_rate < gc.historical_win_rate {
            return false;
        }
    }
    true
}

/// Run a full gauntlet. `seeds` supplies map seeds (must have at least
/// `max(champion_seeds, historical_seeds)` entries). `historical` is the
/// pre-sampled historical champion genomes (usually up to `historical_count`).
pub fn run_gauntlet(
    challenger: &[f32],
    champion: &[f32],
    historical: &[Vec<f32>],
    seeds: &[u64],
    config: &GameConfig,
    gc: &GauntletConfig,
) -> GauntletResult {
    let champ_n = gc.champion_seeds as usize;
    assert!(
        champ_n <= seeds.len(),
        "not enough seeds for champion matches"
    );
    let (champion_wins, champion_total) =
        play_matches(challenger, champion, &seeds[..champ_n], config);

    let mut historical_wins = 0u32;
    let mut historical_total = 0u32;
    let hist_n = gc.historical_seeds as usize;
    for h in historical.iter().take(gc.historical_count) {
        let (w, t) = play_matches(challenger, h, &seeds[..hist_n.min(seeds.len())], config);
        historical_wins += w;
        historical_total += t;
    }

    let champion_win_rate = champion_wins as f32 / champion_total as f32;
    let historical_win_rate = if historical_total > 0 {
        historical_wins as f32 / historical_total as f32
    } else {
        1.0
    };

    GauntletResult {
        promoted: should_promote(
            champion_wins,
            champion_total,
            historical_wins,
            historical_total,
            gc,
        ),
        champion_wins,
        champion_total,
        champion_win_rate,
        historical_wins,
        historical_total,
        historical_win_rate,
    }
}

/// Play `a` vs `b` on every seed, both spawn sides (mirror fairness).
/// Returns (wins for `a`, total matches).
fn play_matches(a: &[f32], b: &[f32], seeds: &[u64], config: &GameConfig) -> (u32, u32) {
    let mut wins = 0u32;
    for &seed in seeds {
        // a = P0, b = P1
        let mut ga = GenomeBot::new(a.to_vec());
        let mut gb = GenomeBot::new(b.to_vec());
        let d0 = run_match_detailed(seed, config, &mut ga, &mut gb);
        if d0.outcome.winner == Some(Player::P0) {
            wins += 1;
        }

        // a = P1, b = P0
        let mut ga = GenomeBot::new(a.to_vec());
        let mut gb = GenomeBot::new(b.to_vec());
        let d1 = run_match_detailed(seed, config, &mut gb, &mut ga);
        if d1.outcome.winner == Some(Player::P1) {
            wins += 1;
        }
    }
    (wins, seeds.len() as u32 * 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_logic_is_exact_and_bounded() {
        let gc = GauntletConfig {
            champion_win_rate: 0.55,
            historical_win_rate: 0.50,
            ..GauntletConfig::default()
        };

        // Exactly at the champion bar (11/20 = 0.55), no historical → promote.
        assert!(should_promote(11, 20, 0, 0, &gc));
        // Just under the bar → reject.
        assert!(!should_promote(10, 20, 0, 0, &gc));
        // Champion passes but historical fails → reject.
        assert!(!should_promote(20, 20, 4, 10, &gc));
        // Both pass → promote.
        assert!(should_promote(20, 20, 5, 10, &gc));
        // No champion matches → never promote.
        assert!(!should_promote(0, 0, 0, 0, &gc));
    }
}
