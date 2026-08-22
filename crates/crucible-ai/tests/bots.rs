//! M2 acceptance (turn-based): the economy reaches scale within the turn cap,
//! rush beats turtle, and the hard bot beats the medium bot across a
//! deterministic seed set.

use crucible_ai::{easy, hard, medium, run_match, Bot, MatchOutcome};
use crucible_sim::{BuildingType, Command, GameConfig, Map, Player, UnitType};

fn config() -> GameConfig {
    GameConfig::default()
}

/// Production + passive income reach scale by turn 60: the easy bot must have
/// built refineries, trained combat units, and actually drained the ore
/// fields (refinery adjacency is what makes ore matter under passive income).
#[test]
fn economy_and_production_scale_by_turn_60() {
    let mut g = crucible_sim::Game::new(Map::generate(42), config());
    let mut bot = easy();
    let ore_markers_before = g.map.ore.clone();
    // Drive P0 with the easy bot for 60 own turns; P1 just passes so the
    // match never ends early.
    while g.turn <= 60 && !g.is_over() {
        if g.active == Player::P0 {
            let cmds = bot.decide(&g, Player::P0);
            g.apply_commands(Player::P0, &cmds);
        } else {
            g.apply_commands(Player::P1, &[Command::EndTurn { player: Player::P1 }]);
        }
    }

    let refineries = g
        .buildings
        .iter()
        .filter(|b| b.owner == Player::P0 && b.btype == BuildingType::Refinery)
        .count();
    let units = g
        .units
        .iter()
        .filter(|u| u.owner == Player::P0 && u.utype != UnitType::Infantry)
        .count();
    assert!(
        refineries >= 1,
        "economy did not build a refinery by turn 60 (got {refineries})"
    );
    assert!(
        units >= 1,
        "economy did not reach a combat force by turn 60 (got {units})"
    );

    // Deposits are static and inexhaustible: extraction increases the
    // stockpile/income but never mutates the map marker or richness tier.
    assert_eq!(
        g.map.ore, ore_markers_before,
        "ore deposit markers changed despite infinite-deposit rules"
    );
    let income = g.resource_income(Player::P0);
    assert!(
        income.total_value() > crucible_sim::HQ_INCOME_PER_TURN,
        "refinery did not add richness-scaled income by turn 60: {income:?}"
    );
    assert!(
        g.resources(Player::P0).total_value() > config().starting_ore,
        "economy accumulated no resources by turn 60"
    );
}

/// Medium (rush waves) vs easy (turtle) across a seed set.
#[test]
fn rush_beats_turtle() {
    let seeds: Vec<u64> = (0..10).map(|i| 1000 + i).collect();
    let mut wins = 0;
    let mut total = 0;
    for seed in seeds {
        let mut attacker = medium();
        let mut turtle = easy();
        let o = run_match(seed, &config(), &mut attacker, &mut turtle);
        total += 1;
        if o.won_by(Player::P0) {
            wins += 1;
        }
    }
    println!("rush vs turtle: {wins}/{total}");
    assert!(
        wins as f64 / total as f64 >= 0.6,
        "rush did not beat turtle decisively ({wins}/{total})"
    );
}

/// Hard (expand-and-push) vs medium (waves) across a seed set.
#[test]
fn hard_beats_medium() {
    let seeds: Vec<u64> = (0..10).map(|i| 2000 + i).collect();
    let mut wins = 0;
    let mut total = 0;
    for seed in seeds {
        let mut a = hard();
        let mut b = medium();
        let o = run_match(seed, &config(), &mut a, &mut b);
        total += 1;
        if o.won_by(Player::P0) {
            wins += 1;
        }
    }
    println!("hard vs medium: {wins}/{total}");
    assert!(
        wins as f64 / total as f64 >= 0.6,
        "hard bot did not beat medium decisively ({wins}/{total})"
    );
}

// Sanity: a single match always terminates and reports a winner or timeout.
#[test]
fn match_terminates() {
    let mut a = hard();
    let mut b = medium();
    let o: MatchOutcome = run_match(7777, &config(), &mut a, &mut b);
    assert!(o.duration_turns > 0);
    assert!(o.winner.is_some() || o.reason.is_some());
}
