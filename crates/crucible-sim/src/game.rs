//! Match orchestration: state container, the turn lifecycle, command
//! application, action budget, win check, and helpers shared by the other
//! modules.
//!
//! The game is strictly alternating-turn: only `self.active` may issue
//! commands; they execute immediately. [`Command::EndTurn`] runs the
//! end-of-turn resolution (turret fire → sweep → fog → win check) and then
//! the opponent's start-of-turn (income → production → unit resets → fog).

use serde::{Deserialize, Serialize};

use crate::entity::{
    building_stats, unit_stats, Building, BuildingType, EntityId, Player, Unit, UnitType, Upgrade,
    HQ_INCOME_PER_TURN, REFINERY_ORE_PER_TURN, REPAIR_COST_DEN, REPAIR_COST_NUM, REPAIR_HP_DEN,
    REPAIR_HP_NUM, REPAIR_MIN_COST,
};
use crate::map::Map;
use crate::orders::{Command, CommandError, NEIGHBOR_OFFSETS};
use crate::tiles::within_range;

/// Runtime-tunable match settings. Tests override via a builder.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameConfig {
    pub max_queue: usize,
    /// Actions each player may issue per own turn (`EndTurn` is free).
    pub actions_per_turn: i32,
    pub starting_ore: i32,
    /// Fraction of a building's cost refunded on sell (50/100 = 50%).
    pub sell_refund_num: i32,
    pub sell_refund_den: i32,
    /// Match length cap in turns. `<= 0` disables the timeout entirely (live
    /// matches are unlimited; training matches set an explicit cap so
    /// degenerate self-play games cannot run forever).
    pub timeout_turns: i32,
}

impl Default for GameConfig {
    fn default() -> Self {
        GameConfig {
            max_queue: 5,
            actions_per_turn: 16,
            starting_ore: 450,
            sell_refund_num: 1,
            sell_refund_den: 2,
            timeout_turns: crate::MATCH_TIMEOUT_TURNS,
        }
    }
}

/// Per-turn action budget (replaces the old realtime APM token bucket).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionBudget {
    spent: i32,
    cap: i32,
}

impl ActionBudget {
    fn new(cap: i32) -> Self {
        ActionBudget { spent: 0, cap }
    }

    pub(crate) fn reset(&mut self) {
        self.spent = 0;
    }

    pub(crate) fn try_spend(&mut self) -> bool {
        if self.spent < self.cap {
            self.spent += 1;
            true
        } else {
            false
        }
    }

    pub fn spent(&self) -> i32 {
        self.spent
    }

