//! Procedural map generation and deterministic grid pathfinding.
//!
//! Maps are generated from a `u64` seed with deliberately asymmetric,
//! constraint-scored landforms and resource zones. Fairness is enforced by
//! spawn envelopes, route-cost bands, and guaranteed resource roles rather
//! than by mirroring every feature. Generation retries with derived seeds until
//! the candidate satisfies those constraints, and falls back to a playable
//! open map if necessary.
//!
//! Terrain is typed (plains/forest/hills/desert/swamp/river/lake/mountain):
//! passability, unit movement cost, and combat defense all derive from it, so
//! the same map description drives pathfinding, movement budgets, and damage
//! reduction.

use std::collections::BinaryHeap;

use serde::{Deserialize, Serialize};

use crate::entity::ResourceType;
use crate::rng::Rng;

/// The playable theatre is intentionally spacious: opening bases occupy only
/// a small corner of the board, leaving room for resource expansion and
/// several distinct fronts.
pub const MAP_SIZE: usize = 128;
pub const MAP_TILES: usize = MAP_SIZE * MAP_SIZE;
const MAP_SCALE: i32 = (MAP_SIZE / 64) as i32;
const MAP_CENTER: i32 = (MAP_SIZE / 2) as i32;

const MAX_GEN_ATTEMPTS: u64 = 256;

const fn scaled(value: i32) -> i32 {
    value * MAP_SCALE
}

fn zero_resource_vec() -> Vec<i32> {
    vec![0; MAP_TILES]
}

fn zero_climate_vec() -> Vec<u8> {
    vec![0; MAP_TILES]
}

/// Tile terrain. Lakes and mountains are impassable; the remaining biomes
/// trade movement speed, cover, and tactical value. `Water` is retained as a
/// serialized compatibility variant but is labelled "Lake" everywhere in the
/// player-facing contract.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Debug)]
pub enum Terrain {
    /// Open temperate ground (the legacy serialized name is preserved).
    Plains,
    /// Passable woodland with defensive tree cover.
    Forest,
    /// Passable high ground with the strongest defensive bonus.
    Hills,
    /// Dry open ground: quick to cross, exposed to fire.
    Desert,
    /// Wet lowland: slow to cross, but useful as concealment.
    Swamp,
    /// Deep lake water; impassable to ground and air units.
    Water,
    /// Shallow river crossing; passable but expensive for ground units.
    River,
    /// Impassable mountain rock.
    Mountain,
}

impl Terrain {
    pub const ALL: [Terrain; 8] = [
        Terrain::Plains,
        Terrain::Forest,
        Terrain::Hills,
        Terrain::Desert,
        Terrain::Swamp,
        Terrain::Water,
        Terrain::River,
        Terrain::Mountain,
    ];

    pub fn is_passable(self) -> bool {
        matches!(
            self,
            Terrain::Plains
                | Terrain::Forest
                | Terrain::Hills
                | Terrain::Desert
                | Terrain::Swamp
                | Terrain::River
        )
    }

    /// Extra movement multiplier for ground units entering this tile.
    pub fn move_mult(self) -> i32 {
        match self {
            Terrain::Plains | Terrain::Desert => 1,
            Terrain::Forest | Terrain::Hills => 2,
            Terrain::Swamp => 3,
            Terrain::River => 3,
            Terrain::Water | Terrain::Mountain => 1,
        }
    }

    /// Damage reduction (percent) for a defender standing on this tile.
    pub fn defense_reduction(self) -> i32 {
        match self {
            Terrain::Forest => 20,
            Terrain::Hills => 30,
            Terrain::Swamp => 10,
            _ => 0,
        }
    }

    /// Stable display label sent to clients and inspection tools.
    pub const fn label(self) -> &'static str {
        match self {
            Terrain::Plains => "Plains",
            Terrain::Forest => "Forest",
            Terrain::Hills => "Hills",
            Terrain::Desert => "Desert",
            Terrain::Swamp => "Swamp",
            Terrain::Water => "Lake",
            Terrain::River => "River",
            Terrain::Mountain => "Mountain",
        }
    }

    /// A compact tactical description for the tile inspector.
    pub const fn tactical_tag(self) -> &'static str {
        match self {
            Terrain::Plains => "open ground",
            Terrain::Forest => "tree cover",
            Terrain::Hills => "high ground",
            Terrain::Desert => "open arid ground",
            Terrain::Swamp => "slow wetland cover",
            Terrain::Water => "impassable lake",
            Terrain::River => "slow crossing",
            Terrain::Mountain => "impassable rock",
        }
    }
}

#[inline]
pub fn tile_index(x: u8, y: u8) -> usize {
    (y as usize) * MAP_SIZE + (x as usize)
}

#[inline]
pub fn tile_coords(idx: usize) -> (u8, u8) {
    ((idx % MAP_SIZE) as u8, (idx / MAP_SIZE) as u8)
}

/// Mirror a coordinate under the legacy point-symmetry helper used by old
/// fixtures and the fallback recipe.
#[inline]
pub fn mirror(x: u8) -> u8 {
    (MAP_SIZE - 1) as u8 - x
}
/// The static world layout. Deposit metadata never mutates during a
/// match: `resource_kind` and `richness` are authoritative. The legacy
/// quantity arrays remain positive presentation/compatibility fields for
/// old snapshots and are not a finite reserve.

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Map {
    pub seed: u64,
    /// Passable terrain (true = walkable). Derived from `terrain`.
    pub passable: Vec<bool>,
    /// Terrain type per tile.
    pub terrain: Vec<Terrain>,
    /// Low-frequency elevation field used to form coherent ridges and basins.
    /// It is presentation metadata as well as the generator's source field.
    #[serde(default = "zero_climate_vec")]
    pub elevation: Vec<u8>,
    /// Low-frequency moisture field used to form forests, deserts, and
    /// wetlands. Kept in the map contract so tools can explain a tile's biome.
    #[serde(default = "zero_climate_vec")]
    pub moisture: Vec<u8>,
    /// Low-frequency temperature field (0 = cold, 255 = hot) derived from
    /// latitude, elevation cooling, and regional noise. Presentation
    /// metadata explaining a tile's biome; also drives the generator's
    /// climate model (tundra vs desert vs jungle).
    #[serde(default = "zero_climate_vec")]
    pub temperature: Vec<u8>,
    /// Legacy ore marker per tile. Positive values identify a deposit and
    /// preserve old replay/map consumers; this is not a remaining reserve.
    pub ore: Vec<i32>,
    /// Legacy crystal marker per tile. Positive values identify a deposit;
    /// this is not a remaining reserve.
    pub crystal: Vec<i32>,
    /// Legacy steel marker per tile; positive values identify a deposit.
    #[serde(default = "zero_resource_vec")]
    pub steel: Vec<i32>,
    /// Legacy coal marker per tile; positive values identify a deposit.
    #[serde(default = "zero_resource_vec")]
    pub coal: Vec<i32>,
    /// Resource kind for each deposit tile. `None` means the tile is empty.
    #[serde(default)]
    pub resource_kind: Vec<Option<ResourceType>>,
    /// Static richness tier for each tile: 1 = poor, 2 = standard, 3 = rich.
    #[serde(default)]
    pub richness: Vec<u8>,
    /// HQ spawn tiles, indexed by player.
    pub hq_tiles: [(u8, u8); 2],
}

impl Map {
    pub fn generate(seed: u64) -> Map {
        for attempt in 0..MAX_GEN_ATTEMPTS {
            let s = seed.wrapping_add(attempt);
            if let Some(map) = try_generate(s) {
                return map;
            }
        }
        // Guaranteed-valid fallback: fully open and resource-complete.
        open_map(seed)
    }

    #[inline]
    pub fn is_passable(&self, x: u8, y: u8) -> bool {
        self.passable[tile_index(x, y)]
    }

    #[inline]
    pub fn terrain_at(&self, x: u8, y: u8) -> Terrain {
        self.terrain[tile_index(x, y)]
    }

    #[inline]
    pub fn ore_at(&self, x: u8, y: u8) -> i32 {
        self.ore[tile_index(x, y)]
    }

    #[inline]
    pub fn crystal_at(&self, x: u8, y: u8) -> i32 {
        self.crystal[tile_index(x, y)]
    }

    /// The resource occupying a tile, if any. Old serialized maps without
    /// `resource_kind` are inferred from their legacy ore/crystal arrays.
    pub fn resource_at(&self, x: u8, y: u8) -> Option<ResourceType> {
        let idx = tile_index(x, y);
        if let Some(kind) = self.resource_kind.get(idx).copied().flatten() {
            return Some(kind);
        }
        if self.ore.get(idx).copied().unwrap_or(0) > 0 {
            Some(ResourceType::Ore)
        } else if self.steel.get(idx).copied().unwrap_or(0) > 0 {
            Some(ResourceType::Steel)
        } else if self.coal.get(idx).copied().unwrap_or(0) > 0 {
            Some(ResourceType::Coal)
        } else if self.crystal.get(idx).copied().unwrap_or(0) > 0 {
            Some(ResourceType::Crystal)
        } else {
            None
        }
    }

    /// Legacy static marker for the resource occupying a tile. Despite the
    /// historical name, this value never decreases during a match. Use
    /// [`Map::resource_richness_at`] for gameplay yield.
    pub fn resource_amount_at(&self, x: u8, y: u8) -> i32 {
        let idx = tile_index(x, y);
        let kind = self.resource_at(x, y);
        let marker = match kind {
            Some(ResourceType::Ore) => self.ore.get(idx).copied().unwrap_or(0),
            Some(ResourceType::Steel) => self.steel.get(idx).copied().unwrap_or(0),
            Some(ResourceType::Coal) => self.coal.get(idx).copied().unwrap_or(0),
            Some(ResourceType::Crystal) => self.crystal.get(idx).copied().unwrap_or(0),
            None => 0,
        };
        if marker > 0 || kind.is_none() {
            marker
        } else {
            // A compact snapshot carries only resource_kind/richness, not the
            // legacy amount array. Return a stable positive marker so old
            // presence checks work, but never use this as a yield or limit.
            100
        }
    }

    /// Static richness tier of a resource tile. Legacy maps derive it from
    /// their original amount markers so they remain usable after
    /// deserialization.
    pub fn resource_richness_at(&self, x: u8, y: u8) -> u8 {
        let idx = tile_index(x, y);
        let stored = self.richness.get(idx).copied().unwrap_or(0);
        if stored > 0 {
            stored.clamp(1, 3)
        } else if self.resource_kind.get(idx).copied().flatten().is_some() {
            // A compact old snapshot may carry only `resource_kind`; retain a
            // usable standard deposit rather than treating it as exhausted.
            2
        } else {
            richness_for_amount(self.resource_amount_at(x, y))
        }
    }

    /// Whether this tile contains a live, inexhaustible deposit.
    pub fn has_resource_at(&self, x: u8, y: u8) -> bool {
        self.resource_at(x, y).is_some() && self.resource_richness_at(x, y) > 0
    }

