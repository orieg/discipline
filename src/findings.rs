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

/// How a kind's title is produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Title {
    /// A fixed title.
    Fixed(&'static str),
    /// The title is built at the reporting site (it carries data, or repeats the
    /// message). Kept so existing baselines, which fingerprint the title, still match;
    /// such titles become fixed once fingerprints key on the code.
    Legacy,
}

/// One kind of finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindingKind {
    /// The gates that may report it. One, except for source-parse findings, which the
    /// first enabled AST gate reports.
    pub gates: &'static [&'static str],
    /// Kebab-case code, unique within each of its gates.
    pub code: &'static str,
    pub title: Title,
}

impl FindingKind {
    /// The fixed title. Panics for a [`Title::Legacy`] kind, whose sites supply one.
    pub fn fixed_title(&self) -> &'static str {
        match self.title {
            Title::Fixed(t) => t,
            Title::Legacy => panic!("finding `{}` has no fixed title", self.code),
        }
    }
}

/// `gate/code` for a finding built as a [`crate::guards::Violation`] literal.
pub fn full_code(gate: &str, kind: &FindingKind) -> String {
    assert!(
        kind.gates.contains(&gate),
        "gate `{gate}` reports `{}`, registered for {:?}",
        kind.code,
        kind.gates
    );
    format!("{gate}/{}", kind.code)
}

macro_rules! findings {
    ($($name:ident = [$($gate:literal),+], $code:literal, $title:expr;)*) => {
        $(pub const $name: FindingKind = FindingKind {
            gates: &[$($gate),+],
            code: $code,
            title: $title,
        };)*
        /// Every kind, in declaration order.
        pub const FINDINGS: &[&FindingKind] = &[$(&$name),*];
    };
}

use Title::{Fixed, Legacy};

