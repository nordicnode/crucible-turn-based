//! Match-end personalization hook (P0–P3 of `ADAPTIVE_AI_LEARNING.md`).
//!
//! P0: record that a player finished a match against the AI (`players`
//! aggregate). Later phases add the per-player strategy profile (L1), the
//! counter-selection layer (L2), and the replay-DB mint job (L3).
//!
//! Every entry point here is **best-effort and non-fatal**: a personalization
//! failure must never surface to the player or break the match report.

use crate::store::Store;

/// Record that `player_id` finished a match against the AI. Upserts the
/// `players` row (first/last seen + match count); no-op for a missing id.
pub fn record_match(store: &Store, player_id: &str) {
    if player_id.trim().is_empty() {
        return;
    }
    if let Err(e) = store.note_player_match(player_id) {
        tracing::warn!("failed to record match for player {player_id}: {e}");
    }
}