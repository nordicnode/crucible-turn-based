//! The unified, difficulty-parameterized scripted commander: `AdaptiveBot`.
//!
//! The menu used to expose **three separate hand-written bots** (`easy`,
//! `medium`, `hard`) plus a client-side "adaptive" hack that just re-picked
//! one of them. All three were, in practice, the *same engine* — shared
//! economy opener, marching/attack code, research helper and batch ledger —
//! parameterized by a handful of hand-tuned dials. That duplication is now
//! collapsed into a **single `AdaptiveBot { difficulty }`** whose behavior is
//! a pure function of a difficulty scalar in `0..=1`.
//!
//! The three historical tiers are kept as *anchor policies* (data, not code):
//! `easy = AdaptiveBot::new(0.15)` (defensive turtle), `medium =
//! AdaptiveBot::new(0.5)` (continuous infantry pressure), `hard =
//! AdaptiveBot::new(0.9)` (expand-and-push with the full tech tree). The
//! trainer, curriculum, balance baseline, gauntlet, and `bots.rs` all construct
//! these via the unchanged `easy()`/`medium()`/`hard()` constructors, so
//! behavior at the anchor difficulties is byte-identical to the old bots.
//!
//! These are **oracle baselines**: they may read the full [`Game`] (see
//! `CONTRACT.md` §5). They exist to bootstrap training, seed the gauntlet
//! baselines, and anchor the regression floor the learned champion must beat.
//! They are deterministic given a map seed, never exceed the sim's per-turn
//! action budget, and always end their turn.
//!
//! # Batch legality
//!
//! The sim applies a command batch **sequentially**, validating each command
//! against the live state left by the previous one. Helpers therefore plan
//! through a [`Plan`] ledger (ore already spent this batch, tiles already
//! claimed, queue slots already booked) so every emitted command passes
//! `validate_command` not just against the pre-batch snapshot but against the
//! state as it will be when its turn comes.

use crucible_sim::map::MAP_SIZE;
use crucible_sim::{
    building_stats, tech::prereqs_met, tech::tech_info, tech::TechId, tiles::chebyshev, unit_stats,
    Building, BuildingType, Command, EntityId, Game, Player, ResourceBundle, ResourceType,
    UnitType,
};

use crate::bot::Bot;

// ---------------------------------------------------------------------------
// Batch planning
// ---------------------------------------------------------------------------

/// What this turn's command batch has already committed. The sim validates a
/// batch one command at a time against mutating state, so helpers must price
/// affordability and space against `pre-state minus plan`, not the snapshot.
#[derive(Default)]
pub(crate) struct Plan {
    pub(crate) resources_spent: ResourceBundle,
    pub(crate) used_tiles: Vec<(u8, u8)>,
    /// Queue slots booked this batch, per building id.
    pub(crate) extra_queue: Vec<(EntityId, usize)>,
}

impl Plan {
    fn resources_left(&self, g: &Game, p: Player) -> ResourceBundle {
        let current = g.resources(p);
        ResourceBundle::new(
            current.ore - self.resources_spent.ore,
            current.steel - self.resources_spent.steel,
            current.coal - self.resources_spent.coal,
            current.crystal - self.resources_spent.crystal,
        )
    }

    fn extra_queued(&self, id: EntityId) -> usize {
        self.extra_queue
            .iter()
            .find(|&&(b, _)| b == id)
            .map_or(0, |&(_, n)| n)
    }

