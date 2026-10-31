//! Pull request checklist reconciliation sentinel (`pr-checklist`).
//!
//! Reconciles ticked checklist items in PR descriptions against actual diff contents
//! to catch vacuous checkoffs (e.g. claiming tests/docs were added when 0 were modified).
//!
//! A test claim is backed by a changed test file, or by a test function the
//! change adds or extends in any file the language packs analyse: most Rust
//! unit tests live in a `mod tests` inside the source file they cover.
//!
//! The same test claim written as a plain bullet or sentence ("Added regression
//! tests for X") is read too, with a closed vocabulary, and reported at
//! `warning` at most: it is a heuristic over prose (#835).

use crate::ast::{default_registry, TestFn};
use crate::gitctx::ChangeKind;
use crate::guards::{Context, GateOutcome};
use crate::tokens::ALLOW_PR_CHECKLIST;
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

pub const GATE: &str = "pr-checklist";

pub fn evaluate_pr_checklist(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.pr_checklist;
    let mut out = GateOutcome::new(GATE);

    let declaration_text = ctx.declaration_body.text.as_deref();
    let Some(pr_body) = ctx.pr_body.as_deref().or(declaration_text) else {
        out.notes
            .push("no PR body supplied; PR checklist reconciliation skipped".to_string());
        return Ok(out);
    };
    // The declared-surface contract reads the body its directive source allows; when that
    // is the body the checkbox rule reads, the section's entries are the contract's alone.
    let declaration = declaration_text.and_then(read_declaration);
    let contract_lines: Vec<usize> = match &declaration {
        Some(d) if Some(pr_body) == declaration_text => d.entry_lines.clone(),
        _ => Vec::new(),
    };
    let checkbox_body: String = pr_body
        .lines()
        .enumerate()
        .map(|(i, l)| {
            if contract_lines.contains(&(i + 1)) {
                ""
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let changed = ctx.git.changed_files()?;
    let changed_paths: Vec<&str> = changed.iter().map(|f| f.path.as_str()).collect();

    let mut has_tests = changed_paths
        .iter()
        .any(|p| p.contains("test") || p.contains("spec") || p.starts_with("tests/"));
    if !has_tests {
        let added = count_added_test_functions(ctx, &changed, &mut out.notes)?;
        if added > 0 {
            out.notes.push(format!(
                "test claim backed by {added} test function(s) added or extended in changed source files"
            ));
            has_tests = true;
        }
    }
    let has_docs = changed_paths
        .iter()
        .any(|p| p.ends_with(".md") || p.starts_with("docs/"));
    let has_benches = changed_paths
        .iter()
        .any(|p| p.contains("bench") || p.starts_with("benches/"));

    let box_re = Regex::new(r"(?i)^[ \t]*-[ \t]*\[[xX]\][ \t]*(.*)$").expect("static regex");
    let checked_count = checkbox_body.lines().filter(|l| box_re.is_match(l)).count();
    out.examined = checked_count;

    let claims = find_unsupported_claims(&checkbox_body, has_tests, has_docs, has_benches);
    for claim in claims {
        let (kind, desc) = match claim.subject {
            "test" => (
                &crate::findings::CHECKLIST_CLAIMS_TESTS,
                "PR checklist claims tests added or extended, but diff adds or extends no test file or test function",
            ),
            "docs" => (
                &crate::findings::CHECKLIST_CLAIMS_DOCS,
                "PR checklist claims documentation updated, but diff contains zero documentation files",
            ),
            "bench" => (
                &crate::findings::CHECKLIST_CLAIMS_BENCHMARKS,
                "PR checklist claims benchmarks updated, but diff contains zero benchmark files",
            ),
            _ => (
                &crate::findings::CHECKLIST_CLAIM_UNSUPPORTED,
                "PR checklist claims unsupported change",
            ),
        };
        if let Some(ov) = ctx.find_override(GATE, kind, ALLOW_PR_CHECKLIST, claim.subject) {
            out.overrides.push(ov.clone());
            out.notes.push(format!(
                "override applied: `{}: {}` for {} checklist claim ({})",
                ov.directive, ov.reason, claim.subject, ov.source
            ));
        } else {
            out.add_violation(
                ctx.overridable(settings.severity),
                kind,
                "PR body",
                claim.line,
                desc,
                format!(
                    "checked item: `{}`; use `discipline:allow(pr-checklist): <reason>` to waive",
                    claim.item_text
                ),
            );
        }
    }

    let prose = find_unsupported_prose_test_claims(pr_body, has_tests);
    for claim in &prose.claims {
        let kind = &crate::findings::PROSE_CLAIMS_TESTS;
        if let Some(ov) = ctx.find_override(GATE, kind, ALLOW_PR_CHECKLIST, "test") {
            out.overrides.push(ov.clone());
            out.notes.push(format!(
                "override applied: `{}: {}` for test claim in prose ({})",
                ov.directive, ov.reason, ov.source
            ));
            continue;
        }
        out.add_violation(
            ctx.overridable(settings.severity).capped_at_warning(),
            kind,
            "PR body",
            claim.line,
            format!(
                "PR body says tests were added or changed (`{}`), but the diff adds or extends no test file or test function",
                claim.phrase
            ),
            "add the tests the description names or reword the claim; use `allow-pr-checklist: test <reason>` to waive",
        );
    }

    out.examined = checked_count + prose.units_read;

    let evidence = DiffEvidence {
        has_tests,
        has_docs,
        has_benches,
        has_directives: !ctx.directives.is_empty(),
    };
    reconcile_declared_surfaces(ctx, declaration, &changed, evidence, &mut out)?;
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// Declared surfaces (#942, #943)
// ---------------------------------------------------------------------------------------

/// The heading of the declaration section, compared case-insensitively.
pub const SECTION_HEADING: &str = "Touched surfaces";

/// One entry of the `Touched surfaces` section's fixed enumeration (#942).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Surface {
    Workflows,
    BuildHooks,
    Manifests,
    Config,
    Directives,
    Docs,
    Tests,
    Benches,
}

impl Surface {
    /// Every entry, in the order the template lists them.
    pub const ALL: [Surface; 8] = [
        Surface::Workflows,
        Surface::BuildHooks,
        Surface::Manifests,
        Surface::Config,
        Surface::Directives,
        Surface::Docs,
        Surface::Tests,
        Surface::Benches,
    ];

    /// The entry as the template writes it (backticks aside).
    pub fn label(self) -> &'static str {
        match self {
            Surface::Workflows => "workflows",
            Surface::BuildHooks => "build hooks",
            Surface::Manifests => "manifests/lockfiles",
            Surface::Config => "discipline.toml",
            Surface::Directives => "directives",
            Surface::Docs => "docs",
            Surface::Tests => "tests",
            Surface::Benches => "benches",
        }
    }

    /// What an `allow-pr-checklist:` directive names to lift a finding about this entry.
    /// `test`, `docs` and `bench` are the checkbox rule's subjects.
    pub fn subject(self) -> &'static str {
        match self {
            Surface::Workflows => "workflows",
            Surface::BuildHooks => "build-hooks",
            Surface::Manifests => "manifests",
            Surface::Config => "discipline.toml",
            Surface::Directives => "directives",
            Surface::Docs => "docs",
            Surface::Tests => "test",
            Surface::Benches => "bench",
        }
    }

    /// The gate that judges a change to a file of this boundary class; `None` for an
    /// entry the gap direction does not apply to.
    pub fn boundary_gate(self) -> Option<&'static str> {
        match self {
            Surface::Workflows => Some("ci-integrity"),
            Surface::BuildHooks => Some("build-hooks"),
            Surface::Manifests => Some("dependency-delta"),
            Surface::Config => Some("config-integrity"),
            _ => None,
        }
    }

    /// The entry an item's text names: the label, case and backticks aside, alone or
    /// followed by an aside (`docs (README.md)`, `tests: the parser`). The aside is not
    /// read: a path in it is not reconciled.
    pub fn parse(item: &str) -> Option<Surface> {
        let text = item.replace(['`', '*'], "").trim().to_ascii_lowercase();
        let text = text.replace(" / ", "/");
        Surface::ALL.into_iter().find(|s| {
            text.strip_prefix(s.label()).is_some_and(|rest| {
                rest.is_empty()
                    || rest.starts_with([' ', '(', ':', ',', ';', '.'])
                    || rest.starts_with('\u{2014}')
            })
        })
    }
}

