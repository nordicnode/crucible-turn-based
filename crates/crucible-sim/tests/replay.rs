//! Replay format test: record an input log, serialize it to JSON, reload it,
//! and reproduce the match byte-identically. This is the property every
//! stored match depends on.

use crucible_sim::{Command, Game, GameConfig, Map, Player, Replay, ReplayResult, UnitType};

/// A small deterministic script: both players build a base, produce units,
/// then skirmish toward the enemy HQ.
fn scripted_replay(seed: u64) -> Replay {
    let cfg = GameConfig {
        timeout_turns: 60,
        ..GameConfig::default()
    };
    let mut g = Game::new(Map::generate(seed), cfg.clone());
    let mut replay = Replay::new(seed, cfg);

    while !g.is_over() && g.turn < 40 {
        let p = g.active;
        for cmd in script_commands(&g, p) {
            replay.record(g.turn, p, cmd.clone());
        }
        let cmds = script_commands(&g, p);
        g.apply_commands(p, &cmds);
    }

    replay.result = Some(ReplayResult {
        winner: g.winner,
        reason: g.win_reason,
        duration_turns: g.turn,
        duration_rounds: g.round,
    });
    replay
}

fn script_commands(g: &Game, p: Player) -> Vec<Command> {
    let mut cmds = Vec::new();
    let hq = g.hq(p).map(|b| b.tile).unwrap_or((8, 8));

    // Opening build on turn 1.
    if g.turn == 1 {
        for (bt, (dx, dy)) in [
            (crucible_sim::BuildingType::Barracks, (1i32, 0)),
            (crucible_sim::BuildingType::Factory, (0, 1)),
        ] {
            let tile = ((hq.0 as i32 + dx) as u8, (hq.1 as i32 + dy) as u8);
            cmds.push(Command::PlaceBuilding {
                player: p,
                btype: bt,
                tile,
            });
        }
    }

    // Keep an infantry coming once the barracks exists.
    if let Some(barracks) = g
        .buildings
        .iter()
        .find(|b| b.owner == p && b.btype == crucible_sim::BuildingType::Barracks)
    {
        let infantry = g
            .units
            .iter()
            .filter(|u| u.owner == p && u.utype == UnitType::Infantry)
            .count();
        if infantry < 3 && barracks.queue.is_empty() {
            cmds.push(Command::TrainUnit {
                player: p,
                building: barracks.id,
                utype: UnitType::Infantry,
            });
        }
    }

    // March on the enemy HQ from turn 6.
    if g.turn >= 6 {
        let enemy_hq = g.hq(p.enemy()).map(|b| b.tile).unwrap_or((55, 55));
        let units: Vec<u32> = g
            .units
            .iter()
            .filter(|u| u.owner == p)
            .map(|u| u.id)
            .collect();
        if !units.is_empty() {
            cmds.push(Command::MoveGroup {
                player: p,
                units,
                waypoint: enemy_hq,
            });
        }
    }

    cmds.push(Command::EndTurn { player: p });
    cmds
}

#[test]
fn replay_reproduces_state_byte_identical() {
    let replay = scripted_replay(4242);
    let json = replay.to_json();
    let parsed = Replay::from_json(&json).expect("replay JSON round-trip");

    let original = crucible_sim::serialize::replay_to_game(&replay);
    let reproduced = crucible_sim::serialize::replay_to_game(&parsed);
    assert_eq!(
        crucible_sim::serialize::snapshot_bytes(&original),
        crucible_sim::serialize::snapshot_bytes(&reproduced),
        "replay did not reproduce byte-identically"
    );
}

#[test]
fn replay_is_small_input_log() {
    let replay = scripted_replay(4242);
    let json = replay.to_json();
    // A 40-turn scripted match must stay a few KB — it is an input log, not a
    // state dump.
    assert!(
        json.len() < 16 * 1024,
        "replay ballooned to {} bytes",
        json.len()
    );
}
