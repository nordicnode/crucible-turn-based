//! Hand-rolled feed-forward MLP on flat `Vec<f32>` arrays. Forward pass only —
//! CRUCIBLE never backpropagates (evolution strategy mutates weights directly).
//!
//! Layout: FEATURE_DIM -> 48 (tanh) -> 48 (tanh) -> OUTPUT (linear scores).
//! The genome is the flat concatenation `[W1, b1, W2, b2, W3, b3]`.
//!
//! # Genome schema
//!
//! `GENOME_SCHEMA_VERSION` is bumped whenever the encoding changes shape or
//! meaning (the v5 turn-based cutover removed the Harvester train slot). All
//! genomes stored under an older version are void — clean cutover.

use crucible_sim::Rng;

use crate::features::FEATURE_DIM;

pub const HIDDEN1: usize = 48;
pub const HIDDEN2: usize = 48;

/// Genome schema version. Bumped on every change to the encoding; stored
/// genomes with a different version are invalid.
pub const GENOME_SCHEMA_VERSION: u32 = 6;

/// Output head layout (see `decision.rs`).
pub const BUILD_OUT: usize = 8;
/// Train slots: infantry, tank, artillery, mammoth, gunship, interceptor.
pub const TRAIN_OUT: usize = 6;
/// Army-wide actions: attack-move, defend, scout, and focus-fire (snipe).
pub const ARMY_ACTION_OUT: usize = 4;
pub const SECTOR_OUT: usize = 64;
pub const TECH_OUT: usize = 4;
/// Snipe target-type head (used only when the army action is `Snipe`):
/// enemy tank, refinery, HQ, or factory.
pub const SNIPE_OUT: usize = 4;
pub const OUTPUT: usize =
    BUILD_OUT + TRAIN_OUT + ARMY_ACTION_OUT + SECTOR_OUT + TECH_OUT + SNIPE_OUT;

pub const W1: usize = FEATURE_DIM * HIDDEN1;
pub const B1: usize = HIDDEN1;
pub const W2: usize = HIDDEN1 * HIDDEN2;
pub const B2: usize = HIDDEN2;
pub const W3: usize = HIDDEN2 * OUTPUT;
pub const B3: usize = OUTPUT;

pub const GENOME_LEN: usize = W1 + B1 + W2 + B2 + W3 + B3;

fn uniform(rng: &mut Rng) -> f32 {
    rng.next_u32() as f32 / u32::MAX as f32
}

/// Standard normal via the sum-of-12-uniforms CLT trick: fully deterministic
/// across platforms (no `ln`/`cos`/`sqrt`), good enough for ES mutation.
fn gaussian(rng: &mut Rng) -> f32 {
    let sum: f32 = (0..12).map(|_| uniform(rng)).sum();
    sum - 6.0
}

/// Xavier-uniform init over `[-b, b]` with `b = sqrt(6 / (fan_in + fan_out))`.
fn xavier_bound(fan_in: usize, fan_out: usize) -> f32 {
    (6.0 / (fan_in + fan_out) as f32).sqrt()
}

/// Initialize a random genome.
pub fn init(rng: &mut Rng) -> Vec<f32> {
    let mut g = vec![0.0f32; GENOME_LEN];
    let mut o = 0;
    init_layer(rng, &mut g[o..o + W1], FEATURE_DIM, HIDDEN1);
    o += W1;
    o += B1; // biases start at zero
    init_layer(rng, &mut g[o..o + W2], HIDDEN1, HIDDEN2);
    o += W2;
    o += B2;
    init_layer(rng, &mut g[o..o + W3], HIDDEN2, OUTPUT);
    g
}

fn init_layer(rng: &mut Rng, w: &mut [f32], fan_in: usize, fan_out: usize) {
    let b = xavier_bound(fan_in, fan_out);
    for v in w.iter_mut() {
        *v = (uniform(rng) * 2.0 - 1.0) * b;
    }
}

