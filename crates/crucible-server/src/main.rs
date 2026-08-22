//! CRUCIBLE server.
//!
//! Serves the static client, REST endpoints for dashboard data/replays, and a
//! WebSocket live-match endpoint. The trainer and dashboard land in M4–M6.

mod http;
mod lifecycle;
mod store;
mod trainer;
mod ws;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use tower_http::services::ServeDir;

use store::Store;
use trainer::{TrainerConfig, TrainerShared};

/// Maximum simultaneous live matches (each runs a full 10 Hz sim).
const MAX_LIVE_MATCHES: usize = 8;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) store: Arc<Store>,
    pub(crate) trainer: Arc<TrainerShared>,
    /// Serializes expensive diagnostic simulations so they cannot starve live
    /// match handling when this server is exposed beyond localhost.
    pub(crate) diagnostics: Arc<tokio::sync::Semaphore>,
    /// Caps concurrent live matches for the same reason.
    pub(crate) live_matches: Arc<tokio::sync::Semaphore>,
    pub(crate) started_at: std::time::Instant,
}

/// Start the 24/7 trainer if `CRUCIBLE_TRAINER=1`. Optional
/// `CRUCIBLE_TRAINER_GENERATIONS=N` runs a bounded fast-forward; `SMALL=1` uses
/// a small, fast configuration.
fn start_trainer(store: Arc<Store>, shared: Arc<TrainerShared>) {
    let enabled = std::env::var("CRUCIBLE_TRAINER")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("on"))
        .unwrap_or(false);
    if !enabled {
        return;
    }
    let generations: Option<usize> = std::env::var("CRUCIBLE_TRAINER_GENERATIONS")
        .ok()
        .and_then(|s| s.parse().ok());
    let small = std::env::var("CRUCIBLE_TRAINER_SMALL")
        .map(|v| v == "1")
        .unwrap_or(false);
    let mut cfg = if small {
        TrainerConfig::small()
    } else {
        TrainerConfig::default()
    };
    // `CRUCIBLE_TRAINER_BOOTSTRAP=1` runs the staged curriculum (plan §5.7)
    // on a cold start, so the self-play loop begins from a competent
    // population and a champion that already beats the hard bot.
    if std::env::var("CRUCIBLE_TRAINER_BOOTSTRAP")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        cfg.bootstrap = true;
    }

    tracing::info!(
        "training started: population {}, mu {}, {} self-play opponents, {} seeds/gen, {} ghosts/gen, match cap {} turns{}",
        cfg.population_size,
        cfg.mu,
        cfg.self_play_opponents,
        cfg.seeds_per_generation,
        cfg.ghosts_per_generation,
        cfg.match_timeout_turns,
        if cfg.bootstrap { " (bootstrap on cold start)" } else { "" },
    );

    tokio::task::spawn_blocking(move || {
        shared
            .running
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let mut trainer = match trainer::Trainer::start(store, shared.clone(), cfg) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("trainer failed to start: {e}");
                shared
                    .running
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                return;
            }
        };
        let mut n = 0usize;
        loop {
            match trainer.run_generation() {
                Ok(Some(p)) => tracing::info!(
                    "promoted genome {} (gen {}, Elo {:.0}, {:.0}% vs champion)",
                    p.genome_id,
                    p.generation,
                    p.elo,
                    p.gauntlet.champion_win_rate * 100.0
                ),
                Ok(None) => {}
                Err(e) => {
                    tracing::error!("trainer error: {e}");
                    break;
                }
            }
            n += 1;
            if let Some(limit) = generations {
                if n >= limit {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        shared
            .running
            .store(false, std::sync::atomic::Ordering::Relaxed);
        tracing::info!("trainer finished after {n} generations");
    });
}

async fn hello() -> impl IntoResponse {
    Json(json!({ "service": "crucible-server", "sim": crucible_sim::VERSION }))
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "uptime_secs": state.started_at.elapsed().as_secs(),
    }))
}

async fn list_replays(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let store = state.store.clone();
    // SQLite calls are blocking; run them off the async runtime.
    let res: Result<Vec<store::StoredMatch>, rusqlite::Error> =
        blocking(move || store.list_matches(100)).await?;
    res.map(|list| Json(json!({ "matches": list })))
        .map_err(err)
}

async fn get_replay(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let store = state.store.clone();
    let res: Result<Option<String>, rusqlite::Error> =
        blocking(move || store.get_replay(id)).await?;
    match res {
        Ok(Some(replay)) => Ok(Json(json!({ "replay": replay }))),
        Ok(None) => Err((StatusCode::NOT_FOUND, "no such replay".to_string())),
        Err(e) => Err(err(e)),
    }
}

/// Run a blocking store operation on the blocking pool so the async runtime
/// never stalls behind the SQLite mutex.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, (StatusCode, String)> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| err(format!("handler task failed: {e}")))
}

fn err(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let addr: SocketAddr = std::env::var("CRUCIBLE_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8787".into())
        .parse()
        .expect("invalid CRUCIBLE_ADDR");

    // `cargo run ... -- start` is the development-friendly entry point: it
    // replaces an older local Crucible server before claiming the same port.
    // Normal invocations stay conservative and fail loudly if another process
    // owns the configured address.
    if std::env::args().skip(1).any(|arg| arg == "start") {
        lifecycle::replace_existing_server(addr);
    }

    let db_path = std::env::var("CRUCIBLE_DB").unwrap_or_else(|_| "data/crucible.db".into());
    if let Some(parent) = std::path::Path::new(&db_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let store = Arc::new(Store::open(&db_path).expect("failed to open SQLite store"));
    let trainer_shared = Arc::new(TrainerShared::default());
    // Surface the checkpointed generation in /api/status before the trainer
    // (if enabled) resumes.
    if let Ok(Some(gen)) = store.latest_generation() {
        trainer_shared
            .generation
            .store(gen, std::sync::atomic::Ordering::Relaxed);
    }
    start_trainer(store.clone(), trainer_shared.clone());

    let state = AppState {
        store,
        trainer: trainer_shared,
        diagnostics: Arc::new(tokio::sync::Semaphore::new(1)),
        live_matches: Arc::new(tokio::sync::Semaphore::new(MAX_LIVE_MATCHES)),
        started_at: std::time::Instant::now(),
    };

    let static_dir = std::env::var("CRUCIBLE_CLIENT_DIR").unwrap_or_else(|_| "client/dist".into());
    tracing::info!("serving static client from {static_dir}");

    let app = Router::new()
        .route("/api/hello", get(hello))
        .route("/api/health", get(health))
        .route("/api/replays", get(list_replays))
        .route("/api/replay/{id}", get(get_replay))
        .route("/api/champion", get(http::champion))
        .route("/api/elo-history", get(http::elo_history))
        .route("/api/lineage/{id}", get(http::lineage))
        .route("/api/museum", get(http::museum))
        .route("/api/status", get(http::status))
        .route("/api/training-stats", get(http::training_stats))
        .route("/api/report/{old}/{new}", post(http::report))
        .route("/api/autobattle/{a}/{b}", post(http::autobattle))
        .route("/ws", get(ws::handler))
        .fallback_service(ServeDir::new(&static_dir).append_index_html_on_directories(true))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind");
    tracing::info!("listening on http://{addr}");
    let pid_file = lifecycle::write_pid_file(addr);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");
    if let Some(path) = pid_file {
        lifecycle::remove_pid_file(&path);
    }
}

/// Wait for Ctrl+C or SIGTERM so in-flight matches and the trainer's current
/// generation can finish persisting instead of the process dying mid-write.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown signal received; draining connections");
}
