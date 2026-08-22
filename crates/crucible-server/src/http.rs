//! Dashboard REST handlers: champion, Elo history, lineage, museum, status,
//! and on-demand change reports. All read-only over the store.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use crucible_ai::{run_match_with_replay, GenomeBot};
use crucible_sim::GameConfig;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::store::{EloPoint, Store, StoredChampion, StoredGenome, TrainingStat};
use crate::AppState;

fn err(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// Run a blocking store operation on the blocking pool so the async runtime
/// never stalls behind the SQLite mutex (the trainer's generation checkpoint
/// can hold it for a while).
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, (StatusCode, String)> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| err(format!("handler task failed: {e}")))
}

fn champion_payload(store: &Store, c: &StoredChampion) -> Value {
    let elo = store
        .elo_history(c.genome_id)
        .ok()
        .and_then(|h| h.last().map(|p| p.elo));
    json!({
        "id": c.id,
        "genome_id": c.genome_id,
        "generation": c.generation,
        "crowned_at": c.crowned_at,
        "dethroned_at": c.dethroned_at,
        "reigning": c.reigning(),
        "gauntlet_record": c.gauntlet_record,
        "era": c.era,
        "elo": elo,
    })
}

pub async fn champion(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let store = state.store.clone();
    let res: Result<Json<Value>, rusqlite::Error> = blocking(move || {
        Ok(match store.get_reigning_champion()? {
            Some(c) => {
                let lineage = store.lineage_chain(c.genome_id).unwrap_or_default();
                Json(json!({
                    "champion": champion_payload(&store, &c),
                    "lineage": lineage,
                }))
            }
            None => Json(json!({ "champion": null })),
        })
    })
    .await?;
    res.map_err(err)
}

#[derive(Deserialize)]
pub struct EloQuery {
    genome_id: Option<i64>,
}

pub async fn elo_history(
    State(state): State<AppState>,
    Query(q): Query<EloQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let store = state.store.clone();
    let genome_id = q.genome_id;
    let res: Result<Option<(i64, Vec<EloPoint>)>, rusqlite::Error> = blocking(move || {
        let genome_id = match genome_id.or_else(|| {
            store
                .get_reigning_champion()
                .ok()
                .flatten()
                .map(|c| c.genome_id)
        }) {
            Some(id) => id,
            None => return Ok(None),
        };
        Ok(Some((genome_id, store.elo_history(genome_id)?)))
    })
    .await?;
    match res {
        Ok(Some((genome_id, points))) => {
            Ok(Json(json!({ "genome_id": genome_id, "points": points })))
        }
        Ok(None) => Ok(Json(json!({ "points": [] }))),
        Err(e) => Err(err(e)),
    }
}

pub async fn lineage(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let store = state.store.clone();
    let res: Result<Vec<StoredGenome>, rusqlite::Error> =
        blocking(move || store.lineage_chain(id)).await?;
    match res {
        Ok(chain) if chain.is_empty() => Err((StatusCode::NOT_FOUND, "no such genome".into())),
        Ok(chain) => Ok(Json(json!({ "genome_id": id, "lineage": chain }))),
        Err(e) => Err(err(e)),
    }
}

pub async fn museum(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let store = state.store.clone();
    let res: Result<Json<Value>, rusqlite::Error> = blocking(move || {
        let champions = store.list_champions()?;
        let list: Vec<Value> = champions
            .iter()
            .map(|c| champion_payload(&store, c))
            .collect();
        Ok(Json(json!({ "champions": list })))
    })
    .await?;
    res.map_err(err)
}

pub async fn status(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let store = state.store.clone();
    let trainer = state.trainer.clone();
    let started = state.started_at;
    let res: Result<Json<Value>, rusqlite::Error> = blocking(move || {
        let count = |t: &str| store.count_rows(t).unwrap_or(0);
        Ok(Json(json!({
            "ok": true,
            "uptime_secs": started.elapsed().as_secs(),
            "counts": {
                "matches": count("matches"),
                "genomes": count("genomes"),
                "champions": count("champions"),
                "events": count("events"),
            },
            "recent_events": store.recent_events(50).unwrap_or_default(),
            "trainer": trainer.snapshot(),
        })))
    })
    .await?;
    res.map_err(err)
}

