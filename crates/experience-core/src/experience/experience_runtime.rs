//! Experience Runtime: the total controller of the Experience system.
//!
//! It answers the question that defines this project: **should the LLM be
//! invoked at all?** The runtime sits BEFORE the LLM in the agent loop, not
//! behind it. If the runtime only ever ran after the LLM decided something, it
//! would be a tool for the LLM — this module exists to make that impossible.

use super::experience::Experience;
use super::experience::ExperienceKind;
use super::experience::ExperienceStatus;
use super::experience::InputScope;
use super::experience_confidence::ConfidenceCounters;
use super::experience_confidence::ConfidenceWeights;
use super::experience_confidence::FeedbackKind;
use super::experience_learner::ExperienceDraft;
use super::experience_learner::ExperienceLearner;
use super::experience_learner::ExperienceTrace;
use super::experience_lifecycle::ExperienceLifecycle;
use super::experience_lifecycle::ForgettingDecision;
use super::experience_lifecycle::LifecycleAction;
use super::experience_lifecycle::LifecycleError;
use super::experience_store::ExperienceStore;
use super::experience_matcher::match_experiences;
use super::experience_usage::ExperienceUsageStore;
use super::experience_state::ExperienceId;
use super::experience_state::ExperienceState;
use super::experience_state::ExecutionMode;
use super::experience_state::RiskLevel;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

/// Result of `ExperienceRuntime::tick` (M2-3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickReport {
    pub expired_elements: Vec<String>,
    pub demoted: u32,
    pub disabled: u32,
}

/// Compiles a raw trace into a distilled experience. The default session
/// injects an LLM-backed compiler (small model call) once the trace is pruned
/// and still messy; tests may inject deterministic ones.
pub trait ExperienceCompiler: Send + Sync {
    fn compile(&self, trace: &ExperienceTrace) -> Option<ExperienceDraft>;
}

/// One row of the management surface (page / CLI / MCP).
#[derive(Debug, Clone, PartialEq)]
pub struct ExperienceManagementEntry {
    pub id: ExperienceId,
    pub name: String,
    pub kind: ExperienceKind,
    pub status: ExperienceStatus,
    pub confidence: f32,
    pub pinned: bool,
    pub last_used: Option<u64>,
}

/// Decision thresholds (first version; borrowed semantics from the reference
/// implementation and refined into the four control levels).
pub(crate) const CONF_EXECUTE: f32 = 0.75;
pub(crate) const CONF_ASSIST: f32 = 0.35;
pub(crate) const SIM_EXECUTE: f32 = 0.50;
pub(crate) const SIM_ASSIST: f32 = 0.30;
/// Hint band: "similar but not confirmed" — a parallel LLM assists with these
/// matches, using the experience only as reference (LLM stays primary and
/// keeps the right to question the experience).
pub(crate) const CONF_HINT: f32 = 0.25;
pub(crate) const SIM_HINT: f32 = 0.10;

/// A candidate experience, ranked by the matcher (highest similarity first).
#[derive(Debug, Clone, Copy)]
pub struct ExperienceCandidate<'a> {
    pub experience: &'a Experience,
    pub similarity: f32,
}

/// Input to the decision algorithm: the current state plus ranked candidates.
#[derive(Debug, Clone, Copy)]
pub struct DecisionInput<'a> {
    pub state: &'a ExperienceState,
    pub candidates: &'a [ExperienceCandidate<'a>],
}

/// What the Experience Runtime decides at the fast decision point.
///
/// Experience is NOT a prerequisite that the LLM must pass before running.
/// It is a fast pseudo-parallel path: the decision itself is the LLM's time
/// lag. Levels:
/// ① ExperienceOnly   - experience runs and completes the task; zero LLM.
/// ② ExperienceFirst  - experience runs first; the LLM checks / takes over
///                       the unfinished part (it can read the execution state).
/// ③ Reference        - experience is injected as reference; LLM leads.
/// ④ Delegate         - no / untrusted experience; fully LLM.
/// + AbortOrAsk       - conflict / high risk: stop auto-execution and ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlDecision {
    /// ① 完全不用 LLM：经验执行并完成任务（LLM 零调用）。
    ExperienceOnly(ExperienceId),
    /// ② 经验为主、LLM 仅检查：经验先执行；未完成时 LLM 读取执行状态后介入。
    ExperienceFirst(ExperienceId),
    /// ③ 经验为参考、LLM 为主：经验内容注入参考，LLM 主导。
    Reference(ExperienceId),
    /// ④ 无经验或经验不可信：完全依赖 LLM。
    Delegate,
    /// 冲突/高风险：停止自动执行，询问或显式交回 LLM。
    AbortOrAsk,
}

impl ControlDecision {
    /// The agent behavior mode corresponding to a decision (Phase 6
    /// formalization): experience executing → Reflex; LLM deliberating →
    /// Deliberation; conflict/risk → Conflict.
    pub fn execution_mode(&self) -> ExecutionMode {
        match self {
            ControlDecision::ExperienceOnly(_) | ControlDecision::ExperienceFirst(_) => {
                ExecutionMode::Reflex
            }
            ControlDecision::Reference(_) | ControlDecision::Delegate => {
                ExecutionMode::Deliberation
            }
            ControlDecision::AbortOrAsk => ExecutionMode::Conflict,
        }
    }
}

/// The total controller of the Experience system (Phase 4).
///
/// Later phases attach: matcher, workflow executor, confidence, store. The
/// decision algorithm itself is implemented now and fully testable; with an
/// empty candidate list it returns `Delegate`, keeping current behavior.
///
/// Note: no `Debug`/`Clone` here on purpose — the runtime is held behind a
/// `tokio::sync::Mutex` and owns a `Box<dyn ExperienceCompiler>`, which cannot
/// be auto-derived.
#[derive(Default)]
pub struct ExperienceRuntime {
    /// Master switch; when disabled, matching/execution/learning are all
    /// skipped so the loop behaves like the original Codex.
    disabled: bool,
    /// The experience store: where experiences live across turns.
    store: ExperienceStore,
    /// Feedback counters per experience (A6 feedback loop).
    counters: BTreeMap<ExperienceId, ConfidenceCounters>,
    /// Phase 7 learner: turns successful LLM traces into candidates.
    learner: ExperienceLearner,
    /// Optional distiller: turns messy pruned traces into usable experiences.
    compiler: Option<Box<dyn ExperienceCompiler>>,
    /// Session-persistent state shared between experience and LLM (M2-2).
    session_state: ExperienceState,
    /// Optional store file for cross-process persistence (M2-5).
    store_path: Option<PathBuf>,
    /// Optional usage/audit file (management channel step 2).
    usage_path: Option<PathBuf>,
}

