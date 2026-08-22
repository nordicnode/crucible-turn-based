//! The bootstrap curriculum (plan §5.7): staged evolution from a random
//! population to a genome that beats the hard scripted bot. Pure — the caller
//! supplies seeds/config and drives generations; no IO or scheduling.
//!
//! Stages, in order: economy (ore mined) → production (army value) → combat
//! (vs idle) → scripted easy → medium → hard → a final combined
//! easy+medium+hard gauntlet stage. Each stage runs a *bounded* number of ES
//! generations and then advances, so the whole schedule is a fixed,
//! reproducible budget. The final measurement is the best genome's win rate
//! against the scripted bots on held-out seeds.

use crucible_ai::{easy, hard, medium, run_match, Bot, GenomeBot};
use crucible_sim::{GameConfig, Player, Rng};

use crate::fitness::{evaluate_economy, evaluate_production, evaluate_vs, Noop};
use crate::population::{EsParams, Population};

const MIX: u64 = 0x9E37_79B9_7F4A_7C15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Economy,
    Production,
    Combat,
    ScriptedEasy,
    ScriptedMedium,
    ScriptedHard,
    /// Final combined stage: optimize against easy+medium+hard at once so the
    /// champion clears the §5.7 regression bar (≥ 90% vs every scripted bot)
    /// instead of overfitting the last bot it trained on.
    ScriptedGauntlet,
    Done,
}

impl Stage {
    pub fn next(self) -> Stage {
        match self {
            Stage::Economy => Stage::Production,
            Stage::Production => Stage::Combat,
            Stage::Combat => Stage::ScriptedEasy,
            Stage::ScriptedEasy => Stage::ScriptedMedium,
            Stage::ScriptedMedium => Stage::ScriptedHard,
            Stage::ScriptedHard => Stage::ScriptedGauntlet,
            Stage::ScriptedGauntlet => Stage::Done,
            Stage::Done => Stage::Done,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Stage::Economy => "economy",
            Stage::Production => "production",
            Stage::Combat => "combat",
            Stage::ScriptedEasy => "scripted-easy",
            Stage::ScriptedMedium => "scripted-medium",
            Stage::ScriptedHard => "scripted-hard",
            Stage::ScriptedGauntlet => "scripted-gauntlet",
            Stage::Done => "done",
        }
    }

