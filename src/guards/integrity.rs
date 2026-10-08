//! Gate-integrity: a change must not quietly lower the bar it is judged by.
//!
//! `discipline.toml` is fully user-configurable, which makes it the cheapest
//! thing for an agent to edit when a gate is in the way. This gate compares
//! the configuration on the base side with the head side and demands a scoped
//! `allow-gate-weakening:` directive for every loosening.

mod config_diff;
mod config_integrity;
mod directions;
mod golden_output;

pub use config_diff::*;
pub use config_integrity::*;
pub use directions::*;
pub use golden_output::*;

// The submodules reach these through `super::`.
use super::{command, issue_link, presets};
