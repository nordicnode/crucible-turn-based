//! Diagnostic: hard vs medium outcomes + easy economy. Prints unit types and
//! HQ health every few turns for the first match, then per-seed outcomes.
use crucible_ai::{easy, hard, medium, Bot};
use crucible_sim::{building_stats, Command, Game, GameConfig, Map, Player, UnitType};
fn building_stats2(bt: crucible_sim::BuildingType) -> crucible_sim::entity::BuildingStats {
    building_stats(bt)
}

fn unit_counts(g: &Game, p: Player) -> String {
    let mut parts = Vec::new();
    for ut in [
        UnitType::Infantry,
        UnitType::Tank,
        UnitType::Artillery,
        UnitType::MammothTank,
        UnitType::Gunship,
        UnitType::Interceptor,
    ] {
        let n = g
            .units
            .iter()
            .filter(|u| u.owner == p && u.utype == ut)
            .count();
        if n > 0 {
            parts.push(format!("{ut:?}x{n}"));
        }
    }
    parts.join(" ")
}

fn main() {
    // --- Easy economy trace ---
    {
        let cfg = GameConfig::default();
        let mut game = Game::new(Map::generate(42), cfg.clone());
        let mut p0 = easy();
        while game.turn <= 60 && !game.is_over() {
            if game.active == Player::P0 {
                let cmds = p0.decide(&game, Player::P0);
                let desc: Vec<String> = cmds
                    .iter()
                    .map(|c| match c {
                        Command::TrainUnit { utype, .. } => format!("train:{utype:?}"),
                        Command::PlaceBuilding { btype, .. } => format!("build:{btype:?}"),
                        Command::MoveGroup { .. } => "move".into(),
                        Command::Attack { .. } => "attack".into(),
                        _ => "end".into(),
                    })
                    .collect();
                if desc.iter().any(|d| d != "end") {
                    println!("EASY turn {}: {desc:?}", game.turn);
                }
                let results = game.apply_commands(Player::P0, &cmds);
                for (i, r) in results.iter().enumerate() {
                    if let Err(e) = r {
                        println!("EASY turn {} cmd[{i}]: REJECTED {e}", game.turn);
                    }
                }
            } else {
                game.apply_commands(Player::P1, &[Command::EndTurn { player: Player::P1 }]);
            }
            if game.turn % 10 == 0 {
                let b: Vec<String> = game
                    .buildings
                    .iter()
                    .filter(|b| b.owner == Player::P0)
                    .map(|b| format!("{:?}", b.btype))
                    .collect();
                println!(
                    "EASY turn {:3}: ore={} units=[{}] builds={b:?}",
                    game.turn,
                    game.ore[0],
                    unit_counts(&game, Player::P0),
                );
            }
        }
    }

    // --- Hard vs medium trace ---
    {
        let cfg = GameConfig {
            timeout_turns: 300,
            ..Default::default()
        };
        let mut game = Game::new(Map::generate(0), cfg.clone());
        let mut p0 = hard();
        let mut p1 = medium();
        let (mut moves, mut attacks) = (0usize, 0usize);
        let mut last_atk_turn = 0;
        let mut seen = std::collections::HashMap::new();
        let mut total_attacks = 0usize;
        let mut total_unit_deaths = 0usize;
        while !game.is_over() {
            let p = game.active;
            let cmds = if p == Player::P0 {
                p0.decide(&game, p)
            } else {
                p1.decide(&game, p)
            };
            for c in &cmds {
                match c {
                    Command::MoveGroup { .. } => moves += 1,
                    Command::Attack { .. } => attacks += 1,
                    _ => {}
                }
            }
            game.apply_commands(p, &cmds);
            for ev in &game.events {
                match &ev.kind {
                    crucible_sim::EventKind::Attacked {
                        attacker, target, ..
                    } => {
                        *seen.entry((*attacker, *target)).or_insert(0) += 1;
                        total_attacks += 1;
                        last_atk_turn = game.turn;
                    }
                    crucible_sim::EventKind::UnitDied { .. } => total_unit_deaths += 1,
                    _ => {}
                }
            }
            if game.turn % 20 == 0 {
                let hq0 = game.hq(Player::P0).map(|b| b.hp).unwrap_or(-1);
                let hq1 = game.hq(Player::P1).map(|b| b.hp).unwrap_or(-1);
                let fwd0: Vec<String> = game
                    .units
                    .iter()
                    .filter(|u| u.owner == Player::P0)
                    .map(|u| format!("({},{})", u.tile.0, u.tile.1))
                    .take(3)
                    .collect();
                println!(
                    "HvM turn {:3}: p0=[{}] p1=[{}] hq0={hq0} hq1={hq1} moves={moves} attacks={attacks} p0pos={fwd0:?}",
                    game.turn,
                    unit_counts(&game, Player::P0),
                    unit_counts(&game, Player::P1),
                );
            }
        }
        println!(
            "HvM seed 0: winner={:?} turns={} total_attacks={total_attacks} deaths={total_unit_deaths} last_atk={last_atk_turn}",
            game.winner, game.turn
        );
        let mut pairs: Vec<_> = seen.iter().collect();
        pairs.sort_by_key(|(k, _)| k.0);
        for ((att, tgt), n) in pairs.iter().take(12) {
            let an = game
                .any_unit(*att)
                .map(|u| format!("{:?}", u.utype))
                .or_else(|| game.any_building(*att).map(|b| format!("{:?}", b.btype)))
                .unwrap_or("gone".into());
            let tn = game
                .any_unit(*tgt)
                .map(|u| format!("{:?}", u.utype))
                .or_else(|| game.any_building(*tgt).map(|b| format!("{:?}", b.btype)))
                .unwrap_or("gone".into());
            println!("  {att} ({an}) -> {tgt} ({tn}) x{n}");
        }
    }

    // --- Medium vs easy over seeds: value breakdown at timeout ---
    let mut wins = 0;
    let mut draws = 0;
    for seed in 0..32u64 {
        let cfg = GameConfig {
            timeout_turns: 300,
            ..Default::default()
        };
        let mut game = Game::new(Map::generate(seed), cfg.clone());
        let mut a = medium();
        let mut b = easy();
        while !game.is_over() {
            let p = game.active;
            let cmds = if p == Player::P0 {
                a.decide(&game, p)
            } else {
                b.decide(&game, p)
            };
            game.apply_commands(p, &cmds);
        }
        let v0 = game.remaining_value(Player::P0);
        let v1 = game.remaining_value(Player::P1);
        let b0: i32 = game
            .buildings
            .iter()
            .filter(|x| x.owner == Player::P0)
            .map(|x| building_stats2(x.btype).cost)
            .sum();
        let b1: i32 = game
            .buildings
            .iter()
            .filter(|x| x.owner == Player::P1)
            .map(|x| building_stats2(x.btype).cost)
            .sum();
        println!(
            "MvE seed {seed}: winner={:?} turns={} value0={v0}(units {}, builds {b0}, ore {}) value1={v1}(units {}, builds {b1}, ore {})",
            game.winner,
            game.turn,
            game.units.iter().filter(|u| u.owner == Player::P0).count(),
            game.units.iter().filter(|u| u.owner == Player::P1).count(),
            game.ore[0],
            game.ore[1],
        );
        if game.winner == Some(Player::P0) {
            wins += 1;
        }
        if game.winner.is_none() {
            draws += 1;
        }
    }
    println!("medium beats easy {wins}/32 draws={draws}");
}