findings! {
    // Source parsing: reported by the first enabled of the four AST gates.
    SOURCE_PARSED_WITH_ERRORS_PREPROCESSOR = ["assertion-reduction", "vacuous-tests", "ignored-tests", "unsafe-safety-comment"], "source-parsed-with-errors-preprocessor", Fixed("Preprocessor or Syntax Parse Warning");
    SOURCE_PARSED_WITH_ERRORS = ["assertion-reduction", "vacuous-tests", "ignored-tests", "unsafe-safety-comment"], "source-parsed-with-errors", Fixed("Source File Could Not Be Fully Parsed");
    NUL_BYTE_ADDED = ["assertion-reduction", "vacuous-tests", "ignored-tests", "unsafe-safety-comment"], "nul-byte-added", Fixed("Source File Contains Newly Added NUL Byte");

    // assertion-reduction
    ASSERTION_BOUND_LOOSENED = ["assertion-reduction"], "assertion-bound-loosened", Fixed("Assertion Bound Loosened");
    MOCKING_INCREASED = ["assertion-reduction"], "mocking-increased-without-stronger-assertions", Fixed("Mocking Grew Without Stronger Assertions");
    FATAL_ASSERTIONS_WEAKENED = ["assertion-reduction"], "fatal-assertions-weakened", Fixed("Fatal Assertions Weakened to Non-Fatal");
    ASSERTIONS_REDUCED = ["assertion-reduction"], "assertions-reduced", Fixed("Assertion Reduction In Existing Test");

    // vacuous-tests
    ASSERTS_ONLY_ON_MOCKS = ["vacuous-tests"], "asserts-only-on-mocks", Fixed("Test Asserts Only On Mocks");
    ASSERTS_ONLY_TRIVIAL = ["vacuous-tests"], "asserts-only-trivial-properties", Fixed("Test Asserts Only Trivial Properties");
    VACUOUS_TEST_ADDED = ["vacuous-tests"], "vacuous-test-added", Fixed("Vacuous Test Added");
    ASSERTION_DENSITY_BELOW_FLOOR = ["vacuous-tests"], "assertion-density-below-floor", Fixed("Insufficient Assertion Density");

    // ignored-tests
    SKIP_JUSTIFICATION_INSUFFICIENT = ["ignored-tests"], "skip-justification-insufficient", Fixed("Unannotated Skip Justification");
    IGNORED_TEST_ADDED = ["ignored-tests"], "ignored-test-added", Fixed("Test Arrives Ignored");
    EXISTING_TEST_SKIPPED = ["ignored-tests"], "existing-test-skipped", Fixed("Test Newly Skipped");
    TEST_SLEEP_ADDED = ["ignored-tests"], "test-sleep-added", Fixed("Test Sleeps");
    TEST_RETRY_ADDED = ["ignored-tests"], "test-retry-added", Fixed("Test Retries On Failure");
    TEST_CONDITIONALLY_SKIPPED = ["ignored-tests"], "test-conditionally-skipped", Fixed("Test Conditionally Skipped");

    // unsafe-safety-comment
    SAFETY_COMMENT_MISSING = ["unsafe-safety-comment"], "safety-comment-missing", Fixed("Unsafe Without SAFETY Comment");

    // deletion-rationale
    FILE_DELETED_WITHOUT_RATIONALE = ["deletion-rationale"], "file-deleted-without-rationale", Fixed("File Deleted Without Rationale");
    TEST_REMOVED_WITHOUT_RATIONALE = ["deletion-rationale"], "test-removed-without-rationale", Fixed("Test Removed Without Rationale");

    // agents-md
    AGENTS_MD_MISSING = ["agents-md"], "agents-md-missing", Fixed("Missing AGENTS.md");
    AGENT_GUIDE_FORKED = ["agents-md"], "agent-guide-forked", Fixed("Forked Agent Guide");

    // time-estimates
    TIME_ESTIMATE = ["time-estimates"], "time-estimate", Fixed("Time Estimate");

    // pii
    HOST_OR_PII_LEAK = ["pii"], "host-or-pii-leak", Fixed("Host / PII Leak");

    // agent-scratch
    AGENT_SCRATCH_TRACKED = ["agent-scratch"], "agent-scratch-tracked", Fixed("Tracked Agent Scratch State");

    // shell-secrets
    SHELL_ARGV_ENV = ["shell-secrets"], "argv-env", Fixed("Unsafe Shell Pattern: ARGV-ENV");
    SHELL_ARGV_DOCKER = ["shell-secrets"], "argv-docker", Fixed("Unsafe Shell Pattern: ARGV-DOCKER");
    SHELL_ARGV_INLINE = ["shell-secrets"], "argv-inline", Fixed("Unsafe Shell Pattern: ARGV-INLINE");
    SHELL_INJECT_XARGS = ["shell-secrets"], "inject-xargs", Fixed("Unsafe Shell Pattern: INJECT-XARGS");
    SHELL_INJECT_PIPE = ["shell-secrets"], "inject-pipe", Fixed("Unsafe Shell Pattern: INJECT-PIPE");
    SECRET_GITHUB_TOKEN = ["shell-secrets"], "github-token", Fixed("Hardcoded Secret: GitHub Token");
    SECRET_AWS_ACCESS_KEY = ["shell-secrets"], "aws-access-key", Fixed("Hardcoded Secret: AWS Access Key");
    SECRET_SLACK_TOKEN = ["shell-secrets"], "slack-token", Fixed("Hardcoded Secret: Slack Token");
    SECRET_LLM_API_TOKEN = ["shell-secrets"], "llm-api-token", Fixed("Hardcoded Secret: OpenAI / Anthropic Token");
    SECRET_PRIVATE_KEY_BLOCK = ["shell-secrets"], "private-key-block", Fixed("Hardcoded Secret: Private Key Block");
    SECRET_BEARER_TOKEN = ["shell-secrets"], "authorization-bearer-token", Fixed("Hardcoded Secret: Authorization Bearer Token");
    SECRET_PASSWORD_FLAG = ["shell-secrets"], "password-flag", Fixed("Hardcoded Secret: Command-Line Password Flag");
    SECRET_CREDENTIAL_ASSIGNMENT = ["shell-secrets"], "credential-assignment", Fixed("Hardcoded Secret: Literal Credential Assignment");

    // issue-link
    DIRECTIVE_IN_SUBJECT_LINE = ["issue-link"], "directive-in-subject-line", Fixed("Directive in Subject Line");
    ISSUE_LINK_MISSING_IN_COMMIT_MESSAGE = ["issue-link"], "issue-link-missing-in-commit-message", Fixed("Missing Tracking Issue Link in Commit Message");
    ISSUE_LINK_MISSING_IN_COMMITS = ["issue-link"], "issue-link-missing-in-commits", Fixed("Missing Tracking Issue Link in Commits");
    ISSUE_LINK_MISSING = ["issue-link"], "issue-link-missing", Fixed("Missing Tracking Issue Link");

    // commit-provenance
    COMMIT_TRAILER_MISSING = ["commit-provenance"], "commit-trailer-missing", Fixed("Commit Trailer Missing");
    AGENT_COMMIT_WITHOUT_REVIEW = ["commit-provenance"], "agent-commit-without-review", Fixed("Agent Commit Without Review");
    AGENT_COMMIT_REVIEWED_BY_AUTHOR = ["commit-provenance"], "agent-commit-reviewed-by-author", Fixed("Agent Commit Reviewed By Its Author");

    // config-integrity
    GATE_WEAKENED = ["config-integrity"], "gate-weakened", Fixed("Gate Weakened By This Change");
    BASE_CONFIGURATION_UNREADABLE = ["config-integrity"], "base-configuration-unreadable", Fixed("Base Configuration Unreadable");
    BASELINE_NEW_FINDINGS = ["config-integrity"], "baseline-new-findings", Fixed("Baseline Contains New Findings Without Directive");
    BASELINE_INCREASED = ["config-integrity"], "baseline-increased", Fixed("Baseline Grew Without Directive");

    // golden-output
    SNAPSHOT_ADDED_FOR_EXISTING_TEST = ["golden-output"], "snapshot-added-for-existing-test", Fixed("Snapshot Added For Existing Test");
    GOLDEN_REGENERATED_WITHOUT_SOURCE_CHANGE = ["golden-output"], "golden-output-regenerated-without-source-change", Fixed("Golden Output Regenerated Without Source Change");
    GOLDEN_CHANGED_WITHOUT_DIRECTIVE = ["golden-output"], "golden-output-changed-without-directive", Fixed("Golden Output Modified Without Directive");

    // stub-bodies
    STUB_BODY_ADDED = ["stub-bodies"], "stub-body-added", Fixed("Stub Body Added");
    BODY_REPLACED_BY_STUB = ["stub-bodies"], "body-replaced-by-stub", Fixed("Function Body Replaced By Stub");

    // error-swallowing
    RESULT_DISCARDED = ["error-swallowing"], "result-discarded", Fixed("Result Discarded");
    VALUE_DISCARDED = ["error-swallowing"], "value-discarded", Fixed("Value Discarded");
    EMPTY_ERROR_HANDLER_ADDED = ["error-swallowing"], "empty-error-handler-added", Fixed("Empty Error Handler Added");
    ERROR_LOGGED_AND_DROPPED = ["error-swallowing"], "error-logged-and-dropped", Fixed("Empty Error Handler Added");
    UNPARSEABLE_INPUT_SKIPPED = ["error-swallowing"], "unparseable-input-skipped", Fixed("Unparseable Input Skipped");
    ERROR_SILENCED = ["error-swallowing"], "error-silenced", Fixed("Error Silenced");

    // instruction-smuggling
    AGENT_INSTRUCTIONS_CHANGED = ["instruction-smuggling"], "agent-instructions-changed", Fixed("Agent Instructions Changed");
    INVISIBLE_CHARACTERS_ADDED = ["instruction-smuggling"], "invisible-characters-added", Fixed("Invisible Characters Added");
    INSTRUCTION_LIKE_TEXT_ADDED = ["instruction-smuggling"], "instruction-like-text-added", Fixed("Instruction-Like Text Added");
    INVISIBLE_CHARACTERS_IN_DESCRIPTION = ["instruction-smuggling"], "invisible-characters-in-description", Fixed("Invisible Characters In Change Description");
    INSTRUCTION_LIKE_TEXT_IN_DESCRIPTION = ["instruction-smuggling"], "instruction-like-text-in-description", Fixed("Instruction-Like Text In Change Description");

    // build-hooks
    INSTALL_HOOK_ADDED = ["build-hooks"], "install-hook-added", Fixed("Install Hook Added");
    INSTALL_HOOK_NETWORK_OR_SHELL = ["build-hooks"], "install-hook-network-or-shell", Fixed("Install Hook Runs Network Or Shell");
    BUILD_SCRIPT_ADDED = ["build-hooks"], "build-script-added", Fixed("Build Script Added");
    BUILD_SCRIPT_NETWORK_OR_SHELL = ["build-hooks"], "build-script-network-or-shell", Fixed("Build Script Gains Network Or Shell Access");
    PACKAGE_MANAGER_CONFIG_CHANGED = ["build-hooks"], "package-manager-config-changed", Fixed("Package Manager Configuration Changed");

    // toolchain-config
    TOOLCHAIN_CHANGE_NOT_ANALYSED = ["toolchain-config"], "toolchain-config-change-not-analysed", Fixed("Toolchain Configuration Changed (not analysed)");
    TOOLCHAIN_CONFIG_DELETED = ["toolchain-config"], "toolchain-config-deleted", Fixed("Toolchain Configuration Deleted");
    TOOLCHAIN_CONFIG_UNREADABLE = ["toolchain-config"], "toolchain-config-unreadable", Fixed("Toolchain Configuration Unreadable");
    TOOLCHAIN_CONFIG_WEAKENED = ["toolchain-config"], "toolchain-config-weakened", Fixed("Toolchain Configuration Weakened");

    // scope-confinement
    FILE_IN_FORBIDDEN_SCOPE = ["scope-confinement"], "file-in-forbidden-scope", Legacy;
    FILE_OUTSIDE_AUTHORIZED_SCOPE = ["scope-confinement"], "file-outside-authorized-scope", Legacy;

    // suppression-delta
    SUPPRESSION_ADDED = ["suppression-delta"], "suppression-added", Legacy;

    // provenance-tags
    TABLE_NUMERICS_UNPROVENANCED = ["provenance-tags"], "table-numerics-unprovenanced", Fixed("Unprovenanced Table Numerics");
    MECHANISM_CLAIM_WITHOUT_EVIDENCE = ["provenance-tags"], "mechanism-claim-without-evidence", Fixed("Mechanism Claim Without Evidence");
    WALL_CLOCK_RATIO_WITHOUT_INTERVAL = ["provenance-tags"], "wall-clock-ratio-without-interval", Fixed("Bare Wall-Clock Ratio Without Interval");
    PAIRED_FIGURES_WITHOUT_WORKLOAD_TAG = ["provenance-tags"], "paired-figures-without-workload-tag", Fixed("Paired Figures Without Workload Tag");
    CROSS_METRIC_FIGURES_WITHOUT_WORKLOAD_TAG = ["provenance-tags"], "cross-metric-figures-without-workload-tag", Fixed("Cross-Metric Figures Without Workload Tag");
    PENDING_MEASUREMENT_WITHOUT_OPEN_ISSUE = ["provenance-tags"], "pending-measurement-without-open-issue", Fixed("Pending Measurement Without Open Issue");
    SUPERSEDED_FIGURE_REPUBLISHED = ["provenance-tags"], "superseded-figure-republished", Fixed("Superseded Figure Republished");

    // ci-integrity
    VERIFICATION_WORKFLOW_DELETED = ["ci-integrity"], "verification-workflow-deleted", Fixed("Deletion of Verification Workflow");
    ROLLUP_NEEDS_REMOVED = ["ci-integrity"], "rollup-needs-removed", Fixed("Rollup Job Dropped Dependency");
    ROLLUP_NEEDS_INCOMPLETE = ["ci-integrity"], "rollup-needs-incomplete", Fixed("Incomplete Rollup Job Needs");
    JOB_COUNT_FILE_MISSING = ["ci-integrity"], "job-count-file-missing", Fixed("Documented Job Count File Missing");
    JOB_COUNT_MISMATCH = ["ci-integrity"], "job-count-mismatch", Fixed("Documented Job Count Mismatch");
    PULL_REQUEST_TARGET_TRIGGER = ["ci-integrity"], "pull-request-target-trigger", Fixed("Dangerous pull_request_target Trigger");
    WORKFLOW_PERMISSIONS_WIDENED = ["ci-integrity"], "workflow-permissions-widened", Fixed("Workflow Permissions Widened");
    WORKFLOW_TIMEOUT_REMOVED = ["ci-integrity"], "workflow-timeout-removed", Fixed("Workflow timeout-minutes Removed");
    VERIFICATION_JOB_REMOVED = ["ci-integrity"], "verification-job-removed", Fixed("Deletion of Verification Job");
    JOB_TIMEOUT_REMOVED = ["ci-integrity"], "job-timeout-removed", Fixed("Job timeout-minutes Removed");
    VERIFICATION_STEP_REMOVED = ["ci-integrity"], "verification-step-removed", Fixed("Deletion of Verification Step");
    JOB_FAILURE_MASKED_CONTINUE_ON_ERROR = ["ci-integrity"], "job-failure-masked-continue-on-error", Fixed("continue-on-error Masks Failure");
    STEP_FAILURE_MASKED_CONTINUE_ON_ERROR = ["ci-integrity"], "step-failure-masked-continue-on-error", Fixed("continue-on-error Masks Failure");
    VERIFICATION_JOB_MASKED_BY_CONDITION = ["ci-integrity"], "verification-job-masked-by-condition", Fixed("Conditional Masking on Verification Job");
    UNPINNED_ACTION = ["ci-integrity"], "unpinned-action", Fixed("Unpinned Third-Party Action");
    DISCIPLINE_ACTION_POLICY_FROM = ["ci-integrity"], "discipline-action-policy-from-weakened", Fixed("Discipline Action Weakened (policy_from)");
    DISCIPLINE_ACTION_DISABLE_INPUT = ["ci-integrity"], "discipline-action-disable-input", Fixed("Discipline Action Weakened (disable input)");
    DISCIPLINE_ACTION_ADVISORY = ["ci-integrity"], "discipline-action-advisory", Fixed("Discipline Action Weakened (advisory: true)");
    DISCIPLINE_ACTION_FAIL_ON_WARNINGS_OFF = ["ci-integrity"], "discipline-action-fail-on-warnings-off", Fixed("Discipline Action Weakened (fail_on_warnings: false)");
    DISCIPLINE_ACTION_CONFIG_OVERRIDE_INVALID = ["ci-integrity"], "discipline-action-config-override-invalid", Fixed("Discipline Action Invalid config_override");
    DISCIPLINE_ACTION_SUITE_CHANGED = ["ci-integrity"], "discipline-action-suite-changed", Fixed("Discipline Action Suite Changed");
    DISCIPLINE_ACTION_DIRECTIVE_SOURCES_WIDENED = ["ci-integrity"], "discipline-action-directive-sources-widened", Fixed("Discipline Action Directive Sources Widened");
    DISCIPLINE_RUN_ADVISORY = ["ci-integrity"], "discipline-run-advisory", Fixed("Discipline Run Weakened (--advisory)");
    COMPILER_DENY_WARNINGS_REMOVED = ["ci-integrity"], "compiler-deny-warnings-removed", Fixed("Compiler Flag Dropped (-D warnings)");
    CARGO_LOCKED_REMOVED = ["ci-integrity"], "cargo-locked-removed", Fixed("Cargo Flag Dropped (--locked)");
    FROZEN_INSTALL_FLAG_REMOVED = ["ci-integrity"], "frozen-install-flag-removed", Fixed("Frozen Install Flag Dropped");
    INSTALL_COMMAND_WEAKENED = ["ci-integrity"], "install-command-weakened", Fixed("Install Command Softened");
    CLIPPY_ALL_TARGETS_REMOVED = ["ci-integrity"], "clippy-all-targets-removed", Fixed("Clippy Flag Dropped (--all-targets)");
    VERIFICATION_STEP_MASKED_BY_CONDITION = ["ci-integrity"], "verification-step-masked-by-condition", Fixed("Conditional Masking on Verification Step");
    VERIFICATION_STEP_NARROWED = ["ci-integrity"], "verification-step-narrowed", Fixed("Verification Step Narrowed");
    EXIT_CODE_MASKED = ["ci-integrity"], "exit-code-masked", Fixed("Command Masks Exit Code");
    PIPELINE_FILE_UNREADABLE = ["ci-integrity"], "pipeline-file-unreadable", Fixed("Pipeline File Unreadable");
    JOB_FAILURE_MASKED_ALLOW_FAILURE = ["ci-integrity"], "job-failure-masked-allow-failure", Fixed("allow_failure Masks Failure");
    VERIFICATION_JOB_MADE_MANUAL = ["ci-integrity"], "verification-job-made-manual", Fixed("Verification Job Made Manual");
    VERIFICATION_JOB_NARROWED = ["ci-integrity"], "verification-job-narrowed", Fixed("Verification Job Narrowed");

    // ci-skip-set
    CHANGE_JOB_MISSING_FROM_NEEDS = ["ci-skip-set"], "change-job-missing-from-needs", Legacy;
    CHANGE_JOB_DID_NOT_SUCCEED = ["ci-skip-set"], "change-job-did-not-succeed", Legacy;
    UNCONDITIONAL_JOB_MISSING_FROM_NEEDS = ["ci-skip-set"], "unconditional-job-missing-from-needs", Legacy;
    UNCONDITIONAL_JOB_SKIPPED = ["ci-skip-set"], "unconditional-job-skipped", Legacy;
    NEEDS_NAMES_UNDEFINED_JOB = ["ci-skip-set"], "needs-names-undefined-job", Legacy;
    SKIP_DECISION_UNVERIFIABLE = ["ci-skip-set"], "skip-decision-unverifiable", Legacy;
    JOB_SKIPPED_WHILE_CONDITION_TRUE = ["ci-skip-set"], "job-skipped-while-condition-true", Legacy;
    JOB_RAN_WHILE_CONDITION_FALSE = ["ci-skip-set"], "job-ran-while-condition-false", Legacy;

    // test-floor
    FLOOR_CONSTANT_MISSING_IN_BASE = ["test-floor"], "floor-constant-missing-in-base", Fixed("Floor Constant Missing in Base Ref");
    FLOOR_CONSTANT_FILE_MISSING_IN_BASE = ["test-floor"], "floor-constant-file-missing-in-base", Fixed("Floor Constant File Missing in Base Ref");
    FLOOR_CONSTANT_DECREASED = ["test-floor"], "floor-constant-decreased", Fixed("Floor Constant Decreased");
    CONFIGURED_FLOOR_DECREASED = ["test-floor"], "configured-floor-decreased", Fixed("Configured Test Floor Decreased");
    REQUIRED_SUITE_MISSING = ["test-floor"], "required-suite-missing", Fixed("Required Test Suite Missing");
    TEST_COUNT_BELOW_FLOOR = ["test-floor"], "test-count-below-floor", Fixed("Test Count Below Floor");

    // dependency-delta
    LOCKFILE_DELETED = ["dependency-delta"], "lockfile-deleted", Fixed("Lockfile Deleted");
    LOCKFILE_ENTRY_FROM_NEW_SOURCE = ["dependency-delta"], "lockfile-entry-from-new-source", Fixed("Lockfile Entry From New Source");
    LOCKFILE_INTEGRITY_HASH_REMOVED = ["dependency-delta"], "lockfile-integrity-hash-removed", Fixed("Lockfile Integrity Hash Dropped");
    MANIFEST_CHANGED_WITHOUT_LOCKFILE = ["dependency-delta"], "manifest-changed-without-lockfile", Fixed("Manifest Changed Without Lockfile");
    DIRECT_DEPENDENCY_ADDED = ["dependency-delta"], "direct-dependency-added", Fixed("New Direct Dependency Added");
    DEPENDENCY_CONSTRAINT_LOOSENED = ["dependency-delta"], "dependency-constraint-loosened", Fixed("Loosened Dependency Constraint");
    DEPENDENCY_SOURCE_CHANGED = ["dependency-delta"], "dependency-source-changed", Fixed("Dependency Source Modified");
    WILDCARD_DEPENDENCY_VERSION = ["dependency-delta"], "wildcard-dependency-version", Fixed("Wildcard Dependency Version");
    UNPINNED_GIT_DEPENDENCY = ["dependency-delta"], "unpinned-git-dependency", Fixed("Unpinned Git Dependency");
    BANNED_DEPENDENCY = ["dependency-delta"], "banned-dependency", Fixed("Banned Dependency");
    DEPENDENCY_OUTSIDE_ALLOWLIST = ["dependency-delta"], "dependency-outside-allowlist", Fixed("Dependency Outside Allowlist");
    DEPENDENCY_OUTSIDE_DENY_ALLOWLIST = ["dependency-delta"], "dependency-outside-deny-allowlist", Fixed("Dependency Outside Allowlist");
    UNAUTHORIZED_GIT_SOURCE = ["dependency-delta"], "unauthorized-git-source", Fixed("Unauthorized Git Repository Source");

    // test-budget
    FUZZ_TARGET_REMOVED = ["test-budget"], "fuzz-target-removed", Fixed("Fuzz Target Removed");
    FUZZ_TARGET_DELETED = ["test-budget"], "fuzz-target-deleted", Fixed("Fuzz Target Deleted");
    TEST_BUDGET_DECREASED = ["test-budget"], "test-budget-decreased", Fixed("Test Budget Reduced");
    SEED_CORPUS_DECREASED = ["test-budget"], "seed-corpus-decreased", Fixed("Seed Corpus Shrunk");

    // pr-checklist
    CHECKLIST_CLAIMS_TESTS = ["pr-checklist"], "checklist-claims-tests", Legacy;
    CHECKLIST_CLAIMS_DOCS = ["pr-checklist"], "checklist-claims-docs", Legacy;
    CHECKLIST_CLAIMS_BENCHMARKS = ["pr-checklist"], "checklist-claims-benchmarks", Legacy;
    CHECKLIST_CLAIM_UNSUPPORTED = ["pr-checklist"], "checklist-claim-unsupported", Legacy;

    // command
    UNTRUSTED_COMMAND_MODIFICATION = ["command"], "untrusted-command-modification", Fixed("Untrusted Command Modification");
    POLICY_FILE_DELETED = ["command"], "policy-file-deleted", Fixed("Policy File Deleted");
    CANARY_DIAGNOSTIC_MISSING = ["command"], "canary-diagnostic-missing", Fixed("Canary Diagnostic Missing");
    CANARY_COMMAND_SUCCEEDED = ["command"], "canary-command-succeeded", Fixed("Canary Command Succeeded");
    COMMAND_FAILED = ["command"], "command-failed", Fixed("Command Exited With Error");
    FORBIDDEN_OUTPUT = ["command"], "forbidden-output", Fixed("Forbidden Output Detected");
    ZERO_ITEMS_EXECUTED = ["command"], "zero-items-executed", Fixed("Zero Items Selected Or Executed");
    COUNT_BELOW_RATCHET = ["command"], "count-below-ratchet", Fixed("Count Ratchet Regression");
    COUNT_PATTERN_UNMATCHED = ["command"], "count-pattern-unmatched", Fixed("Count Pattern Did Not Match");

    // sanitizers
    SANITIZER_CANARY_DIAGNOSTIC_MISSING = ["sanitizers"], "canary-diagnostic-missing", Legacy;
    SANITIZER_COULD_NOT_RUN = ["sanitizers"], "could-not-run", Legacy;
    SANITIZER_VIOLATION_DETECTED = ["sanitizers"], "violation-detected", Legacy;

    // msrv
    MSRV_DECLARATION_MISSING = ["msrv"], "msrv-declaration-missing", Legacy;
    MSRV_COMMAND_FAILED = ["msrv"], "msrv-command-failed", Legacy;

    // miri
    MIRI_COULD_NOT_RUN = ["miri"], "could-not-run", Legacy;
    MIRI_ZERO_TESTS_EXECUTED = ["miri"], "zero-tests-executed", Legacy;
    MIRI_UNDEFINED_BEHAVIOR = ["miri"], "undefined-behavior-detected", Legacy;

    // unsafe-budget
    UNSAFE_BUDGET_EXCEEDED = ["unsafe-budget"], "budget-exceeded", Legacy;
    UNSAFE_ADDED_WITHOUT_AUTHORIZATION = ["unsafe-budget"], "unsafe-added-without-authorization", Legacy;
    UNSAFE_COUNT_INCREASED = ["unsafe-budget"], "unsafe-count-increased", Legacy;

    // bench-regression
    BENCHMARK_ARTIFACT_DELETED = ["bench-regression"], "benchmark-artifact-deleted", Fixed("Benchmark Artifact Deleted");
    NEW_ARTIFACT_BASELINE_MISSING = ["bench-regression"], "new-artifact-baseline-missing", Fixed("New Benchmark Artifact Lacks Baseline");
    BENCHMARK_PROVENANCE_MISMATCH = ["bench-regression"], "benchmark-provenance-mismatch", Fixed("Mismatched Benchmark Provenance");
    CROSS_HOST_COMPARISON = ["bench-regression"], "cross-host-comparison", Fixed("Cross-Host Benchmark Comparison Mismatch");
    BENCHMARK_BASELINE_MISSING = ["bench-regression"], "benchmark-baseline-missing", Fixed("Missing Benchmark Baseline");
    OVERRIDE_VOID_CITATION_DOES_NOT_MEASURE = ["bench-regression"], "override-void-citation-does-not-measure", Fixed("Regression Override Is Void — Citation Does Not Measure This Code");
    OVERRIDE_UNVERIFIED_CITATION_UNDECIDABLE = ["bench-regression"], "override-unverified-citation-undecidable", Fixed("Regression Override Not Verified — Citation Undecidable");
    COUNTER_REGRESSED = ["bench-regression"], "counter-regressed", Fixed("Instruction Count Regressed");
    BENCHMARK_REMOVED = ["bench-regression"], "benchmark-removed", Fixed("Benchmark Removed");
    NEW_OR_RENAMED_ARM_BASELINE_MISSING = ["bench-regression"], "new-or-renamed-arm-baseline-missing", Fixed("New or Renamed Benchmark Lacks Baseline");
    PERFORMANCE_REGRESSED = ["bench-regression"], "performance-regressed", Fixed("Benchmark Performance Regressed");
    OVERRIDE_VOID_NO_RESOLVABLE_CITATION = ["bench-regression"], "override-void-no-resolvable-citation", Fixed("Regression Override Is Void — No Resolvable Citation");
    OVERRIDE_VOID_NAMES_NO_REGRESSED_ARM = ["bench-regression"], "override-void-names-no-regressed-arm", Fixed("Regression Override Is Void — Names No Regressed Arm");
    COUNTER_REGRESSED_UNAPPROVED_ARM = ["bench-regression"], "counter-regressed-unapproved-arm", Fixed("Instruction Count Regressed (Unapproved Arm)");
    STALE_ARM_EXEMPTION = ["bench-regression"], "stale-arm-exemption", Fixed("Stale Benchmark Arm Exemption");
    PAIRED_RATIO_NOT_COMPARABLE = ["bench-regression"], "paired-ratio-not-comparable", Fixed("Paired Ratio Not Comparable");
    PAIRED_RATIO_CELL_MISSING = ["bench-regression"], "paired-ratio-cell-missing", Fixed("Paired Ratio Cell Missing");
    PAIRED_RATIO_INCONSISTENT_WITH_ROUNDS = ["bench-regression"], "paired-ratio-inconsistent-with-rounds", Fixed("Paired Ratio Disagrees With Its Rounds");
    PAIRED_RATIO_REGRESSED = ["bench-regression"], "paired-ratio-regressed", Fixed("Paired Ratio Regressed");
    RATIO_BASELINE_LOOSENED = ["bench-regression"], "ratio-baseline-loosened", Fixed("Ratio Baseline Loosened");
    RATIO_BASELINE_CHANGED_WITH_SOURCE = ["bench-regression"], "ratio-baseline-changed-with-source", Fixed("Ratio Baseline Changed With Source");
    PAIRED_RATIO_RUN_MISSING = ["bench-regression"], "paired-ratio-run-missing", Fixed("Paired Ratio Run Missing");

    // archive-contents
    REQUIRED_ARCHIVE_PATH_MISSING = ["archive-contents"], "required-path-missing", Fixed("Missing Required Archive Path");
    FORBIDDEN_ARCHIVE_ENTRY = ["archive-contents"], "forbidden-entry", Fixed("Forbidden Entry Found in Archive");
    SOURCE_LEAKED_IN_ARCHIVE = ["archive-contents"], "source-leaked", Fixed("Source Leaked In Archive");
    SOURCE_MAP_SHIPPED = ["archive-contents"], "source-map-shipped", Fixed("Source Map Shipped");

    // manifest-sync
    MANIFEST_DRIFT = ["manifest-sync"], "manifest-drift", Fixed("Manifest Synchronization Drift");

    // version-lockstep
    VERSION_MISMATCH = ["version-lockstep"], "version-mismatch", Fixed("Version Declaration Lockstep Mismatch");

    // A run that could not complete (the pseudo-gate `engine`, reported only in `--json-out`).
    ENGINE_COULD_NOT_RUN = ["engine"], "could-not-run", Legacy;
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