/// What the `Touched surfaces` section of a body declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Declaration {
    /// 1-based line of the section heading.
    pub heading_line: usize,
    /// The ticked entries, with their lines.
    pub declared: Vec<(Surface, usize)>,
    /// Every checkbox line of the section, ticked or not: the contract's, not the
    /// checkbox rule's.
    pub entry_lines: Vec<usize>,
    /// Checkbox items outside the enumeration, with their lines; not read.
    pub unknown: Vec<(String, usize)>,
}

impl Declaration {
    pub fn declares(&self, s: Surface) -> bool {
        self.declared.iter().any(|(d, _)| *d == s)
    }
}

static HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ {0,3}(#{1,6})(?:[ \t]+(.*?))?[ \t#]*$").expect("static regex"));
static ENTRY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[ \t]*(?:[-*+]|\d+[.)])[ \t]+\[([ xX])\][ \t]*(.*)$").expect("static regex")
});
static FENCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ {0,3}(`{3,}|~{3,})").expect("static regex"));

/// The `Touched surfaces` section of `body`, read as a closed vocabulary: the first
/// heading of any level whose text is [`SECTION_HEADING`], up to the next heading of the
/// same or a higher level. A heading or an entry inside a fenced code block, an HTML
/// comment or a `>` quote is not read (#896: an example is not a declaration). `None`
/// when the body has no such section.
pub fn read_declaration(body: &str) -> Option<Declaration> {
    let mut fence: Option<String> = None;
    let mut in_comment = false;
    let mut section: Option<(usize, Declaration)> = None;
    for (idx, raw) in body.lines().enumerate() {
        let line_no = idx + 1;
        // HTML comments, which may span lines: the text outside them is kept.
        let mut line = String::new();
        let mut rest = raw;
        loop {
            if in_comment {
                match rest.find("-->") {
                    Some(end) => {
                        rest = &rest[end + 3..];
                        in_comment = false;
                    }
                    None => break,
                }
            } else {
                match rest.find("<!--") {
                    Some(start) => {
                        line.push_str(&rest[..start]);
                        rest = &rest[start + 4..];
                        in_comment = true;
                    }
                    None => {
                        line.push_str(rest);
                        break;
                    }
                }
            }
        }
        if let Some(open) = &fence {
            let closes = FENCE_RE.captures(&line).is_some_and(|c| {
                let m = &c[1];
                m.starts_with(&open[..1]) && m.len() >= open.len()
            }) && line
                .trim_start()
                .trim_start_matches(['`', '~'])
                .trim()
                .is_empty();
            if closes {
                fence = None;
            }
            continue;
        }
        if let Some(c) = FENCE_RE.captures(&line) {
            fence = Some(c[1].to_string());
            continue;
        }
        if line.trim_start().starts_with('>') {
            continue;
        }
        if let Some(h) = HEADING_RE.captures(&line) {
            let level = h[1].len();
            let text = h
                .get(2)
                .map_or("", |m| m.as_str())
                .replace(['*', '_', '`'], "");
            match &section {
                Some((open_level, _)) if level <= *open_level => break,
                Some(_) => continue,
                None if text.trim().eq_ignore_ascii_case(SECTION_HEADING) => {
                    section = Some((
                        level,
                        Declaration {
                            heading_line: line_no,
                            ..Default::default()
                        },
                    ));
                }
                None => {}
            }
            continue;
        }
        let Some((_, decl)) = section.as_mut() else {
            continue;
        };
        let Some(entry) = ENTRY_RE.captures(&line) else {
            continue;
        };
        decl.entry_lines.push(line_no);
        let ticked = !entry[1].trim().is_empty();
        match Surface::parse(&entry[2]) {
            Some(s) if ticked => decl.declared.push((s, line_no)),
            Some(_) => {}
            None => decl.unknown.push((entry[2].trim().to_string(), line_no)),
        }
    }
    section.map(|(_, d)| d)
}

/// The body the declared-surface contract reads, chosen through the directive sources
/// (`directives.sources`), and where it came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeclarationBody {
    pub text: Option<String>,
    /// Where `text` came from, or why there is none.
    pub source: String,
}

/// The body the contract reads, by the same sources directives come through: the pull
/// request's body (`pr-body`); on a push, the body of the one merged pull request the
/// pushed commits arrived through (`merged-pr-body`, read by the directive lookup and
/// passed in as `merged`); else the head commit's message (`commits`). A source the
/// policy leaves out is not read, and the contract then sees no declaration.
pub fn select_declaration_body(
    pr_body: Option<&str>,
    merged: &[crate::tokens::MergedBody],
    head_commit_body: Option<&str>,
    sources: &[String],
) -> DeclarationBody {
    let allowed = |s: &str| sources.iter().any(|x| x == s);
    let none = |why: &str| DeclarationBody {
        text: None,
        source: why.to_string(),
    };
    if let Some(body) = pr_body {
        return if allowed("pr-body") {
            DeclarationBody {
                text: Some(body.to_string()),
                source: "the pull request body".to_string(),
            }
        } else {
            none("the pull request body, which `directives.sources` does not list (`pr-body`)")
        };
    }
    if let [m] = merged {
        if allowed("merged-pr-body") {
            return DeclarationBody {
                text: Some(m.body.clone()),
                source: format!("the body of merged pull request #{}", m.number),
            };
        }
    }
    match head_commit_body {
        Some(body) if allowed("commits") => DeclarationBody {
            text: Some(body.to_string()),
            source: "the head commit message".to_string(),
        },
        Some(_) => {
            none("the head commit message, which `directives.sources` does not list (`commits`)")
        }
        None => none("no pull request body or commit message"),
    }
}

/// What the diff holds for the entries that are not file classes, and for the checkbox
/// rule's own subjects (the same evidence, so a ticked `tests` means what a ticked
/// `Tests added` means).
#[derive(Debug, Clone, Copy, Default)]
pub struct DiffEvidence {
    pub has_tests: bool,
    pub has_docs: bool,
    pub has_benches: bool,
    /// The run read at least one directive (PR body, commits, merged pull request).
    pub has_directives: bool,
}

/// Which boundary and file classes a path falls in.
pub struct SurfaceClassifier {
    workflows: crate::guards::PathFilter,
    manifests: crate::guards::PathFilter,
    config_paths: Vec<String>,
}

impl SurfaceClassifier {
    /// The classes as the gates that own them read them: `ci-integrity`'s `workflows`,
    /// `dependency-delta`'s `manifests` and lockfile names, `build-hooks`' build scripts
    /// and package-manager configuration, and the run's configuration file.
    pub fn new(config: &crate::config::DisciplineConfig, config_path: &str) -> Result<Self> {
        Ok(Self {
            workflows: crate::guards::PathFilter::new(&config.gates.ci_integrity.workflows)?,
            manifests: crate::guards::PathFilter::new(&config.gates.dependency_delta.manifests)?,
            config_paths: [config_path, "discipline.toml"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        })
    }

    fn is_lockfile(path: &str) -> bool {
        let name = path.rsplit('/').next().unwrap_or(path);
        crate::guards::dependency::LOCKFILE_NAMES.contains(&name)
    }

    /// The file classes of `path` (workflows, build hooks, manifests/lockfiles,
    /// `discipline.toml`).
    pub fn classes(&self, path: &str) -> Vec<Surface> {
        let mut out = Vec::new();
        if self.workflows.matches(path) {
            out.push(Surface::Workflows);
        }
        if crate::guards::build_hooks::is_build_hook_path(path) {
            out.push(Surface::BuildHooks);
        }
        if self.manifests.matches(path) || Self::is_lockfile(path) {
            out.push(Surface::Manifests);
        }
        if self.config_paths.iter().any(|c| c == path) {
            out.push(Surface::Config);
        }
        out
    }

    /// The authority-boundary class of `path` the gap direction holds to a declaration:
    /// a workflow, a build hook, a lockfile (a manifest alone is not one) or the
    /// configuration.
    pub fn boundary_classes(&self, path: &str) -> Vec<Surface> {
        self.classes(path)
            .into_iter()
            .filter(|s| *s != Surface::Manifests || Self::is_lockfile(path))
            .collect()
    }
}

/// One boundary class the change touches and the section does not declare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeclaredBoundary {
    pub surface: Surface,
    /// The changed files of that class; a rename is one entry, `old -> new`.
    pub files: Vec<String>,
}

