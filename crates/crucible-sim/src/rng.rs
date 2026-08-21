//! The single injected source of randomness.
//!
//! The sim never draws entropy from anywhere else. Map generation and any
//! stochastic-but-reproducible choice flows through this wrapper, seeded from
//! a `u64`.
//!
//! This is a self-contained xoshiro256** (seeded via SplitMix64) rather than
//! an external rand crate: it is ~30 lines of pure integer arithmetic, has no
//! `getrandom`/OS dependency (which would break the pure wasm target), and is
//! auditable-by-inspection as byte-identical on native and wasm.

/// Deterministic seeded PRNG.
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    /// Create a PRNG from a 64-bit seed.
    pub fn from_seed(seed: u64) -> Self {
        let mut state = seed;
        let mut s = [0u64; 4];
        for slot in &mut s {
            state = splitmix64(state);
            *slot = state;
        }
        Rng { s }
    }

    /// Next raw 64-bit value (xoshiro256**).
    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Next raw 32-bit value.
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniform value in `[0, n)`. `n` must be `> 0`.
    ///
    /// Uses modulo (with negligible bias at our scales); determinism, not
    /// cryptographic uniformity, is the requirement here.
    pub fn below(&mut self, n: u64) -> u64 {
        debug_assert!(n > 0, "Rng::below called with n=0");
        self.next_u64() % n
    }

    /// Uniform value in `[lo, hi)`.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        debug_assert!(hi > lo, "Rng::range requires hi > lo");
        lo + (self.below((hi - lo) as u64) as i64)
    }

    /// True with probability `num / denom`.
    pub fn chance(&mut self, num: u64, denom: u64) -> bool {
        debug_assert!(denom > 0);
        self.below(denom) < num
    }

    /// Fisher-Yates shuffle of a slice (deterministic given the seed).
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below((i + 1) as u64) as usize;
            items.swap(i, j);
        }
    }
}

#[inline]
fn splitmix64(state: u64) -> u64 {
    let mut z = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let mut a = Rng::from_seed(0xDEADBEEF);
        let mut b = Rng::from_seed(0xDEADBEEF);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_differ() {
        let mut a = Rng::from_seed(1);
        let mut b = Rng::from_seed(2);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn below_stays_in_range() {
        let mut r = Rng::from_seed(42);
        for _ in 0..10_000 {
            let v = r.below(64);
            assert!(v < 64);
        }
    }

    #[test]
    fn known_sequence_is_stable() {
        // Pin the exact stream so any accidental change to the PRNG is caught.
        let mut r = Rng::from_seed(0);
        let first8: Vec<u64> = (0..8).map(|_| r.next_u64()).collect();
        assert_eq!(
            first8,
            vec![
                1905207664160064169,
                7642312046547803776,
                7003759831383473959,
                2435594535647819530,
                9339948524129368383,
                12646608302616112355,
                3055573321689229946,
                17495720581888191501,
            ]
        );
    }
}
