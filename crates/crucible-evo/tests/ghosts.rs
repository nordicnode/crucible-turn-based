//! M7 acceptance: a recorded "cheese" strategy (which beats the champion) is
//! turned into a ghost, and a focused self-play run learns to beat it within a
//! bounded generation budget.
//!
//! The cheese is the medium scripted bot: a refinery-funded pressure build
//! (early infantry waves, then artillery) that destroys a no-op champion's HQ
//! and wrecks untrained (random) policies outright.
//!
//! Post-port dynamics: cross-map pushes are far slower than the RTS's 3-minute
//! budget, so a pure-economy turtle that repairs its HQ every turn (30% max HP
//! per repair, ~450 HP/turn on the HQ) outlasts any scripted rush at the
//! timeout — the bootstrapped economy lineage counters the cheese by value
//! before any focused training. The honest M7 claim is therefore the pipeline
//! one: a recorded cheese that destroys untrained policies becomes a ghost,
//! and focused ghost fitness turns a random population into a population that
//! beats it ≥ 75%. Matches use the same marathon budget as the balance suite
//! (300 turns).

use crucible_ai::{init, run_match_detailed, run_match_with_replay, GenomeBot, GENOME_LEN};
use crucible_evo::{ghost_fitness, EsParams, Ghost, Population};
use crucible_sim::{GameConfig, Player, Rng};

const GHOST_SEEDS: [u64; 4] = [10, 11, 12, 13];

fn test_config() -> GameConfig {
    GameConfig {
        timeout_turns: 300, // marathon: enough for a recorded push to resolve vs a real economy
        ..GameConfig::default()
    }
}

/// Record the cheese beating a no-op champion (destroys its HQ).
fn record_cheese(seed: u64, config: &GameConfig) -> crucible_sim::Replay {
    let mut human = crucible_ai::medium();
    let mut champion = GenomeBot::new(vec![0.0f32; GENOME_LEN]); // no-op
    let (_outcome, replay) = run_match_with_replay(seed, config, &mut human, &mut champion);
    assert_eq!(
        replay.result.as_ref().and_then(|r| r.winner),
        Some(Player::P0),
        "cheese must beat the champion on seed {seed}"
    );
    replay
}

fn ghosts(config: &GameConfig) -> Vec<Ghost> {
    GHOST_SEEDS
        .iter()
        .map(|&seed| Ghost::from_replay(&record_cheese(seed, config), Player::P0))
        .collect()
}

/// Win rate of `genome` (playing P1) against the ghosts.
fn win_rate_vs_ghosts(genome: &[f32], ghosts: &[Ghost], config: &GameConfig) -> f32 {
    let mut wins = 0u32;
    for ghost in ghosts {
        let mut g = ghost.clone();
        let mut genome_bot = GenomeBot::new(genome.to_vec());
        let d = run_match_detailed(ghost.map_seed(), config, &mut g, &mut genome_bot);
        if d.outcome.winner == Some(Player::P1) {
            wins += 1;
        }
    }
    wins as f32 / ghosts.len() as f32
}

#[test]
fn training_learns_to_beat_the_cheese_ghost() {
    let config = test_config();
    let ghosts = ghosts(&config);

    // The cheese must genuinely threaten untrained policies: a random genome
    // loses the recorded push (HQ destroyed long before the timeout).
    let random = init(&mut Rng::from_seed(3));
    let before = win_rate_vs_ghosts(&random, &ghosts, &config);
    assert!(
        before < 0.5,
        "the cheese should beat untrained (random) policies before focused training (got {before})"
    );

    // Focused training vs the cheese ghost, from a fresh random population.
    let generations = 8;
    let mut rng = Rng::from_seed(2024);
    let mut pop = Population::init(
        &mut rng,
        EsParams {
            population_size: 24,
            mu: 6,
            sigma: 0.08,
            ..EsParams::default()
        },
    );
    let mut fitnesses: Vec<f32> = pop
        .genomes
        .iter()
        .map(|g| ghost_fitness(g, &ghosts, &config))
        .collect();
    for _ in 0..generations {
        pop = pop.step(&mut rng, &fitnesses);
        fitnesses = pop
            .genomes
            .iter()
            .map(|g| ghost_fitness(g, &ghosts, &config))
            .collect();
    }

    let after = win_rate_vs_ghosts(&pop.genomes[pop.best_index(&fitnesses)], &ghosts, &config);
    assert!(
        after > before,
        "win rate vs the cheese ghost must improve: {before} -> {after}"
    );
    assert!(
        after >= 0.75,
        "best genome must beat the cheese ghost >= 75% after {generations} focused generations (got {after})"
    );
}

#[test]
fn ghost_win_rate_is_well_defined() {
    let config = test_config();
    let ghosts = ghosts(&config);
    let genome = init(&mut Rng::from_seed(3));
    let rate = win_rate_vs_ghosts(&genome, &ghosts, &config);
    assert!((0.0..=1.0).contains(&rate));
}
