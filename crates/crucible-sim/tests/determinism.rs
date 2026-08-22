//! Determinism acceptance tests: cross-run byte-identical snapshots (golden
//! hashes) and strategic map invariants.
//!
//! The golden hashes pin the *exact* serialized state of a scripted match at
//! specific turns. If any change alters sim behavior (float use, HashMap
//! iteration, entity order, rand drift, ...) these hashes change and fail.
//!
//! The scenario and constants live in `crucible_sim::golden` so that the native
//! test here and the wasm test in `crucible-client-wasm/tests/wasm_parity.rs`
//! exercise *identical* code paths against the *same* constants — proving
//! native/wasm parity rather than two hand-kept copies drifting apart.

use crucible_sim::map::{MAP_SIZE, MAP_TILES};
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
fn strategic_map_invariants_over_10k_seeds() {
    // The modern generator is intentionally asymmetric: mirrored maps are
    // predictable, not strategically interesting. Fairness is instead a
    // scored constraint over spawn envelopes, route cost, and resource bands.
    let mut saw_asymmetry = false;
    for seed in 0..10_000u64 {
        let map = Map::generate(seed);
        assert_eq!(map.passable.len(), MAP_TILES);
        assert_eq!(map.terrain.len(), MAP_TILES);
        assert_eq!(map.resource_kind.len(), MAP_TILES);
        assert_eq!(map.richness.len(), MAP_TILES);

        let mut counts = [0usize; 4];
        let mut asymmetry = false;
        for idx in 0..MAP_TILES {
            assert_eq!(
                map.passable[idx],
                map.terrain[idx].is_passable(),
                "passability drift seed {seed} idx {idx}"
            );
            let (x, y) = crucible_sim::map::tile_coords(idx);
            let mirror = MAP_SIZE as u8 - 1;
            let mirror_idx = crucible_sim::map::tile_index(mirror - x, mirror - y);
            if map.terrain[idx] != map.terrain[mirror_idx]
                || map.resource_kind[idx] != map.resource_kind[mirror_idx]
                || map.ore[idx] != map.ore[mirror_idx]
                || map.steel[idx] != map.steel[mirror_idx]
                || map.coal[idx] != map.coal[mirror_idx]
                || map.crystal[idx] != map.crystal[mirror_idx]
            {
                asymmetry = true;
            }
            let kind = map.resource_at(x, y);
            let amount = map.resource_amount_at(x, y);
            match kind {
                Some(resource) => {
                    assert!(amount > 0, "empty resource metadata seed {seed} idx {idx}");
                    assert!((1..=3).contains(&map.resource_richness_at(x, y)));
                    counts[resource.index()] += 1;
                }
                None => {
                    assert_eq!(
                        amount, 0,
                        "resource amount without kind seed {seed} idx {idx}"
                    );
                    assert_eq!(
                        map.richness[idx], 0,
                        "richness without resource seed {seed} idx {idx}"
                    );
                }
            }
        }
        assert!(
            counts.iter().all(|&count| count >= 5),
            "resource roster incomplete for seed {seed}: {counts:?}"
        );
        assert!(map.is_passable(map.hq_tiles[0].0, map.hq_tiles[0].1));
        assert!(map.is_passable(map.hq_tiles[1].0, map.hq_tiles[1].1));
        assert_ne!(map.hq_tiles[0], map.hq_tiles[1]);
        saw_asymmetry |= asymmetry;
    }
    assert!(saw_asymmetry, "generator unexpectedly remained mirrored");
}

/// The player-facing opening contract: ore sits at (or just beyond) the edge
/// of the HQ's opening sightline, steel and coal are within a short scout,
/// and the home view is never a blank plains pad — it always contains a mix
/// of terrain. This pins the "interesting, resource-rich spawn" guarantee.
#[test]
fn opening_resources_are_close_and_terrain_is_varied() {
    use crucible_sim::entity::ResourceType;
    for seed in 0..3000u64 {
        let map = Map::generate(seed);
        for &hq in &map.hq_tiles {
            let mut ore_d = i32::MAX;
            let mut steel_d = i32::MAX;
            let mut coal_d = i32::MAX;
            let mut kinds = [false; 8];
            for idx in 0..MAP_TILES {
                let t = crucible_sim::map::tile_coords(idx);
                let d = crucible_sim::tiles::chebyshev(hq.0, hq.1, t.0, t.1);
                if d <= 7 {
                    kinds[match map.terrain[idx] {
                        crucible_sim::map::Terrain::Plains => 0,
                        crucible_sim::map::Terrain::Forest => 1,
                        crucible_sim::map::Terrain::Hills => 2,
                        crucible_sim::map::Terrain::Desert => 3,
                        crucible_sim::map::Terrain::Swamp => 4,
                        crucible_sim::map::Terrain::Water => 5,
                        crucible_sim::map::Terrain::River => 6,
                        crucible_sim::map::Terrain::Mountain => 7,
                    }] = true;
                }
                match map.resource_at(t.0, t.1) {
                    Some(ResourceType::Ore) => ore_d = ore_d.min(d),
                    Some(ResourceType::Steel) => steel_d = steel_d.min(d),
                    Some(ResourceType::Coal) => coal_d = coal_d.min(d),
                    _ => {}
                }
            }
            assert!(ore_d <= 10, "seed {seed} hq {hq:?}: ore {ore_d} tiles out");
            assert!(
                steel_d <= 16,
                "seed {seed} hq {hq:?}: steel {steel_d} tiles out"
            );
            assert!(
                coal_d <= 16,
                "seed {seed} hq {hq:?}: coal {coal_d} tiles out"
            );
            assert!(
                kinds.iter().filter(|&&k| k).count() >= 2,
                "seed {seed} hq {hq:?}: home view is single-terrain"
            );
        }
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
        // A refinery claims the deposit tile itself; no adjacent slot is
        // needed and the resource remains extractable under the footprint.
        let tile = ore;
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

#[allow(dead_code)]
fn _balance_refs() {
    let _ = building_stats(BuildingType::Hq).hp;
    let _ = unit_stats(UnitType::Tank).cost;
}
