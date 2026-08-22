//! wasm-bindgen shim exposing the deterministic sim to the browser client.
//! Used for local replay/spectate only — live matches are server-authoritative.

use wasm_bindgen::prelude::*;

use crucible_sim::serialize;
use crucible_sim::{Map, Replay};

/// Sim version string (also proves the sim crate linked into wasm).
#[wasm_bindgen]
pub fn sim_version() -> String {
    format!("crucible-sim {}", crucible_sim::VERSION)
}

/// Generate a map from a seed and return its symmetric HQ tiles as JSON.
#[wasm_bindgen]
pub fn map_hq_json(seed: u64) -> String {
    let map = crucible_sim::Map::generate(seed);
    serde_json::to_string(&map.hq_tiles).expect("infallible")
}

/// Build a JS-side error value. On wasm this is a real string; native test
/// builds have no JS runtime to allocate one, so they fall back to `undefined`
/// (the Ok path is what the native tests exercise).
fn js_err(msg: String) -> JsValue {
    #[cfg(target_arch = "wasm32")]
    {
        JsValue::from_str(&msg)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = msg;
        JsValue::UNDEFINED
    }
}

/// Parse a replay from browser-supplied JSON. Malformed or legacy-format
/// replays return an `Err` instead of panicking — a panic in wasm is fatal to
/// the whole page, and spectate should degrade to a visible error instead.
fn parse_replay(replay_json: &str) -> Result<Replay, JsValue> {
    serde_json::from_str(replay_json).map_err(|e| js_err(format!("invalid replay JSON: {e}")))
}

/// Re-run a replay to a given turn and return the game snapshot as JSON.
/// Deterministic: identical input produces byte-identical output on native
/// and wasm. Supports seeking to any turn (forward or backward).
#[wasm_bindgen]
pub fn replay_snapshot_json(replay_json: &str, turn: i32) -> Result<String, JsValue> {
    let replay = parse_replay(replay_json)?;
    let game = replay_to_turn(&replay, turn);
    Ok(serialize::snapshot_json(&game))
}

/// Re-run a replay's command log up to (and including) `turn`, returning the
/// resulting game. Commands execute immediately in log order; `EndTurn`
/// entries drive the lifecycle.
fn replay_to_turn(replay: &Replay, turn: i32) -> crucible_sim::Game {
    let mut game = crucible_sim::Game::new(Map::generate(replay.map_seed), replay.config.clone());
    for tc in &replay.commands {
        if tc.turn > turn || game.is_over() {
            break;
        }
        // Advance the lifecycle to the command's turn before applying it.
        while game.turn < tc.turn && !game.is_over() {
            game.end_turn();
        }
        if game.is_over() {
            break;
        }
        game.apply_commands(tc.player, std::slice::from_ref(&tc.command));
    }
    // Seek onward to `turn` even past the last recorded command.
    while !game.is_over() && game.turn <= turn {
        game.end_turn();
    }
    game
}

/// Static replay metadata for the spectate screen: the map (passability, HQ
/// spawns, initial generic resource layout) plus the recorded outcome. Called once per
/// replay; the per-frame payload in [`replay_frame`] stays lean.
#[wasm_bindgen]
pub fn replay_meta(replay_json: &str) -> Result<String, JsValue> {
    let replay = parse_replay(replay_json)?;
    let map = Map::generate(replay.map_seed);
    let duration = replay
        .result
        .as_ref()
        .map(|r| r.duration_turns)
        .unwrap_or(replay.config.timeout_turns);
    let duration_rounds = replay
        .result
        .as_ref()
        .map(|r| {
            if r.duration_rounds > 0 {
                r.duration_rounds
            } else {
                (r.duration_turns + 1).div_euclid(2).max(1)
            }
        })
        .unwrap_or((duration + 1).div_euclid(2).max(1));
    Ok(serde_json::json!({
        "map_seed": replay.map_seed,
        "passable": map.passable,
        "terrain": map.terrain,
        "elevation": map.elevation,
        "moisture": map.moisture,
        "temperature": map.temperature,
        "terrain_rules": crucible_sim::map::Terrain::ALL
            .into_iter()
            .map(|terrain| serde_json::json!({
                "kind": format!("{terrain:?}"),
                "label": terrain.label(),
                "passable": terrain.is_passable(),
                "move_multiplier": terrain.move_mult(),
                "defense_reduction": terrain.defense_reduction(),
                "tactical_tag": terrain.tactical_tag(),
            }))
            .collect::<Vec<_>>(),
        "hq_tiles": map.hq_tiles,
        "ore": map.ore,
        "steel": map.steel,
        "coal": map.coal,
        "crystal": map.crystal,
        "resource_kind": map.resource_kind,
        "richness": map.richness,
        "duration_turns": duration,
        "duration_rounds": duration_rounds,
        "winner": replay.result.as_ref().and_then(|r| r.winner.map(|p| p.index() as u8)),
        "win_reason": replay.result.as_ref().and_then(|r| r.reason),
    })
    .to_string())
}

