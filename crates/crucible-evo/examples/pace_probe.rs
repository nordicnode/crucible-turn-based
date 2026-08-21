//! Pacing probe: print per-seed match durations (seconds) and win reasons for
//! the bot tiers, to tune match length into the 5–10 minute band.
//!   cargo run -q -p crucible-evo --example pace_probe

use crucible_ai::{run_match, Bot};
use crucible_sim::{GameConfig, WinReason};

type BotFactory = fn() -> Box<dyn Bot>;

fn main() {
    let cfg = GameConfig::default(); // 15-minute timeout
    let seeds: Vec<u64> = (0..32).collect();

    let matchups: Vec<(&str, BotFactory, BotFactory)> = vec![
        (
            "medium>easy",
            || Box::new(crucible_ai::medium()),
            || Box::new(crucible_ai::easy()),
        ),
        (
            "hard>medium",
            || Box::new(crucible_ai::hard()),
            || Box::new(crucible_ai::medium()),
        ),
    ];

    for (name, mk_a, mk_b) in matchups {
        println!("=== {name} ===");
        let mut secs = Vec::new();
        for &seed in &seeds {
            let mut a = mk_a();
            let mut b = mk_b();
            let o = run_match(seed, &cfg, &mut *a, &mut *b);
            let s = o.duration_turns;
            secs.push(s);
            let reason = match o.reason {
                Some(WinReason::HqDestroyed) => "HQ",
                Some(WinReason::Timeout) => "timeout",
                None => "?",
            };
            println!(
                "  seed {seed:>2}: {s:>4}s {reason:<8} winner {:?}",
                o.winner
            );
        }
        secs.sort_unstable();
        println!("  p50 = {}s", secs[secs.len() / 2]);
        let timeouts = secs.iter().filter(|&&s| s >= 900).count();
        println!("  timeouts = {timeouts}/{}", secs.len());
    }
}