/// The two directions of the contract over plain inputs: the declared entries the diff
/// does not resolve (false claims), and the boundary classes the diff touches that the
/// declaration leaves out (gaps). `changed` is each changed file as (head path, base
/// path); `generated(path)` says a lockfile is generated output no one declares.
pub fn reconcile(
    declaration: &Declaration,
    changed: &[(String, String)],
    classifier: &SurfaceClassifier,
    evidence: DiffEvidence,
    generated: &dyn Fn(&str) -> bool,
) -> (Vec<(Surface, usize)>, Vec<UndeclaredBoundary>) {
    let touched = |s: Surface| -> bool {
        match s {
            Surface::Directives => evidence.has_directives,
            Surface::Docs => evidence.has_docs,
            Surface::Tests => evidence.has_tests,
            Surface::Benches => evidence.has_benches,
            _ => changed.iter().any(|(p, o)| {
                classifier.classes(p).contains(&s) || classifier.classes(o).contains(&s)
            }),
        }
    };
    let mut false_claims = Vec::new();
    for (s, line) in &declaration.declared {
        if !touched(*s) && !false_claims.iter().any(|(d, _)| d == s) {
            false_claims.push((*s, *line));
        }
    }
    let mut gaps: Vec<UndeclaredBoundary> = Vec::new();
    for (path, old) in changed {
        let mut classes = classifier.boundary_classes(path);
        for c in classifier.boundary_classes(old) {
            if !classes.contains(&c) {
                classes.push(c);
            }
        }
        for s in classes {
            if declaration.declares(s) || (s == Surface::Manifests && generated(path)) {
                continue;
            }
            let shown = if old != path {
                format!("{old} -> {path}")
            } else {
                path.clone()
            };
            match gaps.iter_mut().find(|g| g.surface == s) {
                Some(g) => g.files.push(shown),
                None => gaps.push(UndeclaredBoundary {
                    surface: s,
                    files: vec![shown],
                }),
            }
        }
    }
    gaps.sort_by_key(|g| g.surface);
    (false_claims, gaps)
}

/// The pull request's author when `exempt_authors` lists it, as the event payload names
/// it; a run with no payload author (a local run, a GitLab pipeline) has none.
fn exempt_author(ctx: &Context, exempt: &[String]) -> Option<String> {
    let author = ctx
        .forge
        .as_ref()
        .and_then(|f| f.pull.as_ref())
        .and_then(|p| p.author.clone())
        .filter(|a| !a.is_empty())?;
    exempt
        .iter()
        .any(|e| e.eq_ignore_ascii_case(&author))
        .then_some(author)
}

