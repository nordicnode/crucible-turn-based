//! Feature-legality fuzz: the feature vector must derive only from the fogged
//! view + own state. Changing *hidden* enemy state must produce a zero delta.

use crucible_ai::{extract_single, FeatureInput};
use crucible_sim::{unit_stats, Game, GameConfig, Map, Player, Unit, UnitType};

fn make_game(seed: u64) -> Game {
    let mut g = Game::new(Map::generate(seed), GameConfig::default());
    // Advance the lifecycle a bit so fog memory/turns are non-trivial.
    for _ in 0..100 {
        g.end_turn();
    }
    g
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
        mp: stats.mp,
        moved: false,
        acted: false,
    });
}

#[test]
fn hidden_enemy_state_has_zero_feature_delta() {
    for seed in [3u64, 42, 999] {
        let base = make_game(seed);
        let mut tampered = base.clone();

        // Pick a tile that P0 cannot currently see.
        let view = base.fog_view(Player::P0);
        let hidden_idx = view
            .visible
            .iter()
            .position(|&v| !v)
            .expect("some tile must be hidden");
        let tile = ((hidden_idx % 64) as u8, (hidden_idx / 64) as u8);

        // Drop a hidden enemy army there.
        spawn_unit(&mut tampered, Player::P1, UnitType::Tank, tile);
        spawn_unit(&mut tampered, Player::P1, UnitType::Artillery, tile);

        let fa = extract_single(&FeatureInput::from_game(&base, Player::P0));
        let fb = extract_single(&FeatureInput::from_game(&tampered, Player::P0));
        assert_eq!(
            fa, fb,
            "hidden enemy state leaked into features (seed {seed})"
        );
    }
}

#[test]
fn visible_enemy_state_does_change_features() {
    let base = make_game(7);
    let mut revealed = base.clone();

    let view = base.fog_view(Player::P0);
    let visible_idx = view
        .visible
        .iter()
        .position(|&v| v)
        .expect("some tile must be visible");
    let tile = ((visible_idx % 64) as u8, (visible_idx / 64) as u8);
    spawn_unit(&mut revealed, Player::P1, UnitType::Tank, tile);

    let fa = extract_single(&FeatureInput::from_game(&base, Player::P0));
    let fb = extract_single(&FeatureInput::from_game(&revealed, Player::P0));
    assert_ne!(fa, fb, "features must respond to a visible enemy");
}
