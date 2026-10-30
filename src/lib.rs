#![recursion_limit = "256"]

//! Universal CI/CD gatekeeper and AI coding agent diff sentinel.
//!
//! # Embedding Discipline
//!
//! The supported integration interface for Discipline is the command-line binary
//! (`discipline`) and its structured machine-readable outputs:
//! - JSON report (`--format json`, `discipline.report.schema.json`)
//! - SARIF 2.1.0 (`--format sarif`, `discipline.sarif`)
//! - JUnit XML (`--format junit`, `junit.xml`)
//! - GitLab Code Quality (`--format gitlab`, `gl-codequality.json`)
//! - MCP server (`discipline mcp`, Model Context Protocol over stdio)
//!
//! The Rust modules in this crate are internal implementation details exposed for
//! binary composition and integration tests; they carry no Rust API stability guarantee.
//! Public API stability applies strictly to the CLI flags, configuration schema, finding
//! codes, and output schemas as documented in `docs/ARCHITECTURE.md` §3.2.

pub mod ast;
pub mod audit;
pub mod audit_html;
pub mod baseline;
pub mod cli;
pub mod comment;
pub mod config;
pub mod could_not_check;
pub mod deep_stack;
pub mod docs;
pub mod doctor;
pub mod doctor_pins;
pub mod doctor_settings;
pub mod escape;
pub mod explain;
pub mod findings;
pub mod forge;
pub mod gitctx;
pub mod guards;
pub mod hook;
pub mod hook_log;
pub mod hookfile;
pub mod init;
pub mod lease;
pub mod mcp;
pub mod output_schema;
pub mod override_policy;
pub mod pretool;
pub mod ratification;
pub mod references;
pub mod refusals;
pub mod replay;
pub mod report;
pub mod review_threads;
pub mod schema;
pub mod selftest;
pub mod session;
pub mod style;
pub mod tokens;
pub mod transcript;
pub mod turn;

pub use config::DisciplineConfig;