    fn book_queue(&mut self, id: EntityId) {
        if let Some(entry) = self.extra_queue.iter_mut().find(|(b, _)| *b == id) {
            entry.1 += 1;
        } else {
            self.extra_queue.push((id, 1));
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn own_building(g: &Game, p: Player, bt: BuildingType) -> Option<&Building> {
    g.buildings
        .iter()
        .find(|b| b.owner == p && b.btype == bt && b.is_operational())
}

fn count_buildings(g: &Game, p: Player, bt: BuildingType) -> usize {
    g.buildings
        .iter()
        .filter(|b| b.owner == p && b.btype == bt && b.is_alive())
        .count()
}

fn count_units(g: &Game, p: Player, ut: UnitType) -> usize {
    g.units
        .iter()
        .filter(|u| u.owner == p && u.utype == ut)
        .count()
}

pub(crate) fn combat_unit_ids(g: &Game, p: Player) -> Vec<EntityId> {
    g.units
        .iter()
        .filter(|u| u.owner == p && unit_stats(u.utype).damage > 0)
        .map(|u| u.id)
        .collect()
}

pub(crate) fn is_valid_build_tile(g: &Game, p: Player, bt: BuildingType, tile: (u8, u8)) -> bool {
    let cmd = Command::PlaceBuilding {
        player: p,
        btype: bt,
        tile,
    };
    g.validate_command(&cmd).is_ok()
}

/// Find a valid placement tile, searching outward from `preferred` in a
/// deterministic ring order, skipping tiles already claimed by this batch.
///
/// For a `Refinery` the preferred tile is only a distance anchor: the search
/// walks every ore tile (nearest to `preferred` first, ties by tile index)
/// and tries each one's free 8-dir neighbors. Refineries are exempt from the
/// base-clump rule — they must instead touch an ore field, so remote ore
/// pockets are the expansion mechanic.
pub(crate) fn find_build_tile(
    g: &Game,
    p: Player,
    bt: BuildingType,
    preferred: (u8, u8),
    plan: &Plan,
) -> Option<(u8, u8)> {
    let free = |t: (u8, u8)| !plan.used_tiles.contains(&t) && is_valid_build_tile(g, p, bt, t);

    if bt == BuildingType::Refinery || bt == BuildingType::CrystalRefinery {
        // A generic refinery claims the exact deposit tile. Search all known
        // resource kinds nearest to the preference point; the validator still
        // decides whether the tile is legal and whether the tile is already
        // occupied by another order in this batch.
        let mut fields: Vec<(usize, (u8, u8))> = (0..crucible_sim::map::MAP_TILES)
            .filter_map(|idx| {
                let t = crucible_sim::map::tile_coords(idx);
                (g.map.resource_amount_at(t.0, t.1) > 0).then_some((idx, t))
            })
            .collect();
        // Prefer ore first: a generic refinery yields its deposit's resource,
        // and the opening refinery must feed the ore economy. Once deposits
        // of every kind sit close to spawn (they now do), nearest-first alone
        // would strand the first refinery on steel/coal and starve ore.
        fields.sort_by_key(|&(idx, t)| {
            let kind_rank = match g.map.resource_at(t.0, t.1) {
                Some(ResourceType::Ore) => 0,
                Some(ResourceType::Steel) => 1,
                Some(ResourceType::Coal) => 2,
                _ => 3,
            };
            (
                kind_rank,
                chebyshev(t.0, t.1, preferred.0, preferred.1),
                idx,
            )
        });
        for (_, field_tile) in fields {
            if free(field_tile) {
                return Some(field_tile);
            }
        }
        return None;
    }

    if free(preferred) {
        return Some(preferred);
    }
    for r in 1..=32i32 {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dy.abs() != r {
                    continue;
                }
                let x = preferred.0 as i32 + dx;
                let y = preferred.1 as i32 + dy;
                if !(0..MAP_SIZE as i32).contains(&x) || !(0..MAP_SIZE as i32).contains(&y) {
                    continue;
                }
                let t = (x as u8, y as u8);
                if free(t) {
                    return Some(t);
                }
            }
        }
    }
    None
}

/// Place `bt` if the player has fewer than `max` of them and the batch can
/// still afford it. Books the cost and tile in `plan` on success.
fn place_if_missing(
    g: &Game,
    p: Player,
    bt: BuildingType,
    preferred: (u8, u8),
    max: usize,
    plan: &mut Plan,
) -> Option<Command> {
    if count_buildings(g, p, bt) >= max {
        return None;
    }
    let cost = building_stats(bt).resource_cost;
    if !plan.resources_left(g, p).can_afford(cost) {
        return None;
    }
    let tile = find_build_tile(g, p, bt, preferred, plan)?;
    plan.resources_spent = plan.resources_spent.saturating_add(cost);
    plan.used_tiles.push(tile);
    Some(Command::PlaceBuilding {
        player: p,
        btype: bt,
        tile,
    })
}

/// Place a generic refinery on the nearest live deposit of `resource`.
/// Resource refineries are the expansion step: unlike ordinary structures
/// they do not need to sit beside the HQ, but the command still goes through
/// the same server validator and batch resource ledger.
fn place_refinery_for_resource(
    g: &Game,
    p: Player,
    resource: ResourceType,
    preferred: (u8, u8),
    plan: &mut Plan,
) -> Option<Command> {
    if g.buildings.iter().any(|b| {
        b.owner == p
            && b.btype.is_refinery()
            && g.map.resource_at(b.tile.0, b.tile.1) == Some(resource)
    }) {
        return None;
    }
    let cost = building_stats(BuildingType::Refinery).resource_cost;
    if !plan.resources_left(g, p).can_afford(cost) {
        return None;
    }
    let mut fields: Vec<(i32, usize, (u8, u8))> = (0..crucible_sim::map::MAP_TILES)
        .filter_map(|idx| {
            let t = crucible_sim::map::tile_coords(idx);
            (g.map.resource_at(t.0, t.1) == Some(resource)
                && g.map.resource_amount_at(t.0, t.1) > 0)
                .then_some((chebyshev(t.0, t.1, preferred.0, preferred.1), idx, t))
        })
        .collect();
    fields.sort_by_key(|&(distance, idx, _)| (distance, idx));
    for (_, _, tile) in fields {
        if plan.used_tiles.contains(&tile)
            || !is_valid_build_tile(g, p, BuildingType::Refinery, tile)
        {
            continue;
        }
        // Do not claim a deposit already served by one of our refineries.
        if g.buildings
            .iter()
            .any(|b| b.owner == p && b.btype.is_refinery() && b.tile == tile)
        {
            continue;
        }
        plan.resources_spent = plan.resources_spent.saturating_add(cost);
        plan.used_tiles.push(tile);
        return Some(Command::PlaceBuilding {
            player: p,
            btype: BuildingType::Refinery,
            tile,
        });
    }
    None
}

/// Train `ut` from the producing building if under `target` and the batch can
/// still afford it.
///
/// `target` counts spawned units plus units still queued, so the bot does not
/// over-commit ore. Queue capacity is checked against the batch's bookings so
/// several trains in one turn cannot overfill a producer. Tech-gated units
/// (artillery, mammoth) stay masked until a TechLab exists, mirroring the
/// sim's validator exactly.
fn train_up_to(
    g: &Game,
    p: Player,
    producer: BuildingType,
    ut: UnitType,
    target: usize,
    plan: &mut Plan,
) -> Option<Command> {
    if matches!(ut, UnitType::Artillery | UnitType::MammothTank)
        && !g
            .buildings
            .iter()
            .any(|b| b.owner == p && b.btype == BuildingType::TechLab && b.is_operational())
    {
        return None;
    }
    // Research-gated units stay masked until the tech is researched,
    // mirroring the sim's validator exactly.
    if let Some(tech) = crucible_sim::entity::unit_requires_tech(ut) {
        if !g.research[p.index()].has(tech) {
            return None;
        }
    }
    let queued: usize = g
        .buildings
        .iter()
        .filter(|b| b.owner == p && b.btype == producer)
        .map(|b| b.queue.iter().filter(|&&q| q == ut).count())
        .sum();
    if count_units(g, p, ut) + queued >= target {
        return None;
    }
    let load = |b: &Building| b.queue.len() + plan.extra_queued(b.id);
    let building = g
        .buildings
        .iter()
        .filter(|b| {
            b.owner == p
                && b.btype == producer
                && b.is_operational()
                && load(b) < g.config.max_queue
        })
        .min_by_key(|b| (load(b), b.id))?;
    let cost = unit_stats(ut).resource_cost;
    if !plan.resources_left(g, p).can_afford(cost) {
        return None;
    }
    plan.resources_spent = plan.resources_spent.saturating_add(cost);
    plan.book_queue(building.id);
    Some(Command::TrainUnit {
        player: p,
        building: building.id,
        utype: ut,
    })
}

/// The closest enemy (unit or building) any of `p`'s combat units can hit
/// this turn, minimizing (distance from shooter, target id). Buildings sort
/// before units on ties (lower ids), so an in-range HQ is focused over the
/// defender beside it — sieges finish games.
fn nearest_hittable_enemy(g: &Game, p: Player) -> Option<EntityId> {
    let units = combat_unit_ids(g, p);
    let mut best: Option<(i32, EntityId)> = None;
    let mut consider = |d: i32, id: EntityId| {
        if best.is_none_or(|(bd, bid)| d < bd || (d == bd && id < bid)) {
            best = Some((d, id));
        }
    };
    for e in &g.units {
        if e.owner == p.enemy() && e.is_alive() {
            for &id in &units {
                let Some(u) = g.unit(p, id) else {
                    continue;
                };
                let s = unit_stats(u.utype);
                let d = chebyshev(u.tile.0, u.tile.1, e.tile.0, e.tile.1);
                if d <= s.range_tiles && d >= s.min_range_tiles {
                    consider(d, e.id);
                }
            }
        }
    }
    for b in &g.buildings {
        if b.owner == p.enemy() && b.is_alive() {
            for &id in &units {
                let Some(u) = g.unit(p, id) else {
                    continue;
                };
                let s = unit_stats(u.utype);
                let d = chebyshev(u.tile.0, u.tile.1, b.tile.0, b.tile.1);
                if d <= s.range_tiles && d >= s.min_range_tiles {
                    consider(d, b.id);
                }
            }
        }
    }
    best.map(|(_, id)| id)
}

/// Shared army micro for the scripted bots and the learned commander: advance
/// toward `objective` every turn, then focus-fire the nearest hittable enemy.
///
/// Units never auto-attack in the turn model (only turrets fire on their
/// own), so a march must be *ordered*: the `MoveGroup` walks everyone forward
/// (move-then-attack is the legal order), and the trailing `Attack` lets the
/// units the march brought into range fire this turn while the rest hold
/// position until the next advance. This keeps armies flowing through each
/// other instead of freezing into a mid-map grind — matches end by HQ
/// destruction rather than timeout.
/// Opening book: while the army is still assembling (three or fewer combat
/// units), the nearest combat unit scouts the closest unclaimed resource
/// pocket instead of sitting by the HQ. Vision of steel/coal/crystal sites
/// lets the bot expand with information — it knows where the contested
/// deposits are before it commits the march. Once the army grows past the
/// scouting window every unit belongs to the push.
pub(crate) fn opening_scout(g: &Game, p: Player) -> Vec<Command> {
    let units = combat_unit_ids(g, p);
    if units.is_empty() || units.len() > 3 {
        return Vec::new();
    }
    let hq = g.hq(p).map(|b| b.tile).unwrap_or((8, 8));
    let mut best: Option<(i32, (u8, u8))> = None;
    for idx in 0..g.map.resource_kind.len() {
        if g.map.resource_kind[idx].is_none() {
            continue;
        }
        let (x, y) = crucible_sim::map::tile_coords(idx);
        // A site is already secured when this player extracts it.
        if g.buildings
            .iter()
            .any(|b| b.owner == p && b.is_alive() && b.tile == (x, y))
        {
            continue;
        }
        let d = chebyshev(x, y, hq.0, hq.1);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, (x, y)));
        }
    }
    let Some((_, objective)) = best else {
        return Vec::new();
    };
    vec![Command::MoveGroup {
        player: p,
        units,
        waypoint: objective,
    }]
}

