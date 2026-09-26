//! The registry of every finding discipline can report.
//!
//! Each kind of finding has a stable code, rendered with its gate as `gate/code`
//! (`ci-integrity/verification-job-removed`). The code is the finding's identity: it is
//! frozen from 1.0 (docs/ARCHITECTURE.md §3.2), and reports, agents and tooling key on it.
//! The title is display text.
//!
//! A gate reports through [`crate::guards::GateOutcome::push`], which takes a kind from
//! this table, so a finding that is not registered does not compile. `tests` below hold
//! the table to its rules.

/// The title a fingerprint-version-1 baseline recorded for this kind (`crate::baseline`):
/// version 1 keyed on titles, so a renamed kind keeps its old title for matching and for
/// `discipline baseline --migrate` until the next major version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V1 {
    /// The title has not changed.
    Same,
    /// The title before it was renamed.
    Was(&'static str),
    /// The old title was the finding's message.
    Message,
    /// The old title carried data the reporting site builds
    /// ([`crate::guards::GateOutcome::push_site`]).
    Site,
}

/// One kind of finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindingKind {
    /// The gates that may report it. One, except for source-parse findings, which the
    /// first enabled AST gate reports; their code always uses the first gate listed.
    pub gates: &'static [&'static str],
    /// Kebab-case code, unique within each of its gates.
    pub code: &'static str,
    /// Display text (docs/GATES.md, "Finding Codes"). Free to change: nothing keys on it.
    pub title: &'static str,
    /// How a version-1 baseline titled it.
    pub v1: V1,
}

impl FindingKind {
    /// The version-1 title of a renamed kind, for a finding built as a
    /// [`crate::guards::Violation`] literal.
    pub fn was_title(&self) -> Option<String> {
        match self.v1 {
            V1::Was(t) => Some(t.to_string()),
            _ => None,
        }
    }
}

/// `gate/code` for a finding `gate` reports. A kind several gates may report (the
/// source-parse findings, which the first enabled AST gate reports) is always coded under
/// its first registered gate, so its code, and the fingerprint built on it, does not
/// depend on which gates are enabled.
pub fn full_code(gate: &str, kind: &FindingKind) -> String {
    assert!(
        kind.gates.contains(&gate),
        "gate `{gate}` reports `{}`, registered for {:?}",
        kind.code,
        kind.gates
    );
    format!("{}/{}", kind.gates[0], kind.code)
}

macro_rules! findings {
    ($($name:ident = [$($gate:literal),+], $code:literal, $title:literal, $v1:expr;)*) => {
        $(pub const $name: FindingKind = FindingKind {
            gates: &[$($gate),+],
            code: $code,
            title: $title,
            v1: $v1,
        };)*
        /// Every kind, in declaration order.
        pub const FINDINGS: &[&FindingKind] = &[$(&$name),*];
    };
}

use V1::{Message, Same, Site, Was};

