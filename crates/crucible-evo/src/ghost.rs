//! Ghosts: frozen opponents reconstructed from a recorded match's input log.
//! A ghost replays one side's command stream (with entity-id remapping so its
//! build/attack references stay correct against a new opponent), plus the pool
//! policy that keeps recent, difficult, and champion-beating human matches
//! weighted higher.
//!
//! Pure — depends only on `crucible-sim`/`crucible-ai`. Immutability: the same
//! inputs always produce the same commands.

use std::collections::HashMap;

use crucible_ai::{drive_bot_turn, Bot, DetailedOutcome, GenomeBot, MatchOutcome};
use crucible_sim::{Command, EntityId, Game, GameConfig, Map, Player, Replay, TimedCommand};

use crate::fitness::shaped_fitness;

/// A frozen policy reconstructed from a replay. Deterministic and immutable.
#[derive(Clone, Debug)]
pub struct Ghost {
    map_seed: u64,
    commands: Vec<TimedCommand>,
    /// Original own entity id -> creation-order index among the ghost's
    /// own entities (units + buildings, id-sorted).
    own_index: HashMap<EntityId, usize>,
    /// Original enemy entity id -> creation-order index among the enemy's
    /// entities. Attacks reference enemy units/buildings, which the fresh
    /// opponent recreates under different ids; remapping by the enemy's own
    /// creation rank keeps the reference coherent in a byte-identical match
    /// and drops it gracefully (a skipped attack) against a divergent one.
    enemy_index: HashMap<EntityId, usize>,
    cursor: usize,
}

impl Ghost {
    /// Build a ghost that replays `player`'s command stream from `replay` on
    /// the replay's own map. Entity references are remapped by creation order
    /// so the ghost stays coherent against a different opponent.
    pub fn from_replay(replay: &Replay, player: Player) -> Ghost {
        let commands: Vec<TimedCommand> = replay
            .commands
            .iter()
            .filter(|tc| tc.player == player)
            .cloned()
            .collect();
        // Commands are recorded in issuance order (seq is monotonic), so the
        // log order IS the replay order — no re-sort needed.

        // Reconstruct the ghost's entity creation order by re-running the
        // original input log. The sim keeps an all-time allocator history,
        // so death/sell cannot make a later command shift onto the wrong
        // creation rank.
        let mut game = Game::new(Map::generate(replay.map_seed), replay.config.clone());
        for cmd in &replay.commands {
            if game.is_over() {
                break;
            }
            game.apply_commands(cmd.player, std::slice::from_ref(&cmd.command));
        }
        let index_of = |side: Player| -> HashMap<EntityId, usize> {
            game.entity_ids_in_creation_order(side)
                .into_iter()
                .enumerate()
                .map(|(i, id)| (id, i))
                .collect()
        };
        let own_index = index_of(player);
        let enemy_index = index_of(player.enemy());

        Ghost {
            map_seed: replay.map_seed,
            commands,
            own_index,
            enemy_index,
            cursor: 0,
        }
    }

    pub fn map_seed(&self) -> u64 {
        self.map_seed
    }

    pub fn command_count(&self) -> usize {
        self.commands.len()
    }

    pub fn reset(&mut self) {
        self.cursor = 0;
    }

    /// The ghost's entities in creation order, including dead/sold ids.
    fn current_entities(&self, game: &Game, player: Player) -> Vec<EntityId> {
        game.entity_ids_in_creation_order(player)
    }

