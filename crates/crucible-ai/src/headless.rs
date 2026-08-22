//! Headless match execution: run two [`Bot`]s to completion and return the
//! outcome. This is the shared evaluation path for scenario tests, scripted
//! bot matchups, and fitness evaluation.
//!
//! The loop is strictly alternating-activation: each round asks P0 and then
//! P1 for exactly one activation. The `Command::EndTurn` every bot appends
//! drives the lifecycle; the shared driver completes a missing EndTurn once so
//! a broken policy cannot stall the match. There is no wall-clock stepping.

use crucible_sim::{Command, Game, GameConfig, Map, Player, Replay, ReplayResult, WinReason};

use crate::bot::Bot;

/// The outcome of a completed match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchOutcome {
    pub winner: Option<Player>,
    pub reason: Option<WinReason>,
    /// The activation number on which the match ended (legacy compatibility).
    pub duration_turns: i32,
    /// The player-facing round containing the terminal activation.
    pub duration_rounds: i32,
}

impl MatchOutcome {
    /// Whether `p` won this match.
    pub fn won_by(&self, p: Player) -> bool {
        self.winner == Some(p)
    }
}

/// A match outcome plus the final remaining value of each side (for margin
/// shaping in fitness evaluation).
#[derive(Clone, Copy, Debug)]
pub struct DetailedOutcome {
    pub outcome: MatchOutcome,
    pub p0_value: i32,
    pub p1_value: i32,
}

/// Drive exactly one activation for `player` and return the commands that
/// were issued. A policy that omits EndTurn is advanced once as a safety
/// fallback; it is never polled repeatedly for the same activation.
///
/// This function is shared by live/server-style orchestration and headless
/// evaluation so both paths have identical turn accounting.
pub fn drive_bot_turn(game: &mut Game, player: Player, bot: &mut dyn Bot) -> Vec<Command> {
    if game.is_over() || game.active != player {
        return Vec::new();
    }
    let commands = bot.decide(game, player);
    game.apply_commands(player, &commands);
    if !game.is_over() && game.active == player {
        game.end_turn();
    }
    commands
}

/// Run a single match to completion between two bots.
///
/// Each bot is polled once per own turn; its commands are applied through the
/// sim's normal validation/action-budget path. `seed` determines the map; the
/// match is fully deterministic given `seed`, `config`, and the two bot
/// programs.
pub fn run_match(seed: u64, config: &GameConfig, a: &mut dyn Bot, b: &mut dyn Bot) -> MatchOutcome {
    run_match_detailed(seed, config, a, b).outcome
}

/// Like [`run_match`], but also reports the final remaining value of each side.
pub fn run_match_detailed(
    seed: u64,
    config: &GameConfig,
    a: &mut dyn Bot,
    b: &mut dyn Bot,
) -> DetailedOutcome {
    run_match_with_replay(seed, config, a, b).0
}

/// Run a match to completion, returning both the outcome and the full input-log
/// replay (map seed + every command + result) so it can be stored, spectated,
/// or re-run byte-identically.
pub fn run_match_with_replay(
    seed: u64,
    config: &GameConfig,
    a: &mut dyn Bot,
    b: &mut dyn Bot,
) -> (DetailedOutcome, Replay) {
    let mut game = Game::new(Map::generate(seed), config.clone());
    let mut replay = Replay::new(seed, config.clone());

    // The activation cap remains the compatibility safety valve for an
    // unlimited config. `drive_bot_turn` itself always makes progress, so a
    // stalled bot cannot consume an unbounded number of decisions.
    let max_turns = if config.timeout_turns > 0 {
        config.timeout_turns + 200
    } else {
        10_000
    };

    while !game.is_over() && game.turn <= max_turns {
        let p = game.active;
        let turn = game.turn;
        let round = game.round;
        let cmds = if p == Player::P0 {
            drive_bot_turn(&mut game, p, a)
        } else {
            drive_bot_turn(&mut game, p, b)
        };
        for c in cmds {
            replay.record_at(round, turn, p, c);
        }
    }

    replay.result = Some(ReplayResult {
        winner: game.winner,
        reason: game.win_reason,
        duration_turns: game.turn,
        duration_rounds: game.round,
    });

    let outcome = DetailedOutcome {
        outcome: MatchOutcome {
            winner: game.winner,
            reason: game.win_reason,
            duration_turns: game.turn,
            duration_rounds: game.round,
        },
        p0_value: game.remaining_value(Player::P0),
        p1_value: game.remaining_value(Player::P1),
    };
    (outcome, replay)
}