pub(crate) fn army_orders(g: &Game, p: Player, objective: (u8, u8)) -> Vec<Command> {
    let units = combat_unit_ids(g, p);
    if units.is_empty() {
        return Vec::new();
    }
    let mut cmds = vec![Command::MoveGroup {
        player: p,
        units: units.clone(),
        waypoint: objective,
    }];
    if let Some(target) = nearest_hittable_enemy(g, p) {
        cmds.push(Command::Attack {
            player: p,
            units,
            target,
        });
    }
    cmds
}

/// The next technology to research, if a Tech Lab exists and research is
/// idle. Picks the highest-priority tech whose prereqs are met and whose
/// crystal cost is affordable (crystal is deducted at completion).
/// Deterministic priority: army power first, then the rocket unlocks, then
/// economy, then the deep tech.
fn research_next(g: &Game, p: Player) -> Option<Command> {
    use TechId::*;
    let _lab = own_building(g, p, BuildingType::TechLab)?;
    let r = &g.research[p.index()];
    if r.researching.is_some() {
        return None;
    }
    const PRIORITY: [TechId; 10] = [
        HighExplosive,
        CompositeArmor,
        TargetingOptics,
        RocketPropulsion,
        EfficientRefining,
        TitaniumAlloys,
        AdvancedBallistics,
        Superconductors,
        AerialSuperiority,
        CrystalNanotech,
    ];
    for t in PRIORITY {
        if r.has(t) || !prereqs_met(t, &r.researched) {
            continue;
        }
        if tech_info(t).crystal_cost > g.resources(p).crystal {
            continue;
        }
        return Some(Command::StartResearch { player: p, tech: t });
    }
    None
}

