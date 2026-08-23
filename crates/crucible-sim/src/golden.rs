//! #[doc(hidden)] — the shared determinism-golden scenario.
//!
//! The golden hashes pin the *exact* serialized state of a scripted match at
//! specific turns. Both the native test (`tests/determinism.rs`) and the wasm
//! test (`crucible-client-wasm/tests/wasm_parity.rs`) call [`golden_hashes`]
//! and compare against the same constants, so native/wasm parity is proven on
//! identical code paths rather than two hand-kept copies.
//!
//! v2 (turn-based): the scenario builds both bases, queues a mixed army,
//! marches it out at turn 6, and fights a real battle from turn 12. Hashes are
//! taken at turns 10 / 30 / 60.

use crate::entity::{BuildingType, EntityId, Player, UnitType};
use crate::map::MAP_SIZE;
use crate::orders::Command;
use crate::serialize::snapshot_bytes;
use crate::{Game, GameConfig, Map};

/// The seed the golden scenario runs on.
pub const SEED: u64 = 12345;

/// Golden snapshot hashes (FNV-1a over `serialize::snapshot_bytes`).
/// Recorded for the turn-based engine; if any change alters sim behavior
/// these change and the tests fail.
///
/// Re-recorded for the asymmetric terrain/deposit generator and its
/// four-resource economy. `MoveGroup` still retargets blocked waypoints (e.g.
/// the enemy HQ) to the nearest free adjacent tile, so the golden armies
/// march out and fight instead of idling at their bases.
pub const GOLDEN_10: u64 = 5246440161438374813;
pub const GOLDEN_30: u64 = 8879655095499981640;
pub const GOLDEN_60: u64 = 5005243208326061243;

pub fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

pub fn hash_snapshot(g: &Game) -> u64 {
    fnv1a(&snapshot_bytes(g))
}

fn find_building(g: &Game, p: Player, bt: BuildingType) -> EntityId {
    g.buildings
        .iter()
        .find(|b| b.owner == p && b.btype == bt)
        .map(|b| b.id)
        .unwrap_or_else(|| panic!("building missing: {bt:?} for {p:?}"))
}

