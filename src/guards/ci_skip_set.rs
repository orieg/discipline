//! Rollup skip-set consistency (`ci-skip-set`).
//!
//! A rollup job (`ci-gate`) has to treat `skipped` as passing, because a
//! conditional matrix skips jobs a change does not touch. On its own that rule
//! cannot tell *skipped because irrelevant* from *skipped because the filter
//! evaluation was wrong*: if change detection succeeds but emits all-false (a
//! path-filter upgrade changing quantifier semantics, a renamed filter key
//! resolving to empty), every conditional job skips, the rollup sees no
//! failure, and a green required context sits over a run that verified
//! nothing.
//!
//! This gate checks the skip set against the data the rollup actually saw. It
//! asserts:
//!
//! 1. the change-detection job succeeded (a filter set nobody computed gates
//!    nothing);
//! 2. the jobs declared unconditional ran — never `skipped`;
//! 3. for every job in the rollup's `needs`, `skipped` holds exactly when its
//!    `if:` evaluates false under the observed outputs and dependency results.
//!
//! The data is runtime-only, so it is supplied by the workflow, never fetched:
//! the rollup passes `${{ toJson(needs) }}` through `DISCIPLINE_CI_CONTEXT`
//! (inline JSON or a path to a file holding it). `github.*` values come from
//! the runner's `GITHUB_*` variables. Without a context the gate reports a
//! named "not evaluated" note: an ordinary diff check has no skip set.
//!
//! Fails closed: unparseable input is an error (exit 2), and an `if:` term the
//! evaluator does not model is a finding, never a guess.

