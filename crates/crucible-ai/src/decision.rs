//! The decision layer: network output scores -> concrete, valid commands.
//!
//! Illegal actions are masked (skipped) before argmax, and any action whose
//! winning score is at/below the threshold is skipped. Ties break to the
//! lowest index, so the mapping is fully deterministic given genome + state.
//! The returned commands still pass through the sim's normal validator.
//!
//! The caller appends [`Command::EndTurn`] (see `bot.rs` for the convention);
//! this layer only produces the action commands.

use crucible_sim::{
    tech::{prereqs_met, tech_info, TechId},
    tiles::chebyshev,
    unit_stats, BuildingType, Command, EntityId, Game, Player, UnitType,
};

use crate::features::{extract, FeatureInput};
use crate::network::{
    forward, ARMY_ACTION_OUT, BUILD_OUT, SECTOR_OUT, SNIPE_OUT, TECH_OUT, TRAIN_OUT,
};
use crate::scripted::{combat_unit_ids, find_build_tile};

const BUILD_TYPES: [BuildingType; BUILD_OUT] = [
    BuildingType::Refinery,
    BuildingType::Barracks,
    BuildingType::Factory,
    BuildingType::TechLab,
    BuildingType::Turret,
    BuildingType::Airfield,
    BuildingType::Radar,
    BuildingType::TeslaCoil,
];
const TRAIN_TYPES: [UnitType; TRAIN_OUT] = [
    UnitType::Infantry,
    UnitType::Tank,
    UnitType::Artillery,
    UnitType::MammothTank,
    UnitType::Gunship,
    UnitType::Interceptor,
];
/// The four research actions the tech head can pick (schema v7: the old
/// damage/hp/range upgrades became technologies). The network scores them;
/// the decision layer masks the illegal ones.
const TECH_TYPES: [TechId; TECH_OUT] = [
    TechId::HighExplosive,
    TechId::CompositeArmor,
    TechId::TargetingOptics,
    TechId::EfficientRefining,
];

/// Actions only fire when their winning score clears this threshold.
const THRESHOLD: f32 = 0.0;

/// Whether the army head should act at all this turn.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ArmyAction {
    Attack,
    Defend,
    Scout,
    /// Focus-fire the highest-priority visible enemy of the snipe-target type
    /// chosen by the snipe head (the `Attack` command, not an attack-move).
    Snipe,
}

/// Snipe-target types, in output-slot order (see `SNIPE_OUT`). The network
/// scores each; the decision layer then picks the currently-visible enemy of
/// the winning type that is closest to the army.
const SNIPE_TYPES: [SnipeTarget; SNIPE_OUT] = [
    SnipeTarget::Unit(UnitType::Tank),
    SnipeTarget::Building(BuildingType::Refinery),
    SnipeTarget::Building(BuildingType::Hq),
    SnipeTarget::Building(BuildingType::Factory),
];

#[derive(Clone, Copy)]
enum SnipeTarget {
    Unit(UnitType),
    Building(BuildingType),
}