    fn id(self) -> u64 {
        match self {
            Stage::Economy => 0,
            Stage::Production => 1,
            Stage::Combat => 2,
            Stage::ScriptedEasy => 3,
            Stage::ScriptedMedium => 4,
            Stage::ScriptedHard => 5,
            Stage::ScriptedGauntlet => 6,
            Stage::Done => 7,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CurriculumConfig {
    pub es: EsParams,
    /// Generations to run in each stage before advancing.
    pub gens_per_stage: usize,
    pub seeds_per_generation: usize,
    /// Match cap for opponent-based stages (combat + scripted).
    pub match_timeout_turns: i32,
    /// Tick cap for the solo shaping stages (economy + production).
    pub shaping_turns: i32,
    pub master_seed: u64,
}

impl Default for CurriculumConfig {
    fn default() -> Self {
        CurriculumConfig {
            es: EsParams::default(),
            gens_per_stage: 6,
            seeds_per_generation: 4,
            match_timeout_turns: 45,
            shaping_turns: 30,
            master_seed: 0xB007_57A6,
        }
    }
}

fn mix(master_seed: u64, stage: Stage, generation: u32) -> u64 {
    master_seed ^ (stage.id().wrapping_mul(MIX)) ^ (generation as u64).wrapping_mul(MIX >> 1)
}

/// Play `genome` against `make_opponent` on every seed, both spawn sides
/// (mirror fairness), and return the genome's win fraction.
fn mirror_win_rate(
    genome: &[f32],
    seeds: &[u64],
    config: &GameConfig,
    make_opponent: impl Fn() -> Box<dyn Bot>,
) -> f32 {
    let mut wins = 0u32;
    let mut total = 0u32;
    for &seed in seeds {
        // genome = P0
        let mut g = GenomeBot::new(genome.to_vec());
        let mut o = make_opponent();
        if run_match(seed, config, &mut g, o.as_mut()).winner == Some(Player::P0) {
            wins += 1;
        }
        total += 1;

        // genome = P1
        let mut g = GenomeBot::new(genome.to_vec());
        let mut o = make_opponent();
        if run_match(seed, config, o.as_mut(), &mut g).winner == Some(Player::P1) {
            wins += 1;
        }
        total += 1;
    }
    if total == 0 {
        0.0
    } else {
        wins as f32 / total as f32
    }
}

pub struct Curriculum {
    pub pop: Population,
    pub stage: Stage,
    pub gens_in_stage: usize,
    pub cfg: CurriculumConfig,
    pub match_config: GameConfig,
    pub shaping_config: GameConfig,
}

impl Curriculum {
    pub fn init(cfg: CurriculumConfig) -> Self {
        let mut rng = Rng::from_seed(cfg.master_seed);
        let pop = Population::init(&mut rng, cfg.es);
        Curriculum {
            pop,
            stage: Stage::Economy,
            gens_in_stage: 0,
            match_config: GameConfig {
                timeout_turns: cfg.match_timeout_turns,
                ..GameConfig::default()
            },
            shaping_config: GameConfig {
                timeout_turns: 10_000,
                ..GameConfig::default()
            },
            cfg,
        }
    }

    fn generation_seeds(&self) -> Vec<u64> {
        let mut rng = Rng::from_seed(mix(self.cfg.master_seed, self.stage, self.pop.generation));
        (0..self.cfg.seeds_per_generation)
            .map(|_| rng.next_u64())
            .collect()
    }

    /// The fitness signal for the current stage (see plan §5.7).
    pub fn evaluate(&self, genome: &[f32]) -> f32 {
        let seeds = self.generation_seeds();
        match self.stage {
            Stage::Economy => {
                evaluate_economy(genome, &seeds, &self.shaping_config, self.cfg.shaping_turns)
            }
            Stage::Production => {
                evaluate_production(genome, &seeds, &self.shaping_config, self.cfg.shaping_turns)
            }
            Stage::Combat => evaluate_vs(genome, &seeds, &self.match_config, || -> Box<dyn Bot> {
                Box::new(Noop)
            }),
            Stage::ScriptedEasy => {
                evaluate_vs(genome, &seeds, &self.match_config, || -> Box<dyn Bot> {
                    Box::new(easy())
                })
            }
            Stage::ScriptedMedium => {
                evaluate_vs(genome, &seeds, &self.match_config, || -> Box<dyn Bot> {
                    Box::new(medium())
                })
            }
            Stage::ScriptedHard => {
                evaluate_vs(genome, &seeds, &self.match_config, || -> Box<dyn Bot> {
                    Box::new(hard())
                })
            }
            Stage::ScriptedGauntlet => {
                // Mean shaped fitness vs all three scripted bots. Optimizing
                // the combined signal keeps the champion strong everywhere
                // (the §5.7 regression bar) rather than against one opponent.
                let e = evaluate_vs(genome, &seeds, &self.match_config, || -> Box<dyn Bot> {
                    Box::new(easy())
                });
                let m = evaluate_vs(genome, &seeds, &self.match_config, || -> Box<dyn Bot> {
                    Box::new(medium())
                });
                let h = evaluate_vs(genome, &seeds, &self.match_config, || -> Box<dyn Bot> {
                    Box::new(hard())
                });
                (e + m + h) / 3.0
            }
            Stage::Done => 0.0,
        }
    }

    /// Run one ES generation under the current stage, advancing the stage when
    /// its bounded budget is spent. Returns the generation's (mean, best)
    /// fitness under that stage's signal.
    pub fn run_generation(&mut self) -> (f32, f32) {
        if self.stage == Stage::Done {
            return (0.0, 0.0);
        }
        let fitnesses: Vec<f32> = self.pop.genomes.iter().map(|g| self.evaluate(g)).collect();
        let (mean, best) = Population::fitness_stats(&fitnesses);
        let mut rng = Rng::from_seed(
            mix(self.cfg.master_seed, self.stage, self.pop.generation)
                .wrapping_add(0x1234_5678_9ABC_DEF0),
        );
        self.pop = self.pop.step(&mut rng, &fitnesses);
        self.gens_in_stage += 1;
        if self.gens_in_stage >= self.cfg.gens_per_stage {
            self.stage = self.stage.next();
            self.gens_in_stage = 0;
        }
        (mean, best)
    }

    /// The elitist best genome produced so far (elites are sorted best-first).
    pub fn best_genome(&self) -> Vec<f32> {
        self.pop.genomes[0].clone()
    }

    /// The best genome's win rate against the hard bot over held-out seeds,
    /// played both spawn sides (mirror fairness, plan §5.7).
    pub fn hard_win_rate(&self, seeds: &[u64]) -> f32 {
        self.scripted_win_rates(seeds)[2]
    }

    /// The best genome's win rate against each scripted bot — easy, medium,
    /// hard — over `seeds`, played both spawn sides (mirror fairness). This is
    /// the permanent regression bar from plan §5.7: a crowned champion must
    /// beat all three ≥ 90%.
    pub fn scripted_win_rates(&self, seeds: &[u64]) -> [f32; 3] {
        let genome = self.best_genome();
        [
            mirror_win_rate(&genome, seeds, &self.match_config, || -> Box<dyn Bot> {
                Box::new(easy())
            }),
            mirror_win_rate(&genome, seeds, &self.match_config, || -> Box<dyn Bot> {
                Box::new(medium())
            }),
            mirror_win_rate(&genome, seeds, &self.match_config, || -> Box<dyn Bot> {
                Box::new(hard())
            }),
        ]
    }

    /// True if the best genome beats every scripted bot at ≥ `threshold`.
    pub fn beats_all_scripted(&self, seeds: &[u64], threshold: f32) -> bool {
        self.scripted_win_rates(seeds)
            .iter()
            .all(|&r| r >= threshold)
    }

    /// Run the whole schedule to completion (through `ScriptedGauntlet`).
    /// Returns the best genome's win rate vs hard over `held_out`.
    pub fn run_to_completion(&mut self, held_out: &[u64]) -> f32 {
        while self.stage != Stage::Done {
            self.run_generation();
        }
        self.hard_win_rate(held_out)
    }
}