use crate::guards::ci_integrity::parse_all_job_needs;
use crate::guards::{exempt_filter, Context, GateOutcome};
use anyhow::{bail, Context as _, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub const GATE: &str = "ci-skip-set";

/// Environment variable carrying the rollup's `toJson(needs)` (inline JSON,
/// or the path of a file holding it).
pub const CONTEXT_ENV: &str = "DISCIPLINE_CI_CONTEXT";

/// `github.<field>` values the evaluator models, and the runner variable each
/// is read from. Any other `github.*` reference is not understood.
const GITHUB_FIELDS: &[(&str, &str)] = &[
    ("event_name", "GITHUB_EVENT_NAME"),
    ("ref", "GITHUB_REF"),
    ("ref_name", "GITHUB_REF_NAME"),
    ("ref_type", "GITHUB_REF_TYPE"),
    ("base_ref", "GITHUB_BASE_REF"),
    ("head_ref", "GITHUB_HEAD_REF"),
    ("repository", "GITHUB_REPOSITORY"),
    ("repository_owner", "GITHUB_REPOSITORY_OWNER"),
];

const STATUS_FUNCTIONS: &[&str] = &["always", "success", "failure", "cancelled"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingKind {
    /// The change-detection job is not in the rollup's `needs` context.
    ChangeJobMissing,
    /// The change-detection job did not succeed.
    ChangeJobNotSuccess,
    /// A declared unconditional job is not in the `needs` context.
    UnconditionalMissing,
    /// A declared unconditional job was skipped.
    UnconditionalSkipped,
    /// The `needs` context names a job the workflow does not define.
    UnknownJob,
    /// The job's `if:` (or a result) holds something the evaluator does not model.
    NotUnderstood,
    /// Skipped although its `if:` is true under the observed context.
    SkippedWhileGateTrue,
    /// Ran although its `if:` is false under the observed context.
    RanWhileGateFalse,
}

#[derive(Debug, Clone)]
pub struct SkipFinding {
    pub job: String,
    pub kind: FindingKind,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct SkipSetReport {
    /// `needs` entries whose result was checked against the workflow.
    pub examined: usize,
    pub findings: Vec<SkipFinding>,
    pub notes: Vec<String>,
}

pub struct SkipSetSpec<'a> {
    /// Change-detection job that must have succeeded; `None` = the workflow has none.
    pub change_job: Option<&'a str>,
    pub unconditional_jobs: &'a [String],
    /// Modeled `github.*` fields (`event_name`, `ref`, ...). An absent field is
    /// not understood, never defaulted.
    pub github: &'a BTreeMap<String, String>,
}

struct Observed {
    result: String,
    outputs: serde_json::Map<String, serde_json::Value>,
}

/// Parse the rollup's `toJson(needs)`. Every entry must carry a string `result`.
fn parse_needs(needs_json: &str) -> Result<BTreeMap<String, Observed>> {
    let v: serde_json::Value = serde_json::from_str(needs_json)
        .context("the needs context is not valid JSON; pass `${{ toJson(needs) }}`")?;
    let obj = v
        .as_object()
        .context("the needs context must be a JSON object keyed by job id")?;
    let mut out = BTreeMap::new();
    for (job, entry) in obj {
        let result = entry
            .get("result")
            .and_then(|r| r.as_str())
            .with_context(|| format!("needs context entry `{job}` has no string `result`"))?;
        let outputs = match entry.get("outputs") {
            None | Some(serde_json::Value::Null) => serde_json::Map::new(),
            Some(serde_json::Value::Object(m)) => m.clone(),
            Some(_) => bail!("needs context entry `{job}` has non-object `outputs`"),
        };
        out.insert(
            job.clone(),
            Observed {
                result: result.to_string(),
                outputs,
            },
        );
    }
    Ok(out)
}

/// Check a rollup's observed skip set against the workflow that produced it.
///
/// `Err` means the inputs could not be read at all (fail closed, exit 2);
/// anything the rule can read but not verify is a [`FindingKind::NotUnderstood`].
pub fn check_skip_set(
    workflow_src: &str,
    needs_json: &str,
    spec: &SkipSetSpec,
) -> Result<SkipSetReport> {
    let wf: serde_yaml::Value =
        serde_yaml::from_str(workflow_src).context("the workflow is not valid YAML")?;
    let jobs = wf
        .get("jobs")
        .and_then(|j| j.as_mapping())
        .context("the workflow has no `jobs` mapping")?;
    let mut job_ifs: HashMap<String, Option<serde_yaml::Value>> = HashMap::new();
    for (k, v) in jobs {
        if let Some(name) = k.as_str() {
            job_ifs.insert(name.to_string(), v.get("if").cloned());
        }
    }
    let job_needs = parse_all_job_needs(workflow_src);
    let observed = parse_needs(needs_json)?;

    let mut report = SkipSetReport::default();
    let mut find = |job: &str, kind: FindingKind, message: String| {
        report.findings.push(SkipFinding {
            job: job.to_string(),
            kind,
            message,
        });
    };

    // 1. The change-detection job succeeded.
    if let Some(cj) = spec.change_job {
        match observed.get(cj) {
            None => find(
                cj,
                FindingKind::ChangeJobMissing,
                format!(
                    "change-detection job `{cj}` is not in the rollup's needs context, so no \
                     filter output can be verified; add it to the rollup's `needs`"
                ),
            ),
            Some(o) if o.result != "success" => find(
                cj,
                FindingKind::ChangeJobNotSuccess,
                format!(
                    "change-detection job `{cj}` result is `{}`, not `success`; every filter \
                     output the conditional jobs read is unreliable",
                    o.result
                ),
            ),
            Some(_) => {}
        }
    }

    // 2. Declared unconditional jobs ran.
    for u in spec.unconditional_jobs {
        match observed.get(u.as_str()) {
            None => find(
                u,
                FindingKind::UnconditionalMissing,
                format!("`{u}` is declared unconditional but is not in the rollup's needs context"),
            ),
            Some(o) if o.result == "skipped" => find(
                u,
                FindingKind::UnconditionalSkipped,
                format!("`{u}` is declared unconditional but was skipped"),
            ),
            Some(_) => {}
        }
    }

    // 3. Per job: skipped iff its `if:` is false under what was observed.
    let any_cancelled = observed.values().any(|o| o.result == "cancelled");
    // output key -> jobs whose `if:` compares it to a boolean
    let mut unemitted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (job, obs) in &observed {
        report.examined += 1;
        let Some(if_val) = job_ifs.get(job) else {
            find(
                job,
                FindingKind::UnknownJob,
                format!(
                    "the needs context names `{job}`, which the workflow does not define; \
                     is `workflow` pointing at the file this rollup runs in?"
                ),
            );
            continue;
        };
        if spec.unconditional_jobs.iter().any(|u| u == job) {
            continue;
        }
        let ran = match obs.result.as_str() {
            "skipped" => false,
            "success" | "failure" | "cancelled" => true,
            other => {
                find(
                    job,
                    FindingKind::NotUnderstood,
                    format!("`{job}` has result `{other}`, which this rule does not model"),
                );
                continue;
            }
        };
        let own_needs: HashSet<String> = job_needs.get(job).cloned().unwrap_or_default();
        let mut env = Env {
            own_needs: &own_needs,
            ancestors: ancestors(job, &job_needs),
            observed: &observed,
            github: spec.github,
            change_job: spec.change_job,
            any_cancelled,
            unknown: Vec::new(),
            unemitted: BTreeSet::new(),
        };
        let should_run = match if_val {
            None => env.status("success", job),
            Some(serde_yaml::Value::Bool(b)) => {
                // A literal `if: true` still carries the implicit `success()`.
                *b && env.status("success", job)
            }
            Some(serde_yaml::Value::String(src)) => match parse_if(src) {
                Ok(expr) => {
                    let v = env.eval(&expr, job);
                    if expr.has_status_call() {
                        v.truthy()
                    } else {
                        env.status("success", job) && v.truthy()
                    }
                }
                Err(e) => {
                    env.unknown.push(e);
                    false
                }
            },
            Some(_) => {
                env.unknown
                    .push("`if:` is neither a string nor a boolean".into());
                false
            }
        };
        for key in std::mem::take(&mut env.unemitted) {
            unemitted.entry(key).or_default().insert(job.clone());
        }
        if !env.unknown.is_empty() {
            let terms = env.unknown.join("; ");
            find(
                job,
                FindingKind::NotUnderstood,
                format!(
                    "`{job}`: cannot verify its skip decision, the `if:` holds what this rule \
                     does not model: {terms}"
                ),
            );
            continue;
        }
        if !ran && should_run {
            find(
                job,
                FindingKind::SkippedWhileGateTrue,
                format!(
                    "`{job}` was skipped, but its `if:` is true under the observed filter \
                     outputs and dependency results; the matrix was narrowed by something \
                     other than the gate"
                ),
            );
        }
        if ran && !should_run {
            find(
                job,
                FindingKind::RanWhileGateFalse,
                format!(
                    "`{job}` ran with result `{}`, but its `if:` is false under the observed \
                     filter outputs and dependency results",
                    obs.result
                ),
            );
        }
    }

    // Notes: named, not failed. Neither can be told apart from a legitimate
    // run by the data alone; the per-job check above is the sound one.
    if let Some(cj) = spec.change_job {
        for (key, readers) in &unemitted {
            let readers: Vec<String> = readers.iter().map(|j| format!("`{j}`")).collect();
            report.notes.push(format!(
                "`{cj}` emitted no value for output `{key}`, which the `if:` of {} compares to \
                 a boolean; a renamed or removed filter reads as false and skips a job for the \
                 wrong reason",
                readers.join(", ")
            ));
        }
        let is_pr = spec.github.get("event_name").map(String::as_str) == Some("pull_request");
        if let Some(o) = observed.get(cj) {
            let values: Vec<String> = o
                .outputs
                .values()
                .map(|v| Val::from_json(v).to_str().to_ascii_lowercase())
                .collect();
            let flags: Vec<&String> = values
                .iter()
                .filter(|v| *v == "true" || *v == "false")
                .collect();
            let others_empty = values
                .iter()
                .all(|v| v == "true" || v == "false" || v.is_empty());
            if is_pr && !flags.is_empty() && flags.iter().all(|v| *v == "false") && others_empty {
                report.notes.push(format!(
                    "every filter output of `{cj}` is false: expected for a change that touches \
                     no filtered path (a docs-only pull request), and named so that a broken \
                     filter evaluation is visible"
                ));
            }
        }
    }

    Ok(report)
}

/// Every job `job` transitively needs. GitHub's `success()` / `failure()` look
/// at the whole ancestor chain, not only the direct `needs`.
fn ancestors(job: &str, needs: &HashMap<String, HashSet<String>>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<String> = needs
        .get(job)
        .map(|n| n.iter().cloned().collect())
        .unwrap_or_default();
    while let Some(j) = stack.pop() {
        if seen.insert(j.clone()) {
            if let Some(n) = needs.get(&j) {
                stack.extend(n.iter().cloned());
            }
        }
    }
    seen.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Expression evaluation: the subset of GitHub Actions expressions a job-level
// `if:` uses. Anything outside it is reported, never approximated.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    LParen,
    RParen,
    Comma,
    Dot,
    And,
    Or,
    Not,
    Eq,
    Ne,
    Str(String),
    Num(f64),
    Ident(String),
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            '.' => {
                out.push(Tok::Dot);
                i += 1;
            }
            '&' if chars.get(i + 1) == Some(&'&') => {
                out.push(Tok::And);
                i += 2;
            }
            '|' if chars.get(i + 1) == Some(&'|') => {
                out.push(Tok::Or);
                i += 2;
            }
            '=' if chars.get(i + 1) == Some(&'=') => {
                out.push(Tok::Eq);
                i += 2;
            }
            '!' if chars.get(i + 1) == Some(&'=') => {
                out.push(Tok::Ne);
                i += 2;
            }
            '!' => {
                out.push(Tok::Not);
                i += 1;
            }
            '\'' => {
                let mut s = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return Err("unterminated string literal".into()),
                        Some('\'') if chars.get(i + 1) == Some(&'\'') => {
                            s.push('\'');
                            i += 2;
                        }
                        Some('\'') => {
                            i += 1;
                            break;
                        }
                        Some(ch) => {
                            s.push(*ch);
                            i += 1;
                        }
                    }
                }
                out.push(Tok::Str(s));
            }
            c if c.is_ascii_digit() => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                let n = text
                    .parse::<f64>()
                    .map_err(|_| format!("number literal `{text}`"))?;
                out.push(Tok::Num(n));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '-')
                {
                    i += 1;
                }
                out.push(Tok::Ident(chars[start..i].iter().collect()));
            }
            other => return Err(format!("operator or character `{other}`")),
        }
    }
    Ok(out)
}