    fn remap(
        &self,
        cmd: &Command,
        own: &[EntityId],
        enemy: &[EntityId],
        player: Player,
    ) -> Option<Command> {
        use Command::*;
        let at_own = |id: &EntityId| self.own_index.get(id).and_then(|&k| own.get(k).copied());
        let at_enemy = |id: &EntityId| {
            self.enemy_index
                .get(id)
                .and_then(|&k| enemy.get(k).copied())
        };
        match cmd {
            PlaceBuilding { btype, tile, .. } => Some(PlaceBuilding {
                player,
                btype: *btype,
                tile: *tile,
            }),
            TrainUnit {
                building, utype, ..
            } => Some(TrainUnit {
                player,
                building: at_own(building)?,
                utype: *utype,
            }),
            MoveGroup {
                units, waypoint, ..
            } => {
                let mut new_units = Vec::with_capacity(units.len());
                for id in units {
                    new_units.push(at_own(id)?);
                }
                Some(MoveGroup {
                    player,
                    units: new_units,
                    waypoint: *waypoint,
                })
            }
            ClearMove { units, .. } => {
                let mut new_units = Vec::with_capacity(units.len());
                for id in units {
                    new_units.push(at_own(id)?);
                }
                Some(ClearMove {
                    player,
                    units: new_units,
                })
            }
            Attack { units, target, .. } => {
                let mut new_units = Vec::with_capacity(units.len());
                for id in units {
                    new_units.push(at_own(id)?);
                }
                Some(Attack {
                    player,
                    units: new_units,
                    target: at_enemy(target)?,
                })
            }
            StartResearch { tech, .. } => Some(StartResearch {
                player,
                tech: *tech,
            }),
            Sell { building, .. } => Some(Sell {
                player,
                building: at_own(building)?,
            }),
            Repair { building, .. } => Some(Repair {
                player,
                building: at_own(building)?,
            }),
            EndTurn { .. } => Some(EndTurn { player }),
        }
    }
}

impl Bot for Ghost {
    fn name(&self) -> &'static str {
        "ghost"
    }

    fn decide(&mut self, game: &Game, player: Player) -> Vec<Command> {
        let mut out = Vec::new();
        // Use the sim's all-time creation history rather than the live
        // entity vectors. Dead entities retain their slots, and the same
        // creation rank is therefore available even after casualties.
        let own = self.current_entities(game, player);
        let enemy = self.current_entities(game, player.enemy());
        // Fire every recorded command whose turn has arrived (turns only move
        // forward; the ghost's cursor never rewinds within a match).
        while self.cursor < self.commands.len() {
            let tc = &self.commands[self.cursor];
            if tc.turn > game.turn {
                break;
            }
            if let Some(cmd) = self.remap(&tc.command, &own, &enemy, player) {
                out.push(cmd);
            }
            self.cursor += 1;
        }
        out
    }
}

/// Run one match: the ghost plays its recorded side (P0), a bot plays P1.
///
/// Both sides are polled once per own activation; the ghost fires its
/// recorded commands when the turn cursor reaches them. The opponent bot
/// keeps the normal per-activation cadence, and the shared driver guarantees
/// that a missing EndTurn advances exactly once.
fn run_ghost_match(ghost: &mut Ghost, bot: &mut dyn Bot, config: &GameConfig) -> DetailedOutcome {
    let mut game = Game::new(Map::generate(ghost.map_seed()), config.clone());
    let max_turns = if config.timeout_turns > 0 {
        config.timeout_turns + 100
    } else {
        10_000
    };
    while !game.is_over() && game.turn <= max_turns {
        let active = game.active;
        if active == Player::P0 {
            drive_bot_turn(&mut game, active, ghost);
        } else {
            drive_bot_turn(&mut game, active, bot);
        }
    }
    DetailedOutcome {
        outcome: MatchOutcome {
            winner: game.winner,
            reason: game.win_reason,
            duration_turns: game.turn,
            duration_rounds: game.round,
        },
        p0_value: game.remaining_value(Player::P0),
        p1_value: game.remaining_value(Player::P1),
    }
}

/// Mean shaped fitness of `genome` against a set of ghosts. Each ghost plays
/// its recorded side (P0) on its own map; the genome plays P1.
pub fn ghost_fitness(genome: &[f32], ghosts: &[Ghost], config: &GameConfig) -> f32 {
    let mut total = 0.0f32;
    for ghost in ghosts {
        let mut g = ghost.clone(); // fresh cursor
        let mut genome_bot = GenomeBot::new(genome.to_vec());
        let d = run_ghost_match(&mut g, &mut genome_bot, config);
        total += shaped_fitness(&d, Player::P1);
    }
    total / ghosts.len().max(1) as f32
}

