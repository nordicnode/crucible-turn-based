//! The [`Bot`] interface: any deterministic policy that plays one turn at a
//! time. Used by the scripted baselines and the learned commander alike — the
//! same interface the headless runner (and the live server) drives.

use crucible_sim::{Command, Game, Player};

/// A deterministic match policy.
///
/// # Calling convention
///
/// [`decide`](Bot::decide) is called **once per own turn**, at the start of
/// the turn, when `game.active == player` and the match is still live. It
/// returns the commands for this turn; they are applied in order through the
/// sim's normal validation + per-turn action-budget path — a bot cannot
/// bypass either.
///
/// Implementors MUST append [`Command::EndTurn`] for `player` as the LAST
/// command of the returned vector: the turn only advances when `EndTurn` is
/// applied, so a bot that omits it stalls the match (the headless runner's
/// deadlock guard eventually cuts such a match short).
///
/// *Baseline scripted bots may consult the full `Game` (they are oracle
/// baselines — see `CONTRACT.md` §5). The learned commander must not; its
/// feature extraction receives only a `FogView`.*
pub trait Bot: Send {
    /// Human-readable name.
    fn name(&self) -> &'static str;

    /// Produce this turn's commands for `player`, ending with
    /// [`Command::EndTurn`].
    fn decide(&mut self, game: &Game, player: Player) -> Vec<Command>;
}