    /// All deposits are infinite. This explicit query keeps wire/UI code from
    /// inferring finite semantics from the legacy amount arrays.
    pub const fn deposits_are_infinite() -> bool {
        true
    }

    /// Legacy extraction hook. Deposits are inexhaustible, so extraction does
    /// not mutate the map; the return value preserves the old API shape for
    /// integrations that used it as a transfer amount.
    #[deprecated(note = "resource deposits are infinite; use refinery_yield instead")]
    pub fn deplete_resource(&mut self, x: u8, y: u8, amount: i32) -> i32 {
        if self.resource_at(x, y).is_some() && self.resource_richness_at(x, y) > 0 {
            amount.max(0)
        } else {
            0
        }
    }

    /// Legacy no-op extraction alias retained for old integrations.
    #[deprecated(note = "resource deposits are infinite; use refinery_yield instead")]
    pub fn deplete_ore(&mut self, x: u8, y: u8, amount: i32) -> i32 {
        #[allow(deprecated)]
        {
            self.deplete_resource(x, y, amount)
        }
    }

    /// Legacy no-op extraction alias retained for old integrations.
    #[deprecated(note = "resource deposits are infinite; use refinery_yield instead")]
    pub fn deplete_crystal(&mut self, x: u8, y: u8, amount: i32) -> i32 {
        #[allow(deprecated)]
        {
            self.deplete_resource(x, y, amount)
        }
    }

    /// Movement points to step from `from` to `to`. Base cost is 1 for an
    /// orthogonal step and 2 for a diagonal (no corner cutting); ground units
    /// pay the destination terrain's extra multiplier. Aircraft fly at base
    /// cost over everything.
    pub fn move_cost(&self, from: (u8, u8), to: (u8, u8), fly: bool) -> i32 {
        let base = if from.0 != to.0 && from.1 != to.1 {
            2
        } else {
            1
        };
        if fly {
            base
        } else {
            base * self.terrain_at(to.0, to.1).move_mult()
        }
    }

    /// Deterministic A* over the passable grid (8-dir, no corner cutting).
    /// Costs are in **movement points** (1 straight, 2 diagonal, terrain
    /// multipliers applied) so the returned path length matches `Unit::mp`
    /// budgeting directly. `blocked` is a dynamic overlay (buildings): a tile
    /// is traversable iff it is passable terrain and not marked blocked.
    /// `fly` (aircraft) skips the overlay entirely — they fly over buildings —
    /// while still routing around impassable terrain.
    pub fn find_path(
        &self,
        from: (u8, u8),
        to: (u8, u8),
        blocked: &[bool],
        fly: bool,
    ) -> Option<Vec<(u8, u8)>> {
        if from == to {
            return Some(vec![]);
        }
        if !self.is_passable(from.0, from.1)
            || !self.is_passable(to.0, to.1)
            || (!fly && (blocked[tile_index(from.0, from.1)] || blocked[tile_index(to.0, to.1)]))
        {
            return None;
        }

        let start = tile_index(from.0, from.1);
        let goal = tile_index(to.0, to.1);

        let mut g_score = vec![i32::MAX; MAP_TILES];
        g_score[start] = 0;

        let mut came_from = vec![u16::MAX; MAP_TILES];

        #[derive(Clone, Copy, Eq, PartialEq)]
        struct Node {
            f: i32,
            g: i32,
            idx: u16,
        }
        // Min-heap: reverse comparison so BinaryHeap pops the smallest f/g/idx.
        impl Ord for Node {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                other
                    .f
                    .cmp(&self.f)
                    .then_with(|| other.g.cmp(&self.g))
                    .then_with(|| other.idx.cmp(&self.idx))
            }
        }
        impl PartialOrd for Node {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        let mut open = BinaryHeap::new();
        open.push(Node {
            f: octile(from, to),
            g: 0,
            idx: start as u16,
        });

        while let Some(node) = open.pop() {
            let cur = node.idx as usize;
            if cur == goal {
                return Some(reconstruct_path(&came_from, start, goal));
            }
            if node.g > g_score[cur] {
                continue;
            }
            let (cx, cy) = tile_coords(cur);

            for (nx, ny, base) in self.neighbors(cx, cy) {
                let nidx = tile_index(nx, ny);
                if !fly && blocked[nidx] {
                    continue;
                }
                let cost = base
                    * if fly {
                        1
                    } else {
                        self.terrain[nidx].move_mult()
                    };
                let tentative = g_score[cur] + cost;
                if tentative < g_score[nidx] {
                    g_score[nidx] = tentative;
                    came_from[nidx] = cur as u16;
                    let f = tentative + octile((nx, ny), to);
                    open.push(Node {
                        f,
                        g: tentative,
                        idx: nidx as u16,
                    });
                }
            }
        }

        None
    }

    /// Passable 8-neighbors with base step costs in movement points
    /// (1 straight, 2 diagonal), forbidding diagonal corner cutting.
    pub fn neighbors(&self, x: u8, y: u8) -> Vec<(u8, u8, i32)> {
        let mut out = Vec::with_capacity(8);
        for (dx, dy) in &[
            (1i8, 0i8),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ] {
            let nx = x as i32 + *dx as i32;
            let ny = y as i32 + *dy as i32;
            if nx < 0 || ny < 0 || nx >= MAP_SIZE as i32 || ny >= MAP_SIZE as i32 {
                continue;
            }
            let (nx, ny) = (nx as u8, ny as u8);
            if !self.is_passable(nx, ny) {
                continue;
            }
            if *dx != 0 && *dy != 0 {
                // No corner cutting: both orthogonal tiles must be passable.
                if !self.is_passable((x as i32 + *dx as i32) as u8, y)
                    || !self.is_passable(x, (y as i32 + *dy as i32) as u8)
                {
                    continue;
                }
                out.push((nx, ny, 2));
            } else {
                out.push((nx, ny, 1));
            }
        }
        out
    }
}

fn octile(from: (u8, u8), to: (u8, u8)) -> i32 {
    let dx = (from.0 as i32 - to.0 as i32).abs();
    let dy = (from.1 as i32 - to.1 as i32).abs();
    // A diagonal costs the same as two plains orthogonal steps in this ruleset
    // (2 MP), so Manhattan distance is the tight admissible lower bound. The
    // previous 2x heuristic could overestimate and make A* choose a needlessly
    // expensive route through a terrain pocket.
    dx + dy
}

fn reconstruct_path(came_from: &[u16], start: usize, goal: usize) -> Vec<(u8, u8)> {
    let mut path = vec![];
    let mut cur = goal;
    while cur != start {
        path.push(tile_coords(cur));
        cur = came_from[cur] as usize;
    }
    path.reverse();
    path
}

/// Attempt a single deterministic generation. Returns `None` if the result is
/// not fully connected, so the caller can retry with the next derived seed.
///
/// Layout recipe (Civ-flavoured, mirror-fair):
///  1. A broken mountain rim rings the theatre, leaving wide corner gates.
///  2. Hill ranges and gated mountain ridges shape mid-field lanes; lakes and
///     forest belts add cover and choke points.
///  3. Each HQ gets a clear plains quarter (radius 5).
///  4. Ore: a base mine ~3 tiles inward (always refinery-able on turn 1), a
///     forward field ~8 tiles in, and contested mid-field expansion sites.
///  5. Crystal: rare fields in the mid/expansion zones gate the late game.
///
/// Placement phase used by the modern generator. The candidate score is
/// deliberately integer-only: the same seed produces the same map on native
/// and wasm, while separate candidates let the generator reject maps that are
/// pretty but strategically broken.
#[derive(Clone, Copy)]
enum SiteZone {
    Local,
    Secondary,
    Contested,
    Strategic,
}

const SITE_OFFSETS: [(i32, i32); 5] = [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)];

/// Build smooth integer-only climate fields from a coarse lattice. Sampling
/// neighboring lattice points keeps biomes in belts and basins instead of
/// producing the noisy checkerboard that independent tile rolls create.
/// Build multi-octave climate fields from a coarse lattice. Sampling
/// neighboring lattice points keeps biomes in belts and basins instead of
/// producing the noisy checkerboard that independent tile rolls create.
/// Two octaves (broad + mid-frequency) produce organic biome boundaries.
fn generate_climate_fields(seed: u64) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    fn sample(seed: u64, x: i32, y: i32, salt: u64) -> u16 {
        (map_hash(
            seed ^ salt,
            x.clamp(0, MAP_SIZE as i32 / 8) as u8,
            y.clamp(0, MAP_SIZE as i32 / 8) as u8,
        ) & 0xff) as u16
    }

    let mut elevation = vec![0u8; MAP_TILES];
    let mut moisture = vec![0u8; MAP_TILES];
    let mut temperature = vec![0u8; MAP_TILES];
    for y in 0..MAP_SIZE as i32 {
        for x in 0..MAP_SIZE as i32 {
            let idx = tile_index(x as u8, y as u8);

            // Octave 1: broad 8×8 lattice (continental scale), stretched
            // over the larger theatre so biomes remain regional rather than
            // repeating the old 64-tile view twice.
            let broad_cell = MAP_SIZE as i32 / 8;
            let gx = x / broad_cell;
            let gy = y / broad_cell;
            let fx = x % broad_cell;
            let fy = y % broad_cell;
            let broad = |salt: u64| -> u8 {
                let a = sample(seed, gx, gy, salt) as i32;
                let b = sample(seed, gx + 1, gy, salt) as i32;
                let c = sample(seed, gx, gy + 1, salt) as i32;
                let d = sample(seed, gx + 1, gy + 1, salt) as i32;
                (((a * (broad_cell - fx) + b * fx) * (broad_cell - fy)
                    + (c * (broad_cell - fx) + d * fx) * fy)
                    / (broad_cell * broad_cell)) as u8
            };

            // Octave 2: mid-frequency 4×4 lattice (regional detail).
            let mid_cell = MAP_SIZE as i32 / 16;
            let mx = x / mid_cell;
            let my = y / mid_cell;
            let mfx = x % mid_cell;
            let mfy = y % mid_cell;
            let mid = |salt: u64| -> u8 {
                let a = sample(seed, mx, my, salt) as i32;
                let b = sample(seed, mx + 1, my, salt) as i32;
                let c = sample(seed, mx, my + 1, salt) as i32;
                let d = sample(seed, mx + 1, my + 1, salt) as i32;
                (((a * (mid_cell - mfx) + b * mfx) * (mid_cell - mfy)
                    + (c * (mid_cell - mfx) + d * mfx) * mfy)
                    / (mid_cell * mid_cell)) as u8
            };

            // Combine octaves: broad gets 70% weight, mid gets 20%, detail
            // noise gets 10%. This produces smooth continents with organic
            // edges rather than either flat plains or noisy checkerboards.
            let detail_e = (map_hash(seed ^ 0xE1E1_0002, x as u8, y as u8) & 0x1f) as u8;
            let broad_e = broad(0xE1E1_0001);
            let mid_e = mid(0xE1E1_0003);
            elevation[idx] =
                broad_e.saturating_mul(7) / 10 + mid_e.saturating_mul(2) / 10 + detail_e / 10;

            // Moisture: broad pattern + latitude bias + mid-frequency
            // variation, producing recognizable climate zones (tropical
            // at the map center, arid/dry at the poles).
            let broad_m = broad(0xA015_0001u64);
            let mid_m = mid(0xA015_0003u64);
            let latitude_bias =
                ((MAP_CENTER - (y - MAP_CENTER).abs()) * 2).clamp(0, MAP_SIZE as i32) as u8;
            let detail_m = (map_hash(seed ^ 0xA015_0002u64, x as u8, y as u8) & 0x1f) as u8;
            moisture[idx] = broad_m.saturating_mul(5) / 8
                + mid_m.saturating_mul(2) / 8
                + latitude_bias / 8
                + detail_m / 8;

            // T6 climate model: temperature is driven by latitude (equator
            // at y = 32), cooled by elevation (lapse rate), with a regional
            // noise band. Equator sea level reads ~215, poles ~45, and
            // mountain peaks drop well below their latitude's baseline.
            let latitude_temp = 40 + (MAP_CENTER - (y - MAP_CENTER).abs()) * 11 / 2;
            let cooling = (elevation[idx] as i32) / 3;
            let noise_t = (map_hash(seed ^ 0x1B3B_0001u64, x as u8, y as u8) & 0x3f) as i32 - 20;
            temperature[idx] = (latitude_temp + noise_t - cooling).clamp(0, 255) as u8;
        }
    }
    (elevation, moisture, temperature)
}

