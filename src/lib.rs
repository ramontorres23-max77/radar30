#![forbid(unsafe_code)]

//! `radar30`: multi-source, engagement-scored research engine over the last 30 days.
//!
//! Architecture modeled after `rust-anything`: single pure-Rust binary providing
//! CLI commands, a zero-SDK stdio MCP server, atomic agent `setup`, `doctor`
//! self-diagnostics, and an embedded `SKILL.md`.

pub mod cluster;
pub mod doctor;
pub mod fusion;
pub mod http;
pub mod mcp;
pub mod mcp_prompts;
pub mod pipeline;
pub mod preflight;
pub mod query;
pub mod render;
pub mod schema;
pub mod setup;
pub mod signals;
pub mod sources;
