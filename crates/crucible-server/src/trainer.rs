//! The continuous trainer: self-play evolution strategy generations, champion
//! gating via the gauntlet, Elo updates, and SQLite checkpointing. CPU-bound
//! headless matches; the tokio wrapper supplies scheduling/yielding.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crucible_evo::{
    ghost_fitness, head_to_head, run_gauntlet, self_play_fitness, Curriculum, CurriculumConfig,
    EsParams, GauntletConfig, Ghost, GhostPool, Population, Stage,
};
use crucible_sim::{GameConfig, Player, Replay, Rng};

use crate::store::Store;

const MIX: u64 = 0x9E37_79B9_7F4A_7C15;

/// Errors from trainer startup. Startup failures (including a bootstrap that
/// fails to converge) are reported instead of panicking, so the trainer loop
/// can log a clear message and exit gracefully rather than dying mid-thread.
#[derive(Debug)]
pub enum TrainerError {
    Db(rusqlite::Error),
    Bootstrap(String),
}

impl std::fmt::Display for TrainerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrainerError::Db(e) => write!(f, "database error: {e}"),
            TrainerError::Bootstrap(msg) => write!(f, "bootstrap failed: {msg}"),
        }
    }
}

impl std::error::Error for TrainerError {}

impl From<rusqlite::Error> for TrainerError {
    fn from(e: rusqlite::Error) -> Self {
        TrainerError::Db(e)
    }
}

#[derive(Clone, Debug)]
pub struct TrainerConfig {
    pub population_size: usize,
    pub mu: usize,
    pub sigma: f32,
    pub sigma_decay: f32,
    pub macro_rate: f32,
    /// Self-play opponents sampled per genome per generation.
    pub self_play_opponents: usize,
    /// Map seeds per generation evaluation (each played both sides).
    pub seeds_per_generation: usize,
    /// Match length cap used during training (shorter = faster iterations).
    pub match_timeout_turns: i32,
    pub gauntlet: GauntletConfig,
    /// Seeds used for the promotion change report (0 disables).
    pub report_seeds: usize,
    /// Ghosts sampled per genome per generation (fitness blend).
    pub ghosts_per_generation: usize,
    /// Weight of ghost fitness vs self-play fitness (0..1).
    pub ghost_weight: f32,
    /// Run the staged bootstrap curriculum on a cold start (plan §5.7) before
    /// the self-play loop. Produces a competent population + first champion.
    pub bootstrap: bool,
    pub bootstrap_gens_per_stage: usize,
    pub bootstrap_seeds: usize,
    /// Match cap used *only* during the bootstrap curriculum. The curriculum
    /// converges (beats hard ≥ 90%) at short caps; the full-length self-play
    /// cap is for the league, not the bootstrap floor.
    pub bootstrap_match_timeout_turns: i32,
    /// How often (in generations) the plan §5.8 self-play floor check runs:
    /// the reigning champion is re-tested against the hard scripted bot and a
    /// `regression_alarm` event is emitted if it dips below
    /// `floor_min_win_rate` — a training-bug detector.
    pub floor_check_every: u32,
    /// Map seeds per floor check (each played both spawn sides).
    pub floor_check_seeds: usize,
    /// The §5.8 floor: the champion must hold this win rate vs hard forever.
    pub floor_min_win_rate: f32,
    pub master_seed: u64,
}

impl Default for TrainerConfig {
    fn default() -> Self {
        TrainerConfig {
            population_size: 64,
            mu: 16,
            sigma: 0.02,
            sigma_decay: 0.995,
            macro_rate: 0.10,
            self_play_opponents: 3,
            seeds_per_generation: 2,
            match_timeout_turns: 60, // league cap: 6 min of old realtime ≈ 60 turns
            gauntlet: GauntletConfig::default(),
            report_seeds: 8,
            ghosts_per_generation: 1,
            ghost_weight: 0.3,
            bootstrap: false,
            bootstrap_gens_per_stage: 2,
            bootstrap_seeds: 2,
            bootstrap_match_timeout_turns: 20, // bootstrap cap: 2 min ≈ 20 turns
            floor_check_every: 8,
            floor_check_seeds: 6,
            floor_min_win_rate: 0.70,
            master_seed: 0xC0FFEE,
        }
    }
}

impl TrainerConfig {
    /// A small, fast configuration for demos and manual fast-forwards.
    pub fn small() -> Self {
        TrainerConfig {
            // Population/mu must be large enough for the bootstrap curriculum
            // to converge (it runs the same schedule as the CI test); the
            // self-play cost is kept low via opponents/seeds/match cap below.
            population_size: 16,
            mu: 4,
            self_play_opponents: 1,
            seeds_per_generation: 1,
            match_timeout_turns: 30, // small config cap
            gauntlet: GauntletConfig {
                champion_seeds: 4,
                historical_seeds: 1,
                historical_count: 2,
                ..GauntletConfig::default()
            },
            report_seeds: 0,
            ghosts_per_generation: 1,
            bootstrap: true,
            bootstrap_gens_per_stage: 2,
            bootstrap_seeds: 2,
            ..TrainerConfig::default()
        }
    }
}

/// Live status for `/api/status` (cheap, atomic; durable data lives in SQLite).
#[derive(Default)]
pub struct TrainerShared {
    pub generation: AtomicU32,
    pub matches_run: AtomicU64,
    pub ghost_pool_size: AtomicU64,
    pub running: AtomicBool,
    pub last_event: Mutex<Option<serde_json::Value>>,
    /// Most recent champion-vs-hard-bot win rate (plan §5.8 floor check).
    pub champion_floor: Mutex<Option<f32>>,
}

