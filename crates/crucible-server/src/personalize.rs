//! Match-end personalization (P1 of `ADAPTIVE_AI_LEARNING.md`): the per-player
//! opponent model.
//!
//! After each match we extract a compact **fingerprint** of the human's
//! behavior from the recorded command stream (building order, unit mix, first
//! attack timing, combat tempo by T30, tech / expansion bias) and fold it into
//! a per-player `PlayerProfile` with a deterministic recency-weighted update.
//! The profile never stores raw replays (those live in `matches`); it is a
//! bounded, serializable summary that P2+ reads at serve time.
//!
//! Every entry point is **best-effort and non-fatal**: a personalization
//! failure must never surface to the player or break the match report.

use std::collections::HashMap;

use crucible_sim::{unit_stats, BuildingType, Command, Player, Replay, UnitType};
use serde::{Deserialize, Serialize};

use crate::store::Store;

/// Recency blend weight: how strongly the newest match moves the model vs the
/// accumulated history. A fresh profile starts here and is carried forward.
const DEFAULT_RECENCY: f32 = 0.7;
/// The human is always P0 in the live server (`p1_type` is the bot).
const HUMAN: Player = Player::P0;

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WinsLosses {
    pub wins: u32,
    pub losses: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct FormEntry {
    /// "w" / "l" / "d".
    pub result: String,
    /// Tier label the AI served ("easy"/"medium"/"hard"/"champion").
    pub tier: String,
    #[serde(default)]
    pub difficulty: Option<f32>,
}

/// The per-player opponent model: a compact, deterministic, bounded summary of
/// how this player plays, unfolded from their matches.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct PlayerProfile {
    /// Building/unit ramps observed, key -> blend weight.
    #[serde(default)]
    pub opening_mix: HashMap<String, f32>,
    /// Fraction of trained units per unit type, key -> blend weight.
    #[serde(default)]
    pub unit_mix: HashMap<String, f32>,
    /// Earliest attack round observed per served tier.
    #[serde(default)]
    pub rush_attack: HashMap<String, i32>,
    /// Combat tempo (0..1): units on the field by turn 30.
    pub tempo: f32,
    /// Tech inclination 0..1 (research / early TechLab).
    pub tech_bias: f32,
    /// Expansion aggressiveness 0..1 (refineries claimed).
    pub expansion_bias: f32,
    /// Outcome tally per served tier.
    #[serde(default)]
    pub vs_archetype: HashMap<String, WinsLosses>,
    /// Recent results (oldest first), capped.
    #[serde(default)]
    pub recent_form: Vec<FormEntry>,
    /// Blend weight for the newest observation.
    #[serde(default = "default_recency")]
    pub recency_weight: f32,
}

fn default_recency() -> f32 {
    DEFAULT_RECENCY
}

// ---------------------------------------------------------------------------
// Fingerprint extraction
// ---------------------------------------------------------------------------

/// One match's worth of player behavior, read off the command stream.
struct Fingerprint {
    /// First few ramp tokens (building/unit), e.g. "R>F>B".
    opening_key: Option<String>,
    /// Trained-unit fractions by unit type.
    unit_shares: HashMap<String, f32>,
    /// First round this player issued an attack (proxy for rush timing).
    first_attack_round: Option<i32>,
    /// Combat strength by turn 30, normalized 0..1.
    tempo: f32,
    /// Built a TechLab or researched anything.
    tech: bool,
    /// Refineries claimed, normalized 0..1.
    expansion: f32,
}

fn building_abbrev(bt: BuildingType) -> &'static str {
    match bt {
        BuildingType::PowerPlant => "P",
        BuildingType::Refinery => "R",
        BuildingType::CrystalRefinery => "CR",
        BuildingType::Barracks => "B",
        BuildingType::Factory => "F",
        BuildingType::TechLab => "T",
        BuildingType::Airfield => "A",
        BuildingType::Radar => "RD",
        BuildingType::TeslaCoil => "TC",
        BuildingType::Turret => "TU",
        BuildingType::AATurret => "AA",
        _ => "?",
    }
}

fn unit_token(ut: UnitType) -> String {
    let name = format!("{ut:?}");
    name[..name.len().min(3)].to_lowercase()
}