/// The declared-surface contract (`docs/GATES.md`, `pr-checklist`): applies when the
/// body the directive sources allow has a `Touched surfaces` section, or always under
/// `require_declared_surfaces` (no section then declares nothing).
fn reconcile_declared_surfaces(
    ctx: &Context,
    declaration: Option<Declaration>,
    changed: &[crate::gitctx::ChangedFile],
    evidence: DiffEvidence,
    out: &mut GateOutcome,
) -> Result<()> {
    let settings = &ctx.config.gates.pr_checklist;
    if let Some(bad) = settings
        .exempt_authors
        .iter()
        .find(|a| a.trim().is_empty() || a.chars().any(char::is_whitespace))
    {
        return Err(crate::could_not_check::tag(
            crate::could_not_check::Reason::Configuration,
            anyhow::anyhow!("`gates.pr-checklist.exempt_authors` entry {bad:?} is not a login"),
        ));
    }
    let source = &ctx.declaration_body.source;
    let declaration = match declaration {
        Some(d) => {
            out.notes.push(format!(
                "`{SECTION_HEADING}` section read from {source}: {} entr{} declared",
                d.declared.len(),
                if d.declared.len() == 1 { "y" } else { "ies" }
            ));
            d
        }
        None if settings.require_declared_surfaces => {
            out.notes.push(format!(
                "no `{SECTION_HEADING}` section in {source}; with `require_declared_surfaces` it declares no surface"
            ));
            Declaration::default()
        }
        None => {
            out.notes.push(format!(
                "no `{SECTION_HEADING}` section in {source}; the declared-surface contract is not applied"
            ));
            return Ok(());
        }
    };
    out.examined += declaration.entry_lines.len();
    for (item, line) in &declaration.unknown {
        out.notes.push(format!(
            "`{SECTION_HEADING}` line {line}: `{item}` is not in the fixed list; not read"
        ));
    }
    let exempt = crate::guards::exempt_filter(settings)?;
    let pairs: Vec<(String, String)> = changed
        .iter()
        .filter(|f| !exempt.matches(&f.path))
        .map(|f| (f.path.clone(), f.old_path.clone()))
        .collect();
    let classifier = SurfaceClassifier::new(ctx.config, ctx.config_path)?;
    // A lockfile the base marks `linguist-generated` is generated output: the base's own
    // root `.gitattributes`, never the change's, decides.
    let mut attributes = crate::ast::gitattributes::GitAttributes::default();
    if let Some(text) = ctx.git.base_content(".gitattributes")? {
        attributes.add_file(".gitattributes", &text);
    }
    let generated = |path: &str| {
        attributes
            .set_on(path)
            .is_some_and(|(attr, _)| attr == "linguist-generated")
    };
    let (false_claims, gaps) = reconcile(&declaration, &pairs, &classifier, evidence, &generated);
    for (path, _) in &pairs {
        if classifier
            .boundary_classes(path)
            .contains(&Surface::Manifests)
            && !declaration.declares(Surface::Manifests)
            && generated(path)
        {
            out.notes.push(format!(
                "`{path}` is `linguist-generated` in the base `.gitattributes`: no declaration expected"
            ));
        }
    }

    for (surface, line) in false_claims {
        let kind = match surface {
            Surface::Tests => &crate::findings::CHECKLIST_CLAIMS_TESTS,
            Surface::Docs => &crate::findings::CHECKLIST_CLAIMS_DOCS,
            Surface::Benches => &crate::findings::CHECKLIST_CLAIMS_BENCHMARKS,
            _ => &crate::findings::CHECKLIST_CLAIM_UNSUPPORTED,
        };
        let what = match surface {
            Surface::Directives => "the change carries no directive".to_string(),
            Surface::Tests => "the diff adds or extends no test file or test function".to_string(),
            Surface::Docs => "the diff contains no documentation file".to_string(),
            Surface::Benches => "the diff contains no benchmark file".to_string(),
            s => format!("the diff changes no {} file", s.label()),
        };
        if let Some(ov) = ctx.find_override(GATE, kind, ALLOW_PR_CHECKLIST, surface.subject()) {
            out.notes.push(format!(
                "override applied: `{}: {}` for declared surface `{}` ({})",
                ov.directive,
                ov.reason,
                surface.label(),
                ov.source
            ));
            out.overrides.push(ov);
            continue;
        }
        out.add_violation(
            ctx.overridable(settings.severity),
            kind,
            "PR body",
            line,
            format!(
                "`{SECTION_HEADING}` declares `{}`, but {what}",
                surface.label()
            ),
            format!(
                "The entry is ticked for a surface this change does not modify; `allow-pr-checklist: {} <reason>` records an exception.",
                surface.subject()
            ),
        );
    }

    let bot = (!gaps.is_empty())
        .then(|| exempt_author(ctx, &settings.exempt_authors))
        .flatten();
    for gap in gaps {
        let surface = gap.surface;
        let gate = surface.boundary_gate().unwrap_or(GATE);
        let judged = ctx.config.gates.settings(gate).is_some_and(|s| s.enabled())
            && !ctx.cli_disabled_gates.iter().any(|d| d == gate);
        let files = gap
            .files
            .iter()
            .map(|f| format!("`{f}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let judged_by = if judged {
            format!("`{gate}` judges the change to the file itself")
        } else {
            format!("`{gate}` is not enabled, so no finding judges the file itself")
        };
        if let Some(author) = &bot {
            out.notes.push(format!(
                "{files} changed ({} surface) without a declaration; pull request opened by `{author}`, listed in `exempt_authors`, so the declaration is optional",
                surface.label()
            ));
            continue;
        }
        let kind = &crate::findings::UNDECLARED_BOUNDARY_DELTA;
        if let Some(ov) = ctx.find_override(GATE, kind, ALLOW_PR_CHECKLIST, surface.subject()) {
            out.notes.push(format!(
                "override applied: `{}: {}` for undeclared surface `{}` ({})",
                ov.directive,
                ov.reason,
                surface.label(),
                ov.source
            ));
            out.overrides.push(ov);
            continue;
        }
        out.lift_or_push(
            None,
            ctx.overridable(settings.severity),
            kind,
            (
                Some("PR body"),
                (declaration.heading_line > 0).then_some(declaration.heading_line),
            ),
            format!(
                "{files} changed (the `{}` surface), and the `{SECTION_HEADING}` section does not declare it; {judged_by}",
                surface.label()
            ),
            &format!(
                "A ticked `{}` entry declares the change; `allow-pr-checklist: {} <reason>` records an exception.",
                surface.label(),
                surface.subject()
            ),
        );
        out.anchor_last(format!("surface:{}", surface.subject()));
    }
    Ok(())
}

/// Test functions the change adds or extends across every changed file a
/// language pack analyses. A file the pack cannot extract adds nothing and is
/// named in `notes`, so an unreadable file never backs a claim.
fn count_added_test_functions(
    ctx: &Context,
    changed: &[crate::gitctx::ChangedFile],
    notes: &mut Vec<String>,
) -> Result<usize> {
    let registry = default_registry();
    let vocab = crate::guards::agent_diff::assert_vocabulary(ctx.config);
    let mut total = 0;
    for file in changed.iter().filter(|f| f.kind != ChangeKind::Deleted) {
        let Some(pack) = registry.find_pack(&file.path) else {
            continue;
        };
        let Some(head_src) = ctx.git.head_content(&file.path)? else {
            continue;
        };
        let head = match pack.extract(&file.path, &head_src, &vocab) {
            Ok(facts) => facts.tests,
            Err(e) => {
                notes.push(format!(
                    "{}: could not extract tests ({e}); not counted as test evidence",
                    file.path
                ));
                continue;
            }
        };
        let base = match ctx.git.base_content(&file.old_path)? {
            Some(src) => match registry.find_pack(&file.old_path) {
                Some(base_pack) => match base_pack.extract(&file.old_path, &src, &vocab) {
                    Ok(facts) => facts.tests,
                    Err(e) => {
                        notes.push(format!(
                            "{}: could not extract base tests ({e}); not counted as test evidence",
                            file.old_path
                        ));
                        continue;
                    }
                },
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        total += added_or_extended_tests(&base, &head);
    }
    Ok(total)
}

/// Tests the head side adds or extends relative to the base side of one file.
///
/// A head test with no base test of the same name is added; a same-named test
/// whose effective assertions grew is extended. Renames are netted out: a
/// test renamed without new assertions adds nothing.
pub fn added_or_extended_tests(base: &[TestFn], head: &[TestFn]) -> usize {
    let mut unmatched_base: Vec<&TestFn> = base.iter().collect();
    let mut unmatched_head = 0usize;
    let mut extended = 0usize;
    for h in head {
        match unmatched_base.iter().position(|b| b.name == h.name) {
            Some(i) => {
                let b = unmatched_base.swap_remove(i);
                if h.effective_asserts() > b.effective_asserts() {
                    extended += 1;
                }
            }
            None => unmatched_head += 1,
        }
    }
    unmatched_head.saturating_sub(unmatched_base.len()) + extended
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecklistClaim {
    pub line: usize,
    pub subject: &'static str,
    pub item_text: String,
}

pub fn find_unsupported_claims(
    pr_body: &str,
    has_tests: bool,
    has_docs: bool,
    has_benches: bool,
) -> Vec<ChecklistClaim> {
    let box_re = Regex::new(r"(?i)^[ \t]*-[ \t]*\[[xX]\][ \t]*(.*)$").expect("static regex");
    let mut claims = Vec::new();
    for (idx, line) in pr_body.lines().enumerate() {
        if let Some(caps) = box_re.captures(line) {
            let item_text = caps.get(1).map_or("", |m| m.as_str()).trim();
            let lower = item_text.to_ascii_lowercase();

            if (lower.contains("test") || lower.contains("tests"))
                && !lower.contains("no test")
                && !lower.contains("n/a")
                && !has_tests
            {
                claims.push(ChecklistClaim {
                    line: idx + 1,
                    subject: "test",
                    item_text: item_text.to_string(),
                });
            }

            if (lower.contains("doc") || lower.contains("docs") || lower.contains("documentation"))
                && !lower.contains("no doc")
                && !lower.contains("n/a")
                && !has_docs
            {
                claims.push(ChecklistClaim {
                    line: idx + 1,
                    subject: "docs",
                    item_text: item_text.to_string(),
                });
            }

            if (lower.contains("bench") || lower.contains("benchmark"))
                && !lower.contains("no bench")
                && !lower.contains("n/a")
                && !has_benches
            {
                claims.push(ChecklistClaim {
                    line: idx + 1,
                    subject: "bench",
                    item_text: item_text.to_string(),
                });
            }
        }
    }
    claims
}

/// A plain bullet or sentence of a PR body that says tests were added or changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProseTestClaim {
    /// 1-based line of the PR body the claim starts on.
    pub line: usize,
    /// The words that make it a claim, as written.
    pub phrase: String,
}

/// What [`find_prose_test_claims`] read: the claims, and how many paragraphs and list
/// items it looked at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProseTestClaims {
    pub claims: Vec<ProseTestClaim>,
    pub units_read: usize,
}

/// Verbs that say tests were added or changed, every tense (`docs/GATES.md`, `pr-checklist`).
const CLAIM_VERBS: &str = "add|adds|added|adding|write|writes|wrote|written|writing|extend|extends|extended|extending|update|updates|updated|updating|expand|expands|expanded|expanding";
/// Verbs that say tests were added only with `test coverage` as their object.
const COVERAGE_VERBS: &str =
    "improve|improves|improved|improving|increase|increases|increased|increasing";
/// The participles of [`CLAIM_VERBS`] in a passive claim (`tests were added`).
const PASSIVE_VERBS: &str = "added|written|extended|updated|expanded";
/// Words that may stand between a verb and the test noun (`added two new regression tests`).
const MODIFIERS: &str = r"a|an|the|new|more|extra|additional|some|several|missing|regression|unit|integration|e2e|end-to-end|property|property-based|snapshot|golden|fuzz|focused|targeted|dedicated|corresponding|matching|basic|existing|its|their|our|one|two|three|four|five|six|seven|eight|nine|ten|\d+";
/// The test nouns.
const NOUN: &str = r"test[ -]cases?|test coverage|tests?";

fn claim_re(template: &str) -> Regex {
    let pattern = template
        .replace("{VERB}", CLAIM_VERBS)
        .replace("{COVER}", COVERAGE_VERBS)
        .replace("{PASSIVE}", PASSIVE_VERBS)
        .replace("{MOD}", &format!(r"(?:(?:{MODIFIERS})\s+){{0,4}}"))
        .replace("{NOUN}", NOUN);
    Regex::new(&format!("(?i){pattern}")).expect("static claim regex")
}

/// `added regression tests`, `improves test coverage`.
static VERB_CLAIM_RE: LazyLock<Regex> = LazyLock::new(|| {
    claim_re(r"\b(?:(?:{VERB})\s+{MOD}(?:{NOUN})|(?:{COVER})\s+(?:the\s+)?test coverage)\b")
});
/// `added a flag and tests`.
static COORDINATED_CLAIM_RE: LazyLock<Regex> =
    LazyLock::new(|| claim_re(r"\b(?:{VERB})\b.{0,60}?\s(?:and|plus)\s+{MOD}(?:{NOUN})\b"));
/// `regression tests were added`, `tests added`.
static PASSIVE_CLAIM_RE: LazyLock<Regex> = LazyLock::new(|| {
    claim_re(
        r"\b{MOD}(?:{NOUN})\s+(?:(?:were|was|have\s+been|has\s+been|are|is|got)\s+)?(?:{PASSIVE})\b",
    )
});
/// `Tests for negative inputs` at the start of a bullet or sentence.
static NOMINAL_CLAIM_RE: LazyLock<Regex> =
    LazyLock::new(|| claim_re(r"^{MOD}(?:{NOUN})\s+(?:for|covering)\b"));
/// `with regression tests`, closing the clause or followed by what they cover, after the
/// change they come with: a clause that opens with `with` describes, it does not claim
/// (`(lib, integration) with two test cases`).
static WITH_CLAIM_RE: LazyLock<Regex> = LazyLock::new(|| {
    claim_re(r"\b(?:with|plus)\s+{MOD}(?:{NOUN})\s*(?:$|(?:for|covering|that|included|added)\b)")
});

/// Where one claim ends and the next begins inside a paragraph or list item.
static CLAUSE_SPLIT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[.;!?:]+(?:\s+|$)|\s[—–]\s|\s--?\s|[()|]").expect("static regex")
});
static BULLET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:[-*+•]|\d+[.)])\s+").expect("static regex"));
static CHECKBOX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:[-*+]|\d+[.)])\s*\[[ xX]\]").expect("static regex"));
/// Another change named as the one that added the tests: `tests added by #505`,
/// `#505 added tests`, `tests added in 1a2b3c4`.
static OTHER_CHANGE_AFTER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:by|in|from|since|via)\s+(?:pr\s+|pull request\s+|commit\s+)?(?:#\d+|[0-9a-f]{7,40}\b)")
        .expect("static regex")
});
static OTHER_CHANGE_BEFORE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:#\d+|\b[0-9a-f]{7,40})\s*$").expect("static regex"));