/// Decide commands for `player` from a genome, a legal observation, and the
/// history embedding (the previous turns' feature vectors, oldest first; see
/// `features::extract`). Callers own the buffer — [`GenomeBot`](crate::GenomeBot)
/// maintains it across turns.
pub fn decide(
    game: &Game,
    player: Player,
    genome: &[f32],
    input: &FeatureInput,
    history: &[Vec<f32>],
) -> Vec<Command> {
    let feats = extract(input, history);
    let out = forward(genome, &feats);
    let mut cmds = Vec::new();

    // --- Build head ---------------------------------------------------------
    let build_base = 0;
    let mut best_build: Option<(f32, usize)> = None;
    for i in 0..BUILD_OUT {
        let btype = BUILD_TYPES[i];
        if build_allowed(game, player, btype) {
            let s = out[build_base + i];
            if s > THRESHOLD && best_build.is_none_or(|(bs, _)| s > bs) {
                best_build = Some((s, i));
            }
        }
    }
    if let Some((_, i)) = best_build {
        let btype = BUILD_TYPES[i];
        if let Some(pref) = build_preferred(game, player, btype) {
            // An empty plan: this is the batch's first placement.
            if let Some(tile) = find_build_tile(game, player, btype, pref, &Default::default()) {
                cmds.push(Command::PlaceBuilding {
                    player,
                    btype,
                    tile,
                });
            }
        }
    }

    // --- Power management (rule, not learned) ------------------------------
    // The learned build head has no PowerPlant slot, so a deterministic rule
    // keeps the AI from being permanently crippled by low power: production
    // runs at half speed once consumption exceeds production, and humans can
    // escape that by building a PowerPlant. The AI gets the same escape hatch.
    // Fires only when one more plant closes the gap, so it never over-spams.
    if game.has_low_power(player) {
        let (prod, cons) = game.power(player);
        if prod + crucible_sim::building_stats(BuildingType::PowerPlant).power >= cons
            && build_allowed(game, player, BuildingType::PowerPlant)
        {
            if let Some(pref) = build_preferred(game, player, BuildingType::PowerPlant) {
                if let Some(tile) = find_build_tile(
                    game,
                    player,
                    BuildingType::PowerPlant,
                    pref,
                    &Default::default(),
                ) {
                    cmds.push(Command::PlaceBuilding {
                        player,
                        btype: BuildingType::PowerPlant,
                        tile,
                    });
                }
            }
        }
    }

    // --- Train head ---------------------------------------------------------
    let train_base = BUILD_OUT;
    let mut best_train: Option<(f32, usize)> = None;
    for i in 0..TRAIN_OUT {
        let utype = TRAIN_TYPES[i];
        if train_allowed(game, player, utype) {
            let s = out[train_base + i];
            if s > THRESHOLD && best_train.is_none_or(|(bs, _)| s > bs) {
                best_train = Some((s, i));
            }
        }
    }
    if let Some((_, i)) = best_train {
        let utype = TRAIN_TYPES[i];
        let producer = producer_for(utype);
        if let Some(b) = game.buildings.iter().find(|b| {
            b.owner == player && b.btype == producer && b.queue.len() < game.config.max_queue
        }) {
            cmds.push(Command::TrainUnit {
                player,
                building: b.id,
                utype,
            });
        }
    }

    // --- Army head ----------------------------------------------------------
    let action_base = BUILD_OUT + TRAIN_OUT;
    let sector_base = action_base + ARMY_ACTION_OUT;
    let mut action = None;
    let mut action_score = THRESHOLD;
    for i in 0..ARMY_ACTION_OUT {
        if out[action_base + i] > action_score {
            action_score = out[action_base + i];
            action = Some(match i {
                0 => ArmyAction::Attack,
                1 => ArmyAction::Defend,
                2 => ArmyAction::Scout,
                _ => ArmyAction::Snipe,
            });
        }
    }
    let sector = argmax(&out[sector_base..sector_base + SECTOR_OUT]);

    if let Some(action) = action {
        let units = combat_unit_ids(game, player);
        if !units.is_empty() {
            let snipe_base = sector_base + SECTOR_OUT + TECH_OUT;
            match action {
                // Focus-fire: pick the target type the snipe head scores highest
                // and lock the army onto the best currently-visible enemy of
                // that type. If none is visible right now, the snipe skips
                // (the features encode visibility, so the network learns when
                // a snipe can actually land).
                ArmyAction::Snipe => {
                    let kind = SNIPE_TYPES[argmax(&out[snipe_base..snipe_base + SNIPE_OUT])];
                    if let Some(target) = snipe_target(game, player, input, kind) {
                        cmds.push(Command::Attack {
                            player,
                            units,
                            target,
                        });
                    }
                }
                _ => {
                    let waypoint = match action {
                        ArmyAction::Defend => input.own_hq_tile,
                        ArmyAction::Scout => sector_center(sector, input.own_hq_tile),
                        ArmyAction::Attack => {
                            if units.len() >= 3 {
                                (63 - input.own_hq_tile.0, 63 - input.own_hq_tile.1)
                            } else {
                                input.own_hq_tile
                            }
                        }
                        ArmyAction::Snipe => unreachable!(),
                    };
                    // Shared scripted micro: advance toward the waypoint, then
                    // focus-fire the nearest hittable enemy (units never
                    // auto-attack in the turn model, so the attack must be
                    // ordered). The network still decides *where* the army
                    // goes; the fighting itself is deterministic script.
                    cmds.extend(crate::scripted::army_orders(game, player, waypoint));
                }
            }
        }
    }

    // --- Tech head ----------------------------------------------------------
    let tech_base = sector_base + SECTOR_OUT;
    let mut best_tech: Option<(f32, TechId)> = None;
    for i in 0..TECH_OUT {
        let tech = TECH_TYPES[i];
        if tech_allowed(game, player, tech) {
            let s = out[tech_base + i];
            if s > THRESHOLD && best_tech.is_none_or(|(bs, _)| s > bs) {
                best_tech = Some((s, tech));
            }
        }
    }
    if let Some((_, tech)) = best_tech {
        cmds.push(Command::StartResearch { player, tech });
    }

    cmds
}

