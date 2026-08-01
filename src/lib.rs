//! # soccer_engine — agnostic 2D soccer simulation + RL game engine
//!
//! The soccer domain code extracted from `des_engine`. It depends on
//! [`des_engine`] for the generic optimization/learning primitives (LP/IP-MIP,
//! the neural-network MLP + policy-gradient, MDP/POMDP, PRNG, the animation
//! framework) but owns all soccer-specific simulation, rules, agents, planner,
//! rotation, and reinforcement learning (tabular Q-learning + neural value head,
//! actor-critic, PFSP league, world model).
//!
//! ## Layout
//! The crate mirrors the original `des::` module tree so the relocated soccer
//! files keep their `crate::des::…` import paths. Generic subsystems are
//! re-exported from `des_engine` inside [`des`]; soccer subsystems are local.
//!
//! ## Transport-agnostic
//! No HTTP transport is required to use the engine — construct a `SoccerMatch` /
//! `SoccerRealtimeSession` and drive it directly (a desktop game does this). The
//! web request→reply bridge and the legacy socket server will move behind opt-in
//! features in a follow-up.

pub use des_engine;

pub mod des;

// Ergonomic top-level facade. Production simulation types remain directly under
// `soccer`; the bounded formal lifecycle is namespaced to avoid ambiguous names
// such as `SoccerRealtimeSession` while preserving the adapter/refinement seam.
pub mod soccer {
    pub use crate::des::general::soccer::*;

    pub mod lifecycle {
        pub use crate::des::general::soccer_lifecycle::*;
    }
}

pub use des::general::soccer_rotation as rotation;
pub use des::soccer_learning;
#[cfg(feature = "planner")]
pub use des::soccer_planner;
pub use des::telemetry;