fn enemy_hq_tile(g: &Game, p: Player) -> (u8, u8) {
    if let Some(hq) = g.hq(p.enemy()) {
        return hq.tile;
    }
    g.map.hq_tiles[p.enemy().index()]
}

/// A tile `dist` tiles from `hq` toward `enemy` (clamped to the map).
fn toward_enemy(hq: (u8, u8), enemy: (u8, u8), dist: i32) -> (u8, u8) {
    let dx = (enemy.0 as i32 - hq.0 as i32).signum();
    let dy = (enemy.1 as i32 - hq.1 as i32).signum();
    (
        (hq.0 as i32 + dx * dist).clamp(0, MAP_SIZE as i32 - 1) as u8,
        (hq.1 as i32 + dy * dist).clamp(0, MAP_SIZE as i32 - 1) as u8,
    )
}

/// Symmetrically orient building offsets toward the quadrant's natural ore pocket.
fn base_offset(hq: (u8, u8), dx: i32, dy: i32) -> (u8, u8) {
    let sx = if hq.0 < (MAP_SIZE / 2) as u8 { dx } else { -dx };
    let sy = if hq.1 < (MAP_SIZE / 2) as u8 { dy } else { -dy };
    (
        (hq.0 as i32 + sx).clamp(0, MAP_SIZE as i32 - 1) as u8,
        (hq.1 as i32 + sy).clamp(0, MAP_SIZE as i32 - 1) as u8,
    )
}

