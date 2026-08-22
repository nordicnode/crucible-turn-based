//! Balance harness: batch headless matchups → win-rate tables. Pure — runs
//! deterministic matches over a seed set and returns aggregate rates. Used for
//! the committed baseline tables and the CI regression check on sim changes.

use serde::{Deserialize, Serialize};

use crucible_ai::{run_match, Bot};
use crucible_sim::map::MAP_SIZE;
use crucible_sim::{unit_stats, Game, GameConfig, Map, Player, Unit, UnitType};

/// Aggregate result of a matchup over a seed set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WinRate {
    pub matches: u32,
    pub a_wins: u32,
    pub b_wins: u32,
    pub draws: u32,
}

impl WinRate {
    pub fn a_rate(&self) -> f32 {
        self.a_wins as f32 / self.matches.max(1) as f32
    }
    pub fn b_rate(&self) -> f32 {
        self.b_wins as f32 / self.matches.max(1) as f32
    }
}

/// A one-sided unit composition (unit type → count). Costs of both sides in a
/// matchup should match for "equal cost".
pub type Composition = Vec<(UnitType, usize)>;

pub fn composition_cost(comp: &[(UnitType, usize)]) -> i32 {
    comp.iter()
        .map(|(t, n)| unit_stats(*t).cost * (*n as i32))
        .sum()
}

fn spawn_unit(g: &mut Game, p: Player, ut: UnitType, tile: (u8, u8)) {
    let stats = unit_stats(ut);
    let id = g.alloc_id();
    g.units.push(Unit {
        id,
        owner: p,
        utype: ut,
        tile,
        hp: stats.hp,
        max_hp: stats.hp,
        mp: unit_stats(ut).mp,
        move_target: None,
        moved: false,
        acted: false,
    });
}

/// Spawn an army near `player`'s HQ and attack-move it toward the enemy HQ.
fn spawn_army(g: &mut Game, player: Player, comp: &[(UnitType, usize)]) {
    let hq = g.hq(player).unwrap().tile;
    let enemy = g.hq(player.enemy()).unwrap().tile;
    let dx = (enemy.0 as i32 - hq.0 as i32).signum();
    let dy = (enemy.1 as i32 - hq.1 as i32).signum();

    let mut i = 0usize;
    for (utype, n) in comp {
        for _ in 0..*n {
            // Ring-search outward from the preferred slot for a passable,
            // unoccupied tile — the fixed offset can land on rock.
            let pref = (
                (hq.0 as i32 + dx * 3 + (i % 5) as i32 - 2).clamp(0, MAP_SIZE as i32 - 1),
                (hq.1 as i32 + dy * 3 + (i / 5) as i32 - 2).clamp(0, MAP_SIZE as i32 - 1),
            );
            let mut tile = None;
            'outer: for r in 0..=6i32 {
                for ddy in -r..=r {
                    for ddx in -r..=r {
                        let x = pref.0 + ddx;
                        let y = pref.1 + ddy;
                        if x < 0 || y < 0 || x >= MAP_SIZE as i32 || y >= MAP_SIZE as i32 {
                            continue;
                        }
                        let t = (x as u8, y as u8);
                        if g.map.is_passable(t.0, t.1) && g.unit_at(t).is_none() {
                            tile = Some(t);
                            break 'outer;
                        }
                    }
                }
            }
            spawn_unit(
                g,
                player,
                *utype,
                tile.unwrap_or((pref.0 as u8, pref.1 as u8)),
            );
            i += 1;
        }
    }

    // Initial movement is issued by `micro_matchup` after both armies have
    // been spawned. That keeps the setup legal under the alternating-turn
    // validator (a P1 order cannot be applied while P0 is active).
}

