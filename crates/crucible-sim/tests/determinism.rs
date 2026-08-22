//! M1 acceptance tests: cross-run byte-identical snapshots (golden hashes) and
//! map fairness invariants.
//!
//! The golden hashes pin the *exact* serialized state of a scripted match at
//! specific turns. If any change alters sim behavior (float use, HashMap
//! iteration, entity order, rand drift, ...) these hashes change and fail.
//!
//! The scenario and constants live in `crucible_sim::golden` so that the native
//! test here and the wasm test in `crucible-client-wasm/tests/wasm_parity.rs`
//! exercise *identical* code paths against the *same* constants — proving
//! native/wasm parity rather than two hand-kept copies drifting apart.

use crucible_sim::{building_stats, unit_stats, BuildingType, Map, UnitType};

#[test]
fn golden_snapshots_are_stable() {
    let got = crucible_sim::golden::golden_hashes();
    println!("golden hashes: {got:?}");

    assert_eq!(got, crucible_sim::golden::GOLDEN, "golden hash changed");
}

#[test]
fn same_seed_replays_byte_identical() {
    for turn in [2i32, 5, 12, 40] {
        let ga = crucible_sim::golden::combat_playout(777, turn);
        let gb = crucible_sim::golden::combat_playout(777, turn);
        assert_eq!(
            crucible_sim::serialize::snapshot_bytes(&ga),
            crucible_sim::serialize::snapshot_bytes(&gb),
            "divergence at turn {turn}"
        );
    }
}

#[test]
fn map_fairness_over_10k_seeds() {
    for seed in 0..10_000u64 {
        let map = Map::generate(seed);
        for idx in 0..(64 * 64) {
            let (x, y) = (idx % 64, idx / 64);
            let midx = (63 - y) * 64 + (63 - x);
            assert_eq!(
                map.passable[idx], map.passable[midx],
                "passable asymmetry seed {seed}"
            );
            assert_eq!(map.ore[idx], map.ore[midx], "ore asymmetry seed {seed}");
        }
        assert_eq!(
            map.hq_tiles[0],
            (63 - map.hq_tiles[1].0, 63 - map.hq_tiles[1].1),
            "HQ mirror seed {seed}"
        );
        assert!(map.is_passable(map.hq_tiles[0].0, map.hq_tiles[0].1));
        assert!(map.is_passable(map.hq_tiles[1].0, map.hq_tiles[1].1));
    }
}

/// Every generated map must let either player put a Refinery down on turn 1:
/// there is always an ore tile near spawn with a free, passable, unpaid-for
/// neighbor inside the build radius. This is the player-facing guarantee that
/// replaced the old "ore at the edge of vision" spawns.
#[test]
fn every_map_supports_turn1_refinery() {
    use crucible_sim::{BuildingType, Command, GameConfig, Player};
    for seed in 0..1000u64 {
        let mut g = crucible_sim::Game::new(
            Map::generate(seed),
            GameConfig {
                starting_ore: 1000,
                ..GameConfig::default()
            },
        );
        let hq = g.hq(Player::P0).unwrap().tile;
        let ore = nearest_ore_tile(&g, hq);
        let tile = free_refinery_slot(&g, ore);
        let res = g.apply_commands(
            Player::P0,
            &[Command::PlaceBuilding {
                player: Player::P0,
                btype: BuildingType::Refinery,
                tile,
            }],
        );
        assert_eq!(
            res,
            vec![Ok(())],
            "seed {seed}: turn-1 refinery rejected at ({},{}) near ore ({},{})",
            tile.0,
            tile.1,
            ore.0,
            ore.1
        );
    }
}

/// The nearest ore tile to `from` (Chebyshev).
fn nearest_ore_tile(g: &crucible_sim::Game, from: (u8, u8)) -> (u8, u8) {
    let mut best: Option<(i32, (u8, u8))> = None;
    for (idx, &amount) in g.map.ore.iter().enumerate() {
        if amount <= 0 {
            continue;
        }
        let t = crucible_sim::map::tile_coords(idx);
        let d = crucible_sim::tiles::chebyshev(from.0, from.1, t.0, t.1);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, t));
        }
    }
    best.unwrap().1
}

/// A passable, unoccupied, non-ore neighbor of `t`, ascending tile-index (the
/// same tie-break the golden scenario and bots use).
fn free_refinery_slot(g: &crucible_sim::Game, t: (u8, u8)) -> (u8, u8) {
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
        if x >= 0 && y >= 0 && x < 64 && y < 64 && !(x == t.0 as i32 && y == t.1 as i32) {
            Some((x as u8, y as u8))
        } else {
            None
        }
    })
    .collect();
    candidates.sort_by_key(|&tt| crucible_sim::map::tile_index(tt.0, tt.1));
    // Fall through to `t` itself if every neighbor is blocked (never for the
    // generated maps, which guarantee connectivity).
    candidates
        .into_iter()
        .find(|&tt| {
            g.map.is_passable(tt.0, tt.1)
                && g.building_at(tt).is_none()
                && g.map.ore_at(tt.0, tt.1) == 0
        })
        .unwrap_or(t)
}

#[allow(dead_code)]
fn _balance_refs() {
    let _ = building_stats(BuildingType::Hq).hp;
    let _ = unit_stats(UnitType::Tank).cost;
}
