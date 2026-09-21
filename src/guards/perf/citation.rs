//! Freshness of the citations an `allow-regression:` reason rests on.
//!
//! `require_sourced_override` admits a regression override only when its reason cites a
//! CI run URL or a committed artifact path. A citation that resolves syntactically can
//! still point at a measurement of other code: a run that was cancelled before the job
//! whose numbers are quoted, a run at a head a force-push rewrote away, or an artifact
//! regenerated before the very change it is asked to excuse. Each of those resolves and
//! measures nothing about the head being gated.
//!
//! Every citation in the reason is checked, not the first: a reason with two citations
//! rests on both, and checking one lets word order decide which source is examined.
//!
//! - A cited run must have a conclusion that carries a measurement: `cancelled`,
//!   `timed_out`, `action_required`, `startup_failure`, `stale` and `skipped` do not.
//! - A cited run that concluded `failure` counts only when every configured measurement
//!   job it started reached its guard step with every earlier step green. A regression
//!   trips the guard on numbers it measured; a crashed benchmark leaves none, and both
//!   conclude `failure`. With no measurement jobs configured the two cannot be told apart.
//! - The cited run's head must be reachable from the head under review (`ahead` or
//!   `identical` in the compare API).
//! - A cited data artifact (`.json`, `.csv`, `.txt`, `.log`, `.out`) must be tracked and
//!   must not have been last committed before the branch's newest change under
//!   `citation_source_paths`. Prose and figures (`.md`, `.svg`) are cited as rules and
//!   context, not as the source of a number, and are not dated.
//!
//! A citation the available instruments cannot decide (no network, unauthenticated, rate
//! limited, a non-GitHub run URL, no base ref) is reported by name and leaves the gate
//! ARMED: "could not check" is never "checked and clean".
//!
//! Run, job and commit data come from the GitHub REST API over HTTPS (`crate::forge`,
//! token from `DISCIPLINE_FORGE_TOKEN`, `GH_TOKEN` or `GITHUB_TOKEN`). The instruments are a
//! trait so every case is testable offline.

use crate::config::MeasurementJob;
use crate::tokens;
use regex::Regex;
use std::sync::LazyLock;

/// Run conclusions that carry no usable measurement.
pub const DEAD_CONCLUSIONS: &[&str] = &[
    "cancelled",
    "timed_out",
    "action_required",
    "startup_failure",
    "stale",
    "skipped",
];

/// The conclusion a run reaches when a regression guard trips (and when a benchmark
/// crashes: only the jobs tell the two apart).
pub const GUARD_TRIP_CONCLUSION: &str = "failure";

/// Step conclusions under which a pre-guard step left its output behind.
const PRODUCED_STEP_CONCLUSIONS: &[&str] = &["success", "skipped"];

/// Guard-step conclusions under which the guard ran to completion on the numbers.
const RAN_GUARD_CONCLUSIONS: &[&str] = &["success", "failure"];

/// Compare statuses under which the run's commit is reachable from the head.
const REACHABLE_STATUSES: &[&str] = &["ahead", "identical"];

/// Pages of the jobs API read before giving up (100 jobs per page).
const MAX_JOB_PAGES: usize = 10;

static GITHUB_RUN_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^https?://github\.com/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)/actions/runs/(\d+)$")
        .unwrap()
});

static DATA_ARTIFACT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\.(?:json|csv|txt|log|out)$").unwrap());

/// The external instruments freshness is decided with.
pub trait CitationInstruments {
    /// A GitHub API GET of `path`, parsed as JSON; `Err` with a reason when unavailable.
    fn gh_api(&self, path: &str) -> std::result::Result<serde_json::Value, String>;
    /// Whether `path` is tracked at head; `None` when git cannot answer.
    fn is_tracked(&self, path: &str) -> Option<bool>;
    /// Commit time of the branch's newest change under `paths`; `Ok(None)` when the branch
    /// changes none of them.
    fn newest_branch_change(&self, paths: &[String]) -> std::result::Result<Option<i64>, String>;
    /// Commit time of the newest commit that changed `path`; `Ok(None)` when none did.
    fn last_change(&self, path: &str) -> std::result::Result<Option<i64>, String>;
    /// Full id of the head under review.
    fn pr_head(&self) -> Option<String>;
}

/// Instruments that can decide nothing: every citation is undecidable.
pub struct Unavailable;

impl CitationInstruments for Unavailable {
    fn gh_api(&self, _path: &str) -> std::result::Result<serde_json::Value, String> {
        Err("no instruments were supplied to this evaluation".to_string())
    }
    fn is_tracked(&self, _path: &str) -> Option<bool> {
        None
    }
    fn newest_branch_change(&self, _paths: &[String]) -> std::result::Result<Option<i64>, String> {
        Err("no instruments were supplied to this evaluation".to_string())
    }
    fn last_change(&self, _path: &str) -> std::result::Result<Option<i64>, String> {
        Err("no instruments were supplied to this evaluation".to_string())
    }
    fn pr_head(&self) -> Option<String> {
        None
    }
}