fn argmax(xs: &[f32]) -> usize {
    let mut best = 0;
    for (i, &v) in xs.iter().enumerate() {
        if v > xs[best] {
            best = i;
        }
    }
    best
}

/// The best currently-visible enemy of `kind` for a focus-fire order: the one
/// closest to the army's centroid (tie → lowest id). Only enemies seen this
/// exact turn are eligible — `last_seen == turn` is the fog invariant that
/// the target is alive and enemy right now, so the emitted `Attack` command
/// always passes the sim's validator.
fn snipe_target(
    game: &Game,
    player: Player,
    input: &FeatureInput,
    kind: SnipeTarget,
) -> Option<EntityId> {
    let turn = game.turn;
    // Army centroid (fallback: own HQ if the army is empty/undefined).
    let units = combat_unit_ids(game, player);
    let (cx, cy) = if units.is_empty() {
        (input.own_hq_tile.0 as i64, input.own_hq_tile.1 as i64)
    } else {
        let mut sx = 0i64;
        let mut sy = 0i64;
        let mut n = 0i64;
        for id in units {
            if let Some(u) = game.unit(player, id) {
                sx += u.tile.0 as i64;
                sy += u.tile.1 as i64;
                n += 1;
            }
        }
        if n == 0 {
            return None;
        }
        (sx / n, sy / n)
    };

    let mut best: Option<(i32, EntityId)> = None; // (chebyshev, id)
    match kind {
        SnipeTarget::Unit(ut) => {
            for m in &input.fog.units {
                if m.utype == ut && m.last_seen == turn {
                    let d = chebyshev(cx as u8, cy as u8, m.tile.0, m.tile.1);
                    if best.is_none_or(|(bd, bid)| d < bd || (d == bd && m.id < bid)) {
                        best = Some((d, m.id));
                    }
                }
            }
        }
        SnipeTarget::Building(bt) => {
            for m in &input.fog.buildings {
                if m.btype == bt && m.last_seen == turn {
                    let d = chebyshev(cx as u8, cy as u8, m.tile.0, m.tile.1);
                    if best.is_none_or(|(bd, bid)| d < bd || (d == bd && m.id < bid)) {
                        best = Some((d, m.id));
                    }
                }
            }
        }
    }
    best.map(|(_, id)| id)
}

fn sector_center(sector: usize, own_hq: (u8, u8)) -> (u8, u8) {
    let mut sx = sector % 8;
    let mut sy = sector / 8;
    if own_hq.0 >= 32 {
        sx = 7 - sx;
    }
    if own_hq.1 >= 32 {
        sy = 7 - sy;
    }
    ((sx * 8 + 4) as u8, (sy * 8 + 4) as u8)
}