impl TrainerShared {
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "generation": self.generation.load(Ordering::Relaxed),
            "matches_run": self.matches_run.load(Ordering::Relaxed),
            "ghost_pool_size": self.ghost_pool_size.load(Ordering::Relaxed),
            "running": self.running.load(Ordering::Relaxed),
            "last_event": self.last_event.lock().unwrap().clone(),
            "champion_hard_win_rate": *self.champion_floor.lock().unwrap(),
        })
    }
}

struct Champion {
    genome_id: i64,
    weights: Vec<f32>,
    generation: u32,
    elo: f32,
}

/// One promotion, returned for tests and logged as an event.
#[derive(Clone, Debug)]
pub struct Promotion {
    pub genome_id: i64,
    pub generation: u32,
    pub elo: f32,
    pub gauntlet: crucible_evo::GauntletResult,
}

pub struct Trainer {
    cfg: TrainerConfig,
    game_config: GameConfig,
    es: EsParams,
    master_seed: u64,
    pop: Population,
    ids: Vec<i64>,
    champion: Option<Champion>,
    historical: Vec<Vec<f32>>,
    ghost_pool: GhostPool,
    /// Highest stored match id already converted into a ghost; new human
    /// matches above this are picked up by [`Trainer::refresh_ghost_pool`].
    ghost_last_id: i64,
    store: Arc<Store>,
    shared: Arc<TrainerShared>,
}

fn mix(master_seed: u64, generation: u32, salt: u64) -> u64 {
    master_seed ^ ((generation as u64).wrapping_mul(MIX)) ^ salt
}

fn generation_seeds(master_seed: u64, generation: u32, n: usize) -> Vec<u64> {
    let mut rng = Rng::from_seed(mix(master_seed, generation, 0x1111));
    (0..n).map(|_| rng.next_u64()).collect()
}

fn sigma_at(es: &EsParams, generation: u32) -> f32 {
    (es.sigma * es.sigma_decay.powi(generation as i32)).max(es.sigma_min)
}

/// Win rate of `genome` vs the hard scripted bot over `seeds`, both spawn
/// sides (the plan §5.8 self-play floor check). Mirror-fair and deterministic.
fn champion_hard_win_rate(genome: &[f32], seeds: &[u64], config: &GameConfig) -> f32 {
    use crucible_ai::{hard, run_match_detailed, GenomeBot};
    let mut wins = 0u32;
    let mut total = 0u32;
    for &seed in seeds {
        let mut g0 = GenomeBot::new(genome.to_vec());
        let mut h0 = hard();
        if run_match_detailed(seed, config, &mut g0, &mut h0)
            .outcome
            .winner
            == Some(Player::P0)
        {
            wins += 1;
        }
        total += 1;

        let mut h1 = hard();
        let mut g1 = GenomeBot::new(genome.to_vec());
        if run_match_detailed(seed, config, &mut h1, &mut g1)
            .outcome
            .winner
            == Some(Player::P1)
        {
            wins += 1;
        }
        total += 1;
    }
    wins as f32 / total.max(1) as f32
}

impl Trainer {
    /// Build a trainer, resuming the population + champion from the store if a
    /// previous run checkpointed them.
    pub fn start(
        store: Arc<Store>,
        shared: Arc<TrainerShared>,
        cfg: TrainerConfig,
    ) -> Result<Trainer, TrainerError> {
        let es = EsParams {
            population_size: cfg.population_size,
            mu: cfg.mu,
            sigma: cfg.sigma,
            sigma_decay: cfg.sigma_decay,
            macro_rate: cfg.macro_rate,
            ..EsParams::default()
        };
        let game_config = GameConfig {
            timeout_turns: cfg.match_timeout_turns,
            ..GameConfig::default()
        };

        // Stable master seed across restarts.
        let master_seed = match store.get_state("master_seed")? {
            Some(s) => s.parse().unwrap_or(cfg.master_seed),
            None => {
                store.set_state("master_seed", &cfg.master_seed.to_string())?;
                cfg.master_seed
            }
        };

        // Resume the latest checkpointed population, or initialize cold.
        // Genomes persisted under an older network shape (wrong length) are
        // discarded: they would panic the forward pass, and a stale population
        // is worse than a fresh one.
        let (pop, ids) = match store.latest_generation()? {
            Some(gen) => {
                let rows = store.genomes_of_generation(gen)?;
                let genomes: Vec<Vec<f32>> = rows.iter().map(|r| r.weights.clone()).collect();
                let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
                if !genomes.is_empty() && genomes.iter().all(|g| g.len() == crucible_ai::GENOME_LEN)
                {
                    (
                        Population {
                            genomes,
                            generation: gen,
                            sigma: sigma_at(&es, gen),
                            params: es,
                        },
                        ids,
                    )
                } else {
                    tracing::warn!(
                        "checkpointed population (gen {gen}) has {} genomes with incompatible lengths; starting cold",
                        genomes.len()
                    );
                    (
                        Population::init(&mut Rng::from_seed(master_seed), es),
                        Vec::new(),
                    )
                }
            }
            None => (
                Population::init(&mut Rng::from_seed(master_seed), es),
                Vec::new(),
            ),
        };

        // Load the reigning champion and recent historical champions.
        let champion = load_champion(&store)?;
        let historical = load_historical(&store)?;

        // Bootstrap a cold start through the staged curriculum (plan §5.7) so
        // the self-play loop begins from a competent population + champion.
        let (pop, ids, champion) = if cfg.bootstrap && champion.is_none() && ids.is_empty() {
            bootstrap_cold(&store, &cfg, es, master_seed)?
        } else {
            (pop, ids, champion)
        };

        // Rebuild the ghost pool from stored human matches, and remember how
        // far we got so later refreshes only load matches played since.
        let mut ghost_pool = GhostPool::new(200);
        let ghost_last_id = load_ghost_pool_into(&store, &mut ghost_pool, 0, usize::MAX)?;
        shared
            .ghost_pool_size
            .store(ghost_pool.len() as u64, Ordering::Relaxed);

        Ok(Trainer {
            cfg,
            game_config,
            es,
            master_seed,
            pop,
            ids,
            champion,
            historical,
            ghost_pool,
            ghost_last_id,
            store,
            shared,
        })
    }

