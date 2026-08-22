//! Deterministic scripted baseline bots: easy (turtle), medium (periodic
//! attack waves), hard (expand-and-push).
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

use crucible_sim::{
    building_stats, tech::prereqs_met, tech::tech_info, tech::TechId, tiles::chebyshev, unit_stats,
    Building, BuildingType, Command, EntityId, Game, Player, UnitType,
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
    pub(crate) ore_spent: i32,
    pub(crate) used_tiles: Vec<(u8, u8)>,
    /// Queue slots booked this batch, per building id.
    pub(crate) extra_queue: Vec<(EntityId, usize)>,
}

impl Plan {
    fn ore_left(&self, g: &Game, p: Player) -> i32 {
        g.ore[p.index()] - self.ore_spent
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
    g.buildings.iter().find(|b| b.owner == p && b.btype == bt)
}

fn count_buildings(g: &Game, p: Player, bt: BuildingType) -> usize {
    g.buildings
        .iter()
        .filter(|b| b.owner == p && b.btype == bt)
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
        // Refineries must touch their resource field: walk every ore (or
        // crystal) tile nearest to `preferred` first and try its free
        // 8-dir neighbors. Remote pockets are the expansion mechanic.
        let fields = if bt == BuildingType::Refinery {
            &g.map.ore
        } else {
            &g.map.crystal
        };
        let mut fields: Vec<(usize, (u8, u8))> = (0..crucible_sim::map::MAP_TILES)
            .filter(|&i| fields[i] > 0)
            .map(|i| (i, crucible_sim::map::tile_coords(i)))
            .collect();
        fields.sort_by_key(|&(idx, t)| (chebyshev(t.0, t.1, preferred.0, preferred.1), idx));
        for (_, field_tile) in fields {
            for &(dx, dy) in crucible_sim::orders::NEIGHBOR_OFFSETS.iter() {
                let x = field_tile.0 as i32 + dx;
                let y = field_tile.1 as i32 + dy;
                if !(0..64).contains(&x) || !(0..64).contains(&y) {
                    continue;
                }
                let t = (x as u8, y as u8);
                if free(t) {
                    return Some(t);
                }
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
                if !(0..64).contains(&x) || !(0..64).contains(&y) {
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
    let cost = building_stats(bt).cost;
    if plan.ore_left(g, p) < cost {
        return None;
    }
    let tile = find_build_tile(g, p, bt, preferred, plan)?;
    plan.ore_spent += cost;
    plan.used_tiles.push(tile);
    Some(Command::PlaceBuilding {
        player: p,
        btype: bt,
        tile,
    })
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
            .any(|b| b.owner == p && b.btype == BuildingType::TechLab && b.is_alive())
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
            b.owner == p && b.btype == producer && b.is_alive() && load(b) < g.config.max_queue
        })
        .min_by_key(|b| (load(b), b.id))?;
    let cost = unit_stats(ut).cost;
    if plan.ore_left(g, p) < cost {
        return None;
    }
    plan.ore_spent += cost;
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
        if tech_info(t).crystal_cost > g.crystal[p.index()] {
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
        (hq.0 as i32 + dx * dist).clamp(0, 63) as u8,
        (hq.1 as i32 + dy * dist).clamp(0, 63) as u8,
    )
}

/// Symmetrically orient building offsets toward the quadrant's natural ore pocket.
fn base_offset(hq: (u8, u8), dx: i32, dy: i32) -> (u8, u8) {
    let sx = if hq.0 < 32 { dx } else { -dx };
    let sy = if hq.1 < 32 { dy } else { -dy };
    (
        (hq.0 as i32 + sx).clamp(0, 63) as u8,
        (hq.1 as i32 + sy).clamp(0, 63) as u8,
    )
}

// ---------------------------------------------------------------------------
// Easy — passive turtle
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct EasyBot {
    /// Number of turrets placed so far; the turtle never rebuilds them, so
    /// sustained waves eventually break through.
    built_turrets: u8,
}

impl Bot for EasyBot {
    fn name(&self) -> &'static str {
        "easy"
    }

    fn decide(&mut self, g: &Game, p: Player) -> Vec<Command> {
        let mut out = Vec::new();
        let mut plan = Plan::default();
        let hq = g.hq(p).map(|b| b.tile).unwrap_or((8, 8));

        // Economy first: refinery + factory. The factory must precede the
        // barracks or the turtle never gets income.
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

        // Defense once income is running: a few infantry + enemy-facing
        // turrets slow the opening rush; the turtle otherwise sits still, so
        // these delay the inevitable rather than win. The turtle spends on
        // fortifications, not a march army — a true turtle never fields a
        // tank-heavy force that could rival a rusher's.
        if own_building(g, p, BuildingType::Factory).is_some() {
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
            // A small garrison: 4 infantry. The turtle's defense is its
            // turrets, not a field army — a garrison this size delays the
            // inevitable rush, it does not out-fight it.
            if let Some(c) = train_up_to(
                g,
                p,
                BuildingType::Barracks,
                UnitType::Infantry,
                4,
                &mut plan,
            ) {
                out.push(c);
            }
            // Turrets replace the army: one at turn 20, a second at 40, a
            // third at 60 — each ~150 HP of defense for 100 ore.
            if g.turn > 20 && self.built_turrets < 1 {
                let t = toward_enemy(hq, enemy_hq_tile(g, p), 2);
                if let Some(c) = place_if_missing(g, p, BuildingType::Turret, t, 1, &mut plan) {
                    out.push(c);
                }
            }
            if g.turn > 40 && self.built_turrets < 2 {
                let t = toward_enemy(hq, enemy_hq_tile(g, p), 3);
                if let Some(c) = place_if_missing(g, p, BuildingType::Turret, t, 2, &mut plan) {
                    out.push(c);
                }
            }
            if g.turn > 60 && self.built_turrets < 3 {
                let t = toward_enemy(hq, enemy_hq_tile(g, p), 4);
                if let Some(c) = place_if_missing(g, p, BuildingType::Turret, t, 3, &mut plan) {
                    out.push(c);
                }
            }
            self.built_turrets = self
                .built_turrets
                .max(count_buildings(g, p, BuildingType::Turret) as u8);
        }

        // A token tank for counter-attacks once invaders break in; nothing
        // more — the turtle's army must stay smaller than a rusher's.
        if let Some(c) = train_up_to(g, p, BuildingType::Factory, UnitType::Tank, 1, &mut plan) {
            out.push(c);
        }

        // Even the turtle fights back when an enemy stands in range —
        // otherwise invaders would chew through it for free.
        out.extend(army_orders(g, p, hq));
        out.push(Command::EndTurn { player: p });
        out
    }
}

/// Public constructor.
pub fn easy() -> EasyBot {
    EasyBot::default()
}

// ---------------------------------------------------------------------------
// Medium — periodic attack waves
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct MediumBot {
    // The rush bot presses from the moment its first wave is ready; there is
    // no interval gate — with tile-based movement and an 80-turn cap, a
    // 20–30 turn "wave" cadence meant armies never met (matches timed out
    // with zero combat). Continuous pressure is the wave: each turn the army
    // advances or strikes, and fresh trains reinforce it.
}

impl Bot for MediumBot {
    fn name(&self) -> &'static str {
        "medium"
    }

