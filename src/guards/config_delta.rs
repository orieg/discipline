//! What `toolchain-config` and `sandbox-config` share: a rule table says which key path of
//! which configuration file loosens in which direction, both sides of a changed file are
//! loaded into one generic tree, and the two trees are compared under the rules.
//!
//! `engine.rs` holds the rules and the comparison, `files.rs` the file classification and
//! the format readers, and `gate.rs` the gate loop both gates run. Each gate keeps its own
//! rule table (`toolchain_config/rules.rs`, `sandbox_config/rules.rs`).

mod engine;
mod files;
mod gate;

pub use engine::*;
pub use files::*;
pub use gate::*;
