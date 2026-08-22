//! The complete player action space and its single validator.
//!
//! Humans, AI commanders, ghosts, and tests all issue commands through
//! [`Game::validate_command`]. Illegal commands are rejected with a reason;
//! there is no separate "AI path" that bypasses validation.
//!
//! Commands execute **immediately** when applied by the active player; the
//! turn only advances via [`Command::EndTurn`], which runs the end-of-turn
//! and start-of-turn lifecycle (turret fire, economy, production, resets).

use serde::{Deserialize, Serialize};

use crate::entity::{
    building_produces, building_stats, unit_requires_tech, unit_stats, BuildingType, EntityId,
    Player, UnitType, PLACE_RADIUS_TILES,
};
use crate::game::Game;
use crate::tech::{prereqs_met, TechId};
use crate::tiles::within_range;

/// A player command. Serialized as a tagged enum for the wire/replay format.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub enum Command {
    PlaceBuilding {
        player: Player,
        btype: BuildingType,
        tile: (u8, u8),
    },
    TrainUnit {
        player: Player,
        building: EntityId,
        utype: UnitType,
    },
    /// Move a group of units toward a waypoint. Each unit walks as far along
    /// its own path as its remaining movement points allow.
    MoveGroup {
        player: Player,
        units: Vec<EntityId>,
        waypoint: (u8, u8),
    },
    /// Attack a specific enemy unit or building with every ordered unit that
    /// is in range and has not acted yet. Advance-Wars damage rules apply
    /// (damage scales with attacker HP; surviving direct defenders counter).
    Attack {
        player: Player,
        units: Vec<EntityId>,
        target: EntityId,
    },
    /// Start researching `tech` (requires an owned, alive Tech Lab). The tech
    /// completes automatically once the research pool covers its cost.
    StartResearch {
        player: Player,
        tech: TechId,
    },
    Sell {
        player: Player,
        building: EntityId,
    },
    Repair {
        player: Player,
        building: EntityId,
    },
    /// End the active player's turn: turrets fire, income is collected,
    /// production advances, then the opponent's turn begins.
    EndTurn {
        player: Player,
    },
}

impl Command {
    pub fn player(&self) -> Player {
        match self {
            Command::PlaceBuilding { player, .. }
            | Command::TrainUnit { player, .. }
            | Command::MoveGroup { player, .. }
            | Command::Attack { player, .. }
            | Command::StartResearch { player, .. }
            | Command::Sell { player, .. }
            | Command::Repair { player, .. }
            | Command::EndTurn { player } => *player,
        }
    }

    /// Whether this command consumes one slot of the per-turn action budget
    /// (`EndTurn` is free).
    pub fn costs_action(&self) -> bool {
        !matches!(self, Command::EndTurn { .. })
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub enum CommandError {
    NotYourEntity,
    EntityDead,
    NotABuilding,
    BuildingCannotTrain,
    RequiresFactory,
    RequiresTechLab,
    RequiresOreAdjacent,
    RequiresCrystalAdjacent,
    TileBlocked,
    TileHasOre,
    RequiresTech,
    TechPrereqNotMet,
    TechAlreadyResearched,
    AlreadyResearching,
    InvalidTile,
    TooFarFromBase,
    NotEnoughOre,
    QueueFull,
    EmptyGroup,
    NoSuchTarget,
    OutOfRange,
    AlreadyActed,
    AlreadyRepaired,
    BuildingFullHealth,
    CantSellHq,
    NotYourTurn,
    MatchOver,
    RateLimited,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CommandError::NotYourEntity => "not your entity",
            CommandError::EntityDead => "entity is dead",
            CommandError::NotABuilding => "not a valid building for this order",
            CommandError::BuildingCannotTrain => "building cannot train this unit",
            CommandError::RequiresFactory => "requires a factory",
            CommandError::RequiresTechLab => "requires a tech lab",
            CommandError::RequiresOreAdjacent => "refinery must be placed next to an ore tile",
            CommandError::RequiresCrystalAdjacent => {
                "crystal refinery must be placed next to a crystal field"
            }
            CommandError::TileBlocked => "tile is blocked",
            CommandError::TileHasOre => "tile contains ore or crystal",
            CommandError::RequiresTech => "unit requires a researched technology",
            CommandError::TechPrereqNotMet => "research prerequisites not met",
            CommandError::TechAlreadyResearched => "technology already researched",
            CommandError::AlreadyResearching => "another technology is already being researched",
            CommandError::InvalidTile => "tile out of bounds",
            CommandError::TooFarFromBase => "too far from your base",
            CommandError::NotEnoughOre => "not enough ore",
            CommandError::QueueFull => "production queue is full",
            CommandError::EmptyGroup => "empty unit group",
            CommandError::NoSuchTarget => "no such target",
            CommandError::OutOfRange => "no ordered unit is in range of the target",
            CommandError::AlreadyActed => "unit already acted this turn",
            CommandError::AlreadyRepaired => "building already repaired this turn",
            CommandError::BuildingFullHealth => "building at full health",
            CommandError::CantSellHq => "cannot sell the HQ",
            CommandError::NotYourTurn => "it is not your turn",
            CommandError::MatchOver => "the match is over",
            CommandError::RateLimited => "action budget exhausted for this turn",
        };
        f.write_str(s)
    }
}

