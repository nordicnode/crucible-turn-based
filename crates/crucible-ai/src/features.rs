//! Fog-legal feature extraction: the commander's only observation of the
//! world.
//!
//! [`FeatureInput`] carries the player's *own* full state plus a
//! [`FogView`] for the enemy. It never contains the live state of hidden enemy
//! entities — that is enforced by construction, and a fuzz test asserts it.

use crucible_sim::{
    fog::FogView, map::MAP_TILES, unit_stats, BuildingType, Game, Player, UnitType, Upgrade,
    FOG_MEMORY_TURNS, HQ_INCOME_PER_TURN, MATCH_TIMEOUT_TURNS, REFINERY_ORE_PER_TURN,
};

/// Time window (turns) over which remembered enemy sightings decay to zero.
/// Matches the sim's fog memory horizon: a sighting is fully faded exactly
/// when the engine forgets it.
pub const DECAY_TURNS: i32 = 6;

/// How many turns of observations are stacked into the network input (plan
/// §5.2 history embedding): the current features plus the previous
/// `HISTORY_TICKS - 1` turns', so the network can read trends instead of only
/// the instantaneous state. Implemented as last-K features (plan §12
/// alternative) to keep the MLP feed-forward. The name keeps its historical
/// `TICKS` spelling so downstream shape code stays stable.
pub const HISTORY_TICKS: usize = 2;

/// One turn's worth of features (fog-legal, see the layout below).
pub const SINGLE_FEATURE_DIM: usize = 112;

/// The stacked network input dimension: `HISTORY_TICKS × SINGLE_FEATURE_DIM`.
pub const FEATURE_DIM: usize = SINGLE_FEATURE_DIM * HISTORY_TICKS;

/// An own building as seen by the feature extractor.
#[derive(Clone, Debug)]
pub struct OwnBuilding {
    pub btype: BuildingType,
    pub queue_len: usize,
    pub hp: i32,
    pub max_hp: i32,
}

/// The commander's legal observation. Own state is complete; enemy state comes
/// only through [`FogView`].
#[derive(Clone, Debug)]
pub struct FeatureInput {
    /// Current turn number (starts at 1).
    pub turn: i32,
    pub ore: i32,
    pub upgrade: Upgrade,
    pub own_units: Vec<UnitType>,
    pub own_buildings: Vec<OwnBuilding>,
    pub own_hq_tile: (u8, u8),
    pub fog: FogView,
}

impl FeatureInput {
    /// Build the observation for `player` from the current game.
    pub fn from_game(game: &Game, player: Player) -> Self {
        let own_units = game
            .units
            .iter()
            .filter(|u| u.owner == player && u.is_alive())
            .map(|u| u.utype)
            .collect();
        let own_buildings = game
            .buildings
            .iter()
            .filter(|b| b.owner == player && b.is_alive())
            .map(|b| OwnBuilding {
                btype: b.btype,
                queue_len: b.queue.len(),
                hp: b.hp,
                max_hp: b.max_hp,
            })
            .collect();
        let own_hq_tile = game
            .hq(player)
            .map(|b| b.tile)
            .unwrap_or(game.map.hq_tiles[player.index()]);
        FeatureInput {
            turn: game.turn,
            ore: game.ore[player.index()],
            upgrade: game.upgrades[player.index()],
            own_units,
            own_buildings,
            own_hq_tile,
            fog: game.fog_view(player),
        }
    }
}