// --- Legality masks ---------------------------------------------------------

fn build_allowed(game: &Game, p: Player, bt: BuildingType) -> bool {
    if bt != BuildingType::Refinery
        && !game
            .buildings
            .iter()
            .any(|b| b.owner == p && b.btype == BuildingType::Refinery)
    {
        return false;
    }
    if (bt == BuildingType::TechLab || bt == BuildingType::Airfield)
        && !game
            .buildings
            .iter()
            .any(|b| b.owner == p && b.btype == BuildingType::Factory)
    {
        return false;
    }
    // Second-tier structures need the TechLab itself.
    if (bt == BuildingType::Radar || bt == BuildingType::TeslaCoil)
        && !game
            .buildings
            .iter()
            .any(|b| b.owner == p && b.btype == BuildingType::TechLab)
    {
        return false;
    }
    let cost = crucible_sim::building_stats(bt).cost;
    if game.ore[p.index()] < cost {
        return false;
    }
    build_preferred(game, p, bt)
        .is_some_and(|pref| find_build_tile(game, p, bt, pref, &Default::default()).is_some())
}

fn base_offset(hq: (u8, u8), dx: i32, dy: i32) -> (u8, u8) {
    let sx = if hq.0 < 32 { dx } else { -dx };
    let sy = if hq.1 < 32 { dy } else { -dy };
    (
        (hq.0 as i32 + sx).clamp(0, 63) as u8,
        (hq.1 as i32 + sy).clamp(0, 63) as u8,
    )
}

fn build_preferred(game: &Game, p: Player, bt: BuildingType) -> Option<(u8, u8)> {
    let hq = game.hq(p)?;
    let hq_tile = hq.tile;
    Some(match bt {
        BuildingType::PowerPlant => base_offset(hq_tile, 0, -2),
        BuildingType::Refinery => base_offset(hq_tile, 2, 0),
        BuildingType::Factory => base_offset(hq_tile, 0, 2),
        BuildingType::Barracks => base_offset(hq_tile, 2, 2),
        BuildingType::TechLab => base_offset(hq_tile, -2, 2),
        BuildingType::Airfield => base_offset(hq_tile, -2, -2),
        BuildingType::Radar => base_offset(hq_tile, -4, 2),
        BuildingType::TeslaCoil => base_offset(hq_tile, 2, -2),
        BuildingType::Turret => base_offset(hq_tile, -2, 0),
        // Refineries ignore the preferred tile (they must touch their field),
        // but the anchor biases the search toward the base pockets.
        BuildingType::CrystalRefinery => base_offset(hq_tile, 3, 0),
        BuildingType::AATurret => base_offset(hq_tile, -3, 0),
        BuildingType::Hq => hq_tile,
    })
}

fn train_allowed(game: &Game, p: Player, ut: UnitType) -> bool {
    // Mirror validate_train exactly: artillery and mammoth need a TechLab.
    if matches!(ut, UnitType::Artillery | UnitType::MammothTank)
        && !game
            .buildings
            .iter()
            .any(|b| b.owner == p && b.btype == BuildingType::TechLab)
    {
        return false;
    }
    // Research-gated units need the tech researched, not just a lab built.
    if let Some(tech) = crucible_sim::entity::unit_requires_tech(ut) {
        if !game.research[p.index()].has(tech) {
            return false;
        }
    }
    if game.ore[p.index()] < unit_stats(ut).cost {
        return false;
    }
    let producer = producer_for(ut);
    game.buildings
        .iter()
        .any(|b| b.owner == p && b.btype == producer && b.queue.len() < game.config.max_queue)
}