pub async fn training_stats(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let store = state.store.clone();
    let res: Result<Vec<TrainingStat>, rusqlite::Error> =
        blocking(move || store.list_training_stats(10_000)).await?;
    res.map(|stats| Json(json!({ "stats": stats })))
        .map_err(err)
}

/// On-demand change report between two stored genomes (dev/diagnostic; the
/// trainer will persist these at promotion time in M6).
pub async fn report(
    State(state): State<AppState>,
    Path((old, new)): Path<(i64, i64)>,
) -> impl IntoResponse {
    let _permit = match state.diagnostics.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                "another diagnostic simulation is already running",
            )
                .into_response()
        }
    };
    let store = &state.store;
    let old_w = match store.get_genome_weights(old) {
        Ok(Some(w)) => w,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such old genome").into_response(),
        Err(e) => return err(e).into_response(),
    };
    let new_w = match store.get_genome_weights(new) {
        Ok(Some(w)) => w,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such new genome").into_response(),
        Err(e) => return err(e).into_response(),
    };

    // Small evaluation set so the endpoint stays snappy.
    let seeds: Vec<u64> = (1..=20).collect();
    let cfg = crucible_sim::GameConfig {
        timeout_turns: 60,
        ..crucible_sim::GameConfig::default()
    };
    match tokio::task::spawn_blocking(move || {
        crucible_evo::change_report(&old_w, &new_w, &seeds, &cfg)
    })
    .await
    {
        Ok(report) => Json(json!({ "report": report })).into_response(),
        Err(e) => err(format!("diagnostic task failed: {e}")).into_response(),
    }
}

#[derive(Deserialize)]
pub struct AutoBattleQuery {
    seed: Option<u64>,
}