/// Fold `replay`'s command stream (for `human`) into a match fingerprint.
fn extract_fingerprint(replay: &Replay, human: Player) -> Fingerprint {
    let mut opening: Vec<String> = Vec::new();
    let mut unit_counts: HashMap<String, u32> = HashMap::new();
    let mut total_trains: u32 = 0;
    let mut combat_by_30: u32 = 0;
    let mut refineries: u32 = 0;
    let mut tech = false;
    let mut first_attack: Option<i32> = None;

    for tc in &replay.commands {
        if tc.player != human {
            continue;
        }
        match &tc.command {
            Command::PlaceBuilding { btype, .. } => {
                if opening.len() < 6 {
                    opening.push(building_abbrev(*btype).to_string());
                }
                if btype.is_refinery() {
                    refineries += 1;
                }
                if *btype == BuildingType::TechLab {
                    tech = true;
                }
            }
            Command::TrainUnit { utype, .. } => {
                if opening.len() < 6 {
                    opening.push(unit_token(*utype));
                }
                let tok = unit_token(*utype);
                *unit_counts.entry(tok).or_insert(0) += 1;
                total_trains += 1;
                if tc.round <= 30 && unit_stats(*utype).damage > 0 {
                    combat_by_30 += 1;
                }
            }
            Command::StartResearch { .. } => tech = true,
            Command::Attack { .. }
                if first_attack.is_none() => {
                    first_attack = Some(tc.round);
                }
            _ => {}
        }
    }

    let unit_shares = if total_trains > 0 {
        unit_counts
            .into_iter()
            .map(|(k, v)| (k, v as f32 / total_trains as f32))
            .collect()
    } else {
        HashMap::new()
    };

    Fingerprint {
        opening_key: {
            let k = opening.join(">");
            if k.is_empty() {
                None
            } else {
                Some(k)
            }
        },
        unit_shares,
        first_attack_round: first_attack,
        // ~20 combat units by T30 is a fairly massed army.
        tempo: (combat_by_30 as f32 / 20.0).clamp(0.0, 1.0),
        tech,
        // ~4 refineries is a very expand-heavy game.
        expansion: (refineries as f32 / 4.0).clamp(0.0, 1.0),
    }
}

// ---------------------------------------------------------------------------
// Profile update
// ---------------------------------------------------------------------------

fn lerp(a: f32, b: f32, w: f32) -> f32 {
    a * (1.0 - w) + b * w
}

/// Decay every existing weight by `(1-w)`, then add `w` to the observed keys.
fn decay_and_add(map: &mut HashMap<String, f32>, observed: &HashMap<String, f32>, w: f32) {
    for v in map.values_mut() {
        *v *= 1.0 - w;
    }
    for (k, v) in observed {
        let e = map.entry(k.clone()).or_insert(0.0);
        *e += w * v;
    }
}

fn update(
    mut profile: PlayerProfile,
    fp: &Fingerprint,
    tier: &str,
    difficulty: Option<f32>,
    human_won: bool,
) -> PlayerProfile {
    let w = profile.recency_weight.clamp(0.1, 0.9);

    // Scalar, exponentially-weighted knobs.
    profile.tempo = lerp(profile.tempo, fp.tempo, w);
    profile.tech_bias = lerp(profile.tech_bias, fp.tech as u8 as f32, w);
    profile.expansion_bias = lerp(profile.expansion_bias, fp.expansion, w);

    // Categorical ramp/unit blends.
    let mut opening_obs = HashMap::new();
    if let Some(k) = &fp.opening_key {
        opening_obs.insert(k.clone(), 1.0);
    }
    decay_and_add(&mut profile.opening_mix, &opening_obs, w);
    decay_and_add(&mut profile.unit_mix, &fp.unit_shares, w);

    // Rush timing: earliest attack round per tier (min-merge).
    if let Some(round) = fp.first_attack_round {
        let e = profile
            .rush_attack
            .entry(tier.to_string())
            .or_insert(i32::MAX);
        *e = (*e).min(round);
    }

    // Outcome tally per tier.
    let vs = profile
        .vs_archetype
        .entry(tier.to_string())
        .or_insert_with(|| WinsLosses { wins: 0, losses: 0 });
    if human_won {
        vs.wins += 1;
    } else {
        vs.losses += 1;
    }

    // Recent form (capped).
    profile.recent_form.push(FormEntry {
        result: if human_won { "w" } else { "l" }.to_string(),
        tier: tier.to_string(),
        difficulty,
    });
    const MAX_FORM: usize = 12;
    while profile.recent_form.len() > MAX_FORM {
        profile.recent_form.remove(0);
    }

    profile
}