impl ExperienceRuntime {
    /// Create a runtime whose store loads from (and persists to) the given
    /// JSON file. A missing/corrupt file yields an empty store (best-effort,
    /// with a warning).
    pub fn load_from_path(path: &Path) -> Self {
        let mut runtime = Self::default();
        match ExperienceStore::load_from_path(path) {
            Ok(store) => runtime.store = store,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(?path, "failed to load experience store: {error}");
            }
        }
        runtime.store_path = Some(path.to_path_buf());
        runtime.usage_path = path.parent().map(|dir| dir.join("usage.json"));
        runtime
    }

    /// Set the master switch (synced from `EXPERIENCE_ENABLED` each turn).
    pub fn set_enabled(&mut self, enabled: bool) {
        self.disabled = !enabled;
    }

    pub fn is_enabled(&self) -> bool {
        !self.disabled
    }

    /// Best-effort persistence to `store_path` (no-op when unset).
    fn persist(&self) {
        let Some(path) = &self.store_path else {
            return;
        };
        if let Err(error) = self.store.save_to_path(path) {
            tracing::warn!(?path, "failed to persist experience store: {error}");
        }
    }

    /// Register (insert or replace) an experience in the store.
    pub fn register(&mut self, experience: Experience) {
        self.store.upsert(experience);
        self.persist();
    }

    pub fn store(&self) -> &ExperienceStore {
        &self.store
    }

    /// Apply one feedback event: update counters, then refresh the stored
    /// experience's confidence score (A6) and drive its lifecycle (Phase 9).
    pub fn record_feedback(
        &mut self,
        id: &ExperienceId,
        feedback: FeedbackKind,
        now_secs: u64,
    ) {
        let counters = self.counters.entry(id.clone()).or_default();
        counters.apply(feedback);
        counters.touch(now_secs);
        let score = counters.score(&ConfidenceWeights::default(), 0.0, 1.0);
        if let Some(mut experience) = self.store.get(id).cloned() {
            experience.confidence = score;
            experience.status = ExperienceLifecycle::apply_feedback(
                experience.status,
                score,
                counters.streak,
                counters.failure_streak,
            );
            self.store.upsert(experience);
            self.persist();
        }
    }

    /// §14.5 misfire accounting: a replay failure caused by the *call* (stale
    /// parameter, source already moved, permission refusal) — not by the
    /// experience's logic. Record the pattern as a failure mode for future
    /// distillation, but do NOT lower quality confidence.
    pub fn record_misfire(&mut self, id: &ExperienceId, note: impl Into<String>) {
        let note = note.into();
        let Some(mut experience) = self.store.get(id).cloned() else {
            return;
        };
        if !experience.failure_modes.contains(&note) {
            experience.failure_modes.push(note);
            const MAX_FAILURE_MODES: usize = 12;
            if experience.failure_modes.len() > MAX_FAILURE_MODES {
                experience.failure_modes.drain(0..experience.failure_modes.len() - MAX_FAILURE_MODES);
            }
            self.store.upsert(experience);
            self.persist();
        }
    }

    /// Explicit lifecycle transition (validate / activate / disable /
    /// revalidate) on a stored experience.
    pub fn transition_experience(
        &mut self,
        id: &ExperienceId,
        action: LifecycleAction,
    ) -> Result<ExperienceStatus, LifecycleError> {
        let result = self.store.transition(id, action);
        if result.is_ok() {
            self.persist();
        }
        result
    }

    pub fn counters_of(&self, id: &ExperienceId) -> Option<&ConfidenceCounters> {
        self.counters.get(id)
    }

    pub fn feedback_stats(&self) -> &BTreeMap<ExperienceId, ConfidenceCounters> {
        &self.counters
    }

    /// Phase 7: observe an LLM execution trace. When repeatability is detected
    /// (consecutive successes or user confirmation), compile it into an
    /// `ExperienceDraft` and register it in the store (status CANDIDATE).
    pub fn observe_trace(
        &mut self,
        trace: &ExperienceTrace,
        user_confirmed: bool,
    ) -> Option<ExperienceDraft> {
        self.admit_trace(trace, user_confirmed, /*force*/ false)
    }

    /// User-explicit release channel: when the user asks to "remember this
    /// procedure / save this as an experience", record the trace even if the
    /// no-improvement gate would otherwise skip it. Quality gates are NOT
    /// bypassed: fake-success traces (no successful step evidence) and
    /// non-executable traces are still refused.
    pub fn force_record(&mut self, trace: &ExperienceTrace) -> Option<ExperienceDraft> {
        self.admit_trace(trace, /*user_confirmed*/ true, /*force*/ true)
    }

    /// Reference-log a usage of one experience (step 2): decision band,
    /// optional effect, and the thread/process/turn context. Reloads the
    /// latest usage.json first (optimistic; never a private copy).
    pub fn record_usage(
        &mut self,
        id: &ExperienceId,
        band: &str,
        outcome: Option<String>,
        thread_id: Option<String>,
        turn_id: Option<String>,
        process: String,
        task: Option<String>,
        now_secs: u64,
    ) {
        let Some(path) = self.usage_path.clone() else {
            return;
        };
        let mut usage = ExperienceUsageStore::load_from_path(&path);
        usage.record_logged(
            id.0.as_str(),
            band,
            outcome,
            thread_id,
            turn_id,
            process,
            task,
            now_secs,
        );
        if let Err(error) = usage.save_to_path(&path) {
            tracing::warn!(?path, "failed to persist experience usage: {error}");
        }
    }

    fn admit_trace(
        &mut self,
        trace: &ExperienceTrace,
        user_confirmed: bool,
        force: bool,
    ) -> Option<ExperienceDraft> {
        if self.disabled {
            return None;
        }
        let mut draft = self.learner.observe(trace, user_confirmed)?;
        if draft.experience.status == ExperienceStatus::Candidate
            && trace.actions.len() > super::experience_learner::MAX_AUTO_CONFIRM_STEPS
        {
            if let Some(compiler) = &self.compiler {
                if let Some(distilled) = compiler.compile(trace) {
                    tracing::info!(
                        experience_id = %distilled.experience.id,
                        "experience distilled from messy trace"
                    );
                    draft = distilled;
                }
            }
        }
        // Silence-audit candidate 2: close the state↔experience loop. The
        // experience inherits the generic environment preconditions the task
        // actually succeeded under (from the learning snapshot), so its
        // conditions are never empty/invented — decide() then re-checks them
        // against the current state before any hit.
        let derived = super::experience_learner::conditions_from_snapshot(
            trace.environment_snapshot.as_ref(),
        );
        super::experience_learner::merge_conditions(&mut draft.experience.conditions, derived);
        // Candidate-3 backfill: completion criteria default to the agent's
        // final observed evidence when nothing more precise was distilled.
        if draft.experience.completion_criteria.is_none() {
            if let Some(evidence) = &trace.final_evidence {
                draft.experience.completion_criteria = Some(evidence.clone());
            }
        }
        // Necessity gate (store side): a draft that is NOT an improvement over
        // an already-validated/active experience for the same signature must
        // not overwrite it. "Improvement" here is deterministic and
        // conservative: a shorter (more distilled) workflow. Re-runs of an
        // already-known task therefore skip storage instead of producing
        // meaningless version bumps.
        if !force {
            // Necessity gate (store side): a draft that is NOT an improvement
            // over an already-validated/active experience for the same
            // signature must not overwrite it. "Improvement" is deterministic
            // and conservative: a shorter (more distilled) workflow. Re-runs
            // of an already-known task therefore skip storage instead of
            // producing meaningless version bumps. `force` (user-explicit
            // release channel) intentionally bypasses this gate.
            let is_improvement = match self.store.get(&draft.experience.id) {
                Some(existing)
                    if matches!(
                        existing.status,
                        ExperienceStatus::Validated | ExperienceStatus::Active
                    ) =>
                {
                    draft.experience.workflow.steps.len() < existing.workflow.steps.len()
                }
                _ => true,
            };
            if !is_improvement {
                tracing::info!(
                    experience_id = %draft.experience.id,
                    "necessity gate: draft is no shorter than stored experience; skipping"
                );
                return None;
            }
        }
        if self.store.upsert_validated(draft.experience.clone()).is_ok() {
            self.persist();
            Some(draft)
        } else {
            None
        }
    }

    /// Inject a distiller (session layer provides the LLM-backed one).
    pub fn set_compiler(&mut self, compiler: Box<dyn ExperienceCompiler>) {
        self.compiler = Some(compiler);
    }

    /// Auto-pipeline admission (distill → validate → activate): replace the
    /// stored candidate with a structure-validated experience under the same
    /// id (bumping its version) and promote VALIDATED → ACTIVE. Zero user
    /// interaction by design: a distilled experience that passes structural
    /// validation and the session dry-run check is usable immediately.
    pub fn admit_auto_experience(
        &mut self,
        experience: Experience,
    ) -> Result<ExperienceStatus, String> {
        let mut experience = experience;
        if let Some(existing) = self.store.get(&experience.id).cloned() {
            // A distilled replacement supersedes the original candidate.
            experience.version = existing.version.saturating_add(1);
        }
        self.store
            .upsert_validated(experience.clone())
            .map_err(|error| format!("{error:?}"))?;
        let next = self
            .store
            .transition(&experience.id, LifecycleAction::Activate)
            .map_err(|error| format!("{error:?}"))?;
        self.persist();
        Ok(next)
    }

    pub fn learner(&self) -> &ExperienceLearner {
        &self.learner
    }

    /// Pull the session-persistent parts (registry / deployed / baselines)
    /// into the turn state. Called at turn start after the fresh build.
    pub fn inherit_session(&mut self, turn_state: &mut ExperienceState) {
        turn_state.inherit_session(&self.session_state);
    }

    /// Snapshot the turn's final state as the session baseline for the next
    /// turn. Called at turn end.
    pub fn commit_session(&mut self, turn_state: &ExperienceState) {
        self.session_state = turn_state.clone();
    }

    pub fn session_state(&self) -> &ExperienceState {
        &self.session_state
    }

    /// Management privilege: an experience exempt from time-based forgetting.
    pub fn set_pinned(&mut self, id: &ExperienceId, pinned: bool) -> bool {
        let result = if pinned {
            self.store.pin(id)
        } else {
            self.store.unpin(id)
        };
        if result {
            self.persist();
        }
        result
    }

    pub fn is_pinned(&self, id: &ExperienceId) -> bool {
        self.store.is_pinned(id)
    }

    /// Rows for the management surface.
    pub fn management_list(&self) -> Vec<ExperienceManagementEntry> {
        self.store
            .all()
            .iter()
            .map(|experience| ExperienceManagementEntry {
                id: experience.id.clone(),
                name: experience.name.clone(),
                kind: experience.kind,
                status: experience.status,
                confidence: experience.confidence,
                pinned: self.store.is_pinned(&experience.id),
                last_used: self
                    .counters
                    .get(&experience.id)
                    .and_then(|counters| counters.last_used),
            })
            .collect()
    }

    /// M2-3 tick: expire stale state elements and forget unused experiences
    /// (pinned ones are exempt).
    pub fn tick(&mut self, now_secs: u64) -> TickReport {
        let expired_elements = self.session_state.tick(now_secs);
        let mut demoted = 0u32;
        let mut disabled = 0u32;
        let ids: Vec<ExperienceId> = self.store.all().iter().map(|e| e.id.clone()).collect();
        for id in ids {
            if self.store.is_pinned(&id) {
                continue;
            }
            let Some(status) = self.store.get(&id).map(|experience| experience.status) else {
                continue;
            };
            let last_used = self
                .counters
                .get(&id)
                .and_then(|counters| counters.last_used);
            match ExperienceLifecycle::forgetting(status, last_used, now_secs, false) {
                ForgettingDecision::Keep => {}
                ForgettingDecision::DemoteToDecaying => {
                    if let Some(mut experience) = self.store.get(&id).cloned() {
                        experience.status = ExperienceStatus::Decaying;
                        self.store.upsert(experience);
                        demoted += 1;
                    }
                }
                ForgettingDecision::Disable => {
                    if let Some(mut experience) = self.store.get(&id).cloned() {
                        experience.status = ExperienceStatus::Disabled;
                        self.store.upsert(experience);
                        disabled += 1;
                    }
                }
            }
        }
        if demoted > 0 || disabled > 0 {
            self.persist();
        }
        TickReport {
            expired_elements,
            demoted,
            disabled,
        }
    }

    /// Full verification chain: match (content existence) -> decide
    /// (state applicability + trust). Answers "is there a corresponding
    /// experience AND is it allowed to act".
    pub fn decide_for_state(&self, state: &ExperienceState) -> ControlDecision {
        if self.disabled {
            return ControlDecision::Delegate;
        }
        let candidates = match_experiences(state, self.store.matchable());
        let input = DecisionInput {
            state,
            candidates: &candidates,
        };
        self.decide(&input)
    }

    /// The fast decision point called from `run_turn` before every sampling
    /// request. Candidates must arrive pre-sorted by similarity (matcher).
    ///
    /// Algorithm (see `docs/architecture/experience-runtime-design.md` §14):
    /// path selection is a PRIOR decision over the task-experience
    /// correspondence — semantic correspondence (SEM), shape isomorphism
    /// (ISO: single↔single, batch↔batch), completion-criteria coverage (COV),
    /// plus trust (TRU). Never discovered by trying paths.
    pub(crate) fn decide(&self, input: &DecisionInput<'_>) -> ControlDecision {
        if self.disabled {
            return ControlDecision::Delegate;
        }
        let state = input.state;
        if state.risk >= RiskLevel::High {
            return ControlDecision::AbortOrAsk;
        }
        let task_text = input
            .state
            .task
            .as_ref()
            .map(|task| task.description.as_str())
            .or_else(|| input.state.current_context.intent.as_deref())
            .unwrap_or_default();
        let scope = task_input_scope(task_text);
        for candidate in input.candidates {
            let exp = candidate.experience;
            let conditions_ok = exp
                .conditions
                .iter()
                .all(|condition| state.satisfies(&condition.key, &condition.expected));
            if !conditions_ok {
                // rejected; later phases record this for audit/LLM context
                continue;
            }
            // SEM gate: when applicable objects are declared, at least one
            // must appear in the current task text.
            let declared = &exp.applicability.applicable_objects;
            let lower_task = task_text.to_lowercase();
            if !declared.is_empty()
                && !declared
                    .iter()
                    .any(|object| lower_task.contains(&object.to_lowercase()))
            {
                continue;
            }
            let iso = shape_isomorphic(exp, scope);
            let cov = criteria_covers(exp, task_text, scope);
            let strong_trust = exp.confidence >= CONF_EXECUTE
                && candidate.similarity >= SIM_EXECUTE
                && exp.risk <= RiskLevel::Medium;
            let medium_trust =
                exp.confidence >= CONF_ASSIST && candidate.similarity >= SIM_ASSIST;

            if iso && cov && strong_trust
                && matches!(exp.kind, ExperienceKind::Reflex | ExperienceKind::Result)
            {
                return ControlDecision::ExperienceOnly(exp.id.clone());
            }
            if iso && cov && medium_trust
                && matches!(exp.kind, ExperienceKind::Reflex | ExperienceKind::Result)
            {
                return ControlDecision::ExperienceFirst(exp.id.clone());
            }
            // Every other correspondence that still relates to the task goes
            // to Reference — the LLM leads and adapts. Never blind-replay a
            // non-isomorphic experience (docx batch lesson, §14.1).
            if candidate.similarity >= SIM_HINT && exp.confidence >= CONF_HINT {
                return ControlDecision::Reference(exp.id.clone());
            }
        }
        ControlDecision::Delegate
    }
}

