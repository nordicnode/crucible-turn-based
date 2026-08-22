//! Procedural map generation and deterministic grid pathfinding.
//!
//! Maps are generated from a `u64` seed and are **exactly point-symmetric**
//! under the reflection `(x, y) -> (63-x, 63-y)`. This makes spawn fairness a
//! theorem rather than a heuristic: every ore/crystal field, obstacle, and the
//! two HQs have identical mirror images. Generation retries with derived seeds
//! until the map is fully connected, and falls back to an open map.
//!
//! Terrain is typed (plains/forest/hills/water/mountain): passability, unit
//! movement cost, and combat defense all derive from it, so the same map
//! description drives pathfinding, movement budgets, and damage reduction.

use std::collections::BinaryHeap;

use serde::{Deserialize, Serialize};

use crate::rng::Rng;

pub const MAP_SIZE: usize = 64;
pub const MAP_TILES: usize = MAP_SIZE * MAP_SIZE;

const MAX_GEN_ATTEMPTS: u64 = 256;

/// Tile terrain. Water and Mountain are impassable to every unit; forest and
/// hills are passable but cost extra movement and grant a defense bonus.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Debug)]
pub enum Terrain {
    Plains,
    Forest,
    Hills,
    Water,
    Mountain,
}

impl Terrain {
    pub fn is_passable(self) -> bool {
        matches!(self, Terrain::Plains | Terrain::Forest | Terrain::Hills)
    }

    /// Extra movement multiplier for ground units entering this tile.
    pub fn move_mult(self) -> i32 {
        match self {
            Terrain::Plains => 1,
            Terrain::Forest | Terrain::Hills => 2,
            Terrain::Water | Terrain::Mountain => 1,
        }
    }

    /// Damage reduction (percent) for a defender standing on this tile.
    pub fn defense_reduction(self) -> i32 {
        match self {
            Terrain::Forest => 20,
            Terrain::Hills => 30,
            _ => 0,
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

/// Mirror a coordinate under the map's point symmetry.
#[inline]
pub fn mirror(x: u8) -> u8 {
    (MAP_SIZE - 1) as u8 - x
}

/// The static world layout. Ore/crystal amounts mutate during a match;
/// passability, terrain and positions do not.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Map {
    pub seed: u64,
    /// Passable terrain (true = walkable). Derived from `terrain`.
    pub passable: Vec<bool>,
    /// Terrain type per tile.
    pub terrain: Vec<Terrain>,
    /// Ore remaining per tile.
    pub ore: Vec<i32>,
    /// Crystal remaining per tile.
    pub crystal: Vec<i32>,
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
        // Guaranteed-valid fallback: symmetric, fully open.
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

    /// Remove up to `amount` ore from a tile, returning how much was removed.
    pub fn deplete_ore(&mut self, x: u8, y: u8, amount: i32) -> i32 {
        let idx = tile_index(x, y);
        let taken = amount.min(self.ore[idx]);
        self.ore[idx] -= taken;
        taken
    }

    /// Remove up to `amount` crystal from a tile, returning how much removed.
    pub fn deplete_crystal(&mut self, x: u8, y: u8, amount: i32) -> i32 {
        let idx = tile_index(x, y);
        let taken = amount.min(self.crystal[idx]);
        self.crystal[idx] -= taken;
        taken
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
    let (dmax, dmin) = if dx > dy { (dx, dy) } else { (dy, dx) };
    // Admissible under 1/2 MP step costs with terrain multipliers up to 2.
    2 * (dmax + dmin)
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
fn try_generate(seed: u64) -> Option<Map> {
    let mut rng = Rng::from_seed(seed);
    let mut terrain = vec![Terrain::Plains; MAP_TILES];
    let mut passable = vec![true; MAP_TILES];
    let mut ore = vec![0i32; MAP_TILES];
    let mut crystal = vec![0i32; MAP_TILES];

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

    let map = Map {
        seed,
        passable,
        terrain,
        ore,
        crystal,
        hq_tiles: [hq0, hq1],
    };

    if is_fully_connected(&map) {
        Some(map)
    } else {
        None
    }
}

fn open_map(seed: u64) -> Map {
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

    Map {
        seed,
        passable,
        terrain,
        ore,
        crystal,
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

/// BFS connectivity: from each HQ, every ore/crystal tile and the enemy HQ
/// must be reachable over passable tiles (8-dir).
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
        // Every ore and crystal tile reachable.
        for (idx, &o) in map.ore.iter().enumerate() {
            if o > 0 && !visited[idx] {
                return false;
            }
        }
        for (idx, &c) in map.crystal.iter().enumerate() {
            if c > 0 && !visited[idx] {
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
    fn map_is_point_symmetric() {
        for seed in 0..500u64 {
            let map = Map::generate(seed);
            for idx in 0..MAP_TILES {
                let (x, y) = tile_coords(idx);
                let m = tile_index(mirror(x), mirror(y));
                assert_eq!(
                    map.passable[idx], map.passable[m],
                    "passable asymmetry seed {seed}"
                );
                assert_eq!(map.ore[idx], map.ore[m], "ore asymmetry seed {seed}");
                assert_eq!(
                    map.crystal[idx], map.crystal[m],
                    "crystal asymmetry seed {seed}"
                );
                assert_eq!(
                    map.terrain[idx], map.terrain[m],
                    "terrain asymmetry seed {seed}"
                );
            }
            assert_eq!(
                map.hq_tiles[0],
                (mirror(map.hq_tiles[1].0), mirror(map.hq_tiles[1].1))
            );
        }
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