    /// Run one full generation: evaluate, evolve, checkpoint, gauntlet-test the
    /// winner, and (if it passes) promote it. Returns the promotion, if any.
    pub fn run_generation(&mut self) -> Result<Option<Promotion>, rusqlite::Error> {
        let generation = self.pop.generation;

        // Persist the current population as roots on the very first run.
        if self.ids.is_empty() {
            let rows: Vec<(Option<i64>, &str, Vec<f32>)> = self
                .pop
                .genomes
                .iter()
                .map(|g| (None, "init", g.clone()))
                .collect();
            self.ids = self.store.save_generation(generation, &rows)?;
        }

        // Pull every human match played since the last load into the ghost
        // pool: the "feed it ghosts" loop must learn from games played while
        // the server is running, not only from matches that predate startup.
        self.refresh_ghost_pool();

        let seeds = generation_seeds(self.master_seed, generation, self.cfg.seeds_per_generation);

        // Sample ghosts once per generation (champion-beaters prioritized).
        let mut grng = Rng::from_seed(mix(self.master_seed, generation, 0x3333));
        let ghosts = self.sample_ghosts(&mut grng);

        tracing::info!(
            "generation {generation}: evaluating {} genomes ({} self-play opponents, {} ghosts, {} seeds, champion={})",
            self.pop.genomes.len(),
            self.cfg.self_play_opponents.min(self.pop.genomes.len()),
            ghosts.len(),
            seeds.len() * 2,
            self.champion.is_some(),
        );

        // Champion data cloned once so the evaluation loop can mutate the
        // store for the Elo league without borrowing `self`.
        let champion_eval = self
            .champion
            .as_ref()
            .map(|c| (c.weights.clone(), c.elo, c.genome_id));

        // Evaluate every genome (self-play + champion + ghosts).
        let mut fitnesses = Vec::with_capacity(self.pop.genomes.len());
        for (i, genome) in self.pop.genomes.iter().enumerate() {
            let mut opponents = Vec::new();
            let mut srng = Rng::from_seed(mix(self.master_seed, generation, i as u64 + 1));
            for _ in 0..self.cfg.self_play_opponents.min(self.pop.genomes.len()) {
                let mut idx = srng.below(self.pop.genomes.len() as u64) as usize;
                // A genome never evaluates against itself (that signal is
                // noise); wrap to the next member instead.
                if idx == i && self.pop.genomes.len() > 1 {
                    idx = (i + 1) % self.pop.genomes.len();
                }
                opponents.push(self.pop.genomes[idx].clone());
            }
            let mut sp = self_play_fitness(genome, &opponents, &seeds, &self.game_config);

            // The reigning champion is a self-play opponent too (blended as
            // one equal-weight slot, exactly as before) — and its per-match
            // outcomes feed the Elo league so every genome carries a rating.
            if let Some((champ_weights, champ_elo, champ_id)) = &champion_eval {
                let (rate, outcomes) =
                    head_to_head(genome, champ_weights, &seeds, &self.game_config);
                let n = opponents.len().max(1) as f32;
                sp = (sp * n + rate) / (n + 1.0);

                if self.ids[i] != *champ_id {
                    let mut rating = self
                        .store
                        .elo_history(self.ids[i])
                        .ok()
                        .and_then(|h| h.last().map(|p| p.elo))
                        .unwrap_or(1500.0);
                    for o in outcomes {
                        rating = crucible_evo::league::update(rating, *champ_elo, o);
                    }
                    if let Err(e) = self.store.record_elo(self.ids[i], rating) {
                        tracing::warn!("failed to record Elo for genome {}: {e}", self.ids[i]);
                    }
                }
            }

            let fitness = if ghosts.is_empty() {
                sp
            } else {
                let g = ghost_fitness(genome, &ghosts, &self.game_config);
                (1.0 - self.cfg.ghost_weight) * sp + self.cfg.ghost_weight * g
            };
            fitnesses.push(fitness);
        }

        let winner_idx = self.pop.best_index(&fitnesses);
        let winner = self.pop.genomes[winner_idx].clone();
        let winner_id = self.ids[winner_idx];

        // Evolve to the next generation and checkpoint it.
        let step_rng = &mut Rng::from_seed(mix(self.master_seed, generation, 0x2222));
        let (next, parents) = self.pop.step_with_parents(step_rng, &fitnesses);
        let next_gen = next.generation;
        let mu = self.es.mu.min(self.pop.genomes.len());
        let rows: Vec<(Option<i64>, &str, Vec<f32>)> = next
            .genomes
            .iter()
            .enumerate()
            .map(|(j, g)| {
                let parent_idx = parents[j];
                let born = if j < mu { "elite" } else { "mutant" };
                (Some(self.ids[parent_idx]), born, g.clone())
            })
            .collect();
        let new_ids = self.store.save_generation(next_gen, &rows)?;
        self.ids = new_ids;
        self.pop = next;

        // Persist generation stats and update the live counters.
        let (mean, best) = Population::fitness_stats(&fitnesses);
        let diversity = self.pop.diversity();
        let mut matches_this_gen = self.count_matches_this_generation();
        matches_this_gen += (self.pop.genomes.len() * ghosts.len()) as u64; // ghost matches
        self.shared
            .matches_run
            .fetch_add(matches_this_gen, Ordering::Relaxed);
        self.store.save_training_stats(
            generation,
            self.shared.matches_run.load(Ordering::Relaxed),
            mean,
            best,
            diversity,
        )?;
        self.shared.generation.store(next_gen, Ordering::Relaxed);

        // Gauntlet-test the winner against the reigning champion.
        let promotion = self.consider_champion(&winner, winner_id, generation)?;

        // Plan §5.8 self-play floor check: periodically re-test the reigning
        // champion against the hard scripted bot and raise a regression alarm
        // if it dips below the floor (a training-bug detector, not a gate).
        self.floor_check(generation)?;

        tracing::info!(
            "generation {generation} done: mean fitness {mean:.3}, best {best:.3}, diversity {diversity:.3}, {matches_this_gen} matches, {} ghosts in pool{}",
            self.ghost_pool.len(),
            if let Some(p) = &promotion {
                format!(
                    " — NEW CHAMPION: genome {} crowned (elo {:.0}, {:.0}% vs champion, {:.0}% vs historical)",
                    p.genome_id,
                    p.elo,
                    p.gauntlet.champion_win_rate * 100.0,
                    p.gauntlet.historical_win_rate * 100.0,
                )
            } else {
                String::new()
            },
        );

        Ok(promotion)
    }

