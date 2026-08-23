//! SQLite persistence. The only place that talks to the database.
//!
//! Schema is versioned via `PRAGMA user_version`; migrations run at boot.
//! Replays are stored as their JSON input log so they stay re-runnable forever.
//! Genomes are stored as JSON weight arrays; champions carry their gauntlet
//! record for full reproducibility.

use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: i32 = 8;

const MIGRATION_V1: &str = "
CREATE TABLE IF NOT EXISTS matches (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    map_seed INTEGER NOT NULL,
    p1_type TEXT NOT NULL,
    p2_type TEXT NOT NULL,
    result TEXT NOT NULL,
    duration_ticks INTEGER NOT NULL,
    replay TEXT NOT NULL,
    created_at INTEGER NOT NULL
);";

const MIGRATION_V2: &str = "
CREATE TABLE IF NOT EXISTS genomes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    generation INTEGER NOT NULL,
    parent_id INTEGER,
    weights TEXT NOT NULL,
    born_from TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS champions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    genome_id INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    crowned_at INTEGER NOT NULL,
    dethroned_at INTEGER,
    gauntlet_record TEXT
);
CREATE TABLE IF NOT EXISTS elo_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    genome_id INTEGER NOT NULL,
    elo REAL NOT NULL,
    at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    at INTEGER NOT NULL
);";

const MIGRATION_V3: &str = "
CREATE TABLE IF NOT EXISTS trainer_state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS training_stats (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    generation INTEGER NOT NULL,
    matches_run INTEGER NOT NULL,
    pop_fitness_mean REAL NOT NULL,
    pop_fitness_best REAL NOT NULL,
    diversity REAL NOT NULL,
    at INTEGER NOT NULL
);";