fn climate_terrain(elevation: u8, moisture: u8, temperature: u8) -> Terrain {
    // Elevation dominates: ridges and peaks read as hills in every climate.
    if elevation >= 190 {
        return Terrain::Hills;
    }
    // Hot, wet equatorial band reads as dense jungle (heavy tree cover).
    if temperature >= 185 && moisture >= 150 {
        return Terrain::Forest;
    }
    // Warm, waterlogged lowlands become swamps; cold wetlands stay frozen
    // tundra (plains) instead.
    if moisture >= 205 && elevation < 165 {
        return if temperature >= 120 {
            Terrain::Swamp
        } else {
            Terrain::Plains
        };
    }
    if moisture <= 48 {
        // Arid ground: deserts form only in warm latitudes; cold arid
        // ground reads as dry tundra (plains). The band near the
        // desert/plains boundary reads as dry plains — an ecotone.
        if temperature >= 70 {
            Terrain::Desert
        } else {
            Terrain::Plains
        }
    } else if moisture >= 150 {
        // Forested: near the forest/plains edge the tile is light
        // woodland, producing a gradual canopy edge rather than a hard
        // line.
        Terrain::Forest
    } else {
        // The transition band (48..150 moisture) is plains — the
        // ecotone between arid and forested biomes.
        Terrain::Plains
    }
}

/// Generate one modern candidate. `Map::generate` retries this candidate with
/// a derived seed when its quality gate rejects it.
fn try_generate(seed: u64) -> Option<Map> {
    let mut rng = Rng::from_seed(seed);
    let (elevation, moisture, temperature) = generate_climate_fields(seed);
    let mut terrain = vec![Terrain::Plains; MAP_TILES];
    let mut passable = vec![true; MAP_TILES];
    for idx in 0..MAP_TILES {
        terrain[idx] = climate_terrain(elevation[idx], moisture[idx], temperature[idx]);
        passable[idx] = terrain[idx].is_passable();
    }
    let mut ore = vec![0i32; MAP_TILES];
    let mut steel = vec![0i32; MAP_TILES];
    let mut coal = vec![0i32; MAP_TILES];
    let mut crystal = vec![0i32; MAP_TILES];

    // Pick opposite, but independently jittered, spawn pads. The halves are
    // not mirror copies; quality scoring below keeps their playable envelopes
    // comparable without making every central feature identical.
    let diagonal = rng.below(2) == 0;
    let hq0 = if diagonal {
        (
            rng.range(scaled(7) as i64, scaled(19) as i64) as u8,
            rng.range(scaled(7) as i64, scaled(19) as i64) as u8,
        )
    } else {
        (
            rng.range(scaled(45) as i64, scaled(57) as i64) as u8,
            rng.range(scaled(7) as i64, scaled(19) as i64) as u8,
        )
    };
    let hq1 = if diagonal {
        (
            rng.range(scaled(45) as i64, scaled(58) as i64) as u8,
            rng.range(scaled(44) as i64, scaled(58) as i64) as u8,
        )
    } else {
        (
            rng.range(scaled(6) as i64, scaled(19) as i64) as u8,
            rng.range(scaled(44) as i64, scaled(58) as i64) as u8,
        )
    };

    // Coherent landforms. Features are painted before the lanes and spawn
    // clearings, so the constraints can deliberately cut roads through them.
    let mountains = (4 + rng.below(4) as usize) * MAP_SCALE as usize;
    for _ in 0..mountains {
        paint_mountain_chain(&mut terrain, &mut passable, &mut rng);
    }
    let lakes = (2 + rng.below(3) as usize) * MAP_SCALE as usize;
    for _ in 0..lakes {
        paint_irregular_blob(&mut terrain, &mut passable, &mut rng, Terrain::Water, 3, 6);
    }
    let rivers = 1 + rng.below(2) as usize;
    for _ in 0..rivers {
        paint_river(&mut terrain, &mut passable, &mut rng, &elevation);
    }
    let groves = (9 + rng.below(7) as usize) * MAP_SCALE as usize;
    for _ in 0..groves {
        let kind = if rng.chance(1, 4) {
            Terrain::Hills
        } else {
            Terrain::Forest
        };
        paint_irregular_blob(&mut terrain, &mut passable, &mut rng, kind, 1, 3);
    }

    // Biome belts use low-frequency patches rather than isolated one-tile
    // stamps. The lanes below deliberately cut through them, producing the
    // readable contrast and route choices expected from a strategy map.
    let deserts = (5 + rng.below(4) as usize) * MAP_SCALE as usize;
    for _ in 0..deserts {
        paint_irregular_blob(&mut terrain, &mut passable, &mut rng, Terrain::Desert, 2, 5);
    }
    let wetlands = (4 + rng.below(3) as usize) * MAP_SCALE as usize;
    for _ in 0..wetlands {
        paint_irregular_blob(&mut terrain, &mut passable, &mut rng, Terrain::Swamp, 2, 4);
    }

    // Two broad guaranteed lanes make the generated theatre tactically rich
    // rather than a maze. Rivers remain in the lanes as expensive crossings;
    // lakes and mountain walls are cut at deterministic road points.
    let center = (MAP_CENTER as u8, MAP_CENTER as u8);
    carve_lane(&mut terrain, &mut passable, hq0, center, scaled(1));
    carve_lane(&mut terrain, &mut passable, hq1, center, scaled(1));
    clear_around(&mut terrain, &mut passable, hq0.0, hq0.1, scaled(5));
    clear_around(&mut terrain, &mut passable, hq1.0, hq1.1, scaled(5));
    // Keep the build ring safe while exposing a varied, useful opening view.
    // The palette follows the local climate instead of a fixed ratio.
    paint_spawn_ring(
        &mut terrain,
        &mut passable,
        hq0,
        seed ^ 0xA5A5_5A5A,
        &elevation,
        &moisture,
        &temperature,
    );
    paint_spawn_ring(
        &mut terrain,
        &mut passable,
        hq1,
        seed ^ 0x5A5A_A5A5,
        &elevation,
        &moisture,
        &temperature,
    );

    // Resource sites are selected from a scored candidate field. Each site is
    // a compact five-tile deposit with an intentional role in the theatre.
    let mut centers = Vec::new();
    let mut placed = 0usize;
    for (resource, zone, own, enemy, amount) in [
        (ResourceType::Ore, SiteZone::Local, hq0, hq1, 1300),
        (ResourceType::Ore, SiteZone::Local, hq1, hq0, 1300),
        (ResourceType::Ore, SiteZone::Secondary, hq0, hq1, 1900),
        (ResourceType::Ore, SiteZone::Secondary, hq1, hq0, 1900),
        (ResourceType::Steel, SiteZone::Secondary, hq0, hq1, 1500),
        (ResourceType::Steel, SiteZone::Secondary, hq1, hq0, 1500),
        (ResourceType::Coal, SiteZone::Secondary, hq0, hq1, 1500),
        (ResourceType::Coal, SiteZone::Secondary, hq1, hq0, 1500),
        (ResourceType::Steel, SiteZone::Contested, hq0, hq1, 1800),
        (ResourceType::Coal, SiteZone::Contested, hq1, hq0, 1800),
        (ResourceType::Crystal, SiteZone::Strategic, hq0, hq1, 600),
        (ResourceType::Crystal, SiteZone::Strategic, hq1, hq0, 600),
    ] {
        if place_deposit_site(
            &mut terrain,
            &mut passable,
            &mut ore,
            &mut steel,
            &mut coal,
            &mut crystal,
            &mut centers,
            seed.wrapping_add(placed as u64 * 0x9E37_79B9),
            resource,
            zone,
            own,
            enemy,
            amount,
        ) {
            placed += 1;
        }
    }

    let (resource_kind, richness) = build_resource_metadata(&ore, &steel, &coal, &crystal);
    let map = Map {
        seed,
        passable,
        terrain,
        elevation,
        moisture,
        temperature,
        ore,
        crystal,
        steel,
        coal,
        resource_kind,
        richness,
        hq_tiles: [hq0, hq1],
    };

    (placed >= 10 && modern_map_quality(&map) && has_opening_resources(&map)).then_some(map)
}

