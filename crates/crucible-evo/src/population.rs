//! The (μ+λ) evolution strategy: a population of flat genome vectors, elitist
//! retention of the top μ, and λ offspring via Gaussian mutation. Pure — the
//! caller supplies fitnesses and the PRNG.

use crucible_ai::mutate;
use crucible_sim::Rng;

#[derive(Clone, Copy, Debug)]
pub struct EsParams {
    pub population_size: usize,
    pub mu: usize,
    pub sigma: f32,
    pub sigma_min: f32,
    /// Multiplicative sigma decay per generation (annealing).
    pub sigma_decay: f32,
    /// Probability of a 3σ macromutation per weight.
    pub macro_rate: f32,
}

impl Default for EsParams {
    fn default() -> Self {
        EsParams {
            population_size: 64,
            mu: 16,
            sigma: 0.02,
            sigma_min: 0.005,
            sigma_decay: 0.995,
            macro_rate: 0.10,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Population {
    pub genomes: Vec<Vec<f32>>,
    pub generation: u32,
    pub sigma: f32,
    pub params: EsParams,
}

impl Population {
    pub fn init(rng: &mut Rng, params: EsParams) -> Self {
        let genomes = (0..params.population_size)
            .map(|_| crucible_ai::init(rng))
            .collect();
        Population {
            genomes,
            generation: 0,
            sigma: params.sigma,
            params,
        }
    }

    /// Produce the next generation from per-genome fitnesses (higher is better).
    pub fn step(&self, rng: &mut Rng, fitnesses: &[f32]) -> Population {
        self.step_with_parents(rng, fitnesses).0
    }

    /// Like [`Population::step`], but also returns the parent *index* of each
    /// genome in the new population (elites point at themselves). Enables
    /// lineage records (the caller maps indices to persistent genome ids).
    pub fn step_with_parents(&self, rng: &mut Rng, fitnesses: &[f32]) -> (Population, Vec<usize>) {
        assert_eq!(fitnesses.len(), self.genomes.len());
        let mu = self.params.mu.min(self.genomes.len());

        // Rank by fitness (ties broken by index for determinism).
        let mut order: Vec<usize> = (0..self.genomes.len()).collect();
        order.sort_by(|&a, &b| {
            fitnesses[b]
                .partial_cmp(&fitnesses[a])
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.cmp(&b))
        });

        let mut next: Vec<Vec<f32>> = Vec::with_capacity(self.params.population_size);
        let mut parents: Vec<usize> = Vec::with_capacity(self.params.population_size);
        for &i in &order[..mu] {
            next.push(self.genomes[i].clone()); // elites survive verbatim
            parents.push(i);
        }
        while next.len() < self.params.population_size {
            let parent_idx = order[rng.below(mu as u64) as usize];
            let mut child = self.genomes[parent_idx].clone();
            mutate(rng, &mut child, self.sigma, self.params.macro_rate);
            next.push(child);
            parents.push(parent_idx);
        }

        (
            Population {
                genomes: next,
                generation: self.generation + 1,
                sigma: (self.sigma * self.params.sigma_decay).max(self.params.sigma_min),
                params: self.params,
            },
            parents,
        )
    }

    /// Index of the best genome in this generation.
    pub fn best_index(&self, fitnesses: &[f32]) -> usize {
        let mut best = 0;
        for (i, &f) in fitnesses.iter().enumerate() {
            if f > fitnesses[best] {
                best = i;
            }
        }
        best
    }

    /// Population diversity: mean L2 distance of each genome from the
    /// population centroid. Higher = more diverse.
    pub fn diversity(&self) -> f32 {
        let n = self.genomes.len();
        if n == 0 {
            return 0.0;
        }
        let dim = self.genomes[0].len();
        let mut centroid = vec![0.0f32; dim];
        for g in &self.genomes {
            for (i, &w) in g.iter().enumerate() {
                centroid[i] += w;
            }
        }
        for c in &mut centroid {
            *c /= n as f32;
        }
        let mut total = 0.0f32;
        for g in &self.genomes {
            let mut d2 = 0.0f32;
            for (i, &w) in g.iter().enumerate() {
                let diff = w - centroid[i];
                d2 += diff * diff;
            }
            total += d2.sqrt();
        }
        total / n as f32
    }

    /// Mean and best fitness across the current generation.
    pub fn fitness_stats(fitnesses: &[f32]) -> (f32, f32) {
        if fitnesses.is_empty() {
            return (0.0, 0.0);
        }
        let mean = fitnesses.iter().sum::<f32>() / fitnesses.len() as f32;
        let best = fitnesses.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        (mean, best)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_keeps_size_and_improves_best() {
        let mut rng = Rng::from_seed(10);
        let params = EsParams {
            population_size: 8,
            mu: 2,
            ..EsParams::default()
        };
        let mut pop = Population::init(&mut rng, params);

        // Fitness = sum of weights (maximization); evolve for a few gens.
        let mut fitnesses: Vec<f32> = pop.genomes.iter().map(|g| g.iter().sum()).collect();
        let start_best = fitnesses.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));

        for _ in 0..5 {
            pop = pop.step(&mut rng, &fitnesses);
            fitnesses = pop.genomes.iter().map(|g| g.iter().sum()).collect();
        }
        let end_best = fitnesses.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));

        assert_eq!(pop.genomes.len(), 8);
        assert_eq!(pop.generation, 5);
        assert!(
            end_best >= start_best,
            "elitism must never regress the best"
        );
    }
}