impl Game {
    /// Validate a command against current state. Pure — mutates nothing.
    pub fn validate_command(&self, cmd: &Command) -> Result<(), CommandError> {
        use CommandError::*;
        if self.is_over() {
            return Err(MatchOver);
        }
        if cmd.player() != self.active {
            return Err(NotYourTurn);
        }
        match cmd {
            Command::PlaceBuilding {
                player,
                btype,
                tile,
            } => self.validate_place(*player, *btype, *tile),
            Command::TrainUnit {
                player,
                building,
                utype,
            } => self.validate_train(*player, *building, *utype),
            Command::MoveGroup {
                player,
                units,
                waypoint,
            } => self.validate_move(*player, units, *waypoint),
            Command::Attack {
                player,
                units,
                target,
            } => self.validate_attack(*player, units, *target),
            Command::StartResearch { player, tech } => {
                if self.count_buildings(*player, BuildingType::TechLab) == 0 {
                    return Err(RequiresTechLab);
                }
                let r = &self.research[player.index()];
                if r.has(*tech) {
                    return Err(TechAlreadyResearched);
                }
                if r.researching.is_some() {
                    return Err(AlreadyResearching);
                }
                if !prereqs_met(*tech, &r.researched) {
                    return Err(TechPrereqNotMet);
                }
                Ok(())
            }
            Command::Sell { player, building } => {
                let b = self.building(*player, *building).ok_or(NotYourEntity)?;
                if !b.is_alive() {
                    return Err(EntityDead);
                }
                if b.btype == BuildingType::Hq {
                    return Err(CantSellHq);
                }
                Ok(())
            }
            Command::Repair { player, building } => {
                let b = self.building(*player, *building).ok_or(NotYourEntity)?;
                if !b.is_alive() {
                    return Err(EntityDead);
                }
                if b.hp >= b.max_hp {
                    return Err(BuildingFullHealth);
                }
                if b.repaired_this_turn {
                    return Err(AlreadyRepaired);
                }
                if self.ore[player.index()] < crate::entity::REPAIR_MIN_COST {
                    return Err(NotEnoughOre);
                }
                Ok(())
            }
            // `EndTurn` legality (active player, match live) was checked above;
            // nothing else to validate.
            Command::EndTurn { .. } => Ok(()),
        }
    }

    fn validate_place(
        &self,
        player: Player,
        btype: BuildingType,
        tile: (u8, u8),
    ) -> Result<(), CommandError> {
        use CommandError::*;
        if btype == BuildingType::Hq {
            return Err(NotABuilding);
        }
        // Tech tree: the TechLab needs a Factory, the Airfield needs a
        // Factory, and the Radar / TeslaCoil sit on the second tier (they need
        // the TechLab itself, which transitively needs the Factory).
        if (btype == BuildingType::TechLab || btype == BuildingType::Airfield)
            && self.count_buildings(player, BuildingType::Factory) == 0
        {
            return Err(RequiresFactory);
        }
        if (btype == BuildingType::Radar || btype == BuildingType::TeslaCoil)
            && self.count_buildings(player, BuildingType::TechLab) == 0
        {
            return Err(RequiresTechLab);
        }
        self.validate_tile(tile)?;
        if self.building_at(tile).is_some() {
            return Err(TileBlocked);
        }
        if self.map.ore_at(tile.0, tile.1) > 0 || self.map.crystal_at(tile.0, tile.1) > 0 {
            return Err(TileHasOre);
        }
        let cost = building_stats(btype).cost;
        if self.ore[player.index()] < cost {
            return Err(NotEnoughOre);
        }
        if btype == BuildingType::Refinery || btype == BuildingType::CrystalRefinery {
            // Refineries must touch their resource field — that is their whole
            // point under passive income. This replaces the clump rule for
            // them: remote refineries are how you claim an expansion pocket.
            let adjacent_ore = NEIGHBOR_OFFSETS.iter().any(|&(dx, dy)| {
                let (x, y) = (tile.0 as i32 + dx, tile.1 as i32 + dy);
                x >= 0
                    && y >= 0
                    && (x as usize) < crate::map::MAP_SIZE
                    && (y as usize) < crate::map::MAP_SIZE
                    && if btype == BuildingType::Refinery {
                        self.map.ore_at(x as u8, y as u8) > 0
                    } else {
                        self.map.crystal_at(x as u8, y as u8) > 0
                    }
            });
            if !adjacent_ore {
                return Err(if btype == BuildingType::Refinery {
                    RequiresOreAdjacent
                } else {
                    RequiresCrystalAdjacent
                });
            }
        } else if !self.near_own_building(player, tile) {
            return Err(TooFarFromBase);
        }
        Ok(())
    }

