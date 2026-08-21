//! Change reports: behavioral fingerprints of a genome and the diff between
//! two champions. Pure — runs headless matches (vs a no-op) to sample the
//! genome's intrinsic build/attack tempo, then summarizes differences in
//! human-readable notes for the dashboard.

use crucible_ai::{Bot, GenomeBot};
use crucible_sim::{BuildingType, Game, GameConfig, Map, Player, UnitType};
use serde::{Deserialize, Serialize};

use crate::fitness::Noop;

/// A genome's aggregate behavior over a set of evaluation matches (means).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub matches: u32,
    /// Mean turn of the first enemy entity killed (proxy for first attack).
    pub first_kill_turn: f32,
    /// Mean match length in turns.
    pub duration_turns: f32,
    pub refineries: f32,
    pub infantry: f32,
    pub tanks: f32,
    pub artillery: f32,
    /// Mean ore spent on economy (refineries).
    pub economy_spend: f32,
    /// Mean ore spent on army (infantry + tanks + artillery).
    pub army_spend: f32,
}

/// Sample a genome's fingerprint by playing it against a no-op on each seed.
/// Deterministic given `genome`, `seeds`, and `config`.
pub fn fingerprint(genome: &[f32], seeds: &[u64], config: &GameConfig) -> Fingerprint {
    let mut acc = Fingerprint::default();
    let n = seeds.len() as f32;
    for &seed in seeds {
        let mut g = GenomeBot::new(genome.to_vec());
        let mut opp = Noop;
        let mut game = Game::new(Map::generate(seed), config.clone());
        let mut first_kill: Option<i32> = None;

        while !game.is_over() {
            if game.active == Player::P0 {
                let cmds = g.decide(&game, Player::P0);
                if !cmds.is_empty() {
                    game.apply_commands(Player::P0, &cmds);
                }
                let _ = &mut opp; // no-op opponent issues nothing
            }
            // Note first enemy kill before advancing (deaths land this turn).
            if first_kill.is_none() {
                first_kill = first_enemy_kill(&game, Player::P1);
            }
            game.end_turn();
        }
        if first_kill.is_none() {
            first_kill = Some(game.turn);
        }

        acc.matches += 1;
        acc.first_kill_turn += first_kill.unwrap_or(game.turn) as f32;
        acc.duration_turns += game.turn as f32;
        acc.refineries += count_buildings(&game, Player::P0, BuildingType::Refinery) as f32;
        acc.infantry += count_units(&game, Player::P0, UnitType::Infantry) as f32;
        acc.tanks += count_units(&game, Player::P0, UnitType::Tank) as f32;
        acc.artillery += count_units(&game, Player::P0, UnitType::Artillery) as f32;

        let (econ, army) = spend_split(&game, Player::P0);
        acc.economy_spend += econ as f32;
        acc.army_spend += army as f32;
    }

    let m = if acc.matches == 0 {
        1.0
    } else {
        acc.matches as f32
    };
    acc.first_kill_turn /= m;
    acc.duration_turns /= m;
    acc.refineries /= n.max(1.0);
    acc.infantry /= n.max(1.0);
    acc.tanks /= n.max(1.0);
    acc.artillery /= n.max(1.0);
    acc.economy_spend /= n.max(1.0);
    acc.army_spend /= n.max(1.0);
    acc
}

/// First turn at which any entity owned by `victim` died.
fn first_enemy_kill(game: &crucible_sim::Game, victim: Player) -> Option<i32> {
    use crucible_sim::EventKind;
    game.events.iter().find_map(|e| match &e.kind {
        EventKind::UnitDied { owner, .. } | EventKind::BuildingDestroyed { owner, .. }
            if *owner == victim =>
        {
            Some(e.turn)
        }
        _ => None,
    })
}

fn count_units(game: &crucible_sim::Game, p: Player, t: UnitType) -> usize {
    game.units
        .iter()
        .filter(|u| u.owner == p && u.utype == t)
        .count()
}

fn count_buildings(game: &crucible_sim::Game, p: Player, t: BuildingType) -> usize {
    game.buildings
        .iter()
        .filter(|b| b.owner == p && b.btype == t)
        .count()
}

fn spend_split(game: &crucible_sim::Game, p: Player) -> (i32, i32) {
    use crucible_sim::{building_stats, unit_stats};
    let mut econ = 0i32;
    let mut army = 0i32;
    for u in &game.units {
        if u.owner == p {
            army += unit_stats(u.utype).cost;
        }
    }
    for b in &game.buildings {
        if b.owner == p && b.btype == BuildingType::Refinery {
            econ += building_stats(b.btype).cost;
        }
    }
    (econ, army)
}

/// A human-readable diff between two champions' fingerprints.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChangeReport {
    pub notes: Vec<String>,
}