/// A ghost in the pool plus its metadata.
#[derive(Clone, Debug)]
pub struct GhostEntry {
    pub id: u64,
    pub ghost: Ghost,
    pub beat_champion: bool,
    /// Deterministic difficulty/teaching priority derived from the stored
    /// human result and opponent. Higher values are sampled and retained more
    /// often than routine games.
    pub priority: u32,
    pub recency: u64,
}

/// The ghost pool: recent and difficult human matches weighted higher,
/// champion-beaters retained, trimmed to a maximum size.
#[derive(Clone, Debug, Default)]
pub struct GhostPool {
    entries: Vec<GhostEntry>,
    max_size: usize,
    next_recency: u64,
}

impl GhostPool {
    pub fn new(max_size: usize) -> Self {
        GhostPool {
            entries: Vec::new(),
            max_size,
            next_recency: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Add a routine-priority ghost. Kept for callers that do not have match
    /// metadata; trainer ingestion uses [`Self::add_scored`].
    pub fn add(&mut self, id: u64, ghost: Ghost, beat_champion: bool) {
        self.add_scored(id, ghost, beat_champion, 1);
    }

    /// Add or refresh one ghost with a deterministic teaching priority. Match
    /// ids are unique, so repeated startup/refresh loads are idempotent.
    pub fn add_scored(&mut self, id: u64, ghost: Ghost, beat_champion: bool, priority: u32) {
        if self.max_size == 0 {
            return;
        }
        if let Some(existing) = self.entries.iter_mut().find(|entry| entry.id == id) {
            existing.beat_champion |= beat_champion;
            existing.priority = existing.priority.max(priority);
            return;
        }
        self.entries.push(GhostEntry {
            id,
            ghost,
            beat_champion,
            priority: priority.max(1),
            recency: self.next_recency,
        });
        self.next_recency += 1;
        // Trim to `max_size`, evicting the lowest-priority **non-beater**;
        // ties evict the oldest. Champion-beaters are retained against a
        // flood of ordinary games, while a full beater pool still ages out.
        while self.entries.len() > self.max_size {
            let evict = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| !entry.beat_champion)
                .min_by_key(|(_, entry)| (entry.priority, entry.recency))
                .map(|(index, _)| index)
                .unwrap_or_else(|| {
                    self.entries
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, entry)| (entry.priority, entry.recency))
                        .map(|(index, _)| index)
                        .unwrap_or(0)
                });
            self.entries.remove(evict);
        }
    }

    /// Champion-beating ghosts, most recent first.
    pub fn champion_beaters(&self) -> Vec<Ghost> {
        let mut v: Vec<&GhostEntry> = self.entries.iter().filter(|e| e.beat_champion).collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.recency));
        v.into_iter().map(|e| e.ghost.clone()).collect()
    }

    /// Sample up to `n` ghosts, weighted by priority and recency, without
    /// replacement.
    pub fn sample(&self, rng: &mut crucible_sim::Rng, n: usize) -> Vec<Ghost> {
        self.sample_where(rng, n, |_| true)
    }

    /// Sample only ordinary ghosts. This lets callers reserve the explicit
    /// champion-beater prefix without returning a duplicate entry.
    pub fn sample_non_beaters(&self, rng: &mut crucible_sim::Rng, n: usize) -> Vec<Ghost> {
        self.sample_where(rng, n, |entry| !entry.beat_champion)
    }

    /// Number of retained champion-beating entries.
    pub fn champion_beater_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.beat_champion)
            .count()
    }

    /// Number of entries at or above a teaching-priority threshold.
    pub fn priority_count(&self, threshold: u32) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.priority >= threshold)
            .count()
    }

    fn sample_where(
        &self,
        rng: &mut crucible_sim::Rng,
        n: usize,
        include: impl Fn(&GhostEntry) -> bool,
    ) -> Vec<Ghost> {
        let mut idx: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| include(entry))
            .map(|(index, _)| index)
            .collect();
        let n = n.min(idx.len());
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            let total: u64 = idx
                .iter()
                .map(|&i| (self.entries[i].priority as u64) * 16 + self.entries[i].recency + 1)
                .sum();
            let mut pick = rng.below(total.max(1));
            let mut chosen = 0usize;
            for (pos, &i) in idx.iter().enumerate() {
                let weight = (self.entries[i].priority as u64) * 16 + self.entries[i].recency + 1;
                if pick < weight {
                    chosen = pos;
                    break;
                }
                pick -= weight;
            }
            out.push(self.entries[idx[chosen]].ghost.clone());
            idx.remove(chosen);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_replay(seed: u64) -> Replay {
        // Record a short human-vs-noop match (the "human" is the easy bot).
        let cfg = GameConfig {
            timeout_turns: 40,
            ..GameConfig::default()
        };
        let mut replay = Replay::new(seed, cfg.clone());
        let mut game = crucible_sim::Game::new(crucible_sim::Map::generate(seed), cfg);
        let mut bot = crucible_ai::easy();
        while !game.is_over() && game.turn <= 40 {
            if game.active == Player::P0 {
                let cmds = bot.decide(&game, Player::P0);
                for c in &cmds {
                    replay.record(game.turn, Player::P0, c.clone());
                }
                game.apply_commands(Player::P0, &cmds);
            }
            game.end_turn();
        }
        replay
    }

    #[test]
    fn ghost_is_immutable_and_deterministic() {
        let replay = sample_replay(11);
        let ghost = Ghost::from_replay(&replay, Player::P0);
        assert_eq!(ghost.command_count(), replay.commands.len());

        // Replaying on the same map reproduces the same command stream.
        let mut g1 = ghost.clone();
        let mut g2 = ghost.clone();
        let mut game = crucible_sim::Game::new(
            crucible_sim::Map::generate(replay.map_seed),
            replay.config.clone(),
        );
        // Step to a couple of turns and compare outputs.
        while game.turn < 12 && !game.is_over() {
            let a = g1.decide(&game, Player::P0);
            let b = g2.decide(&game, Player::P0);
            assert_eq!(a, b, "ghost diverged at turn {}", game.turn);
            game.end_turn();
        }
    }

    #[test]
    fn pool_keeps_beaters_and_trims_oldest() {
        let replay = sample_replay(3);
        let mut pool = GhostPool::new(3);
        let g = || Ghost::from_replay(&replay, Player::P0);
        pool.add(1, g(), false);
        pool.add(2, g(), true); // beat the champion
        pool.add(3, g(), false);
        pool.add(4, g(), false); // pushes out entry 1 (max 3)

        assert_eq!(pool.len(), 3);
        assert_eq!(pool.champion_beaters().len(), 1);

        // Sampling is deterministic and never exceeds the requested count.
        let mut rng = crucible_sim::Rng::from_seed(7);
        let a = pool.sample(&mut rng, 2);
        let mut rng2 = crucible_sim::Rng::from_seed(7);
        let b = pool.sample(&mut rng2, 2);
        assert_eq!(a.len(), 2);
        assert!(a[0].command_count() > 0);
        // Same seed ⇒ same sample.
        assert_eq!(a.len(), b.len());
        assert_eq!(a[0].map_seed(), b[0].map_seed());
    }

    #[test]
    fn champion_beaters_survive_recency_eviction() {
        // The pool must retain champion-beaters even when ordinary matches
        // flood in afterwards; only an all-beater pool evicts its oldest.
        let replay = sample_replay(4);
        let g = || Ghost::from_replay(&replay, Player::P0);
        let mut pool = GhostPool::new(2);
        pool.add(1, g(), true); // champion-beater
        pool.add(2, g(), false);
        pool.add(3, g(), false); // ordinary match: evicts entry 2, not the beater
        assert_eq!(pool.len(), 2);
        assert_eq!(pool.champion_beaters().len(), 1);

        // A flood of ordinary matches still cannot evict the beater.
        for id in 4..=50u64 {
            pool.add(id, g(), false);
        }
        assert_eq!(pool.len(), 2);
        assert_eq!(pool.champion_beaters().len(), 1);
        assert_eq!(
            pool.champion_beaters()[0].command_count(),
            g().command_count()
        );

        // An all-beater pool trims its oldest beater (size still respected).
        let mut all = GhostPool::new(2);
        all.add(1, g(), true);
        all.add(2, g(), true);
        all.add(3, g(), true);
        assert_eq!(all.len(), 2);
        assert_eq!(all.champion_beaters().len(), 2);
    }

    #[test]
    fn priority_sampling_is_deterministic_and_idempotent() {
        let replay = sample_replay(6);
        let g = || Ghost::from_replay(&replay, Player::P0);
        let mut pool = GhostPool::new(3);
        pool.add_scored(1, g(), false, 1);
        pool.add_scored(2, g(), false, 8);
        pool.add_scored(3, g(), true, 12);
        // Re-reading an existing row updates metadata rather than duplicating
        // it, which is what makes trainer startup/refresh safe to repeat.
        pool.add_scored(2, g(), false, 10);
        assert_eq!(pool.len(), 3);
        assert_eq!(pool.champion_beater_count(), 1);

        let mut a_rng = crucible_sim::Rng::from_seed(19);
        let mut b_rng = crucible_sim::Rng::from_seed(19);
        let a = pool.sample_non_beaters(&mut a_rng, 2);
        let b = pool.sample_non_beaters(&mut b_rng, 2);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].map_seed(), b[0].map_seed());
    }

    #[test]
    fn ghost_fitness_is_deterministic() {
        let replay = sample_replay(5);
        let ghost = Ghost::from_replay(&replay, Player::P0);
        let genome = crucible_ai::init(&mut crucible_sim::Rng::from_seed(9));
        let cfg = GameConfig {
            timeout_turns: 30,
            ..GameConfig::default()
        };
        let a = ghost_fitness(&genome, std::slice::from_ref(&ghost), &cfg);
        let b = ghost_fitness(&genome, &[ghost], &cfg);
        assert_eq!(a, b);
        assert!(a.is_finite());
    }

    struct NoopBot;
    impl crucible_ai::Bot for NoopBot {
        fn name(&self) -> &'static str {
            "noop"
        }
        fn decide(&mut self, _g: &Game, _p: Player) -> Vec<Command> {
            Vec::new()
        }
    }

    #[test]
    fn ghost_replays_commands_at_recorded_turn() {
        // A command recorded at turn N must fire exactly when the ghost's
        // cursor reaches that turn — not earlier, not dropped.
        let cfg = GameConfig {
            timeout_turns: 40,
            ..GameConfig::default()
        };
        let seed = 42u64;
        let mut replay = Replay::new(seed, cfg.clone());
        let mut game = Game::new(Map::generate(seed), cfg.clone());
        // Advance to turn 3 (P0 ends, P1 ends, P0 ends → turn 4 is P0's).
        while game.turn < 4 && !game.is_over() {
            game.end_turn();
        }
        let hq = game.hq(Player::P0).unwrap().tile;
        // A generic refinery claims the nearest live deposit tile itself.
        let place = (0..crucible_sim::map::MAP_TILES)
            .map(crucible_sim::map::tile_coords)
            .filter(|&t| game.map.resource_amount_at(t.0, t.1) > 0)
            .min_by_key(|&t| {
                (
                    (t.0 as i32 - hq.0 as i32).abs() + (t.1 as i32 - hq.1 as i32).abs(),
                    crucible_sim::map::tile_index(t.0, t.1),
                )
            })
            .expect("map has a resource tile");
        let cmd = Command::PlaceBuilding {
            player: Player::P0,
            btype: crucible_sim::BuildingType::Refinery,
            tile: place,
        };
        replay.record(4, Player::P0, cmd.clone());
        replay.record(4, Player::P0, Command::EndTurn { player: Player::P0 });
        game.apply_commands(Player::P0, &[cmd]);

        // Drive the ghost with the same per-turn polling run_ghost_match uses:
        // poll only the active player, and only force-end the turn if the
        // commands (which end with EndTurn) didn't already advance it.
        let mut ghost = Ghost::from_replay(&replay, Player::P0);
        let mut game = Game::new(Map::generate(seed), cfg.clone());
        while game.turn <= 6 && !game.is_over() {
            let active = game.active;
            if active == Player::P0 {
                let cmds = ghost.decide(&game, Player::P0);
                game.apply_commands(Player::P0, &cmds);
            }
            if game.active == active && !game.is_over() {
                game.end_turn();
            }
        }
        assert!(
            game.buildings
                .iter()
                .any(|b| b.owner == Player::P0 && b.btype == crucible_sim::BuildingType::Refinery),
            "ghost dropped the recorded refinery command"
        );

        // The production runner applies the same polling and completes against
        // a noop opponent; P0 wins the timeout by value (the refinery tips the
        // otherwise-tied bases).
        let mut ghost = Ghost::from_replay(&replay, Player::P0);
        let outcome = run_ghost_match(&mut ghost, &mut NoopBot, &cfg);
        assert!(outcome.outcome.duration_turns > 4);
        assert_eq!(outcome.outcome.winner, Some(Player::P0));
        assert!(outcome.p0_value > outcome.p1_value);
    }

    #[test]
    fn ghost_maps_entities_by_creation_order_not_survivors() {
        // Drive a hard-vs-hard match long enough for the ghost side (P0) to
        // suffer casualties, then verify the ghost can replay *every* recorded
        // command against a fresh, byte-identical match. The old survivor-only
        // mapping dropped commands whose target died in the original match and
        // mis-ranked survivors whenever the two matches' live sets differed.
        let cfg = GameConfig {
            timeout_turns: 130,
            ..GameConfig::default()
        };
        let seed = 2026u64;
        let mut game = Game::new(Map::generate(seed), cfg.clone());
        let mut replay = Replay::new(seed, cfg.clone());
        let mut p0 = crucible_ai::hard();
        let mut p1 = crucible_ai::hard();
        while !game.is_over() && game.turn <= 130 {
            let active = game.active;
            let cmds = if active == Player::P0 {
                p0.decide(&game, active)
            } else {
                p1.decide(&game, active)
            };
            for c in &cmds {
                replay.record(game.turn, active, c.clone());
            }
            game.apply_commands(active, &cmds);
            // Bots append EndTurn; only force-end if the turn didn't advance.
            if game.active == active && !game.is_over() {
                game.end_turn();
            }
        }

        // The scenario must include P0 casualties for this to be a regression
        // test of the survivor-mapping bug.
        let p0_deaths = game
            .events
            .iter()
            .filter(|e| {
                matches!(
                    &e.kind,
                    crucible_sim::EventKind::UnitDied {
                        owner: Player::P0,
                        ..
                    }
                )
            })
            .count();
        assert!(p0_deaths > 0, "test scenario must include P0 casualties");

        let ghost = Ghost::from_replay(&replay, Player::P0);

        // Fresh match with the same seed and the same opponent is byte-identical
        // to the original, so every recorded command must still be emitted.
        let mut g = ghost.clone();
        let mut fresh = Game::new(Map::generate(replay.map_seed), replay.config.clone());
        let mut opp = crucible_ai::hard();
        let mut emitted = 0usize;
        while !fresh.is_over() && fresh.turn <= 130 {
            let active = fresh.active;
            if active == Player::P0 {
                let cmds = g.decide(&fresh, Player::P0);
                emitted += cmds.len();
                fresh.apply_commands(Player::P0, &cmds);
            } else {
                let b = opp.decide(&fresh, Player::P1);
                fresh.apply_commands(Player::P1, &b);
            }
            if fresh.active == active && !fresh.is_over() {
                fresh.end_turn();
            }
        }
        assert_eq!(
            emitted,
            ghost.command_count(),
            "ghost dropped commands during entity remapping"
        );
    }
}