    fn decide(&mut self, g: &Game, p: Player) -> Vec<Command> {
        let mut out = Vec::new();
        let mut plan = Plan::default();
        let hq = g.hq(p).map(|b| b.tile).unwrap_or((8, 8));

        // Rush economy: refinery first (the turtle does the same — a rush
        // needs income to sustain the wave), but the spending priorities
        // diverge: barracks + factory over turrets, and every spare ore is
        // an infantry body, not a fortification.
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
        // Late-game siege: a TechLab once the first wave is on the march, so
        // wave two brings artillery (range 3, 110 dmg) that out-trades the
        // turtle's turrets (12 dmg) and cracks a base infantry alone cannot.
        // The lab also starts the research tree; the rocket troopers it
        // unlocks reinforce later waves.
        if g.turn > 30 {
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
        // The medium never researches: its identity is cheap continuous
        // pressure, and the wave needs every ore point as a body, not a tech.
        // (Hard is the tech pusher and owns the research tree.)

        // The wave: infantry bodies (cheap, 1-turn build) as the hammer,
        // tanks as the anvil once the infantry train is running.
        if let Some(c) = train_up_to(
            g,
            p,
            BuildingType::Barracks,
            UnitType::Infantry,
            10,
            &mut plan,
        ) {
            out.push(c);
        }
        // Artillery out-prioritizes tanks in the factory queue once the
        // TechLab is up: it is the siege tool that cracks a turtled base
        // (range 3, 110 dmg vs the turret's 12), and if tank orders run
        // first the queue never gets around to it. Tanks still join, but
        // only once the artillery train is satisfied.
        if own_building(g, p, BuildingType::TechLab).is_some() && g.turn > 35 {
            if let Some(c) = train_up_to(
                g,
                p,
                BuildingType::Factory,
                UnitType::Artillery,
                3,
                &mut plan,
            ) {
                out.push(c);
            }
        }
        if count_units(g, p, UnitType::Infantry) >= 4 {
            if let Some(c) = train_up_to(g, p, BuildingType::Factory, UnitType::Tank, 4, &mut plan)
            {
                out.push(c);
            }
        }

        // Concentrated waves: hold the force at home until it reaches the
        // wave threshold, then march the whole group together. Trickling one
        // unit at a time into a defended base just feeds the turrets; a
        // concentrated arrival overwhelms them. Once a wave is committed
        // (any unit near the enemy base), it keeps pressing to the death —
        // freezing survivors mid-assault below the threshold just lets the
        // turtle rebuild for free. The next wave builds up only after this
        // one is spent.
        let objective = enemy_hq_tile(g, p);
        let combat = combat_unit_ids(g, p);
        let committed = combat.iter().any(|&id| {
            g.unit(p, id)
                .is_some_and(|u| chebyshev(u.tile.0, u.tile.1, objective.0, objective.1) <= 12)
        });
        if combat.len() >= 8 || committed {
            out.extend(army_orders(g, p, objective));
        }
        out.push(Command::EndTurn { player: p });
        out
    }
}

/// Public constructor.
pub fn medium() -> MediumBot {
    MediumBot::default()
}

// ---------------------------------------------------------------------------
// Hard — expand-and-push
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct HardBot {
    // Same continuous-pressure rationale as MediumBot: turn-based movement
    // made the old 20-turn interval gate stall armies until timeout.
}

impl Bot for HardBot {
    fn name(&self) -> &'static str {
        "hard"
    }

