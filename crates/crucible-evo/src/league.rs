//! Elo tracking for league matches. Standard Elo with K=24 and draws handled
//! (a draw is half a win). Server-side only: this is *metadata*, not sim state,
//! so it lives outside the determinism contract that governs game-state math.

/// K-factor: how many Elo points one match can move a rating.
pub const K: f32 = 24.0;

/// Expected score for `rating` against `opponent` (in [0, 1]).
pub fn expected(rating: f32, opponent: f32) -> f32 {
    1.0 / (1.0 + 10f32.powf((opponent - rating) / 400.0))
}

/// Actual score for a result. Win = 1, draw = 0.5, loss = 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Win,
    Draw,
    Loss,
}

impl Outcome {
    fn score(self) -> f32 {
        match self {
            Outcome::Win => 1.0,
            Outcome::Draw => 0.5,
            Outcome::Loss => 0.0,
        }
    }
}

/// Update a rating after one match. Returns the new rating.
pub fn update(rating: f32, opponent: f32, result: Outcome) -> f32 {
    rating + K * (result.score() - expected(rating, opponent))
}

/// A monotonic sequence of Elo samples for one genome.
#[derive(Clone, Debug, Default)]
pub struct EloHistory {
    pub entries: Vec<(u32, f32)>, // (league match count so far, rating)
}

impl EloHistory {
    /// Record a new sample; `matches_played` is the running total after this
    /// match (must be strictly increasing for a clean history).
    pub fn record(&mut self, matches_played: u32, rating: f32) {
        self.entries.push((matches_played, rating));
    }

    pub fn latest(&self) -> Option<f32> {
        self.entries.last().map(|(_, r)| *r)
    }

    pub fn as_points(&self) -> Vec<(u32, f32)> {
        self.entries.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn higher_rating_wins_gain_less_than_lower_rating_upset() {
        // Equal match: winner takes ~half of K.
        let a = update(1500.0, 1500.0, Outcome::Win);
        assert!((a - 1512.0).abs() < 0.001);

        // Upset: 1400 beats 1600 → big gain.
        let upset = update(1400.0, 1600.0, Outcome::Win);
        let favorite = update(1600.0, 1400.0, Outcome::Win);
        assert!(upset - 1400.0 > favorite - 1600.0);

        // Draw pulls both ratings toward each other.
        let low_after_draw = update(1400.0, 1600.0, Outcome::Draw);
        let high_after_draw = update(1600.0, 1400.0, Outcome::Draw);
        assert!(low_after_draw > 1400.0);
        assert!(high_after_draw < 1600.0);
    }

    #[test]
    fn history_records_latest() {
        let mut h = EloHistory::default();
        assert_eq!(h.latest(), None);
        h.record(1, 1480.0);
        h.record(2, 1500.0);
        assert_eq!(h.latest(), Some(1500.0));
        assert_eq!(h.as_points().len(), 2);
    }
}