#[derive(Debug, Clone)]
enum Expr {
    Lit(Val),
    /// Property path, e.g. `needs.detect-changes.outputs.rust-src`.
    Path(Vec<String>),
    Call(String, Vec<Expr>),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    /// `a == b` (`negated` = `!=`).
    Cmp(Box<Expr>, Box<Expr>, bool),
}

impl Expr {
    /// GitHub prepends `success() &&` unless the expression calls a status function.
    fn has_status_call(&self) -> bool {
        match self {
            Expr::Call(name, args) => {
                STATUS_FUNCTIONS.contains(&name.to_ascii_lowercase().as_str())
                    || args.iter().any(Expr::has_status_call)
            }
            Expr::Not(e) => e.has_status_call(),
            Expr::And(a, b) | Expr::Or(a, b) | Expr::Cmp(a, b, _) => {
                a.has_status_call() || b.has_status_call()
            }
            Expr::Lit(_) | Expr::Path(_) => false,
        }
    }

    fn render(&self) -> String {
        match self {
            Expr::Lit(v) => match v {
                Val::Str(s) => format!("'{s}'"),
                other => other.to_str(),
            },
            Expr::Path(p) => p.join("."),
            Expr::Call(n, a) => format!(
                "{n}({})",
                a.iter().map(Expr::render).collect::<Vec<_>>().join(", ")
            ),
            Expr::Not(e) => format!("!{}", e.render()),
            Expr::And(a, b) => format!("{} && {}", a.render(), b.render()),
            Expr::Or(a, b) => format!("{} || {}", a.render(), b.render()),
            Expr::Cmp(a, b, neg) => {
                format!(
                    "{} {} {}",
                    a.render(),
                    if *neg { "!=" } else { "==" },
                    b.render()
                )
            }
        }
    }
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn or(&mut self) -> Result<Expr, String> {
        let mut e = self.and()?;
        while self.peek() == Some(&Tok::Or) {
            self.pos += 1;
            e = Expr::Or(Box::new(e), Box::new(self.and()?));
        }
        Ok(e)
    }
    fn and(&mut self) -> Result<Expr, String> {
        let mut e = self.unary()?;
        while self.peek() == Some(&Tok::And) {
            self.pos += 1;
            e = Expr::And(Box::new(e), Box::new(self.unary()?));
        }
        Ok(e)
    }
    fn unary(&mut self) -> Result<Expr, String> {
        if self.peek() == Some(&Tok::Not) {
            self.pos += 1;
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        let lhs = self.primary()?;
        match self.peek() {
            Some(Tok::Eq) | Some(Tok::Ne) => {
                let neg = self.next() == Some(Tok::Ne);
                let rhs = self.primary()?;
                if matches!(self.peek(), Some(Tok::Eq) | Some(Tok::Ne)) {
                    return Err("chained comparison".into());
                }
                Ok(Expr::Cmp(Box::new(lhs), Box::new(rhs), neg))
            }
            _ => Ok(lhs),
        }
    }
    fn primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Tok::LParen) => {
                let e = self.or()?;
                if self.next() != Some(Tok::RParen) {
                    return Err("unbalanced parentheses".into());
                }
                Ok(e)
            }
            Some(Tok::Not) => Ok(Expr::Not(Box::new(self.primary()?))),
            Some(Tok::Str(s)) => Ok(Expr::Lit(Val::Str(s))),
            Some(Tok::Num(n)) => Ok(Expr::Lit(Val::Num(n))),
            Some(Tok::Ident(id)) => {
                match id.as_str() {
                    "true" => return Ok(Expr::Lit(Val::Bool(true))),
                    "false" => return Ok(Expr::Lit(Val::Bool(false))),
                    "null" => return Ok(Expr::Lit(Val::Null)),
                    _ => {}
                }
                if self.peek() == Some(&Tok::LParen) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if self.peek() == Some(&Tok::RParen) {
                        self.pos += 1;
                    } else {
                        loop {
                            args.push(self.or()?);
                            match self.next() {
                                Some(Tok::Comma) => continue,
                                Some(Tok::RParen) => break,
                                _ => return Err(format!("malformed call to `{id}`")),
                            }
                        }
                    }
                    return Ok(Expr::Call(id, args));
                }
                let mut path = vec![id];
                while self.peek() == Some(&Tok::Dot) {
                    self.pos += 1;
                    match self.next() {
                        Some(Tok::Ident(seg)) => path.push(seg),
                        _ => return Err(format!("malformed property path `{}`", path.join("."))),
                    }
                }
                Ok(Expr::Path(path))
            }
            Some(t) => Err(format!("unexpected token {t:?}")),
            None => Err("unexpected end of expression".into()),
        }
    }
}

/// Parse a job-level `if:`. A value wrapped whole in `${{ }}` is unwrapped.
fn parse_if(src: &str) -> Result<Expr, String> {
    let mut body = src.trim();
    if let Some(inner) = body.strip_prefix("${{").and_then(|b| b.strip_suffix("}}")) {
        body = inner.trim();
    }
    if body.contains("${{") {
        return Err(format!("mixed literal and `${{{{ }}}}` in `{body}`"));
    }
    let mut p = Parser {
        toks: lex(body).map_err(|e| format!("unsupported syntax in `{body}`: {e}"))?,
        pos: 0,
    };
    let e = p
        .or()
        .map_err(|e| format!("unsupported syntax in `{body}`: {e}"))?;
    if p.pos < p.toks.len() {
        return Err(format!("unsupported syntax in `{body}`: trailing tokens"));
    }
    Ok(e)
}

#[derive(Debug, Clone, PartialEq)]
enum Val {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
}

