//! P1 Experience Runtime (M3): orchestrate Gate decide/execute over the
//! Store, probing State and running compiled workflows through an Executor.
//!
//! Contract (step-gate.md §8.2): decide + execute + verify + return all
//! happen inside one synchronous call. When the gate call returns, the
//! takeover is over — the Agent receives either a MISS (pass-through) or a
//! final GateHitResult (completed / partial / failed).

pub mod probe;
pub mod runner;
pub mod runtime;

pub use probe::Probe;
pub use runtime::ExperienceGateRuntime;
pub use runtime::GateContext;
pub use runtime::GateOutcome;
pub use runner::CapabilityRunner;