/// Instruments that answer from canned data: GitHub API responses keyed by API path, and
/// git answers. Used by the self-test and to replay recorded API responses offline.
#[derive(Debug, Clone, Default)]
pub struct CannedInstruments {
    /// API path (e.g. `repos/o/r/actions/runs/1`) to the JSON it returns.
    pub responses: std::collections::BTreeMap<String, serde_json::Value>,
    pub tracked: std::collections::BTreeSet<String>,
    pub last_change: std::collections::BTreeMap<String, i64>,
    pub branch_change: Option<i64>,
    pub head: Option<String>,
}

impl CitationInstruments for CannedInstruments {
    fn gh_api(&self, path: &str) -> std::result::Result<serde_json::Value, String> {
        self.responses
            .get(path)
            .cloned()
            .ok_or_else(|| format!("no recorded response for `{path}`"))
    }
    fn is_tracked(&self, path: &str) -> Option<bool> {
        Some(self.tracked.contains(path))
    }
    fn newest_branch_change(&self, _paths: &[String]) -> std::result::Result<Option<i64>, String> {
        Ok(self.branch_change)
    }
    fn last_change(&self, path: &str) -> std::result::Result<Option<i64>, String> {
        Ok(self.last_change.get(path).copied())
    }
    fn pr_head(&self) -> Option<String> {
        self.head.clone()
    }
}

/// Live instruments: the GitHub API over HTTPS (see [`crate::forge`]), and the repository.
pub struct LiveInstruments<'a> {
    pub git: &'a crate::gitctx::GitCtx,
}

impl<'a> LiveInstruments<'a> {
    pub fn new(git: &'a crate::gitctx::GitCtx) -> Self {
        Self { git }
    }
}

impl CitationInstruments for LiveInstruments<'_> {
    fn gh_api(&self, path: &str) -> std::result::Result<serde_json::Value, String> {
        use crate::forge::ForgeApi;
        let github = crate::forge::Forge {
            kind: crate::forge::ForgeKind::GitHub,
            url: "https://github.com".to_string(),
            repo: String::new(),
        };
        crate::forge::HttpApi::from_env()
            .get(&github, path)?
            .ok_or_else(|| format!("GitHub API `{path}`: not found (HTTP 404)"))
    }

    fn is_tracked(&self, path: &str) -> Option<bool> {
        self.git.is_tracked(path).ok()
    }

    fn newest_branch_change(&self, paths: &[String]) -> std::result::Result<Option<i64>, String> {
        self.git
            .newest_commit_time_touching(paths, true)
            .map_err(|e| format!("{e:#}"))
    }

    fn last_change(&self, path: &str) -> std::result::Result<Option<i64>, String> {
        self.git
            .newest_commit_time_touching(&[path.to_string()], false)
            .map_err(|e| format!("{e:#}"))
    }

    fn pr_head(&self) -> Option<String> {
        self.git.head_oid()
    }
}

/// What freshness is judged against.
pub struct FreshnessPolicy<'a> {
    pub measurement_jobs: &'a [MeasurementJob],
    pub source_paths: &'a [String],
}

/// `problems` void the override; `undecidable` names citations that could not be checked
/// and also leave the gate armed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FreshnessReport {
    pub problems: Vec<String>,
    pub undecidable: Vec<String>,
}

impl FreshnessReport {
    /// True only when every citation was checked and none is stale.
    pub fn is_fresh(&self) -> bool {
        self.problems.is_empty() && self.undecidable.is_empty()
    }
}

fn str_field<'v>(v: &'v serde_json::Value, key: &str) -> &'v str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("")
}

/// Every job of a run's latest attempt, or why the list could not be read in full.
///
/// A partial list could omit the very job that crashed, so pages that do not add up to
/// `total_count` are "could not check", never a list to decide on.
fn run_jobs(
    instruments: &dyn CitationInstruments,
    repo: &str,
    run_id: &str,
) -> std::result::Result<Vec<serde_json::Value>, String> {
    let mut jobs = Vec::new();
    for page in 1..=MAX_JOB_PAGES {
        let body = instruments.gh_api(&format!(
            "repos/{repo}/actions/runs/{run_id}/jobs?per_page=100&page={page}"
        ))?;
        let batch = body
            .get("jobs")
            .and_then(|j| j.as_array())
            .ok_or("jobs page has no `jobs` array")?;
        let total = body
            .get("total_count")
            .and_then(|t| t.as_u64())
            .ok_or("jobs page has no `total_count`")? as usize;
        jobs.extend(batch.iter().filter(|j| j.is_object()).cloned());
        if jobs.len() >= total || batch.is_empty() {
            return if jobs.len() == total {
                Ok(jobs)
            } else {
                Err(format!("jobs pages list {} of {total} jobs", jobs.len()))
            };
        }
    }
    Err(format!("more than {MAX_JOB_PAGES} pages of jobs"))
}