// ---------------------------------------------------------------------------
// Serving helpers
// ---------------------------------------------------------------------------

/// Map a server opponent label to a tier tag for the profile.
fn opponent_tier(opponent: &str) -> &'static str {
    match opponent {
        "easy" => "easy",
        "medium" => "medium",
        "hard" => "hard",
        "adaptive" => "medium",
        _ => {
            if let Some(scalar) = opponent.strip_prefix("adaptive:") {
                if let Ok(d) = scalar.parse::<f32>() {
                    return crucible_ai::tier_name(d);
                }
            }
            "champion"
        }
    }
}

/// Map a server opponent label to a difficulty scalar (for form entries).
fn opponent_difficulty(opponent: &str) -> Option<f32> {
    match opponent {
        "easy" => Some(0.3),
        "medium" => Some(0.55),
        "hard" => Some(0.85),
        "adaptive" => Some(0.55),
        _ => opponent
            .strip_prefix("adaptive:")
            .and_then(|s| s.parse::<f32>().ok()),
    }
}

// ---------------------------------------------------------------------------
// Public hook
// ---------------------------------------------------------------------------

/// `store` best-effort persistence is handled internally and never propagated.
fn load(store: &Store, player_id: &str) -> PlayerProfile {
    match store.get_player_profile(player_id) {
        Ok(Some(json)) => serde_json::from_str(&json).unwrap_or_default(),
        _ => PlayerProfile::default(),
    }
}

fn save(store: &Store, player_id: &str, profile: &PlayerProfile) -> Result<(), ()> {
    let json = serde_json::to_string(profile).map_err(|_| ())?;
    store.save_player_profile(player_id, &json).map_err(|_| ())
}

/// Extract a fingerprint from the finished match and fold it into the
/// player's model. Best-effort; logs on failure.
pub fn record_match(
    store: &Store,
    player_id: &str,
    replay: &Replay,
    winner: Option<Player>,
    opponent: &str,
) {
    if player_id.trim().is_empty() {
        return;
    }
    // Ensure the players row exists: it is both the P0 match counter and the
    // FK parent for player_profiles.
    if let Err(e) = store.note_player_match(player_id) {
        tracing::warn!("failed to note player match for {player_id}: {e}");
    }
    let tier = opponent_tier(opponent);
    let difficulty = opponent_difficulty(opponent);
    let human_won = winner == Some(HUMAN);

    let mut profile = load(store, player_id);
    let fp = extract_fingerprint(replay, HUMAN);
    profile = update(profile, &fp, tier, difficulty, human_won);

    if let Err(()) = save(store, player_id, &profile) {
        tracing::warn!("failed to save player profile for {player_id}");
    }
}