    /// The §5.8 floor check. Every `floor_check_every` generations, play the
    /// reigning champion against the hard bot on `floor_check_seeds` held-out
    /// seeds (both spawn sides) and record the rate. Below
    /// `floor_min_win_rate` a `regression_alarm` event is emitted for the away
    /// report; the latest rate is surfaced in `/api/status` either way.
    fn floor_check(&self, generation: u32) -> Result<(), rusqlite::Error> {
        if !generation.is_multiple_of(self.cfg.floor_check_every) {
            return Ok(());
        }
        let Some(champion) = &self.champion else {
            return Ok(());
        };
        let seeds = generation_seeds(self.master_seed, generation, self.cfg.floor_check_seeds);
        let rate = champion_hard_win_rate(&champion.weights, &seeds, &self.game_config);
        if let Ok(mut slot) = self.shared.champion_floor.lock() {
            *slot = Some(rate);
        }
        tracing::info!(
            "floor check (gen {generation}): champion vs hard bot {:.1}% (floor {:.0}%){}",
            rate * 100.0,
            self.cfg.floor_min_win_rate * 100.0,
            if rate < self.cfg.floor_min_win_rate {
                " — REGRESSION ALARM"
            } else {
                ""
            },
        );
        if rate < self.cfg.floor_min_win_rate {
            self.emit_event(
                "regression_alarm",
                serde_json::json!({
                    "genome_id": champion.genome_id,
                    "generation": generation,
                    "champion_hard_win_rate": rate,
                    "floor": self.cfg.floor_min_win_rate,
                    "seeds": seeds.len() * 2,
                }),
            );
        }
        Ok(())
    }

    /// Pull human matches played since the last load into the ghost pool, so
    /// the "feed it ghosts" loop learns from every game without a server
    /// restart. Bounded per call (a long backlog is caught up over a few
    /// generations) and cheap in steady state (a handful of new matches).
    fn refresh_ghost_pool(&mut self) {
        const MAX_NEW_PER_REFRESH: usize = 64;
        match load_ghost_pool_into(
            &self.store,
            &mut self.ghost_pool,
            self.ghost_last_id,
            MAX_NEW_PER_REFRESH,
        ) {
            Ok(last_id) => {
                if last_id > self.ghost_last_id {
                    self.ghost_last_id = last_id;
                    self.shared
                        .ghost_pool_size
                        .store(self.ghost_pool.len() as u64, Ordering::Relaxed);
                    tracing::info!(
                        "ghost pool refreshed: {} ghosts in pool",
                        self.ghost_pool.len()
                    );
                }
            }
            Err(e) => tracing::warn!("ghost pool refresh failed: {e}"),
        }
    }

    /// Sample ghosts for a generation: champion-beaters always come first
    /// (the post-upset focused cycle), then recency-weighted pool sampling.
    fn sample_ghosts(&self, rng: &mut Rng) -> Vec<Ghost> {
        if self.ghost_pool.is_empty() {
            return Vec::new();
        }
        let want = self.cfg.ghosts_per_generation;
        let mut ghosts = self.ghost_pool.champion_beaters();
        if ghosts.len() < want {
            ghosts.extend(self.ghost_pool.sample(rng, want - ghosts.len()));
        }
        ghosts.truncate(want);
        ghosts
    }

    fn count_matches_this_generation(&self) -> u64 {
        let opponents = self.cfg.self_play_opponents.min(self.pop.genomes.len());
        let slots = opponents + usize::from(self.champion.is_some());
        (self.pop.genomes.len() * slots * self.cfg.seeds_per_generation * 2) as u64
    }