/// Guaranteed-valid open fallback. It still uses the scored resource placer so
/// tests, editor previews, and a pathological generation seed see the same
/// resource semantics as a normal match.
fn open_map(seed: u64) -> Map {
    let mut rng = Rng::from_seed(seed ^ 0xD1B5_4A32_9C77_0E11);
    let diagonal = rng.below(2) == 0;
    let hq0 = if diagonal {
        (
            rng.range(scaled(10) as i64, scaled(16) as i64) as u8,
            rng.range(scaled(10) as i64, scaled(16) as i64) as u8,
        )
    } else {
        (
            rng.range(scaled(48) as i64, scaled(54) as i64) as u8,
            rng.range(scaled(10) as i64, scaled(16) as i64) as u8,
        )
    };
    let hq1 = if diagonal {
        (
            rng.range(scaled(48) as i64, scaled(54) as i64) as u8,
            rng.range(scaled(48) as i64, scaled(54) as i64) as u8,
        )
    } else {
        (
            rng.range(scaled(10) as i64, scaled(16) as i64) as u8,
            rng.range(scaled(48) as i64, scaled(54) as i64) as u8,
        )
    };
    let elevation = vec![0; MAP_TILES];
    let moisture = vec![0; MAP_TILES];
    let temperature = vec![0; MAP_TILES];
    let mut terrain = vec![Terrain::Plains; MAP_TILES];
    let mut passable = vec![true; MAP_TILES];
    clear_around(&mut terrain, &mut passable, hq0.0, hq0.1, 5);
    clear_around(&mut terrain, &mut passable, hq1.0, hq1.1, 5);
    paint_spawn_ring(
        &mut terrain,
        &mut passable,
        hq0,
        seed ^ 0xA5A5_5A5A,
        &elevation,
        &moisture,
        &temperature,
    );
    paint_spawn_ring(
        &mut terrain,
        &mut passable,
        hq1,
        seed ^ 0x5A5A_A5A5,
        &elevation,
        &moisture,
        &temperature,
    );
    let mut ore = vec![0i32; MAP_TILES];
    let mut steel = vec![0i32; MAP_TILES];
    let mut coal = vec![0i32; MAP_TILES];
    let mut crystal = vec![0i32; MAP_TILES];
    let mut centers = Vec::new();
    let sites = [
        (ResourceType::Ore, SiteZone::Local, hq0, hq1, 1300),
        (ResourceType::Ore, SiteZone::Local, hq1, hq0, 1300),
        (ResourceType::Steel, SiteZone::Secondary, hq0, hq1, 1500),
        (ResourceType::Steel, SiteZone::Secondary, hq1, hq0, 1500),
        (ResourceType::Coal, SiteZone::Contested, hq0, hq1, 1500),
        (ResourceType::Coal, SiteZone::Contested, hq1, hq0, 1500),
        (ResourceType::Crystal, SiteZone::Strategic, hq0, hq1, 600),
        (ResourceType::Crystal, SiteZone::Strategic, hq1, hq0, 600),
    ];
    for (i, (resource, zone, own, enemy, amount)) in sites.into_iter().enumerate() {
        let _ = place_deposit_site(
            &mut terrain,
            &mut passable,
            &mut ore,
            &mut steel,
            &mut coal,
            &mut crystal,
            &mut centers,
            seed.wrapping_add(i as u64 * 0x517C_C1B7),
            resource,
            zone,
            own,
            enemy,
            amount,
        );
    }
    let (resource_kind, richness) = build_resource_metadata(&ore, &steel, &coal, &crystal);
    Map {
        seed,
        passable,
        terrain,
        elevation,
        moisture,
        temperature,
        ore,
        crystal,
        steel,
        coal,
        resource_kind,
        richness,
        hq_tiles: [hq0, hq1],
    }
}

/// Paint a tectonic mountain range: a spline that meanders smoothly from a
/// map-edge origin across the interior, with a short secondary fault branch.
/// Heading noise is drawn once per step unconditionally, so boundary
/// clipping can never perturb downstream RNG state.
fn paint_mountain_chain(terrain: &mut [Terrain], passable: &mut [bool], rng: &mut Rng) {
    let start_side = rng.below(4);
    let (mut x, mut y) = match start_side {
        0 => (
            rng.range(scaled(12) as i64, scaled(52) as i64) as i32,
            scaled(8) + rng.below(scaled(5) as u64) as i32,
        ),
        1 => (
            rng.range(scaled(12) as i64, scaled(52) as i64) as i32,
            scaled(51) + rng.below(scaled(5) as u64) as i32,
        ),
        2 => (
            scaled(8) + rng.below(scaled(5) as u64) as i32,
            rng.range(scaled(12) as i64, scaled(52) as i64) as i32,
        ),
        _ => (
            scaled(51) + rng.below(scaled(5) as u64) as i32,
            rng.range(scaled(12) as i64, scaled(52) as i64) as i32,
        ),
    };
    let steps = rng.range(scaled(22) as i64, scaled(38) as i64) as i32;
    let width = 1 + rng.below(2) as i32;
    // Heading in radians; drifts slowly each step (tectonic ridges meander
    // rather than kink).
    let mut heading = rng.range(0, 628) as f64 / 100.0;
    let branch_at = 6 + rng.below(8) as i32;
    let mut branch_left = 0; // steps remaining on the secondary fault line
    let mut branch_heading = heading + 0.9;
    for step in 0..steps {
        // Draw heading noise unconditionally (boundary-stable).
        let turn = (rng.next_u64() % 23) as f64 - 11.0; // ±11 degrees
        heading += turn.to_radians();
        x += (heading.cos() * 1.7).round() as i32;
        y += (heading.sin() * 1.7).round() as i32;
        x = x.clamp(scaled(6), scaled(57));
        y = y.clamp(scaled(6), scaled(57));
        if step == branch_at {
            branch_heading = heading + 0.9;
            branch_left = 7 + rng.below(5);
        }
        // Paint the ridge spine; a narrow gate every so often forms passes.
        let gate = step > 0 && step % 9 == 0 && branch_left == 0;
        if !gate {
            for dy in -width..=width {
                for dx in -width..=width {
                    set_terrain(terrain, passable, x + dx, y + dy, Terrain::Mountain);
                }
            }
        }
        // The secondary fault line branches off and continues a short way.
        if branch_left > 0 {
            branch_left -= 1;
            branch_heading += (rng.next_u64() % 21) as f64 / 100.0 - 0.10;
            let bx = x + (branch_heading.cos() * 1.4).round() as i32;
            let by = y + (branch_heading.sin() * 1.4).round() as i32;
            for dy in -width..=width {
                for dx in -width..=width {
                    set_terrain(terrain, passable, bx + dx, by + dy, Terrain::Mountain);
                }
            }
        }
    }
}

fn paint_irregular_blob(
    terrain: &mut [Terrain],
    passable: &mut [bool],
    rng: &mut Rng,
    kind: Terrain,
    min_radius: i32,
    max_radius: i32,
) {
    let cx = rng.range(scaled(10) as i64, scaled(54) as i64) as i32;
    let cy = rng.range(scaled(10) as i64, scaled(54) as i64) as i32;
    let radius = rng.range(
        (min_radius * MAP_SCALE) as i64,
        ((max_radius + 1) * MAP_SCALE) as i64,
    ) as i32;
    // Pre-roll the edge-noise seed once, so the blob's boundary clipping
    // does not change how many RNG draws happen. This keeps the generator's
    // downstream state stable regardless of where the blob center sits.
    let noise_seed = rng.next_u64();
    for y in (cy - radius).max(scaled(1))..=(cy + radius).min(scaled(63) - 1) {
        for x in (cx - radius).max(scaled(1))..=(cx + radius).min(scaled(63) - 1) {
            let dx = x - cx;
            let dy = y - cy;
            let edge_noise = map_hash(noise_seed, x as u8, y as u8) % 5;
            if dx * dx + dy * dy <= radius * radius + edge_noise as i32 - 2 {
                set_terrain(terrain, passable, x, y, kind);
            }
        }
    }
}

/// Paint a deterministic ring of passable biomes around a spawn clearing.
/// The ring starts outside the build radius, so the opening is safe without
/// making the first visible screen a featureless green square. The palette
/// follows the local climate (temperature + moisture) rather than a fixed
/// ratio, so spawns in cold latitudes read as tundra and tropical spawns
/// read as jungle.
fn paint_spawn_ring(
    terrain: &mut [Terrain],
    passable: &mut [bool],
    hq: (u8, u8),
    seed: u64,
    elevation: &[u8],
    moisture: &[u8],
    temperature: &[u8],
) {
    for y in (hq.1 as i32 - scaled(8)).max(1)..=(hq.1 as i32 + scaled(8)).min(MAP_SIZE as i32 - 2) {
        for x in
            (hq.0 as i32 - scaled(8)).max(1)..=(hq.0 as i32 + scaled(8)).min(MAP_SIZE as i32 - 2)
        {
            let dx = (x - hq.0 as i32).abs();
            let dy = (y - hq.1 as i32).abs();
            let distance = dx.max(dy);
            if !(scaled(6)..=scaled(8)).contains(&distance) {
                continue;
            }
            let idx = tile_index(x as u8, y as u8);
            let m = moisture[idx];
            let t = temperature[idx];
            let e = elevation[idx];
            let n = map_hash(seed, x as u8, y as u8) % 100;
            let kind = if e >= 150 {
                // High plateau ring: rocky outcrops dominate.
                if n < 46 {
                    Terrain::Hills
                } else if n < 72 {
                    Terrain::Forest
                } else {
                    Terrain::Plains
                }
            } else if t < 70 {
                // Cold ring: tundra and bare rock.
                if n < 34 {
                    Terrain::Hills
                } else if n < 62 {
                    Terrain::Forest
                } else {
                    Terrain::Plains
                }
            } else if t >= 185 && m >= 120 {
                // Hot, wet ring: jungle with marsh pockets.
                if n < 46 {
                    Terrain::Forest
                } else if n < 72 {
                    Terrain::Swamp
                } else {
                    Terrain::Hills
                }
            } else if m <= 60 {
                // Arid ring: desert with rocky outcrops.
                if n < 52 {
                    Terrain::Desert
                } else if n < 74 {
                    Terrain::Hills
                } else {
                    Terrain::Plains
                }
            } else {
                // Temperate ring: mixed woodland, hills, and meadow.
                if n < 35 {
                    Terrain::Forest
                } else if n < 58 {
                    Terrain::Hills
                } else if n < 78 {
                    Terrain::Plains
                } else {
                    Terrain::Swamp
                }
            };
            set_terrain(terrain, passable, x, y, kind);
        }
    }
}