// ---------------------------------------------------------------------------
// The unified commander
// ---------------------------------------------------------------------------

/// Anchor difficulties for the three historical tiers. `difficulty` falls
/// into the band of the nearest anchor, so stepping `0.15 → 0.5 → 0.9` moves
/// through the full spectrum; anything between picks the matching archetype.
const D_EASY: f32 = 0.15;
const D_MEDIUM: f32 = 0.5;
const D_HARD: f32 = 0.9;

/// Whether a training spec may fire right now.
#[derive(Clone, Copy)]
enum Gate {
    /// No condition beyond affordability/buildings.
    Always,
    /// A Steel refinery is secured (built/claimed).
    SteelSecured,
    /// Steel AND Coal refineries are secured AND a Factory exists.
    SteelCoalAndFactory,
    /// At least `n` infantry are fielded.
    InfantryGe(usize),
    /// A TechLab exists and `turn > n` (`n == 0` → any turn once it exists).
    TechLabAfter(i32),
    /// `turn > n`.
    Turn(i32),
}

#[derive(Clone, Copy)]
struct TrainSpec {
    unit: UnitType,
    producer: BuildingType,
    target: u32,
    gate: Gate,
}

/// One difficulty archetype: strategy expressed as data (not three separate
/// `decide()` bodies), read by the single `AdaptiveBot::decide` below.
#[derive(Clone, Copy)]
struct Policy {
    /// Build Factory before Barracks (vs Barracks-first to flood bodies).
    factory_first: bool,
    /// Build a PowerPlant in the opening.
    powerplant: bool,
    /// Turns after which to secure Steel / Coal refineries (`0` = never).
    steel_turn: i32,
    coal_turn: i32,
    /// Defensive turret plan.
    turrets: u8,
    turret_start_turn: i32,
    turret_distance: i32,
    /// Ordered army training plan (priority order = fielding order).
    army: &'static [TrainSpec],
    /// Drive the research tree from a TechLab.
    research: bool,
    /// TechLab build time: `0` = ASAP, `i32::MAX` = never.
    techlab_turn: i32,
    /// SAM/AA anti-air cap and switch.
    sam_scale: u32,
    aa: bool,
    /// Late-game second factory / testsla defense (`0` = off).
    second_factory: bool,
    tesla_turn: i32,
    /// Opening scout window (turn cutoff; `0` = none).
    scout_until: i32,
    /// Minimum combat units before the march fires (`0` = always defend).
    march_threshold: i32,
    /// Distance within which any unit counts as "committed" and keeps pressing.
    committed_dist: i32,
    /// Defend the own HQ (true) vs push the enemy HQ (false).
    defend: bool,
}

