//! The learned commander as a [`Bot`]: a genome evaluated through the feature
//! extractor + network + decision layer. It observes only [`FeatureInput`],
//! so its behavior is fog-legal by construction.

use crucible_sim::{Command, Game, Player};

use crate::bot::Bot;
use crate::decision::decide;
use crate::features::{extract_single, FeatureInput, HISTORY_TICKS};

/// A genome playing as a commander.
pub struct GenomeBot {
    pub genome: Vec<f32>,
    /// The history embedding (plan §5.2): this commander's previous command
    /// ticks' feature vectors, oldest first, at most `HISTORY_TICKS - 1`.
    /// Empty at the start of a match — the extractor zero-pads it, so the
    /// network sees an all-zero "start of match" previous observation.
    history: Vec<Vec<f32>>,
}

impl GenomeBot {
    pub fn new(genome: Vec<f32>) -> Self {
        GenomeBot {
            genome,
            history: Vec::new(),
        }
    }
}

impl Bot for GenomeBot {
    fn name(&self) -> &'static str {
        "genome"
    }

    fn decide(&mut self, game: &Game, player: Player) -> Vec<Command> {
        let input = FeatureInput::from_game(game, player);
        let mut cmds = decide(game, player, &self.genome, &input, &self.history);
        // Record this turn's observation for the next decision.
        self.history.push(extract_single(&input));
        while self.history.len() >= HISTORY_TICKS {
            self.history.remove(0);
        }
        // The Bot contract: EndTurn is always the last command, or the turn
        // never advances and the match deadlocks.
        cmds.push(Command::EndTurn { player });
        cmds
    }
}
