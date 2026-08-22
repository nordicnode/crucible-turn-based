//! Ghosts: frozen opponents reconstructed from a recorded match's input log.
//! A ghost replays one side's command stream (with entity-id remapping so its
//! build/attack references stay correct against a new opponent), plus the pool
//! policy that keeps recent/champion-beating human matches weighted higher.
//!
//! Pure — depends only on `crucible-sim`/`crucible-ai`. Immutability: the same
//! inputs always produce the same commands.

use std::collections::{HashMap, HashSet};

use crucible_ai::{Bot, DetailedOutcome, GenomeBot, MatchOutcome};
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
    /// Every own entity id the ghost has *ever* seen in the current fresh
    /// match, in creation order (id-sorted). Dead entities keep their slot,
    /// so `own_index` ranks map onto the k-th created entity even after
    /// casualties — a live-only snapshot would silently misalign.
    known_own: Vec<EntityId>,
    /// Same contract for the enemy side. Attacks remap their target through
    /// `enemy_index`; a live-only list is wrong the moment the enemy suffers
    /// any casualty (ranks shift by the deaths), so targets of later attacks
    /// silently got dropped once an earlier enemy unit died.
    known_enemy: Vec<EntityId>,
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
        // original match and unioning the ids of *every* entity the ghost
        // ever created — survivors and those later destroyed or sold. Ids are
        // allocated strictly ascending, so sorting the union yields creation
        // order, which is the order a fresh match creates the ghost's
        // entities too. (A survivors-only snapshot was wrong: commands
        // referencing units that died in the original match were dropped, and
        // survivors were mapped to the wrong creation rank whenever the two
        // matches' live sets differed.)
        let mut game = Game::new(Map::generate(replay.map_seed), replay.config.clone());
        let mut created: HashSet<EntityId> = HashSet::new();
        let mut owners: HashMap<EntityId, Player> = HashMap::new();
        {
            let mut capture = |g: &Game| {
                for u in &g.units {
                    created.insert(u.id);
                    owners.insert(u.id, u.owner);
                }
                for b in &g.buildings {
                    created.insert(b.id);
                    owners.insert(b.id, b.owner);
                }
            };
            // Re-run the full match exactly as `serialize::replay_to_game`
            // does, capturing after every command application. Commands
            // execute immediately in log order; `EndTurn` entries drive the
            // turn lifecycle.
            capture(&game);
            for cmd in &replay.commands {
                if game.is_over() {
                    break;
                }
                game.apply_commands(cmd.player, std::slice::from_ref(&cmd.command));
                capture(&game);
            }
        }
        let index_of = |side: Player| -> HashMap<EntityId, usize> {
            let mut ids: Vec<EntityId> = created
                .iter()
                .filter(|&&id| owners.get(&id) == Some(&side))
                .copied()
                .collect();
            ids.sort_unstable();
            ids.iter().enumerate().map(|(i, &id)| (id, i)).collect()
        };
        let own_index = index_of(player);
        let enemy_index = index_of(player.enemy());

        Ghost {
            map_seed: replay.map_seed,
            commands,
            own_index,
            enemy_index,
            known_own: Vec::new(),
            known_enemy: Vec::new(),
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
        self.known_own.clear();
        self.known_enemy.clear();
    }

    /// The ghost's own entities in creation order (sorted by id).
    fn current_entities(&self, game: &Game, player: Player) -> Vec<EntityId> {
        let mut ids: Vec<EntityId> = game
            .units
            .iter()
            .filter(|u| u.owner == player)
            .map(|u| u.id)
            .chain(
                game.buildings
                    .iter()
                    .filter(|b| b.owner == player)
                    .map(|b| b.id),
            )
            .collect();
        ids.sort_unstable();
        ids
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
        // Grow the all-time own creation list with any newly visible own
        // entities (ids ascend, so id-sorted order is creation order; dead
        // entities keep their slots). Commands referencing entities created
        // by the current turn's commands fire next turn at the earliest, so
        // the list is complete for everything the cursor can reach.
        let live_own = self.current_entities(game, player);
        for id in live_own.iter().copied() {
            if !self.known_own.contains(&id) {
                self.known_own.push(id);
            }
        }
        self.known_own.sort_unstable();
        let live_enemy = self.current_entities(game, player.enemy());
        for id in live_enemy.into_iter() {
            if !self.known_enemy.contains(&id) {
                self.known_enemy.push(id);
            }
        }
        self.known_enemy.sort_unstable();
        // Fire every recorded command whose turn has arrived (turns only move
        // forward; the ghost's cursor never rewinds within a match).
        while self.cursor < self.commands.len() {
            let tc = &self.commands[self.cursor];
            if tc.turn > game.turn {
                break;
            }
            if let Some(cmd) = self.remap(&tc.command, &self.known_own, &self.known_enemy, player) {
                out.push(cmd);
            }
            self.cursor += 1;
        }
        out
    }
}