fn producer_for(ut: UnitType) -> BuildingType {
    match ut {
        UnitType::Tank | UnitType::Artillery | UnitType::MammothTank => BuildingType::Factory,
        UnitType::SamLauncher => BuildingType::Factory,
        UnitType::Infantry => BuildingType::Barracks,
        UnitType::Scout | UnitType::RocketTrooper => BuildingType::Barracks,
        UnitType::Gunship | UnitType::Interceptor => BuildingType::Airfield,
    }
}

fn tech_allowed(game: &Game, p: Player, tech: TechId) -> bool {
    game.buildings
        .iter()
        .any(|b| b.owner == p && b.btype == BuildingType::TechLab)
        && !game.research[p.index()].has(tech)
        && game.research[p.index()].researching.is_none()
        && prereqs_met(tech, &game.research[p.index()].researched)
        && tech_info(tech).crystal_cost <= game.crystal[p.index()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::init;
    use crucible_sim::{Game, GameConfig, Map, Rng, Unit};

    fn spawn(g: &mut Game, owner: Player, utype: UnitType, tile: (u8, u8)) -> EntityId {
        let stats = unit_stats(utype);
        let id = g.alloc_id();
        g.units.push(Unit {
            id,
            owner,
            utype,
            tile,
            hp: stats.hp,
            max_hp: stats.hp,
            mp: stats.mp,
            moved: false,
            acted: false,
        });
        id
    }

    /// A free, passable, ore-free tile adjacent to an ore tile and within the
    /// base clump radius of `hq` — the Refinery rule (must touch ore) made
    /// fixed offsets near the HQ invalid on generated maps.
    fn refinery_tile(g: &Game, hq: (u8, u8), reserved: &[(u8, u8)]) -> (u8, u8) {
        use crucible_sim::map::{tile_coords, MAP_TILES};
        use crucible_sim::orders::NEIGHBOR_OFFSETS;
        let ore_tile = (0..MAP_TILES)
            .map(tile_coords)
            .filter(|&t| g.map.ore_at(t.0, t.1) > 0)
            .min_by_key(|&t| chebyshev(hq.0, hq.1, t.0, t.1))
            .expect("map has ore");
        for (dx, dy) in NEIGHBOR_OFFSETS {
            let (x, y) = (ore_tile.0 as i32 + dx, ore_tile.1 as i32 + dy);
            if x < 0 || y < 0 || x >= MAP_TILES as i32 / 64 || y >= MAP_TILES as i32 / 64 {
                continue;
            }
            let t = (x as u8, y as u8);
            if g.map.is_passable(t.0, t.1)
                && g.map.ore_at(t.0, t.1) == 0
                && g.building_at(t).is_none()
                && !reserved.contains(&t)
                && chebyshev(hq.0, hq.1, t.0, t.1) <= 5
            {
                return t;
            }
        }
        panic!("no refinery placement tile");
    }

    #[test]
    fn snipe_target_picks_visible_enemy_only() {
        let mut g = Game::new(crucible_sim::open_test_map(1), GameConfig::default());
        // P0's HQ sits with 5-tile vision. Place a P1 tank right beside it
        // (visible), an own infantry to attack with, and another P1 tank far
        // away (never seen).
        let near_tank = spawn(&mut g, Player::P1, UnitType::Tank, (51, 53));
        // An own infantry right beside the tank (in range to strike it later).
        let own = spawn(&mut g, Player::P0, UnitType::Infantry, (51, 52));
        let _far_tank = spawn(&mut g, Player::P1, UnitType::Tank, (10, 10));
        // Refresh fog so the near tank is recorded as seen this turn.
        g.fog_phase();

        let input = FeatureInput::from_game(&g, Player::P0);
        // The visible tank is the only candidate of its type.
        let hit = snipe_target(&g, Player::P0, &input, SnipeTarget::Unit(UnitType::Tank));
        assert_eq!(hit, Some(near_tank));

        // No refinery exists (and none is visible): the snipe must skip.
        let none = snipe_target(
            &g,
            Player::P0,
            &input,
            SnipeTarget::Building(BuildingType::Refinery),
        );
        assert!(none.is_none());

        // A target that dies in view stays in fog memory until it expires
        // (memory is expiry-based, not death-pruned), so the snipe may still
        // nominate its id this turn. The sim's validator is the real safety
        // net: an Attack on the swept id is rejected (NoSuchTarget). Kill the
        // tank through the real combat path (sweep_dead is crate-internal):
        // soften it to exactly lethal, then strike it with the infantry.
        let tidx = g.units.iter().position(|u| u.id == near_tank).unwrap();
        g.units[tidx].hp = unit_stats(UnitType::Infantry).damage;
        let kill = Command::Attack {
            player: Player::P0,
            units: vec![own],
            target: near_tank,
        };
        assert!(g.validate_command(&kill).is_ok(), "{kill:?} must be legal");
        g.apply_commands(Player::P0, &[kill]);

        let input = FeatureInput::from_game(&g, Player::P0);
        let maybe = snipe_target(&g, Player::P0, &input, SnipeTarget::Unit(UnitType::Tank));
        assert!(
            maybe.is_some(),
            "the fog must still remember the tank that died in view"
        );
        if let Some(id) = maybe {
            // A fresh unit (the first one already acted) attacking the swept
            // id must be rejected with NoSuchTarget.
            let scout = spawn(&mut g, Player::P0, UnitType::Infantry, (10, 10));
            let cmd = Command::Attack {
                player: Player::P0,
                units: vec![scout],
                target: id,
            };
            assert_eq!(
                g.validate_command(&cmd).unwrap_err(),
                crucible_sim::CommandError::NoSuchTarget,
                "an attack on a dead target must be rejected"
            );
        }
    }

    #[test]
    fn decide_is_deterministic_and_valid() {
        let mut g = Game::new(Map::generate(5), GameConfig::default());
        g.fog_phase();
        let mut rng = Rng::from_seed(1);
        let genome = init(&mut rng);
        let input = FeatureInput::from_game(&g, Player::P0);
        let a = decide(&g, Player::P0, &genome, &input, &[]);
        let b = decide(&g, Player::P0, &genome, &input, &[]);
        assert_eq!(a, b);
        // Every emitted command must pass validation.
        for cmd in &a {
            assert!(g.validate_command(cmd).is_ok(), "invalid command {cmd:?}");
        }
    }

    #[test]
    fn illegal_actions_are_masked() {
        // With zero ore and no buildings, no build/train action is possible.
        let mut g = Game::new(Map::generate(5), GameConfig::default());
        g.ore[0] = 0;
        g.fog_phase();
        let mut rng = Rng::from_seed(2);
        let genome = init(&mut rng);
        let input = FeatureInput::from_game(&g, Player::P0);
        let cmds = decide(&g, Player::P0, &genome, &input, &[]);
        for cmd in &cmds {
            assert!(!matches!(
                cmd,
                Command::PlaceBuilding { .. } | Command::TrainUnit { .. }
            ));
        }
    }

    #[test]
    fn air_power_actions_are_learnable() {
        // The learned policy must be able to reach the air power actions: the
        // Airfield build slot is legal once a Refinery + Factory exist, and
        // both aircraft train slots are legal once an Airfield exists. Masking
        // must never permanently hide them from the network.
        let mut g = Game::new(Map::generate(5), GameConfig::default());
        g.ore[0] = 10_000;
        let hq = g.hq(Player::P0).unwrap().tile;

        // Refinery (must touch ore under the new rule) + Factory (the Factory
        // gate for Airfield).
        let reserved = [(hq.0, hq.1 + 2), (hq.0 + 2, hq.1 + 2)];
        for (bt, tile) in [
            (BuildingType::Refinery, refinery_tile(&g, hq, &reserved)),
            (BuildingType::Factory, (hq.0, hq.1 + 2)),
        ] {
            let cmd = Command::PlaceBuilding {
                player: Player::P0,
                btype: bt,
                tile,
            };
            assert!(g.validate_command(&cmd).is_ok(), "{cmd:?} must be legal");
            g.apply_commands(Player::P0, &[cmd]);
        }
        assert!(build_allowed(&g, Player::P0, BuildingType::Airfield));

        // Place the Airfield and verify both aircraft trains are unmasked.
        let cmd = Command::PlaceBuilding {
            player: Player::P0,
            btype: BuildingType::Airfield,
            tile: (hq.0 + 2, hq.1 + 2),
        };
        assert!(g.validate_command(&cmd).is_ok(), "{cmd:?} must be legal");
        g.apply_commands(Player::P0, &[cmd]);
        assert!(train_allowed(&g, Player::P0, UnitType::Gunship));
        assert!(train_allowed(&g, Player::P0, UnitType::Interceptor));

        // The produced units actually come from the Airfield.
        assert_eq!(producer_for(UnitType::Gunship), BuildingType::Airfield);
        assert_eq!(producer_for(UnitType::Interceptor), BuildingType::Airfield);
    }

    #[test]
    fn tech_tree_actions_are_learnable() {
        // The second tier (Radar / TeslaCoil buildings, MammothTank +
        // Artillery trains, and the Range research) must never be permanently
        // masked once its prerequisites exist, so the network can learn the
        // whole tree.
        let mut g = Game::new(Map::generate(5), GameConfig::default());
        g.ore[0] = 100_000;
        let hq = g.hq(Player::P0).unwrap().tile;

        // Locked before the TechLab exists.
        assert!(!build_allowed(&g, Player::P0, BuildingType::Radar));
        assert!(!build_allowed(&g, Player::P0, BuildingType::TeslaCoil));
        assert!(!train_allowed(&g, Player::P0, UnitType::MammothTank));
        assert!(!train_allowed(&g, Player::P0, UnitType::Artillery));

        // Refinery (must touch ore under the new rule) + Factory.
        let reserved = [(hq.0, hq.1 + 2), (hq.0 + 2, hq.1 + 2)];
        for (bt, tile) in [
            (BuildingType::Refinery, refinery_tile(&g, hq, &reserved)),
            (BuildingType::Factory, (hq.0, hq.1 + 2)),
        ] {
            let cmd = Command::PlaceBuilding {
                player: Player::P0,
                btype: bt,
                tile,
            };
            assert!(g.validate_command(&cmd).is_ok(), "{cmd:?} must be legal");
            g.apply_commands(Player::P0, &[cmd]);
        }
        let cmd = Command::PlaceBuilding {
            player: Player::P0,
            btype: BuildingType::TechLab,
            tile: (hq.0 + 2, hq.1 + 2),
        };
        assert!(g.validate_command(&cmd).is_ok(), "{cmd:?} must be legal");
        g.apply_commands(Player::P0, &[cmd]);

        // Everything on the second tier is now unmasked.
        assert!(build_allowed(&g, Player::P0, BuildingType::Radar));
        assert!(build_allowed(&g, Player::P0, BuildingType::TeslaCoil));
        assert!(train_allowed(&g, Player::P0, UnitType::MammothTank));
        assert!(train_allowed(&g, Player::P0, UnitType::Artillery));
        // MammothTank trains from the Factory.
        assert_eq!(producer_for(UnitType::MammothTank), BuildingType::Factory);
        // The tier-1 research options are all reachable with a lab built and
        // research idle; a second start is masked until the first completes.
        for tech in [
            TechId::HighExplosive,
            TechId::CompositeArmor,
            TechId::TargetingOptics,
        ] {
            assert!(tech_allowed(&g, Player::P0, tech));
        }
        let cmd = Command::StartResearch {
            player: Player::P0,
            tech: TechId::HighExplosive,
        };
        assert!(g.validate_command(&cmd).is_ok(), "{cmd:?} must be legal");
        g.apply_commands(Player::P0, &[cmd]);
        assert!(!tech_allowed(&g, Player::P0, TechId::CompositeArmor));
    }
}
