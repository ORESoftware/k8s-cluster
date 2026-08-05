//! Deterministic, transport-agnostic soccer match lifecycle.
//!
//! The high-dimensional simulation, policy, and learning code may evolve independently,
//! but every consumer needs one small, stable event envelope for kickoff, possession,
//! scoring, restart, clock, and finish behavior.  This module is deliberately integer-
//! and enum-only so model traces can refine production code without hiding floating-point
//! or scheduler nondeterminism in an adapter.

use serde::{Deserialize, Serialize};

/// Finite bounds used by the checked formal profile.
pub const FORMAL_MAX_TICKS: u32 = 4;
pub const FORMAL_SCORE_CAP: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchPhase {
    PreKickoff,
    InPlay,
    Stoppage,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Home,
    Away,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Possession {
    None,
    Home,
    Away,
}

impl From<Side> for Possession {
    fn from(side: Side) -> Self {
        match side {
            Side::Home => Self::Home,
            Side::Away => Self::Away,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchAction {
    Start,
    Tick,
    HomePossession,
    AwayPossession,
    LooseBall,
    HomeGoal,
    AwayGoal,
    Restart,
    Finish,
}

/// Stable projection shared with the JSON-lines/ITF formal adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MatchProjection {
    pub phase: MatchPhase,
    pub tick: u32,
    pub home_score: u8,
    pub away_score: u8,
    pub possession: Possession,
    pub restart_side: Side,
}

/// Production lifecycle state. Continuous physics and policy state intentionally live
/// outside this type and may only feed it canonical lifecycle events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoccerMatch {
    state: MatchProjection,
    max_ticks: u32,
    score_cap: u8,
}

impl SoccerMatch {
    pub fn with_limits(max_ticks: u32, score_cap: u8) -> Result<Self, &'static str> {
        if max_ticks == 0 {
            return Err("max_ticks must be greater than zero");
        }
        if score_cap == 0 {
            return Err("score_cap must be greater than zero");
        }
        Ok(Self {
            state: MatchProjection {
                phase: MatchPhase::PreKickoff,
                tick: 0,
                home_score: 0,
                away_score: 0,
                possession: Possession::None,
                restart_side: Side::Home,
            },
            max_ticks,
            score_cap,
        })
    }

    /// Fixture whose bounds exactly match `formal/fm.toml`.
    pub fn formal_fixture() -> Self {
        Self::with_limits(FORMAL_MAX_TICKS, FORMAL_SCORE_CAP)
            .expect("formal lifecycle bounds are valid")
    }

    pub fn projection(&self) -> MatchProjection {
        self.state
    }

    pub fn apply(&mut self, action: MatchAction) -> MatchProjection {
        if self.state.phase == MatchPhase::Finished {
            return self.state;
        }

        match action {
            MatchAction::Finish => {
                self.state.phase = MatchPhase::Finished;
                self.state.possession = Possession::None;
                return self.state;
            }
            MatchAction::Start if self.state.phase == MatchPhase::PreKickoff => {
                self.state.phase = MatchPhase::InPlay;
                self.state.possession = self.state.restart_side.into();
                return self.state;
            }
            MatchAction::Restart if self.state.phase == MatchPhase::Stoppage => {
                self.state.phase = MatchPhase::InPlay;
                self.state.possession = self.state.restart_side.into();
                return self.state;
            }
            _ => {}
        }

        if self.state.phase != MatchPhase::InPlay {
            return self.state;
        }

        match action {
            MatchAction::Tick => {
                self.state.tick = self.state.tick.saturating_add(1).min(self.max_ticks);
                if self.state.tick >= self.max_ticks {
                    self.state.phase = MatchPhase::Finished;
                    self.state.possession = Possession::None;
                }
            }
            MatchAction::HomePossession => self.state.possession = Possession::Home,
            MatchAction::AwayPossession => self.state.possession = Possession::Away,
            MatchAction::LooseBall => self.state.possession = Possession::None,
            MatchAction::HomeGoal if self.state.home_score < self.score_cap => {
                self.state.home_score += 1;
                self.state.phase = MatchPhase::Stoppage;
                self.state.possession = Possession::None;
                self.state.restart_side = Side::Away;
            }
            MatchAction::AwayGoal if self.state.away_score < self.score_cap => {
                self.state.away_score += 1;
                self.state.phase = MatchPhase::Stoppage;
                self.state.possession = Possession::None;
                self.state.restart_side = Side::Home;
            }
            MatchAction::Start
            | MatchAction::Restart
            | MatchAction::Finish
            | MatchAction::HomeGoal
            | MatchAction::AwayGoal => {}
        }

        self.state
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.state.tick > self.max_ticks {
            return Err("tick exceeds configured maximum");
        }
        if self.state.home_score > self.score_cap || self.state.away_score > self.score_cap {
            return Err("score exceeds configured cap");
        }
        if matches!(
            self.state.phase,
            MatchPhase::PreKickoff | MatchPhase::Stoppage | MatchPhase::Finished
        ) && self.state.possession != Possession::None
        {
            return Err("non-playing phase cannot retain possession");
        }
        Ok(())
    }
}

impl Default for SoccerMatch {
    fn default() -> Self {
        Self::formal_fixture()
    }
}

/// A deterministic session seam suitable for desktop, server, and formal replay callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoccerRealtimeSession {
    match_state: SoccerMatch,
}