const EASY: Policy = Policy {
    factory_first: true,
    powerplant: false,
    steel_turn: 0,
    coal_turn: 0,
    turrets: 3,
    turret_start_turn: 20,
    turret_distance: 2,
    army: &[
        TrainSpec {
            unit: UnitType::Infantry,
            producer: BuildingType::Barracks,
            target: 4,
            gate: Gate::Always,
        },
        TrainSpec {
            unit: UnitType::Tank,
            producer: BuildingType::Factory,
            target: 1,
            gate: Gate::Always,
        },
    ],
    research: false,
    techlab_turn: i32::MAX,
    sam_scale: 0,
    aa: false,
    second_factory: false,
    tesla_turn: 0,
    scout_until: 0,
    march_threshold: 0,
    committed_dist: 0,
    defend: true,
};

const MEDIUM: Policy = Policy {
    factory_first: false,
    powerplant: false,
    steel_turn: 8,
    coal_turn: 22,
    turrets: 0,
    turret_start_turn: 0,
    turret_distance: 3,
    army: &[
        TrainSpec {
            unit: UnitType::Infantry,
            producer: BuildingType::Barracks,
            target: 10,
            gate: Gate::SteelSecured,
        },
        TrainSpec {
            unit: UnitType::Artillery,
            producer: BuildingType::Factory,
            target: 3,
            gate: Gate::TechLabAfter(35),
        },
        TrainSpec {
            unit: UnitType::Tank,
            producer: BuildingType::Factory,
            target: 4,
            gate: Gate::InfantryGe(4),
        },
    ],
    research: false,
    techlab_turn: 30,
    sam_scale: 0,
    aa: false,
    second_factory: false,
    tesla_turn: 0,
    scout_until: 0,
    march_threshold: 8,
    committed_dist: 12,
    defend: false,
};

const HARD: Policy = Policy {
    factory_first: true,
    powerplant: true,
    steel_turn: 10,
    coal_turn: 24,
    turrets: 1,
    turret_start_turn: 15,
    turret_distance: 3,
    army: &[
        TrainSpec {
            unit: UnitType::Tank,
            producer: BuildingType::Factory,
            target: 14,
            gate: Gate::SteelCoalAndFactory,
        },
        TrainSpec {
            unit: UnitType::Infantry,
            producer: BuildingType::Barracks,
            target: 8,
            gate: Gate::SteelCoalAndFactory,
        },
        TrainSpec {
            unit: UnitType::RocketTrooper,
            producer: BuildingType::Barracks,
            target: 2,
            gate: Gate::SteelCoalAndFactory,
        },
        TrainSpec {
            unit: UnitType::Artillery,
            producer: BuildingType::Factory,
            target: 4,
            gate: Gate::SteelCoalAndFactory,
        },
        TrainSpec {
            unit: UnitType::MammothTank,
            producer: BuildingType::Factory,
            target: 2,
            gate: Gate::Turn(80),
        },
    ],
    research: true,
    techlab_turn: 0,
    sam_scale: 4,
    aa: true,
    second_factory: true,
    tesla_turn: 45,
    scout_until: 14,
    march_threshold: 4,
    committed_dist: 0,
    defend: false,
};

/// Resolve a difficulty scalar to its nearest anchor archetype.
fn policy_for(difficulty: f32) -> Policy {
    let anchors = [(D_EASY, EASY), (D_MEDIUM, MEDIUM), (D_HARD, HARD)];
    let mut best = EASY;
    let mut best_d = f32::MAX;
    for (anchor, policy) in anchors {
        let d = (difficulty - anchor).abs();
        // On a tie pick the easier archetype (lower difficulty).
        if d < best_d {
            best_d = d;
            best = policy;
        }
    }
    best
}

/// The single, difficulty-parameterized commander. `difficulty` in `0..=1`
/// selects the strategy: low → defensive turtle, mid → infantry pressure,
/// high → expand-and-push with the full tech tree.
pub struct AdaptiveBot {
    difficulty: f32,
}

impl AdaptiveBot {
    pub fn new(difficulty: f32) -> Self {
        Self {
            difficulty: difficulty.clamp(0.0, 1.0),
        }
    }

    pub fn difficulty(&self) -> f32 {
        self.difficulty
    }
}

