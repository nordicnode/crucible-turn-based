//! Integer tile geometry.
//!
//! The turn-based game has no continuous positions: everything is a tile
//! `(u8, u8)`. **Range envelopes use Chebyshev distance** — `max(|dx|,|dy|)`
//! — matching the 8-directional movement grid: a unit's range-1 envelope is
//! its full 3×3 neighborhood, so diagonal adjacency counts as contact and
//! two armies marching at each other can never interleave into a mutual
//! out-of-range stall. No floats, no sqrt.

/// Chebyshev distance between two tiles (8-directional move count).
#[inline]
pub fn chebyshev(ax: u8, ay: u8, bx: u8, by: u8) -> i32 {
    let dx = (ax as i32 - bx as i32).abs();
    let dy = (ay as i32 - by as i32).abs();
    dx.max(dy)
}

/// Whether tile `b` is within Chebyshev radius `r` of tile `a`.
#[inline]
pub fn within_range(ax: u8, ay: u8, bx: u8, by: u8, r: i32) -> bool {
    chebyshev(ax, ay, bx, by) <= r
}
