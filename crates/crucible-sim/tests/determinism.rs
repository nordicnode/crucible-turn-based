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

#[allow(dead_code)]
fn _balance_refs() {
    let _ = building_stats(BuildingType::Hq).hp;
    let _ = unit_stats(UnitType::Tank).cost;
}