findings! {
    // Source parsing: reported by the first enabled of the four AST gates.
    SOURCE_PARSED_WITH_ERRORS_PREPROCESSOR = ["assertion-reduction", "vacuous-tests", "ignored-tests", "unsafe-safety-comment"], "source-parsed-with-errors-preprocessor", "Source File Parsed With Errors (preprocessor)", Was("Preprocessor or Syntax Parse Warning");
    SOURCE_PARSED_WITH_ERRORS = ["assertion-reduction", "vacuous-tests", "ignored-tests", "unsafe-safety-comment"], "source-parsed-with-errors", "Source File Parsed With Errors", Was("Source File Could Not Be Fully Parsed");
    NUL_BYTE_ADDED = ["assertion-reduction", "vacuous-tests", "ignored-tests", "unsafe-safety-comment"], "nul-byte-added", "NUL Byte Added To Source File", Was("Source File Contains Newly Added NUL Byte");

    // assertion-reduction
    ASSERTION_BOUND_LOOSENED = ["assertion-reduction"], "assertion-bound-loosened", "Assertion Bound Loosened", Same;
    MOCKING_INCREASED = ["assertion-reduction"], "mocking-increased-without-stronger-assertions", "Mocking Increased Without Stronger Assertions", Was("Mocking Grew Without Stronger Assertions");
    FATAL_ASSERTIONS_WEAKENED = ["assertion-reduction"], "fatal-assertions-weakened", "Fatal Assertions Weakened To Non-Fatal", Was("Fatal Assertions Weakened to Non-Fatal");
    ASSERTIONS_REDUCED = ["assertion-reduction"], "assertions-reduced", "Assertion Count Decreased In Existing Test", Was("Assertion Reduction In Existing Test");

    // vacuous-tests
    ASSERTS_ONLY_ON_MOCKS = ["vacuous-tests"], "asserts-only-on-mocks", "Test Asserts Only On Mocks", Same;
    ASSERTS_ONLY_TRIVIAL = ["vacuous-tests"], "asserts-only-trivial-properties", "Test Asserts Only Trivial Properties", Same;
    VACUOUS_TEST_ADDED = ["vacuous-tests"], "vacuous-test-added", "Vacuous Test Added", Same;
    ASSERTION_DENSITY_BELOW_FLOOR = ["vacuous-tests"], "assertion-density-below-floor", "Insufficient Assertion Density", Same;

    // ignored-tests
    SKIP_JUSTIFICATION_INSUFFICIENT = ["ignored-tests"], "skip-justification-insufficient", "Skip Justification Insufficient", Was("Unannotated Skip Justification");
    IGNORED_TEST_ADDED = ["ignored-tests"], "ignored-test-added", "Ignored Test Added", Was("Test Arrives Ignored");
    EXISTING_TEST_SKIPPED = ["ignored-tests"], "existing-test-skipped", "Existing Test Skipped", Was("Test Newly Skipped");
    TEST_SLEEP_ADDED = ["ignored-tests"], "test-sleep-added", "Test Sleep Added", Was("Test Sleeps");
    TEST_RETRY_ADDED = ["ignored-tests"], "test-retry-added", "Test Retry Added", Was("Test Retries On Failure");
    TEST_CONDITIONALLY_SKIPPED = ["ignored-tests"], "test-conditionally-skipped", "Test Conditionally Skipped", Same;

    // unsafe-safety-comment
    SAFETY_COMMENT_MISSING = ["unsafe-safety-comment"], "safety-comment-missing", "Unsafe Without SAFETY Comment", Same;

    // deletion-rationale
    FILE_DELETED_WITHOUT_RATIONALE = ["deletion-rationale"], "file-deleted-without-rationale", "File Deleted Without Rationale", Same;
    TEST_REMOVED_WITHOUT_RATIONALE = ["deletion-rationale"], "test-removed-without-rationale", "Test Removed Without Rationale", Same;

    // agents-md
    AGENTS_MD_MISSING = ["agents-md"], "agents-md-missing", "AGENTS.md Missing", Was("Missing AGENTS.md");
    AGENT_GUIDE_FORKED = ["agents-md"], "agent-guide-forked", "Forked Agent Guide", Same;

    // time-estimates
    TIME_ESTIMATE = ["time-estimates"], "time-estimate", "Time Estimate", Same;

    // pii
    HOST_OR_PII_LEAK = ["pii"], "host-or-pii-leak", "Host / PII Leak", Same;

    // agent-scratch
    AGENT_SCRATCH_TRACKED = ["agent-scratch"], "agent-scratch-tracked", "Tracked Agent Scratch State", Same;

    // shell-secrets
    SHELL_ARGV_ENV = ["shell-secrets"], "argv-env", "Unsafe Shell Pattern (ARGV-ENV)", Was("Unsafe Shell Pattern: ARGV-ENV");
    SHELL_ARGV_DOCKER = ["shell-secrets"], "argv-docker", "Unsafe Shell Pattern (ARGV-DOCKER)", Was("Unsafe Shell Pattern: ARGV-DOCKER");
    SHELL_ARGV_INLINE = ["shell-secrets"], "argv-inline", "Unsafe Shell Pattern (ARGV-INLINE)", Was("Unsafe Shell Pattern: ARGV-INLINE");
    SHELL_INJECT_XARGS = ["shell-secrets"], "inject-xargs", "Unsafe Shell Pattern (INJECT-XARGS)", Was("Unsafe Shell Pattern: INJECT-XARGS");
    SHELL_INJECT_PIPE = ["shell-secrets"], "inject-pipe", "Unsafe Shell Pattern (INJECT-PIPE)", Was("Unsafe Shell Pattern: INJECT-PIPE");
    SECRET_GITHUB_TOKEN = ["shell-secrets"], "github-token", "Hardcoded Secret (GitHub Token)", Was("Hardcoded Secret: GitHub Token");
    SECRET_AWS_ACCESS_KEY = ["shell-secrets"], "aws-access-key", "Hardcoded Secret (AWS Access Key)", Was("Hardcoded Secret: AWS Access Key");
    SECRET_SLACK_TOKEN = ["shell-secrets"], "slack-token", "Hardcoded Secret (Slack Token)", Was("Hardcoded Secret: Slack Token");
    SECRET_LLM_API_TOKEN = ["shell-secrets"], "llm-api-token", "Hardcoded Secret (OpenAI / Anthropic Token)", Was("Hardcoded Secret: OpenAI / Anthropic Token");
    SECRET_PRIVATE_KEY_BLOCK = ["shell-secrets"], "private-key-block", "Hardcoded Secret (Private Key Block)", Was("Hardcoded Secret: Private Key Block");
    SECRET_BEARER_TOKEN = ["shell-secrets"], "authorization-bearer-token", "Hardcoded Secret (Authorization Bearer Token)", Was("Hardcoded Secret: Authorization Bearer Token");
    SECRET_PASSWORD_FLAG = ["shell-secrets"], "password-flag", "Hardcoded Secret (Command-Line Password Flag)", Was("Hardcoded Secret: Command-Line Password Flag");
    SECRET_CREDENTIAL_ASSIGNMENT = ["shell-secrets"], "credential-assignment", "Hardcoded Secret (Literal Credential Assignment)", Was("Hardcoded Secret: Literal Credential Assignment");

    // issue-link
    DIRECTIVE_IN_SUBJECT_LINE = ["issue-link"], "directive-in-subject-line", "Directive In Subject Line", Was("Directive in Subject Line");
    ISSUE_LINK_MISSING_IN_COMMIT_MESSAGE = ["issue-link"], "issue-link-missing-in-commit-message", "Tracking Issue Link Missing In Commit Message", Was("Missing Tracking Issue Link in Commit Message");
    ISSUE_LINK_MISSING_IN_COMMITS = ["issue-link"], "issue-link-missing-in-commits", "Tracking Issue Link Missing In Commits", Was("Missing Tracking Issue Link in Commits");
    ISSUE_LINK_MISSING = ["issue-link"], "issue-link-missing", "Tracking Issue Link Missing", Was("Missing Tracking Issue Link");

    // commit-provenance
    COMMIT_TRAILER_MISSING = ["commit-provenance"], "commit-trailer-missing", "Commit Trailer Missing", Same;
    AGENT_COMMIT_WITHOUT_REVIEW = ["commit-provenance"], "agent-commit-without-review", "Agent Commit Without Review", Same;
    AGENT_COMMIT_REVIEWED_BY_AUTHOR = ["commit-provenance"], "agent-commit-reviewed-by-author", "Agent Commit Reviewed By Its Author", Same;

    // config-integrity
    GATE_WEAKENED = ["config-integrity"], "gate-weakened", "Gate Weakened By This Change", Same;
    BASE_CONFIGURATION_UNREADABLE = ["config-integrity"], "base-configuration-unreadable", "Base Configuration Unreadable", Same;
    BASELINE_NEW_FINDINGS = ["config-integrity"], "baseline-new-findings", "Baseline Contains New Findings", Was("Baseline Contains New Findings Without Directive");
    BASELINE_INCREASED = ["config-integrity"], "baseline-increased", "Baseline Increased", Was("Baseline Grew Without Directive");
    BASELINE_MIGRATION_NOT_ALONE = ["config-integrity"], "baseline-migration-not-alone", "Baseline Migration Mixed With Other Changes", Same;

    // golden-output
    SNAPSHOT_ADDED_FOR_EXISTING_TEST = ["golden-output"], "snapshot-added-for-existing-test", "Snapshot Added For Existing Test", Same;
    GOLDEN_REGENERATED_WITHOUT_SOURCE_CHANGE = ["golden-output"], "golden-output-regenerated-without-source-change", "Golden Output Regenerated Without Source Change", Same;
    GOLDEN_CHANGED_WITHOUT_DIRECTIVE = ["golden-output"], "golden-output-changed-without-directive", "Golden Output Changed", Was("Golden Output Modified Without Directive");

    // stub-bodies
    STUB_BODY_ADDED = ["stub-bodies"], "stub-body-added", "Stub Body Added", Same;
    BODY_REPLACED_BY_STUB = ["stub-bodies"], "body-replaced-by-stub", "Function Body Replaced By Stub", Same;

    // error-swallowing
    RESULT_DISCARDED = ["error-swallowing"], "result-discarded", "Result Discarded", Same;
    VALUE_DISCARDED = ["error-swallowing"], "value-discarded", "Value Discarded", Same;
    EMPTY_ERROR_HANDLER_ADDED = ["error-swallowing"], "empty-error-handler-added", "Empty Error Handler Added", Same;
    ERROR_LOGGED_AND_DROPPED = ["error-swallowing"], "error-logged-and-dropped", "Error Logged And Dropped", Was("Empty Error Handler Added");
    UNPARSEABLE_INPUT_SKIPPED = ["error-swallowing"], "unparseable-input-skipped", "Unparseable Input Skipped", Same;
    ERROR_SILENCED = ["error-swallowing"], "error-silenced", "Error Silenced", Same;

    // instruction-smuggling
    AGENT_INSTRUCTIONS_CHANGED = ["instruction-smuggling"], "agent-instructions-changed", "Agent Instructions Changed", Same;
    INVISIBLE_CHARACTERS_ADDED = ["instruction-smuggling"], "invisible-characters-added", "Invisible Characters Added", Same;
    INSTRUCTION_LIKE_TEXT_ADDED = ["instruction-smuggling"], "instruction-like-text-added", "Instruction-Like Text Added", Same;
    INVISIBLE_CHARACTERS_IN_DESCRIPTION = ["instruction-smuggling"], "invisible-characters-in-description", "Invisible Characters In Change Description", Same;
    INSTRUCTION_LIKE_TEXT_IN_DESCRIPTION = ["instruction-smuggling"], "instruction-like-text-in-description", "Instruction-Like Text In Change Description", Same;

    // build-hooks
    INSTALL_HOOK_ADDED = ["build-hooks"], "install-hook-added", "Install Hook Added", Same;
    INSTALL_HOOK_NETWORK_OR_SHELL = ["build-hooks"], "install-hook-network-or-shell", "Install Hook Runs Network Or Shell", Same;
    BUILD_SCRIPT_ADDED = ["build-hooks"], "build-script-added", "Build Script Added", Same;
    BUILD_SCRIPT_NETWORK_OR_SHELL = ["build-hooks"], "build-script-network-or-shell", "Build Script Gains Network Or Shell Access", Same;
    PACKAGE_MANAGER_CONFIG_CHANGED = ["build-hooks"], "package-manager-config-changed", "Package Manager Configuration Changed", Same;

    // toolchain-config
    TOOLCHAIN_CHANGE_NOT_ANALYSED = ["toolchain-config"], "toolchain-config-change-not-analysed", "Toolchain Configuration Change Not Analysed", Was("Toolchain Configuration Changed (not analysed)");
    TOOLCHAIN_CONFIG_DELETED = ["toolchain-config"], "toolchain-config-deleted", "Toolchain Configuration Deleted", Same;
    TOOLCHAIN_CONFIG_UNREADABLE = ["toolchain-config"], "toolchain-config-unreadable", "Toolchain Configuration Unreadable", Same;
    TOOLCHAIN_CONFIG_WEAKENED = ["toolchain-config"], "toolchain-config-weakened", "Toolchain Configuration Weakened", Same;

    // scope-confinement
    FILE_IN_FORBIDDEN_SCOPE = ["scope-confinement"], "file-in-forbidden-scope", "File In Forbidden Scope", Message;
    FILE_OUTSIDE_AUTHORIZED_SCOPE = ["scope-confinement"], "file-outside-authorized-scope", "File Outside Authorized Scope", Message;

    // suppression-delta
    SUPPRESSION_ADDED = ["suppression-delta"], "suppression-added", "Suppression Added", Message;

    // provenance-tags
    TABLE_NUMERICS_UNPROVENANCED = ["provenance-tags"], "table-numerics-unprovenanced", "Unprovenanced Table Numerics", Same;
    MECHANISM_CLAIM_WITHOUT_EVIDENCE = ["provenance-tags"], "mechanism-claim-without-evidence", "Mechanism Claim Without Evidence", Same;
    WALL_CLOCK_RATIO_WITHOUT_INTERVAL = ["provenance-tags"], "wall-clock-ratio-without-interval", "Bare Wall-Clock Ratio Without Interval", Same;
    PAIRED_FIGURES_WITHOUT_WORKLOAD_TAG = ["provenance-tags"], "paired-figures-without-workload-tag", "Paired Figures Without Workload Tag", Same;
    CROSS_METRIC_FIGURES_WITHOUT_WORKLOAD_TAG = ["provenance-tags"], "cross-metric-figures-without-workload-tag", "Cross-Metric Figures Without Workload Tag", Same;
    PENDING_MEASUREMENT_WITHOUT_OPEN_ISSUE = ["provenance-tags"], "pending-measurement-without-open-issue", "Pending Measurement Without Open Issue", Same;
    SUPERSEDED_FIGURE_REPUBLISHED = ["provenance-tags"], "superseded-figure-republished", "Superseded Figure Republished", Same;

    // ci-integrity
    VERIFICATION_WORKFLOW_DELETED = ["ci-integrity"], "verification-workflow-deleted", "Verification Workflow Deleted", Was("Deletion of Verification Workflow");
    ROLLUP_NEEDS_REMOVED = ["ci-integrity"], "rollup-needs-removed", "Rollup Job Needs Entry Removed", Was("Rollup Job Dropped Dependency");
    ROLLUP_NEEDS_INCOMPLETE = ["ci-integrity"], "rollup-needs-incomplete", "Rollup Job Needs Incomplete", Was("Incomplete Rollup Job Needs");
    JOB_COUNT_FILE_MISSING = ["ci-integrity"], "job-count-file-missing", "Documented Job Count File Missing", Same;
    JOB_COUNT_MISMATCH = ["ci-integrity"], "job-count-mismatch", "Documented Job Count Mismatch", Same;
    PULL_REQUEST_TARGET_TRIGGER = ["ci-integrity"], "pull-request-target-trigger", "Dangerous Trigger (pull_request_target)", Was("Dangerous pull_request_target Trigger");
    WORKFLOW_PERMISSIONS_WIDENED = ["ci-integrity"], "workflow-permissions-widened", "Workflow Permissions Widened", Same;
    WORKFLOW_TIMEOUT_REMOVED = ["ci-integrity"], "workflow-timeout-removed", "Workflow Timeout Removed (timeout-minutes)", Was("Workflow timeout-minutes Removed");
    VERIFICATION_JOB_REMOVED = ["ci-integrity"], "verification-job-removed", "Verification Job Removed", Was("Deletion of Verification Job");
    JOB_TIMEOUT_REMOVED = ["ci-integrity"], "job-timeout-removed", "Job Timeout Removed (timeout-minutes)", Was("Job timeout-minutes Removed");
    VERIFICATION_STEP_REMOVED = ["ci-integrity"], "verification-step-removed", "Verification Step Removed", Was("Deletion of Verification Step");
    JOB_FAILURE_MASKED_CONTINUE_ON_ERROR = ["ci-integrity"], "job-failure-masked-continue-on-error", "Verification Job Failure Masked (continue-on-error)", Was("continue-on-error Masks Failure");
    STEP_FAILURE_MASKED_CONTINUE_ON_ERROR = ["ci-integrity"], "step-failure-masked-continue-on-error", "Verification Step Failure Masked (continue-on-error)", Was("continue-on-error Masks Failure");
    VERIFICATION_JOB_MASKED_BY_CONDITION = ["ci-integrity"], "verification-job-masked-by-condition", "Verification Job Masked By Condition", Was("Conditional Masking on Verification Job");
    UNPINNED_ACTION = ["ci-integrity"], "unpinned-action", "Unpinned Third-Party Action", Same;
    DISCIPLINE_ACTION_POLICY_FROM = ["ci-integrity"], "discipline-action-policy-from-weakened", "Discipline Action Weakened (policy_from)", Same;
    DISCIPLINE_ACTION_DISABLE_INPUT = ["ci-integrity"], "discipline-action-disable-input", "Discipline Action Weakened (disable input)", Same;
    DISCIPLINE_ACTION_ADVISORY = ["ci-integrity"], "discipline-action-advisory", "Discipline Action Weakened (advisory: true)", Same;
    DISCIPLINE_ACTION_FAIL_ON_WARNINGS_OFF = ["ci-integrity"], "discipline-action-fail-on-warnings-off", "Discipline Action Weakened (fail_on_warnings: false)", Same;
    DISCIPLINE_ACTION_CONFIG_OVERRIDE_INVALID = ["ci-integrity"], "discipline-action-config-override-invalid", "Discipline Action Input Invalid (config_override)", Was("Discipline Action Invalid config_override");
    DISCIPLINE_ACTION_SUITE_CHANGED = ["ci-integrity"], "discipline-action-suite-changed", "Discipline Action Suite Changed", Same;
    DISCIPLINE_ACTION_DIRECTIVE_SOURCES_WIDENED = ["ci-integrity"], "discipline-action-directive-sources-widened", "Discipline Action Directive Sources Widened", Same;
    DISCIPLINE_RUN_ADVISORY = ["ci-integrity"], "discipline-run-advisory", "Discipline Run Weakened (--advisory)", Same;
    COMPILER_DENY_WARNINGS_REMOVED = ["ci-integrity"], "compiler-deny-warnings-removed", "Compiler Flag Removed (-D warnings)", Was("Compiler Flag Dropped (-D warnings)");
    CARGO_LOCKED_REMOVED = ["ci-integrity"], "cargo-locked-removed", "Cargo Flag Removed (--locked)", Was("Cargo Flag Dropped (--locked)");
    FROZEN_INSTALL_FLAG_REMOVED = ["ci-integrity"], "frozen-install-flag-removed", "Frozen Install Flag Removed", Was("Frozen Install Flag Dropped");
    INSTALL_COMMAND_WEAKENED = ["ci-integrity"], "install-command-weakened", "Install Command Weakened", Was("Install Command Softened");
    CLIPPY_ALL_TARGETS_REMOVED = ["ci-integrity"], "clippy-all-targets-removed", "Clippy Flag Removed (--all-targets)", Was("Clippy Flag Dropped (--all-targets)");
    VERIFICATION_STEP_MASKED_BY_CONDITION = ["ci-integrity"], "verification-step-masked-by-condition", "Verification Step Masked By Condition", Was("Conditional Masking on Verification Step");
    VERIFICATION_STEP_NARROWED = ["ci-integrity"], "verification-step-narrowed", "Verification Step Narrowed", Same;
    EXIT_CODE_MASKED = ["ci-integrity"], "exit-code-masked", "Command Exit Code Masked", Was("Command Masks Exit Code");
    PIPELINE_FILE_UNREADABLE = ["ci-integrity"], "pipeline-file-unreadable", "Pipeline File Unreadable", Same;
    JOB_FAILURE_MASKED_ALLOW_FAILURE = ["ci-integrity"], "job-failure-masked-allow-failure", "Verification Job Failure Masked (allow_failure)", Was("allow_failure Masks Failure");
    VERIFICATION_JOB_MADE_MANUAL = ["ci-integrity"], "verification-job-made-manual", "Verification Job Made Manual", Same;
    VERIFICATION_JOB_NARROWED = ["ci-integrity"], "verification-job-narrowed", "Verification Job Narrowed", Same;

    // ci-skip-set
    CHANGE_JOB_MISSING_FROM_NEEDS = ["ci-skip-set"], "change-job-missing-from-needs", "Change-Detection Job Missing From Needs", Site;
    CHANGE_JOB_DID_NOT_SUCCEED = ["ci-skip-set"], "change-job-did-not-succeed", "Change-Detection Job Did Not Succeed", Site;
    UNCONDITIONAL_JOB_MISSING_FROM_NEEDS = ["ci-skip-set"], "unconditional-job-missing-from-needs", "Unconditional Job Missing From Needs", Site;
    UNCONDITIONAL_JOB_SKIPPED = ["ci-skip-set"], "unconditional-job-skipped", "Unconditional Job Skipped", Site;
    NEEDS_NAMES_UNDEFINED_JOB = ["ci-skip-set"], "needs-names-undefined-job", "Undefined Job In Needs", Site;
    SKIP_DECISION_UNVERIFIABLE = ["ci-skip-set"], "skip-decision-unverifiable", "Skip Decision Unverifiable", Site;
    JOB_SKIPPED_WHILE_CONDITION_TRUE = ["ci-skip-set"], "job-skipped-while-condition-true", "Job Skipped While Condition True", Site;
    JOB_RAN_WHILE_CONDITION_FALSE = ["ci-skip-set"], "job-ran-while-condition-false", "Job Ran While Condition False", Site;

    // test-floor
    FLOOR_CONSTANT_MISSING_IN_BASE = ["test-floor"], "floor-constant-missing-in-base", "Floor Constant Missing In Base Ref", Was("Floor Constant Missing in Base Ref");
    FLOOR_CONSTANT_FILE_MISSING_IN_BASE = ["test-floor"], "floor-constant-file-missing-in-base", "Floor Constant File Missing In Base Ref", Was("Floor Constant File Missing in Base Ref");
    FLOOR_CONSTANT_DECREASED = ["test-floor"], "floor-constant-decreased", "Floor Constant Decreased", Same;
    CONFIGURED_FLOOR_DECREASED = ["test-floor"], "configured-floor-decreased", "Configured Test Floor Decreased", Same;
    REQUIRED_SUITE_MISSING = ["test-floor"], "required-suite-missing", "Required Test Suite Missing", Same;
    TEST_COUNT_BELOW_FLOOR = ["test-floor"], "test-count-below-floor", "Test Count Below Floor", Same;

    // dependency-delta
    LOCKFILE_DELETED = ["dependency-delta"], "lockfile-deleted", "Lockfile Deleted", Same;
    LOCKFILE_ENTRY_FROM_NEW_SOURCE = ["dependency-delta"], "lockfile-entry-from-new-source", "Lockfile Entry From New Source", Same;
    LOCKFILE_INTEGRITY_HASH_REMOVED = ["dependency-delta"], "lockfile-integrity-hash-removed", "Lockfile Integrity Hash Removed", Was("Lockfile Integrity Hash Dropped");
    MANIFEST_CHANGED_WITHOUT_LOCKFILE = ["dependency-delta"], "manifest-changed-without-lockfile", "Manifest Changed Without Lockfile", Same;
    DIRECT_DEPENDENCY_ADDED = ["dependency-delta"], "direct-dependency-added", "Direct Dependency Added", Was("New Direct Dependency Added");
    DEPENDENCY_CONSTRAINT_LOOSENED = ["dependency-delta"], "dependency-constraint-loosened", "Dependency Constraint Loosened", Was("Loosened Dependency Constraint");
    DEPENDENCY_SOURCE_CHANGED = ["dependency-delta"], "dependency-source-changed", "Dependency Source Changed", Was("Dependency Source Modified");
    WILDCARD_DEPENDENCY_VERSION = ["dependency-delta"], "wildcard-dependency-version", "Wildcard Dependency Version", Same;
    UNPINNED_GIT_DEPENDENCY = ["dependency-delta"], "unpinned-git-dependency", "Unpinned Git Dependency", Same;
    BANNED_DEPENDENCY = ["dependency-delta"], "banned-dependency", "Banned Dependency", Same;
    DEPENDENCY_OUTSIDE_ALLOWLIST = ["dependency-delta"], "dependency-outside-allowlist", "Dependency Outside Allowlist (allow_dependencies)", Was("Dependency Outside Allowlist");
    DEPENDENCY_OUTSIDE_DENY_ALLOWLIST = ["dependency-delta"], "dependency-outside-deny-allowlist", "Dependency Outside Allowlist (deny.toml)", Was("Dependency Outside Allowlist");
    UNAUTHORIZED_GIT_SOURCE = ["dependency-delta"], "unauthorized-git-source", "Unauthorized Git Repository Source", Same;

    // test-budget
    FUZZ_TARGET_REMOVED = ["test-budget"], "fuzz-target-removed", "Fuzz Target Removed", Same;
    FUZZ_TARGET_DELETED = ["test-budget"], "fuzz-target-deleted", "Fuzz Target Deleted", Same;
    TEST_BUDGET_DECREASED = ["test-budget"], "test-budget-decreased", "Test Budget Decreased", Was("Test Budget Reduced");
    SEED_CORPUS_DECREASED = ["test-budget"], "seed-corpus-decreased", "Seed Corpus Size Decreased", Was("Seed Corpus Shrunk");

    // pr-checklist
    CHECKLIST_CLAIMS_TESTS = ["pr-checklist"], "checklist-claims-tests", "Checklist Claim Unsupported (tests)", Message;
    CHECKLIST_CLAIMS_DOCS = ["pr-checklist"], "checklist-claims-docs", "Checklist Claim Unsupported (docs)", Message;
    CHECKLIST_CLAIMS_BENCHMARKS = ["pr-checklist"], "checklist-claims-benchmarks", "Checklist Claim Unsupported (benchmarks)", Message;
    CHECKLIST_CLAIM_UNSUPPORTED = ["pr-checklist"], "checklist-claim-unsupported", "Checklist Claim Unsupported", Message;

    // command
    UNTRUSTED_COMMAND_MODIFICATION = ["command"], "untrusted-command-modification", "Untrusted Command Modification", Same;
    POLICY_FILE_DELETED = ["command"], "policy-file-deleted", "Policy File Deleted", Same;
    CANARY_DIAGNOSTIC_MISSING = ["command"], "canary-diagnostic-missing", "Canary Diagnostic Missing", Same;
    CANARY_COMMAND_SUCCEEDED = ["command"], "canary-command-succeeded", "Canary Command Succeeded", Same;
    COMMAND_FAILED = ["command"], "command-failed", "Command Failed", Was("Command Exited With Error");
    FORBIDDEN_OUTPUT = ["command"], "forbidden-output", "Forbidden Output Detected", Same;
    ZERO_ITEMS_EXECUTED = ["command"], "zero-items-executed", "Zero Items Selected Or Executed", Same;
    COUNT_BELOW_RATCHET = ["command"], "count-below-ratchet", "Command Count Below Ratchet Floor", Was("Count Ratchet Regression");
    COUNT_PATTERN_UNMATCHED = ["command"], "count-pattern-unmatched", "Count Pattern Unmatched", Was("Count Pattern Did Not Match");

    // sanitizers
    SANITIZER_CANARY_DIAGNOSTIC_MISSING = ["sanitizers"], "canary-diagnostic-missing", "Canary Diagnostic Missing", Message;
    SANITIZER_VIOLATION_DETECTED = ["sanitizers"], "violation-detected", "Sanitizer Violation Detected", Message;

    // msrv
    MSRV_DECLARATION_MISSING = ["msrv"], "msrv-declaration-missing", "MSRV Declaration Missing", Message;
    MSRV_COMMAND_FAILED = ["msrv"], "msrv-command-failed", "MSRV Command Failed", Message;

    // miri
    MIRI_ZERO_TESTS_EXECUTED = ["miri"], "zero-tests-executed", "Zero Tests Executed", Message;
    MIRI_UNDEFINED_BEHAVIOR = ["miri"], "undefined-behavior-detected", "Undefined Behavior Detected", Message;

    // unsafe-budget
    UNSAFE_BUDGET_EXCEEDED = ["unsafe-budget"], "budget-exceeded", "Unsafe Budget Exceeded", Message;
    UNSAFE_ADDED_WITHOUT_AUTHORIZATION = ["unsafe-budget"], "unsafe-added-without-authorization", "Unsafe Code Added", Message;
    UNSAFE_COUNT_INCREASED = ["unsafe-budget"], "unsafe-count-increased", "Unsafe Count Increased", Message;

    // bench-regression
    BENCHMARK_ARTIFACT_DELETED = ["bench-regression"], "benchmark-artifact-deleted", "Benchmark Artifact Deleted", Same;
    NEW_ARTIFACT_BASELINE_MISSING = ["bench-regression"], "new-artifact-baseline-missing", "Benchmark Baseline Missing For New Artifact", Was("New Benchmark Artifact Lacks Baseline");
    BENCHMARK_PROVENANCE_MISMATCH = ["bench-regression"], "benchmark-provenance-mismatch", "Benchmark Provenance Mismatch", Was("Mismatched Benchmark Provenance");
    CROSS_HOST_COMPARISON = ["bench-regression"], "cross-host-comparison", "Cross-Host Benchmark Comparison Mismatch", Same;
    BENCHMARK_BASELINE_MISSING = ["bench-regression"], "benchmark-baseline-missing", "Benchmark Baseline Missing", Was("Missing Benchmark Baseline");
    OVERRIDE_VOID_CITATION_DOES_NOT_MEASURE = ["bench-regression"], "override-void-citation-does-not-measure", "Regression Override Void (Citation Does Not Measure This Code)", Was("Regression Override Is Void — Citation Does Not Measure This Code");
    OVERRIDE_UNVERIFIED_CITATION_UNDECIDABLE = ["bench-regression"], "override-unverified-citation-undecidable", "Regression Override Unverified (Citation Undecidable)", Was("Regression Override Not Verified — Citation Undecidable");
    COUNTER_REGRESSED = ["bench-regression"], "counter-regressed", "Deterministic Counter Regressed", Was("Instruction Count Regressed");
    BENCHMARK_REMOVED = ["bench-regression"], "benchmark-removed", "Benchmark Removed", Same;
    NEW_OR_RENAMED_ARM_BASELINE_MISSING = ["bench-regression"], "new-or-renamed-arm-baseline-missing", "Benchmark Baseline Missing For New Or Renamed Arm", Was("New or Renamed Benchmark Lacks Baseline");
    PERFORMANCE_REGRESSED = ["bench-regression"], "performance-regressed", "Benchmark Performance Regressed", Same;
    OVERRIDE_VOID_NO_RESOLVABLE_CITATION = ["bench-regression"], "override-void-no-resolvable-citation", "Regression Override Void (No Resolvable Citation)", Was("Regression Override Is Void — No Resolvable Citation");
    OVERRIDE_VOID_NAMES_NO_REGRESSED_ARM = ["bench-regression"], "override-void-names-no-regressed-arm", "Regression Override Void (Names No Regressed Arm)", Was("Regression Override Is Void — Names No Regressed Arm");
    COUNTER_REGRESSED_UNAPPROVED_ARM = ["bench-regression"], "counter-regressed-unapproved-arm", "Deterministic Counter Regressed (Unapproved Arm)", Was("Instruction Count Regressed (Unapproved Arm)");
    STALE_ARM_EXEMPTION = ["bench-regression"], "stale-arm-exemption", "Stale Benchmark Arm Exemption", Same;
    PAIRED_RATIO_NOT_COMPARABLE = ["bench-regression"], "paired-ratio-not-comparable", "Paired Ratio Not Comparable", Same;
    PAIRED_RATIO_CELL_MISSING = ["bench-regression"], "paired-ratio-cell-missing", "Paired Ratio Cell Missing", Same;
    PAIRED_RATIO_INCONSISTENT_WITH_ROUNDS = ["bench-regression"], "paired-ratio-inconsistent-with-rounds", "Paired Ratio Inconsistent With Rounds", Was("Paired Ratio Disagrees With Its Rounds");
    PAIRED_RATIO_REGRESSED = ["bench-regression"], "paired-ratio-regressed", "Paired Ratio Regressed", Same;
    RATIO_BASELINE_LOOSENED = ["bench-regression"], "ratio-baseline-loosened", "Ratio Baseline Loosened", Same;
    RATIO_BASELINE_CHANGED_WITH_SOURCE = ["bench-regression"], "ratio-baseline-changed-with-source", "Ratio Baseline Changed With Source", Same;
    PAIRED_RATIO_RUN_MISSING = ["bench-regression"], "paired-ratio-run-missing", "Paired Ratio Run Missing", Same;

    // archive-contents
    REQUIRED_ARCHIVE_PATH_MISSING = ["archive-contents"], "required-path-missing", "Required Archive Path Missing", Was("Missing Required Archive Path");
    FORBIDDEN_ARCHIVE_ENTRY = ["archive-contents"], "forbidden-entry", "Forbidden Entry In Archive", Was("Forbidden Entry Found in Archive");
    SOURCE_LEAKED_IN_ARCHIVE = ["archive-contents"], "source-leaked", "Source Leaked In Archive", Same;
    SOURCE_MAP_SHIPPED = ["archive-contents"], "source-map-shipped", "Source Map Shipped", Same;

    // manifest-sync
    MANIFEST_DRIFT = ["manifest-sync"], "manifest-drift", "Manifest Synchronization Drift", Same;

    // version-lockstep
    VERSION_MISMATCH = ["version-lockstep"], "version-mismatch", "Version Declaration Lockstep Mismatch", Same;

    // A run that could not complete (the pseudo-gate `engine`, reported only in `--json-out`).
    ENGINE_COULD_NOT_RUN = ["engine"], "could-not-run", "Check Could Not Run", Message;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_kebab(s: &str) -> bool {
        !s.is_empty()
            && !s.starts_with('-')
            && !s.ends_with('-')
            && !s.contains("--")
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    }

    #[test]
    fn codes_are_kebab_case_and_unique_within_each_gate() {
        let mut seen = std::collections::HashSet::new();
        for k in FINDINGS {
            assert!(is_kebab(k.code), "`{}` is not kebab-case", k.code);
            for g in k.gates {
                assert!(
                    seen.insert((*g, k.code)),
                    "`{g}/{}` is registered twice",
                    k.code
                );
            }
        }
    }

    /// The title grammar (docs/GATES.md, "Finding Codes").
    #[test]
    fn titles_follow_the_grammar() {
        let mut bad = Vec::new();
        let mut per_gate = std::collections::HashSet::new();
        for k in FINDINGS {
            let t = k.title;
            let outside = t.split('(').next().unwrap_or(t).trim_end();
            let capitalised = outside.split_whitespace().all(|w| {
                w.chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '/')
            });
            let closed = !t.contains('(') || (t.matches('(').count() == 1 && t.ends_with(')'));
            if !t.is_ascii() || t.contains('{') || !capitalised || !closed || t.is_empty() {
                bad.push(format!("{}/{}: `{t}`", k.gates[0], k.code));
            }
            if !per_gate.insert((k.gates[0], t)) {
                bad.push(format!("{}: `{t}` twice", k.gates[0]));
            }
        }
        assert_eq!(bad, Vec::<String>::new());
    }

    #[test]
    fn a_finding_several_gates_report_is_coded_under_its_first_gate() {
        for gate in SOURCE_PARSED_WITH_ERRORS.gates {
            assert_eq!(
                full_code(gate, &SOURCE_PARSED_WITH_ERRORS),
                "assertion-reduction/source-parsed-with-errors"
            );
        }
        assert_eq!(
            full_code("ci-integrity", &UNPINNED_ACTION),
            "ci-integrity/unpinned-action"
        );
    }

    #[test]
    fn every_shipped_gate_has_a_registered_finding() {
        for g in crate::config::GATES.iter().filter(|g| g.available) {
            assert!(
                FINDINGS.iter().any(|k| k.gates.contains(&g.id)),
                "gate `{}` reports nothing registered in src/findings.rs",
                g.id
            );
        }
    }

    #[test]
    fn every_gate_named_is_registered() {
        let unknown: Vec<String> = FINDINGS
            .iter()
            .flat_map(|k| k.gates.iter().map(move |g| (*g, k.code)))
            .filter(|(g, _)| *g != "engine" && crate::config::gate_info(g).is_none())
            .map(|(g, code)| format!("{g}/{code}"))
            .collect();
        assert_eq!(unknown, Vec::<String>::new());
    }
}