/// A word anywhere before the claim that negates it (`no tests added`, `did not add tests`).
const NEGATIONS: &[&str] = &[
    "no", "not", "never", "without", "cannot", "nor", "todo", "tbd", "fixme", "n/a", "skip",
    "skipped", "skipping",
];
/// The word just before the claim when the claim is deferred, asked for, or describes
/// something else (`to add tests`, `will add tests`, `a change that adds tests`).
const DEFERRALS: &[&str] = &[
    "to", "will", "would", "should", "could", "can", "may", "might", "must", "shall", "need",
    "needs", "please", "that", "which", "who", "when", "whenever", "if", "whether", "unless",
    "once", "until", "lets", "let's",
];
/// Words after the claim that say it is not done (`tests to follow`, `tests not needed`).
const AFTER_NEGATIONS: &[&str] = &[
    "not needed",
    "not required",
    "not necessary",
    "not applicable",
    "to follow",
    "follow-up",
    "follow up",
    "followup",
    "later",
    "pending",
    "todo",
    "tbd",
    "n/a",
    "unnecessary",
    "deferred",
    "out of scope",
];
/// Words after a test noun that make it an adjective (`added test fixtures`, `the tests
/// directory`) or a statement about tests that already run (`tests pass`).
const NOUN_FOLLOWERS: &[&str] = &[
    "directory",
    "dir",
    "dirs",
    "folder",
    "helper",
    "helpers",
    "util",
    "utils",
    "utility",
    "utilities",
    "harness",
    "infra",
    "infrastructure",
    "data",
    "fixture",
    "fixtures",
    "config",
    "configuration",
    "workflow",
    "workflows",
    "job",
    "jobs",
    "step",
    "steps",
    "target",
    "targets",
    "runner",
    "runners",
    "framework",
    "dependency",
    "dependencies",
    "command",
    "commands",
    "script",
    "scripts",
    "mode",
    "env",
    "environment",
    "server",
    "account",
    "user",
    "users",
    "plan",
    "matrix",
    "tooling",
    "output",
    "outputs",
    "result",
    "results",
    "log",
    "logs",
    "name",
    "names",
    "id",
    "ids",
    "flag",
    "flags",
    "feature",
    "gate",
    "gates",
    "crate",
    "crates",
    "workspace",
    "package",
    "pass",
    "passes",
    "passed",
    "passing",
    "run",
    "runs",
    "ran",
    "fail",
    "fails",
    "failed",
    "failing",
    "still",
    "already",
];
/// Verbs after a nominal `Tests for X` that make it a statement about tests that exist.
const NOMINAL_STATES: &[&str] = &[
    "pass",
    "passes",
    "passed",
    "passing",
    "exist",
    "exists",
    "existed",
    "live",
    "lives",
    "run",
    "runs",
    "ran",
    "fail",
    "fails",
    "failed",
    "failing",
    "remain",
    "remains",
    "already",
    "still",
    "is",
    "are",
    "was",
    "were",
    "cover",
    "covers",
    "covered",
    "unchanged",
];

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .replace('’', "'")
        .split(|c: char| !(c.is_alphanumeric() || matches!(c, '\'' | '/' | '-')))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

fn is_negation(word: &str) -> bool {
    NEGATIONS.contains(&word) || word.ends_with("n't")
}

fn is_deferral(word: &str) -> bool {
    DEFERRALS.contains(&word) || word.ends_with("'ll")
}

/// Whether the text before a claim in its clause negates or defers it.
fn negated_before(prefix: &str) -> bool {
    let w = words(prefix);
    w.iter().any(|x| is_negation(x)) || w.last().is_some_and(|x| is_deferral(x))
}

/// Whether the claim is about another change: one named right before or right after it.
fn names_other_change(prefix: &str, suffix: &str) -> bool {
    OTHER_CHANGE_BEFORE_RE.is_match(prefix) || OTHER_CHANGE_AFTER_RE.is_match(suffix)
}

/// Whether the text after a claim in its clause says it is not done.
fn negated_after(suffix: &str) -> bool {
    let lower = format!(" {} ", words(suffix).join(" "));
    AFTER_NEGATIONS
        .iter()
        .any(|n| lower.contains(&format!(" {n} ")))
}

/// Whether the test noun at the end of a match is used as an adjective or in a statement
/// about tests that already exist: followed by a hyphen, an underscore or a listed word.
fn noun_continues(suffix: &str) -> bool {
    if suffix.starts_with(['-', '_']) {
        return true;
    }
    words(suffix)
        .first()
        .is_some_and(|w| NOUN_FOLLOWERS.contains(&w.as_str()))
}

/// The value of a field that says no test was added (`Tests added: none`).
const FIELD_NONE: &[&str] = &["none", "nothing", "no", "n/a", "na", "nil", "0"];

/// Whether the clause is a field label (it ends at a colon) whose value, `after`, says
/// nothing was added.
fn field_value_none(separator: &str, after: &str) -> bool {
    separator.contains(':')
        && words(after)
            .first()
            .is_some_and(|w| FIELD_NONE.contains(&w.as_str()))
}

