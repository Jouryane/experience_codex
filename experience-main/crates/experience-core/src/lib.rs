//! Experience core — pure logic distilled from codex-main (Apache-2.0).
//!
//! This crate is runtime-agnostic: Experience State, matching, decision,
//! learner, lifecycle, confidence, store and process log. Controller and
//! agent adapters live in sibling crates.

pub mod experience;
pub mod domain;
pub mod exec;
pub mod policy;
pub mod redact;
pub mod safety;
pub mod shell_translate;
pub mod similarity;
pub mod state_source;
pub mod store;
pub mod template_induce;