impl Bot for AdaptiveBot {
    fn name(&self) -> &'static str {
        if self.difficulty >= D_HARD - 0.1 {
            "hard"
        } else if self.difficulty >= (D_MEDIUM + D_EASY) / 2.0 {
            "medium"
        } else {
            "easy"
        }
    }

    fn decide(&mut self, g: &Game, p: Player) -> Vec<Command> {
        let pol = policy_for(self.difficulty);
        let mut out = Vec::new();
        let mut plan = Plan::default();
        let hq = g.hq(p).map(|b| b.tile).unwrap_or((8, 8));
        let enemy = enemy_hq_tile(g, p);

        // --- Economy openers (all tiers: refinery first) ---
        if let Some(c) = place_if_missing(
            g,
            p,
            BuildingType::Refinery,
            base_offset(hq, 2, 0),
            1,
            &mut plan,
        ) {
            out.push(c);
        }
        if pol.factory_first {
            if let Some(c) = place_if_missing(
                g,
                p,
                BuildingType::Factory,
                base_offset(hq, 0, 2),
                1,
                &mut plan,
            ) {
                out.push(c);
            }
        }
        if let Some(c) = place_if_missing(
            g,
            p,
            BuildingType::Barracks,
            base_offset(hq, 2, 2),
            1,
            &mut plan,
        ) {
            out.push(c);
        }
        if !pol.factory_first {
            if let Some(c) = place_if_missing(
                g,
                p,
                BuildingType::Factory,
                base_offset(hq, 0, 2),
                1,
                &mut plan,
            ) {
                out.push(c);
            }
        }
        if pol.powerplant {
            if let Some(c) = place_if_missing(
                g,
                p,
                BuildingType::PowerPlant,
                base_offset(hq, 0, -2),
                1,
                &mut plan,
            ) {
                out.push(c);
            }
        }

        // --- Expansion refineries (0 = never) ---
        let steel_secured = g.buildings.iter().any(|b| {
            b.owner == p
                && b.btype.is_refinery()
                && g.map.resource_at(b.tile.0, b.tile.1) == Some(ResourceType::Steel)
        });
        if pol.steel_turn > 0 && g.turn > pol.steel_turn {
            if let Some(c) = place_refinery_for_resource(
                g,
                p,
                ResourceType::Steel,
                base_offset(hq, 6, 0),
                &mut plan,
            ) {
                out.push(c);
            }
        }
        let coal_secured = g.buildings.iter().any(|b| {
            b.owner == p
                && b.btype.is_refinery()
                && g.map.resource_at(b.tile.0, b.tile.1) == Some(ResourceType::Coal)
        });
        if pol.coal_turn > 0 && g.turn > pol.coal_turn {
            if let Some(c) = place_refinery_for_resource(
                g,
                p,
                ResourceType::Coal,
                base_offset(hq, 10, 0),
                &mut plan,
            ) {
                out.push(c);
            }
        }
        let had_factory = own_building(g, p, BuildingType::Factory).is_some();

        // --- TechLab ---
        if pol.techlab_turn != i32::MAX {
            let due = pol.techlab_turn == 0 || g.turn > pol.techlab_turn;
            if due {
                if let Some(c) = place_if_missing(
                    g,
                    p,
                    BuildingType::TechLab,
                    base_offset(hq, -2, 2),
                    1,
                    &mut plan,
                ) {
                    out.push(c);
                }
            }
        }

        // --- Army training (policy's ordered table) ---
        for spec in pol.army {
            let gate_ok = match spec.gate {
                Gate::Always => true,
                Gate::SteelSecured => steel_secured,
                Gate::SteelCoalAndFactory => steel_secured && coal_secured && had_factory,
                Gate::InfantryGe(n) => count_units(g, p, UnitType::Infantry) >= n,
                Gate::TechLabAfter(n) => {
                    own_building(g, p, BuildingType::TechLab).is_some() && (n == 0 || g.turn > n)
                }
                Gate::Turn(n) => g.turn > n,
            };
            if gate_ok {
                if let Some(c) = train_up_to(
                    g,
                    p,
                    spec.producer,
                    spec.unit,
                    spec.target as usize,
                    &mut plan,
                ) {
                    out.push(c);
                }
            }
        }

        // --- Research tree ---
        if pol.research {
            if let Some(c) = research_next(g, p) {
                out.push(c);
            }
        }

        // --- Defense: turrets ---
        for i in 0..pol.turrets {
            if g.turn > pol.turret_start_turn + i as i32 * 20 {
                if let Some(c) = place_if_missing(
                    g,
                    p,
                    BuildingType::Turret,
                    toward_enemy(hq, enemy, pol.turret_distance + i as i32),
                    i as usize + 1,
                    &mut plan,
                ) {
                    out.push(c);
                }
            }
        }

        // --- Late-game: second factory, Tesla defense ---
        if pol.second_factory && own_building(g, p, BuildingType::TechLab).is_some() {
            if let Some(c) = place_if_missing(
                g,
                p,
                BuildingType::Factory,
                base_offset(hq, 0, 4),
                2,
                &mut plan,
            ) {
                out.push(c);
            }
        }
        if pol.tesla_turn > 0
            && g.turn > pol.tesla_turn
            && own_building(g, p, BuildingType::TechLab).is_some()
        {
            if g.can_afford(p, building_stats(BuildingType::PowerPlant).resource_cost) {
                if let Some(c) = place_if_missing(
                    g,
                    p,
                    BuildingType::PowerPlant,
                    base_offset(hq, 0, -4),
                    2,
                    &mut plan,
                ) {
                    out.push(c);
                }
            }
            if g.can_afford(p, building_stats(BuildingType::TeslaCoil).resource_cost) {
                if let Some(c) = place_if_missing(
                    g,
                    p,
                    BuildingType::TeslaCoil,
                    toward_enemy(hq, enemy, 3),
                    1,
                    &mut plan,
                ) {
                    out.push(c);
                }
            }
        }

        // --- Anti-air reaction ---
        if pol.aa {
            let enemy_air = g
                .units
                .iter()
                .filter(|u| u.owner == p.enemy() && unit_stats(u.utype).air)
                .count();
            if enemy_air > 0 {
                if g.research[p.index()].has(TechId::RocketPropulsion) {
                    if let Some(c) = train_up_to(
                        g,
                        p,
                        BuildingType::Factory,
                        UnitType::SamLauncher,
                        pol.sam_scale.min(enemy_air as u32).max(1) as usize,
                        &mut plan,
                    ) {
                        out.push(c);
                    }
                }
                if own_building(g, p, BuildingType::TechLab).is_some() {
                    if let Some(c) = place_if_missing(
                        g,
                        p,
                        BuildingType::AATurret,
                        toward_enemy(hq, enemy, 3),
                        2,
                        &mut plan,
                    ) {
                        out.push(c);
                    }
                }
            }
        }

        // --- Opening scout (windowed; no-op once the army exceeds 3) ---
        if pol.scout_until > 0 && g.turn <= pol.scout_until {
            out.extend(opening_scout(g, p));
        }

        // --- March ---
        let objective = if pol.defend { hq } else { enemy };
        let combat = combat_unit_ids(g, p);
        let committed = pol.committed_dist > 0
            && combat.iter().any(|&id| {
                g.unit(p, id).is_some_and(|u| {
                    chebyshev(u.tile.0, u.tile.1, objective.0, objective.1)
                        <= pol.committed_dist
                })
            });
        if pol.march_threshold == 0
            || combat.len() as i32 >= pol.march_threshold
            || committed
        {
            out.extend(army_orders(g, p, objective));
        }

        out.push(Command::EndTurn { player: p });
        out
    }
}

/// Public constructors. The historical tier names are kept (with unchanged
/// signatures) so the trainer, curriculum, balance baseline, gauntlet and
/// `bots.rs` acceptances all keep working — each is just an `AdaptiveBot` at
/// its anchor difficulty.
pub fn easy() -> AdaptiveBot {
    AdaptiveBot::new(D_EASY)
}

pub fn medium() -> AdaptiveBot {
    AdaptiveBot::new(D_MEDIUM)
}

pub fn hard() -> AdaptiveBot {
    AdaptiveBot::new(D_HARD)
}

/// Build an `AdaptiveBot` at an explicit difficulty (`0..=1`). The server
/// uses this for the `adaptive` / `adaptive:<scalar>` opponent.
pub fn adaptive(difficulty: f32) -> AdaptiveBot {
    AdaptiveBot::new(difficulty)
}