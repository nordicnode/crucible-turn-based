//! Fog of war: per-player visibility and last-seen memory.
//!
//! [`FogView`] is the *only* object the AI commander receives. It carries
//! remembered enemy positions with their last-seen turn (so staleness can be
//! applied in feature extraction), never the live state of hidden entities.

use serde::{Deserialize, Serialize};

use crate::entity::{BuildingType, EntityId, Player, UnitType};
use crate::game::Game;
use crate::map::{tile_index, MAP_TILES};
use crate::tiles::within_range;

fn default_known_tiles() -> Vec<bool> {
    vec![false; MAP_TILES]
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct RememberedUnit {
    pub id: EntityId,
    pub tile: (u8, u8),
    /// The turn this entity was last observed.
    pub last_seen: i32,
    pub utype: UnitType,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct RememberedBuilding {
    pub id: EntityId,
    pub tile: (u8, u8),
    pub last_seen: i32,
    pub btype: BuildingType,
}

/// Per-player fog memory, part of serialized game state.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct FogMemory {
    pub units: Vec<RememberedUnit>,
    pub buildings: Vec<RememberedBuilding>,
    /// Ore tiles this player has ever seen (fields don't move, but must be scouted).
    pub known_ore: Vec<bool>,
    /// Crystal tiles this player has ever seen (same scouting contract as ore).
    #[serde(default)]
    pub known_crystal: Vec<bool>,
    /// Steel tiles this player has ever seen.
    #[serde(default = "default_known_tiles")]
    pub known_steel: Vec<bool>,
    /// Coal tiles this player has ever seen.
    #[serde(default = "default_known_tiles")]
    pub known_coal: Vec<bool>,
    /// Every tile this player has ever seen (monotonic; powers the AI's
    /// "unexplored fraction" observation). `#[serde(default)]` keeps old
    /// persisted states loadable — a missing field starts fully unexplored.
    #[serde(default)]
    pub explored: Vec<bool>,
}

impl Default for FogMemory {
    fn default() -> Self {
        FogMemory {
            units: Vec::new(),
            buildings: Vec::new(),
            known_ore: vec![false; MAP_TILES],
            known_crystal: vec![false; MAP_TILES],
            known_steel: vec![false; MAP_TILES],
            known_coal: vec![false; MAP_TILES],
            explored: vec![false; MAP_TILES],
        }
    }
}

/// A player's legal observation of the world at one turn.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct FogView {
    pub player: Player,
    pub turn: i32,
    /// Tiles currently visible.
    pub visible: Vec<bool>,
    /// Enemy units, merged with current positions where visible.
    pub units: Vec<RememberedUnit>,
    /// Enemy buildings, merged with current positions where visible.
    pub buildings: Vec<RememberedBuilding>,
    pub known_ore: Vec<bool>,
    pub known_crystal: Vec<bool>,
    pub known_steel: Vec<bool>,
    pub known_coal: Vec<bool>,
    /// Every tile this player has ever seen.
    pub explored: Vec<bool>,
}

impl Game {
    /// Update fog memory for both players. Call after every command batch and
    /// at each turn boundary (so freshly-killed units drop out and new
    /// positions stick).
    pub fn fog_phase(&mut self) {
        for player in Player::ALL {
            let visible = self.compute_visible(player);
            self.update_memory(player, &visible);
        }
    }

    /// Build the legal observation for a player.
    pub fn fog_view(&self, player: Player) -> FogView {
        let visible = self.compute_visible(player);
        let mem = &self.fog[player.index()];

        let mut units: Vec<RememberedUnit> = mem.units.clone();
        // Upsert live positions for anything visible now.
        for u in &self.units {
            if u.owner == player.enemy() && visible[tile_index(u.tile.0, u.tile.1)] {
                if let Some(m) = units.iter_mut().find(|m| m.id == u.id) {
                    m.tile = u.tile;
                    m.last_seen = self.turn;
                } else {
                    units.push(RememberedUnit {
                        id: u.id,
                        tile: u.tile,
                        last_seen: self.turn,
                        utype: u.utype,
                    });
                }
            }
        }

        let mut buildings: Vec<RememberedBuilding> = mem.buildings.clone();
        for b in &self.buildings {
            if b.owner == player.enemy() && visible[tile_index(b.tile.0, b.tile.1)] {
                if let Some(m) = buildings.iter_mut().find(|m| m.id == b.id) {
                    m.last_seen = self.turn;
                } else {
                    buildings.push(RememberedBuilding {
                        id: b.id,
                        tile: b.tile,
                        last_seen: self.turn,
                        btype: b.btype,
                    });
                }
            }
        }

        // Drop memories that expired unseen (buildings can also die unseen —
        // a remembered refinery that was destroyed out of sight). Memory is
        // removed only by expiry or by re-observing the location; hidden
        // deaths are never consulted to prune fresh memories.
        let cutoff = self.turn - crate::entity::FOG_MEMORY_TURNS;
        units.retain(|m| m.last_seen >= cutoff);
        buildings.retain(|m| m.last_seen >= cutoff);

        FogView {
            player,
            turn: self.turn,
            visible,
            units,
            buildings,
            known_ore: mem.known_ore.clone(),
            known_crystal: mem.known_crystal.clone(),
            known_steel: mem.known_steel.clone(),
            known_coal: mem.known_coal.clone(),
            explored: mem.explored.clone(),
        }
    }