/// The recency weight parameter, exposed so P2 config can read it if needed.
#[allow(dead_code)] // P2 serving layer reads this
pub fn recency_weight() -> f32 {
    DEFAULT_RECENCY
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crucible_sim::tech::TechId;
    use crucible_sim::{Command, GameConfig};

    fn replay_with(cmds: &[(i32, Command)]) -> Replay {
        let mut r = Replay::new(7, GameConfig::default());
        for (round, cmd) in cmds {
            let player = match cmd {
                Command::PlaceBuilding { player, .. } => *player,
                Command::TrainUnit { player, .. } => *player,
                Command::Attack { player, .. } => *player,
                Command::StartResearch { player, .. } => *player,
                _ => Player::P0,
            };
            r.record_at(*round, (round * 2) - 1, player, cmd.clone());
        }
        r
    }

    fn pb(bt: BuildingType, x: u8, y: u8) -> Command {
        Command::PlaceBuilding {
            player: Player::P0,
            btype: bt,
            tile: (x, y),
        }
    }
    fn tr(ut: UnitType) -> Command {
        Command::TrainUnit {
            player: Player::P0,
            building: 1,
            utype: ut,
        }
    }

    #[test]
    fn fingerprint_reflects_command_stream() {
        let replay = replay_with(&[
            (2, pb(BuildingType::Refinery, 2, 0)),
            (3, pb(BuildingType::Factory, 0, 2)),
            (4, pb(BuildingType::Barracks, 2, 2)),
            (5, tr(UnitType::Infantry)),
            (6, tr(UnitType::Infantry)),
            (7, tr(UnitType::Tank)),
            (12, Command::Attack {
                player: Player::P0,
                units: vec![1],
                target: 9,
            }),
            (14, Command::StartResearch {
                player: Player::P0,
                tech: TechId::HighExplosive,
            }),
        ]);
        let fp = extract_fingerprint(&replay, Player::P0);
        // Opening ramp: R > F > B then the infantry/other trains.
        assert_eq!(fp.opening_key.as_deref(), Some("R>F>B>inf>inf>tan"));
        // 2 infantry + 1 tank = 3 trains; tempo mid.
        assert_eq!(fp.unit_shares.get("inf"), Some(&(2.0 / 3.0)));
        assert_eq!(fp.unit_shares.get("tan"), Some(&(1.0 / 3.0)));
        assert!(fp.tempo > 0.0);
        assert!(fp.tech);
        assert_eq!(fp.first_attack_round, Some(12));
    }

    #[test]
    fn update_folds_recency_and_counts_outcome() {
        let replay = replay_with(&[
            (2, pb(BuildingType::Refinery, 2, 0)),
            (5, tr(UnitType::Tank)),
            (12, Command::Attack {
                player: Player::P0,
                units: vec![1],
                target: 9,
            }),
        ]);
        let fp = extract_fingerprint(&replay, Player::P0);

        let p1 = update(PlayerProfile::default(), &fp, "hard", Some(0.85), true);
        assert_eq!(p1.vs_archetype["hard"].wins, 1);
        assert_eq!(p1.vs_archetype["hard"].losses, 0);
        assert_eq!(p1.rush_attack["hard"], 12);
        assert_eq!(p1.recent_form.len(), 1);

        // A later, later-attacking match keeps the earliest rush.
        let mut replay2 = replay.clone();
        replay2.commands.retain(|_t| false);
        replay2.record_at(20, 39, Player::P0, Command::Attack {
            player: Player::P0,
            units: vec![1],
            target: 9,
        });
        let fp2 = extract_fingerprint(&replay2, Player::P0);
        let p2 = update(p1.clone(), &fp2, "hard", Some(0.85), false);
        assert_eq!(p2.rush_attack["hard"], 12, "earliest rush is kept");
        assert_eq!(p2.vs_archetype["hard"].losses, 1);
        assert_eq!(p2.recent_form.len(), 2);

        // Determinism: same input -> same output.
        let p3 = update(p1.clone(), &fp2, "hard", Some(0.85), false);
        assert_eq!(p2, p3);
    }

    #[test]
    fn profiles_are_independent() {
        let store = Store::in_memory().unwrap();
        let replay = replay_with(&[(2, pb(BuildingType::Refinery, 2, 0))]);
        record_match(&store, "alice", &replay, Some(Player::P0), "hard");
        record_match(&store, "bob", &replay, Some(Player::P0), "medium");
        let a = load(&store, "alice");
        let b = load(&store, "bob");
        assert_eq!(a.vs_archetype["hard"].wins, 1);
        assert!(!a.vs_archetype.contains_key("medium"));
        assert_eq!(b.vs_archetype["medium"].wins, 1);
        assert!(!b.vs_archetype.contains_key("hard"));
    }

    #[test]
    fn missing_opponent_tags_as_champion() {
        let store = Store::in_memory().unwrap();
        let replay = replay_with(&[(2, pb(BuildingType::Refinery, 2, 0))]);
        record_match(&store, "c", &replay, Some(Player::P0), "champion");
        let p = load(&store, "c");
        assert_eq!(p.vs_archetype["champion"].wins, 1);
        assert_eq!(p.recent_form[0].difficulty, None);
    }
}