/// Construct the scripted opening (bases + mixed army queues) at turn 1.
///
/// Refineries are placed on the map's natural ore pocket; everything else
/// clusters around the HQ. Construction is allowed to mature between the
/// infrastructure and production phases so the golden scenario exercises the
/// operational-building contract.
pub fn build_game(seed: u64) -> Game {
    let cfg = GameConfig {
        starting_ore: 100_000,
        starting_steel: 100_000,
        starting_coal: 100_000,
        starting_crystal: 100_000,
        timeout_turns: 10_000,
        ..GameConfig::default()
    };
    let mut g = Game::new(Map::generate(seed), cfg);

    for p in Player::ALL {
        // Alternating turns: hand the turn to `p` before it builds.
        if g.active != p {
            let _ = g.apply_commands(g.active, &[Command::EndTurn { player: g.active }]);
        }
        let (hx, hy) = g.hq(p).unwrap().tile;
        // The refinery claims the deposit tile itself, so the natural pocket
        // is both the resource source and the structure's footprint.
        let refinery_tile = nearest_ore_tile(&g, (hx, hy));
        let placements = [
            (BuildingType::PowerPlant, (hx as i32 - 2, hy as i32 - 2)),
            (BuildingType::Factory, (hx as i32, hy as i32 + 2)),
            (BuildingType::Barracks, (hx as i32 + 2, hy as i32 + 2)),
            (BuildingType::Turret, (hx as i32 - 2, hy as i32)),
        ];
        for (bt, (x, y)) in placements {
            let tile = (
                x.clamp(0, MAP_SIZE as i32 - 1) as u8,
                y.clamp(0, MAP_SIZE as i32 - 1) as u8,
            );
            let command = Command::PlaceBuilding {
                player: p,
                btype: bt,
                tile,
            };
            let result = g.apply_commands(p, std::slice::from_ref(&command));
            assert_eq!(
                result,
                vec![Ok(())],
                "golden placement {bt:?} at {tile:?} failed: {result:?}"
            );
        }
        // TechLab requires an operational Factory, not merely a reserved
        // construction site.
        let factory = find_building(&g, p, BuildingType::Factory);
        wait_until_operational(&mut g, p, factory);
        let techlab = Command::PlaceBuilding {
            player: p,
            btype: BuildingType::TechLab,
            tile: (
                (hx as i32).clamp(0, MAP_SIZE as i32 - 1) as u8,
                (hy as i32 - 2).clamp(0, MAP_SIZE as i32 - 1) as u8,
            ),
        };
        assert_eq!(
            g.apply_commands(p, std::slice::from_ref(&techlab)),
            vec![Ok(())],
            "golden placement TechLab failed"
        );
        if let Some(tile) = refinery_tile {
            let _ = g.apply_commands(
                p,
                &[Command::PlaceBuilding {
                    player: p,
                    btype: BuildingType::Refinery,
                    tile,
                }],
            );
        }
        if let Some(ctile) = nearest_crystal_tile(&g, (hx, hy)) {
            let _ = g.apply_commands(
                p,
                &[Command::PlaceBuilding {
                    player: p,
                    btype: BuildingType::CrystalRefinery,
                    tile: ctile,
                }],
            );
        }
    }

    for p in Player::ALL {
        if g.active != p {
            let _ = g.apply_commands(g.active, &[Command::EndTurn { player: g.active }]);
        }
        let factory = find_building(&g, p, BuildingType::Factory);
        let barracks = find_building(&g, p, BuildingType::Barracks);
        wait_until_operational(&mut g, p, factory);
        wait_until_operational(&mut g, p, barracks);
        let techlab = find_building(&g, p, BuildingType::TechLab);
        wait_until_operational(&mut g, p, techlab);
        let _ = g.apply_commands(
            p,
            &[Command::StartResearch {
                player: p,
                tech: crate::tech::TechId::HighExplosive,
            }],
        );
        let cmds = [
            Command::TrainUnit {
                player: p,
                building: factory,
                utype: UnitType::Tank,
            },
            Command::TrainUnit {
                player: p,
                building: factory,
                utype: UnitType::Tank,
            },
            Command::TrainUnit {
                player: p,
                building: factory,
                utype: UnitType::Artillery,
            },
            Command::TrainUnit {
                player: p,
                building: barracks,
                utype: UnitType::Infantry,
            },
            Command::TrainUnit {
                player: p,
                building: barracks,
                utype: UnitType::Infantry,
            },
            Command::TrainUnit {
                player: p,
                building: barracks,
                utype: UnitType::Infantry,
            },
        ];
        let _ = g.apply_commands(p, &cmds);
    }

    g
}

fn wait_until_operational(g: &mut Game, player: Player, building: EntityId) {
    while !g
        .building(player, building)
        .is_some_and(|b| b.is_operational())
    {
        let active = g.active;
        let result = g.apply_commands(active, &[Command::EndTurn { player: active }]);
        assert_eq!(result, vec![Ok(())]);
    }
}

fn nearest_crystal_tile(g: &Game, from: (u8, u8)) -> Option<(u8, u8)> {
    let mut best: Option<(i32, (u8, u8))> = None;
    for (idx, &amount) in g.map.crystal.iter().enumerate() {
        if amount <= 0 {
            continue;
        }
        let t = crate::map::tile_coords(idx);
        let d = crate::tiles::chebyshev(from.0, from.1, t.0, t.1);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, t));
        }
    }
    best.map(|(_, t)| t)
}

fn nearest_ore_tile(g: &Game, from: (u8, u8)) -> Option<(u8, u8)> {
    let mut best: Option<(i32, (u8, u8))> = None;
    for (idx, &amount) in g.map.ore.iter().enumerate() {
        if amount <= 0 {
            continue;
        }
        let t = crate::map::tile_coords(idx);
        let d = crate::tiles::chebyshev(from.0, from.1, t.0, t.1);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, t));
        }
    }
    best.map(|(_, t)| t)
}

