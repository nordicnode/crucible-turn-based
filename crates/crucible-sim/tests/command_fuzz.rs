//! Command fuzz: random command streams must never crash the sim, and every
//! command must either apply cleanly or be rejected by the validator — never
//! silently misapplied. Driven by the injected PRNG, so any failure is
//! deterministic and reproducible. State invariants are checked every turn.

use crucible_sim::map::MAP_SIZE;
use crucible_sim::{
    building_stats, tech::TechId, unit_stats, BuildingType, Command, CommandError, Game,
    GameConfig, Map, Player, Rng, UnitType,
};

const TURNS: i32 = 120;

fn config() -> GameConfig {
    GameConfig {
        timeout_turns: TURNS + 10,
        ..GameConfig::default()
    }
}

/// A random command drawn from the whole action space (valid tiles, entity
/// ids, and types — so the validator's rejection paths are exercised, not
/// just its happy paths). Coordinates are always in-bounds; whether the tile
/// is legal is the validator's job.
fn random_command(rng: &mut Rng, g: &Game, p: Player) -> Command {
    let tile = (
        (rng.below(MAP_SIZE as u64) as u8),
        (rng.below(MAP_SIZE as u64) as u8),
    );
    let bt = match rng.below(12) {
        0 => BuildingType::Hq,
        1 => BuildingType::PowerPlant,
        2 => BuildingType::Refinery,
        3 => BuildingType::Barracks,
        4 => BuildingType::Factory,
        5 => BuildingType::TechLab,
        6 => BuildingType::Airfield,
        7 => BuildingType::Radar,
        8 => BuildingType::TeslaCoil,
        9 => BuildingType::Turret,
        10 => BuildingType::CrystalRefinery,
        _ => BuildingType::AATurret,
    };
    let ut = match rng.below(9) {
        0 => UnitType::Infantry,
        1 => UnitType::Scout,
        2 => UnitType::RocketTrooper,
        3 => UnitType::Tank,
        4 => UnitType::Artillery,
        5 => UnitType::MammothTank,
        6 => UnitType::Gunship,
        7 => UnitType::Interceptor,
        _ => UnitType::SamLauncher,
    };
    let building_id = g
        .buildings
        .get(rng.below((g.buildings.len().max(1)) as u64) as usize)
        .map(|b| b.id)
        .unwrap_or(1);
    let unit_id = g
        .units
        .get(rng.below((g.units.len().max(1)) as u64) as usize)
        .map(|u| u.id)
        .unwrap_or(1);
    let units = match rng.below(4) {
        0 => vec![unit_id],
        1 => vec![unit_id, unit_id.wrapping_add(1)],
        2 => Vec::new(),
        _ => (0..rng.below(5)).map(|_| unit_id).collect(),
    };
    let tech = match rng.below(10) {
        0 => TechId::HighExplosive,
        1 => TechId::CompositeArmor,
        2 => TechId::TargetingOptics,
        3 => TechId::EfficientRefining,
        4 => TechId::RocketPropulsion,
        5 => TechId::TitaniumAlloys,
        6 => TechId::AerialSuperiority,
        7 => TechId::Superconductors,
        8 => TechId::CrystalNanotech,
        _ => TechId::AdvancedBallistics,
    };

    match rng.below(8) {
        0 => Command::PlaceBuilding {
            player: p,
            btype: bt,
            tile,
        },
        1 => Command::TrainUnit {
            player: p,
            building: building_id,
            utype: ut,
        },
        2 => Command::MoveGroup {
            player: p,
            units,
            waypoint: tile,
        },
        3 => Command::Attack {
            player: p,
            units,
            target: unit_id,
        },
        4 => Command::StartResearch { player: p, tech },
        5 => Command::Sell {
            player: p,
            building: building_id,
        },
        6 => Command::Repair {
            player: p,
            building: building_id,
        },
        _ => Command::EndTurn { player: p },
    }
}

/// Check the invariants the fuzz run must never violate.
fn check_invariants(g: &Game) {
    // Ids are unique and nonzero.
    let mut unit_ids: Vec<u32> = g.units.iter().map(|u| u.id).collect();
    unit_ids.sort_unstable();
    unit_ids.dedup();
    assert_eq!(unit_ids.len(), g.units.len(), "duplicate unit ids");
    assert!(unit_ids.iter().all(|&id| id > 0), "zero unit id");

    let mut building_ids: Vec<u32> = g.buildings.iter().map(|b| b.id).collect();
    building_ids.sort_unstable();
    building_ids.dedup();
    assert_eq!(
        building_ids.len(),
        g.buildings.len(),
        "duplicate building ids"
    );

    // All entities are on the board.
    for u in &g.units {
        assert!(
            u.tile.0 < MAP_SIZE as u8 && u.tile.1 < MAP_SIZE as u8,
            "unit off the map"
        );
        assert!(u.hp > 0, "dead unit still present");
        assert!(
            u.mp >= 0 && u.mp <= unit_stats(u.utype).mp,
            "mp out of range"
        );
    }
    for b in &g.buildings {
        assert!(
            b.tile.0 < MAP_SIZE as u8 && b.tile.1 < MAP_SIZE as u8,
            "building off the map"
        );
        assert!(b.hp > 0, "dead building still present");
    }

    // No two living units share a tile.
    let mut tiles: Vec<(u8, u8)> = g.units.iter().map(|u| u.tile).collect();
    tiles.sort_unstable();
    let n = tiles.len();
    tiles.dedup();
    assert_eq!(tiles.len(), n, "two units stack on one tile");

    // Ore never goes negative.
    assert!(g.ore[0] >= 0 && g.ore[1] >= 0, "negative ore");

    // Turn counter sane.
    assert!(g.turn >= 1, "turn below 1");
}

/// Drive `seed` for `TURNS` turns, issuing random command batches, and return
/// the final snapshot. Panics on any invariant violation (the fuzz's job).
fn fuzz_run(seed: u64) -> Vec<u8> {
    let mut rng = Rng::from_seed(seed);
    let mut g = Game::new(Map::generate(seed), config());
    while !g.is_over() && g.turn <= TURNS {
        let p = g.active;
        let batch_len = rng.below(6);
        let cmds: Vec<Command> = (0..batch_len)
            .map(|_| random_command(&mut rng, &g, p))
            .collect();
        for r in g.apply_commands(p, &cmds) {
            // Every result is Ok or a named rejection — never a panic.
            let _ = r;
        }
        check_invariants(&g);
        if g.is_over() {
            break;
        }
        // Guarantee progress even if the fuzzer never plays EndTurn.
        if g.active == p {
            let _ = g.apply_commands(p, &[Command::EndTurn { player: p }]);
        }
        check_invariants(&g);
    }
    crucible_sim::serialize::snapshot_bytes(&g)
}

#[test]
fn random_command_streams_never_crash_or_violate_state() {
    for seed in [1u64, 7, 42, 1337, 90_210] {
        fuzz_run(seed);
    }
}

#[test]
fn fuzz_is_deterministic() {
    for seed in [1u64, 7, 42, 1337, 90_210] {
        let a = fuzz_run(seed);
        let b = fuzz_run(seed);
        assert_eq!(a, b, "fuzz run diverged for seed {seed}");
    }
}

// Silence unused-import warnings for types referenced only in doc context.
#[allow(dead_code)]
fn _refs() {
    let _ = building_stats(BuildingType::Hq);
    let _ = CommandError::RateLimited;
}
