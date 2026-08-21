//! Balance-tuning probe: prints counter-matrix win rates and bot-tier
//! match-length medians for a seed set. Run with:
//!   cargo run -p crucible-evo --example counter_probe

use crucible_evo::{bot_tier_lengths, counter_matrix, median, micro_matchup_rate, WinRate};
use crucible_sim::{GameConfig, UnitType};

fn main() {
    let cfg = GameConfig {
        timeout_turns: 300,
        ..GameConfig::default()
    };
    let seeds: Vec<u64> = (0..16).collect();

    println!("=== counter matrix (16 seeds) ===");
    for (name, r) in counter_matrix(&seeds, &cfg) {
        print_rate(name, r);
    }

    println!("\n=== cross-check: artillery vs infantry (equal cost) ===");
    print_rate(
        "artillery>infantry",
        micro_matchup_rate(
            &seeds,
            &[(UnitType::Artillery, 3)],
            &[(UnitType::Infantry, 12)],
            &cfg,
        ),
    );

    println!("\n=== bot-tier match length p50 (default 15-min timeout) ===");
    let full = GameConfig::default();
    let medium_easy = bot_tier_lengths(
        &seeds,
        &full,
        || Box::new(crucible_ai::medium()),
        || Box::new(crucible_ai::easy()),
    );
    let hard_medium = bot_tier_lengths(
        &seeds,
        &full,
        || Box::new(crucible_ai::hard()),
        || Box::new(crucible_ai::medium()),
    );
    println!("medium>easy p50: {} turns", median(medium_easy));
    println!("hard>medium p50: {} turns", median(hard_medium));
}

fn print_rate(name: &str, r: WinRate) {
    println!(
        "{name}: a {}/{} ({:.0}%)  b {}/{} ({:.0}%)  draws {}",
        r.a_wins,
        r.matches,
        r.a_rate() * 100.0,
        r.b_wins,
        r.matches,
        r.b_rate() * 100.0,
        r.draws
    );
}