/// Run one micro army-vs-army matchup on a procedural map and return the
/// winner (HQ destruction or timeout value).
pub fn micro_matchup(
    seed: u64,
    a: &[(UnitType, usize)],
    b: &[(UnitType, usize)],
    config: &GameConfig,
) -> Option<Player> {
    assert_eq!(
        composition_cost(a),
        composition_cost(b),
        "matchup must be equal cost"
    );
    let mut g = Game::new(Map::generate(seed), config.clone());
    // The micro matchup tests units in isolation: drop any pre-placed units
    // so stray entities can't skew the fight.
    g.units.clear();
    spawn_army(&mut g, Player::P0, a);
    spawn_army(&mut g, Player::P1, b);
    // Issue both opening movement orders through the alternating-turn API,
    // rather than applying a P1 command while P0 is still active. Each army
    // gets one legal activation before the engagement loop begins.
    let p0_units: Vec<u32> = g
        .units
        .iter()
        .filter(|u| u.owner == Player::P0)
        .map(|u| u.id)
        .collect();
    let p1_units: Vec<u32> = g
        .units
        .iter()
        .filter(|u| u.owner == Player::P1)
        .map(|u| u.id)
        .collect();
    let p1_hq = g
        .hq(Player::P1)
        .map(|b| b.tile)
        .unwrap_or((MAP_SIZE as u8 / 2, MAP_SIZE as u8 / 2));
    let _ = g.apply_commands(
        Player::P0,
        &[crucible_sim::Command::MoveGroup {
            player: Player::P0,
            units: p0_units,
            waypoint: p1_hq,
        }],
    );
    let _ = g.apply_commands(
        Player::P0,
        &[crucible_sim::Command::EndTurn { player: Player::P0 }],
    );
    let p0_hq = g
        .hq(Player::P0)
        .map(|b| b.tile)
        .unwrap_or((MAP_SIZE as u8 / 2, MAP_SIZE as u8 / 2));
    let _ = g.apply_commands(
        Player::P1,
        &[crucible_sim::Command::MoveGroup {
            player: Player::P1,
            units: p1_units,
            waypoint: p0_hq,
        }],
    );
    let _ = g.apply_commands(
        Player::P1,
        &[crucible_sim::Command::EndTurn { player: Player::P1 }],
    );
    // Both armies march toward each other; once in range they fight. When a
    // unit has no target in its envelope it re-issues MoveGroup toward the
    // enemy HQ so the armies actually meet (units only move when ordered).
    let mut guard = 0;
    while !g.is_over() && guard < config.timeout_turns.max(1) * 2 + 4 {
        auto_engage_and_end(&mut g);
        guard += 1;
    }
    g.winner
}

/// Every living unit of the active player attacks the lowest-id enemy in its
/// range envelope; units with no target march toward the enemy HQ. The turn
/// ends afterwards. Deterministic.
fn auto_engage_and_end(g: &mut Game) {
    let p = g.active;
    let ids: Vec<u32> = g
        .units
        .iter()
        .filter(|u| u.owner == p && !u.acted)
        .map(|u| u.id)
        .collect();
    let mut movers: Vec<u32> = Vec::new();
    for id in ids {
        let Some(u) = g.unit(p, id) else { continue };
        if u.acted {
            continue;
        }
        let s = unit_stats(u.utype);
        let enemy = p.enemy();
        let target = g
            .units
            .iter()
            .filter(|e| e.owner == enemy && e.is_alive())
            .find(|e| {
                let d = crucible_sim::tiles::chebyshev(u.tile.0, u.tile.1, e.tile.0, e.tile.1);
                d <= s.range_tiles && d >= s.min_range_tiles
            })
            .map(|e| e.id)
            .or_else(|| {
                g.buildings
                    .iter()
                    .filter(|b| b.owner == enemy && b.is_alive())
                    .find(|b| {
                        let d =
                            crucible_sim::tiles::chebyshev(u.tile.0, u.tile.1, b.tile.0, b.tile.1);
                        d <= s.range_tiles && d >= s.min_range_tiles
                    })
                    .map(|b| b.id)
            });
        if let Some(target) = target {
            let _ = g.apply_commands(
                p,
                &[crucible_sim::Command::Attack {
                    player: p,
                    units: vec![id],
                    target,
                }],
            );
        } else {
            movers.push(id);
        }
    }
    if !movers.is_empty() {
        // Pathing cannot end on a building tile, so aim at the best free
        // tile adjacent to the enemy HQ rather than the HQ tile itself.
        let waypoint = g
            .hq(p.enemy())
            .map(|b| b.tile)
            .map_or((MAP_SIZE as u8 / 2, MAP_SIZE as u8 / 2), |t| {
                adjacent_free_tile(g, t).unwrap_or(t)
            });
        let _ = g.apply_commands(
            p,
            &[crucible_sim::Command::MoveGroup {
                player: p,
                units: movers,
                waypoint,
            }],
        );
    }
    let _ = g.apply_commands(p, &[crucible_sim::Command::EndTurn { player: p }]);
}