/// Run one match: the ghost plays its recorded side (P0), a bot plays P1.
///
/// Both sides are polled once per own turn (the game is strictly
/// alternating); the ghost fires its recorded commands when the turn cursor
/// reaches them. The opponent bot keeps the normal per-turn cadence.
fn run_ghost_match(ghost: &mut Ghost, bot: &mut dyn Bot, config: &GameConfig) -> DetailedOutcome {
    let mut game = Game::new(Map::generate(ghost.map_seed()), config.clone());
    // Deadlock guard only: an unlimited config must not truncate the ghost's
    // recorded command stream.
    let max_turns = if config.timeout_turns > 0 {
        config.timeout_turns + 100
    } else {
        10_000
    };
    while !game.is_over() && game.turn <= max_turns {
        let active = game.active;
        if active == Player::P0 {
            let ghost_cmds = ghost.decide(&game, Player::P0);
            game.apply_commands(Player::P0, &ghost_cmds);
        } else {
            let bot_cmds = bot.decide(&game, Player::P1);
            game.apply_commands(Player::P1, &bot_cmds);
        }
        // Guarantee progress even if a policy forgets EndTurn.
        if game.active == active && !game.is_over() {
            game.end_turn();
        }
    }
    DetailedOutcome {
        outcome: MatchOutcome {
            winner: game.winner,
            reason: game.win_reason,
            duration_turns: game.turn,
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
    pub recency: u64,
}

/// The ghost pool: recent human matches weighted higher, champion-beaters
/// retained, trimmed to a maximum size.
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

    pub fn add(&mut self, id: u64, ghost: Ghost, beat_champion: bool) {
        self.entries.push(GhostEntry {
            id,
            ghost,
            beat_champion,
            recency: self.next_recency,
        });
        self.next_recency += 1;
        // Trim to `max_size`, evicting the oldest **non-beater** first: the
        // pool policy promises champion-beaters are retained, so a burst of
        // ordinary matches must not push out the one strategy that beat the
        // champion. Only when the pool is entirely beaters does the oldest
        // beater give way.
        while self.entries.len() > self.max_size {
            let evict = self
                .entries
                .iter()
                .position(|e| !e.beat_champion)
                .unwrap_or(0);
            self.entries.remove(evict);
        }
    }

    /// Champion-beating ghosts, most recent first.
    pub fn champion_beaters(&self) -> Vec<Ghost> {
        let mut v: Vec<&GhostEntry> = self.entries.iter().filter(|e| e.beat_champion).collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.recency));
        v.into_iter().map(|e| e.ghost.clone()).collect()
    }

    /// Sample up to `n` ghosts, weighted by recency (recent = higher weight),
    /// without replacement.
    pub fn sample(&self, rng: &mut crucible_sim::Rng, n: usize) -> Vec<Ghost> {
        let n = n.min(self.entries.len());
        if n == 0 {
            return Vec::new();
        }
        let mut idx: Vec<usize> = (0..self.entries.len()).collect();
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            let total: u64 = idx.iter().map(|&i| self.entries[i].recency + 1).sum();
            let mut pick = rng.below(total);
            let mut chosen = 0usize;
            for (pos, &i) in idx.iter().enumerate() {
                let w = self.entries[i].recency + 1;
                if pick < w {
                    chosen = pos;
                    break;
                }
                pick -= w;
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
        // Refinery must be ore-adjacent; find the nearest ore tile.
        let ore_tile = (0..crucible_sim::map::MAP_TILES)
            .map(crucible_sim::map::tile_coords)
            .filter(|&t| game.map.ore_at(t.0, t.1) > 0)
            .min_by_key(|&t| (t.0 as i32 - hq.0 as i32).abs() + (t.1 as i32 - hq.1 as i32).abs())
            .expect("map has ore");
        let place = [
            (ore_tile.0 as i32 + 1, ore_tile.1 as i32),
            (ore_tile.0 as i32 - 1, ore_tile.1 as i32),
            (ore_tile.0 as i32, ore_tile.1 as i32 + 1),
            (ore_tile.0 as i32, ore_tile.1 as i32 - 1),
        ]
        .into_iter()
        .map(|(x, y)| (x as u8, y as u8))
        .find(|&t| game.map.is_passable(t.0, t.1))
        .expect("free ore-adjacent tile");
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
        // Drive a hard-vs-hard match so the ghost side (P0) suffers
        // casualties, then verify the ghost can replay *every* recorded
        // command against a fresh, byte-identical match. The old survivor-only
        // mapping dropped commands whose target died in the original match and
        // mis-ranked survivors whenever the two matches' live sets differed.
        let cfg = GameConfig {
            timeout_turns: 90,
            ..GameConfig::default()
        };
        let seed = 2026u64;
        let mut game = Game::new(Map::generate(seed), cfg.clone());
        let mut replay = Replay::new(seed, cfg.clone());
        let mut p0 = crucible_ai::hard();
        let mut p1 = crucible_ai::hard();
        while !game.is_over() && game.turn <= 90 {
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
        while !fresh.is_over() && fresh.turn <= 90 {
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
