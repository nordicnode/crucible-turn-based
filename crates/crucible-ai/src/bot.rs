//! The [`Bot`] interface: any deterministic policy that plays one turn at a
//! time. Used by the scripted baselines and the learned commander alike — the
//! same interface the headless runner (and the live server) drives.

use crucible_sim::{Command, Game, Player};

/// A deterministic match policy.
///
/// # Calling convention
///
/// [`decide`](Bot::decide) is called **once per own activation**, at the start
/// of the activation, when `game.active == player` and the match is still
/// live. P0 and P1 activations are paired into one player-facing round by the
/// live/headless drivers. It returns the commands for this activation; they
/// are applied in order through the sim's normal validation + per-activation
/// action-budget path — a bot cannot
/// bypass either.
///
/// Implementors MUST append [`Command::EndTurn`] for `player` as the LAST
/// command of the returned vector: the activation only advances when
/// `EndTurn` is applied. The shared driver force-completes one missing
/// EndTurn, but policies should still emit it so replay logs describe intent.
///
/// *Baseline scripted bots may consult the full `Game` (they are oracle
/// baselines — see `CONTRACT.md` §5). The learned commander must not; its
/// feature extraction receives only a `FogView`.*
pub trait Bot: Send {
    /// Human-readable name.
    fn name(&self) -> &'static str;

    /// Produce this activation's commands for `player`, ending with
    /// [`Command::EndTurn`].
    fn decide(&mut self, game: &Game, player: Player) -> Vec<Command>;
}