/// Paint a river that follows the elevation field downhill from a
/// high-elevation source to a low point or map edge. Uses a greedy
/// steepest-descent walk (deterministic) with a small jitter to avoid
/// perfectly straight channels. Lakes may form where the descent pools.
fn paint_river(terrain: &mut [Terrain], passable: &mut [bool], rng: &mut Rng, elevation: &[u8]) {
    // Pick a high-elevation source tile in the interior.
    let mut best_h = 0u8;
    let mut source = (MAP_CENTER as u8, MAP_CENTER as u8);
    for _ in 0..32 {
        let x = rng.range(scaled(10) as i64, scaled(54) as i64) as i32;
        let y = rng.range(scaled(10) as i64, scaled(54) as i64) as i32;
        let e = elevation[tile_index(x as u8, y as u8)];
        if e > best_h {
            best_h = e;
            source = (x as u8, y as u8);
        }
    }
    if best_h < 100 {
        // No significant high ground to start from; fall back to a straight
        // axis with drift (the old behavior) so every seed still gets a river.
        let vertical = rng.below(2) == 0;
        let mut drift = rng.range(scaled(10) as i64, scaled(54) as i64) as i32;
        for along in 0..MAP_SIZE as i32 {
            if vertical {
                set_terrain(terrain, passable, drift, along, Terrain::River);
            } else {
                set_terrain(terrain, passable, along, drift, Terrain::River);
            }
            if rng.chance(1, 3) {
                drift =
                    (drift + if rng.chance(1, 2) { 1 } else { -1 }).clamp(scaled(7), scaled(56));
            }
        }
        return;
    }

    // Greedy steepest-descent: walk to the lowest unvisited neighbor until
    // we reach a low-elevation tile or the map edge. The walk marks River
    // tiles; if it descends into a local minimum (no neighbor is lower),
    // it pools into a small lake.
    let mut current = source;
    let mut visited = std::collections::HashSet::new();
    visited.insert(tile_index(current.0, current.1));
    let width = 1 + rng.below(2) as i32; // 1 or 2 tiles wide
    for _step in 0..128 {
        // Paint the river tile(s) at current.
        for dy in 0..width {
            for dx in 0..width {
                let x = current.0 as i32 + dx - (width - 1) / 2;
                let y = current.1 as i32 + dy - (width - 1) / 2;
                if in_bounds(x, y) {
                    set_terrain(terrain, passable, x, y, Terrain::River);
                }
            }
        }

        let cur_e = elevation[tile_index(current.0, current.1)];

        // Stop if we've reached low ground (potential lake/outlet).
        if cur_e < 60 {
            break;
        }

        // Find the steepest-descent neighbor (lowest elevation, unvisited,
        // in-bounds). Ties broken by a deterministic jitter.
        let mut best: Option<(u8, (u8, u8))> = None;
        for (dx, dy) in [
            (-1i32, 0i32),
            (1, 0),
            (0, -1),
            (0, 1),
            (-1, -1),
            (-1, 1),
            (1, -1),
            (1, 1),
        ] {
            let nx = current.0 as i32 + dx;
            let ny = current.1 as i32 + dy;
            if !in_bounds(nx, ny) {
                continue;
            }
            let nidx = tile_index(nx as u8, ny as u8);
            if visited.contains(&nidx) {
                continue;
            }
            let ne = elevation[nidx];
            // Skip tiles already painted as mountain or water (river flows
            // around them, not through).
            if !terrain[nidx].is_passable() && terrain[nidx] != Terrain::River {
                continue;
            }
            // Prefer descending, but allow a small climb to escape plateaus.
            if best.is_none_or(|(be, _)| ne < be) {
                best = Some((ne, (nx as u8, ny as u8)));
            }
        }

        match best {
            Some((ne, next)) if ne <= cur_e => {
                current = next;
                visited.insert(tile_index(current.0, current.1));
            }
            _ => {
                // Local minimum: pool into a small lake.
                for dy in -1..=1i32 {
                    for dx in -1..=1i32 {
                        let x = current.0 as i32 + dx;
                        let y = current.1 as i32 + dy;
                        if in_bounds(x, y) {
                            set_terrain(terrain, passable, x, y, Terrain::Water);
                        }
                    }
                }
                break;
            }
        }
    }
}

fn carve_lane(
    terrain: &mut [Terrain],
    passable: &mut [bool],
    from: (u8, u8),
    to: (u8, u8),
    width: i32,
) {
    let steps = (from.0 as i32 - to.0 as i32)
        .abs()
        .max((from.1 as i32 - to.1 as i32).abs());
    for i in 0..=steps {
        let x = from.0 as i32 + (to.0 as i32 - from.0 as i32) * i / steps.max(1);
        let y = from.1 as i32 + (to.1 as i32 - from.1 as i32) * i / steps.max(1);
        for dy in -width..=width {
            for dx in -width..=width {
                let tx = x + dx;
                let ty = y + dy;
                if in_bounds(tx, ty) {
                    let idx = tile_index(tx as u8, ty as u8);
                    if terrain[idx] != Terrain::River {
                        set_terrain(terrain, passable, tx, ty, Terrain::Plains);
                    }
                }
            }
        }
    }
}

