//! SQLite persistence. The only place that talks to the database.
//!
//! Schema is versioned via `PRAGMA user_version`; migrations run at boot.
//! Replays are stored as their JSON input log so they stay re-runnable forever.
//! Genomes are stored as JSON weight arrays; champions carry their gauntlet
//! record for full reproducibility.

use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: i32 = 5;

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
        Ok(json.map(|s| serde_json::from_str(&s).expect("stored weights are valid JSON")))
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

    /// Atomically persist one generation's genomes and return their ids (in
    /// the same order as `rows`). `rows` = (parent_id, born_from, weights).
    pub fn save_generation(
        &self,
        generation: u32,
        rows: &[(Option<i64>, &str, Vec<f32>)],
    ) -> Result<Vec<i64>, rusqlite::Error> {
        let mut conn = self.conn.lock().expect("store mutex poisoned");
        let tx = conn.transaction()?;
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
            }
        }
        // Re-read ids in insertion order.
        let ids: Vec<i64> = {
            let mut stmt =
                tx.prepare("SELECT id FROM genomes WHERE generation = ?1 ORDER BY id ASC")?;
            let rows = stmt.query_map([generation], |r| r.get(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
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