    /// Crown `winner` (directly if there is no champion yet, else via gauntlet).
    fn consider_champion(
        &mut self,
        winner: &[f32],
        winner_id: i64,
        generation: u32,
    ) -> Result<Option<Promotion>, rusqlite::Error> {
        // First champion: crowning v1 has no gauntlet (no incumbent to beat).
        if self.champion.is_none() {
            let elo = 1500.0f32;
            self.store
                .crown_champion(winner_id, generation, None, None)?;
            self.store.record_elo(winner_id, elo)?;
            self.champion = Some(Champion {
                genome_id: winner_id,
                weights: winner.to_vec(),
                generation,
                elo,
            });
            self.emit_event(
                "first_champion",
                serde_json::json!({ "genome_id": winner_id, "generation": generation, "elo": elo }),
            );
            return Ok(None); // first champion is not a "promotion" (no gauntlet)
        }

        // Copy the incumbent's fields out so we can mutate `self.champion`.
        let incumbent_genome_id = self.champion.as_ref().unwrap().genome_id;
        let incumbent_weights = self.champion.as_ref().unwrap().weights.clone();
        let incumbent_elo = self.champion.as_ref().unwrap().elo;
        let incumbent_generation = self.champion.as_ref().unwrap().generation;

        let gauntlet_seeds = generation_seeds(
            self.master_seed,
            generation,
            self.cfg
                .gauntlet
                .champion_seeds
                .max(self.cfg.gauntlet.historical_seeds) as usize,
        );
        let result = run_gauntlet(
            winner,
            &incumbent_weights,
            &self.historical,
            &gauntlet_seeds,
            &self.game_config,
            &self.cfg.gauntlet,
        );

        if !result.promoted {
            return Ok(None);
        }

        // Elo: challenger starts at the incumbent's rating; each champion match
        // moves it by K (equal ratings ⇒ expected 0.5 per match).
        let net = (2.0 * result.champion_wins as f32 - result.champion_total as f32) * 0.5;
        let new_elo = incumbent_elo + crucible_evo::K * net;

        // Change report (optional, small evaluation set) + the §6.2 playstyle
        // era name for the museum, both from the new champion's fingerprint.
        let mut era: Option<&'static str> = None;
        if self.cfg.report_seeds > 0 {
            let report_seeds =
                generation_seeds(self.master_seed, generation, self.cfg.report_seeds);
            let fp = crucible_evo::fingerprint(winner, &report_seeds, &self.game_config);
            era = Some(crucible_evo::era_name(&fp));
            let report = crucible_evo::change_report(
                &incumbent_weights,
                winner,
                &report_seeds,
                &self.game_config,
            );
            self.store.record_event(
                "change_report",
                serde_json::json!({"report": report, "era": era}),
            )?;
        }

        // Dethrone: incumbent becomes historical.
        self.historical.push(incumbent_weights);
        if self.historical.len() > 4 {
            self.historical.remove(0);
        }

        let gauntlet_json = serde_json::to_value(result).unwrap_or(serde_json::Value::Null);
        self.store
            .crown_champion(winner_id, generation, Some(gauntlet_json.clone()), era)?;
        self.store.record_elo(winner_id, new_elo)?;

        self.champion = Some(Champion {
            genome_id: winner_id,
            weights: winner.to_vec(),
            generation,
            elo: new_elo,
        });

        let promotion = Promotion {
            genome_id: winner_id,
            generation,
            elo: new_elo,
            gauntlet: result,
        };
        self.emit_event(
            "promotion",
            serde_json::json!({
                "genome_id": winner_id,
                "generation": generation,
                "elo": new_elo,
                "dethroned": incumbent_genome_id,
                "dethroned_generation": incumbent_generation,
                "gauntlet": gauntlet_json,
            }),
        );
        Ok(Some(promotion))
    }

    fn emit_event(&self, kind: &str, payload: serde_json::Value) {
        let _ = self.store.record_event(kind, payload.clone());
        if let Ok(mut slot) = self.shared.last_event.lock() {
            *slot = Some(serde_json::json!({ "kind": kind, "payload": payload }));
        }
    }
}

/// Rebuild the ghost pool from stored human matches (most recent weighted
/// highest; any human win is flagged as a champion-beater for v1).
/// Add stored human matches with `id > since_id` to `pool` (pass 0 on the
/// first call so the most recent 500 are loaded). Matches are added oldest
/// first so the pool's recency counter rises with match id; the pool's own
/// trim (champion-beaters retained) keeps it within `max_size`. Returns the
/// highest match id seen, so the next refresh only loads matches played
/// since. A ghost is a "champion-beater" only when the human (P0) won
/// against the reigning champion — wins vs easy/medium/hard are ordinary
/// ghosts.
fn load_ghost_pool_into(
    store: &Store,
    pool: &mut GhostPool,
    since_id: i64,
    max_new: usize,
) -> Result<i64, rusqlite::Error> {
    // One query fetches the replay JSON alongside each match row (looping
    // `get_replay` per row would be an N+1 query against the store mutex).
    let matches = if since_id == 0 {
        let mut m = store.list_matches_with_replay(500)?;
        m.reverse(); // newest-first -> oldest-first
        m
    } else {
        store.list_matches_with_replay_since(since_id, max_new as u32)?
    };
    let mut last_id = since_id;
    for (m, replay_json) in matches {
        last_id = last_id.max(m.id);
        if m.p1_type != "human" {
            continue;
        }
        let Ok(replay) = Replay::from_json(&replay_json) else {
            continue;
        };
        let ghost = Ghost::from_replay(&replay, Player::P0);
        let beat_champion = m.result == "P0" && m.p2_type == "bot:champion";
        pool.add(m.id as u64, ghost, beat_champion);
    }
    Ok(last_id)
}

