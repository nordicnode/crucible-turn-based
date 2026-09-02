// In-memory store for Crucible server: champions, replays, training stats, and saves.

export interface StoredChampion {
  id: number;
  genome_id: number;
  generation: number;
  crowned_at: number;
  dethroned_at: number | null;
  reigning: boolean;
  gauntlet_record: { wins: number; losses: number };
  era: string;
  elo: number;
}

export interface EloPoint {
  genome_id: number;
  elo: number;
  at: number;
}

export interface TrainingStat {
  generation: number;
  matches_run: number;
  pop_fitness_mean: number;
  pop_fitness_best: number;
  diversity: number;
  at: number;
}

export interface ReplaySummary {
  id: number;
  map_seed: number;
  p1_type: string;
  p2_type: string;
  result: string;
  duration_turns: number;
  duration_rounds?: number;
  created_at: number;
}

export interface StoredReplay {
  id: number;
  summary: ReplaySummary;
  data: string; // JSON string
}

class Store {
  startedAt = Date.now();
  champions: StoredChampion[] = [
    {
      id: 1,
      genome_id: 101,
      generation: 1,
      crowned_at: Date.now() - 14400000,
      dethroned_at: Date.now() - 10800000,
      reigning: false,
      gauntlet_record: { wins: 6, losses: 4 },
      era: "Genesis Vanguard",
      elo: 1020,
    },
    {
      id: 2,
      genome_id: 102,
      generation: 2,
      crowned_at: Date.now() - 10800000,
      dethroned_at: Date.now() - 7200000,
      reigning: false,
      gauntlet_record: { wins: 8, losses: 2 },
      era: "Armored Blitz",
      elo: 1145,
    },
    {
      id: 3,
      genome_id: 103,
      generation: 3,
      crowned_at: Date.now() - 7200000,
      dethroned_at: Date.now() - 3600000,
      reigning: false,
      gauntlet_record: { wins: 9, losses: 1 },
      era: "Combined Siege",
      elo: 1260,
    },
    {
      id: 4,
      genome_id: 104,
      generation: 4,
      crowned_at: Date.now() - 3600000,
      dethroned_at: null,
      reigning: true,
      gauntlet_record: { wins: 10, losses: 0 },
      era: "Neural Apex",
      elo: 1385,
    },
  ];

  eloHistory: EloPoint[] = [
    { genome_id: 101, elo: 1020, at: Date.now() - 14400000 },
    { genome_id: 102, elo: 1145, at: Date.now() - 10800000 },
    { genome_id: 103, elo: 1260, at: Date.now() - 7200000 },
    { genome_id: 104, elo: 1385, at: Date.now() - 3600000 },
  ];

  trainingStats: TrainingStat[] = [
    { generation: 1, matches_run: 50, pop_fitness_mean: 46.2, pop_fitness_best: 64.0, diversity: 0.84, at: Date.now() - 14400000 },
    { generation: 2, matches_run: 115, pop_fitness_mean: 58.7, pop_fitness_best: 76.5, diversity: 0.78, at: Date.now() - 10800000 },
    { generation: 3, matches_run: 180, pop_fitness_mean: 72.1, pop_fitness_best: 88.0, diversity: 0.71, at: Date.now() - 7200000 },
    { generation: 4, matches_run: 250, pop_fitness_mean: 84.5, pop_fitness_best: 96.2, diversity: 0.65, at: Date.now() - 3600000 },
  ];

  replays: StoredReplay[] = [
    {
      id: 1,
      summary: {
        id: 1,
        map_seed: 42,
        p1_type: "Human Commander",
        p2_type: "Neural Apex (Gen 4)",
        result: "P0 Victory (HQ Destroyed)",
        duration_turns: 28,
        duration_rounds: 14,
        created_at: Date.now() - 7200000,
      },
      data: JSON.stringify({
        meta: {
          map_seed: 42,
          passable: Array.from({ length: 16384 }, () => true),
          terrain: Array.from({ length: 16384 }, () => "Plains"),
          hq_tiles: [[24, 24], [104, 104]],
          ore: Array.from({ length: 16384 }, () => 0),
          crystal: Array.from({ length: 16384 }, () => 0),
          duration_turns: 28,
          duration_rounds: 14,
          winner: 0,
          win_reason: "HqDestroyed",
        },
        frames: [],
      }),
    },
  ];

  recentEvents = [
    { kind: "champion_crowned", payload: { generation: 4, elo: 1385, name: "Neural Apex" }, at: Date.now() - 3600000 },
    { kind: "generation_completed", payload: { generation: 4, fitness: 84.5 }, at: Date.now() - 1800000 },
  ];

  savedMatch: { opponent: string; game: unknown; replay: unknown } | null = null;

  getReigningChampion(): StoredChampion | null {
    return this.champions.find((c) => c.reigning) ?? null;
  }

  getMuseum(): StoredChampion[] {
    return this.champions;
  }

  getLineage(genomeId: number): Array<{ id: number; generation: number; elo: number }> {
    return this.champions
      .filter((c) => c.genome_id <= genomeId)
      .map((c) => ({ id: c.id, generation: c.generation, elo: c.elo }));
  }

  addReplay(summary: ReplaySummary, data: string): number {
    const id = this.replays.length + 1;
    summary.id = id;
    this.replays.unshift({ id, summary, data });
    return id;
  }

  getReplay(id: number): string | null {
    const r = this.replays.find((item) => item.id === id);
    return r ? r.data : null;
  }

  listReplays(): ReplaySummary[] {
    return this.replays.map((r) => r.summary);
  }
}

export const store = new Store();