/// Decay weight for a sighting at `last_seen` given the current turn, in [0,1].
#[inline]
fn decay(turn: i32, last_seen: i32) -> f32 {
    let age = (turn - last_seen).max(0);
    (1.0 - age as f32 / DECAY_TURNS as f32).clamp(0.0, 1.0)
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

fn count_own(input: &FeatureInput, ut: UnitType) -> usize {
    input.own_units.iter().filter(|&&u| u == ut).count()
}

fn count_buildings(input: &FeatureInput, bt: BuildingType) -> usize {
    input.own_buildings.iter().filter(|b| b.btype == bt).count()
}

/// Extract the fixed-size feature vector.
///
/// Layout (documented so `network.rs` and `decision.rs` stay in sync):
/// ```text
/// [0]     banked ore / 2000
/// [1..6]  own building counts: refinery/4, factory/4, barracks/4, techlab/2, turret/8
/// [6]     own infantry / 16
/// [7]     own tanks / 16
/// [8]     own artillery / 16
/// [9]     own mammoth tanks / 6
/// [10]    own army value / 2000
/// [11]    observed enemy infantry (decayed) / 16
/// [12]    observed enemy tanks (decayed) / 16
/// [13]    observed enemy artillery (decayed) / 16
/// [14]    observed enemy mammoth tanks (decayed) / 6
/// [15]    observed enemy aircraft (decayed) / 8
/// [16..22] observed enemy buildings (decayed): hq, refinery+powerplant,
///         factory, barracks, techlab+airfield, turret, radar+coil — each /4
///         except hq /1
/// [22..86] enemy building presence per 8x8 sector (64), decayed, capped
/// [86]    unexplored fraction (1 - explored tiles / total)
/// [87]    own airfields / 2
/// [88]    turn / timeout (MATCH_TIMEOUT_TURNS)
/// [89]    own HQ HP fraction
/// [90]    factory queue depth / 8
/// [91]    barracks queue depth / 8
/// [92]    idle production buildings / 8
/// [93..96] upgrade one-hot: none, damage, hp, range
/// [97]    own gunships / 8
/// [98]    own interceptors / 8
/// [99]    own radar / 2
/// [100]   own tesla coils / 4
/// [101]   income estimate per turn: (refineries*REFINERY_ORE_PER_TURN +
///         HQ_INCOME_PER_TURN) / 1000
/// [102]   observed enemy radar (decayed) / 2
/// [103]   observed enemy tesla coils (decayed) / 4
/// [104]   observed enemy airfields (decayed) / 2
/// [105..112] reserved (0)
/// ```
pub fn extract_single(input: &FeatureInput) -> Vec<f32> {
    let mut f = vec![0.0f32; SINGLE_FEATURE_DIM];
    let turn = input.turn;

    f[0] = clamp01(input.ore as f32 / 2000.0);

    f[1] = clamp01(count_buildings(input, BuildingType::Refinery) as f32 / 4.0);
    f[2] = clamp01(count_buildings(input, BuildingType::Factory) as f32 / 4.0);
    f[3] = clamp01(count_buildings(input, BuildingType::Barracks) as f32 / 4.0);
    f[4] = clamp01(count_buildings(input, BuildingType::TechLab) as f32 / 2.0);
    f[5] = clamp01(count_buildings(input, BuildingType::Turret) as f32 / 8.0);

    // Own unit counts by type (all six types, no harvester any more).
    f[6] = clamp01(count_own(input, UnitType::Infantry) as f32 / 16.0);
    f[7] = clamp01(count_own(input, UnitType::Tank) as f32 / 16.0);
    f[8] = clamp01(count_own(input, UnitType::Artillery) as f32 / 16.0);
    f[9] = clamp01(count_own(input, UnitType::MammothTank) as f32 / 6.0);
    f[97] = clamp01(count_own(input, UnitType::Gunship) as f32 / 8.0);
    f[98] = clamp01(count_own(input, UnitType::Interceptor) as f32 / 8.0);

    // Second-tier presence: radar dishes and tesla coils.
    f[99] = clamp01(count_buildings(input, BuildingType::Radar) as f32 / 2.0);
    f[100] = clamp01(count_buildings(input, BuildingType::TeslaCoil) as f32 / 4.0);

    let army_value: i32 = input.own_units.iter().map(|&u| unit_stats(u).cost).sum();
    f[10] = clamp01(army_value as f32 / 2000.0);

    // Observed enemy units (fog-decayed by turns since last seen).
    for m in &input.fog.units {
        let w = decay(turn, m.last_seen);
        match m.utype {
            UnitType::Infantry => f[11] = clamp01(f[11] + w / 16.0),
            UnitType::Tank => f[12] = clamp01(f[12] + w / 16.0),
            UnitType::Artillery => f[13] = clamp01(f[13] + w / 16.0),
            UnitType::MammothTank => f[14] = clamp01(f[14] + w / 6.0),
            UnitType::Gunship | UnitType::Interceptor => f[15] = clamp01(f[15] + w / 8.0),
        }
    }

    // Observed enemy buildings (decayed).
    for m in &input.fog.buildings {
        let w = decay(turn, m.last_seen);
        let idx = match m.btype {
            BuildingType::Hq => 16,
            BuildingType::Refinery | BuildingType::PowerPlant => 17,
            BuildingType::Factory => 18,
            BuildingType::Barracks => 19,
            BuildingType::TechLab | BuildingType::Airfield => 20,
            BuildingType::Turret => 21,
            BuildingType::Radar | BuildingType::TeslaCoil => 22,
        };
        f[idx] = clamp01(f[idx] + if idx == 16 { w } else { w / 4.0 });
    }

    // Enemy building presence per 8x8 sector (oriented relative to own HQ corner).
    let (flip_x, flip_y) = (input.own_hq_tile.0 >= 32, input.own_hq_tile.1 >= 32);
    for m in &input.fog.buildings {
        let mut sx = (m.tile.0 as usize) / 8;
        let mut sy = (m.tile.1 as usize) / 8;
        if flip_x {
            sx = 7 - sx;
        }
        if flip_y {
            sy = 7 - sy;
        }
        let w = decay(turn, m.last_seen);
        f[23 + sy * 8 + sx] = clamp01(f[23 + sy * 8 + sx] + w);
    }

    // Unexplored fraction: the fog memory tracks every tile ever seen, so the
    // commander knows how much of the battlefield remains to be scouted.
    let explored = input.fog.explored.iter().filter(|&&e| e).count();
    f[86] = 1.0 - explored as f32 / MAP_TILES as f32;

    // Own airfield count (the network can see when air power is available).
    f[87] = clamp01(count_buildings(input, BuildingType::Airfield) as f32 / 2.0);

    f[88] = clamp01(turn as f32 / MATCH_TIMEOUT_TURNS as f32);

    // Own HQ HP fraction.
    let hq = input
        .own_buildings
        .iter()
        .find(|b| b.btype == BuildingType::Hq);
    f[89] = hq
        .map(|b| clamp01(b.hp as f32 / b.max_hp as f32))
        .unwrap_or(0.0);

    // Production queues and idle buildings.
    let mut factory_q = 0usize;
    let mut barracks_q = 0usize;
    let mut idle = 0usize;
    for b in &input.own_buildings {
        match b.btype {
            BuildingType::Factory => factory_q += b.queue_len,
            BuildingType::Barracks => barracks_q += b.queue_len,
            _ => {}
        }
        if matches!(
            b.btype,
            BuildingType::Factory | BuildingType::Barracks | BuildingType::Airfield
        ) && b.queue_len == 0
        {
            idle += 1;
        }
    }
    f[90] = clamp01(factory_q as f32 / 8.0);
    f[91] = clamp01(barracks_q as f32 / 8.0);
    f[92] = clamp01(idle as f32 / 8.0);

    // Upgrade one-hot.
    match input.upgrade {
        Upgrade::None => f[93] = 1.0,
        Upgrade::Damage => f[94] = 1.0,
        Upgrade::Hp => f[95] = 1.0,
        Upgrade::Range => f[96] = 1.0,
    }

    // Income estimate under passive economy: each refinery drains up to
    // REFINERY_ORE_PER_TURN from adjacent fields, the HQ trickles a constant.
    let income = count_buildings(input, BuildingType::Refinery) as i32 * REFINERY_ORE_PER_TURN
        + HQ_INCOME_PER_TURN;
    f[101] = clamp01(income as f32 / 1000.0);

    // Observed enemy second-tier structures (decayed), separately normalized.
    for m in &input.fog.buildings {
        let w = decay(turn, m.last_seen);
        match m.btype {
            BuildingType::Radar => f[102] = clamp01(f[102] + w / 2.0),
            BuildingType::TeslaCoil => f[103] = clamp01(f[103] + w / 4.0),
            BuildingType::Airfield => f[104] = clamp01(f[104] + w / 2.0),
            _ => {}
        }
    }

    f
}

/// Stack the history embedding: the previous turns' single-turn vectors
/// (oldest first, at most `HISTORY_TICKS - 1` of them) followed by the
/// current one. A missing history is zero-padded, so the first turns of a
/// match still produce a full-size input — the network can learn that an
/// all-zero previous observation means "start of match".
///
/// The history is itself fog-legal by construction: it is derived from
/// previous [`FeatureInput`]s, so stacking it cannot leak hidden state.
pub fn extract(input: &FeatureInput, history: &[Vec<f32>]) -> Vec<f32> {
    let mut f: Vec<f32> = Vec::with_capacity(FEATURE_DIM);
    for prev in history.iter().take(HISTORY_TICKS - 1) {
        f.extend_from_slice(prev);
    }
    let prev_dim = (HISTORY_TICKS - 1) * SINGLE_FEATURE_DIM;
    f.resize(prev_dim, 0.0);
    f.extend(extract_single(input));
    f
}

// Keep the fog-memory horizon import honest: DECAY_TURNS must equal it or the
// fade-out and the engine's forgetting disagree.
const _: () = assert!(DECAY_TURNS == FOG_MEMORY_TURNS);

#[cfg(test)]
mod tests {
    use super::*;
    use crucible_sim::{Game, GameConfig, Map};

    #[test]
    fn feature_dim_is_stable() {
        let g = Game::new(Map::generate(1), GameConfig::default());
        let input = FeatureInput::from_game(&g, Player::P0);
        let f = extract(&input, &[]);
        assert_eq!(f.len(), FEATURE_DIM);
        assert_eq!(f.len(), 224);
        assert_eq!(extract_single(&input).len(), 112);
    }

    #[test]
    fn history_stacks_previous_observations() {
        let g = Game::new(Map::generate(1), GameConfig::default());
        let input = FeatureInput::from_game(&g, Player::P0);
        let single = extract_single(&input);

        // No history: the previous slots are zero-padded.
        let no_hist = extract(&input, &[]);
        assert_eq!(no_hist.len(), 224);
        assert!(no_hist[..112].iter().all(|&v| v == 0.0));
        assert_eq!(&no_hist[112..], &single[..]);

        // One previous turn: it fills the first 112 slots verbatim.
        let hist = extract(&input, std::slice::from_ref(&single));
        assert_eq!(&hist[..112], &single[..]);
        assert_eq!(&hist[112..], &single[..]);

        // A longer history is trimmed to the oldest-first window.
        let two = vec![vec![1.0f32; 112], single.clone()];
        let trimmed = extract(&input, &two);
        assert_eq!(&trimmed[..112], &two[0][..]);
        assert_eq!(&trimmed[112..], &single[..]);
    }

    #[test]
    fn features_are_bounded() {
        let g = Game::new(Map::generate(1), GameConfig::default());
        let f = extract(&FeatureInput::from_game(&g, Player::P0), &[]);
        for v in &f {
            assert!(v.is_finite());
            assert!((0.0..=1.0).contains(v));
        }
    }

    #[test]
    fn decay_fades_over_six_turns() {
        assert_eq!(decay(10, 10), 1.0);
        assert_eq!(decay(13, 10), 0.5);
        assert_eq!(decay(16, 10), 0.0);
        assert_eq!(decay(20, 10), 0.0);
    }

    #[test]
    fn income_estimate_reflects_refineries() {
        let g = Game::new(Map::generate(1), GameConfig::default());
        let input = FeatureInput::from_game(&g, Player::P0);
        let f = extract_single(&input);
        // No refinery yet: HQ trickle only.
        assert_eq!(f[101], HQ_INCOME_PER_TURN as f32 / 1000.0);
    }
}