/// §14: whether the current task text speaks about a batch of inputs.
fn task_input_scope(task: &str) -> InputScope {
    let lower = task.to_lowercase();
    const BATCH_MARKERS: &[&str] = &[
        "所有",
        "全部",
        "每个",
        "遍历",
        "批量",
        "所有文档",
        "所有文件",
        "all ",
        "all files",
        "each",
        "batch",
        "every",
    ];
    if BATCH_MARKERS.iter().any(|marker| lower.contains(marker)) || task.contains('*') {
        InputScope::Batch
    } else {
        InputScope::Single
    }
}

/// §14 ISO: shape isomorphism. Unknown metadata is conservative — it can
/// never grant ①/② (re-distillation must declare scope/parameterization).
fn shape_isomorphic(experience: &Experience, task_scope: InputScope) -> bool {
    match experience.applicability.input_scope {
        InputScope::Any => true,
        InputScope::Single => task_scope == InputScope::Single,
        InputScope::Batch => task_scope == InputScope::Batch,
        InputScope::Unknown => false,
    }
}

/// §14 COV: does the experience's completion criteria cover the current task
/// goal? Without criteria we cannot judge completion — no ①/②.
fn criteria_covers(experience: &Experience, task: &str, task_scope: InputScope) -> bool {
    let Some(criteria) = experience.completion_criteria.as_deref() else {
        return false;
    };
    if task_scope == InputScope::Batch {
        const BATCH_WORDS: &[&str] = &[
            "所有",
            "全部",
            "每个",
            "遍历",
            "批量",
            "个文件",
            "个文档",
            "all",
            "each",
            "batch",
            "every",
        ];
        let lower = criteria.to_lowercase();
        return BATCH_WORDS.iter().any(|word| lower.contains(word))
            || (task.contains('*') && criteria.contains('*'));
    }
    !criteria.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::experience::Applicability;
    use super::super::experience::ExperienceCondition;
    use super::super::experience::InputScope as TestScope;
    use super::super::experience::ExperienceStatus;
    use super::super::experience::ExperienceTrigger;
    use super::super::experience::ExperienceWorkflow;

    fn test_experience(kind: ExperienceKind, confidence: f32, risk: RiskLevel) -> Experience {
        Experience {
            id: ExperienceId("e1".to_string()),
            name: "test".to_string(),
            kind,
            trigger: ExperienceTrigger::default(),
            conditions: Vec::new(),
            workflow: ExperienceWorkflow::default(),
            completion_criteria: Some("done".to_string()),
            failure_modes: Vec::new(),
            assets: Default::default(),
            applicability: Applicability {
                input_scope: TestScope::Single,
                applicable_objects: Vec::new(),
                parameterized: true,
            },
            title: None,
            note: None,
            created_at: None,
            confidence,
            risk,
            status: ExperienceStatus::Active,
            version: 1,
        }
    }

    #[test]
    fn no_candidates_delegates() {
        let runtime = ExperienceRuntime::default();
        let state = ExperienceState::default();
        let input = DecisionInput {
            state: &state,
            candidates: &[],
        };
        assert_eq!(runtime.decide(&input), ControlDecision::Delegate);
    }

    #[test]
    fn reflex_high_confidence_is_experience_only() {
        let runtime = ExperienceRuntime::default();
        let state = ExperienceState::default();
        let exp = test_experience(ExperienceKind::Reflex, 0.9, RiskLevel::Low);
        let candidates = [ExperienceCandidate {
            experience: &exp,
            similarity: 0.8,
        }];
        let input = DecisionInput {
            state: &state,
            candidates: &candidates,
        };
        assert_eq!(
            runtime.decide(&input),
            ControlDecision::ExperienceOnly(ExperienceId("e1".to_string()))
        );
    }

    #[test]
    fn reflex_medium_confidence_is_experience_first() {
        let runtime = ExperienceRuntime::default();
        let state = ExperienceState::default();
        let exp = test_experience(ExperienceKind::Reflex, 0.5, RiskLevel::Low);
        let candidates = [ExperienceCandidate {
            experience: &exp,
            similarity: 0.4,
        }];
        let input = DecisionInput {
            state: &state,
            candidates: &candidates,
        };
        assert_eq!(
            runtime.decide(&input),
            ControlDecision::ExperienceFirst(ExperienceId("e1".to_string()))
        );
    }

    #[test]
    fn reference_kind_is_reference() {
        let runtime = ExperienceRuntime::default();
        let state = ExperienceState::default();
        let exp = test_experience(ExperienceKind::Reference, 0.6, RiskLevel::Low);
        let candidates = [ExperienceCandidate {
            experience: &exp,
            similarity: 0.5,
        }];
        let input = DecisionInput {
            state: &state,
            candidates: &candidates,
        };
        assert_eq!(
            runtime.decide(&input),
            ControlDecision::Reference(ExperienceId("e1".to_string()))
        );
    }

    #[test]
    fn hint_band_similar_but_unconfirmed_goes_to_parallel_llm() {
        let runtime = ExperienceRuntime::default();
        let state = ExperienceState::default();
        let exp = test_experience(ExperienceKind::Process, 0.3, RiskLevel::Low);
        let candidates = [ExperienceCandidate {
            experience: &exp,
            similarity: 0.15,
        }];
        let input = DecisionInput {
            state: &state,
            candidates: &candidates,
        };
        assert_eq!(
            runtime.decide(&input),
            ControlDecision::Reference(ExperienceId("e1".to_string()))
        );
    }

    #[test]
    fn batch_task_with_single_scope_experience_is_reference_not_only() {
        // docx lesson (§14.1): high-confidence single-file experience must
        // NOT blind-replay on a batch task.
        let runtime = ExperienceRuntime::default();
        let mut state = ExperienceState::default();
        state.current_context.intent =
            Some("把桌面所有 Word 文档按标题分类".to_string());
        let exp = test_experience(ExperienceKind::Reflex, 0.9, RiskLevel::Low);
        let candidates = [ExperienceCandidate {
            experience: &exp,
            similarity: 0.8,
        }];
        let input = DecisionInput {
            state: &state,
            candidates: &candidates,
        };
        assert_eq!(
            runtime.decide(&input),
            ControlDecision::Reference(ExperienceId("e1".to_string()))
        );
    }

    #[test]
    fn unknown_scope_is_conservative_never_only() {
        let runtime = ExperienceRuntime::default();
        let state = ExperienceState::default();
        let mut exp = test_experience(ExperienceKind::Reflex, 0.9, RiskLevel::Low);
        exp.applicability = Applicability::default(); // Unknown scope
        let candidates = [ExperienceCandidate {
            experience: &exp,
            similarity: 0.8,
        }];
        let input = DecisionInput {
            state: &state,
            candidates: &candidates,
        };
        assert_eq!(
            runtime.decide(&input),
            ControlDecision::Reference(ExperienceId("e1".to_string()))
        );
    }

    #[test]
    fn high_risk_state_aborts() {
        let runtime = ExperienceRuntime::default();
        let mut state = ExperienceState::default();
        state.risk = RiskLevel::High;
        let exp = test_experience(ExperienceKind::Reflex, 0.9, RiskLevel::Low);
        let candidates = [ExperienceCandidate {
            experience: &exp,
            similarity: 0.9,
        }];
        let input = DecisionInput {
            state: &state,
            candidates: &candidates,
        };
        assert_eq!(runtime.decide(&input), ControlDecision::AbortOrAsk);
    }

    #[test]
    fn unsatisfied_conditions_are_rejected() {
        let runtime = ExperienceRuntime::default();
        let state = ExperienceState::default();
        let mut exp = test_experience(ExperienceKind::Reflex, 0.9, RiskLevel::Low);
        exp.conditions = vec![ExperienceCondition {
            key: "environment.detection.missing_flag".to_string(),
            expected: serde_json::json!(true),
        }];
        let candidates = [ExperienceCandidate {
            experience: &exp,
            similarity: 0.9,
        }];
        let input = DecisionInput {
            state: &state,
            candidates: &candidates,
        };
        assert_eq!(runtime.decide(&input), ControlDecision::Delegate);
    }

    #[test]
    fn decide_for_state_full_chain() {
        let mut runtime = ExperienceRuntime::default();
        let mut state = ExperienceState::default();
        state.task = Some(super::super::experience_state::Task {
            description: "move pdfs to archive".to_string(),
            status: Default::default(),
        });
        let mut exp = test_experience(ExperienceKind::Reflex, 0.9, RiskLevel::Low);
        exp.trigger = super::super::experience::ExperienceTrigger {
            // sim = 3 hits / (3 keywords + 3 field slots) = 0.6 >= 0.5 -> ①
            keywords: vec![
                "move".to_string(),
                "pdf".to_string(),
                "archive".to_string(),
            ],
            ..Default::default()
        };
        runtime.register(exp);
        assert_eq!(
            runtime.decide_for_state(&state),
            ControlDecision::ExperienceOnly(ExperienceId("e1".to_string()))
        );

        // A different task does not match: delegate.
        let mut other = ExperienceState::default();
        other.task = Some(super::super::experience_state::Task {
            description: "review project structure".to_string(),
            status: Default::default(),
        });
        assert_eq!(runtime.decide_for_state(&other), ControlDecision::Delegate);
    }

    #[test]
    fn feedback_updates_counters_and_experience_confidence() {
        let mut runtime = ExperienceRuntime::default();
        let mut exp = test_experience(ExperienceKind::Reflex, 0.9, RiskLevel::Low);
        exp.id = ExperienceId("fx".to_string());
        runtime.register(exp);

        for _ in 0..8 {
            runtime.record_feedback(&ExperienceId("fx".to_string()), FeedbackKind::Success, 1_700_000_000);
        }
        let counters = runtime
            .counters_of(&ExperienceId("fx".to_string()))
            .expect("counters");
        assert_eq!(counters.success_count, 8);
        assert_eq!(counters.streak, 8);
        let expected = counters.score(&ConfidenceWeights::default(), 0.0, 1.0);
        let stored = runtime
            .store()
            .get(&ExperienceId("fx".to_string()))
            .unwrap()
            .confidence;
        assert!((stored - expected).abs() < 1e-4);

        runtime.record_feedback(&ExperienceId("fx".to_string()), FeedbackKind::Failure, 1_700_000_000);
        assert_eq!(
            runtime
                .counters_of(&ExperienceId("fx".to_string()))
                .unwrap()
                .streak,
            0
        );
    }

    #[test]
    fn observe_trace_registers_candidate_into_store() {
        use super::super::experience_learner::TraceAction;
        use super::super::experience_learner::TraceOutcome;

        let mut runtime = ExperienceRuntime::default();
        let trace = ExperienceTrace {
            task: "move pdfs to archive".to_string(),
            actions: vec![TraceAction {
                name: "tool.shell".to_string(),
                args: serde_json::json!({}),
            }],
            steps: Vec::new(),
            environment_snapshot: None,
            final_evidence: None,
            outcome: TraceOutcome::Success,
            llm_used: true,
        };
        assert!(runtime.observe_trace(&trace, false).is_none());
        let draft = runtime.observe_trace(&trace, false).expect("draft on 2nd success");
        assert_eq!(runtime.store().len(), 1);
        assert_eq!(
            draft.experience.status,
            super::super::experience::ExperienceStatus::Candidate
        );
    }

    #[test]
    fn observe_trace_brings_environment_conditions_into_experience() {
        use super::super::experience_learner::TraceAction;
        use super::super::experience_learner::TraceOutcome;

        let mut runtime = ExperienceRuntime::default();
        let trace = ExperienceTrace {
            task: "move pdfs to archive".to_string(),
            actions: vec![TraceAction {
                name: "tool.shell".to_string(),
                args: serde_json::json!({}),
            }],
            steps: Vec::new(),
            environment_snapshot: Some(serde_json::json!({
                "elements": [{"key": "runtime.subprocess_allowed", "value": true}],
                "environment": {"detections": [{"key": "os", "present": true}]},
            })),
            final_evidence: Some("moved".to_string()),
            outcome: TraceOutcome::Success,
            llm_used: true,
        };
        let draft = runtime.observe_trace(&trace, true).expect("draft");
        assert!(draft
            .experience
            .conditions
            .iter()
            .any(|condition| condition.key == "runtime.subprocess_allowed"));
        let stored = runtime.store().get(&draft.experience.id).expect("stored");
        assert_eq!(stored.conditions, draft.experience.conditions);
        // Candidate-3: the agent's final evidence becomes the completion
        // criteria when nothing more precise was distilled.
        assert_eq!(stored.completion_criteria.as_deref(), Some("moved"));
    }

    #[test]
    fn necessity_gate_skips_repeat_without_improvement() {
        use super::super::experience_learner::TraceAction;
        use super::super::experience_learner::TraceOutcome;

        fn trace_with(task: &str, cmds: &[&str]) -> ExperienceTrace {
            ExperienceTrace {
                task: task.to_string(),
                actions: cmds
                    .iter()
                    .map(|cmd| TraceAction {
                        name: "exec_command".to_string(),
                        args: serde_json::json!({ "cmd": cmd }),
                    })
                    .collect(),
                steps: Vec::new(),
                environment_snapshot: None,
                final_evidence: None,
                outcome: TraceOutcome::Success,
                llm_used: true,
            }
        }

        let mut runtime = ExperienceRuntime::default();
        let task = "move pngs to pic";
        assert!(
            runtime
                .observe_trace(&trace_with(task, &["scan", "move"]), true)
                .is_some()
        );
        // Same steps again: no improvement -> the gate skips storage.
        assert!(
            runtime
                .observe_trace(&trace_with(task, &["scan", "move"]), true)
                .is_none()
        );
        // A shorter (more distilled) workflow IS an improvement.
        assert!(
            runtime
                .observe_trace(&trace_with(task, &["move"]), true)
                .is_some()
        );
        let stored = runtime.store().all();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].workflow.steps.len(), 1);
    }

    #[test]
    fn force_record_bypasses_gate_but_keeps_quality_gates() {
        use super::super::experience_learner::TraceAction;
        use super::super::experience_learner::TraceOutcome;
        use super::super::experience_learner::TraceStep;

        fn trace_with(task: &str, cmds: &[&str]) -> ExperienceTrace {
            ExperienceTrace {
                task: task.to_string(),
                actions: cmds
                    .iter()
                    .map(|cmd| TraceAction {
                        name: "exec_command".to_string(),
                        args: serde_json::json!({ "cmd": cmd }),
                    })
                    .collect(),
                steps: Vec::new(),
                environment_snapshot: None,
                final_evidence: None,
                outcome: TraceOutcome::Success,
                llm_used: true,
            }
        }

        let mut runtime = ExperienceRuntime::default();
        let task = "package python script for reuse";
        assert!(
            runtime
                .observe_trace(&trace_with(task, &["venv", "build"]), true)
                .is_some()
        );
        // Normal path would skip (no improvement)...
        assert!(
            runtime
                .observe_trace(&trace_with(task, &["venv", "build"]), true)
                .is_none()
        );
        // ...but the user-explicit release channel records anyway.
        assert!(
            runtime
                .force_record(&trace_with(task, &["venv", "build"]))
                .is_some()
        );
        assert_eq!(runtime.store().len(), 1);

        // Quality gates are NOT bypassed: fake-success (all failed steps)
        // still refuses even under force.
        let mut fake = trace_with("download market data", &["fetch", "save"]);
        fake.steps = vec![TraceStep {
            name: "exec_command".to_string(),
            args: serde_json::json!({ "cmd": "fetch" }),
            call_id: Some("c1".to_string()),
            ok: Some(false),
            summary: Some("refused".to_string()),
        }];
        assert!(runtime.force_record(&fake).is_none());
    }

    #[test]
    fn misfire_records_pattern_without_confidence_penalty() {
        let mut runtime = ExperienceRuntime::default();
        let id = ExperienceId("misfire-1".to_string());
        let mut exp = test_experience(ExperienceKind::Reflex, 0.9, RiskLevel::Low);
        exp.id = id.clone();
        runtime.register(exp.clone());
        runtime.record_misfire(&id, "文件不存在：源已被移走");
        let stored = runtime.store().get(&id).expect("stored");
        assert_eq!(stored.confidence, 0.9);
        assert!(stored.failure_modes.contains(&"文件不存在：源已被移走".to_string()));
    }

    #[test]
    fn record_usage_writes_reference_log_file() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("exp-runtime-usage-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let store_path = dir.join("store.json");
        let mut runtime = ExperienceRuntime::load_from_path(&store_path);
        let id = ExperienceId("exp-ref-1".to_string());
        runtime.record_usage(
            &id,
            "reference",
            None,
            Some("thread-1".to_string()),
            Some("turn-1".to_string()),
            "test".to_string(),
            Some("分类 word 文档".to_string()),
            1234567890,
        );
        let usage = ExperienceUsageStore::load_from_path(&dir.join("usage.json"));
        let entry = &usage.entries["exp-ref-1"];
        assert_eq!(entry.logs.len(), 1);
        assert_eq!(entry.logs[0].thread_id.as_deref(), Some("thread-1"));
        assert_eq!(entry.logs[0].turn_id.as_deref(), Some("turn-1"));
        assert_eq!(entry.logs[0].band, "reference");
        assert_eq!(entry.logs[0].task.as_deref(), Some("分类 word 文档"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn master_switch_disables_matching_and_learning() {
        use super::super::experience_learner::TraceAction;
        use super::super::experience_learner::TraceOutcome;
        let mut runtime = ExperienceRuntime::default();
        runtime.set_enabled(false);
        assert!(!runtime.is_enabled());

        let exp = test_experience(ExperienceKind::Reflex, 0.9, RiskLevel::Low);
        let state = ExperienceState::default();
        let candidates = [ExperienceCandidate {
            experience: &exp,
            similarity: 0.9,
        }];
        let input = DecisionInput {
            state: &state,
            candidates: &candidates,
        };
        assert_eq!(runtime.decide(&input), ControlDecision::Delegate);
        assert_eq!(runtime.decide_for_state(&state), ControlDecision::Delegate);

        let trace = ExperienceTrace {
            task: "archive downloads".to_string(),
            actions: vec![TraceAction {
                name: "tool.shell".to_string(),
                args: serde_json::json!({}),
            }],
            steps: Vec::new(),
            environment_snapshot: None,
            final_evidence: Some("done".to_string()),
            outcome: TraceOutcome::Success,
            llm_used: true,
        };
        assert!(runtime.observe_trace(&trace, true).is_none());
        assert!(runtime.force_record(&trace).is_none());
    }

    #[test]
    fn feedback_drives_lifecycle_in_runtime() {
        let mut runtime = ExperienceRuntime::default();
        let mut exp = test_experience(ExperienceKind::Reflex, 0.2, RiskLevel::Low);
        exp.id = ExperienceId("lc".to_string());
        exp.status = super::super::experience::ExperienceStatus::Candidate;
        runtime.register(exp);
        let id = ExperienceId("lc".to_string());

        runtime.record_feedback(&id, FeedbackKind::Success, 1_700_000_000);
        assert_eq!(
            runtime.store().get(&id).unwrap().status,
            super::super::experience::ExperienceStatus::Validated
        );

        for _ in 0..20 {
            runtime.record_feedback(&id, FeedbackKind::Success, 1_700_000_000);
        }
        assert_eq!(
            runtime.store().get(&id).unwrap().status,
            super::super::experience::ExperienceStatus::Active
        );

        for _ in 0..5 {
            runtime.record_feedback(&id, FeedbackKind::Failure, 1_700_000_000);
        }
        assert_eq!(
            runtime.store().get(&id).unwrap().status,
            super::super::experience::ExperienceStatus::Disabled
        );
    }

    #[test]
    fn decision_maps_to_execution_mode() {
        use super::super::experience_state::ExecutionMode;
        assert_eq!(
            ControlDecision::ExperienceOnly(ExperienceId("e".to_string())).execution_mode(),
            ExecutionMode::Reflex
        );
        assert_eq!(
            ControlDecision::ExperienceFirst(ExperienceId("e".to_string())).execution_mode(),
            ExecutionMode::Reflex
        );
        assert_eq!(
            ControlDecision::Reference(ExperienceId("e".to_string())).execution_mode(),
            ExecutionMode::Deliberation
        );
        assert_eq!(ControlDecision::Delegate.execution_mode(), ExecutionMode::Deliberation);
        assert_eq!(ControlDecision::AbortOrAsk.execution_mode(), ExecutionMode::Conflict);
    }

    #[test]
    fn session_state_inherit_and_commit_round_trip() {
        use super::super::experience_state::ElementSource;
        use super::super::experience_state::StateElement;

        let mut runtime = ExperienceRuntime::default();
        let mut turn = ExperienceState::default();
        turn.elements.push(StateElement {
            key: "runtime.subprocess_allowed".to_string(),
            value: serde_json::json!(true),
            source: ElementSource::Detection,
            verified_at: None,
            ttl: None,
        });
        turn.task = Some(super::super::experience_state::Task {
            description: "t".to_string(),
            status: Default::default(),
        });
        runtime.commit_session(&turn);

        let mut next = ExperienceState::default(); // no new input
        runtime.inherit_session(&mut next);
        assert_eq!(next.elements.len(), 1);
        assert_eq!(next.task.unwrap().description, "t");
    }

    #[test]
    fn tick_forgets_unused_but_not_pinned() {
        let mut runtime = ExperienceRuntime::default();
        let now = 1_700_000_000u64;

        let mut old = test_experience(ExperienceKind::Reflex, 0.9, RiskLevel::Low);
        old.id = ExperienceId("old".to_string());
        old.status = ExperienceStatus::Active;
        runtime.register(old);
        runtime.record_feedback(&ExperienceId("old".to_string()), FeedbackKind::Success, now - 40 * 86_400);

        let mut pinned = test_experience(ExperienceKind::Reflex, 0.9, RiskLevel::Low);
        pinned.id = ExperienceId("pinned".to_string());
        pinned.status = ExperienceStatus::Active;
        runtime.register(pinned);
        runtime.record_feedback(&ExperienceId("pinned".to_string()), FeedbackKind::Success, now - 100 * 86_400);
        runtime.set_pinned(&ExperienceId("pinned".to_string()), true);

        let report = runtime.tick(now);
        assert_eq!(report.demoted, 1);
        assert_eq!(report.disabled, 0);
        assert_eq!(
            runtime.store().get(&ExperienceId("old".to_string())).unwrap().status,
            ExperienceStatus::Decaying
        );
        assert_eq!(
            runtime.store().get(&ExperienceId("pinned".to_string())).unwrap().status,
            ExperienceStatus::Active
        );

        // Management rows reflect pin + age.
        let rows = runtime.management_list();
        assert_eq!(rows.len(), 2);
        let pinned_row = rows.iter().find(|row| row.id == ExperienceId("pinned".to_string())).unwrap();
        assert!(pinned_row.pinned);
        assert!(pinned_row.last_used.is_some());
    }

    #[test]
    fn store_persists_across_runtime_reload() {
        let path = std::env::temp_dir().join(format!(
            "expcheck-runtime-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let mut runtime = ExperienceRuntime::load_from_path(&path);
            let mut exp = test_experience(ExperienceKind::Reflex, 0.9, RiskLevel::Low);
            exp.id = ExperienceId("p1".to_string());
            runtime.register(exp);
            runtime.set_pinned(&ExperienceId("p1".to_string()), true);
        }
        let loaded = ExperienceRuntime::load_from_path(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded.store().len(), 1);
        assert!(loaded.is_pinned(&ExperienceId("p1".to_string())));
    }

    #[test]
    fn injected_compiler_distills_messy_trace() {
        use super::super::experience_learner::TraceAction;
        use super::super::experience::ExperienceKind;

        struct DistillCompiler;
        impl ExperienceCompiler for DistillCompiler {
            fn compile(&self, trace: &ExperienceTrace) -> Option<ExperienceDraft> {
                let mut draft = ExperienceDraft {
                    experience: test_experience(ExperienceKind::Process, 0.9, RiskLevel::Low),
                    source_trace_task: trace.task.clone(),
                    created_at: "t".to_string(),
                };
                draft.experience.id = ExperienceId("distilled-1".to_string());
                draft.experience.status = super::super::experience::ExperienceStatus::Validated;
                draft.experience.trigger.keywords = vec!["instagram".to_string()];
                draft.experience.workflow.steps = vec![
                    super::super::experience::ExperienceWorkflowStep {
                        name: "exec_command".to_string(),
                        args: serde_json::json!({"cmd": "attach && read"}),
                    },
                ];
                Some(draft)
            }
        }

        let mut runtime = ExperienceRuntime::default();
        runtime.set_compiler(Box::new(DistillCompiler));
        let messy = ExperienceTrace {
            task: "open instagram".to_string(),
            actions: (0..=super::super::experience_learner::MAX_AUTO_CONFIRM_STEPS)
                .map(|i| TraceAction {
                    name: format!("step_{i}"),
                    args: serde_json::json!({}),
                })
                .collect(),
            steps: Vec::new(),
            environment_snapshot: None,
            final_evidence: None,
            outcome: super::super::experience_learner::TraceOutcome::Success,
            llm_used: true,
        };
        let draft = runtime.observe_trace(&messy, true).expect("draft");
        assert_eq!(draft.experience.id, ExperienceId("distilled-1".to_string()));
        assert_eq!(
            draft.experience.status,
            super::super::experience::ExperienceStatus::Validated
        );
    }

    #[test]
    fn admit_auto_experience_replaces_candidate_and_activates() {
        let mut runtime = ExperienceRuntime::default();
        let candidate_id = ExperienceId("cand-admit-1".to_string());
        let mut candidate = test_experience(ExperienceKind::Process, 0.20, RiskLevel::Low);
        candidate.id = candidate_id.clone();
        candidate.status = super::super::experience::ExperienceStatus::Candidate;
        runtime.register(candidate.clone());

        let mut distilled = test_experience(ExperienceKind::Process, 0.90, RiskLevel::Low);
        distilled.id = candidate_id.clone();
        distilled.status = super::super::experience::ExperienceStatus::Validated;
        distilled.trigger.keywords = vec!["move".to_string()];
        distilled.workflow.steps = vec![super::super::experience::ExperienceWorkflowStep {
            name: "exec_command".to_string(),
            args: serde_json::json!({"cmd": "move all"}),
        }];

        let status = runtime
            .admit_auto_experience(distilled.clone())
            .expect("admission");
        assert_eq!(
            status,
            super::super::experience::ExperienceStatus::Active
        );
        let stored = runtime.store().get(&candidate_id).expect("stored");
        assert_eq!(stored.status, super::super::experience::ExperienceStatus::Active);
        assert_eq!(stored.version, candidate.version + 1);
        assert_eq!(stored.workflow.steps.len(), 1);
    }
}
