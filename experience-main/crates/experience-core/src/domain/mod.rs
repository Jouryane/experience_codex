//! P1 Domain Model (p1-development-plan.md v1.3).
//!
//! This is the *new* architecture's compiled schema — distinct from the
//! legacy `experience` module (kept as migration reference). It locks the
//! three hard constraints in code:
//!
//! 1. Experience does not plan: workflow is a list of pre-compiled steps;
//! 2. Experience does not forge state: `completed` requires verified
//!    postconditions — constructor fixes the status flags; truthfulness is
//!    guaranteed by runtime verification order (never asserted by the type);
//! 3. Experience does not swallow failure: partial/failed are final statuses
//!    carrying executed side effects for Agent takeover.

pub mod action;
pub mod capability;
pub mod experience;
pub mod gate;
pub mod predicate;
pub mod template;

pub use action::ActionPattern;
pub use action::ActionProposal;
pub use experience::Experience;
pub use experience::ExperienceStatus;
pub use experience::FailurePolicy;
pub use experience::SchemaIssue;
pub use experience::UndoPolicy;
pub use experience::VerificationStep;
pub use experience::WorkflowStep;
pub use gate::CompletionStatus;
pub use gate::ExecutedSideEffect;
pub use gate::ExecutedStep;
pub use gate::GateDecision;
pub use gate::GateHitResult;
pub use gate::GateTier;
pub use gate::TemplateBindingAudit;
pub use gate::VerificationStatus;
pub use predicate::Predicate;
pub use predicate::StateFact;
pub use predicate::TruthValue;
pub use template::ExperienceTemplate;
pub use template::capture_parameter;
pub use template::stable_fingerprint;
pub use template::TemplateBindError;
pub use template::TemplateParameter;