fn map_hash(seed: u64, x: u8, y: u8) -> u64 {
    let mut z =
        seed ^ ((x as u64 + 0x9E37) << 17) ^ ((y as u64 + 0x7F4A) << 33) ^ 0xA076_1D64_78BD_642F;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn any_resource_at(ore: &[i32], steel: &[i32], coal: &[i32], crystal: &[i32], idx: usize) -> bool {
    ore[idx] > 0 || steel[idx] > 0 || coal[idx] > 0 || crystal[idx] > 0
}

fn site_available(
    terrain: &[Terrain],
    ore: &[i32],
    steel: &[i32],
    coal: &[i32],
    crystal: &[i32],
    center: (u8, u8),
) -> bool {
    SITE_OFFSETS.iter().all(|&(dx, dy)| {
        let x = center.0 as i32 + dx;
        let y = center.1 as i32 + dy;
        in_bounds(x, y)
            && terrain[tile_index(x as u8, y as u8)].is_passable()
            && !any_resource_at(ore, steel, coal, crystal, tile_index(x as u8, y as u8))
    })
}

fn candidate_score(
    terrain: &[Terrain],
    seed: u64,
    center: (u8, u8),
    own: (u8, u8),
    enemy: (u8, u8),
    zone: SiteZone,
    resource: ResourceType,
) -> Option<i32> {
    let own_d = (center.0 as i32 - own.0 as i32)
        .abs()
        .max((center.1 as i32 - own.1 as i32).abs());
    let enemy_d = (center.0 as i32 - enemy.0 as i32)
        .abs()
        .max((center.1 as i32 - enemy.1 as i32).abs());
    let mut score = (map_hash(seed, center.0, center.1) % 97) as i32;
    score += SITE_OFFSETS
        .iter()
        .filter(|&&(dx, dy)| {
            let x = center.0 as i32 + dx;
            let y = center.1 as i32 + dy;
            in_bounds(x, y) && terrain[tile_index(x as u8, y as u8)].is_passable()
        })
        .count() as i32
        * 20;
    score += match terrain[tile_index(center.0, center.1)] {
        Terrain::Plains => 30,
        Terrain::Hills => 24,
        Terrain::Forest => 8,
        Terrain::Desert => 27,
        Terrain::Swamp => 4,
        Terrain::River => -40,
        Terrain::Water | Terrain::Mountain => return None,
    };
    // Terrain-resource correlation: steel prefers hills (mining), coal
    // prefers deserts/sedimentary, crystal prefers forests/mountains
    // (veins), and ore is flexible (any passable ground). This gives
    // terrain strategic meaning for resource expansion.
    let terrain_bonus: i32 = match (resource, terrain[tile_index(center.0, center.1)]) {
        (ResourceType::Steel, Terrain::Hills) => 120,
        (ResourceType::Steel, Terrain::Mountain) => 80,
        (ResourceType::Coal, Terrain::Desert) => 120,
        (ResourceType::Coal, Terrain::Hills) => 60,
        (ResourceType::Crystal, Terrain::Forest) => 100,
        (ResourceType::Crystal, Terrain::Hills) => 80,
        (ResourceType::Ore, Terrain::Hills) => 40,
        (ResourceType::Ore, Terrain::Plains) => 30,
        _ => 0,
    };
    score += terrain_bonus;
    match zone {
        SiteZone::Local => {
            if !(scaled(4)..=scaled(10)).contains(&own_d) || enemy_d < scaled(18) {
                return None;
            }
            score += 3000 - (own_d - scaled(6)).abs() * 130 + enemy_d * 4;
        }
        SiteZone::Secondary => {
            if !(scaled(11)..=scaled(23)).contains(&own_d) || enemy_d < scaled(13) {
                return None;
            }
            score += 2200 - (own_d - scaled(16)).abs() * 50 + enemy_d * 3;
        }
        SiteZone::Contested => {
            if own_d < scaled(17)
                || enemy_d < scaled(17)
                || own_d > scaled(42)
                || enemy_d > scaled(42)
            {
                return None;
            }
            score +=
                1800 - (own_d - enemy_d).abs() * 28 - (own_d + enemy_d - scaled(58)).abs() * 13;
        }
        SiteZone::Strategic => {
            if own_d < scaled(20) || enemy_d < scaled(20) {
                return None;
            }
            score += 1400 - (own_d - enemy_d).abs() * 18 - (own_d + enemy_d - scaled(66)).abs() * 9;
        }
    }
    Some(score)
}

#[allow(clippy::too_many_arguments)]
fn choose_site(
    terrain: &[Terrain],
    passable: &[bool],
    ore: &[i32],
    steel: &[i32],
    coal: &[i32],
    crystal: &[i32],
    centers: &[(u8, u8)],
    seed: u64,
    own: (u8, u8),
    enemy: (u8, u8),
    zone: SiteZone,
    resource: ResourceType,
) -> Option<(u8, u8)> {
    let mut best: Option<(i32, usize, (u8, u8))> = None;
    for y in 2..(MAP_SIZE as u8 - 2) {
        for x in 2..(MAP_SIZE as u8 - 2) {
            let center = (x, y);
            let idx = tile_index(x, y);
            if !passable[idx]
                || !site_available(terrain, ore, steel, coal, crystal, center)
                || centers.iter().any(|&c| {
                    (c.0 as i32 - x as i32)
                        .abs()
                        .max((c.1 as i32 - y as i32).abs())
                        < scaled(6)
                })
            {
                continue;
            }
            let Some(score) = candidate_score(terrain, seed, center, own, enemy, zone, resource)
            else {
                continue;
            };
            if best.is_none_or(|(bs, bi, _)| score > bs || (score == bs && idx < bi)) {
                best = Some((score, idx, center));
            }
        }
    }
    best.map(|(_, _, center)| center)
}

#[allow(clippy::too_many_arguments)]
fn stamp_deposit(
    terrain: &mut [Terrain],
    passable: &mut [bool],
    ore: &mut [i32],
    steel: &mut [i32],
    coal: &mut [i32],
    crystal: &mut [i32],
    kind: ResourceType,
    center: (u8, u8),
    amount: i32,
) {
    let center_amount = amount * 18 / 13;
    let ring_amount = amount * 12 / 13;
    for &(dx, dy) in &SITE_OFFSETS {
        let x = center.0 as i32 + dx;
        let y = center.1 as i32 + dy;
        if !in_bounds(x, y) {
            continue;
        }
        let idx = tile_index(x as u8, y as u8);
        let value = if dx == 0 && dy == 0 {
            center_amount
        } else {
            ring_amount
        };
        match kind {
            ResourceType::Ore => ore[idx] = value,
            ResourceType::Steel => steel[idx] = value,
            ResourceType::Coal => coal[idx] = value,
            ResourceType::Crystal => crystal[idx] = value,
        }
        // Deposit sites are intentionally passable; a refinery can occupy the
        // exact center tile and hills/forest remain meaningful around it.
        if !terrain[idx].is_passable() {
            set_terrain(terrain, passable, x, y, Terrain::Plains);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn place_deposit_site(
    terrain: &mut [Terrain],
    passable: &mut [bool],
    ore: &mut [i32],
    steel: &mut [i32],
    coal: &mut [i32],
    crystal: &mut [i32],
    centers: &mut Vec<(u8, u8)>,
    seed: u64,
    kind: ResourceType,
    zone: SiteZone,
    own: (u8, u8),
    enemy: (u8, u8),
    amount: i32,
) -> bool {
    let Some(center) = choose_site(
        terrain, passable, ore, steel, coal, crystal, centers, seed, own, enemy, zone, kind,
    ) else {
        return false;
    };
    stamp_deposit(
        terrain, passable, ore, steel, coal, crystal, kind, center, amount,
    );
    centers.push(center);
    true
}

fn route_cost(map: &Map, from: (u8, u8), path: &[(u8, u8)]) -> i32 {
    let mut cost = 0;
    let mut previous = from;
    for &tile in path {
        cost += map.move_cost(previous, tile, false);
        previous = tile;
    }
    cost
}

fn has_opening_resources(map: &Map) -> bool {
    for &hq in &map.hq_tiles {
        let local_ore = (0..MAP_TILES).any(|idx| {
            let tile = tile_coords(idx);
            chebyshev_distance(tile, hq) <= scaled(10)
                && map.resource_at(tile.0, tile.1) == Some(ResourceType::Ore)
        });
        let local_secondary = (0..MAP_TILES).any(|idx| {
            let tile = tile_coords(idx);
            chebyshev_distance(tile, hq) <= scaled(23)
                && map
                    .resource_at(tile.0, tile.1)
                    .is_some_and(|kind| matches!(kind, ResourceType::Steel | ResourceType::Coal))
        });
        if !local_ore || !local_secondary {
            return false;
        }
    }
    true
}

fn chebyshev_distance(a: (u8, u8), b: (u8, u8)) -> i32 {
    (a.0 as i32 - b.0 as i32)
        .abs()
        .max((a.1 as i32 - b.1 as i32).abs())
}

fn modern_map_quality(map: &Map) -> bool {
    let empty = vec![false; MAP_TILES];
    let Some(path01) = map.find_path(map.hq_tiles[0], map.hq_tiles[1], &empty, false) else {
        return false;
    };
    let Some(path10) = map.find_path(map.hq_tiles[1], map.hq_tiles[0], &empty, false) else {
        return false;
    };
    let cost01 = route_cost(map, map.hq_tiles[0], &path01);
    let cost10 = route_cost(map, map.hq_tiles[1], &path10);
    let max_cost = cost01.max(cost10).max(1);
    if (cost01 - cost10).abs() * 100 > max_cost * 28 {
        return false;
    }
    let mut terrain_counts = [0usize; 8];
    let mut resource_counts = [0usize; 4];
    for idx in 0..MAP_TILES {
        terrain_counts[match map.terrain[idx] {
            Terrain::Plains => 0,
            Terrain::Forest => 1,
            Terrain::Hills => 2,
            Terrain::Desert => 3,
            Terrain::Swamp => 4,
            Terrain::Water => 5,
            Terrain::River => 6,
            Terrain::Mountain => 7,
        }] += 1;
        if let Some(kind) = map.resource_at((idx % MAP_SIZE) as u8, (idx / MAP_SIZE) as u8) {
            if map.resource_amount_at((idx % MAP_SIZE) as u8, (idx / MAP_SIZE) as u8) > 0 {
                resource_counts[kind.index()] += 1;
            }
        }
    }
    // Reject bland or over-blocked candidates. Rivers only need one coherent
    // channel; their crossings remain possible because River is passable.
    if terrain_counts[1] < 35
        || terrain_counts[2] < 25
        || terrain_counts[3] < 35
        || terrain_counts[4] < 25
        || terrain_counts[5] < 25
        || terrain_counts[6] < 30
        || terrain_counts[7] < 35
    {
        return false;
    }
    resource_counts.iter().all(|&count| count >= 5)
        && is_fully_connected(map)
        && has_terrain_contiguity(map)
}

/// Quality gate: the largest connected component of each major biome must
/// contain at least 8 tiles, so the map has coherent regions rather than
/// scattered single-tile biomes.
fn has_terrain_contiguity(map: &Map) -> bool {
    for &target in &[
        Terrain::Forest,
        Terrain::Hills,
        Terrain::Desert,
        Terrain::Swamp,
    ] {
        let mut visited = vec![false; MAP_TILES];
        let mut largest = 0usize;
        for idx in 0..MAP_TILES {
            if visited[idx] || map.terrain[idx] != target {
                continue;
            }
            // Flood-fill this component.
            let mut stack = vec![idx];
            let mut size = 0;
            while let Some(i) = stack.pop() {
                if visited[i] || map.terrain[i] != target {
                    continue;
                }
                visited[i] = true;
                size += 1;
                let (x, y) = tile_coords(i);
                for (nx, ny, _) in map.neighbors(x, y) {
                    stack.push(tile_index(nx, ny));
                }
            }
            largest = largest.max(size);
        }
        if largest < 8 {
            return false;
        }
    }
    true
}

#[allow(dead_code)]
fn try_generate_legacy(seed: u64) -> Option<Map> {
    let mut rng = Rng::from_seed(seed);
    let mut terrain = vec![Terrain::Plains; MAP_TILES];
    let mut passable = vec![true; MAP_TILES];
    let mut ore = vec![0i32; MAP_TILES];
    let mut crystal = vec![0i32; MAP_TILES];
    let mut steel = vec![0i32; MAP_TILES];
    let mut coal = vec![0i32; MAP_TILES];

    // Real random 4-corner spawning (mirrored):
    // 0: Top-Left (7..18, 7..18) -> HQ1 in Bottom-Right
    // 1: Bottom-Left (7..18, 45..56) -> HQ1 in Top-Right
    // 2: Top-Right (45..56, 7..18) -> HQ1 in Bottom-Left
    // 3: Bottom-Right (45..56, 45..56) -> HQ1 in Top-Left
    let corner = rng.below(4);
    let (hx, hy) = match corner {
        0 => (rng.range(7, 18) as u8, rng.range(7, 18) as u8),
        1 => (rng.range(7, 18) as u8, rng.range(45, 56) as u8),
        2 => (rng.range(45, 56) as u8, rng.range(7, 18) as u8),
        _ => (rng.range(45, 56) as u8, rng.range(45, 56) as u8),
    };
    let hq0 = (hx, hy);
    let hq1 = (mirror(hx), mirror(hy));
    // Which direction does the arena center lie from this HQ?
    let (dxin, dyin) = (if hx < 32 { 1 } else { -1 }, if hy < 32 { 1 } else { -1 });

    // ---- Landforms (every feature is stamped as a mirrored pair) ----

    // Mountain rim around the battlefield; wide corner gaps keep spawns open.
    stamp_rim(&mut terrain, &mut passable, &mut rng);

    // Hill ranges: slow, defensive high ground in the mid-field.
    let hill_ranges = 1 + rng.below(2) as usize; // 1..=2
    for _ in 0..hill_ranges {
        let cx = rng.range(16, 48) as u8;
        let cy = rng.range(16, 48) as u8;
        let steps = (10 + rng.below(12)) as i32; // 10..=21 cells
        let dir = match rng.below(4) {
            0 => (1i32, 1i32),
            1 => (-1i32, 1i32),
            2 => (1i32, 0i32),
            _ => (0i32, 1i32),
        };
        if steps > 0 {
            stamp_ridge(
                &mut terrain,
                &mut passable,
                cx as i32,
                cy as i32,
                steps,
                dir,
                Terrain::Hills,
            );
            stamp_ridge(
                &mut terrain,
                &mut passable,
                mirror(cx) as i32,
                mirror(cy) as i32,
                steps,
                (-dir.0, -dir.1),
                Terrain::Hills,
            );
        }
    }

    // Gated mountain ridges through the middle (choke points with gates).
    let ridges = 1 + rng.below(2) as usize; // 1..=2
    for _ in 0..ridges {
        let cx = rng.range(20, 44) as u8;
        let cy = rng.range(20, 44) as u8;
        let steps = (12 + rng.below(10)) as i32; // 12..=21 cells
        let dir = match rng.below(4) {
            0 => (1i32, 1i32),
            1 => (-1i32, 1i32),
            2 => (1i32, 0i32),
            _ => (0i32, 1i32),
        };
        if steps > 0 {
            stamp_ridge(
                &mut terrain,
                &mut passable,
                cx as i32,
                cy as i32,
                steps,
                dir,
                Terrain::Mountain,
            );
            stamp_ridge(
                &mut terrain,
                &mut passable,
                mirror(cx) as i32,
                mirror(cy) as i32,
                steps,
                (-dir.0, -dir.1),
                Terrain::Mountain,
            );
        }
    }

    // Lakes: impassable water bodies.
    let lakes = 1 + rng.below(2) as usize; // 1..=2
    for _ in 0..lakes {
        let cx = rng.range(16, 48) as u8;
        let cy = rng.range(16, 48) as u8;
        let radius = 2 + rng.below(3) as i32; // 2..=4
        for (x, y) in [(cx, cy), (mirror(cx), mirror(cy))] {
            stamp_blob(&mut terrain, &mut passable, x, y, radius, Terrain::Water);
        }
    }

    // Forest belts: scattered groves that slow movement and give cover.
    let groves = 2 + rng.below(3) as usize; // 2..=4
    for _ in 0..groves {
        let cx = rng.range(12, 52) as u8;
        let cy = rng.range(12, 52) as u8;
        let radius = 1 + rng.below(2) as i32; // 1..=2
        for (x, y) in [(cx, cy), (mirror(cx), mirror(cy))] {
            stamp_blob(&mut terrain, &mut passable, x, y, radius, Terrain::Forest);
        }
    }

    // Carve a clear plains quarter around each HQ (radius 5) after the
    // landforms so spawns are never walled in and the build ring stays clear.
    for (x, y) in [hq0, hq1] {
        clear_around(&mut terrain, &mut passable, x, y, 5);
    }

    // ---- Ore ----

    // Base mine: a compact field anchored ~3 tiles inward from the HQ. Visible
    // from spawn, and there is always a free refinery slot adjacent to it
    // within build radius, so a turn-1 refinery is guaranteed on every map.
    stamp_ore_cluster_amount(
        &mut terrain,
        &mut passable,
        &mut ore,
        (hx as i32 + 3 * dxin) as u8,
        (hy as i32 + 3 * dyin) as u8,
        1300,
    );

    // Forward field: a slightly richer deposit a few tiles further in, giving
    // each side a reason to expand toward the contested middle.
    stamp_ore_cluster_amount(
        &mut terrain,
        &mut passable,
        &mut ore,
        (hx as i32 + 8 * dxin) as u8,
        (hy as i32 + 8 * dyin) as u8,
        1700,
    );

    // Contestable mid-field expansion sites (mirrored), kept clear of both
    // bases so they read as neutral territory.
    let sites = 2 + rng.below(2) as usize; // 2..=3
    let mut placed = 0;
    let mut guard = 0;
    while placed < sites && guard < 300 {
        guard += 1;
        let sx = rng.range(14, 50) as u8;
        let sy = rng.range(14, 50) as u8;
        if !valid_site_center(&passable, sx, sy, hq0, hq1) {
            continue;
        }
        let amount = 1500 + 100 * rng.below(5) as i32; // 1500..=1900 per tile
        stamp_ore_cluster_amount(&mut terrain, &mut passable, &mut ore, sx, sy, amount);
        placed += 1;
    }

    // ---- Crystal ----
    // Rare strategic resource: 2-4 mirrored fields beyond the early-game
    // reach, so the top tier of research demands real expansion.
    let crystal_sites = 2 + rng.below(2) as usize; // 2..=3
    let mut cplaced = 0;
    let mut cguard = 0;
    while cplaced < crystal_sites && cguard < 300 {
        cguard += 1;
        let sx = rng.range(16, 48) as u8;
        let sy = rng.range(16, 48) as u8;
        if !valid_site_center(&passable, sx, sy, hq0, hq1) {
            continue;
        }
        if ore[tile_index(sx, sy)] > 0 {
            continue;
        }
        stamp_crystal_cluster(
            &mut terrain,
            &mut passable,
            &mut crystal,
            sx,
            sy,
            400 + 50 * rng.below(5) as i32,
        );
        cplaced += 1;
    }

    // ---- Steel and Coal ----
    // Industrial resources occupy their own mirrored fields. They are placed
    // after the legacy ore/crystal recipe so old map geometry remains stable,
    // while a second deterministic search guarantees that fields do not
    // overlap and both resource types exist on every generated map.
    let mut material_sites = 0;
    let mut material_guard = 0;
    while material_sites < 4 && material_guard < 600 {
        material_guard += 1;
        let sx = rng.range(14, 50) as u8;
        let sy = rng.range(14, 50) as u8;
        if !valid_resource_center(&passable, &ore, &steel, &coal, &crystal, sx, sy) {
            continue;
        }
        let kind = if material_sites % 2 == 0 {
            ResourceType::Steel
        } else {
            ResourceType::Coal
        };
        stamp_material_cluster_amount(
            &mut terrain,
            &mut passable,
            &mut steel,
            &mut coal,
            kind,
            sx,
            sy,
            1100 + 100 * rng.below(7) as i32,
        );
        material_sites += 1;
    }

    let (resource_kind, richness) = build_resource_metadata(&ore, &steel, &coal, &crystal);
    let map = Map {
        seed,
        passable,
        terrain,
        elevation: vec![0; MAP_TILES],
        moisture: vec![0; MAP_TILES],
        temperature: vec![0; MAP_TILES],
        ore,
        crystal,
        steel,
        coal,
        resource_kind,
        richness,
        hq_tiles: [hq0, hq1],
    };

    if is_fully_connected(&map) {
        Some(map)
    } else {
        None
    }
}

#[allow(dead_code)]
fn open_map_legacy(seed: u64) -> Map {
    let mut rng = Rng::from_seed(seed);
    let corner = rng.below(4);
    let (hx, hy) = match corner {
        0 => (10u8, 10u8),
        1 => (10u8, 53u8),
        2 => (53u8, 10u8),
        _ => (53u8, 53u8),
    };
    let hq0 = (hx, hy);
    let hq1 = (mirror(hx), mirror(hy));
    let mut terrain = vec![Terrain::Plains; MAP_TILES];
    let mut passable = vec![true; MAP_TILES];
    let mut ore = vec![0i32; MAP_TILES];
    let mut crystal = vec![0i32; MAP_TILES];
    let mut steel = vec![0i32; MAP_TILES];
    let mut coal = vec![0i32; MAP_TILES];

    // Keep the fallback consistent with the main generator: a base mine close
    // enough for a turn-1 refinery plus a centre field.
    let (dxin, dyin) = (if hx < 32 { 1 } else { -1 }, if hy < 32 { 1 } else { -1 });
    stamp_ore_cluster_amount(
        &mut terrain,
        &mut passable,
        &mut ore,
        (hx as i32 + 3 * dxin) as u8,
        (hy as i32 + 3 * dyin) as u8,
        1300,
    );
    stamp_ore_cluster_amount(&mut terrain, &mut passable, &mut ore, 32, 22, 1500);
    stamp_crystal_cluster(&mut terrain, &mut passable, &mut crystal, 32, 42, 400);
    stamp_material_cluster_amount(
        &mut terrain,
        &mut passable,
        &mut steel,
        &mut coal,
        ResourceType::Steel,
        22,
        32,
        1200,
    );
    stamp_material_cluster_amount(
        &mut terrain,
        &mut passable,
        &mut steel,
        &mut coal,
        ResourceType::Coal,
        32,
        22,
        1200,
    );
    let (resource_kind, richness) = build_resource_metadata(&ore, &steel, &coal, &crystal);

    Map {
        seed,
        passable,
        terrain,
        elevation: vec![0; MAP_TILES],
        moisture: vec![0; MAP_TILES],
        temperature: vec![0; MAP_TILES],
        ore,
        crystal,
        steel,
        coal,
        resource_kind,
        richness,
        hq_tiles: [hq0, hq1],
    }
}

fn in_bounds(x: i32, y: i32) -> bool {
    x >= 0 && y >= 0 && x < MAP_SIZE as i32 && y < MAP_SIZE as i32
}

/// Set a tile's terrain (and derived passability); no-op out of bounds.
fn set_terrain(terrain: &mut [Terrain], passable: &mut [bool], x: i32, y: i32, t: Terrain) {
    if in_bounds(x, y) {
        let idx = tile_index(x as u8, y as u8);
        terrain[idx] = t;
        passable[idx] = t.is_passable();
    }
}

/// A rounded blob of `t` terrain (rocks → Mountain, lakes → Water, groves →
/// Forest) stamped at `(cx, cy)` and its point-mirror.
fn stamp_blob(
    terrain: &mut [Terrain],
    passable: &mut [bool],
    cx: u8,
    cy: u8,
    radius: i32,
    t: Terrain,
) {
    let r2 = radius * radius;
    for y in (cy as i32 - radius).max(0)..=(cy as i32 + radius).min(MAP_SIZE as i32 - 1) {
        for x in (cx as i32 - radius).max(0)..=(cx as i32 + radius).min(MAP_SIZE as i32 - 1) {
            let dx = x - cx as i32;
            let dy = y - cy as i32;
            if dx * dx + dy * dy <= r2 {
                set_terrain(terrain, passable, x, y, t);
            }
        }
    }
}

fn clear_around(terrain: &mut [Terrain], passable: &mut [bool], cx: u8, cy: u8, radius: i32) {
    for y in (cy as i32 - radius).max(0)..=(cy as i32 + radius).min(MAP_SIZE as i32 - 1) {
        for x in (cx as i32 - radius).max(0)..=(cx as i32 + radius).min(MAP_SIZE as i32 - 1) {
            set_terrain(terrain, passable, x, y, Terrain::Plains);
        }
    }
}

/// A rock ridge walking `steps` cells in direction `dir` from `(x0, y0)`, with
/// a periodic gate so the ridge reads as a gated canyon wall rather than a
/// sealed barricade. Single-cell thick so its point-mirror is exact, and pure
/// integer arithmetic (deterministic on native and wasm).
fn stamp_ridge(
    terrain: &mut [Terrain],
    passable: &mut [bool],
    x0: i32,
    y0: i32,
    steps: i32,
    dir: (i32, i32),
    t: Terrain,
) {
    let mut x = x0;
    let mut y = y0;
    let mut since_gate = steps; // first cell is always terrain
    for _ in 0..steps {
        if !in_bounds(x, y) {
            break;
        }
        if since_gate >= 7 {
            since_gate = 0; // leave this cell as a 1-tile gate
        } else {
            set_terrain(terrain, passable, x, y, t);
            since_gate += 1;
        }
        x += dir.0;
        y += dir.1;
    }
}

/// Broken mountain rim ringing the battlefield along all four edges. The rim
/// covers the middle ~60% of each edge and has a few carved gates, leaving
/// every corner quarter wide open for the spawns. Deterministic + symmetric.
fn stamp_rim(terrain: &mut [Terrain], passable: &mut [bool], rng: &mut Rng) {
    let thick = 1 + rng.below(2) as i64; // 1..=2 rows
    let a = rng.range(16, 26); // left gap half-width
    let b = rng.range(40, 48); // right gap half-width start
    let gates = 1 + rng.below(2) as usize; // 1..=2 carved gates

    // Top belt: x in a..=b, y in 0..thick; bottom is its point-mirror.
    for y in 0..thick {
        for x in a..=b {
            set_terrain(terrain, passable, x as i32, y as i32, Terrain::Mountain);
            set_terrain(
                terrain,
                passable,
                mirror(x as u8) as i32,
                mirror(y as u8) as i32,
                Terrain::Mountain,
            );
        }
    }
    // Left belt: x in 0..thick, y in a..=b; right is its point-mirror.
    for x in 0..thick {
        for y in a..=b {
            set_terrain(terrain, passable, x as i32, y as i32, Terrain::Mountain);
            set_terrain(
                terrain,
                passable,
                mirror(x as u8) as i32,
                mirror(y as u8) as i32,
                Terrain::Mountain,
            );
        }
    }

    // Carve gates as small clearings through the belts (and their mirrors).
    for _ in 0..gates {
        let gx = rng.range(a, b) as u8;
        let gy = rng.below(thick as u64) as u8;
        for (x, y) in [
            (gx, gy),
            (mirror(gx), mirror(gy)),
            (gy, gx),
            (mirror(gy), mirror(gx)),
        ] {
            clear_around(terrain, passable, x, y, 1);
        }
    }
}

/// Stamp a small ore cluster at `(cx, cy)` *and* its point-mirror image, so
/// the map stays exactly symmetric. The center tile is ~1.38x the nominal
/// amount and the five ring tiles ~0.92x, so the *total* ore in a field stays
/// the same as a flat cluster while rich/poor tiles make fields feel organic.
/// Ore tiles are carved to plains so they stay buildable-adjacent and reachable.
fn stamp_ore_cluster_amount(
    terrain: &mut [Terrain],
    passable: &mut [bool],
    ore: &mut [i32],
    cx: u8,
    cy: u8,
    amount: i32,
) {
    let center = amount * 18 / 13;
    let ring = amount * 12 / 13;
    for (dx, dy) in &[(0i32, 0i32), (1, 0), (0, 1), (1, 1), (-1, 1), (0, -1)] {
        let amt = if *dx == 0 && *dy == 0 { center } else { ring };
        let (x, y) = (cx as i32 + dx, cy as i32 + dy);
        if in_bounds(x, y) {
            let (x, y) = (x as u8, y as u8);
            ore[tile_index(x, y)] = amt;
            set_terrain(terrain, passable, x as i32, y as i32, Terrain::Plains);
            let (mx, my) = (mirror(x), mirror(y));
            ore[tile_index(mx, my)] = amt;
            set_terrain(terrain, passable, mx as i32, my as i32, Terrain::Plains);
        }
    }
}

/// A crystal field (rare strategic resource), mirrored like ore.
fn stamp_crystal_cluster(
    terrain: &mut [Terrain],
    passable: &mut [bool],
    crystal: &mut [i32],
    cx: u8,
    cy: u8,
    amount: i32,
) {
    for (dx, dy) in &[(0i32, 0i32), (1, 0), (0, 1), (1, 1), (-1, 1), (0, -1)] {
        let (x, y) = (cx as i32 + dx, cy as i32 + dy);
        if in_bounds(x, y) {
            let (x, y) = (x as u8, y as u8);
            crystal[tile_index(x, y)] = amount;
            set_terrain(terrain, passable, x as i32, y as i32, Terrain::Plains);
            let (mx, my) = (mirror(x), mirror(y));
            crystal[tile_index(mx, my)] = amount;
            set_terrain(terrain, passable, mx as i32, my as i32, Terrain::Plains);
        }
    }
}

/// Stamp a static Steel or Coal cluster and its point mirror. The legacy
/// amount arrays are positive markers only; every resulting deposit is
/// inexhaustible and its richness is derived below.
#[allow(clippy::too_many_arguments)]
fn stamp_material_cluster_amount(
    terrain: &mut [Terrain],
    passable: &mut [bool],
    steel: &mut [i32],
    coal: &mut [i32],
    kind: ResourceType,
    cx: u8,
    cy: u8,
    amount: i32,
) {
    let center = amount * 18 / 13;
    let ring = amount * 12 / 13;
    for (dx, dy) in &[(0i32, 0i32), (1, 0), (0, 1), (1, 1), (-1, 1), (0, -1)] {
        let amt = if *dx == 0 && *dy == 0 { center } else { ring };
        let (x, y) = (cx as i32 + dx, cy as i32 + dy);
        if !in_bounds(x, y) {
            continue;
        }
        let (x, y) = (x as u8, y as u8);
        let (mx, my) = (mirror(x), mirror(y));
        for (tx, ty) in [(x, y), (mx, my)] {
            let idx = tile_index(tx, ty);
            match kind {
                ResourceType::Steel => steel[idx] = amt,
                ResourceType::Coal => coal[idx] = amt,
                ResourceType::Ore | ResourceType::Crystal => {}
            }
            set_terrain(terrain, passable, tx as i32, ty as i32, Terrain::Plains);
        }
    }
}

fn valid_resource_center(
    passable: &[bool],
    ore: &[i32],
    steel: &[i32],
    coal: &[i32],
    crystal: &[i32],
    cx: u8,
    cy: u8,
) -> bool {
    for (dx, dy) in &[(0i32, 0i32), (1, 0), (0, 1), (1, 1), (-1, 1), (0, -1)] {
        let x = cx as i32 + dx;
        let y = cy as i32 + dy;
        if !in_bounds(x, y) {
            return false;
        }
        for (tx, ty) in [(x as u8, y as u8), (mirror(x as u8), mirror(y as u8))] {
            let idx = tile_index(tx, ty);
            if !passable[idx] || ore[idx] > 0 || steel[idx] > 0 || coal[idx] > 0 || crystal[idx] > 0
            {
                return false;
            }
        }
    }
    true
}

fn richness_for_amount(amount: i32) -> u8 {
    if amount >= 1700 {
        3
    } else if amount >= 900 {
        2
    } else if amount > 0 {
        1
    } else {
        0
    }
}

fn build_resource_metadata(
    ore: &[i32],
    steel: &[i32],
    coal: &[i32],
    crystal: &[i32],
) -> (Vec<Option<ResourceType>>, Vec<u8>) {
    let mut kinds = vec![None; MAP_TILES];
    let mut richness = vec![0; MAP_TILES];
    for idx in 0..MAP_TILES {
        let kind = if ore[idx] > 0 {
            Some(ResourceType::Ore)
        } else if steel[idx] > 0 {
            Some(ResourceType::Steel)
        } else if coal[idx] > 0 {
            Some(ResourceType::Coal)
        } else if crystal[idx] > 0 {
            Some(ResourceType::Crystal)
        } else {
            None
        };
        kinds[idx] = kind;
        richness[idx] = richness_for_amount(match kind {
            Some(ResourceType::Ore) => ore[idx],
            Some(ResourceType::Steel) => steel[idx],
            Some(ResourceType::Coal) => coal[idx],
            Some(ResourceType::Crystal) => crystal[idx],
            None => 0,
        });
    }
    (kinds, richness)
}

fn valid_site_center(passable: &[bool], x: u8, y: u8, hq0: (u8, u8), hq1: (u8, u8)) -> bool {
    if !passable[tile_index(x, y)] {
        return false;
    }
    // Not too close to either HQ (avoid merging main fields).
    let d0 = (x as i32 - hq0.0 as i32)
        .abs()
        .max((y as i32 - hq0.1 as i32).abs());
    let d1 = (x as i32 - hq1.0 as i32)
        .abs()
        .max((y as i32 - hq1.1 as i32).abs());
    d0 > 12 && d1 > 12
}

/// BFS connectivity: from each HQ, every resource tile and the enemy HQ must
/// be reachable over passable tiles (8-dir).
fn is_fully_connected(map: &Map) -> bool {
    for (start, other_hq) in [
        (map.hq_tiles[0], map.hq_tiles[1]),
        (map.hq_tiles[1], map.hq_tiles[0]),
    ] {
        let mut visited = vec![false; MAP_TILES];
        let mut stack = vec![tile_index(start.0, start.1)];
        visited[tile_index(start.0, start.1)] = true;
        while let Some(idx) = stack.pop() {
            let (x, y) = tile_coords(idx);
            for (nx, ny, _) in map.neighbors(x, y) {
                let nidx = tile_index(nx, ny);
                if !visited[nidx] {
                    visited[nidx] = true;
                    stack.push(nidx);
                }
            }
        }
        // Enemy HQ reachable.
        if !visited[tile_index(other_hq.0, other_hq.1)] {
            return false;
        }
        // Every resource tile reachable.
        for (idx, is_visited) in visited.iter().copied().enumerate().take(MAP_TILES) {
            if map.resource_amount_at((idx % MAP_SIZE) as u8, (idx / MAP_SIZE) as u8) > 0
                && !is_visited
            {
                return false;
            }
        }
    }
    true
}

/// A map with no obstacles for scenario tests that need full freedom.
#[allow(dead_code)]
pub fn open_test_map(seed: u64) -> Map {
    open_map(seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_is_deterministic_and_meets_quality_constraints() {
        let mut observed_asymmetry = false;
        for seed in 0..500u64 {
            let map = Map::generate(seed);
            let again = Map::generate(seed);
            assert_eq!(
                serde_json::to_vec(&map).unwrap(),
                serde_json::to_vec(&again).unwrap(),
                "map regeneration drifted for seed {seed}"
            );
            let empty = empty_blocked();
            let path01 = map
                .find_path(map.hq_tiles[0], map.hq_tiles[1], &empty, false)
                .unwrap();
            let path10 = map
                .find_path(map.hq_tiles[1], map.hq_tiles[0], &empty, false)
                .unwrap();
            let c01 = route_cost(&map, map.hq_tiles[0], &path01);
            let c10 = route_cost(&map, map.hq_tiles[1], &path10);
            assert!(
                (c01 - c10).abs() * 100 <= c01.max(c10).max(1) * 28,
                "route imbalance seed {seed}: {c01} vs {c10}"
            );

            let mut terrain_counts = [0usize; 8];
            let mut resource_counts = [0usize; 4];
            for idx in 0..MAP_TILES {
                terrain_counts[match map.terrain[idx] {
                    Terrain::Plains => 0,
                    Terrain::Forest => 1,
                    Terrain::Hills => 2,
                    Terrain::Desert => 3,
                    Terrain::Swamp => 4,
                    Terrain::Water => 5,
                    Terrain::River => 6,
                    Terrain::Mountain => 7,
                }] += 1;
                if let Some(kind) = map.resource_at((idx % MAP_SIZE) as u8, (idx / MAP_SIZE) as u8)
                {
                    if map.resource_amount_at((idx % MAP_SIZE) as u8, (idx / MAP_SIZE) as u8) > 0 {
                        resource_counts[kind.index()] += 1;
                    }
                }
                let (x, y) = tile_coords(idx);
                let mirror_idx = tile_index(mirror(x), mirror(y));
                if map.terrain[idx] != map.terrain[mirror_idx]
                    || map.resource_kind[idx] != map.resource_kind[mirror_idx]
                {
                    observed_asymmetry = true;
                }
            }
            assert!(terrain_counts[1] >= 35, "too few trees for seed {seed}");
            assert!(terrain_counts[2] >= 25, "too few hills for seed {seed}");
            assert!(terrain_counts[3] >= 35, "too little desert for seed {seed}");
            assert!(
                terrain_counts[4] >= 25,
                "too little wetland for seed {seed}"
            );
            assert!(terrain_counts[5] >= 25, "too few lakes for seed {seed}");
            assert!(terrain_counts[6] >= 30, "too little river for seed {seed}");
            assert!(terrain_counts[7] >= 35, "too few mountains for seed {seed}");
            assert!(resource_counts.iter().all(|&count| count >= 5));
        }
        assert!(
            observed_asymmetry,
            "generator unexpectedly remained mirrored"
        );
    }

    #[test]
    fn passability_matches_terrain() {
        for seed in 0..300u64 {
            let map = Map::generate(seed);
            for (idx, t) in map.terrain.iter().enumerate() {
                assert_eq!(
                    map.passable[idx],
                    t.is_passable(),
                    "passable/terrain mismatch seed {seed} idx {idx}"
                );
            }
        }
    }

    #[test]
    fn path_between_hqs_exists() {
        for seed in 0..200u64 {
            let map = Map::generate(seed);
            let p = map.find_path(map.hq_tiles[0], map.hq_tiles[1], &empty_blocked(), false);
            assert!(p.is_some(), "no path between HQs for seed {seed}");
        }
    }

    fn empty_blocked() -> Vec<bool> {
        vec![false; MAP_TILES]
    }

    #[test]
    fn pathfinding_is_deterministic() {
        let map = Map::generate(12345);
        let a = map.find_path(map.hq_tiles[0], map.hq_tiles[1], &empty_blocked(), false);
        let b = map.find_path(map.hq_tiles[0], map.hq_tiles[1], &empty_blocked(), false);
        assert_eq!(a, b);
        // A* on a connected map never returns a path that re-enters start.
        let path = a.unwrap();
        assert!(path.len() < MAP_TILES);
    }
}