/// Why a `failure` run holds no usable measurement, or `None` if it does.
///
/// Every configured measurement job that started must have reached its guard step with
/// every earlier step green (or skipped by its own condition), and at least one must have
/// started. A job skipped by a path filter measured nothing and is ignored.
fn measurement_problem(
    jobs: &[serde_json::Value],
    measurement: &[MeasurementJob],
) -> Option<String> {
    let mut started = 0;
    for job in jobs {
        let name = str_field(job, "name");
        let Some(spec) = measurement.iter().find(|m| m.job == name) else {
            continue;
        };
        let guard = spec.guard.as_str();
        let conclusion = str_field(job, "conclusion").to_lowercase();
        if conclusion == "skipped" {
            continue;
        }
        started += 1;
        if conclusion == "success" {
            continue;
        }
        if conclusion != GUARD_TRIP_CONCLUSION {
            let c = if conclusion.is_empty() {
                "none"
            } else {
                &conclusion
            };
            return Some(format!(
                "job `{name}` concluded `{c}` — it never reached its report"
            ));
        }
        let mut steps: Vec<&serde_json::Value> = job
            .get("steps")
            .and_then(|s| s.as_array())
            .map(|s| s.iter().filter(|x| x.is_object()).collect())
            .unwrap_or_default();
        steps.sort_by_key(|s| s.get("number").and_then(|n| n.as_u64()).unwrap_or(0));
        if !steps.iter().any(|s| str_field(s, "name") == guard) {
            return Some(format!(
                "job `{name}` has no `{guard}` step — its report cannot be located"
            ));
        }
        for step in steps {
            let step_name = str_field(step, "name");
            let sc = str_field(step, "conclusion").to_lowercase();
            let shown = if sc.is_empty() { "none" } else { sc.as_str() };
            if step_name == guard {
                if !RAN_GUARD_CONCLUSIONS.contains(&sc.as_str()) {
                    return Some(format!(
                        "job `{name}`'s regression guard concluded `{shown}` — it never read the numbers"
                    ));
                }
                break;
            }
            if !PRODUCED_STEP_CONCLUSIONS.contains(&sc.as_str()) {
                return Some(format!(
                    "job `{name}` step `{step_name}` concluded `{shown}` before the regression guard ran — the numbers it would report were never produced"
                ));
            }
        }
    }
    if started == 0 {
        return Some("no measurement job ran in it — it holds no gated numbers".to_string());
    }
    None
}