/// Name a champion's playstyle era from its behavioral fingerprint (plan
/// §6.2: "a simple classifier over fingerprints names playstyle eras").
/// Purely heuristic; the notes in the change report carry the specifics.
pub fn era_name(f: &Fingerprint) -> &'static str {
    let first_kill_turns = f.first_kill_turn;
    // Aggression first: a very early first kill is the defining signal.
    if f.matches > 0 && first_kill_turns < 9.0 {
        return "The Rush Years";
    }
    // Defensive economy: many refineries and a late first kill.
    if f.refineries >= 2.0 && first_kill_turns > 30.0 {
        return "The Turtle Dynasty";
    }
    // Composition-dominant eras (most-built unit class wins the name).
    let (inf, tnk, art) = (f.infantry, f.tanks, f.artillery);
    if art > tnk && art > inf {
        return "The Siege Dynasty";
    }
    if tnk > inf && tnk > art {
        return "The Armored Age";
    }
    if inf > tnk && inf > art {
        return "The Legion Era";
    }
    // Economy-heavy (bank before army).
    if f.economy_spend > f.army_spend && f.army_spend > 0.0 {
        return "The Banker Era";
    }
    "The Balanced Era"
}

/// Compare `old` (dethroned) to `new` (incoming) champion and produce notes.
pub fn diff(old: &Fingerprint, new: &Fingerprint) -> ChangeReport {
    let mut notes = Vec::new();
    let turns = |t: f32| format!("T{:.0}", t);

    let earlier = new.first_kill_turn < old.first_kill_turn;
    notes.push(format!(
        "first kill {} ({} → {})",
        if earlier { "earlier" } else { "later" },
        turns(old.first_kill_turn),
        turns(new.first_kill_turn),
    ));

    if (new.refineries - old.refineries).abs() >= 0.5 {
        notes.push(format!(
            "expands {} ({:.1} → {:.1} refineries)",
            if new.refineries > old.refineries {
                "more"
            } else {
                "less"
            },
            old.refineries,
            new.refineries,
        ));
    }

    let dominant = |f: &Fingerprint| -> &'static str {
        let v = [
            (f.infantry, "infantry"),
            (f.tanks, "tanks"),
            (f.artillery, "artillery"),
        ];
        v.iter()
            .fold(("none", f32::NEG_INFINITY), |a, &(n, name)| {
                if n > a.1 {
                    (name, n)
                } else {
                    a
                }
            })
            .0
    };
    let old_dom = dominant(old);
    let new_dom = dominant(new);
    if old_dom != new_dom {
        notes.push(format!(
            "composition shifted from {old_dom} toward {new_dom}"
        ));
    }

    let old_ratio = if old.army_spend > 0.0 {
        old.economy_spend / old.army_spend
    } else {
        0.0
    };
    let new_ratio = if new.army_spend > 0.0 {
        new.economy_spend / new.army_spend
    } else {
        0.0
    };
    notes.push(format!(
        "econ:army spend ratio {:.2} → {:.2}",
        old_ratio, new_ratio
    ));

    ChangeReport { notes }
}

/// Convenience: run the whole report pipeline (fingerprint both, then diff).
pub fn change_report(
    old_genome: &[f32],
    new_genome: &[f32],
    seeds: &[u64],
    config: &GameConfig,
) -> ChangeReport {
    let a = fingerprint(old_genome, seeds, config);
    let b = fingerprint(new_genome, seeds, config);
    diff(&a, &b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_deterministic() {
        let genome = crucible_ai::init(&mut crucible_sim::Rng::from_seed(5));
        let cfg = GameConfig {
            timeout_turns: 50,
            ..GameConfig::default()
        };
        let a = fingerprint(&genome, &[3, 4], &cfg);
        let b = fingerprint(&genome, &[3, 4], &cfg);
        assert_eq!(a, b);
        assert_eq!(a.matches, 2);
        assert!(a.duration_turns > 0.0);
    }

    #[test]
    fn era_names_cover_the_classifier() {
        let rush = Fingerprint {
            matches: 4,
            first_kill_turn: 5.0,
            ..Fingerprint::default()
        };
        assert_eq!(era_name(&rush), "The Rush Years");

        let turtle = Fingerprint {
            matches: 4,
            first_kill_turn: 40.0,
            refineries: 3.0,
            ..Fingerprint::default()
        };
        assert_eq!(era_name(&turtle), "The Turtle Dynasty");

        let siege = Fingerprint {
            matches: 4,
            first_kill_turn: 2000.0,
            artillery: 6.0,
            tanks: 2.0,
            ..Fingerprint::default()
        };
        assert_eq!(era_name(&siege), "The Siege Dynasty");

        let armored = Fingerprint {
            matches: 4,
            first_kill_turn: 2000.0,
            tanks: 8.0,
            infantry: 3.0,
            ..Fingerprint::default()
        };
        assert_eq!(era_name(&armored), "The Armored Age");

        let banker = Fingerprint {
            matches: 4,
            first_kill_turn: 2000.0,
            economy_spend: 900.0,
            army_spend: 400.0,
            ..Fingerprint::default()
        };
        assert_eq!(era_name(&banker), "The Banker Era");

        assert_eq!(era_name(&Fingerprint::default()), "The Balanced Era");
    }

    #[test]
    fn diff_produces_notes() {
        let a = Fingerprint {
            matches: 10,
            first_kill_turn: 10.0,
            refineries: 1.0,
            tanks: 5.0,
            army_spend: 750.0,
            economy_spend: 400.0,
            ..Fingerprint::default()
        };

        let b = Fingerprint {
            first_kill_turn: 6.0,
            refineries: 3.0,
            infantry: 8.0,
            ..a
        };

        let report = diff(&a, &b);
        assert!(!report.notes.is_empty());
        assert!(report.notes[0].contains("earlier"));
    }
}