impl Val {
    fn from_json(v: &serde_json::Value) -> Val {
        match v {
            serde_json::Value::Null => Val::Null,
            serde_json::Value::Bool(b) => Val::Bool(*b),
            serde_json::Value::Number(n) => Val::Num(n.as_f64().unwrap_or(f64::NAN)),
            serde_json::Value::String(s) => Val::Str(s.clone()),
            other => Val::Str(other.to_string()),
        }
    }
    fn truthy(&self) -> bool {
        match self {
            Val::Null => false,
            Val::Bool(b) => *b,
            Val::Num(n) => *n != 0.0 && !n.is_nan(),
            Val::Str(s) => !s.is_empty(),
        }
    }
    fn to_num(&self) -> f64 {
        match self {
            Val::Null => 0.0,
            Val::Bool(b) => f64::from(u8::from(*b)),
            Val::Num(n) => *n,
            Val::Str(s) => {
                let t = s.trim();
                if t.is_empty() {
                    0.0
                } else {
                    t.parse::<f64>().unwrap_or(f64::NAN)
                }
            }
        }
    }
    fn to_str(&self) -> String {
        match self {
            Val::Null => String::new(),
            Val::Bool(b) => b.to_string(),
            Val::Num(n) => n.to_string(),
            Val::Str(s) => s.clone(),
        }
    }
    /// GitHub's loose equality: same type compares directly (strings ignore
    /// case); mixed types are coerced to numbers, and NaN equals nothing.
    fn loose_eq(&self, other: &Val) -> bool {
        match (self, other) {
            (Val::Null, Val::Null) => true,
            (Val::Bool(a), Val::Bool(b)) => a == b,
            (Val::Str(a), Val::Str(b)) => a.to_lowercase() == b.to_lowercase(),
            (Val::Num(a), Val::Num(b)) => a == b,
            (a, b) => {
                let (x, y) = (a.to_num(), b.to_num());
                !x.is_nan() && !y.is_nan() && x == y
            }
        }
    }
}

struct Env<'a> {
    own_needs: &'a HashSet<String>,
    ancestors: Vec<String>,
    observed: &'a BTreeMap<String, Observed>,
    github: &'a BTreeMap<String, String>,
    change_job: Option<&'a str>,
    any_cancelled: bool,
    unknown: Vec<String>,
    /// Change-detection outputs compared to a boolean but never emitted.
    unemitted: BTreeSet<String>,
}

impl Env<'_> {
    fn status(&mut self, name: &str, job: &str) -> bool {
        match name {
            "always" => true,
            "cancelled" => self.any_cancelled,
            "success" | "failure" => {
                let mut results = Vec::new();
                for a in &self.ancestors {
                    match self.observed.get(a) {
                        Some(o) => results.push(o.result.clone()),
                        None => {
                            self.unknown.push(format!(
                                "`{job}` depends on `{a}`, whose result is not in the rollup's \
                                 needs context"
                            ));
                            return false;
                        }
                    }
                }
                if name == "success" {
                    results.iter().all(|r| r == "success")
                } else {
                    results.iter().any(|r| r == "failure")
                }
            }
            _ => unreachable!("only status functions reach here"),
        }
    }

    /// Every sub-expression is evaluated (no short-circuit) so an
    /// unmodelled term anywhere in the `if:` is reported.
    fn eval(&mut self, e: &Expr, job: &str) -> Val {
        match e {
            Expr::Lit(v) => v.clone(),
            Expr::Path(p) => self.resolve(p, job),
            Expr::Not(x) => Val::Bool(!self.eval(x, job).truthy()),
            Expr::And(a, b) => {
                let (l, r) = (self.eval(a, job), self.eval(b, job));
                if l.truthy() {
                    r
                } else {
                    l
                }
            }
            Expr::Or(a, b) => {
                let (l, r) = (self.eval(a, job), self.eval(b, job));
                if l.truthy() {
                    l
                } else {
                    r
                }
            }
            Expr::Cmp(a, b, neg) => {
                self.note_unemitted(a, b);
                self.note_unemitted(b, a);
                let (l, r) = (self.eval(a, job), self.eval(b, job));
                Val::Bool(l.loose_eq(&r) != *neg)
            }
            Expr::Call(name, args) => {
                let lname = name.to_ascii_lowercase();
                if STATUS_FUNCTIONS.contains(&lname.as_str()) {
                    if !args.is_empty() {
                        self.unknown.push(format!("`{}`", e.render()));
                        return Val::Null;
                    }
                    return Val::Bool(self.status(&lname, job));
                }
                let vals: Vec<Val> = args.iter().map(|a| self.eval(a, job)).collect();
                match (lname.as_str(), vals.as_slice()) {
                    ("contains", [hay, needle]) => Val::Bool(
                        hay.to_str()
                            .to_lowercase()
                            .contains(&needle.to_str().to_lowercase()),
                    ),
                    ("startswith", [s, p]) => Val::Bool(
                        s.to_str()
                            .to_lowercase()
                            .starts_with(&p.to_str().to_lowercase()),
                    ),
                    ("endswith", [s, p]) => Val::Bool(
                        s.to_str()
                            .to_lowercase()
                            .ends_with(&p.to_str().to_lowercase()),
                    ),
                    _ => {
                        self.unknown.push(format!("`{}`", e.render()));
                        Val::Null
                    }
                }
            }
        }
    }

    fn note_unemitted(&mut self, path: &Expr, other: &Expr) {
        let (Some(cj), Expr::Path(p), Expr::Lit(Val::Str(lit))) = (self.change_job, path, other)
        else {
            return;
        };
        if p.len() == 4
            && p[0] == "needs"
            && p[1] == cj
            && p[2] == "outputs"
            && (lit.eq_ignore_ascii_case("true") || lit.eq_ignore_ascii_case("false"))
        {
            let emitted = self
                .observed
                .get(cj)
                .is_some_and(|o| o.outputs.contains_key(&p[3]));
            if !emitted {
                self.unemitted.insert(p[3].clone());
            }
        }
    }

    fn resolve(&mut self, p: &[String], job: &str) -> Val {
        let text = p.join(".");
        match p.first().map(String::as_str) {
            Some("needs") if p.len() >= 3 => {
                let dep = &p[1];
                if !self.own_needs.contains(dep) {
                    self.unknown.push(format!(
                        "`{text}`: `{dep}` is not in `{job}`'s own `needs`, so GitHub does not \
                         expose it here"
                    ));
                    return Val::Null;
                }
                let Some(obs) = self.observed.get(dep) else {
                    self.unknown.push(format!(
                        "`{text}`: `{dep}` is not in the rollup's needs context"
                    ));
                    return Val::Null;
                };
                match (p[2].as_str(), p.len()) {
                    ("result", 3) => Val::Str(obs.result.clone()),
                    ("outputs", 4) => obs.outputs.get(&p[3]).map_or(Val::Null, Val::from_json),
                    _ => {
                        self.unknown.push(format!("`{text}`"));
                        Val::Null
                    }
                }
            }
            Some("github") if p.len() == 2 && GITHUB_FIELDS.iter().any(|(f, _)| *f == p[1]) => {
                match self.github.get(&p[1]) {
                    Some(v) => Val::Str(v.clone()),
                    None => {
                        let var = GITHUB_FIELDS
                            .iter()
                            .find(|(f, _)| *f == p[1])
                            .map(|(_, v)| *v)
                            .unwrap_or_default();
                        self.unknown
                            .push(format!("`{text}`: `{var}` is not set in this environment"));
                        Val::Null
                    }
                }
            }
            _ => {
                self.unknown.push(format!("`{text}`"));
                Val::Null
            }
        }
    }
}

