#![recursion_limit = "256"]

pub mod ast;
pub mod baseline;
pub mod cli;
pub mod config;
pub mod docs;
pub mod doctor;
pub mod forge;
pub mod gitctx;
pub mod guards;
pub mod hook;
pub mod override_policy;
pub mod report;
pub mod schema;
pub mod selftest;
pub mod style;
pub mod tokens;

pub use config::DisciplineConfig;
pub use gitctx as git;