impl SoccerRealtimeSession {
    pub fn new(match_state: SoccerMatch) -> Self {
        Self { match_state }
    }

    pub fn formal_fixture() -> Self {
        Self::new(SoccerMatch::formal_fixture())
    }

    pub fn projection(&self) -> MatchProjection {
        self.match_state.projection()
    }

    pub fn apply(&mut self, action: MatchAction) -> MatchProjection {
        self.match_state.apply(action)
    }

    /// Replays canonical events and returns the initial projection followed by every
    /// post-event projection.
    pub fn replay<I>(&mut self, actions: I) -> Vec<MatchProjection>
    where
        I: IntoIterator<Item = MatchAction>,
    {
        let mut trace = vec![self.projection()];
        for action in actions {
            trace.push(self.apply(action));
        }
        trace
    }
}

impl Default for SoccerRealtimeSession {
    fn default() -> Self {
        Self::formal_fixture()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_stops_play_and_gives_restart_to_conceding_side() {
        let mut session = SoccerRealtimeSession::formal_fixture();
        session.apply(MatchAction::Start);
        session.apply(MatchAction::HomePossession);
        let after = session.apply(MatchAction::HomeGoal);

        assert_eq!(after.phase, MatchPhase::Stoppage);
        assert_eq!(after.home_score, 1);
        assert_eq!(after.away_score, 0);
        assert_eq!(after.possession, Possession::None);
        assert_eq!(after.restart_side, Side::Away);

        let restarted = session.apply(MatchAction::Restart);
        assert_eq!(restarted.phase, MatchPhase::InPlay);
        assert_eq!(restarted.possession, Possession::Away);
    }

    #[test]
    fn finished_match_is_absorbing() {
        let mut session = SoccerRealtimeSession::formal_fixture();
        session.apply(MatchAction::Start);
        let finished = session.apply(MatchAction::Finish);

        for action in [
            MatchAction::Tick,
            MatchAction::HomeGoal,
            MatchAction::AwayGoal,
            MatchAction::Restart,
        ] {
            assert_eq!(session.apply(action), finished);
        }
    }

    #[test]
    fn identical_event_sequences_replay_identically() {
        let actions = [
            MatchAction::Start,
            MatchAction::AwayGoal,
            MatchAction::Restart,
            MatchAction::AwayPossession,
            MatchAction::Tick,
            MatchAction::Finish,
        ];
        let first = SoccerRealtimeSession::formal_fixture().replay(actions);
        let second = SoccerRealtimeSession::formal_fixture().replay(actions);
        assert_eq!(first, second);
    }

    #[test]
    fn clock_and_score_bounds_are_enforced() {
        let mut session = SoccerRealtimeSession::formal_fixture();
        session.apply(MatchAction::Start);
        for _ in 0..FORMAL_SCORE_CAP + 2 {
            session.apply(MatchAction::HomeGoal);
            session.apply(MatchAction::Restart);
        }
        for _ in 0..FORMAL_MAX_TICKS + 2 {
            session.apply(MatchAction::Tick);
        }

        let final_state = session.projection();
        assert_eq!(final_state.home_score, FORMAL_SCORE_CAP);
        assert_eq!(final_state.tick, FORMAL_MAX_TICKS);
        assert_eq!(final_state.phase, MatchPhase::Finished);
        assert!(SoccerMatch {
            state: final_state,
            max_ticks: FORMAL_MAX_TICKS,
            score_cap: FORMAL_SCORE_CAP,
        }
        .validate()
        .is_ok());
    }
}