    fn validate_train(
        &self,
        player: Player,
        building: EntityId,
        utype: UnitType,
    ) -> Result<(), CommandError> {
        use CommandError::*;
        let b = self.building(player, building).ok_or(NotYourEntity)?;
        if !b.is_alive() {
            return Err(EntityDead);
        }
        if !building_produces(b.btype).contains(&utype) {
            return Err(BuildingCannotTrain);
        }
        if (utype == UnitType::Artillery || utype == UnitType::MammothTank)
            && self.count_buildings(player, BuildingType::TechLab) == 0
        {
            return Err(RequiresTechLab);
        }
        if let Some(tech) = unit_requires_tech(utype) {
            if !self.research[player.index()].has(tech) {
                return Err(RequiresTech);
            }
        }
        let cost = unit_stats(utype).cost;
        if self.ore[player.index()] < cost {
            return Err(NotEnoughOre);
        }
        if b.queue.len() >= self.config.max_queue {
            return Err(QueueFull);
        }
        Ok(())
    }

    fn validate_move(
        &self,
        player: Player,
        units: &[EntityId],
        waypoint: (u8, u8),
    ) -> Result<(), CommandError> {
        use CommandError::*;
        if units.is_empty() {
            return Err(EmptyGroup);
        }
        for id in units {
            let u = self.unit(player, *id).ok_or(NotYourEntity)?;
            if !u.is_alive() {
                return Err(EntityDead);
            }
            if u.mp <= 0 || u.moved {
                return Err(AlreadyActed);
            }
        }
        self.validate_tile(waypoint)?;
        Ok(())
    }

    fn validate_attack(
        &self,
        player: Player,
        units: &[EntityId],
        target: EntityId,
    ) -> Result<(), CommandError> {
        use CommandError::*;
        if units.is_empty() {
            return Err(EmptyGroup);
        }
        for id in units {
            let u = self.unit(player, *id).ok_or(NotYourEntity)?;
            if !u.is_alive() {
                return Err(EntityDead);
            }
            if u.acted {
                return Err(AlreadyActed);
            }
        }
        let enemy = player.enemy();
        let target_pos = self
            .units
            .iter()
            .find(|u| u.id == target && u.owner == enemy && u.is_alive())
            .map(|u| u.tile)
            .or_else(|| {
                self.buildings
                    .iter()
                    .find(|b| b.id == target && b.owner == enemy && b.is_alive())
                    .map(|b| b.tile)
            })
            .ok_or(NoSuchTarget)?;
        // At least one ordered unit must actually be able to strike.
        let any_in_range = units.iter().any(|&id| {
            let Some(u) = self.unit(player, id) else {
                return false;
            };
            let stats = unit_stats(u.utype);
            let d = crate::tiles::chebyshev(u.tile.0, u.tile.1, target_pos.0, target_pos.1);
            d <= stats.range_tiles && d >= stats.min_range_tiles
        });
        if !any_in_range {
            return Err(OutOfRange);
        }
        Ok(())
    }

    fn validate_tile(&self, tile: (u8, u8)) -> Result<(), CommandError> {
        use CommandError::*;
        if tile.0 as usize >= crate::map::MAP_SIZE || tile.1 as usize >= crate::map::MAP_SIZE {
            return Err(CommandError::InvalidTile);
        }
        if !self.map.is_passable(tile.0, tile.1) {
            return Err(TileBlocked);
        }
        Ok(())
    }

    /// The target tile must be within [`PLACE_RADIUS_TILES`] of at least one own
    /// building (any building, including ones still under construction). This is
    /// what keeps a base in one connected clump instead of scattered structures.
    /// Refineries are exempt — see `validate_place`.
    fn near_own_building(&self, player: Player, tile: (u8, u8)) -> bool {
        self.buildings
            .iter()
            .filter(|b| b.owner == player)
            .any(|b| within_range(tile.0, tile.1, b.tile.0, b.tile.1, PLACE_RADIUS_TILES))
    }

    pub(crate) fn count_buildings(&self, player: Player, btype: BuildingType) -> usize {
        self.buildings
            .iter()
            .filter(|b| b.owner == player && b.btype == btype)
            .count()
    }
}

/// The eight neighbor offsets (movement is 8-directional, diagonal costs 1 MP).
pub const NEIGHBOR_OFFSETS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];