    pub fn cap(&self) -> i32 {
        self.cap
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum WinReason {
    HqDestroyed,
    Timeout,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum EventKind {
    BuildingPlaced {
        player: Player,
        btype: BuildingType,
        tile: (u8, u8),
    },
    UnitTrained {
        player: Player,
        utype: UnitType,
        tile: (u8, u8),
    },
    UnitDied {
        id: EntityId,
        owner: Player,
    },
    BuildingDestroyed {
        id: EntityId,
        owner: Player,
    },
    OreMined {
        player: Player,
        amount: i32,
    },
    Attacked {
        attacker: EntityId,
        target: EntityId,
        damage: i32,
        /// Whether the defender's counterattack also resolved.
        countered: bool,
    },
    Sold {
        player: Player,
        btype: BuildingType,
        refund: i32,
    },
    UpgradeChosen {
        player: Player,
        upgrade: Upgrade,
    },
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct GameEvent {
    pub turn: i32,
    pub kind: EventKind,
}

/// The complete match state. The only mutable root of the simulation.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Game {
    pub config: GameConfig,
    pub map: Map,
    pub buildings: Vec<Building>,
    pub units: Vec<Unit>,
    pub ore: [i32; 2],
    pub upgrades: [Upgrade; 2],
    /// Current turn number, starting at 1.
    pub turn: i32,
    /// Whose turn it is.
    pub active: Player,
    pub winner: Option<Player>,
    pub win_reason: Option<WinReason>,
    /// Whether the match has reached a terminal result. Kept separate from
    /// `winner` so a legitimate draw can end the match without naming a side.
    #[serde(default)]
    pub over: bool,
    pub budgets: [ActionBudget; 2],
    pub next_id: EntityId,
    pub events: Vec<GameEvent>,
    /// Number of commands dropped by the action budget, per player.
    pub dropped_commands: [u32; 2],
    /// Per-player fog-of-war memory.
    pub fog: [crate::fog::FogMemory; 2],
}

impl Game {
    pub fn new(map: Map, config: GameConfig) -> Self {
        let mut next_id = 1u32;
        let mut buildings = Vec::new();
        for (p, tile) in Player::ALL.iter().zip(map.hq_tiles.iter()) {
            let stats = building_stats(BuildingType::Hq);
            buildings.push(Building {
                id: next_id,
                owner: *p,
                btype: BuildingType::Hq,
                tile: *tile,
                hp: stats.hp,
                max_hp: stats.hp,
                queue: Vec::new(),
                progress: 0,
                cooldown: 0,
                repaired_this_turn: false,
            });
            next_id += 1;
        }
        Game {
            config: config.clone(),
            map,
            buildings,
            units: Vec::new(),
            ore: [config.starting_ore; 2],
            upgrades: [Upgrade::None; 2],
            turn: 1,
            active: Player::P0,
            winner: None,
            win_reason: None,
            over: false,
            budgets: [
                ActionBudget::new(config.actions_per_turn),
                ActionBudget::new(config.actions_per_turn),
            ],
            next_id,
            events: Vec::new(),
            dropped_commands: [0, 0],
            fog: [
                crate::fog::FogMemory::default(),
                crate::fog::FogMemory::default(),
            ],
        }
    }

    // -- lookups -------------------------------------------------------------

    pub fn alloc_id(&mut self) -> EntityId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn unit(&self, player: Player, id: EntityId) -> Option<&Unit> {
        self.units.iter().find(|u| u.id == id && u.owner == player)
    }

    pub fn unit_mut(&mut self, player: Player, id: EntityId) -> Option<&mut Unit> {
        self.units
            .iter_mut()
            .find(|u| u.id == id && u.owner == player)
    }

    pub fn building(&self, player: Player, id: EntityId) -> Option<&Building> {
        self.buildings
            .iter()
            .find(|b| b.id == id && b.owner == player)
    }

    pub fn building_mut(&mut self, player: Player, id: EntityId) -> Option<&mut Building> {
        self.buildings
            .iter_mut()
            .find(|b| b.id == id && b.owner == player)
    }

    /// Find any building (any owner) by id.
    pub fn any_building(&self, id: EntityId) -> Option<&Building> {
        self.buildings.iter().find(|b| b.id == id)
    }

    /// Find any unit (any owner) by id.
    pub fn any_unit(&self, id: EntityId) -> Option<&Unit> {
        self.units.iter().find(|u| u.id == id)
    }

    pub fn building_at(&self, tile: (u8, u8)) -> Option<EntityId> {
        self.buildings.iter().find(|b| b.tile == tile).map(|b| b.id)
    }

    /// The living unit occupying a tile, if any.
    pub fn unit_at(&self, tile: (u8, u8)) -> Option<EntityId> {
        self.units
            .iter()
            .find(|u| u.is_alive() && u.tile == tile)
            .map(|u| u.id)
    }

    /// The HQ building of a player, if alive.
    pub fn hq(&self, player: Player) -> Option<&Building> {
        self.buildings
            .iter()
            .find(|b| b.owner == player && b.btype == BuildingType::Hq)
    }

    pub fn push_event(&mut self, kind: EventKind) {
        self.events.push(GameEvent {
            turn: self.turn,
            kind,
        });
    }

    /// Per-tile blocked overlay: tiles occupied by a building (terrain
    /// passability is separate, in [`Map::passable`]).
    pub fn blocked_grid(&self) -> Vec<bool> {
        let mut b = vec![false; crate::map::MAP_TILES];
        for x in &self.buildings {
            b[crate::map::tile_index(x.tile.0, x.tile.1)] = true;
        }
        b
    }

    // -- power ---------------------------------------------------------------

    /// Return (power_produced, power_consumed) for a given player.
    pub fn power(&self, player: Player) -> (i32, i32) {
        let mut produced = 0;
        let mut consumed = 0;
        for b in &self.buildings {
            if b.owner == player && b.is_alive() {
                let stats = building_stats(b.btype);
                if stats.power > 0 {
                    produced += stats.power;
                } else if stats.power < 0 {
                    consumed += -stats.power;
                }
            }
        }
        (produced, consumed)
    }

    /// True if a player's power consumption exceeds their power production.
    pub fn has_low_power(&self, player: Player) -> bool {
        let (prod, cons) = self.power(player);
        cons > prod
    }

    // -- command application ---------------------------------------------------

    /// Validate and apply a batch of commands for one player. Returns a
    /// per-command result so the server can report exactly which were dropped
    /// and why. Commands are applied in the order given; the action budget is
    /// shared across the batch.
    pub fn apply_commands(
        &mut self,
        player: Player,
        cmds: &[Command],
    ) -> Vec<Result<(), CommandError>> {
        cmds.iter().map(|cmd| self.apply_one(player, cmd)).collect()
    }

    fn apply_one(&mut self, player: Player, cmd: &Command) -> Result<(), CommandError> {
        self.validate_command(cmd)?;
        if cmd.costs_action() && !self.budgets[player.index()].try_spend() {
            self.dropped_commands[player.index()] += 1;
            return Err(CommandError::RateLimited);
        }
        self.execute(player, cmd.clone());
        Ok(())
    }

    fn execute(&mut self, player: Player, cmd: Command) {
        match cmd {
            Command::PlaceBuilding { btype, tile, .. } => {
                let stats = building_stats(btype);
                self.ore[player.index()] -= stats.cost;
                let id = self.alloc_id();
                self.buildings.push(Building {
                    id,
                    owner: player,
                    btype,
                    tile,
                    hp: stats.hp,
                    max_hp: stats.hp,
                    queue: Vec::new(),
                    progress: 0,
                    cooldown: 0,
                    repaired_this_turn: false,
                });
                self.push_event(EventKind::BuildingPlaced {
                    player,
                    btype,
                    tile,
                });
                self.fog_phase();
            }
            Command::TrainUnit {
                building, utype, ..
            } => {
                let stats = unit_stats(utype);
                self.ore[player.index()] -= stats.cost;
                if let Some(b) = self.building_mut(player, building) {
                    b.queue.push(utype);
                }
            }
            Command::MoveGroup {
                units, waypoint, ..
            } => {
                self.execute_move_group(player, &units, waypoint);
                self.fog_phase();
            }
            Command::Attack { units, target, .. } => {
                self.execute_attack(player, &units, target);
                self.sweep_dead();
                self.check_win();
                self.fog_phase();
            }
            Command::ChooseUpgrade { lab, upgrade, .. } => {
                if self.building(player, lab).is_some() {
                    self.upgrades[player.index()] = upgrade;
                    self.push_event(EventKind::UpgradeChosen { player, upgrade });
                }
            }
            Command::Sell { building, .. } => {
                let btype = self.building(player, building).map(|b| b.btype);
                if let Some(bt) = btype {
                    let stats = building_stats(bt);
                    let refund =
                        stats.cost * self.config.sell_refund_num / self.config.sell_refund_den;
                    self.ore[player.index()] += refund;
                    self.push_event(EventKind::Sold {
                        player,
                        btype: bt,
                        refund,
                    });
                    let id = building;
                    self.buildings.retain(|b| b.id != id);
                    self.fog_phase();
                }
            }
            Command::Repair { building, .. } => {
                let cost = self.repair_cost(building).unwrap_or(REPAIR_MIN_COST);
                if self.ore[player.index()] >= cost {
                    if let Some(b) = self
                        .buildings
                        .iter_mut()
                        .find(|b| b.id == building && b.owner == player && b.is_alive())
                    {
                        if !b.repaired_this_turn && b.hp < b.max_hp {
                            self.ore[player.index()] -= cost;
                            b.repaired_this_turn = true;
                            b.hp = (b.hp + b.max_hp * REPAIR_HP_NUM / REPAIR_HP_DEN).min(b.max_hp);
                        }
                    }
                }
            }
            Command::EndTurn { .. } => {
                self.end_turn();
            }
        }
    }

    /// The ore cost of repairing `building` this turn (its remaining missing
    /// HP determines the charge); `None` when it cannot be repaired.
    pub fn repair_cost(&self, building: EntityId) -> Option<i32> {
        let b = self.any_building(building)?;
        if !b.is_alive() || b.hp >= b.max_hp {
            return None;
        }
        let cost = building_stats(b.btype).cost * REPAIR_COST_NUM / REPAIR_COST_DEN;
        Some(cost.max(REPAIR_MIN_COST))
    }

    // -- movement ------------------------------------------------------------

    fn execute_move_group(&mut self, player: Player, units: &[EntityId], waypoint: (u8, u8)) {
        let blocked = self.blocked_grid();
        // The ordered waypoint is often a building (e.g. the enemy HQ, a
        // refinery to seize): `find_path` refuses a blocked destination, so
        // retarget to the nearest free tile beside it — armies march *next
        // to* structures, never onto them.
        let target = if blocked[crate::map::tile_index(waypoint.0, waypoint.1)] {
            self.nearest_free_tile(waypoint).unwrap_or(waypoint)
        } else {
            waypoint
        };
        for &id in units {
            let Some(u) = self.unit(player, id) else {
                continue;
            };
            if !u.is_alive() || u.mp <= 0 || u.moved {
                continue;
            }
            let fly = unit_stats(u.utype).air;
            let Some(path) = self.map.find_path(u.tile, target, &blocked, fly) else {
                continue;
            };
            // Walk as far as MP allows, stopping before any occupied tile.
            let mut steps = 0usize;
            for &(tx, ty) in path.iter() {
                if steps as i32 >= u.mp {
                    break;
                }
                if self.unit_at((tx, ty)).is_some() {
                    break;
                }
                steps += 1;
            }
            if steps > 0 {
                let dest = path[steps - 1];
                if let Some(u) = self.unit_mut(player, id) {
                    u.tile = dest;
                    u.mp -= steps as i32;
                    u.moved = true;
                }
            }
        }
    }

    // -- combat ----------------------------------------------------------------

    /// Effective attack damage of a unit after its research upgrade.
    pub(crate) fn effective_damage(&self, utype: UnitType, owner: Player, hp: i32) -> i32 {
        let stats = unit_stats(utype);
        let dmg = match self.upgrades[owner.index()] {
            Upgrade::Damage => stats.damage * 5 / 4,
            _ => stats.damage,
        };
        // Advance-Wars scaling: damaged units deal proportionally less.
        dmg * hp / stats.hp.max(1)
    }

    /// Effective range of a unit after its research upgrade.
    pub(crate) fn effective_range(&self, utype: UnitType, owner: Player) -> i32 {
        let stats = unit_stats(utype);
        match self.upgrades[owner.index()] {
            Upgrade::Range => stats.range_tiles + 1,
            _ => stats.range_tiles,
        }
    }

    fn execute_attack(&mut self, player: Player, units: &[EntityId], target: EntityId) {
        // Snapshot the target's position once; it cannot move mid-resolution.
        let enemy = player.enemy();
        let target_tile = self
            .units
            .iter()
            .find(|u| u.id == target && u.owner == enemy && u.is_alive())
            .map(|u| u.tile)
            .or_else(|| {
                self.buildings
                    .iter()
                    .find(|b| b.id == target && b.owner == enemy && b.is_alive())
                    .map(|b| b.tile)
            });
        let Some(target_tile) = target_tile else {
            return;
        };

        for &id in units {
            let Some(u) = self.unit(player, id) else {
                continue;
            };
            if !u.is_alive() || u.acted {
                continue;
            }
            let (utype, owner, hp, max_hp, tile) = (u.utype, u.owner, u.hp, u.max_hp, u.tile);
            let stats = unit_stats(utype);
            let range = self.effective_range(utype, owner);
            let d = crate::tiles::chebyshev(tile.0, tile.1, target_tile.0, target_tile.1);
            if d > range || d < stats.min_range_tiles {
                continue; // out of this unit's envelope; skips without spending
            }

            // Primary strike.
            let dmg = self.effective_damage(utype, owner, hp);
            self.apply_damage(target, dmg);

            // Counterattack: surviving direct-combat defenders strike back at
            // the attacker. Turrets never counter here (they auto-fire on
            // their own turn); artillery never counters (out of its min range
            // envelope unless the attacker stood inside it — handled by the
            // same range check below).
            let mut countered = false;
            let defender_is_unit = self.units.iter().any(|x| x.id == target && x.is_alive());
            if defender_is_unit {
                if let Some(d) = self.any_unit(target) {
                    let dstats = unit_stats(d.utype);
                    let drange = self.effective_range(d.utype, d.owner);
                    let dd = crate::tiles::chebyshev(d.tile.0, d.tile.1, tile.0, tile.1);
                    if dd <= drange && dd >= dstats.min_range_tiles {
                        let cdmg = self.effective_damage(d.utype, d.owner, d.hp);
                        self.apply_damage(id, cdmg);
                        countered = true;
                    }
                }
            }

            if let Some(u) = self.unit_mut(player, id) {
                u.acted = true;
            }
            self.push_event(EventKind::Attacked {
                attacker: id,
                target,
                damage: dmg,
                countered,
            });
            let _ = max_hp;
        }
    }

    /// Apply damage to a unit or building by id. Deaths are marked (hp <= 0);
    /// actual removal happens in `sweep_dead`.
    fn apply_damage(&mut self, id: EntityId, amount: i32) {
        if let Some(u) = self.units.iter_mut().find(|u| u.id == id) {
            u.hp -= amount;
            return;
        }
        if let Some(b) = self.buildings.iter_mut().find(|b| b.id == id) {
            b.hp -= amount;
        }
    }

    /// Remove dead entities (ascending id order preserved by retain).
    pub(crate) fn sweep_dead(&mut self) {
        let dead_units: Vec<EntityId> = self
            .units
            .iter()
            .filter(|u| !u.is_alive())
            .map(|u| u.id)
            .collect();
        for id in dead_units {
            if let Some(u) = self.units.iter().find(|u| u.id == id) {
                self.push_event(EventKind::UnitDied { id, owner: u.owner });
            }
        }
        self.units.retain(|u| u.is_alive());

        let dead_buildings: Vec<EntityId> = self
            .buildings
            .iter()
            .filter(|b| !b.is_alive())
            .map(|b| b.id)
            .collect();
        for id in dead_buildings {
            if let Some(b) = self.buildings.iter().find(|b| b.id == id) {
                self.push_event(EventKind::BuildingDestroyed { id, owner: b.owner });
            }
        }
        self.buildings.retain(|b| b.is_alive());
    }

    // -- turn lifecycle ---------------------------------------------------------

    /// End the active player's turn and run the full lifecycle:
    /// turret fire → sweep → fog → win check → opponent start-of-turn
    /// (income → production → resets → fog).
    pub fn end_turn(&mut self) {
        if self.is_over() {
            return;
        }
        let finished = self.active;

        // 1. Turrets of the finishing player fire once each (ascending id),
        //    targeting the lowest-id enemy in range.
        let turret_ids: Vec<EntityId> = self
            .buildings
            .iter()
            .filter(|b| b.owner == finished && b.is_alive())
            .filter(|b| building_stats(b.btype).damage > 0)
            .map(|b| b.id)
            .collect();
        for tid in turret_ids {
            let Some(b) = self.buildings.iter().find(|b| b.id == tid) else {
                continue;
            };
            let stats = building_stats(b.btype);
            let enemy = b.owner.enemy();
            let victim = self
                .units
                .iter()
                .filter(|u| u.owner == enemy && u.is_alive())
                .find(|u| within_range(b.tile.0, b.tile.1, u.tile.0, u.tile.1, stats.range_tiles))
                .map(|u| u.id)
                .or_else(|| {
                    self.buildings
                        .iter()
                        .filter(|x| x.owner == enemy && x.is_alive() && x.btype != BuildingType::Hq)
                        .find(|x| {
                            within_range(b.tile.0, b.tile.1, x.tile.0, x.tile.1, stats.range_tiles)
                        })
                        .map(|x| x.id)
                })
                // The HQ itself is a valid target when nothing else stands in
                // range — sieges must be able to finish the game.
                .or_else(|| {
                    self.buildings
                        .iter()
                        .filter(|x| x.owner == enemy && x.is_alive() && x.btype == BuildingType::Hq)
                        .find(|x| {
                            within_range(b.tile.0, b.tile.1, x.tile.0, x.tile.1, stats.range_tiles)
                        })
                        .map(|x| x.id)
                });
            if let Some(victim) = victim {
                self.apply_damage(victim, stats.damage);
                self.push_event(EventKind::Attacked {
                    attacker: tid,
                    target: victim,
                    damage: stats.damage,
                    countered: false,
                });
            }
        }
        self.sweep_dead();
        self.fog_phase();
        self.check_win();
        if self.is_over() {
            return;
        }

        // 2. Hand over to the opponent and run their start-of-turn.
        self.active = finished.enemy();
        self.turn += 1;
        self.start_of_turn(self.active);
    }

    /// Start-of-turn resolution for `player`: income, production, resets.
    fn start_of_turn(&mut self, player: Player) {
        // Income: HQ trickle + refineries draining adjacent ore tiles.
        self.ore[player.index()] += HQ_INCOME_PER_TURN;
        let refinery_ids: Vec<EntityId> = self
            .buildings
            .iter()
            .filter(|b| b.owner == player && b.is_alive() && b.btype == BuildingType::Refinery)
            .map(|b| b.id)
            .collect();
        for rid in refinery_ids {
            let Some(b) = self.buildings.iter().find(|b| b.id == rid) else {
                continue;
            };
            let (bx, by) = b.tile;
            // Adjacent ore tiles in ascending tile-index order.
            let mut adjacent: Vec<(u8, u8)> = NEIGHBOR_OFFSETS
                .iter()
                .filter_map(|&(dx, dy)| {
                    let (x, y) = (bx as i32 + dx, by as i32 + dy);
                    if x >= 0
                        && y >= 0
                        && (x as usize) < crate::map::MAP_SIZE
                        && (y as usize) < crate::map::MAP_SIZE
                        && self.map.ore_at(x as u8, y as u8) > 0
                    {
                        Some((x as u8, y as u8))
                    } else {
                        None
                    }
                })
                .collect();
            adjacent.sort_by_key(|t| crate::map::tile_index(t.0, t.1));
            let mut mined = 0;
            for t in adjacent {
                if mined >= REFINERY_ORE_PER_TURN {
                    break;
                }
                mined += self
                    .map
                    .deplete_ore(t.0, t.1, REFINERY_ORE_PER_TURN - mined);
            }
            if mined > 0 {
                self.ore[player.index()] += mined;
                self.push_event(EventKind::OreMined {
                    player,
                    amount: mined,
                });
            }
        }

        // Production: queues advance one step per turn; low power halves the
        // rate (advance only on the player's own even-numbered turns),
        // preserving the 50%-speed semantic *symmetrically*. P0's start-of-
        // turn lands on odd global turns and P1's on even, so a global
        // `turn % 2` check would freeze one side entirely under low power.
        let low_power = self.has_low_power(player);
        let own_turn_count = (self.turn + player.index() as i32) / 2;
        let advances = !low_power || own_turn_count % 2 == 0;
        if advances {
            let producer_ids: Vec<EntityId> = self
                .buildings
                .iter()
                .filter(|b| b.owner == player && b.is_alive() && !b.queue.is_empty())
                .map(|b| b.id)
                .collect();
            for bid in producer_ids {
                let Some(item) = self
                    .building(player, bid)
                    .and_then(|b| b.queue.first().copied())
                else {
                    continue;
                };
                let build_time = unit_stats(item).build_time_turns;
                let (tile, _progress, done) = {
                    let Some(b) = self.building_mut(player, bid) else {
                        continue;
                    };
                    b.progress += 1;
                    if b.progress < build_time {
                        (b.tile, b.progress, false)
                    } else {
                        (b.tile, b.progress, true)
                    }
                };
                if !done {
                    continue;
                }
                // Spawn on a free adjacent tile.
                let Some(spawn_tile) = self.pick_spawn_tile(tile) else {
                    // Nowhere to place: hold at completion (retry next turn).
                    if let Some(b) = self.building_mut(player, bid) {
                        b.progress = build_time;
                    }
                    continue;
                };
                if let Some(b) = self.building_mut(player, bid) {
                    b.progress = 0;
                    b.queue.remove(0);
                }
                self.spawn_unit(player, item, spawn_tile, None);
            }
        }

        // Reset repair flags.
        for b in self.buildings.iter_mut() {
            if b.owner == player {
                b.repaired_this_turn = false;
            }
        }

        // Reset the acting player's budget and units.
        self.budgets[player.index()].reset();
        for u in self.units.iter_mut() {
            if u.owner == player {
                u.mp = unit_stats(u.utype).mp;
                u.moved = false;
                u.acted = false;
            }
        }

        self.fog_phase();
    }

    /// A free passable tile adjacent to `tile` for spawning a produced unit.
    /// Deterministic: ascending tile-index order.
    pub(crate) fn pick_spawn_tile(&self, tile: (u8, u8)) -> Option<(u8, u8)> {
        let mut candidates: Vec<(u8, u8)> = NEIGHBOR_OFFSETS
            .iter()
            .filter_map(|&(dx, dy)| {
                let (x, y) = (tile.0 as i32 + dx, tile.1 as i32 + dy);
                if x >= 0
                    && y >= 0
                    && (x as usize) < crate::map::MAP_SIZE
                    && (y as usize) < crate::map::MAP_SIZE
                {
                    Some((x as u8, y as u8))
                } else {
                    None
                }
            })
            .collect();
        candidates.sort_by_key(|t| crate::map::tile_index(t.0, t.1));
        candidates.into_iter().find(|&t| {
            self.map.is_passable(t.0, t.1)
                && self.building_at(t).is_none()
                && self.unit_at(t).is_none()
        })
    }

    pub(crate) fn spawn_unit(
        &mut self,
        owner: Player,
        utype: UnitType,
        tile: (u8, u8),
        _rally: Option<(u8, u8)>,
    ) {
        let stats = unit_stats(utype);
        let id = self.alloc_id();
        self.units.push(Unit {
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
        self.push_event(EventKind::UnitTrained {
            player: owner,
            utype,
            tile,
        });
    }

    /// The nearest passable, unblocked, unit-free tile to `t` (ties broken by
    /// ascending tile index), for marches that must stop beside a structure.
    fn nearest_free_tile(&self, t: (u8, u8)) -> Option<(u8, u8)> {
        let mut best: Option<(i32, usize, (u8, u8))> = None; // (dist, tile_idx, tile)
        for r in 0..=3i32 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dy.abs() != r {
                        continue;
                    }
                    let (x, y) = (t.0 as i32 + dx, t.1 as i32 + dy);
                    if x < 0
                        || y < 0
                        || x >= crate::map::MAP_SIZE as i32
                        || y >= crate::map::MAP_SIZE as i32
                    {
                        continue;
                    }
                    let tt = (x as u8, y as u8);
                    if self.map.is_passable(tt.0, tt.1)
                        && self.building_at(tt).is_none()
                        && self.unit_at(tt).is_none()
                    {
                        let idx = crate::map::tile_index(tt.0, tt.1);
                        if best.is_none_or(|(bd, bi, _)| r < bd || (r == bd && idx < bi)) {
                            best = Some((r, idx, tt));
                        }
                    }
                }
            }
        }
        best.map(|(_, _, tt)| tt)
    }

    // -- win condition -----------------------------------------------------------

    /// True once the match has ended (timeout or HQ destruction).
    pub fn is_over(&self) -> bool {
        // `winner` keeps snapshots produced before the explicit draw marker
        // replayable: those snapshots ended only when a winner was present.
        self.over || self.winner.is_some()
    }

    /// Remaining **military** value for timeout scoring: the cost of every
    /// living unit and building. Banked ore is deliberately excluded — it
    /// cannot win the game, and counting it rewards hoarding (a turtle that
    /// never fights beats an army-builder at timeout). The stronger fielded
    /// force is the decisive side.
    pub fn remaining_value(&self, player: Player) -> i32 {
        let mut value = 0;
        for u in &self.units {
            if u.owner == player {
                value += unit_stats(u.utype).cost;
            }
        }
        for b in &self.buildings {
            if b.owner == player {
                value += building_stats(b.btype).cost;
            }
        }
        value
    }

    /// Check and record the win condition. Call after every combat resolution
    /// and at end of turn.
    pub fn check_win(&mut self) {
        if self.is_over() {
            return;
        }
        // HQ destroyed?
        let p0_dead = self.hq(Player::P0).is_none();
        let p1_dead = self.hq(Player::P1).is_none();
        if p0_dead && p1_dead {
            self.winner = None;
            self.win_reason = Some(WinReason::HqDestroyed);
            self.over = true;
            return;
        }
        if p0_dead {
            self.winner = Some(Player::P1);
            self.win_reason = Some(WinReason::HqDestroyed);
            self.over = true;
            return;
        }
        if p1_dead {
            self.winner = Some(Player::P0);
            self.win_reason = Some(WinReason::HqDestroyed);
            self.over = true;
            return;
        }
        // Timeout? (`timeout_turns <= 0` means no limit.)
        if self.config.timeout_turns > 0 && self.turn > self.config.timeout_turns {
            let v0 = self.remaining_value(Player::P0);
            let v1 = self.remaining_value(Player::P1);
            self.win_reason = Some(WinReason::Timeout);
            self.winner = match v0.cmp(&v1) {
                std::cmp::Ordering::Greater => Some(Player::P0),
                std::cmp::Ordering::Less => Some(Player::P1),
                std::cmp::Ordering::Equal => None,
            };
            self.over = true;
        }
    }
}