/// Evaluate an expression with no job context (used by `self-test` and tests):
/// returns its truthiness and the terms the evaluator did not model.
pub fn eval_standalone(src: &str, github: &BTreeMap<String, String>) -> (bool, Vec<String>) {
    let expr = match parse_if(src) {
        Ok(e) => e,
        Err(e) => return (false, vec![e]),
    };
    let none = HashSet::new();
    let observed = BTreeMap::new();
    let mut env = Env {
        own_needs: &none,
        ancestors: Vec::new(),
        observed: &observed,
        github,
        change_job: None,
        any_cancelled: false,
        unknown: Vec::new(),
        unemitted: BTreeSet::new(),
    };
    let v = env.eval(&expr, "<expression>");
    (v.truthy(), env.unknown)
}

/// The modeled `github.*` fields, read from the runner's environment.
pub fn github_from_env() -> BTreeMap<String, String> {
    GITHUB_FIELDS
        .iter()
        .filter_map(|(field, var)| std::env::var(var).ok().map(|v| (field.to_string(), v)))
        .collect()
}

/// Line of the `<job>:` key in the workflow, for the report.
fn job_line(workflow_src: &str, job: &str) -> Option<usize> {
    let key = format!("{job}:");
    workflow_src
        .lines()
        .position(|l| l.starts_with(' ') && l.trim() == key)
        .map(|i| i + 1)
}

fn registered(kind: FindingKind) -> &'static crate::findings::FindingKind {
    use crate::findings as f;
    match kind {
        FindingKind::ChangeJobMissing => &f::CHANGE_JOB_MISSING_FROM_NEEDS,
        FindingKind::ChangeJobNotSuccess => &f::CHANGE_JOB_DID_NOT_SUCCEED,
        FindingKind::UnconditionalMissing => &f::UNCONDITIONAL_JOB_MISSING_FROM_NEEDS,
        FindingKind::UnconditionalSkipped => &f::UNCONDITIONAL_JOB_SKIPPED,
        FindingKind::UnknownJob => &f::NEEDS_NAMES_UNDEFINED_JOB,
        FindingKind::NotUnderstood => &f::SKIP_DECISION_UNVERIFIABLE,
        FindingKind::SkippedWhileGateTrue => &f::JOB_SKIPPED_WHILE_CONDITION_TRUE,
        FindingKind::RanWhileGateFalse => &f::JOB_RAN_WHILE_CONDITION_FALSE,
    }
}

fn title(kind: FindingKind, job: &str) -> String {
    match kind {
        FindingKind::ChangeJobMissing => {
            format!("change-detection job `{job}` missing from the needs context")
        }
        FindingKind::ChangeJobNotSuccess => format!("change-detection job `{job}` did not succeed"),
        FindingKind::UnconditionalMissing => {
            format!("unconditional job `{job}` missing from the needs context")
        }
        FindingKind::UnconditionalSkipped => format!("unconditional job `{job}` was skipped"),
        FindingKind::UnknownJob => format!("needs context names undefined job `{job}`"),
        FindingKind::NotUnderstood => format!("`{job}` skip decision cannot be verified"),
        FindingKind::SkippedWhileGateTrue => format!("`{job}` skipped although its `if:` is true"),
        FindingKind::RanWhileGateFalse => format!("`{job}` ran although its `if:` is false"),
    }
}

fn remediation(kind: FindingKind) -> &'static str {
    match kind {
        FindingKind::ChangeJobMissing | FindingKind::UnconditionalMissing => {
            "Add the job to the rollup's `needs` so its result is in `toJson(needs)`, or correct `[gates.ci-skip-set]`."
        }
        FindingKind::ChangeJobNotSuccess => {
            "Fix the change-detection job; no conditional job's skip can be trusted until it succeeds."
        }
        FindingKind::UnconditionalSkipped => {
            "An unconditional job must run on every change; find what skipped it (a failed or skipped dependency, an added `if:`)."
        }
        FindingKind::UnknownJob => {
            "Point `[gates.ci-skip-set] workflow` at the workflow file this rollup runs in."
        }
        FindingKind::NotUnderstood => {
            "Rewrite the `if:` in the modeled subset (needs.<job>.outputs/result, github.event_name/ref/..., ==, !=, !, &&, ||, contains/startsWith/endsWith, status functions), or exempt the workflow."
        }
        FindingKind::SkippedWhileGateTrue => {
            "The job should have run. Check the filter action's version and quantifier, renamed filter keys, and the job's dependencies; re-run once fixed."
        }
        FindingKind::RanWhileGateFalse => {
            "The job ran under a false gate: the observed outputs disagree with what the runner evaluated. Check for a stale or re-run change-detection result."
        }
    }
}

fn load_context(raw: &str) -> Result<String> {
    let t = raw.trim();
    if t.starts_with('{') {
        return Ok(t.to_string());
    }
    std::fs::read_to_string(t)
        .with_context(|| format!("{CONTEXT_ENV} names `{t}`, which could not be read"))
        .map_err(|e| crate::could_not_check::tag(crate::could_not_check::Reason::Configuration, e))
}

/// The configured default, which only fits GitHub Actions.
pub const DEFAULT_WORKFLOW: &str = ".github/workflows/ci.yml";

/// The workflow whose rollup supplies the context. An explicit `workflow` is used as is.
/// Left at the default, the running workflow named by `GITHUB_WORKFLOW_REF`
/// (`owner/repo/<path>@<ref>`) wins, then the first `ci.yml` that exists under
/// `.github/`, `.gitea/` or `.forgejo/workflows/`.
pub fn resolve_workflow(
    configured: &str,
    workflow_ref: Option<&str>,
    exists: impl Fn(&str) -> bool,
) -> Result<String> {
    if configured != DEFAULT_WORKFLOW {
        return Ok(configured.to_string());
    }
    if let Some(r) = workflow_ref {
        let path = r.split('@').next().unwrap_or_default();
        // Drop `owner/repo/`: the workflow path starts at the first dot directory.
        if let Some(i) = path.find("/.") {
            let candidate = &path[i + 1..];
            if exists(candidate) {
                return Ok(candidate.to_string());
            }
        }
    }
    let candidates = [
        DEFAULT_WORKFLOW,
        ".gitea/workflows/ci.yml",
        ".forgejo/workflows/ci.yml",
    ];
    candidates
        .iter()
        .find(|c| exists(c))
        .map(|c| c.to_string())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no workflow to check: set `[gates.ci-skip-set] workflow` (tried {})",
                candidates.join(", ")
            )
        })
}