fn is_run_url(citation: &str) -> bool {
    let lower = citation.to_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn check_run(
    citation: &str,
    policy: &FreshnessPolicy<'_>,
    instruments: &dyn CitationInstruments,
    report: &mut FreshnessReport,
) {
    let Some(caps) = GITHUB_RUN_URL.captures(citation) else {
        report.undecidable.push(format!(
            "`{citation}` (not a GitHub Actions run URL; its freshness cannot be checked)"
        ));
        return;
    };
    let (owner, name, run_id) = (&caps[1], &caps[2], &caps[3]);
    if [owner, name]
        .iter()
        .any(|seg| seg.starts_with('.') || seg.is_empty())
    {
        report.problems.push(format!(
            "`{citation}` does not name a repository — the citation resolves to nothing"
        ));
        return;
    }
    let repo = format!("{owner}/{name}");
    let body = match instruments.gh_api(&format!("repos/{repo}/actions/runs/{run_id}")) {
        Ok(b) => b,
        Err(why) => {
            report
                .undecidable
                .push(format!("run {run_id} (GitHub API unavailable: {why})"));
            return;
        }
    };
    let conclusion = str_field(&body, "conclusion").to_lowercase();
    if conclusion.is_empty() {
        report.undecidable.push(format!(
            "run {run_id} (still in progress; no conclusion yet)"
        ));
        return;
    }
    if DEAD_CONCLUSIONS.contains(&conclusion.as_str()) {
        report.problems.push(format!(
            "run {run_id} concluded `{conclusion}` — a run that did not complete may have skipped the job whose numbers are quoted"
        ));
        return;
    }
    if conclusion == GUARD_TRIP_CONCLUSION {
        if policy.measurement_jobs.is_empty() {
            report.undecidable.push(format!(
                "run {run_id} (concluded `failure`; no `citation_measurement_jobs` are configured to tell a tripped guard from a crashed benchmark)"
            ));
            return;
        }
        match run_jobs(instruments, &repo, run_id) {
            Err(why) => {
                report.undecidable.push(format!(
                    "run {run_id} (concluded `failure`; its jobs could not be read to tell a tripped guard from a crashed benchmark: {why})"
                ));
                return;
            }
            Ok(jobs) => {
                if let Some(missing) = measurement_problem(&jobs, policy.measurement_jobs) {
                    report
                        .problems
                        .push(format!("run {run_id} concluded `failure` and {missing}"));
                    return;
                }
            }
        }
    }
    let head_sha = str_field(&body, "head_sha").to_string();
    let Some(pr_head) = instruments.pr_head().filter(|h| !h.is_empty()) else {
        report.undecidable.push(format!(
            "run {run_id} (no head revision to compare against)"
        ));
        return;
    };
    if head_sha.is_empty() {
        report
            .undecidable
            .push(format!("run {run_id} (the API reported no head revision)"));
        return;
    }
    let short: String = head_sha.chars().take(8).collect();
    match instruments.gh_api(&format!("repos/{repo}/compare/{head_sha}...{pr_head}")) {
        Err(why) => report.undecidable.push(format!(
            "run {run_id} (could not compare {short} to head: {why})"
        )),
        Ok(cmp) => {
            let status = str_field(&cmp, "status");
            if status.is_empty() {
                report.undecidable.push(format!(
                    "run {run_id} (the compare API reported no status for {short})"
                ));
            } else if !REACHABLE_STATUSES.contains(&status) {
                report.problems.push(format!(
                    "run {run_id} measured {short}, which is `{status}` relative to the head — it did not measure this code"
                ));
            }
        }
    }
}

fn check_artifact(
    citation: &str,
    policy: &FreshnessPolicy<'_>,
    instruments: &dyn CitationInstruments,
    newest_code: &mut Option<std::result::Result<Option<i64>, String>>,
    report: &mut FreshnessReport,
) {
    match instruments.is_tracked(citation) {
        None => {
            report.undecidable.push(format!(
                "`{citation}` (git could not say whether it is tracked)"
            ));
            return;
        }
        Some(false) => {
            report.problems.push(format!(
                "`{citation}` is not a tracked file — the citation resolves to nothing"
            ));
            return;
        }
        Some(true) => {}
    }
    if !DATA_ARTIFACT.is_match(citation) {
        return;
    }
    if policy.source_paths.is_empty() {
        report.undecidable.push(format!(
            "`{citation}` (no `citation_source_paths` are configured to date the branch's changes against)"
        ));
        return;
    }
    let newest =
        newest_code.get_or_insert_with(|| instruments.newest_branch_change(policy.source_paths));
    let newest = match newest {
        Err(why) => {
            report.undecidable.push(format!(
                "`{citation}` (the branch's changes could not be dated: {why})"
            ));
            return;
        }
        // The branch changes no gated source: nothing to be stale against.
        Ok(None) => return,
        Ok(Some(t)) => *t,
    };
    match instruments.last_change(citation) {
        Err(why) => report
            .undecidable
            .push(format!("`{citation}` (its history could not be read: {why})")),
        Ok(None) => report
            .undecidable
            .push(format!("`{citation}` (no commit history)")),
        Ok(Some(at)) if at < newest => report.problems.push(format!(
            "`{citation}` was last regenerated before this branch's newest change under {} — it describes the code this change replaced",
            policy.source_paths.join(", ")
        )),
        Ok(Some(_)) => {}
    }
}

/// Checks every citation in `reason` for freshness.
pub fn check_citation_freshness(
    reason: &str,
    policy: &FreshnessPolicy<'_>,
    instruments: &dyn CitationInstruments,
) -> FreshnessReport {
    let mut report = FreshnessReport::default();
    let mut newest_code = None;
    for citation in tokens::extract_citations(reason) {
        if is_run_url(&citation) {
            check_run(&citation, policy, instruments, &mut report);
        } else {
            check_artifact(
                &citation,
                policy,
                instruments,
                &mut newest_code,
                &mut report,
            );
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};

    const RUN: &str = "https://github.com/acme/widgets/actions/runs/1001";
    const ART: &str = "results/fallback_maturity.json";
    const HEAD: &str = "1839df01";

    const MEASURE: &str = "Perf / Instruction Counts";
    const MEASURE_STEPS: &[&str] = &[
        "Set up job",
        "Run actions/checkout",
        "Instruction counts (merge base)",
        "Instruction counts (this branch)",
        "Generate Report and Enforce Regression Guard",
        "Post Benchmark Comment",
        "Complete job",
    ];
    const GUARD: &str = "Generate Report and Enforce Regression Guard";
    const SMOKE: &str = "Perf / Instruction Counts Smoke";
    const FUEL: &str = "Perf / Fuel Counts";
    const FUEL_STEPS: &[&str] = &[
        "Set up job",
        "Fuel counts, 32-bit engine",
        "Fuel counts, 64-bit engine",
        "Fuel regression guard vs committed baseline",
        "Publish tables",
        "Complete job",
    ];
    const FUEL_GUARD: &str = "Fuel regression guard vs committed baseline";

    fn measurement_jobs() -> Vec<MeasurementJob> {
        [(MEASURE, GUARD), (SMOKE, GUARD), (FUEL, FUEL_GUARD)]
            .iter()
            .map(|(j, g)| MeasurementJob {
                job: j.to_string(),
                guard: g.to_string(),
            })
            .collect()
    }

    fn source_paths() -> Vec<String> {
        vec!["crates".to_string(), "include".to_string()]
    }

    /// Serves GitHub API responses and git answers from fixtures.
    #[derive(Default)]
    struct Fixture {
        runs: BTreeMap<String, serde_json::Value>,
        jobs: BTreeMap<String, Vec<serde_json::Value>>,
        compare: BTreeMap<String, String>,
        tracked: BTreeSet<String>,
        last_change: BTreeMap<String, i64>,
        branch_change: Option<i64>,
        no_base: bool,
        calls: std::cell::RefCell<Vec<String>>,
    }

    impl CitationInstruments for Fixture {
        fn gh_api(&self, path: &str) -> std::result::Result<serde_json::Value, String> {
            self.calls.borrow_mut().push(path.to_string());
            let unavailable = || Err("fixture: no response".to_string());
            if let Some(rest) = path.strip_prefix("repos/acme/widgets/actions/runs/") {
                if let Some((id, _)) = rest.split_once("/jobs") {
                    return match self.jobs.get(id) {
                        Some(jobs) => Ok(json!({"total_count": jobs.len(), "jobs": jobs})),
                        None => unavailable(),
                    };
                }
                return self.runs.get(rest).cloned().map_or_else(unavailable, Ok);
            }
            if let Some(rest) = path.strip_prefix("repos/acme/widgets/compare/") {
                let (sha, head) = rest.split_once("...").unwrap();
                assert_eq!(head, HEAD, "compared against the wrong head");
                return match self.compare.get(sha) {
                    Some(status) => Ok(json!({ "status": status })),
                    None => unavailable(),
                };
            }
            unavailable()
        }
        fn is_tracked(&self, path: &str) -> Option<bool> {
            Some(self.tracked.contains(path))
        }
        fn newest_branch_change(
            &self,
            paths: &[String],
        ) -> std::result::Result<Option<i64>, String> {
            assert_eq!(paths, source_paths().as_slice());
            if self.no_base {
                return Err("no base".to_string());
            }
            Ok(self.branch_change)
        }
        fn last_change(&self, path: &str) -> std::result::Result<Option<i64>, String> {
            Ok(self.last_change.get(path).copied())
        }
        fn pr_head(&self) -> Option<String> {
            Some(HEAD.to_string())
        }
    }

    fn fixture() -> Fixture {
        Fixture {
            tracked: BTreeSet::from([ART.to_string()]),
            ..Default::default()
        }
    }

    fn run(conclusion: Option<&str>, head_sha: &str) -> serde_json::Value {
        json!({ "conclusion": conclusion, "head_sha": head_sha })
    }

    fn job(
        name: &str,
        steps: &[&str],
        conclusion: &str,
        set: &[(&str, &str)],
    ) -> serde_json::Value {
        let steps: Vec<_> = steps
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let c = set
                    .iter()
                    .find(|(n, _)| n == s)
                    .map(|(_, c)| *c)
                    .unwrap_or("success");
                json!({ "number": i + 1, "name": s, "conclusion": c })
            })
            .collect();
        json!({ "name": name, "conclusion": conclusion, "steps": steps })
    }

    fn other(name: &str, conclusion: &str) -> serde_json::Value {
        json!({ "name": name, "conclusion": conclusion, "steps": [] })
    }

    /// Step `at` ends `how`; later steps are skipped except the guard, which runs
    /// `if: always()` and fails on the output that is missing.
    fn broke(name: &str, steps: &[&str], at: &str, how: &str, guard: &str) -> serde_json::Value {
        let idx = steps.iter().position(|s| *s == at).unwrap();
        let mut set: Vec<(&str, &str)> = steps[idx + 1..]
            .iter()
            .filter(|s| **s != guard && **s != "Complete job")
            .map(|s| (*s, "skipped"))
            .collect();
        set.push((at, how));
        set.push((guard, "failure"));
        let conclusion = if how == "cancelled" {
            "cancelled"
        } else {
            "failure"
        };
        job(name, steps, conclusion, &set)
    }

    fn tripped() -> serde_json::Value {
        job(
            MEASURE,
            MEASURE_STEPS,
            "failure",
            &[(GUARD, "failure"), ("Post Benchmark Comment", "skipped")],
        )
    }

    fn check(reason: &str, fx: &Fixture) -> FreshnessReport {
        let jobs = measurement_jobs();
        let paths = source_paths();
        let policy = FreshnessPolicy {
            measurement_jobs: &jobs,
            source_paths: &paths,
        };
        check_citation_freshness(reason, &policy, fx)
    }

    fn failure_run_fixture(status: &str, jobs: Vec<serde_json::Value>) -> Fixture {
        let mut fx = fixture();
        fx.runs
            .insert("1001".into(), run(Some("failure"), "2804a495"));
        fx.compare.insert("2804a495".into(), status.into());
        fx.jobs.insert("1001".into(), jobs);
        fx
    }

    fn reason_with_run() -> String {
        format!("map_insert +1.35% is the covered fallback; counts in {RUN}")
    }

    #[test]
    fn failure_run_at_a_rewritten_head_is_stale_even_though_its_guard_tripped() {
        let fx = failure_run_fixture(
            "diverged",
            vec![
                other("Docs / Hygiene", "failure"),
                tripped(),
                job(FUEL, FUEL_STEPS, "success", &[]),
                other("CI Gate / All Checks Passed", "failure"),
            ],
        );
        let r = check(&reason_with_run(), &fx);
        assert!(r.undecidable.is_empty(), "{r:?}");
        assert_eq!(r.problems.len(), 1, "{r:?}");
        assert!(r.problems[0].contains("is `diverged`"), "{r:?}");
    }

    #[test]
    fn failure_run_whose_guard_tripped_on_measured_numbers_is_admitted() {
        let fx = failure_run_fixture(
            "ahead",
            vec![
                tripped(),
                job(FUEL, FUEL_STEPS, "success", &[]),
                other("CI Gate / All Checks Passed", "failure"),
            ],
        );
        let r = check(&reason_with_run(), &fx);
        assert!(r.is_fresh(), "{r:?}");
    }

    #[test]
    fn failure_run_with_unreadable_jobs_is_undecidable_not_admitted() {
        let mut fx = failure_run_fixture("ahead", vec![]);
        fx.jobs.clear();
        let r = check(&reason_with_run(), &fx);
        assert!(r.problems.is_empty(), "{r:?}");
        assert_eq!(r.undecidable.len(), 1, "{r:?}");
        assert!(
            r.undecidable[0].contains("tell a tripped guard from a crashed benchmark"),
            "{r:?}"
        );
        assert!(!r.is_fresh());
    }

    #[test]
    fn benchmark_step_that_failed_before_the_guard_voids_the_run() {
        let crashed = broke(
            MEASURE,
            MEASURE_STEPS,
            "Instruction counts (this branch)",
            "failure",
            GUARD,
        );
        let fx = failure_run_fixture("ahead", vec![crashed]);
        let r = check(&reason_with_run(), &fx);
        assert_eq!(r.problems.len(), 1, "{r:?}");
        assert!(
            r.problems[0]
                .contains("(this branch)` concluded `failure` before the regression guard ran"),
            "{r:?}"
        );
    }

    #[test]
    fn benchmark_step_cancelled_inside_a_failure_run_voids_it() {
        let cancelled = broke(
            MEASURE,
            MEASURE_STEPS,
            "Instruction counts (this branch)",
            "cancelled",
            GUARD,
        );
        let fx = failure_run_fixture("ahead", vec![cancelled]);
        let r = check(&reason_with_run(), &fx);
        assert_eq!(r.problems.len(), 1, "{r:?}");
        assert!(
            r.problems[0].contains("concluded `cancelled` — it never reached its report"),
            "{r:?}"
        );
    }

    #[test]
    fn guard_that_did_not_run_to_completion_never_read_the_numbers() {
        let fx = failure_run_fixture(
            "ahead",
            vec![job(
                MEASURE,
                MEASURE_STEPS,
                "failure",
                &[(GUARD, "cancelled")],
            )],
        );
        let r = check(&reason_with_run(), &fx);
        assert_eq!(r.problems.len(), 1, "{r:?}");
        assert!(
            r.problems[0].contains("regression guard concluded `cancelled`"),
            "{r:?}"
        );
    }

    #[test]
    fn failure_only_after_the_guard_still_holds_the_report() {
        let late = job(
            MEASURE,
            MEASURE_STEPS,
            "failure",
            &[("Post Benchmark Comment", "failure")],
        );
        let fx = failure_run_fixture("ahead", vec![late]);
        assert!(check(&reason_with_run(), &fx).is_fresh());
    }

    #[test]
    fn every_started_measurement_job_must_reach_its_guard() {
        let mut smoke = broke(
            MEASURE,
            MEASURE_STEPS,
            "Instruction counts (this branch)",
            "failure",
            GUARD,
        );
        smoke["name"] = json!(SMOKE);
        let fx = failure_run_fixture("ahead", vec![tripped(), smoke]);
        let r = check(&reason_with_run(), &fx);
        assert_eq!(r.problems.len(), 1, "{r:?}");
        assert!(r.problems[0].contains("Smoke"), "{r:?}");
    }

    #[test]
    fn failure_run_where_no_measurement_job_ran_holds_no_numbers() {
        let fx = failure_run_fixture(
            "ahead",
            vec![
                other(MEASURE, "skipped"),
                other(FUEL, "skipped"),
                other("CI Gate / All Checks Passed", "failure"),
            ],
        );
        let r = check(&reason_with_run(), &fx);
        assert_eq!(r.problems.len(), 1, "{r:?}");
        assert!(r.problems[0].contains("no measurement job ran"), "{r:?}");
    }

    #[test]
    fn job_without_its_guard_step_cannot_locate_a_report() {
        // A job that measured and gated in one step: a failure there is a trip or a crash
        // with nothing to tell them apart.
        let unsplit = json!({
            "name": FUEL,
            "conclusion": "failure",
            "steps": [
                {"number": 1, "name": "Set up job", "conclusion": "success"},
                {"number": 9, "name": "Fuel counts, 32-bit engine vs committed baseline", "conclusion": "failure"},
                {"number": 10, "name": "Fuel counts, 64-bit engine vs committed baseline", "conclusion": "skipped"},
            ],
        });
        let fx = failure_run_fixture("ahead", vec![tripped(), unsplit]);
        let r = check(&reason_with_run(), &fx);
        assert_eq!(r.problems.len(), 1, "{r:?}");
        assert!(
            r.problems[0].contains("has no `Fuel regression guard vs committed baseline` step"),
            "{r:?}"
        );
    }

    #[test]
    fn tripped_guard_with_measurement_steps_green_is_admitted_and_skipped_guard_is_not() {
        let tripped_fuel = job(FUEL, FUEL_STEPS, "failure", &[(FUEL_GUARD, "failure")]);
        let fx = failure_run_fixture(
            "ahead",
            vec![job(MEASURE, MEASURE_STEPS, "success", &[]), tripped_fuel],
        );
        assert!(check(&reason_with_run(), &fx).is_fresh());

        // A guard that is skipped after a failed measurement: the earlier step decides.
        let crashed_fuel = job(
            FUEL,
            FUEL_STEPS,
            "failure",
            &[
                ("Fuel counts, 64-bit engine", "failure"),
                (FUEL_GUARD, "skipped"),
            ],
        );
        let fx = failure_run_fixture(
            "ahead",
            vec![job(MEASURE, MEASURE_STEPS, "success", &[]), crashed_fuel],
        );
        let r = check(&reason_with_run(), &fx);
        assert_eq!(r.problems.len(), 1, "{r:?}");
        assert!(
            r.problems[0].contains("64-bit engine` concluded `failure`"),
            "{r:?}"
        );
    }

    #[test]
    fn admitted_failure_run_is_still_held_to_reachability() {
        let fx = failure_run_fixture(
            "diverged",
            vec![tripped(), job(FUEL, FUEL_STEPS, "success", &[])],
        );
        let r = check(&reason_with_run(), &fx);
        assert_eq!(r.problems.len(), 1, "{r:?}");
        assert!(r.problems[0].contains("is `diverged`"), "{r:?}");
    }

    fn two_citation_reason(url_first: bool) -> String {
        if url_first {
            format!("leaf trade measured in CI run {RUN} (fewer reallocations in {ART})")
        } else {
            format!("leaf trade (fewer reallocations in {ART}; measured in CI run {RUN})")
        }
    }

    #[test]
    fn cancelled_run_voids_the_citation() {
        let mut fx = fixture();
        fx.runs
            .insert("1001".into(), run(Some("cancelled"), "dfc5f456"));
        fx.compare.insert("dfc5f456".into(), "diverged".into());
        fx.last_change.insert(ART.into(), 200);
        fx.branch_change = Some(100);
        let r = check(&two_citation_reason(false), &fx);
        assert!(
            r.problems
                .iter()
                .any(|p| p.contains("concluded `cancelled`")),
            "{r:?}"
        );
        // The dead run is judged on its conclusion alone; its head is never compared.
        assert!(!fx.calls.borrow().iter().any(|c| c.contains("/compare/")));
    }

    #[test]
    fn successful_run_at_a_rewritten_head_measured_other_code() {
        let mut fx = fixture();
        fx.runs
            .insert("1001".into(), run(Some("success"), "dfc5f456"));
        fx.compare.insert("dfc5f456".into(), "diverged".into());
        fx.last_change.insert(ART.into(), 200);
        fx.branch_change = Some(100);
        let r = check(&two_citation_reason(false), &fx);
        assert!(
            r.problems.iter().any(|p| p.contains("is `diverged`")),
            "{r:?}"
        );
    }

    #[test]
    fn stale_artifact_voids_the_override_whichever_citation_comes_first() {
        for url_first in [false, true] {
            let mut fx = fixture();
            fx.runs
                .insert("1001".into(), run(Some("success"), "14c12649"));
            fx.compare.insert("14c12649".into(), "ahead".into());
            // Artifact committed before the branch's newest source change.
            fx.last_change.insert(ART.into(), 100);
            fx.branch_change = Some(200);
            let r = check(&two_citation_reason(url_first), &fx);
            assert_eq!(r.problems.len(), 1, "url_first={url_first}: {r:?}");
            assert!(
                r.problems[0].contains("describes the code this change replaced"),
                "{r:?}"
            );
            assert!(
                r.problems[0].contains(ART),
                "the report names the citation: {r:?}"
            );
        }
    }

    #[test]
    fn artifact_regenerated_after_the_code_change_is_fresh() {
        let mut fx = fixture();
        fx.runs
            .insert("1001".into(), run(Some("success"), "14c12649"));
        fx.compare.insert("14c12649".into(), "ahead".into());
        fx.last_change.insert(ART.into(), 300);
        fx.branch_change = Some(200);
        assert!(check(&two_citation_reason(false), &fx).is_fresh());
    }

    #[test]
    fn identical_head_is_the_ordinary_same_head_case() {
        let mut fx = fixture();
        fx.runs.insert("1001".into(), run(Some("success"), HEAD));
        fx.compare.insert(HEAD.into(), "identical".into());
        fx.last_change.insert(ART.into(), 300);
        fx.branch_change = Some(200);
        assert!(check(&two_citation_reason(false), &fx).is_fresh());
    }

    #[test]
    fn untracked_artifact_resolves_to_nothing() {
        let fx = Fixture::default();
        let r = check("trade, see results/baseline_imaginary.json", &fx);
        assert!(
            r.problems.iter().any(|p| p.contains("not a tracked file")),
            "{r:?}"
        );
    }

    #[test]
    fn prose_citations_are_rules_not_measurements() {
        let mut fx = Fixture::default();
        fx.tracked.insert("docs/BENCHMARKING.md".into());
        fx.last_change.insert("docs/BENCHMARKING.md".into(), 1);
        fx.branch_change = Some(999);
        fx.runs
            .insert("1001".into(), run(Some("success"), "14c12649"));
        fx.compare.insert("14c12649".into(), "ahead".into());
        let r = check(
            &format!("exempt under docs/BENCHMARKING.md rule 16, run {RUN}"),
            &fx,
        );
        assert!(r.is_fresh(), "{r:?}");
    }

    #[test]
    fn unavailable_api_is_undecidable_never_a_pass() {
        let fx = fixture();
        let r = check(&format!("coordination trade refs CI run {RUN}"), &fx);
        assert!(r.problems.is_empty(), "{r:?}");
        assert_eq!(r.undecidable.len(), 1, "{r:?}");
        assert!(r.undecidable[0].contains("GitHub API unavailable"), "{r:?}");
        assert!(!r.is_fresh());
    }

    #[test]
    fn without_a_base_the_branch_cannot_be_dated() {
        let mut fx = fixture();
        fx.no_base = true;
        fx.runs
            .insert("1001".into(), run(Some("success"), "14c12649"));
        fx.compare.insert("14c12649".into(), "ahead".into());
        fx.last_change.insert(ART.into(), 1);
        let r = check(&two_citation_reason(false), &fx);
        assert!(r.problems.is_empty(), "{r:?}");
        assert!(r.undecidable.iter().any(|u| u.contains("no base")), "{r:?}");
    }

    #[test]
    fn run_still_in_progress_is_undecidable() {
        let mut fx = fixture();
        fx.runs.insert("1001".into(), run(None, "2cf974a9"));
        fx.compare.insert("2cf974a9".into(), "ahead".into());
        let r = check(&reason_with_run(), &fx);
        assert!(r.problems.is_empty(), "{r:?}");
        assert!(
            r.undecidable.iter().any(|u| u.contains("in progress")),
            "{r:?}"
        );
    }

    #[test]
    fn skipped_workflow_sources_nothing() {
        let mut fx = fixture();
        fx.runs
            .insert("1001".into(), run(Some("skipped"), "2cf974a9"));
        fx.compare.insert("2cf974a9".into(), "ahead".into());
        let r = check(&reason_with_run(), &fx);
        assert!(
            r.problems.iter().any(|p| p.contains("concluded `skipped`")),
            "{r:?}"
        );
    }

    #[test]
    fn branch_that_changes_no_source_has_nothing_to_be_stale_against() {
        let mut fx = fixture();
        fx.runs
            .insert("1001".into(), run(Some("success"), "14c12649"));
        fx.compare.insert("14c12649".into(), "ahead".into());
        fx.last_change.insert(ART.into(), 100);
        fx.branch_change = None;
        assert!(check(&two_citation_reason(false), &fx).is_fresh());
    }

    #[test]
    fn failure_run_without_configured_measurement_jobs_is_undecidable() {
        let fx = failure_run_fixture("ahead", vec![tripped()]);
        let paths = source_paths();
        let policy = FreshnessPolicy {
            measurement_jobs: &[],
            source_paths: &paths,
        };
        let r = check_citation_freshness(&reason_with_run(), &policy, &fx);
        assert!(r.problems.is_empty(), "{r:?}");
        assert!(
            r.undecidable
                .iter()
                .any(|u| u.contains("citation_measurement_jobs")),
            "{r:?}"
        );
    }

    #[test]
    fn data_artifact_without_configured_source_paths_is_undecidable() {
        let mut fx = fixture();
        fx.last_change.insert(ART.into(), 100);
        let jobs = measurement_jobs();
        let policy = FreshnessPolicy {
            measurement_jobs: &jobs,
            source_paths: &[],
        };
        let r = check_citation_freshness(&format!("trade in {ART}"), &policy, &fx);
        assert!(r.problems.is_empty(), "{r:?}");
        assert!(
            r.undecidable
                .iter()
                .any(|u| u.contains("citation_source_paths")),
            "{r:?}"
        );
    }

    #[test]
    fn run_url_outside_github_actions_is_undecidable() {
        let fx = fixture();
        let r = check(
            "trade measured in https://gitlab.example.com/acme/widgets/pipelines/77",
            &fx,
        );
        assert!(r.problems.is_empty(), "{r:?}");
        assert_eq!(r.undecidable.len(), 1, "{r:?}");
        assert!(
            fx.calls.borrow().is_empty(),
            "no API call for a non-GitHub run"
        );
    }

    #[test]
    fn every_citation_is_checked_not_only_the_first() {
        let mut fx = fixture();
        fx.runs
            .insert("1001".into(), run(Some("success"), "14c12649"));
        fx.compare.insert("14c12649".into(), "ahead".into());
        fx.runs
            .insert("1002".into(), run(Some("cancelled"), "14c12649"));
        let reason =
            format!("trade in {RUN} and https://github.com/acme/widgets/actions/runs/1002");
        let r = check(&reason, &fx);
        assert_eq!(r.problems.len(), 1, "{r:?}");
        assert!(r.problems[0].contains("1002"), "{r:?}");
    }

    #[test]
    fn repository_segments_that_walk_the_api_path_are_refused() {
        let fx = fixture();
        let r = check(
            "trade in https://github.com/../widgets/actions/runs/1001",
            &fx,
        );
        assert!(!r.is_fresh(), "{r:?}");
        assert!(
            fx.calls.borrow().is_empty(),
            "no API call for `..` segments"
        );
    }
}