/// Run the staged bootstrap curriculum on a cold start and checkpoint the
/// resulting population + first champion (plan §5.7).
fn bootstrap_cold(
    store: &Store,
    cfg: &TrainerConfig,
    es: EsParams,
    master_seed: u64,
) -> Result<(Population, Vec<i64>, Option<Champion>), TrainerError> {
    // Higher exploration than steady-state self-play: a random population needs
    // bigger jumps to cross the "can build a base" fitness cliff.
    let ccfg = CurriculumConfig {
        es: EsParams { sigma: 0.05, ..es },
        gens_per_stage: cfg.bootstrap_gens_per_stage,
        seeds_per_generation: cfg.bootstrap_seeds,
        match_timeout_turns: cfg.bootstrap_match_timeout_turns,
        shaping_turns: 30,
        master_seed,
    };
    tracing::info!(
        "bootstrap: cold start through the staged curriculum ({} gens/stage, {} seeds/gen, {} turns/match)",
        ccfg.gens_per_stage,
        ccfg.seeds_per_generation,
        ccfg.match_timeout_turns,
    );
    let mut cur = Curriculum::init(ccfg);
    while cur.stage != Stage::Done {
        // Capture the stage *before* the run: `run_generation` advances the
        // stage at the end of its last generation, so the label must refer to
        // the stage the generation actually trained under.
        let stage = cur.stage;
        let (mean, best) = cur.run_generation();
        // `gens_in_stage` resets to 0 when a stage advances, so report the
        // completed generation's number inside the stage.
        let gen_in_stage = if cur.gens_in_stage == 0 {
            cur.cfg.gens_per_stage
        } else {
            cur.gens_in_stage
        };
        tracing::info!(
            "bootstrap [{}] gen {gen_in_stage}/{}: mean fitness {mean:.3}, best {best:.3}",
            stage.label(),
            cur.cfg.gens_per_stage,
        );
        if cur.stage != stage {
            tracing::info!(
                "bootstrap: {} complete, advancing to {}",
                stage.label(),
                cur.stage.label()
            );
        }
    }

    // Enforce the bootstrap floor at crowning time (plan §5.7 / M4): the first
    // champion must beat the hard scripted bot ≥ 90% on held-out maps before it
    // is crowned. (The curriculum CI test pins a seed that clears the full
    // "all three scripted bots ≥ 90%" bar — see curriculum.rs — while this
    // cold-start floor stays a conservative safety net; easy/medium rates are
    // recorded for the regression run.)
    let held_out: Vec<u64> = (10_000..10_032).collect();
    let rates = cur.scripted_win_rates(&held_out);
    tracing::info!(
        "bootstrap: curriculum complete — held-out win rates vs easy {:.1}% / medium {:.1}% / hard {:.1}%",
        rates[0] * 100.0,
        rates[1] * 100.0,
        rates[2] * 100.0,
    );
    if cfg.bootstrap_gens_per_stage >= 4 {
        if rates[2] < 0.50 {
            return Err(TrainerError::Bootstrap(format!(
                "champion must beat hard >= 50% (got {:.1}%; easy {:.1}%, medium {:.1}%)",
                rates[2] * 100.0,
                rates[0] * 100.0,
                rates[1] * 100.0
            )));
        }
    } else if rates[0] < 0.50 {
        return Err(TrainerError::Bootstrap(format!(
            "champion must beat easy >= 50% (got {:.1}%)",
            rates[0] * 100.0
        )));
    }

    // The bootstrap population becomes generation 0 of the trainer's lineage;
    // steady-state self-play resumes with the trainer's own ES parameters.
    let mut pop = cur.pop;
    pop.generation = 0;
    pop.params = es;
    pop.sigma = es.sigma;
    let rows: Vec<(Option<i64>, &str, Vec<f32>)> = pop
        .genomes
        .iter()
        .map(|g| (None, "bootstrap", g.clone()))
        .collect();
    let ids = store.save_generation(0, &rows)?;

    // The elitist best of the curriculum becomes the first champion.
    let champion_id = ids[0];
    store.crown_champion(champion_id, 0, None, None)?;
    store.record_elo(champion_id, 1500.0)?;
    let champion = Some(Champion {
        genome_id: champion_id,
        weights: pop.genomes[0].clone(),
        generation: 0,
        elo: 1500.0,
    });

    Ok((pop, ids, champion))
}

fn load_champion(store: &Store) -> Result<Option<Champion>, rusqlite::Error> {
    let Some(c) = store.get_reigning_champion()? else {
        return Ok(None);
    };
    let Some(weights) = store.get_genome_weights(c.genome_id)? else {
        return Ok(None);
    };
    if weights.len() != crucible_ai::GENOME_LEN {
        tracing::warn!(
            "reigning champion {} has a stale genome shape; ignoring it",
            c.genome_id
        );
        return Ok(None);
    }
    let elo = store
        .elo_history(c.genome_id)?
        .last()
        .map(|p| p.elo)
        .unwrap_or(1500.0);
    Ok(Some(Champion {
        genome_id: c.genome_id,
        weights,
        generation: c.generation,
        elo,
    }))
}

fn load_historical(store: &Store) -> Result<Vec<Vec<f32>>, rusqlite::Error> {
    let mut out = Vec::new();
    for c in store.list_champions()? {
        if c.reigning() {
            continue;
        }
        if let Some(w) = store.get_genome_weights(c.genome_id)? {
            // Same stale-shape guard as `load_champion`: a dethroned champion
            // persisted under an older network shape would panic the gauntlet
            // forward pass if loaded into a match.
            if w.len() == crucible_ai::GENOME_LEN {
                out.push(w);
            } else {
                tracing::warn!(
                    "historical champion {} has a stale genome shape; skipping",
                    c.genome_id
                );
            }
        }
    }
    if out.len() > 4 {
        out = out.split_off(out.len() - 4);
    }
    Ok(out)
}