    fn decide(&mut self, g: &Game, p: Player) -> Vec<Command> {
        let mut out = Vec::new();
        let mut plan = Plan::default();
        let hq = g.hq(p).map(|b| b.tile).unwrap_or((8, 8));

        // Core production buildings.
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

        // Early army: 8 infantry + 6 tanks. Once Rocket Propulsion lands,
        // a couple of rocket troopers (tech-gated in `train_up_to`) join the
        // barracks queue — they out-trade armor without diluting it.
        if let Some(c) = train_up_to(
            g,
            p,
            BuildingType::Barracks,
            UnitType::Infantry,
            8,
            &mut plan,
        ) {
            out.push(c);
        }
        if let Some(c) = train_up_to(
            g,
            p,
            BuildingType::Barracks,
            UnitType::RocketTrooper,
            2,
            &mut plan,
        ) {
            out.push(c);
        }
        if let Some(c) = train_up_to(g, p, BuildingType::Factory, UnitType::Tank, 6, &mut plan) {
            out.push(c);
        }

        // One early turret to blunt the opening rush: hard must survive the
        // wave before its tech and dual factories come online. A single
        // enemy-facing turret (range 3) peels one attacker per turn off the
        // march — cheap insurance against the one build order that beats a
        // slow expand.
        if g.turn > 15 {
            let t = toward_enemy(hq, enemy_hq_tile(g, p), 3);
            if let Some(c) = place_if_missing(g, p, BuildingType::Turret, t, 1, &mut plan) {
                out.push(c);
            }
        }

        // Tech Lab & the research tree (damage first, then the rocket
        // unlocks). `research_next` fires every turn research is idle, so the
        // lab keeps working through the whole tree.
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
        // Hard is the tech pusher: research runs from the moment the lab is
        // up, powering the whole tree by the late game.
        if let Some(c) = research_next(g, p) {
            out.push(c);
        }

        // Dual Factory mass production.
        if own_building(g, p, BuildingType::TechLab).is_some() {
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

        // Second-tier tech once the army is fielded and the bank allows it
        // (a Tesla Coil guards the base, with a second PowerPlant paying its
        // bill; mammoth tanks form the late-game siege core). The turn gates
        // keep the tech spend from starving the massed army, and keep the
        // hard benchmark exercising the full tech tree.
        if own_building(g, p, BuildingType::TechLab).is_some() && g.turn > 45 {
            // Second PowerPlant once the bank allows (pays the coil's bill).
            if g.ore[p.index()] >= 300 {
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
            // Tesla Coil guard once the bank can absorb a 250 ore spend.
            if g.ore[p.index()] >= 350 {
                if let Some(c) = place_if_missing(
                    g,
                    p,
                    BuildingType::TeslaCoil,
                    toward_enemy(hq, enemy_hq_tile(g, p), 3),
                    1,
                    &mut plan,
                ) {
                    out.push(c);
                }
            }
        }
        if own_building(g, p, BuildingType::TechLab).is_some()
            && g.turn > 80
            && g.ore[p.index()] >= 300
        {
            if let Some(c) = train_up_to(
                g,
                p,
                BuildingType::Factory,
                UnitType::MammothTank,
                2,
                &mut plan,
            ) {
                out.push(c);
            }
        }

        // Mass late-game armor: 14 tanks + 4 artillery.
        if let Some(c) = train_up_to(g, p, BuildingType::Factory, UnitType::Tank, 14, &mut plan) {
            out.push(c);
        }
        if let Some(c) = train_up_to(
            g,
            p,
            BuildingType::Factory,
            UnitType::Artillery,
            4,
            &mut plan,
        ) {
            out.push(c);
        }

        // Tactical push: advance or strike every turn once a minimum army is
        // fielded (continuous pressure; see MediumBot for why).
        let combat = combat_unit_ids(g, p).len();
        if combat >= 4 {
            out.extend(army_orders(g, p, enemy_hq_tile(g, p)));
        }
        out.push(Command::EndTurn { player: p });
        out
    }
}

/// Public constructor.
pub fn hard() -> HardBot {
    HardBot::default()
}