/// One lean spectate frame: both players' entities (full state, no fog),
/// resource wallets, and scores at a given turn. `kind` strings use the serde variant names
/// (`"Infantry"`, `"Hq"`, …) to match the live match protocol.
#[wasm_bindgen]
pub fn replay_frame(replay_json: &str, turn: i32) -> Result<String, JsValue> {
    let replay = parse_replay(replay_json)?;
    let game = replay_to_turn(&replay, turn);
    let units: Vec<serde_json::Value> = game
        .units
        .iter()
        .map(|u| {
            serde_json::json!({
                "id": u.id,
                "kind": u.utype,
                "owner": u.owner,
                "x": u.tile.0 as f32 + 0.5,
                "y": u.tile.1 as f32 + 0.5,
                "hp": u.hp,
                "max_hp": u.max_hp,
                "mp": u.mp,
                "max_mp": crucible_sim::unit_stats(u.utype).mp,
                "move_target": u.move_target,
                "movement_path": game.movement_path(u.id),
                "moved": u.moved,
                "acted": u.acted,
            })
        })
        .collect();
    let buildings: Vec<serde_json::Value> = game
        .buildings
        .iter()
        .map(|b| {
            serde_json::json!({
                "id": b.id,
                "kind": b.btype,
                "owner": b.owner,
                "x": b.tile.0 as f32 + 0.5,
                "y": b.tile.1 as f32 + 0.5,
                "hp": b.hp,
                "max_hp": b.max_hp,
                "queue": b.queue,
                "progress": if b.queue.is_empty() { serde_json::Value::Null } else { serde_json::json!(b.progress) },
                "build_time": b.queue.first().map(|u| crucible_sim::unit_stats(*u).build_time_turns),
                "construction_progress": b.construction_progress,
                "construction_time": b.construction_time(),
            })
        })
        .collect();
    let (p0_prod, p0_cons) = game.power(crucible_sim::Player::P0);
    let (p1_prod, p1_cons) = game.power(crucible_sim::Player::P1);
    Ok(serde_json::json!({
        "turn": game.turn,
        "round": game.round,
        "active": game.active.index() as u8,
        "ore0": game.ore[0],
        "ore1": game.ore[1],
        "steel0": game.steel[0],
        "steel1": game.steel[1],
        "coal0": game.coal[0],
        "coal1": game.coal[1],
        "crystal0": game.crystal[0],
        "crystal1": game.crystal[1],
        "resources0": game.resources(crucible_sim::Player::P0),
        "resources1": game.resources(crucible_sim::Player::P1),
        "income0": game.resource_income(crucible_sim::Player::P0),
        "income1": game.resource_income(crucible_sim::Player::P1),
        "power0": [p0_prod, p0_cons],
        "power1": [p1_prod, p1_cons],
        "units": units,
        "buildings": buildings,
        "winner": game.winner.map(|p| p.index() as u8),
        "win_reason": game.win_reason,
    })
    .to_string())
}

/// Re-run a replay to completion and return the result plus a deterministic
/// snapshot hash (FNV-1a over the serialized final state). The hash lets the
/// browser verify native/wasm parity byte-for-byte.
#[wasm_bindgen]
pub fn replay_result(replay_json: &str) -> Result<String, JsValue> {
    let replay = parse_replay(replay_json)?;
    let game = replay_to_turn(&replay, i32::MAX);
    let hash = fnv1a(&serialize::snapshot_bytes(&game));
    Ok(serde_json::json!({
        "reason": game.win_reason,
        "duration_turns": game.turn,
        "duration_rounds": game.round,
        "hash": hash,
    })
    .to_string())
}

fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fnv1a_native(data: &[u8]) -> u64 {
        fnv1a(data)
    }

    #[test]
    fn replay_result_matches_direct_run() {
        let seed = 7u64;
        let cfg = crucible_sim::GameConfig {
            timeout_turns: 30,
            ..crucible_sim::GameConfig::default()
        };
        let replay = Replay::new(seed, cfg.clone());
        let result: serde_json::Value =
            serde_json::from_str(&replay_result(&replay.to_json()).unwrap()).unwrap();

        let mut game = crucible_sim::Game::new(crucible_sim::Map::generate(seed), cfg);
        while !game.is_over() {
            game.end_turn();
        }
        assert_eq!(
            result["hash"].as_u64().unwrap(),
            fnv1a_native(&serialize::snapshot_bytes(&game))
        );
        assert_eq!(result["duration_turns"].as_i64().unwrap() as i32, game.turn);
        assert_eq!(
            result["duration_rounds"].as_i64().unwrap() as i32,
            game.round
        );
    }

    #[test]
    fn snapshot_at_turn_matches_direct_playout() {
        let seed = 9u64;
        let cfg = crucible_sim::GameConfig {
            timeout_turns: 10_000,
            ..crucible_sim::GameConfig::default()
        };
        let replay = Replay::new(seed, cfg.clone());
        let snap: serde_json::Value =
            serde_json::from_str(&replay_snapshot_json(&replay.to_json(), 25).unwrap()).unwrap();

        let mut game = crucible_sim::Game::new(crucible_sim::Map::generate(seed), cfg);
        while game.turn <= 25 && !game.is_over() {
            game.end_turn();
        }
        let direct: serde_json::Value =
            serde_json::from_str(&serialize::snapshot_json(&game)).unwrap();
        assert_eq!(snap, direct);
    }

    #[test]
    fn replay_frame_and_meta_match_snapshot() {
        use crucible_sim::{BuildingType, Command, GameConfig, Player};

        let seed = 21u64;
        let cfg = GameConfig {
            timeout_turns: 50,
            ..GameConfig::default()
        };
        let mut replay = Replay::new(seed, cfg.clone());
        // A generic refinery claims the nearest live deposit tile itself.
        let map = crucible_sim::Map::generate(seed);
        let hq = map.hq_tiles[0];
        let place = (0..crucible_sim::map::MAP_TILES)
            .map(crucible_sim::map::tile_coords)
            .filter(|&t| map.resource_amount_at(t.0, t.1) > 0)
            .min_by_key(|&t| {
                (
                    (t.0 as i32 - hq.0 as i32).abs() + (t.1 as i32 - hq.1 as i32).abs(),
                    crucible_sim::map::tile_index(t.0, t.1),
                )
            })
            .expect("map has a resource tile");
        replay.record(
            1,
            Player::P0,
            Command::PlaceBuilding {
                player: Player::P0,
                btype: BuildingType::Refinery,
                tile: place,
            },
        );
        replay.record(1, Player::P0, Command::EndTurn { player: Player::P0 });
        replay.record(2, Player::P1, Command::EndTurn { player: Player::P1 });
        let rj = replay.to_json();

        let meta: serde_json::Value = serde_json::from_str(&replay_meta(&rj).unwrap()).unwrap();
        assert_eq!(meta["map_seed"].as_u64().unwrap(), seed);
        assert_eq!(
            meta["passable"].as_array().unwrap().len(),
            crucible_sim::map::MAP_TILES
        );

        let snap: serde_json::Value =
            serde_json::from_str(&replay_snapshot_json(&rj, 5).unwrap()).unwrap();
        let frame: serde_json::Value =
            serde_json::from_str(&replay_frame(&rj, 5).unwrap()).unwrap();
        // Seeking to turn 5 runs the lifecycle through turn 5, then one more
        // end_turn hands off (turn 6) before the loop's `<= turn` check exits.
        assert_eq!(frame["turn"].as_i64().unwrap(), 6);
        assert_eq!(frame["round"].as_i64().unwrap(), 3);
        assert_eq!(
            snap["units"].as_array().unwrap().len(),
            frame["units"].as_array().unwrap().len()
        );
        assert_eq!(
            snap["buildings"].as_array().unwrap().len(),
            frame["buildings"].as_array().unwrap().len()
        );
        // Kind strings are serde variant names (capitalized) — what the client
        // renderer expects.
        let kinds: Vec<&str> = frame["buildings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["kind"].as_str().unwrap())
            .collect();
        assert!(kinds.contains(&"Hq") && kinds.contains(&"Refinery"));
    }

    #[test]
    fn malformed_replay_returns_error_not_panic() {
        // Browser-supplied replay data must never panic the wasm module: a
        // panic is fatal to the page, while an Err is a catchable JS error.
        assert!(replay_meta("{not json").is_err());
        assert!(replay_frame("", 0).is_err());
        assert!(replay_result("null").is_err());
        assert!(replay_snapshot_json("[]", 10).is_err());
    }
}