pub fn evaluate_ci_skip_set(ctx: &Context) -> Result<GateOutcome> {
    let raw = std::env::var(CONTEXT_ENV).ok();
    evaluate_with(ctx, raw.as_deref(), &github_from_env())
}

fn evaluate_with(
    ctx: &Context,
    raw_context: Option<&str>,
    github: &BTreeMap<String, String>,
) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.ci_skip_set;
    let mut out = GateOutcome::new(GATE);

    let Some(raw) = raw_context.filter(|r| !r.trim().is_empty()) else {
        out.notes.push(format!(
            "not evaluated: {CONTEXT_ENV} is not set. This rule checks a rollup job's runtime \
             `needs` context and runs only when the rollup supplies it; a diff check has no \
             skip set to verify"
        ));
        return Ok(out);
    };
    let workflow = resolve_workflow(
        &settings.workflow,
        std::env::var("GITHUB_WORKFLOW_REF").ok().as_deref(),
        |p| ctx.git.head_content(p).ok().flatten().is_some(),
    )?;
    if workflow != settings.workflow {
        out.notes.push(format!("workflow: `{workflow}` (detected)"));
    }
    if exempt_filter(settings)?.matches(&workflow) {
        out.notes.push(format!(
            "not evaluated: `{workflow}` matches `exempt_paths`"
        ));
        return Ok(out);
    }

    let needs_json = load_context(raw)?;
    let workflow_src = ctx
        .git
        .head_content(&workflow)?
        .with_context(|| {
            format!("`[gates.ci-skip-set] workflow = \"{workflow}\"` is not a file at HEAD")
        })
        .map_err(|e| {
            crate::could_not_check::tag(crate::could_not_check::Reason::Configuration, e)
        })?;
    let change_job = Some(settings.change_job.as_str()).filter(|s| !s.is_empty());
    let spec = SkipSetSpec {
        change_job,
        unconditional_jobs: &settings.unconditional_jobs,
        github,
    };
    let report = check_skip_set(&workflow_src, &needs_json, &spec)
        .with_context(|| format!("could not read the skip set of `{workflow}`"))?;

    out.examined = report.examined;
    out.notes.extend(report.notes);
    for f in report.findings {
        out.push_site(
            settings.severity,
            registered(f.kind),
            title(f.kind, &f.job),
            (Some(&workflow), job_line(&workflow_src, &f.job)),
            f.message,
            remediation(f.kind),
        );
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;

    /// The reference consumer's rollup fixture: a change-detection job, one
    /// unconditional job, and two jobs gated on a path filter.
    const WORKFLOW: &str = r#"
on: pull_request
jobs:
  detect-changes:
    runs-on: ubuntu-latest
    outputs:
      tooling: ${{ steps.filter.outputs.tooling }}
      rust-src: ${{ steps.filter.outputs.rust-src }}
    steps:
      - run: "true"
  docs-lint:
    runs-on: ubuntu-latest
    steps:
      - run: "true"
  lint:
    needs: detect-changes
    if: needs.detect-changes.outputs.tooling == 'true'
    runs-on: ubuntu-latest
    steps:
      - run: "true"
  miri:
    needs: [detect-changes]
    if: ${{ needs.detect-changes.outputs.rust-src == 'true' }}
    runs-on: ubuntu-latest
    steps:
      - run: "true"
  ci-gate:
    if: always()
    needs: [detect-changes, docs-lint, lint, miri]
    runs-on: ubuntu-latest
    steps:
      - run: "true"
"#;

    fn needs(results: &[(&str, &str)], outputs: &[(&str, &str)]) -> String {
        let mut m = serde_json::Map::new();
        for (job, result) in results {
            let mut entry = serde_json::Map::new();
            entry.insert("result".into(), serde_json::Value::from(*result));
            let mut outs = serde_json::Map::new();
            if *job == "detect-changes" {
                for (k, v) in outputs {
                    outs.insert((*k).into(), serde_json::Value::from(*v));
                }
            }
            entry.insert("outputs".into(), serde_json::Value::Object(outs));
            m.insert((*job).into(), serde_json::Value::Object(entry));
        }
        serde_json::Value::Object(m).to_string()
    }

    fn github(event: &str) -> BTreeMap<String, String> {
        BTreeMap::from([("event_name".to_string(), event.to_string())])
    }

    fn run(workflow: &str, ctx: &str, event: &str) -> SkipSetReport {
        run_u(workflow, ctx, event, &["docs-lint"])
    }

    fn run_u(workflow: &str, ctx: &str, event: &str, unconditional: &[&str]) -> SkipSetReport {
        let unconditional: Vec<String> = unconditional.iter().map(|s| s.to_string()).collect();
        let gh = github(event);
        let spec = SkipSetSpec {
            change_job: Some("detect-changes"),
            unconditional_jobs: &unconditional,
            github: &gh,
        };
        check_skip_set(workflow, ctx, &spec).expect("inputs parse")
    }

    fn kinds(r: &SkipSetReport) -> Vec<(String, FindingKind)> {
        r.findings.iter().map(|f| (f.job.clone(), f.kind)).collect()
    }

    const OK: &[(&str, &str)] = &[
        ("detect-changes", "success"),
        ("docs-lint", "success"),
        ("lint", "success"),
        ("miri", "skipped"),
    ];
    const OK_OUT: &[(&str, &str)] = &[("tooling", "true"), ("rust-src", "false")];

    // ---- ported controls: same fixtures, same verdicts ----------------------

    #[test]
    fn consistent_run_passes() {
        let r = run(WORKFLOW, &needs(OK, OK_OUT), "pull_request");
        assert!(r.findings.is_empty(), "{:?}", kinds(&r));
        assert_eq!(r.examined, 4);
    }

    #[test]
    fn docs_only_all_false_passes_with_one_notice() {
        let docs_only = [
            ("detect-changes", "success"),
            ("docs-lint", "success"),
            ("lint", "skipped"),
            ("miri", "skipped"),
        ];
        let out = [("tooling", "false"), ("rust-src", "false")];
        let r = run(WORKFLOW, &needs(&docs_only, &out), "pull_request");
        assert!(r.findings.is_empty(), "{:?}", kinds(&r));
        assert_eq!(r.notes.len(), 1, "{:?}", r.notes);
        assert!(r.notes[0].contains("every filter output"), "{:?}", r.notes);
    }

    #[test]
    fn job_that_ran_under_a_false_gate_is_caught() {
        let broken = [
            ("detect-changes", "success"),
            ("docs-lint", "success"),
            ("lint", "success"),
            ("miri", "skipped"),
        ];
        let out = [("tooling", "false"), ("rust-src", "false")];
        let r = run(WORKFLOW, &needs(&broken, &out), "pull_request");
        assert_eq!(
            kinds(&r),
            vec![("lint".to_string(), FindingKind::RanWhileGateFalse)]
        );
    }

    /// The defect the reference pins verbatim: an id list read by
    /// `contains(...)`, not `== 'true'`. Dropping it made every job that ran
    /// because its own definition changed look like one that ran with a
    /// false gate.
    const JOBDIFF: &str = r#"
jobs:
  detect-changes:
    runs-on: ubuntu-latest
  miri:
    needs: detect-changes
    if: needs.detect-changes.outputs.rust-src == 'true' || contains(needs.detect-changes.outputs.changed-jobs, '|miri|')
    runs-on: ubuntu-latest
"#;

    #[test]
    fn job_run_via_changed_jobs_is_consistent() {
        let st = [("detect-changes", "success"), ("miri", "success")];
        let out = [("rust-src", "false"), ("changed-jobs", "|miri|")];
        let r = run_u(JOBDIFF, &needs(&st, &out), "pull_request", &[]);
        assert!(r.findings.is_empty(), "{:?}", kinds(&r));
    }

    #[test]
    fn skipped_despite_changed_jobs_is_caught() {
        let st = [("detect-changes", "success"), ("miri", "skipped")];
        let out = [("rust-src", "false"), ("changed-jobs", "|miri|")];
        let r = run_u(JOBDIFF, &needs(&st, &out), "pull_request", &[]);
        assert_eq!(
            kinds(&r),
            vec![("miri".to_string(), FindingKind::SkippedWhileGateTrue)]
        );
    }

    #[test]
    fn skipped_but_should_run_is_caught() {
        let mut bad = OK.to_vec();
        bad[2] = ("lint", "skipped");
        let r = run(WORKFLOW, &needs(&bad, OK_OUT), "pull_request");
        assert_eq!(
            kinds(&r),
            vec![("lint".to_string(), FindingKind::SkippedWhileGateTrue)]
        );
    }

    #[test]
    fn unconditional_skip_is_caught() {
        let mut bad = OK.to_vec();
        bad[1] = ("docs-lint", "skipped");
        let r = run(WORKFLOW, &needs(&bad, OK_OUT), "pull_request");
        assert_eq!(
            kinds(&r),
            vec![("docs-lint".to_string(), FindingKind::UnconditionalSkipped)]
        );
    }

    #[test]
    fn failed_change_detection_is_caught() {
        let mut bad = OK.to_vec();
        bad[0] = ("detect-changes", "failure");
        let r = run(WORKFLOW, &needs(&bad, OK_OUT), "pull_request");
        assert!(
            kinds(&r).contains(&(
                "detect-changes".to_string(),
                FindingKind::ChangeJobNotSuccess
            )),
            "{:?}",
            kinds(&r)
        );
    }

    #[test]
    fn push_fallback_is_honoured() {
        let wf = r#"
jobs:
  detect-changes:
    runs-on: ubuntu-latest
  t:
    needs: detect-changes
    if: needs.detect-changes.outputs.rust-src == 'true' || github.event_name != 'pull_request'
    runs-on: ubuntu-latest
"#;
        let st = [("detect-changes", "success"), ("t", "skipped")];
        let out = [("rust-src", "false")];
        let pushed = run_u(wf, &needs(&st, &out), "push", &[]);
        assert_eq!(
            kinds(&pushed),
            vec![("t".to_string(), FindingKind::SkippedWhileGateTrue)]
        );
        // Positive control: the same skip on a pull request is consistent.
        let pr = run_u(wf, &needs(&st, &out), "pull_request", &[]);
        assert!(pr.findings.is_empty(), "{:?}", kinds(&pr));
    }

    // ---- fail-closed evaluation ---------------------------------------------

    #[test]
    fn unknown_if_term_is_an_error_not_a_guess() {
        let wf = WORKFLOW.replace(
            "if: needs.detect-changes.outputs.tooling == 'true'",
            "if: needs.detect-changes.outputs.tooling == 'true' || github.event.pull_request.draft",
        );
        let r = run(&wf, &needs(OK, OK_OUT), "pull_request");
        assert_eq!(
            kinds(&r),
            vec![("lint".to_string(), FindingKind::NotUnderstood)]
        );
        assert!(r.findings[0]
            .message
            .contains("github.event.pull_request.draft"));
    }

    #[test]
    fn unsupported_syntax_is_an_error() {
        let wf = WORKFLOW.replace(
            "if: needs.detect-changes.outputs.tooling == 'true'",
            "if: fromJSON(needs.detect-changes.outputs.tooling)",
        );
        let r = run(&wf, &needs(OK, OK_OUT), "pull_request");
        assert_eq!(
            kinds(&r),
            vec![("lint".to_string(), FindingKind::NotUnderstood)]
        );
    }

    #[test]
    fn needs_reference_outside_the_jobs_own_needs_is_an_error() {
        let wf = WORKFLOW.replace("  lint:\n    needs: detect-changes\n", "  lint:\n");
        let r = run(&wf, &needs(OK, OK_OUT), "pull_request");
        assert_eq!(
            kinds(&r),
            vec![("lint".to_string(), FindingKind::NotUnderstood)]
        );
    }

    #[test]
    fn missing_event_name_is_not_guessed() {
        let wf = r#"
jobs:
  t:
    if: github.event_name == 'push'
    runs-on: ubuntu-latest
"#;
        let empty = BTreeMap::new();
        let spec = SkipSetSpec {
            change_job: None,
            unconditional_jobs: &[],
            github: &empty,
        };
        let r = check_skip_set(wf, &needs(&[("t", "skipped")], &[]), &spec).unwrap();
        assert_eq!(
            kinds(&r),
            vec![("t".to_string(), FindingKind::NotUnderstood)]
        );
    }

    #[test]
    fn missing_change_job_and_unconditional_job_are_errors() {
        let st = [("lint", "success"), ("miri", "skipped")];
        let r = run(WORKFLOW, &needs(&st, OK_OUT), "pull_request");
        let k = kinds(&r);
        assert!(
            k.contains(&("detect-changes".to_string(), FindingKind::ChangeJobMissing)),
            "{k:?}"
        );
        assert!(
            k.contains(&("docs-lint".to_string(), FindingKind::UnconditionalMissing)),
            "{k:?}"
        );
    }

    #[test]
    fn context_job_absent_from_workflow_is_an_error() {
        let mut st = OK.to_vec();
        st.push(("renamed-job", "skipped"));
        let r = run(WORKFLOW, &needs(&st, OK_OUT), "pull_request");
        assert_eq!(
            kinds(&r),
            vec![("renamed-job".to_string(), FindingKind::UnknownJob)]
        );
    }

    #[test]
    fn unparseable_inputs_fail_closed() {
        let gh = github("pull_request");
        let spec = SkipSetSpec {
            change_job: Some("detect-changes"),
            unconditional_jobs: &[],
            github: &gh,
        };
        assert!(check_skip_set(WORKFLOW, "not json", &spec).is_err());
        assert!(check_skip_set(WORKFLOW, "[1, 2]", &spec).is_err());
        assert!(check_skip_set(WORKFLOW, r#"{"lint": {"outputs": {}}}"#, &spec).is_err());
        assert!(check_skip_set("jobs: [", &needs(OK, OK_OUT), &spec).is_err());
        assert!(check_skip_set("name: x", &needs(OK, OK_OUT), &spec).is_err());
    }

    // ---- GitHub status-function semantics -----------------------------------

    /// With no status function in `if:`, GitHub prepends `success()`: a job
    /// whose dependency skipped is skipped too, and that is consistent.
    #[test]
    fn implicit_success_follows_skipped_ancestor() {
        let wf = r#"
jobs:
  detect-changes:
    runs-on: ubuntu-latest
  build:
    needs: detect-changes
    if: needs.detect-changes.outputs.rust-src == 'true'
    runs-on: ubuntu-latest
  bench:
    needs: build
    runs-on: ubuntu-latest
  report:
    needs: build
    if: always()
    runs-on: ubuntu-latest
  docs:
    needs: [detect-changes, build]
    if: needs.detect-changes.outputs.rust-src == 'false'
    runs-on: ubuntu-latest
"#;
        let out = [("rust-src", "false")];
        // `docs`'s filter term is true, but `build` skipped, so the implicit
        // `success()` is false and GitHub skips it: consistent.
        let st = [
            ("detect-changes", "success"),
            ("build", "skipped"),
            ("bench", "skipped"),
            ("report", "success"),
            ("docs", "skipped"),
        ];
        let r = run_u(wf, &needs(&st, &out), "pull_request", &[]);
        assert!(r.findings.is_empty(), "{:?}", kinds(&r));
        // Negative control: `always()` means `report` must run.
        let st2 = [
            ("detect-changes", "success"),
            ("build", "skipped"),
            ("bench", "skipped"),
            ("report", "skipped"),
            ("docs", "skipped"),
        ];
        let r2 = run_u(wf, &needs(&st2, &out), "pull_request", &[]);
        assert_eq!(
            kinds(&r2),
            vec![("report".to_string(), FindingKind::SkippedWhileGateTrue)]
        );
    }

    #[test]
    fn ancestor_missing_from_context_is_not_guessed() {
        let wf = r#"
jobs:
  build:
    runs-on: ubuntu-latest
  bench:
    needs: build
    runs-on: ubuntu-latest
"#;
        let empty = BTreeMap::new();
        let spec = SkipSetSpec {
            change_job: None,
            unconditional_jobs: &[],
            github: &empty,
        };
        let r = check_skip_set(wf, &needs(&[("bench", "skipped")], &[]), &spec).unwrap();
        assert_eq!(
            kinds(&r),
            vec![("bench".to_string(), FindingKind::NotUnderstood)]
        );
    }

    // ---- the silent-narrowing signature --------------------------------------

    /// A renamed filter key: the `if:` reads an output change detection never
    /// emitted, so it resolves to empty and every such job skips. The skip is
    /// consistent with the expression, so it is not a violation, but it is
    /// named rather than passed silently.
    #[test]
    fn unemitted_boolean_filter_output_is_named() {
        let st = [
            ("detect-changes", "success"),
            ("docs-lint", "success"),
            ("lint", "success"),
            ("miri", "skipped"),
        ];
        let out = [("tooling", "true")]; // `rust-src` missing
        let r = run(WORKFLOW, &needs(&st, &out), "pull_request");
        assert!(r.findings.is_empty(), "{:?}", kinds(&r));
        assert!(
            r.notes
                .iter()
                .any(|n| n.contains("rust-src") && n.contains("miri")),
            "{:?}",
            r.notes
        );
        // Positive control: when it is emitted there is no such note.
        let r2 = run(WORKFLOW, &needs(&st, OK_OUT), "pull_request");
        assert!(r2.notes.is_empty(), "{:?}", r2.notes);
    }

    // ---- expression evaluator -------------------------------------------------

    #[test]
    fn expression_semantics_match_github() {
        let gh = github("pull_request");
        let eval = |src: &str| -> (bool, Vec<String>) { eval_standalone(src, &gh) };
        assert_eq!(eval("github.event_name == 'PULL_REQUEST'"), (true, vec![]));
        assert_eq!(eval("!(github.event_name == 'push')"), (true, vec![]));
        assert_eq!(eval("github.event_name == 'push' && true"), (false, vec![]));
        assert_eq!(eval("contains('|a|b|', '|B|')"), (true, vec![]));
        assert_eq!(eval("startsWith('refs/heads/x', 'refs/')"), (true, vec![]));
        assert_eq!(eval("'true' == true"), (false, vec![]));
        assert_eq!(eval("'1' == 1"), (true, vec![]));
        assert_eq!(eval("'' == null"), (true, vec![]));
        assert_eq!(eval("'it''s' == 'it''s'"), (true, vec![]));
        assert_eq!(eval("false || 'x'"), (true, vec![]));
        assert!(!eval("vars.X == 'y'").1.is_empty());
        assert!(!eval("github.event_name < 'z'").1.is_empty());
        assert!(!eval("github.event_name == \"push\"").1.is_empty());
    }

    #[test]
    fn the_workflow_is_detected_when_left_at_the_default() {
        let exists = |files: &'static [&'static str]| move |p: &str| files.contains(&p);
        // Explicit configuration wins.
        assert_eq!(
            resolve_workflow("ci/pipeline.yml", None, exists(&[])).unwrap(),
            "ci/pipeline.yml"
        );
        // The running workflow, as the runner names it.
        assert_eq!(
            resolve_workflow(
                DEFAULT_WORKFLOW,
                Some("o/r/.github/workflows/checks.yml@refs/pull/7/merge"),
                exists(&[".github/workflows/checks.yml", ".github/workflows/ci.yml"])
            )
            .unwrap(),
            ".github/workflows/checks.yml"
        );
        // Gitea and Forgejo layouts.
        assert_eq!(
            resolve_workflow(
                DEFAULT_WORKFLOW,
                None,
                exists(&[".forgejo/workflows/ci.yml"])
            )
            .unwrap(),
            ".forgejo/workflows/ci.yml"
        );
        assert_eq!(
            resolve_workflow(DEFAULT_WORKFLOW, None, exists(&[".gitea/workflows/ci.yml"])).unwrap(),
            ".gitea/workflows/ci.yml"
        );
        // Nothing to check is an error that says what was tried.
        let err = resolve_workflow(DEFAULT_WORKFLOW, None, exists(&[])).unwrap_err();
        assert!(
            err.to_string().contains(".forgejo/workflows/ci.yml"),
            "{err}"
        );
    }
}
