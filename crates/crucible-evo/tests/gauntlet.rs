//! M5 acceptance: promotion happens **only** when the gauntlet thresholds are
//! met. We rig it with a genome trained offline on the economy fitness (which
//! reliably beats a no-op champion) and assert both the promote and reject
//! paths of the full, real-match gauntlet.

use crucible_ai::{run_match_detailed, GenomeBot, GENOME_LEN};
use crucible_evo::{evaluate_economy, run_gauntlet, EsParams, GauntletConfig, Population};
use crucible_sim::{GameConfig, Player, Rng};

/// Train a genome on the economy fitness long enough that it reliably mines
/// ore (and therefore beats a no-op champion on timeout value).
fn train_economy_genome(seed: u64) -> Vec<f32> {
    let params = EsParams {
        population_size: 12,
        mu: 3,
        sigma: 0.08,
        ..EsParams::default()
    };
    let train_seeds = [seed];
    let cfg = GameConfig {
        timeout_turns: 10_000,
        ..GameConfig::default()
    };
    let ticks = 900;

    let mut rng = Rng::from_seed(seed);
    let mut pop = Population::init(&mut rng, params);
    let mut fitnesses: Vec<f32> = pop
        .genomes
        .iter()
        .map(|g| evaluate_economy(g, &train_seeds, &cfg, ticks))
        .collect();
    for _ in 0..3 {
        pop = pop.step(&mut rng, &fitnesses);
        fitnesses = pop
            .genomes
            .iter()
            .map(|g| evaluate_economy(g, &train_seeds, &cfg, ticks))
            .collect();
    }
    let best = pop.best_index(&fitnesses);
    pop.genomes[best].clone()
}

#[test]
fn gauntlet_promotes_only_when_thresholds_met() {
    let challenger = train_economy_genome(77);
    let champion = vec![0.0f32; GENOME_LEN]; // all-zero genome = no-op

    let cfg = GameConfig {
        timeout_turns: 120,
        ..GameConfig::default()
    };

    // Sanity: the challenger beats the no-op champion, deterministically.
    {
        let mut a = GenomeBot::new(challenger.clone());
        let mut b = GenomeBot::new(champion.clone());
        let d = run_match_detailed(11, &cfg, &mut a, &mut b);
        assert_eq!(
            d.outcome.winner,
            Some(Player::P0),
            "offline-trained challenger must beat the no-op champion"
        );
    }

    let gc = GauntletConfig {
        champion_seeds: 2,
        champion_win_rate: 0.55,
        historical_seeds: 1,
        historical_win_rate: 0.50,
        historical_count: 0,
    };
    let seeds = [11u64, 22u64];

    // Challenger beats champion on every seed → promotion.
    let promoted = run_gauntlet(&challenger, &champion, &[], &seeds, &cfg, &gc);
    assert!(promoted.promoted, "expected promotion: {promoted:?}");
    assert_eq!(promoted.champion_total, 4); // 2 seeds × both sides
    assert_eq!(promoted.champion_wins, 4);

    // Reversed roles: the no-op challenger cannot win a single match.
    let rejected = run_gauntlet(&champion, &challenger, &[], &seeds, &cfg, &gc);
    assert!(!rejected.promoted, "expected rejection: {rejected:?}");
    assert_eq!(rejected.champion_wins, 0);
}

#[test]
fn gauntlet_is_deterministic() {
    let challenger = train_economy_genome(99);
    let champion = vec![0.0f32; GENOME_LEN];
    let cfg = GameConfig {
        timeout_turns: 40,
        ..GameConfig::default()
    };
    let gc = GauntletConfig {
        champion_seeds: 1,
        champion_win_rate: 0.55,
        historical_seeds: 1,
        historical_win_rate: 0.50,
        historical_count: 0,
    };
    let seeds = [5u64];

    let a = run_gauntlet(&challenger, &champion, &[], &seeds, &cfg, &gc);
    let b = run_gauntlet(&challenger, &champion, &[], &seeds, &cfg, &gc);
    assert_eq!(a, b);
}