/// Turn-based cutover (clean cutover): replays are now measured in turns, and
/// every pre-cutover row (matches, genomes, champions, Elo, stats, events) is
/// void — old realtime replays are unwatchable and old genomes have the wrong
/// network shape. The `matches` table is recreated with `duration_turns` and
/// the rest is emptied so the trainer resumes from a cold, consistent state.
const MIGRATION_V5: &str = "
DROP TABLE IF EXISTS matches;
CREATE TABLE matches (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    map_seed INTEGER NOT NULL,
    p1_type TEXT NOT NULL,
    p2_type TEXT NOT NULL,
    result TEXT NOT NULL,
    duration_turns INTEGER NOT NULL,
    replay TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
DELETE FROM genomes;
DELETE FROM champions;
DELETE FROM elo_history;
DELETE FROM training_stats;
DELETE FROM trainer_state;
DELETE FROM events;
";

/// Live-match save slots (F2 save/resume). One row per abandoned match;
/// resuming deletes the row so a save is consumed exactly once.
const MIGRATION_V6: &str = "
CREATE TABLE IF NOT EXISTS saves (
    key TEXT PRIMARY KEY,
    opponent TEXT NOT NULL,
    game_json TEXT NOT NULL,
    replay_json TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
";

/// A stored live-match save: opponent label plus the serialized game.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct StoredSave {
    pub key: String,
    pub opponent: String,
    pub game_json: String,
    pub replay_json: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct StoredMatch {
    pub id: i64,
    pub map_seed: u64,
    pub p1_type: String,
    pub p2_type: String,
    pub result: String,
    pub duration_turns: i32,
    pub created_at: i64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct StoredGenome {
    pub id: i64,
    pub generation: u32,
    pub parent_id: Option<i64>,
    pub born_from: String,
    pub created_at: i64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct StoredChampion {
    pub id: i64,
    pub genome_id: i64,
    pub generation: u32,
    pub crowned_at: i64,
    pub dethroned_at: Option<i64>,
    pub gauntlet_record: Option<serde_json::Value>,
    /// §6.2 playstyle-era name (computed at promotion from the champion's
    /// behavioral fingerprint); older champions may have none.
    pub era: Option<String>,
}

impl StoredChampion {
    pub fn reigning(&self) -> bool {
        self.dethroned_at.is_none()
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct EloPoint {
    pub genome_id: i64,
    pub elo: f32,
    pub at: i64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct StoredEvent {
    pub id: i64,
    pub kind: String,
    pub payload: serde_json::Value,
    pub at: i64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct StoredGenomeFull {
    pub id: i64,
    pub generation: u32,
    pub parent_id: Option<i64>,
    pub born_from: String,
    pub weights: Vec<f32>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TrainingStat {
    pub generation: u32,
    pub matches_run: u64,
    pub pop_fitness_mean: f32,
    pub pop_fitness_best: f32,
    pub diversity: f32,
    pub at: i64,
}

/// A SQLite connection guarded for use from axum handlers.
pub struct Store {
    conn: Mutex<Connection>,
}

/// A single (pseudo-anonymous) player and their aggregate match count.
#[allow(dead_code)] // read by the L2 serving layer (P1)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerRecord {
    pub id: String,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    /// Matches played against the adaptive AI.
    pub matches: u64,
}

impl Store {
    /// Open (or create) the database at `path` and run migrations.
    pub fn open(path: &str) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // The store is a single mutex-guarded connection, but a concurrent
        // external process (e.g. a `sqlite3` inspection) can still lock the
        // file; wait instead of failing with SQLITE_BUSY.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&conn)?;
        Ok(Store {
            conn: Mutex::new(conn),
        })
    }

    /// Open an in-memory store (tests).
    #[allow(dead_code)]
    pub fn in_memory() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_in_memory()?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&conn)?;
        Ok(Store {
            conn: Mutex::new(conn),
        })
    }

    // --- Matches / replays -------------------------------------------------

    pub fn save_match(
        &self,
        map_seed: u64,
        p1_type: &str,
        p2_type: &str,
        result: &str,
        duration_turns: i32,
        replay_json: &str,
    ) -> Result<i64, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO matches (map_seed, p1_type, p2_type, result, duration_turns, replay, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                map_seed as i64,
                p1_type,
                p2_type,
                result,
                duration_turns,
                replay_json,
                unix_now(),
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_replay(&self, id: i64) -> Result<Option<String>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.query_row("SELECT replay FROM matches WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()
    }

    // --- Players (adaptive-opponent personalization) --------------------

    /// Record that `player_id` played one more match, creating the row on
    /// first sight. Best-effort aggregate (first/last seen + count); the
    /// per-player strategy profile (P1) lives in a separate table.
    pub fn note_player_match(&self, player_id: &str) -> Result<(), rusqlite::Error> {
        let now = unix_now();
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO players (id, first_seen_at, last_seen_at, matches)
             VALUES (?1, ?2, ?2, 1)
             ON CONFLICT(id) DO UPDATE SET
                last_seen_at = excluded.last_seen_at,
                matches = matches + 1",
            rusqlite::params![player_id, now],
        )?;
        Ok(())
    }

    /// Fetch a player row by id, if present.
    #[allow(dead_code)] // read by the L2 serving layer (P1)
    pub fn get_player(&self, player_id: &str) -> Result<Option<PlayerRecord>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.query_row(
            "SELECT id, first_seen_at, last_seen_at, matches FROM players WHERE id = ?1",
            [player_id],
            |r| {
                Ok(PlayerRecord {
                    id: r.get(0)?,
                    first_seen_at: r.get(1)?,
                    last_seen_at: r.get(2)?,
                    matches: r.get::<_, i64>(3)? as u64,
                })
            },
        )
        .optional()
    }

    /// Load a player's stored strategy profile JSON, if present.
    pub fn get_player_profile(&self, player_id: &str) -> Result<Option<String>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.query_row(
            "SELECT profile_json FROM player_profiles WHERE player_id = ?1",
            [player_id],
            |r| r.get(0),
        )
        .optional()
    }

    /// Persist a player's strategy profile JSON (upsert).
    pub fn save_player_profile(
        &self,
        player_id: &str,
        profile_json: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO player_profiles (player_id, profile_json, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(player_id) DO UPDATE SET
                profile_json = excluded.profile_json,
                updated_at = excluded.updated_at",
            rusqlite::params![player_id, profile_json, unix_now()],
        )?;
        Ok(())
    }

    // --- Save / resume -----------------------------------------------------

    /// Store a live match snapshot under `key` (F2). Overwrites any previous
    /// snapshot with the same key.
    pub fn save_game(
        &self,
        key: &str,
        opponent: &str,
        game_json: &str,
        replay_json: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO saves (key, opponent, game_json, replay_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(key) DO UPDATE SET opponent = excluded.opponent,
                                             game_json = excluded.game_json,
                                             replay_json = excluded.replay_json,
                                             created_at = excluded.created_at",
            rusqlite::params![key, opponent, game_json, replay_json, unix_now()],
        )?;
        Ok(())
    }

    /// The most recently saved live match, if any (F2 resume entry point).
    pub fn latest_save(&self) -> Result<Option<StoredSave>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT key, opponent, game_json, replay_json FROM saves ORDER BY created_at DESC, rowid DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map([], |row| {
            Ok(StoredSave {
                key: row.get(0)?,
                opponent: row.get(1)?,
                game_json: row.get(2)?,
                replay_json: row.get(3)?,
            })
        })?;
        rows.next().transpose()
    }

    /// Remove a consumed save (resumed matches are single-use).
    pub fn delete_save(&self, key: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute("DELETE FROM saves WHERE key = ?1", [key])?;
        Ok(())
    }

    pub fn list_matches(&self, limit: u32) -> Result<Vec<StoredMatch>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, map_seed, p1_type, p2_type, result, duration_turns, created_at
             FROM matches ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |row| {
            Ok(StoredMatch {
                id: row.get(0)?,
                map_seed: row.get::<_, i64>(1)? as u64,
                p1_type: row.get(2)?,
                p2_type: row.get(3)?,
                result: row.get(4)?,
                duration_turns: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    }

    /// Fetch matches **and** their replay JSON in a single query. The ghost
    /// pool needs both; looping [`Store::get_replay`] per row would do an
    /// N+1 query against the mutex.
    pub fn list_matches_with_replay(
        &self,
        limit: u32,
    ) -> Result<Vec<(StoredMatch, String)>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, map_seed, p1_type, p2_type, result, duration_turns, created_at, replay
             FROM matches ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |row| {
            Ok((
                StoredMatch {
                    id: row.get(0)?,
                    map_seed: row.get::<_, i64>(1)? as u64,
                    p1_type: row.get(2)?,
                    p2_type: row.get(3)?,
                    result: row.get(4)?,
                    duration_turns: row.get(5)?,
                    created_at: row.get(6)?,
                },
                row.get::<_, String>(7)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    }

    /// Fetch matches (with replay JSON) with `id > since`, ascending — the
    /// incremental ghost-pool refresh: new human matches played since the last
    /// load become training ghosts without a server restart.
    pub fn list_matches_with_replay_since(
        &self,
        since: i64,
        limit: u32,
    ) -> Result<Vec<(StoredMatch, String)>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, map_seed, p1_type, p2_type, result, duration_turns, created_at, replay
             FROM matches WHERE id > ?1 ORDER BY id ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map([since, limit as i64], |row| {
            Ok((
                StoredMatch {
                    id: row.get(0)?,
                    map_seed: row.get::<_, i64>(1)? as u64,
                    p1_type: row.get(2)?,
                    p2_type: row.get(3)?,
                    result: row.get(4)?,
                    duration_turns: row.get(5)?,
                    created_at: row.get(6)?,
                },
                row.get::<_, String>(7)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    }

    // --- Genomes / lineage -------------------------------------------------

    /// Persist a genome's weights; returns its id.
    // Used by the trainer (M6).
    #[allow(dead_code)]
    pub fn save_genome(
        &self,
        generation: u32,
        parent_id: Option<i64>,
        born_from: &str,
        weights: &[f32],
    ) -> Result<i64, rusqlite::Error> {
        let weights_json = serde_json::to_string(weights).expect("weights serialize");
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO genomes (generation, parent_id, weights, born_from, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![generation, parent_id, weights_json, born_from, unix_now()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    // Used by the trainer (M6).
    #[allow(dead_code)]
    pub fn get_genome(&self, id: i64) -> Result<Option<StoredGenome>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.query_row(
            "SELECT id, generation, parent_id, born_from, created_at FROM genomes WHERE id = ?1",
            [id],
            |row| {
                Ok(StoredGenome {
                    id: row.get(0)?,
                    generation: row.get(1)?,
                    parent_id: row.get(2)?,
                    born_from: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        )
        .optional()
    }

    pub fn get_genome_weights(&self, id: i64) -> Result<Option<Vec<f32>>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let json: Option<String> = conn
            .query_row("SELECT weights FROM genomes WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        json.map(|s| {
            serde_json::from_str(&s).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })
        })
        .transpose()
    }

    /// Ancestor chain for a genome, most-recent first (includes `id` itself).
    pub fn lineage_chain(&self, id: i64) -> Result<Vec<StoredGenome>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut out = Vec::new();
        let mut cur = Some(id);
        while let Some(c) = cur {
            let rec: Option<StoredGenome> = conn
                .query_row(
                    "SELECT id, generation, parent_id, born_from, created_at FROM genomes WHERE id = ?1",
                    [c],
                    |row| {
                        Ok(StoredGenome {
                            id: row.get(0)?,
                            generation: row.get(1)?,
                            parent_id: row.get(2)?,
                            born_from: row.get(3)?,
                            created_at: row.get(4)?,
                        })
                    },
                )
                .optional()?;
            match rec {
                Some(r) => {
                    cur = r.parent_id;
                    out.push(r);
                }
                None => cur = None,
            }
        }
        Ok(out)
    }

    pub fn count_rows(&self, table: &str) -> Result<i64, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        // Table names come from a fixed internal whitelist.
        let sql = match table {
            "matches" | "genomes" | "champions" | "elo_history" | "events" => {
                format!("SELECT COUNT(*) FROM {table}")
            }
            _ => return Ok(0),
        };
        conn.query_row(&sql, [], |row| row.get(0))
    }

    // --- Champions / museum ------------------------------------------------

    /// Crown a new champion, dethroning the current reigning one. Returns the
    /// new champion row id.
    // Used by the trainer (M6).
    #[allow(dead_code)]
    pub fn crown_champion(
        &self,
        genome_id: i64,
        generation: u32,
        gauntlet_record: Option<serde_json::Value>,
        era: Option<&str>,
    ) -> Result<i64, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let now = unix_now();
        conn.execute(
            "UPDATE champions SET dethroned_at = ?1 WHERE dethroned_at IS NULL",
            [now],
        )?;
        let record_json = gauntlet_record.map(|v| v.to_string());
        conn.execute(
            "INSERT INTO champions (genome_id, generation, crowned_at, dethroned_at, gauntlet_record, era)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
            rusqlite::params![genome_id, generation, now, record_json, era],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_reigning_champion(&self) -> Result<Option<StoredChampion>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        row_champion(
            &conn,
            "SELECT id, genome_id, generation, crowned_at, dethroned_at, gauntlet_record, era
             FROM champions WHERE dethroned_at IS NULL ORDER BY crowned_at DESC LIMIT 1",
            [],
        )
    }

    pub fn list_champions(&self) -> Result<Vec<StoredChampion>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, genome_id, generation, crowned_at, dethroned_at, gauntlet_record, era
             FROM champions ORDER BY crowned_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(StoredChampion {
                id: row.get(0)?,
                genome_id: row.get(1)?,
                generation: row.get(2)?,
                crowned_at: row.get(3)?,
                dethroned_at: row.get(4)?,
                gauntlet_record: row
                    .get::<_, Option<String>>(5)?
                    .map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)),
                era: row.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    }

    // --- Elo ---------------------------------------------------------------

    // Used by the trainer (M6).
    #[allow(dead_code)]
    pub fn record_elo(&self, genome_id: i64, elo: f32) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO elo_history (genome_id, elo, at) VALUES (?1, ?2, ?3)",
            rusqlite::params![genome_id, elo, unix_now()],
        )?;
        Ok(())
    }

    pub fn elo_history(&self, genome_id: i64) -> Result<Vec<EloPoint>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT genome_id, elo, at FROM elo_history WHERE genome_id = ?1 ORDER BY at ASC",
        )?;
        let rows = stmt.query_map([genome_id], |row| {
            Ok(EloPoint {
                genome_id: row.get(0)?,
                elo: row.get(1)?,
                at: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    }

    // --- Events (away report) ----------------------------------------------

    // Used by the trainer (M6).
    #[allow(dead_code)]
    pub fn record_event(
        &self,
        kind: &str,
        payload: serde_json::Value,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO events (kind, payload, at) VALUES (?1, ?2, ?3)",
            rusqlite::params![kind, payload.to_string(), unix_now()],
        )?;
        Ok(())
    }

    pub fn recent_events(&self, limit: u32) -> Result<Vec<StoredEvent>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt =
            conn.prepare("SELECT id, kind, payload, at FROM events ORDER BY id DESC LIMIT ?1")?;
        let rows = stmt.query_map([limit], |row| {
            Ok(StoredEvent {
                id: row.get(0)?,
                kind: row.get(1)?,
                payload: serde_json::from_str::<serde_json::Value>(&row.get::<_, String>(2)?)
                    .unwrap_or(serde_json::Value::Null),
                at: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    }

    // --- Trainer state / checkpoints ---------------------------------------

    pub fn set_state(&self, key: &str, value: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO trainer_state (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    pub fn get_state(&self, key: &str) -> Result<Option<String>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.query_row(
            "SELECT value FROM trainer_state WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()
    }

    /// Remove model checkpoints that cannot be interpreted under a new genome
    /// schema while preserving human match/replay data. The trainer calls this
    /// before creating generation-zero rows so stale ids cannot be mixed with
    /// a fresh population.
    pub fn reset_model_checkpoints(&self) -> Result<(), rusqlite::Error> {
        let mut conn = self.conn.lock().expect("store mutex poisoned");
        let tx = conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM champions;
             DELETE FROM elo_history;
             DELETE FROM genomes;
             DELETE FROM training_stats;
             DELETE FROM trainer_state WHERE key = 'genome_schema_version';",
        )?;
        tx.commit()
    }

    /// Atomically persist one generation's genomes and return their ids (in
    /// the same order as `rows`). `rows` = (parent_id, born_from, weights).
    pub fn save_generation(
        &self,
        generation: u32,
        rows: &[(Option<i64>, &str, Vec<f32>)],
    ) -> Result<Vec<i64>, rusqlite::Error> {
        let mut conn = self.conn.lock().expect("store mutex poisoned");
        let tx = conn.transaction()?;
        let mut ids = Vec::with_capacity(rows.len());
        {
            let mut stmt = tx.prepare(
                "INSERT INTO genomes (generation, parent_id, weights, born_from, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (parent, born, weights) in rows {
                let weights_json = serde_json::to_string(weights).expect("weights serialize");
                stmt.execute(rusqlite::params![
                    generation,
                    parent,
                    weights_json,
                    born,
                    unix_now(),
                ])?;
                // Capture each inserted row's id directly instead of re-reading
                // by generation: a crash-restart within a generation would
                // otherwise re-insert rows and make the by-generation re-read
                // return duplicates, corrupting the parent/winner association.
                ids.push(tx.last_insert_rowid());
            }
        }
        tx.commit()?;
        Ok(ids)
    }

    pub fn latest_generation(&self) -> Result<Option<u32>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let max: Option<Option<u32>> = conn
            .query_row("SELECT MAX(generation) FROM genomes", [], |row| row.get(0))
            .optional()?;
        Ok(max.flatten())
    }

    pub fn genomes_of_generation(
        &self,
        generation: u32,
    ) -> Result<Vec<StoredGenomeFull>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, generation, parent_id, born_from, weights
             FROM genomes WHERE generation = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([generation], |row| {
            let weights_json: String = row.get(4)?;
            Ok(StoredGenomeFull {
                id: row.get(0)?,
                generation: row.get(1)?,
                parent_id: row.get(2)?,
                born_from: row.get(3)?,
                weights: serde_json::from_str(&weights_json).unwrap_or_default(),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    }

    pub fn save_training_stats(
        &self,
        generation: u32,
        matches_run: u64,
        pop_fitness_mean: f32,
        pop_fitness_best: f32,
        diversity: f32,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO training_stats
             (generation, matches_run, pop_fitness_mean, pop_fitness_best, diversity, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                generation,
                matches_run as i64,
                pop_fitness_mean,
                pop_fitness_best,
                diversity,
                unix_now(),
            ],
        )?;
        Ok(())
    }

    pub fn list_training_stats(&self, limit: u32) -> Result<Vec<TrainingStat>, rusqlite::Error> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT generation, matches_run, pop_fitness_mean, pop_fitness_best, diversity, at
             FROM training_stats ORDER BY generation ASC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |row| {
            Ok(TrainingStat {
                generation: row.get(0)?,
                matches_run: row.get::<_, i64>(1)? as u64,
                pop_fitness_mean: row.get(2)?,
                pop_fitness_best: row.get(3)?,
                diversity: row.get(4)?,
                at: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    }
}

/// Canonical label for the `matches.result` column: `"P0"`, `"P1"`, or
/// `"draw"`. Older rows may hold Debug-formatted values (e.g. `"Some(0)"`)
/// from before this format; consumers should only treat the exact labels as
/// decisive. `"abandoned"` (a disconnected mid-match replay) is written by
/// the WS loop, not produced here.
pub fn result_label(winner: Option<crucible_sim::Player>) -> String {
    match winner {
        Some(crucible_sim::Player::P0) => "P0".to_string(),
        Some(crucible_sim::Player::P1) => "P1".to_string(),
        None => "draw".to_string(),
    }
}

fn row_champion(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Option<StoredChampion>, rusqlite::Error> {
    conn.query_row(sql, params, |row| {
        Ok(StoredChampion {
            id: row.get(0)?,
            genome_id: row.get(1)?,
            generation: row.get(2)?,
            crowned_at: row.get(3)?,
            dethroned_at: row.get(4)?,
            gauntlet_record: row
                .get::<_, Option<String>>(5)?
                .map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)),
            era: row.get(6)?,
        })
    })
    .optional()
}

/// §6.2: each champion carries a playstyle-era name computed from its
/// behavioral fingerprint at promotion time (a nullable column so older DBs
/// migrate cleanly and eras stay backward compatible).
const MIGRATION_V4: &str = "
ALTER TABLE champions ADD COLUMN era TEXT;";

const MIGRATION_V7: &str = "
CREATE TABLE IF NOT EXISTS players (
    id TEXT PRIMARY KEY,
    first_seen_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    matches INTEGER NOT NULL DEFAULT 0
);";

// P1: per-player strategy profile (adaptive-opponent learning). The whole
// model is one bounded JSON blob so it stays trivially serializable and
// deterministic; `updated_at` is bookkeeping only.
const MIGRATION_V8: &str = "
CREATE TABLE IF NOT EXISTS player_profiles (
    player_id TEXT PRIMARY KEY REFERENCES players(id),
    profile_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);";

fn migrate(conn: &Connection) -> Result<(), rusqlite::Error> {
    let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(&format!("BEGIN; {MIGRATION_V1} COMMIT;"))?;
    }
    if version < 2 {
        conn.execute_batch(&format!("BEGIN; {MIGRATION_V2} COMMIT;"))?;
    }
    if version < 3 {
        conn.execute_batch(&format!("BEGIN; {MIGRATION_V3} COMMIT;"))?;
    }
    if version < 4 {
        conn.execute_batch(&format!("BEGIN; {MIGRATION_V4} COMMIT;"))?;
    }
    if version < 5 {
        conn.execute_batch(&format!("BEGIN; {MIGRATION_V5} COMMIT;"))?;
    }
    if version < 6 {
        conn.execute_batch(&format!("BEGIN; {MIGRATION_V6} COMMIT;"))?;
    }
    if version < 7 {
        conn.execute_batch(&format!("BEGIN; {MIGRATION_V7} COMMIT;"))?;
    }
    if version < 8 {
        conn.execute_batch(&format!("BEGIN; {MIGRATION_V8} COMMIT;"))?;
    }
    if version < SCHEMA_VERSION {
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    Ok(())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_resume_round_trip_is_single_use() {
        let store = Store::in_memory().unwrap();
        assert!(store.latest_save().unwrap().is_none());
        store
            .save_game("save:1", "hard", "{\"turn\":7}", "{\"seed\":1}")
            .unwrap();
        let save = store.latest_save().unwrap().expect("save exists");
        assert_eq!(save.key, "save:1");
        assert_eq!(save.opponent, "hard");
        assert_eq!(save.game_json, "{\"turn\":7}");
        assert_eq!(save.replay_json, "{\"seed\":1}");
        // A newer save wins; resuming consumes it (single-use).
        store
            .save_game("save:2", "medium", "{\"turn\":3}", "{}")
            .unwrap();
        assert_eq!(store.latest_save().unwrap().unwrap().key, "save:2");
        store.delete_save("save:2").unwrap();
        assert_eq!(store.latest_save().unwrap().unwrap().key, "save:1");
    }

    #[test]
    fn player_match_counter_is_upserted_per_id() {
        let store = Store::in_memory().unwrap();
        // Unknown id reads as absent.
        assert!(store.get_player("u1").unwrap().is_none());

        store.note_player_match("u1").unwrap();
        store.note_player_match("u1").unwrap();
        let p = store.get_player("u1").unwrap().expect("player row");
        assert_eq!(p.id, "u1");
        assert_eq!(p.matches, 2);
        assert!(p.first_seen_at > 0);
        assert!(p.last_seen_at >= p.first_seen_at);

        // Players are independent.
        store.note_player_match("u2").unwrap();
        let p2 = store.get_player("u2").unwrap().expect("second player");
        assert_eq!(p2.matches, 1);
        assert_eq!(store.get_player("u1").unwrap().unwrap().matches, 2);
    }

    #[test]
    fn player_profile_round_trip() {
        let store = Store::in_memory().unwrap();
        assert!(store.get_player_profile("u1").unwrap().is_none());
        // player_profiles has an FK to players; create the parent first.
        store.note_player_match("u1").unwrap();
        store
            .save_player_profile("u1", r#"{"tempo":0.5,"recency_weight":0.7}"#)
            .unwrap();
        let got = store
            .get_player_profile("u1")
            .unwrap()
            .expect("profile row");
        assert!(got.contains("\"tempo\":0.5"));
        // Upsert replaces.
        store.save_player_profile("u1", r#"{"tempo":0.9}"#).unwrap();
        let updated = store.get_player_profile("u1").unwrap().unwrap();
        assert!(updated.contains("\"tempo\":0.9"));
    }

    #[test]
    fn incremental_match_fetch_returns_only_newer_rows() {
        let store = Store::in_memory().unwrap();
        for seed in 1..=5u64 {
            store
                .save_match(seed, "human", "bot:hard", "P0", 100, "{}")
                .unwrap();
        }

        // Full fetch is newest-first (id DESC).
        let all = store.list_matches_with_replay(10).unwrap();
        assert_eq!(all.len(), 5);
        assert_eq!(all[0].0.id, 5);

        // Incremental fetch after the first three rows: exactly the rest,
        // ascending (id 4, 5).
        let since = store.list_matches(10).unwrap()[2].id; // id of the 3rd newest
        let inc = store.list_matches_with_replay_since(since, 10).unwrap();
        assert_eq!(inc.len(), 2);
        assert_eq!(inc[0].0.id, since + 1);
        assert_eq!(inc[1].0.id, since + 2);

        // Limit applies.
        let limited = store.list_matches_with_replay_since(0, 2).unwrap();
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].0.id, 1);
    }

    #[test]
    fn save_and_load_replay_round_trips() {
        let store = Store::in_memory().unwrap();
        let id = store
            .save_match(42, "human", "bot:hard", "P0", 1234, r#"{"version":1}"#)
            .unwrap();
        assert_eq!(
            store.get_replay(id).unwrap().as_deref(),
            Some(r#"{"version":1}"#)
        );
        let list = store.list_matches(10).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].map_seed, 42);
    }

    #[test]
    fn champion_crowning_dethrones_previous() {
        let store = Store::in_memory().unwrap();
        let a = store
            .save_genome(0, None, "init", &[0.1, 0.2, 0.3])
            .unwrap();
        let b = store
            .save_genome(1, Some(a), "mutant", &[0.4, 0.5])
            .unwrap();

        store.crown_champion(a, 0, None, None).unwrap();
        assert_eq!(store.get_reigning_champion().unwrap().unwrap().genome_id, a);

        store.crown_champion(b, 1, None, None).unwrap();
        let champions = store.list_champions().unwrap();
        assert_eq!(champions.len(), 2);
        assert!(champions.iter().all(|c| c.reigning() == (c.genome_id == b)));

        // Museum lists the dethroned champion first.
        assert_eq!(champions[0].genome_id, a);
        assert_eq!(champions[1].genome_id, b);
        assert!(champions[0].dethroned_at.is_some());
        assert!(champions[1].dethroned_at.is_none());
    }

    #[test]
    fn generation_checkpoint_and_training_stats() {
        let store = Store::in_memory().unwrap();

        // First generation: two root genomes.
        let ids = store
            .save_generation(
                0,
                &[
                    (None, "init", vec![0.1, 0.2]),
                    (None, "init", vec![0.3, 0.4]),
                ],
            )
            .unwrap();
        assert_eq!(ids.len(), 2);

        // Second generation: first genome descends from id[0].
        store
            .save_generation(1, &[(Some(ids[0]), "mutant", vec![0.5, 0.6])])
            .unwrap();

        assert_eq!(store.latest_generation().unwrap(), Some(1));
        let gen0 = store.genomes_of_generation(0).unwrap();
        assert_eq!(gen0.len(), 2);
        assert_eq!(gen0[0].weights, vec![0.1, 0.2]);
        let gen1 = store.genomes_of_generation(1).unwrap();
        assert_eq!(gen1[0].parent_id, Some(ids[0]));

        store.save_training_stats(1, 12, 0.5, 0.9, 0.33).unwrap();
        let stats = store.list_training_stats(10).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].generation, 1);
        assert_eq!(stats[0].matches_run, 12);

        store.set_state("master_seed", "12345").unwrap();
        assert_eq!(
            store.get_state("master_seed").unwrap().as_deref(),
            Some("12345")
        );
    }

    #[test]
    fn genome_weights_round_trip_and_elo_events() {
        let store = Store::in_memory().unwrap();
        let w = vec![0.25f32, -1.0, 7.5];
        let id = store.save_genome(2, Some(1), "mutant", &w).unwrap();

        assert_eq!(store.get_genome_weights(id).unwrap().unwrap(), w);
        let meta = store.get_genome(id).unwrap().unwrap();
        assert_eq!(meta.generation, 2);
        assert_eq!(meta.parent_id, Some(1));

        store.record_elo(id, 1500.0).unwrap();
        store.record_elo(id, 1524.0).unwrap();
        let history = store.elo_history(id).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].elo, 1500.0);

        store
            .record_event("promotion", serde_json::json!({"genome": id}))
            .unwrap();
        let events = store.recent_events(10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "promotion");
    }

    /// Full pipeline: run a match vs the hard bot, store its replay, reload it
    /// from SQLite, and re-run it byte-identically.
    #[test]
    fn stored_match_replays_byte_identically() {
        use crucible_ai::{hard, Bot};
        use crucible_sim::{
            serialize, Command, Game, GameConfig, Map, Player, Replay, ReplayResult,
        };

        let store = Store::in_memory().unwrap();
        let cfg = GameConfig {
            timeout_turns: 60, // short match for the test
            ..GameConfig::default()
        };
        let seed = 99u64;

        let mut g = Game::new(Map::generate(seed), cfg.clone());
        let mut replay = Replay::new(seed, cfg);
        let mut bot = hard();
        while !g.is_over() {
            // Alternate-turn driver: P0 passes, P1 plays the hard bot.
            if g.active == Player::P0 {
                let cmds = vec![Command::EndTurn { player: Player::P0 }];
                for c in &cmds {
                    replay.record(g.turn, Player::P0, c.clone());
                }
                g.apply_commands(Player::P0, &cmds);
            } else {
                let cmds = bot.decide(&g, Player::P1);
                for c in &cmds {
                    replay.record(g.turn, Player::P1, c.clone());
                }
                g.apply_commands(Player::P1, &cmds);
            }
        }
        replay.result = Some(ReplayResult {
            winner: g.winner,
            reason: g.win_reason,
            duration_turns: g.turn,
            duration_rounds: g.round,
        });

        let id = store
            .save_match(
                seed,
                "human",
                "bot:hard",
                &format!("{:?}", g.winner),
                g.turn,
                &replay.to_json(),
            )
            .unwrap();

        let loaded = Replay::from_json(&store.get_replay(id).unwrap().unwrap()).unwrap();
        let repro = serialize::replay_to_game(&loaded);
        assert_eq!(
            serialize::snapshot_bytes(&g),
            serialize::snapshot_bytes(&repro),
            "stored replay did not reproduce the match byte-identically"
        );
    }
}