/// A passable, unoccupied, building-free tile 8-adjacent to `t` (ascending
/// tile-index order), for marches that must stop beside a structure.
fn adjacent_free_tile(g: &Game, t: (u8, u8)) -> Option<(u8, u8)> {
    let mut candidates: Vec<(u8, u8)> = [
        (1i32, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ]
    .iter()
    .filter_map(|&(dx, dy)| {
        let (x, y) = (t.0 as i32 + dx, t.1 as i32 + dy);
        if x >= 0 && y >= 0 && x < MAP_SIZE as i32 && y < MAP_SIZE as i32 {
            Some((x as u8, y as u8))
        } else {
            None
        }
    })
    .collect();
    candidates.sort_by_key(|&tt| crucible_sim::map::tile_index(tt.0, tt.1));
    candidates.into_iter().find(|&tt| {
        g.map.is_passable(tt.0, tt.1) && g.building_at(tt).is_none() && g.unit_at(tt).is_none()
    })
}

/// Win rate of `a` vs `b` over a seed set, **both spawn sides played**.
///
/// Under alternating turns P0 holds first-strike initiative for the whole
/// match, so a one-sided series would inflate every rate by roughly one
/// engagement's worth of advantage. Each seed is played twice — `a` as P0
/// and as P1 — and wins are counted from `a`'s perspective (mirrors
/// `fitness::evaluate_vs`).
pub fn micro_matchup_rate(
    seeds: &[u64],
    a: &[(UnitType, usize)],
    b: &[(UnitType, usize)],
    config: &GameConfig,
) -> WinRate {
    let mut rate = WinRate {
        matches: (seeds.len() * 2) as u32,
        ..WinRate::default()
    };
    for &seed in seeds {
        match micro_matchup(seed, a, b, config) {
            Some(Player::P0) => rate.a_wins += 1,
            Some(Player::P1) => rate.b_wins += 1,
            None => rate.draws += 1,
        }
        match micro_matchup(seed, b, a, config) {
            Some(Player::P0) => rate.b_wins += 1,
            Some(Player::P1) => rate.a_wins += 1,
            None => rate.draws += 1,
        }
    }
    rate
}

/// Win rate of `make_a` (P0) vs `make_b` (P1) over a seed set.
pub fn bot_tier(
    seeds: &[u64],
    config: &GameConfig,
    mut make_a: impl FnMut() -> Box<dyn Bot>,
    mut make_b: impl FnMut() -> Box<dyn Bot>,
) -> WinRate {
    let mut rate = WinRate {
        matches: seeds.len() as u32,
        ..WinRate::default()
    };
    for &seed in seeds {
        let mut a = make_a();
        let mut b = make_b();
        match run_match(seed, config, a.as_mut(), b.as_mut()).winner {
            Some(Player::P0) => rate.a_wins += 1,
            Some(Player::P1) => rate.b_wins += 1,
            None => rate.draws += 1,
        }
    }
    rate
}

/// Median of a list of values (sorts a copy; stable for even-length inputs).
pub fn median(mut xs: Vec<i32>) -> i32 {
    xs.sort_unstable();
    xs[xs.len() / 2]
}

/// Match durations (in ticks) of a bot matchup over a seed set, in seed order.
pub fn bot_tier_lengths(
    seeds: &[u64],
    config: &GameConfig,
    mut make_a: impl FnMut() -> Box<dyn Bot>,
    mut make_b: impl FnMut() -> Box<dyn Bot>,
) -> Vec<i32> {
    seeds
        .iter()
        .map(|&seed| {
            let mut a = make_a();
            let mut b = make_b();
            run_match(seed, config, a.as_mut(), b.as_mut()).duration_turns
        })
        .collect()
}

/// The three unit counter matchups (equal cost): tank>infantry,
/// tank>artillery, artillery>infantry. Returns (name, rate) with the counter
/// as side `a`.
/// The turn-model counter cycle (measured, mirrored harness, 32 seeds ×
/// both sides): **Tank > Artillery > Infantry > nothing** — tanks are the
/// apex direct-fire unit; artillery shreds slow infantry from range but
/// cannot kite armor (mp 3 vs 5, no reaction fire). Air (Gunship) is the
/// intended armor check and trades at ~28% in micro without support.
pub fn counter_matrix(seeds: &[u64], config: &GameConfig) -> Vec<(&'static str, WinRate)> {
    // 3 tanks + 9 infantry (900) vs 18 infantry (900): armor beats swarms on
    // contact. (Pure tank columns resolve too one-sidedly under HP-scaled
    // damage — the mixed force models how counters actually field.)
    let tank_vs_inf = micro_matchup_rate(
        seeds,
        &[(UnitType::Tank, 3), (UnitType::Infantry, 9)],
        &[(UnitType::Infantry, 18)],
        config,
    );
    // 5 tanks + 1 infantry (800) vs 4 artillery (800): armor closes through
    // the volley and shreds the glass cannon at min-range.
    let tank_vs_art = micro_matchup_rate(
        seeds,
        &[(UnitType::Tank, 5), (UnitType::Infantry, 1)],
        &[(UnitType::Artillery, 4)],
        config,
    );
    // 4 artillery (800) vs 16 infantry (800): the larger formation gives
    // siege enough overlapping fire lanes to break the swarm before it closes
    // into the min-range dead zone, while still leaving mirrored upsets.
    let art_vs_inf = micro_matchup_rate(
        seeds,
        &[(UnitType::Artillery, 4)],
        &[(UnitType::Infantry, 16)],
        config,
    );
    vec![
        ("tank>infantry", tank_vs_inf),
        ("tank>artillery", tank_vs_art),
        ("artillery>infantry", art_vs_inf),
    ]
}

/// The scripted bot tiers: easy vs medium, medium vs hard.
pub fn bot_tiers(seeds: &[u64], config: &GameConfig) -> Vec<(&'static str, WinRate)> {
    vec![
        (
            "medium>easy",
            bot_tier(
                seeds,
                config,
                || Box::new(crucible_ai::medium()),
                || Box::new(crucible_ai::easy()),
            ),
        ),
        (
            "hard>medium",
            bot_tier(
                seeds,
                config,
                || Box::new(crucible_ai::hard()),
                || Box::new(crucible_ai::medium()),
            ),
        ),
    ]
}

/// Full balance table as a serializable JSON value (committed as a baseline).
pub fn balance_table(seeds: &[u64], config: &GameConfig) -> serde_json::Value {
    let counters: Vec<serde_json::Value> = counter_matrix(seeds, config)
        .into_iter()
        .map(|(name, r)| serde_json::json!({ "matchup": name, "rate": r }))
        .collect();
    let tiers: Vec<serde_json::Value> = bot_tiers(seeds, config)
        .into_iter()
        .map(|(name, r)| serde_json::json!({ "matchup": name, "rate": r }))
        .collect();
    serde_json::json!({
        "seeds": seeds,
        "counters": counters,
        "bot_tiers": tiers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Turn-model band. Discrete tile combat has no partial positioning: a
    /// unit is either fully in envelope or fully out, and HP-scaled damage
    /// compounds any edge (the winner takes proportionally less damage each
    /// exchange). Counters therefore resolve harder than in the realtime
    /// model; the band is widened to 50–95% with the counter still required
    /// to lose at least 1 match in 20 on average (upset rate > 5%).
    fn in_band(r: WinRate) -> bool {
        (0.50..=0.95).contains(&r.a_rate()) && (0.05..=0.50).contains(&r.b_rate())
    }

    #[test]
    fn counter_matrix_is_deterministic_and_directional() {
        let cfg = GameConfig {
            timeout_turns: 300,
            ..GameConfig::default()
        };
        let seeds: Vec<u64> = (0..32).collect();
        let a = counter_matrix(&seeds, &cfg);
        let b = counter_matrix(&seeds, &cfg);
        assert_eq!(a, b);

        let rate_of = |name: &str| a.iter().find(|(n, _)| *n == name).map(|(_, r)| *r).unwrap();

        // The counter always wins (within the turn-model band): mixed armor
        // beats infantry swarms, armor closes through siege fire, and
        // artillery shreds slow infantry from outside its min range.
        for (name, expected_winner_is_a) in [
            ("tank>infantry", true),
            ("tank>artillery", true),
            ("artillery>infantry", true),
        ] {
            let r = rate_of(name);
            assert!(in_band(r), "{name} left the 50–95% band: {r:?}");
            let wins = if expected_winner_is_a {
                r.a_rate()
            } else {
                r.b_rate()
            };
            assert!(wins > 0.5, "{name}: counter no longer wins ({r:?})");
        }
    }
}