/// The claim one clause makes, if any: the matched words. `beside_paren`: the clause sits
/// inside parentheses or a parenthesis follows it, where a `with` claim is an aside or a
/// described item (`(gate or delete with tests)`, `each with the test (path, name)`).
fn clause_claim(clause: &str, beside_paren: bool) -> Option<String> {
    let checks: [(&Regex, bool); 4] = [
        (&VERB_CLAIM_RE, true),
        (&COORDINATED_CLAIM_RE, true),
        (&WITH_CLAIM_RE, false),
        (&PASSIVE_CLAIM_RE, false),
    ];
    for (re, check_noun) in checks {
        for m in re.find_iter(clause) {
            let (prefix, suffix) = (&clause[..m.start()], &clause[m.end()..]);
            if negated_before(prefix) || negated_after(suffix) || names_other_change(prefix, suffix)
            {
                continue;
            }
            if check_noun && noun_continues(suffix) {
                continue;
            }
            if std::ptr::eq(re, &*WITH_CLAIM_RE) && (beside_paren || prefix.trim().is_empty()) {
                continue;
            }
            return Some(m.as_str().split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }
    let m = NOMINAL_CLAIM_RE.find(clause)?;
    let suffix = &clause[m.end()..];
    let suffix_words = words(suffix);
    let passive = PASSIVE_VERBS
        .split('|')
        .any(|v| suffix_words.iter().any(|w| w == v));
    let state = suffix_words
        .iter()
        .any(|w| NOMINAL_STATES.contains(&w.as_str()));
    (!negated_after(suffix) && (passive || !state))
        .then(|| m.as_str().split_whitespace().collect::<Vec<_>>().join(" "))
}

/// HTML blocks whose content is not the description's own prose: a comment, and a
/// blockquote, the HTML form of a `>` quote (a dependency bot quotes upstream release
/// notes in one). Blockquotes nest; comments do not.
const MASKED_BLOCKS: [(&str, &str, bool); 2] = [
    ("<!--", "-->", false),
    ("<blockquote", "</blockquote>", true),
];

/// `body` with HTML comments and blockquotes, directive lines and checkbox items blanked,
/// line for line: none of them is a claim the description makes in prose. A ticked
/// checkbox is the checkbox rule's, an unticked one is a to-do.
fn mask_non_prose(body: &str) -> String {
    let directive_lines: Vec<usize> =
        crate::tokens::directive_lines(body, crate::tokens::OverrideSource::PrBody)
            .into_iter()
            .map(|(i, _)| i)
            .collect();
    // The open block and how deeply it is nested.
    let mut open: Option<(usize, usize)> = None;
    let mut out = Vec::new();
    for (idx, line) in body.lines().enumerate() {
        // ASCII lowercasing keeps byte offsets, so positions found in `lower` index `line`.
        let lower = line.to_ascii_lowercase();
        let mut kept = String::new();
        let mut pos = 0;
        loop {
            let rest = &lower[pos..];
            match open {
                Some((block, depth)) => {
                    let (start, end, nests) = MASKED_BLOCKS[block];
                    let deeper = rest.find(start).filter(|_| nests);
                    match (deeper, rest.find(end)) {
                        (Some(s), e) if e.is_none_or(|e| s < e) => {
                            open = Some((block, depth + 1));
                            pos += s + start.len();
                        }
                        (_, Some(e)) => {
                            open = (depth > 1).then_some((block, depth - 1));
                            pos += e + end.len();
                        }
                        _ => break,
                    }
                }
                None => {
                    let first = MASKED_BLOCKS
                        .iter()
                        .enumerate()
                        .filter_map(|(i, (start, _, _))| rest.find(start).map(|at| (at, i)))
                        .min();
                    match first {
                        Some((at, block)) => {
                            kept.push_str(&line[pos..pos + at]);
                            open = Some((block, 1));
                            pos += at + MASKED_BLOCKS[block].0.len();
                        }
                        None => {
                            kept.push_str(&line[pos..]);
                            break;
                        }
                    }
                }
            }
        }
        if directive_lines.contains(&idx) || CHECKBOX_RE.is_match(&kept) {
            kept.clear();
        }
        out.push(kept);
    }
    out.join("\n")
}

/// Plain bullets and sentences of a PR body that say tests were added or changed
/// (`docs/GATES.md`, `pr-checklist`): a claim verb before a test noun (`added regression
/// tests`, `Add tests for`), the passive (`tests were added`), a bullet or sentence that
/// opens with `Tests for` or `Test coverage for`, and `with tests`. Code spans, fenced
/// and indented code, quoted lines (`>`, `<blockquote>`), double-quoted text, HTML
/// comments, directive lines, checkbox items, headings, questions and `TODO:` lines are
/// not read. A claim negated (`no tests`, `without tests`, `tests not needed`, a `Tests
/// added: none` field) or deferred (`will add tests`, `tests to follow`) is not one, nor
/// is a `with tests` next to a parenthesis. One claim per paragraph or list item.
pub fn find_prose_test_claims(pr_body: &str) -> ProseTestClaims {
    use crate::guards::claim_labels::{
        extract_prose_units, is_question_or_todo, strip_non_claim_spans,
    };
    let masked = mask_non_prose(pr_body);
    let mut result = ProseTestClaims::default();
    for unit in extract_prose_units(&masked) {
        result.units_read += 1;
        // The unit as one text, wrapped lines joined, with where each line starts.
        let mut text = String::new();
        let mut starts: Vec<(usize, usize)> = Vec::new();
        for (ln, line) in &unit.lines {
            if is_question_or_todo(line) {
                continue;
            }
            let cleaned = strip_non_claim_spans(line)
                .replace("**", "")
                .replace("__", "");
            let cleaned = BULLET_RE.replace(&cleaned, "");
            if !text.is_empty() {
                text.push(' ');
            }
            starts.push((text.len(), *ln));
            text.push_str(&cleaned);
        }
        let mut offset = 0;
        let mut boundaries: Vec<(usize, usize)> = CLAUSE_SPLIT_RE
            .find_iter(&text)
            .map(|m| (m.start(), m.end()))
            .collect();
        boundaries.push((text.len(), text.len()));
        let mut prev_separator = "";
        for (end, next) in boundaries {
            let raw = &text[offset..end];
            let separator = &text[end..next];
            let lead = raw.len() - raw.trim_start().len();
            let clause = raw.trim();
            let clause_start = offset + lead;
            let beside_paren = prev_separator.contains('(') || separator.contains('(');
            prev_separator = separator;
            offset = next;
            let Some(phrase) = clause_claim(clause, beside_paren) else {
                continue;
            };
            if field_value_none(separator, &text[next..]) {
                continue;
            }
            let first = phrase.split(' ').next().unwrap_or_default();
            let at = clause_start + clause.find(first).unwrap_or(0);
            let line = starts
                .iter()
                .rev()
                .find(|(s, _)| *s <= at)
                .map_or(unit.lines[0].0, |(_, ln)| *ln);
            result.claims.push(ProseTestClaim { line, phrase });
            break;
        }
    }
    result
}

/// [`find_prose_test_claims`] reconciled against the diff: no claim stands when the
/// change adds or extends a test (`has_tests`, the notion the checkbox rule uses).
pub fn find_unsupported_prose_test_claims(pr_body: &str, has_tests: bool) -> ProseTestClaims {
    let mut read = find_prose_test_claims(pr_body);
    if has_tests {
        read.claims.clear();
    }
    read
}

#[cfg(test)]
mod tests {
    use super::added_or_extended_tests;
    use crate::ast::TestFn;
    use crate::config::{PrChecklistGate, Severity};

    fn t(name: &str, asserts: usize) -> TestFn {
        TestFn {
            name: name.to_string(),
            total_asserts: asserts,
            ..Default::default()
        }
    }

    #[test]
    fn added_or_extended_tests_counts_new_and_grown_tests_only() {
        let base = [t("tests::doubles", 1)];
        // Positive: a new test beside an existing one.
        assert_eq!(
            added_or_extended_tests(&base, &[t("tests::doubles", 1), t("tests::triples", 1)]),
            1
        );
        // Positive: an existing test gains an assertion.
        assert_eq!(added_or_extended_tests(&base, &[t("tests::doubles", 2)]), 1);
        // Negative: nothing about the tests changed.
        assert_eq!(added_or_extended_tests(&base, &[t("tests::doubles", 1)]), 0);
        // Negative: a rename is not a new test.
        assert_eq!(added_or_extended_tests(&base, &[t("tests::doubled", 1)]), 0);
        // Negative: a test removed.
        assert_eq!(added_or_extended_tests(&base, &[]), 0);
        // Positive: a new file's tests are all new.
        assert_eq!(added_or_extended_tests(&[], &[t("a", 0), t("b", 1)]), 2);
    }

    fn claims(body: &str) -> Vec<(usize, String)> {
        super::find_prose_test_claims(body)
            .claims
            .into_iter()
            .map(|c| (c.line, c.phrase))
            .collect()
    }

    #[test]
    fn prose_test_claim_bullet_and_sentence_forms() {
        // Positive: a bullet, first-person past tense.
        assert_eq!(
            claims("Fixes the doubling bug.\n\n- Added regression tests for negative inputs\n"),
            vec![(3, "Added regression tests".to_string())]
        );
        // Positive: a sentence after another sentence.
        assert_eq!(
            claims("Fixes the doubling bug. I added regression tests for negative inputs.\n"),
            vec![(1, "added regression tests".to_string())]
        );
        // Positive: imperative and present tense.
        assert_eq!(claims("- Add regression tests for the parser\n").len(), 1);
        assert_eq!(
            claims("This change adds unit tests for `parse`.\n").len(),
            1
        );
        assert_eq!(
            claims("- Updated the existing tests to the new API\n").len(),
            1
        );
        // Positive: passive, nominal, `with tests`, coverage, coordinated nouns.
        assert_eq!(
            claims("- Regression tests were added for the parser\n").len(),
            1
        );
        assert_eq!(claims("- Tests for negative inputs\n").len(), 1);
        assert_eq!(claims("- Test coverage for the reader\n").len(), 1);
        assert_eq!(
            claims("Fixes the reader, with regression tests.\n").len(),
            1
        );
        // Negative: a clause that opens with `with` describes something else.
        assert!(claims("Three documents (lib, doctest) with two test cases.\n").is_empty());
        // Negative: a `with` aside in parentheses, or a described item a parenthesis
        // goes on to detail.
        assert!(claims("These belong to section B (gate or delete with tests).\n").is_empty());
        assert!(claims(
            "- §32.4: the gates, each with the test (`path:line`, name), and a status.\n"
        )
        .is_empty());
        // Positive: the text after the parenthetical aside is still read.
        assert_eq!(
            claims("The reader (see #12) now rejects it, with regression tests.\n").len(),
            1
        );
        assert_eq!(claims("Improves test coverage of the writer.\n").len(), 1);
        assert_eq!(claims("- Added the flag and tests for it\n").len(), 1);
        // Positive: a sentence wrapped over two lines is read whole, on its first line.
        assert_eq!(
            claims("Fixes the bug.\nI added regression\ntests for it.\n"),
            vec![(2, "added regression tests".to_string())]
        );
        // Negative: the same paragraph without a claim.
        assert!(claims("Fixes the doubling bug.\n\n- Negative inputs now double\n").is_empty());
    }

    #[test]
    fn prose_test_claim_negated_or_deferred_is_not_a_claim() {
        for body in [
            "- No tests added: the change is a rename\n",
            "No tests: the change is a rename.\n",
            "- Refactor without tests\n",
            "Tests not needed for a comment change.\n",
            "- Did not add tests\n",
            "I didn't add tests.\n",
            "TODO: tests\n",
            "TODO: add tests for the parser\n",
            "- Will add tests in a follow-up\n",
            "- Regression tests to follow\n",
            "Should we add tests for this?\n",
            "The gate reports a change that adds tests without a test file.\n",
            "Remember to add tests.\n",
            "Two drift tests added by #505 compare the installers.\n",
            "- #505 added tests for the installers\n",
            // A template field whose value says no test was added.
            "- Tests added/extended: none (doc comments). The doc tests pass.\n",
            "Tests added: N/A\n",
            "Tests added: nothing, the change is a rename.\n",
        ] {
            assert!(claims(body).is_empty(), "{body:?}: {:?}", claims(body));
        }
    }

    #[test]
    fn prose_test_claim_outside_prose_is_not_read() {
        for body in [
            "Run `added regression tests` as a phrase.\n",
            "```\nAdded regression tests\n```\n",
            "    Added regression tests\n",
            "> Added regression tests for negative inputs\n",
            "<!-- Added regression tests for negative inputs -->\n",
            "<!--\nDescribe the tests you added:\n- Added tests for X\n-->\n",
            "The reviewer wrote \"added regression tests\" in the template.\n",
            "## Added regression tests\n",
            "- [x] Added regression tests for negative inputs\n",
            "- [ ] Added regression tests for negative inputs\n",
            "allow-pr-checklist: test regression tests were added in a separate PR\n",
            // An HTML blockquote, as a dependency bot quotes upstream release notes.
            "<blockquote>\n<h2>v4.2.2</h2>\n<ul>\n<li>Expand unit test coverage for <code>isGhes</code></li>\n</ul>\n</blockquote>\n",
            "<blockquote><p>Added regression tests for negative inputs</p></blockquote>\n",
        ] {
            assert!(claims(body).is_empty(), "{body:?}: {:?}", claims(body));
        }
    }

    #[test]
    fn prose_tests_that_exist_or_pass_are_not_claims_of_change() {
        // A statement that tests exist or pass says nothing about what the change added;
        // a count of passing tests is a different claim (`docs/GATES.md`).
        for body in [
            "All 57 tests pass.\n",
            "- Existing tests cover this path\n",
            "- Tests for the parser pass\n",
            "Tests for the reader live in `tests/reader.rs`.\n",
            "- Added test fixtures for the reader\n",
            "- Added a tests directory to the ignore list\n",
            "- Added test-only feature flag\n",
            "- Updated test plan\n",
        ] {
            assert!(claims(body).is_empty(), "{body:?}: {:?}", claims(body));
        }
        // A nominal claim whose clause says the tests were added is one.
        assert_eq!(claims("- Tests for the parser were added\n").len(), 1);
    }

    #[test]
    fn prose_test_claim_backed_by_a_test_change_stands_down() {
        let body = "Fixes the doubling bug.\n\n- Added regression tests for negative inputs\n";
        // Positive: no test in the diff.
        assert_eq!(
            super::find_unsupported_prose_test_claims(body, false)
                .claims
                .len(),
            1
        );
        // Negative: the diff adds or extends a test.
        let backed = super::find_unsupported_prose_test_claims(body, true);
        assert!(backed.claims.is_empty());
        assert_eq!(backed.units_read, 2);
    }

    #[test]
    fn prose_test_claim_documented_misses() {
        // `docs/GATES.md`, `pr-checklist`, "What it does NOT catch".
        for body in [
            "- Covered the new branch\n",
            "- Tested the parser with a new case\n",
            "I made sure not to skip it, and added tests.\n",
        ] {
            assert!(claims(body).is_empty(), "{body:?}: {:?}", claims(body));
        }
        // A negation after the claim does not cancel it.
        assert_eq!(
            claims("- Added tests for the case where no input is given\n").len(),
            1
        );
        // A `with` claim next to a parenthesis is not read.
        assert!(claims("Fixes the reader, with regression tests (see #12).\n").is_empty());
        // A field whose value names tests is still a claim.
        assert_eq!(claims("Tests added: two, for the reader.\n").len(), 1);
    }

    #[test]
    fn prose_test_claim_one_per_item() {
        let found = claims("- Added tests for A and added tests for B\n- Added unit tests\n");
        assert_eq!(found.iter().map(|c| c.0).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn test_pr_checklist_defaults() {
        let gate = PrChecklistGate::default();
        assert!(!gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
        assert!(!gate.require_declared_surfaces);
        assert!(gate.exempt_authors.is_empty());
    }

    // ---- declared surfaces (#943) --------------------------------------------------

    use super::{
        read_declaration, reconcile, select_declaration_body, DiffEvidence, Surface,
        SurfaceClassifier,
    };

    const LABELS: [&str; 8] = [
        "workflows",
        "build hooks",
        "manifests/lockfiles",
        "`discipline.toml`",
        "directives",
        "docs",
        "tests",
        "benches",
    ];

    fn section(ticked: &[&str]) -> String {
        let mut b = String::from("Summary.\n\n## Touched surfaces\n\n");
        for l in LABELS {
            let mark = if ticked.contains(&l) { "x" } else { " " };
            b.push_str(&format!("- [{mark}] {l}\n"));
        }
        b
    }

    fn classifier() -> SurfaceClassifier {
        SurfaceClassifier::new(
            &crate::config::DisciplineConfig::default(),
            "discipline.toml",
        )
        .unwrap()
    }

    fn changed(paths: &[&str]) -> Vec<(String, String)> {
        paths
            .iter()
            .map(|p| (p.to_string(), p.to_string()))
            .collect()
    }

    fn run(
        ticked: &[&str],
        paths: &[(String, String)],
        evidence: DiffEvidence,
    ) -> (Vec<Surface>, Vec<(Surface, Vec<String>)>) {
        let decl = read_declaration(&section(ticked)).unwrap();
        let (claims, gaps) = reconcile(&decl, paths, &classifier(), evidence, &|_| false);
        (
            claims.into_iter().map(|(s, _)| s).collect(),
            gaps.into_iter().map(|g| (g.surface, g.files)).collect(),
        )
    }

    #[test]
    fn the_section_is_read_as_a_closed_vocabulary() {
        let decl =
            read_declaration(&section(&["workflows", "`discipline.toml`", "tests"])).unwrap();
        let declared: Vec<Surface> = decl.declared.iter().map(|(s, _)| *s).collect();
        assert_eq!(
            declared,
            vec![Surface::Workflows, Surface::Config, Surface::Tests]
        );
        assert_eq!(decl.heading_line, 3);
        assert_eq!(decl.entry_lines.len(), 8);
        // Entry text: the label, case and backticks aside, with an aside that is not read.
        assert_eq!(Surface::parse("Docs (README.md)"), Some(Surface::Docs));
        assert_eq!(
            Surface::parse("**build hooks**: build.rs"),
            Some(Surface::BuildHooks)
        );
        assert_eq!(
            Surface::parse("manifests / lockfiles"),
            Some(Surface::Manifests)
        );
        assert_eq!(Surface::parse("`discipline.toml`"), Some(Surface::Config));
        // Outside the list: not read, never guessed.
        assert_eq!(Surface::parse("documentation"), None);
        assert_eq!(Surface::parse("docsite"), None);
        assert_eq!(Surface::parse("Added regression tests"), None);
        let other = read_declaration("## Touched surfaces\n- [x] the parser\n").unwrap();
        assert!(other.declared.is_empty());
        assert_eq!(other.unknown, vec![("the parser".to_string(), 2)]);
        // No section: none.
        assert_eq!(read_declaration("## What & Why\n- [x] workflows\n"), None);
        // The section ends at the next heading of its level or above.
        let ended =
            read_declaration("## Touched surfaces\n- [ ] docs\n### Detail\n- [x] tests\n## Verification\n- [x] workflows\n")
                .unwrap();
        let declared: Vec<Surface> = ended.declared.iter().map(|(s, _)| *s).collect();
        assert_eq!(declared, vec![Surface::Tests]);
    }

    #[test]
    fn a_fenced_quoted_or_commented_section_is_not_read() {
        // #896: an example is not a declaration.
        for body in [
            "Example:\n\n```markdown\n## Touched surfaces\n- [x] workflows\n```\n",
            "~~~\n## Touched surfaces\n- [x] workflows\n~~~\n",
            "> ## Touched surfaces\n> - [x] workflows\n",
            "<!--\n## Touched surfaces\n- [x] workflows\n-->\n",
        ] {
            assert_eq!(read_declaration(body), None, "{body:?}");
        }
        // Inside a real section, a fenced or commented entry is not read either.
        let decl = read_declaration(
            "## Touched surfaces\n<!-- - [x] docs -->\n```\n- [x] workflows\n```\n- [x] tests\n",
        )
        .unwrap();
        let declared: Vec<Surface> = decl.declared.iter().map(|(s, _)| *s).collect();
        assert_eq!(declared, vec![Surface::Tests]);
        // A fence opened by four backticks is not closed by three.
        assert_eq!(
            read_declaration("````\n```\n## Touched surfaces\n- [x] docs\n```\n````\n"),
            None
        );
    }

    #[test]
    fn false_claim_direction_positive_and_negative() {
        let none = DiffEvidence::default();
        // Positive: each file class declared, the diff touches none of them.
        let (claims, gaps) = run(
            &[
                "workflows",
                "build hooks",
                "manifests/lockfiles",
                "`discipline.toml`",
            ],
            &changed(&["src/lib.rs"]),
            none,
        );
        assert_eq!(
            claims,
            vec![
                Surface::Workflows,
                Surface::BuildHooks,
                Surface::Manifests,
                Surface::Config
            ]
        );
        assert!(gaps.is_empty());
        // Positive: the evidence entries with no evidence.
        let (claims, _) = run(
            &["directives", "docs", "tests", "benches"],
            &changed(&["a.txt"]),
            none,
        );
        assert_eq!(
            claims,
            vec![
                Surface::Directives,
                Surface::Docs,
                Surface::Tests,
                Surface::Benches
            ]
        );
        // Negative: each declared entry resolves.
        let all = DiffEvidence {
            has_tests: true,
            has_docs: true,
            has_benches: true,
            has_directives: true,
        };
        let (claims, gaps) = run(
            &LABELS,
            &changed(&[
                ".github/workflows/ci.yml",
                "build.rs",
                "Cargo.toml",
                "discipline.toml",
            ]),
            all,
        );
        assert!(claims.is_empty(), "{claims:?}");
        assert!(gaps.is_empty(), "{gaps:?}");
        // Negative: a manifest alone resolves `manifests/lockfiles`.
        let (claims, _) = run(&["manifests/lockfiles"], &changed(&["Cargo.toml"]), none);
        assert!(claims.is_empty());
    }

    #[test]
    fn gap_direction_positive_and_negative() {
        let none = DiffEvidence::default();
        // Positive: each boundary class changed, nothing declared: one gap per class.
        let (_, gaps) = run(
            &[],
            &changed(&[
                ".github/workflows/ci.yml",
                ".github/workflows/release.yml",
                "build.rs",
                "Cargo.lock",
                "discipline.toml",
            ]),
            none,
        );
        assert_eq!(
            gaps,
            vec![
                (
                    Surface::Workflows,
                    vec![
                        ".github/workflows/ci.yml".to_string(),
                        ".github/workflows/release.yml".to_string()
                    ]
                ),
                (Surface::BuildHooks, vec!["build.rs".to_string()]),
                (Surface::Manifests, vec!["Cargo.lock".to_string()]),
                (Surface::Config, vec!["discipline.toml".to_string()]),
            ]
        );
        // Negative: the same changes, declared.
        let (_, gaps) = run(
            &[
                "workflows",
                "build hooks",
                "manifests/lockfiles",
                "`discipline.toml`",
            ],
            &changed(&[
                ".github/workflows/ci.yml",
                "build.rs",
                "Cargo.lock",
                "discipline.toml",
            ]),
            none,
        );
        assert!(gaps.is_empty());
        // Negative: a manifest alone, docs, tests and source files are no boundary.
        let (_, gaps) = run(
            &[],
            &changed(&["Cargo.toml", "README.md", "tests/a.rs", "src/lib.rs"]),
            none,
        );
        assert!(gaps.is_empty(), "{gaps:?}");
    }

    #[test]
    fn a_rename_is_one_entry_and_counts_both_sides() {
        let none = DiffEvidence::default();
        let renamed = vec![(
            ".github/workflows/test.yml".to_string(),
            ".github/workflows/ci.yml".to_string(),
        )];
        // Declared: the rename resolves the entry, and no gap.
        let (claims, gaps) = run(&["workflows"], &renamed, none);
        assert!(claims.is_empty() && gaps.is_empty());
        // Undeclared: one gap naming both sides, not two.
        let (_, gaps) = run(&[], &renamed, none);
        assert_eq!(
            gaps,
            vec![(
                Surface::Workflows,
                vec![".github/workflows/ci.yml -> .github/workflows/test.yml".to_string()]
            )]
        );
        // A workflow moved out of the workflow directory is still a workflow change.
        let moved = vec![(
            "old/ci.yml".to_string(),
            ".github/workflows/ci.yml".to_string(),
        )];
        let (_, gaps) = run(&[], &moved, none);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].0, Surface::Workflows);
    }

    #[test]
    fn a_generated_lockfile_needs_no_declaration() {
        let decl = read_declaration(&section(&[])).unwrap();
        let paths = changed(&["Cargo.lock", "build.rs"]);
        let generated = |p: &str| p == "Cargo.lock";
        let (_, gaps) = reconcile(
            &decl,
            &paths,
            &classifier(),
            DiffEvidence::default(),
            &generated,
        );
        // The lockfile is exempt; a build hook is never generated output.
        let surfaces: Vec<Surface> = gaps.iter().map(|g| g.surface).collect();
        assert_eq!(surfaces, vec![Surface::BuildHooks]);
    }

    #[test]
    fn the_declaration_body_follows_the_directive_sources() {
        let all: Vec<String> = ["pr-body", "commits", "merged-pr-body"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let merged = [crate::tokens::MergedBody {
            number: 7,
            author: "a".into(),
            body: "merged".into(),
        }];
        // A pull request's body, when `pr-body` is a source.
        let b = select_declaration_body(Some("pr"), &merged, Some("commit"), &all);
        assert_eq!(b.text.as_deref(), Some("pr"));
        // Not a source: no declaration, and no fallback to another text.
        let commits_only = vec!["commits".to_string()];
        let b = select_declaration_body(Some("pr"), &[], Some("commit"), &commits_only);
        assert_eq!(b.text, None);
        assert!(b.source.contains("pr-body"), "{}", b.source);
        // A push: the merged pull request's body, else the head commit's message.
        let b = select_declaration_body(None, &merged, Some("commit"), &all);
        assert_eq!(b.text.as_deref(), Some("merged"));
        let b = select_declaration_body(None, &[], Some("commit"), &all);
        assert_eq!(b.text.as_deref(), Some("commit"));
        let no_commits = vec!["pr-body".to_string()];
        assert_eq!(
            select_declaration_body(None, &[], Some("commit"), &no_commits).text,
            None
        );
        assert_eq!(select_declaration_body(None, &[], None, &all).text, None);
    }
}