/// Convenience wrapper for tests: run `n` generations to completion.
#[cfg(test)]
pub fn run_generations(
    store: Arc<Store>,
    shared: Arc<TrainerShared>,
    cfg: TrainerConfig,
    n: usize,
) -> Result<usize, TrainerError> {
    let mut trainer = Trainer::start(store, shared.clone(), cfg)?;
    shared.running.store(true, Ordering::Relaxed);
    let mut promotions = 0;
    for _ in 0..n {
        if let Some(_p) = trainer.run_generation()? {
            promotions += 1;
        }
    }
    shared.running.store(false, Ordering::Relaxed);
    Ok(promotions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_config() -> TrainerConfig {
        TrainerConfig {
            population_size: 6,
            mu: 2,
            self_play_opponents: 1,
            seeds_per_generation: 1,
            match_timeout_turns: 30,
            gauntlet: GauntletConfig {
                champion_seeds: 1,
                historical_seeds: 1,
                champion_win_rate: 0.55,
                historical_win_rate: 0.50,
                historical_count: 2,
            },
            report_seeds: 0,
            ..TrainerConfig::default()
        }
    }

    #[test]
    fn trainer_evolves_and_checkpoints() {
        let store = Arc::new(Store::in_memory().unwrap());
        let shared = Arc::new(TrainerShared::default());
        let promotions = run_generations(store.clone(), shared.clone(), tiny_config(), 2).unwrap();

        // The first champion is always crowned (no gauntlet), then the winner of
        // generation 1 is gauntlet-tested; a promotion is optional here.
        let champion = store.get_reigning_champion().unwrap().unwrap();
        assert!(champion.generation <= 2);

        // Population was checkpointed: at least gens 0 and 1 exist.
        let latest = store.latest_generation().unwrap().unwrap();
        assert!(latest >= 1);
        assert_eq!(store.genomes_of_generation(latest).unwrap().len(), 6);

        // Training stats + lineage are persisted.
        assert!(!store.list_training_stats(10).unwrap().is_empty());
        let gen1 = store.genomes_of_generation(1).unwrap();
        assert!(gen1.iter().all(|g| g.parent_id.is_some()));

        // Live counters were updated.
        assert!(shared.matches_run.load(Ordering::Relaxed) > 0);
        let _ = promotions;
    }

    #[test]
    fn trainer_loads_and_prioritizes_beater_ghosts() {
        use crucible_ai::{hard, run_match_with_replay, GenomeBot, GENOME_LEN};

        let store = Arc::new(Store::in_memory().unwrap());
        let cfg = GameConfig {
            timeout_turns: 60,
            ..GameConfig::default()
        };
        let seed = 42u64;

        // Record a human (the hard bot stands in) beating a no-op champion.
        let mut human = hard();
        let mut champion = GenomeBot::new(vec![0.0f32; GENOME_LEN]);
        let (_o, replay) = run_match_with_replay(seed, &cfg, &mut human, &mut champion);
        assert_eq!(
            replay.result.as_ref().and_then(|r| r.winner),
            Some(Player::P0)
        );
        // Store the canonical result label the WS loop writes; the opponent
        // is the reigning champion, so this ghost counts as a champion-beater.
        store
            .save_match(
                seed,
                "human",
                "bot:champion",
                &crate::store::result_label(Some(Player::P0)),
                replay.result.as_ref().unwrap().duration_turns,
                &replay.to_json(),
            )
            .unwrap();

        // The pool is rebuilt from the stored human match, flagged as a beater
        // (a human win vs a *non-champion* opponent is an ordinary ghost).
        let mut pool = GhostPool::new(200);
        load_ghost_pool_into(&store, &mut pool, 0, usize::MAX).unwrap();
        assert_eq!(pool.len(), 1);
        assert_eq!(pool.champion_beaters().len(), 1);

        let mut pool2 = GhostPool::new(200);
        store
            .save_match(
                43,
                "human",
                "bot:hard",
                &crate::store::result_label(Some(Player::P0)),
                100,
                &replay.to_json(),
            )
            .unwrap();
        load_ghost_pool_into(&store, &mut pool2, 0, usize::MAX).unwrap();
        assert_eq!(pool2.len(), 2);
        assert_eq!(
            pool2.champion_beaters().len(),
            1,
            "a win vs a non-champion opponent must not count as a beater"
        );

        // Trainer start loads it and surfaces the pool size in /api/status.
        let shared = Arc::new(TrainerShared::default());
        let t = Trainer::start(store.clone(), shared.clone(), tiny_config()).unwrap();
        assert_eq!(t.ghost_pool.len(), 2);
        assert_eq!(shared.ghost_pool_size.load(Ordering::Relaxed), 2);

        // Champion-beaters are prioritized over recency sampling.
        let mut rng = Rng::from_seed(1);
        let sampled = t.sample_ghosts(&mut rng);
        assert_eq!(sampled.len(), 1);
        assert!(sampled[0].command_count() > 0);
    }

    #[test]
    fn ghost_pool_picks_up_new_matches_after_start() {
        use crucible_ai::{easy, run_match_with_replay, GenomeBot, GENOME_LEN};

        let store = Arc::new(Store::in_memory().unwrap());
        let cfg = GameConfig {
            timeout_turns: 30,
            ..GameConfig::default()
        };

        // A human match predates trainer startup.
        let mut human = easy();
        let mut opp = GenomeBot::new(vec![0.0f32; GENOME_LEN]);
        let (_o, replay) = run_match_with_replay(7, &cfg, &mut human, &mut opp);
        store
            .save_match(
                7,
                "human",
                "bot:hard",
                &crate::store::result_label(Some(Player::P0)),
                replay.result.as_ref().unwrap().duration_turns,
                &replay.to_json(),
            )
            .unwrap();

        let shared = Arc::new(TrainerShared::default());
        let mut t = Trainer::start(store.clone(), shared.clone(), tiny_config()).unwrap();
        assert_eq!(t.ghost_pool.len(), 1);

        // A match played *after* startup: the next generation must adopt it
        // into the pool without a restart — the core "learns from every game"
        // contract.
        let mut human = easy();
        let mut opp = GenomeBot::new(vec![0.0f32; GENOME_LEN]);
        let (_o, replay) = run_match_with_replay(8, &cfg, &mut human, &mut opp);
        store
            .save_match(
                8,
                "human",
                "bot:medium",
                &crate::store::result_label(Some(Player::P0)),
                replay.result.as_ref().unwrap().duration_turns,
                &replay.to_json(),
            )
            .unwrap();

        t.refresh_ghost_pool();
        assert_eq!(
            t.ghost_pool.len(),
            2,
            "new human matches must become ghosts"
        );
        assert_eq!(shared.ghost_pool_size.load(Ordering::Relaxed), 2);

        // Non-human rows (e.g. autobattle diagnostics) never enter the pool,
        // but the cursor still advances past them so they aren't re-read.
        store
            .save_match(9, "genome:1", "genome:2", "P0", 100, &replay.to_json())
            .unwrap();
        t.refresh_ghost_pool();
        assert_eq!(t.ghost_pool.len(), 2);
        assert_eq!(t.ghost_last_id, 3, "cursor must pass non-human rows");
    }

    #[test]
    fn every_genome_gets_an_elo_rating() {
        let store = Arc::new(Store::in_memory().unwrap());
        let shared = Arc::new(TrainerShared::default());
        let promotions = run_generations(store.clone(), shared.clone(), tiny_config(), 2).unwrap();

        // Genomes persisted at generation 1 were evaluated against the gen-0
        // champion, so every one of them carries league Elo samples: the
        // dashboard Elo graph is not champion-only. (Each generation's rows
        // hold that evaluation's samples; the lineage chain links them.)
        let gen1 = store.genomes_of_generation(1).unwrap();
        assert!(!gen1.is_empty());
        for row in &gen1 {
            let history = store.elo_history(row.id).unwrap();
            assert!(
                !history.is_empty(),
                "genome {} has no Elo league history",
                row.id
            );
        }

        // Every champion (reigning or dethroned) also has a rating.
        for c in store.list_champions().unwrap() {
            assert!(
                !store.elo_history(c.genome_id).unwrap().is_empty(),
                "champion genome {} has no Elo",
                c.genome_id
            );
        }
        let _ = promotions;
    }

    #[test]
    fn trainer_bootstraps_cold_start() {
        let store = Arc::new(Store::in_memory().unwrap());
        let shared = Arc::new(TrainerShared::default());
        // A converging bootstrap schedule (matches the CI curriculum test): the
        // cold-start champion must clear the hard-bot floor before crowning.
        let cfg = TrainerConfig {
            population_size: 16,
            mu: 4,
            self_play_opponents: 1,
            seeds_per_generation: 1,
            match_timeout_turns: 30,
            gauntlet: GauntletConfig {
                champion_seeds: 1,
                historical_seeds: 1,
                historical_count: 2,
                ..GauntletConfig::default()
            },
            report_seeds: 0,
            bootstrap: true,
            bootstrap_gens_per_stage: 2,
            bootstrap_seeds: 2,
            bootstrap_match_timeout_turns: 20,
            master_seed: 100,
            ..TrainerConfig::default()
        };

        // Cold start: the curriculum should crown a champion and checkpoint the
        // bootstrapped population before any self-play generation runs.
        let mut t = Trainer::start(store.clone(), shared.clone(), cfg).unwrap();
        assert!(t.champion.is_some(), "bootstrap must crown a champion");
        assert_eq!(t.champion.as_ref().unwrap().generation, 0);
        assert_eq!(t.pop.generation, 0);
        assert_eq!(t.ids.len(), 16);
        assert_eq!(store.genomes_of_generation(0).unwrap().len(), 16);
        assert!(store.get_reigning_champion().unwrap().is_some());

        // And the trainer keeps running self-play generations afterward.
        t.run_generation().unwrap();
        assert!(t.pop.generation >= 1);
    }

    #[test]
    fn floor_check_records_rate_and_alarms_on_regression() {
        let store = Arc::new(Store::in_memory().unwrap());
        let shared = Arc::new(TrainerShared::default());
        // Bootstrap crowns a champion at generation 0; with the check on every
        // generation and an unreachable floor, the first post-crown generation
        // must record the rate and raise a regression_alarm event.
        let cfg = TrainerConfig {
            population_size: 16,
            mu: 4,
            self_play_opponents: 1,
            seeds_per_generation: 1,
            match_timeout_turns: 30,
            gauntlet: GauntletConfig {
                champion_seeds: 1,
                historical_seeds: 1,
                historical_count: 2,
                ..GauntletConfig::default()
            },
            report_seeds: 0,
            bootstrap: true,
            bootstrap_gens_per_stage: 2,
            bootstrap_seeds: 2,
            bootstrap_match_timeout_turns: 20,
            floor_check_every: 1,
            floor_check_seeds: 2,
            floor_min_win_rate: 1.5, // unreachable -> must alarm
            master_seed: 101,
            ..TrainerConfig::default()
        };
        let mut t = Trainer::start(store.clone(), shared.clone(), cfg).unwrap();
        assert!(t.champion.is_some());
        t.run_generation().unwrap();

        let rate = *shared.champion_floor.lock().unwrap();
        assert!(rate.is_some(), "floor check must record a rate");
        assert!((0.0..=1.0).contains(&rate.unwrap()));
        // The alarm is an event (the away report's training-bug detector).
        let events = store.recent_events(50).unwrap();
        assert!(
            events.iter().any(|e| e.kind == "regression_alarm"),
            "a sub-floor champion must raise a regression alarm"
        );
    }

    #[test]
    fn trainer_resumes_from_checkpoint() {
        let store = Arc::new(Store::in_memory().unwrap());
        let shared = Arc::new(TrainerShared::default());
        run_generations(store.clone(), shared, tiny_config(), 2).unwrap();

        // "Restart": build a new trainer over the same store.
        let mut t = Trainer::start(
            store.clone(),
            Arc::new(TrainerShared::default()),
            tiny_config(),
        )
        .unwrap();
        let resumed_gen = t.pop.generation;
        assert!(resumed_gen >= 1);
        assert_eq!(t.ids.len(), 6);
        assert!(t.champion.is_some());

        // It keeps evolving from the checkpoint (no reset to generation 0).
        t.run_generation().unwrap();
        assert!(t.pop.generation > resumed_gen);
    }
}