/// Run one champion-vs-ancestor (or any two stored genomes) auto-battle and
/// return the result plus a full, re-runnable replay.
pub async fn autobattle(
    State(state): State<AppState>,
    Path((a, b)): Path<(i64, i64)>,
    Query(q): Query<AutoBattleQuery>,
) -> impl IntoResponse {
    let _permit = match state.diagnostics.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                "another diagnostic simulation is already running",
            )
                .into_response()
        }
    };
    let store = &state.store;
    let wa = match store.get_genome_weights(a) {
        Ok(Some(w)) => w,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such genome a").into_response(),
        Err(e) => return err(e).into_response(),
    };
    let wb = match store.get_genome_weights(b) {
        Ok(Some(w)) => w,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such genome b").into_response(),
        Err(e) => return err(e).into_response(),
    };

    let seed = q.seed.unwrap_or(1);
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || {
        // Diagnostics must have a predictable CPU ceiling; live matches retain
        // the normal configurable full-length timeout in the websocket path.
        let cfg = GameConfig {
            timeout_turns: 60,
            ..GameConfig::default()
        };
        let mut ba = GenomeBot::new(wa);
        let mut bb = GenomeBot::new(wb);
        let (outcome, replay) = run_match_with_replay(seed, &cfg, &mut ba, &mut bb);
        let replay_json = replay.to_json();
        let replay_id = store
            .save_match(
                seed,
                &format!("genome:{a}"),
                &format!("genome:{b}"),
                &crate::store::result_label(outcome.outcome.winner),
                outcome.outcome.duration_turns,
                &replay_json,
            )
            .ok();
        (outcome, replay_json, replay_id)
    })
    .await
    {
        Ok((outcome, replay_json, replay_id)) => Json(json!({
            "seed": seed,
            "winner": outcome.outcome.winner.map(|p| p.index() as u8),
            "reason": outcome.outcome.reason,
            "duration_turns": outcome.outcome.duration_turns,
            "duration_rounds": outcome.outcome.duration_rounds,
            "replay_id": replay_id,
            "replay": serde_json::from_str::<Value>(&replay_json).unwrap_or(Value::Null),
        }))
        .into_response(),
        Err(e) => err(format!("diagnostic task failed: {e}")).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use axum::{body::Body, routing::get, Router};
    use std::sync::Arc;
    use tower::ServiceExt;
    fn seeded_state() -> AppState {
        let store = Arc::new(Store::in_memory().unwrap());
        let a = store.save_genome(0, None, "init", &[0.1, 0.2]).unwrap();
        let b = store
            .save_genome(1, Some(a), "mutant", &[0.3, 0.4])
            .unwrap();
        store.crown_champion(a, 0, None, None).unwrap();
        store.crown_champion(b, 1, None, None).unwrap();
        store.record_elo(a, 1500.0).unwrap();
        store.record_elo(b, 1550.0).unwrap();
        AppState {
            store,
            trainer: Arc::new(crate::trainer::TrainerShared::default()),
            diagnostics: Arc::new(tokio::sync::Semaphore::new(1)),
            live_matches: Arc::new(tokio::sync::Semaphore::new(8)),
            started_at: std::time::Instant::now(),
        }
    }

    fn router(state: AppState) -> Router {
        Router::new()
            .route("/api/champion", get(champion))
            .route("/api/elo-history", get(elo_history))
            .route("/api/lineage/{id}", get(lineage))
            .route("/api/museum", get(museum))
            .route("/api/status", get(status))
            .with_state(state)
    }

    async fn body_json(response: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn museum_lists_dethroned_champions() {
        let app = router(seeded_state());
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/museum")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        let champions = json["champions"].as_array().unwrap();
        assert_eq!(champions.len(), 2);
        assert_eq!(champions[0]["reigning"], false);
        assert_eq!(champions[1]["reigning"], true);
        assert!(champions[1]["elo"].as_f64().unwrap() > 0.0);
    }

    #[tokio::test]
    async fn champion_and_lineage_and_elo_endpoints() {
        let app = router(seeded_state());

        // Champion: the second-crowned genome reigns.
        let champion_resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/champion")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let champion_json = body_json(champion_resp).await;
        let champion = &champion_json["champion"];
        let b_id = champion["genome_id"].as_i64().unwrap();
        assert_eq!(champion["generation"], 1);
        assert_eq!(champion["elo"], 1550.0);

        // Lineage: b -> a -> (root).
        let lineage_resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/api/lineage/{b_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let lineage_json = body_json(lineage_resp).await;
        assert_eq!(lineage_json["lineage"].as_array().unwrap().len(), 2);

        // Elo history for the reigning champion.
        let elo_resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/api/elo-history?genome_id={b_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let elo_json = body_json(elo_resp).await;
        assert_eq!(elo_json["points"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn autobattle_matches_headless_result() {
        let store = Arc::new(Store::in_memory().unwrap());
        let a_w = vec![0.0f32; crucible_ai::GENOME_LEN]; // no-op
        let b_w = crucible_ai::init(&mut crucible_sim::Rng::from_seed(1));
        let a = store.save_genome(0, None, "init", &a_w).unwrap();
        let b = store.save_genome(0, None, "init", &b_w).unwrap();
        let state = AppState {
            store,
            trainer: Arc::new(crate::trainer::TrainerShared::default()),
            diagnostics: Arc::new(tokio::sync::Semaphore::new(1)),
            live_matches: Arc::new(tokio::sync::Semaphore::new(8)),
            started_at: std::time::Instant::now(),
        };

        let app = Router::new()
            .route("/api/autobattle/{a}/{b}", axum::routing::post(autobattle))
            .with_state(state);
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/autobattle/{a}/{b}?seed=5"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;

        // Direct headless run with the same seed must agree on the winner.
        let (outcome, _replay) = run_match_with_replay(
            5,
            &GameConfig {
                timeout_turns: 60,
                ..GameConfig::default()
            },
            &mut GenomeBot::new(a_w),
            &mut GenomeBot::new(b_w),
        );
        let direct: Value = outcome
            .outcome
            .winner
            .map(|p| json!(p.index() as u8))
            .unwrap_or(Value::Null);
        assert_eq!(json["winner"], direct);
        assert!(json["replay"].is_object());
    }
}
