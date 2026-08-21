//! Sweep curriculum master seeds after a network change to find a seed that
//! converges to >= 90% vs hard (and ideally vs all three scripted bots).
use crucible_evo::{Curriculum, CurriculumConfig, EsParams, Stage};

fn ci_config(master_seed: u64) -> CurriculumConfig {
    CurriculumConfig {
        es: EsParams {
            population_size: 16,
            mu: 4,
            sigma: 0.05,
            ..EsParams::default()
        },
        gens_per_stage: 4,
        seeds_per_generation: 3,
        match_timeout_turns: 20,
        shaping_turns: 30,
        master_seed,
    }
}

fn main() {
    let held_out: Vec<u64> = (1000..1032).collect();
    for seed in [0u64, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10] {
        let mut c = Curriculum::init(ci_config(seed));
        let mut generations = 0u32;
        while c.stage != Stage::Done {
            c.run_generation();
            generations += 1;
        }
        let rates = c.scripted_win_rates(&held_out);
        println!(
            "seed {seed}: {generations} gens -> easy {:.1}% medium {:.1}% hard {:.1}%",
            rates[0] * 100.0,
            rates[1] * 100.0,
            rates[2] * 100.0
        );
    }
}