/// `out[i] = b[i] + sum_j w[i*n + j] * x[j]`, applying tanh if requested.
///
/// The bias slice sits immediately after the weights in the genome layout.
fn affine(w: &[f32], b: &[f32], x: &[f32], out: &mut [f32], activate: bool) {
    for (i, o) in out.iter_mut().enumerate() {
        let row = &w[i * x.len()..(i + 1) * x.len()];
        let mut s = b[i];
        for (&wv, &xv) in row.iter().zip(x.iter()) {
            s += wv * xv;
        }
        *o = if activate { s.tanh() } else { s };
    }
}

/// Forward pass. Input length must equal [`FEATURE_DIM`].
pub fn forward(genome: &[f32], input: &[f32]) -> Vec<f32> {
    assert_eq!(
        input.len(),
        FEATURE_DIM,
        "network input must be FEATURE_DIM"
    );
    debug_assert_eq!(genome.len(), GENOME_LEN);

    let mut h1 = vec![0.0f32; HIDDEN1];
    let mut h2 = vec![0.0f32; HIDDEN2];
    let mut out = vec![0.0f32; OUTPUT];

    let mut o = 0;
    affine(
        &genome[o..o + W1],
        &genome[o + W1..o + W1 + B1],
        input,
        &mut h1,
        true,
    );
    o += W1 + B1;
    affine(
        &genome[o..o + W2],
        &genome[o + W2..o + W2 + B2],
        &h1,
        &mut h2,
        true,
    );
    o += W2 + B2;
    affine(
        &genome[o..o + W3],
        &genome[o + W3..o + W3 + B3],
        &h2,
        &mut out,
        false,
    );
    out
}

/// Gaussian mutation in place. `sigma` is the per-weight standard deviation;
/// `macro_rate` is the probability of re-perturbing each weight at `3*sigma`.
pub fn mutate(rng: &mut Rng, genome: &mut [f32], sigma: f32, macro_rate: f32) {
    for v in genome.iter_mut() {
        *v += gaussian(rng) * sigma;
        if uniform(rng) < macro_rate {
            *v += gaussian(rng) * sigma * 3.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genome_len_matches_layer_sizes() {
        // FEATURE_DIM is 224 with the plan §5.2 history embedding (2 stacked
        // turns): W1 = 224*48, W3 = 48*90 (TRAIN_OUT dropped to 6 in the
        // turn-based schema; the snipe head keeps its 4 target outputs and
        // the army head its 4 actions).
        assert_eq!(OUTPUT, 90);
        assert_eq!(GENOME_LEN, 224 * 48 + 48 + 48 * 48 + 48 + 48 * 90 + 90);
        assert_eq!(GENOME_LEN, 17_562);
    }

    #[test]
    fn forward_is_deterministic_and_bounded() {
        let mut rng = Rng::from_seed(7);
        let g = init(&mut rng);
        let input = vec![0.5f32; FEATURE_DIM];
        let a = forward(&g, &input);
        let b = forward(&g, &input);
        assert_eq!(a, b);
        assert_eq!(a.len(), OUTPUT);
        for v in &a {
            assert!(v.is_finite());
        }
    }

    #[test]
    fn mutate_changes_weights_and_stays_finite() {
        let mut rng = Rng::from_seed(3);
        let mut g = init(&mut rng);
        let before = g.clone();
        mutate(&mut rng, &mut g, 0.05, 0.1);
        assert_ne!(g, before);
        assert!(g.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn same_seed_same_genome() {
        let mut a = Rng::from_seed(42);
        let mut b = Rng::from_seed(42);
        assert_eq!(init(&mut a), init(&mut b));
    }

    #[test]
    fn zero_genome_forward_is_zero() {
        let g = vec![0.0f32; GENOME_LEN];
        let input = vec![0.25f32; FEATURE_DIM];
        let out = forward(&g, &input);
        assert!(out.iter().all(|&v| v == 0.0));
    }
}