/// Run a head-to-head series and report win counts. `a` plays P0, `b` plays P1.
pub fn series(
    seeds: impl Iterator<Item = u64>,
    config: &GameConfig,
    make_a: impl Fn() -> Box<dyn Bot>,
    make_b: impl Fn() -> Box<dyn Bot>,
) -> SeriesReport {
    let mut report = SeriesReport::default();
    for seed in seeds {
        let mut a = make_a();
        let mut b = make_b();
        let outcome = run_match(seed, config, a.as_mut(), b.as_mut());
        match outcome.winner {
            Some(Player::P0) => report.a_wins += 1,
            Some(Player::P1) => report.b_wins += 1,
            None => report.draws += 1,
        }
        report.matches += 1;
    }
    report
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SeriesReport {
    pub matches: u32,
    pub a_wins: u32,
    pub b_wins: u32,
    pub draws: u32,
}

impl SeriesReport {
    /// `a`'s win rate as a fraction in [0, 1].
    pub fn a_win_rate(&self) -> f64 {
        if self.matches == 0 {
            0.0
        } else {
            self.a_wins as f64 / self.matches as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripted::{hard, medium};
    use crate::GenomeBot;
    use crucible_sim::serialize;
    use crucible_sim::Command;

    #[test]
    fn replay_reproduces_match_exactly() {
        let cfg = GameConfig {
            timeout_turns: 60,
            ..GameConfig::default()
        };
        let mut a = GenomeBot::new(crucible_ai_genome(7));
        let mut b = hard();
        let (outcome, replay) = run_match_with_replay(3, &cfg, &mut a, &mut b);

        // Re-run the input log; commands execute immediately, so the fresh
        // game must land on the identical final state with no extra stepping.
        let repro = serialize::replay_to_game(&replay);
        assert_eq!(
            repro.winner,
            replay.result.as_ref().and_then(|r| r.winner),
            "replayed winner must match the recorded result"
        );
        assert_eq!(repro.winner, outcome.outcome.winner);
        assert_eq!(repro.turn, outcome.outcome.duration_turns);
        assert_eq!(repro.round, outcome.outcome.duration_rounds);
    }

    #[test]
    fn replay_is_deterministic() {
        let cfg = GameConfig {
            timeout_turns: 40,
            ..GameConfig::default()
        };
        let g = crucible_ai_genome(11);
        let (o1, r1) = run_match_with_replay(5, &cfg, &mut GenomeBot::new(g.clone()), &mut hard());
        let (o2, r2) = run_match_with_replay(5, &cfg, &mut GenomeBot::new(g), &mut hard());
        assert_eq!(o1.outcome, o2.outcome);
        assert_eq!(r1.to_json(), r2.to_json());
    }

    /// A bot that never ends its activation must not be polled repeatedly for
    /// the same activation.
    #[test]
    fn stalling_bot_cannot_hang_the_runner() {
        struct Stall;
        impl Bot for Stall {
            fn name(&self) -> &'static str {
                "stall"
            }
            fn decide(&mut self, _g: &Game, _p: Player) -> Vec<Command> {
                Vec::new()
            }
        }
        let cfg = GameConfig::default();
        let mut s = Stall;
        let mut h = medium();
        let o = run_match(9, &cfg, &mut s, &mut h);
        assert!(!o.won_by(Player::P0));
        assert!(o.duration_turns > 0);
        assert!(o.duration_rounds > 0);
    }

    fn crucible_ai_genome(seed: u64) -> Vec<f32> {
        crate::init(&mut crucible_sim::Rng::from_seed(seed))
    }
}