    /// Tiles currently visible to `player`: every tile within vision radius
    /// of any of their living entities. Deterministic integer math.
    pub fn compute_visible(&self, player: Player) -> Vec<bool> {
        let mut vis = vec![false; MAP_TILES];
        for u in &self.units {
            if u.owner != player || !u.is_alive() {
                continue;
            }
            let r = crate::entity::unit_stats(u.utype).vision_tiles;
            stamp_radius(&mut vis, u.tile, r);
        }
        for b in &self.buildings {
            if b.owner != player || !b.is_operational() {
                continue;
            }
            let r = crate::entity::building_stats(b.btype).vision_tiles;
            stamp_radius(&mut vis, b.tile, r);
        }
        vis
    }

    fn update_memory(&mut self, player: Player, visible: &[bool]) {
        let mem = &mut self.fog[player.index()];
        for (idx, seen) in visible.iter().enumerate() {
            if *seen {
                mem.explored[idx] = true;
            }
        }
        for (idx, is_visible) in visible.iter().copied().enumerate().take(MAP_TILES) {
            if !is_visible
                || self.map.resource_amount_at(
                    (idx % crate::map::MAP_SIZE) as u8,
                    (idx / crate::map::MAP_SIZE) as u8,
                ) <= 0
            {
                continue;
            }
            match self.map.resource_at(
                (idx % crate::map::MAP_SIZE) as u8,
                (idx / crate::map::MAP_SIZE) as u8,
            ) {
                Some(crate::entity::ResourceType::Ore) => mem.known_ore[idx] = true,
                Some(crate::entity::ResourceType::Crystal) => mem.known_crystal[idx] = true,
                Some(crate::entity::ResourceType::Steel) => mem.known_steel[idx] = true,
                Some(crate::entity::ResourceType::Coal) => mem.known_coal[idx] = true,
                None => {}
            }
        }

        // Upsert live enemy sightings where visible.
        for u in &self.units {
            if u.owner == player.enemy() && visible[tile_index(u.tile.0, u.tile.1)] {
                if let Some(m) = mem.units.iter_mut().find(|m| m.id == u.id) {
                    m.tile = u.tile;
                    m.last_seen = self.turn;
                } else {
                    mem.units.push(RememberedUnit {
                        id: u.id,
                        tile: u.tile,
                        last_seen: self.turn,
                        utype: u.utype,
                    });
                }
            }
        }
        for b in &self.buildings {
            if b.owner == player.enemy() && visible[tile_index(b.tile.0, b.tile.1)] {
                if let Some(m) = mem.buildings.iter_mut().find(|m| m.id == b.id) {
                    m.last_seen = self.turn;
                } else {
                    mem.buildings.push(RememberedBuilding {
                        id: b.id,
                        tile: b.tile,
                        last_seen: self.turn,
                        btype: b.btype,
                    });
                }
            }
        }

        // Expire stale memories.
        let cutoff = self.turn - crate::entity::FOG_MEMORY_TURNS;
        mem.units.retain(|m| m.last_seen >= cutoff);
        mem.buildings.retain(|m| m.last_seen >= cutoff);
    }
}

/// Mark every tile within Euclidean radius `r` of `tile` as visible.
fn stamp_radius(vis: &mut [bool], tile: (u8, u8), r: i32) {
    let r_i = r as i64;
    let lo_x = (tile.0 as i64 - r_i).max(0) as u8;
    let hi_x = (tile.0 as i64 + r_i).min((crate::map::MAP_SIZE - 1) as i64) as u8;
    let lo_y = (tile.1 as i64 - r_i).max(0) as u8;
    let hi_y = (tile.1 as i64 + r_i).min((crate::map::MAP_SIZE - 1) as i64) as u8;
    for y in lo_y..=hi_y {
        for x in lo_x..=hi_x {
            if within_range(tile.0, tile.1, x, y, r) {
                vis[tile_index(x, y)] = true;
            }
        }
    }
}