/// March each side's combat units toward the enemy HQ at turn 6 and fight —
/// exercising movement, focus-fire, counters, and turret auto-fire.
pub fn issue_attack_orders(g: &mut Game) {
    for p in Player::ALL {
        let enemy_hq = g.hq(p.enemy()).unwrap().tile;
        let combat: Vec<EntityId> = g
            .units
            .iter()
            .filter(|u| u.owner == p)
            .map(|u| u.id)
            .collect();
        if !combat.is_empty() {
            let _ = g.apply_commands(
                p,
                &[Command::MoveGroup {
                    player: p,
                    units: combat,
                    waypoint: enemy_hq,
                }],
            );
        }
    }
}

/// Drive the scripted playout to `target_turn`, injecting orders at turn 6.
pub fn combat_playout(seed: u64, target_turn: i32) -> Game {
    let mut g = build_game(seed);
    while g.turn <= target_turn && !g.is_over() {
        if g.turn == 6 {
            issue_attack_orders(&mut g);
        }
        // Every unit attacks the closest enemy in range before ending the
        // turn, so the battle actually resolves instead of units idling.
        auto_engage(&mut g);
        g.end_turn();
    }
    g
}

/// Issue an Attack command for every unit with a living enemy in range
/// (lowest-id target), then end the turn. Deterministic by construction.
fn auto_engage(g: &mut Game) {
    let active = g.active;
    let ids: Vec<EntityId> = g
        .units
        .iter()
        .filter(|u| u.owner == active && !u.acted)
        .map(|u| u.id)
        .collect();
    for id in ids {
        let Some(u) = g.unit(active, id) else {
            continue;
        };
        if u.acted {
            continue;
        }
        let range = g.effective_range(u.utype, active);
        let min_r = crate::entity::unit_stats(u.utype).min_range_tiles;
        let enemy = active.enemy();
        let target = g
            .units
            .iter()
            .filter(|e| e.owner == enemy && e.is_alive())
            .find(|e| {
                let d = crate::tiles::chebyshev(u.tile.0, u.tile.1, e.tile.0, e.tile.1);
                d <= range && d >= min_r
            })
            .map(|e| e.id)
            .or_else(|| {
                g.buildings
                    .iter()
                    .filter(|b| b.owner == enemy && b.is_alive())
                    .find(|b| {
                        let d = crate::tiles::chebyshev(u.tile.0, u.tile.1, b.tile.0, b.tile.1);
                        d <= range && d >= min_r
                    })
                    .map(|b| b.id)
            });
        if let Some(target) = target {
            let _ = g.apply_commands(
                active,
                &[Command::Attack {
                    player: active,
                    units: vec![id],
                    target,
                }],
            );
        }
    }
    let _ = g.apply_commands(active, &[Command::EndTurn { player: active }]);
}

pub fn combat_hashes() -> [u64; 3] {
    [
        hash_snapshot(&combat_playout(SEED, 10)),
        hash_snapshot(&combat_playout(SEED, 30)),
        hash_snapshot(&combat_playout(SEED, 60)),
    ]
}

/// The committed combat-golden values, in the same order as [`combat_hashes`].
/// `combat_hashes` and `golden_hashes` are the same function, so the two
/// constants must agree (the wasm parity test checks `COMBAT_GOLDEN`; the
/// native determinism test checks `GOLDEN`).
pub const COMBAT_GOLDEN: [u64; 3] = [GOLDEN_10, GOLDEN_30, GOLDEN_60];

pub fn golden_hashes() -> [u64; 3] {
    combat_hashes()
}

/// The committed golden values, in the same order as [`golden_hashes`].
pub const GOLDEN: [u64; 3] = [GOLDEN_10, GOLDEN_30, GOLDEN_60];
