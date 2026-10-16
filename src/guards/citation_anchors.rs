//! Citation anchors sentinel (`citation-anchors`).
//!
//! Verifies `path:line@sha "<text>"` citations against quoted line content and git history.

use crate::config::UnanchoredCitationsMode;
use crate::findings::FindingKind;
use crate::gitctx::CommitLookup;
use crate::guards::{Context, GateOutcome, PathFilter};
use anyhow::Result;

use std::sync::LazyLock;

pub const GATE: &str = "citation-anchors";

static VALID_ANCHOR_TAIL: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"^@[0-9a-fA-F]{4,40}\s+["'][^"']+["']"#).unwrap());

#[derive(Debug, Clone)]
pub struct AnchoredCitation {
    pub line_num: usize,
    pub path: String,
    pub line: usize,
    pub commit: String,
    pub quoted_text: String,
}

#[derive(Debug, Clone)]
pub struct UnanchoredCitation {
    pub line_num: usize,
    pub path: String,
    pub line: usize,
}

fn is_plausible_citation_path(path: &str) -> bool {
    if path.is_empty()
        || path.starts_with("http://")
        || path.starts_with("https://")
        || path.contains(' ')
        || path.contains('\t')
    {
        return false;
    }
    let has_slash = path.contains('/');
    let has_ext = path.rsplit('.').next().is_some_and(|ext| {
        !ext.is_empty()
            && ext.len() <= 6
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
            && ext != path
    });
    has_slash || has_ext
}

pub fn extract_anchored_citations(content: &str) -> Vec<AnchoredCitation> {
    let mut citations = Vec::new();
    let re =
        regex::Regex::new(r#"([a-zA-Z0-9_./\-]+):(\d+)@([0-9a-fA-F]{4,40})\s+["']([^"']+)["']"#)
            .unwrap();

    for (idx, line) in content.lines().enumerate() {
        let line_num = idx + 1;
        for caps in re.captures_iter(line) {
            let path = caps.get(1).unwrap().as_str();
            if !is_plausible_citation_path(path) {
                continue;
            }
            let line_val: usize = match caps.get(2).unwrap().as_str().parse() {
                Ok(v) if v > 0 => v,
                _ => continue,
            };
            let commit = caps.get(3).unwrap().as_str().to_string();
            let quoted_text = caps.get(4).unwrap().as_str().to_string();

            citations.push(AnchoredCitation {
                line_num,
                path: path.to_string(),
                line: line_val,
                commit,
                quoted_text,
            });
        }
    }

    citations
}

pub fn extract_unanchored_citations(content: &str) -> Vec<UnanchoredCitation> {
    let mut citations = Vec::new();
    let re = regex::Regex::new(r#"(?:^|[\s`"'(\[])([a-zA-Z0-9_./\-]+):(\d+)"#).unwrap();

    for (idx, line) in content.lines().enumerate() {
        let line_num = idx + 1;
        for caps in re.captures_iter(line) {
            let path = caps.get(1).unwrap().as_str();
            if !is_plausible_citation_path(path) {
                continue;
            }
            let line_match = caps.get(2).unwrap();
            let after_match = &line[line_match.end()..];
            if VALID_ANCHOR_TAIL.is_match(after_match) {
                continue;
            }
            let line_val: usize = match line_match.as_str().parse() {
                Ok(v) if v > 0 => v,
                _ => continue,
            };

            citations.push(UnanchoredCitation {
                line_num,
                path: path.to_string(),
                line: line_val,
            });
        }
    }

    citations
}

/// What a cited commit is, for [`judge_anchor`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CitedCommit {
    /// A commit the base's history holds (the base or an ancestor): its full id.
    OnBase(String),
    /// A commit in the local object database that the base's history does not hold: one
    /// the change itself made, or one from another branch. Its full id.
    NotOnBase(String),
    /// No commit by that id (missing, ambiguous, or another kind of object).
    Unresolvable,
}

/// One finding of [`judge_anchor`].
#[derive(Debug, Clone)]
pub struct AnchorFinding {
    pub kind: &'static FindingKind,
    pub message: String,
    pub remediation: &'static str,
}

/// Non-whitespace characters of a quoted text.
pub fn quote_char_count(quoted: &str) -> usize {
    quoted.chars().filter(|c| !c.is_whitespace()).count()
}

/// Judges one anchored citation: a quote of at least `min_quote_chars` non-whitespace
/// characters, a commit on the base's history (`resolve`), and the quoted text on the
/// cited line of the cited file at that commit (`content_at(full_id, path)`).
pub fn judge_anchor(
    cit: &AnchoredCitation,
    min_quote_chars: usize,
    resolve: impl FnOnce(&str) -> Result<CitedCommit>,
    content_at: impl FnOnce(&str, &str) -> Result<Option<String>>,
) -> Result<Option<AnchorFinding>> {
    let expected = cit.quoted_text.trim();
    let held = quote_char_count(expected);
    if held < min_quote_chars {
        return Ok(Some(AnchorFinding {
            kind: &crate::findings::QUOTE_TOO_SHORT,
            message: format!(
                "citation `{}:{}@{}` quotes \"{expected}\" ({held} non-whitespace character(s), fewer than {min_quote_chars}): a quote that short is found on almost any line",
                cit.path, cit.line, cit.commit
            ),
            remediation: "quote enough of the cited line to identify it (`gates.citation-anchors.min_quote_chars`)",
        }));
    }

    let full_oid = match resolve(&cit.commit)? {
        CitedCommit::OnBase(full) => full,
        CitedCommit::NotOnBase(_) => {
            return Ok(Some(AnchorFinding {
                kind: &crate::findings::COMMIT_UNRESOLVABLE,
                message: format!(
                    "commit `{}` in citation `{}:{}` is not on the base's history: evidence that only this change (or another branch) holds can be written and deleted again before merge",
                    cit.commit, cit.path, cit.line
                ),
                remediation: "cite a commit the base branch already contains",
            }));
        }
        CitedCommit::Unresolvable => {
            return Ok(Some(AnchorFinding {
                kind: &crate::findings::COMMIT_UNRESOLVABLE,
                message: format!(
                    "commit `{}` in citation `{}:{}` could not be resolved in repository history",
                    cit.commit, cit.path, cit.line
                ),
                remediation:
                    "ensure cited commit is fetched or update anchor to a reachable commit",
            }));
        }
    };

    let Some(target_content) = content_at(&full_oid, &cit.path)? else {
        return Ok(Some(AnchorFinding {
            kind: &crate::findings::FILE_NOT_FOUND,
            message: format!(
                "file `{}` cited at commit `{}` was not found",
                cit.path, cit.commit
            ),
            remediation: "verify path exists in the cited commit tree",
        }));
    };
    let target_lines: Vec<&str> = target_content.lines().collect();
    if cit.line < 1 || cit.line > target_lines.len() {
        return Ok(Some(AnchorFinding {
            kind: &crate::findings::LINE_OUT_OF_BOUNDS,
            message: format!(
                "cited line {} is out of bounds in `{}` (file has {} lines)",
                cit.line,
                cit.path,
                target_lines.len()
            ),
            remediation: "update citation line number to match cited commit source",
        }));
    }
    let actual = target_lines[cit.line - 1].trim_end();
    if !actual.contains(expected) {
        return Ok(Some(AnchorFinding {
            kind: &crate::findings::TEXT_MISMATCH,
            message: format!(
                "citation `{}:{}` text mismatch at commit `{}`: expected \"{}\", found \"{}\"",
                cit.path,
                cit.line,
                cit.commit,
                expected,
                actual.trim()
            ),
            remediation: "update citation anchor line or commit to match repository source",
        }));
    }
    Ok(None)
}

/// Whether a `DISCIPLINE_NO_NETWORK` value turns network access off, read the way the
/// forge client reads it (`src/forge.rs`): set, not blank, and neither `0` nor `false`.
pub fn network_disabled(value: Option<&str>) -> bool {
    value
        .map(str::trim)
        .is_some_and(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
}

/// Evaluates citation anchors across inspected documents.
pub fn evaluate_citation_anchors(ctx: &Context) -> Result<GateOutcome> {
    let no_network = network_disabled(std::env::var("DISCIPLINE_NO_NETWORK").ok().as_deref());
    evaluate_with_network(ctx, no_network)
}

/// [`evaluate_citation_anchors`] with the network switch given rather than read from the
/// environment.
pub fn evaluate_with_network(ctx: &Context, no_network: bool) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.citation_anchors;
    let mut out = GateOutcome::new(GATE);

    let files_to_check = if settings.diff_only {
        let changed = ctx.git.changed_files()?;
        changed
            .into_iter()
            .filter(|f| !f.is_deleted())
            .map(|f| f.path)
            .collect::<Vec<_>>()
    } else {
        ctx.git.tracked_files()?
    };

    let exempt = PathFilter::new(&settings.exempt_paths)?;
    let mut tracker = TrackerTitles::new(ctx, no_network);

    for path in files_to_check {
        if exempt.matches(&path) {
            continue;
        }

        let Some(content) = ctx.git.head_content(&path)? else {
            continue;
        };

        // 1. Anchored citations
        for cit in extract_anchored_citations(&content) {
            out.examined += 1;
            let resolve = |commit: &str| -> Result<CitedCommit> {
                Ok(match ctx.git.lookup_commit(commit) {
                    Ok(CommitLookup::Commit(full)) => {
                        if ctx.git.is_reachable_from_base(&full)? {
                            CitedCommit::OnBase(full)
                        } else {
                            CitedCommit::NotOnBase(full)
                        }
                    }
                    _ => CitedCommit::Unresolvable,
                })
            };
            let content_at = |full: &str, cited: &str| ctx.git.commit_content(full, cited);
            if let Some(f) = judge_anchor(&cit, settings.min_quote_chars, resolve, content_at)? {
                out.add_violation(
                    ctx.overridable(settings.severity),
                    f.kind,
                    &path,
                    cit.line_num,
                    f.message,
                    f.remediation,
                );
            }
        }

        // 2. Unanchored citations
        if settings.unanchored_citations != UnanchoredCitationsMode::Ignore {
            let unanchored = extract_unanchored_citations(&content);
            for cit in unanchored {
                out.examined += 1;
                let sev = match settings.unanchored_citations {
                    UnanchoredCitationsMode::Warn => crate::config::Severity::Warning,
                    UnanchoredCitationsMode::Reject => ctx.overridable(settings.severity),
                    UnanchoredCitationsMode::Ignore => unreachable!(),
                };
                out.add_violation(
                    sev,
                    &crate::findings::UNANCHORED_CITATION,
                    &path,
                    cit.line_num,
                    format!("unanchored citation `{}:{}`", cit.path, cit.line),
                    "anchor citation with commit sha and quoted text: `path:line@commit \"<quoted line>\"`",
                );
            }
        }

        // 3. Optional tracker titles
        if settings.verify_tracker_titles {
            tracker.verify(&path, &content, &mut out)?;
        }
    }

    if out.examined == 0 {
        out.notes
            .push("no citation anchors found to verify".to_string());
    }

    Ok(out)
}

static TRACKER_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"#(\d+)\s+\("([^"]+)"\)"#).expect("valid regex"));

/// The 1-based line of each `#<n> ("<title>")` reference in `content`, in order, counted
/// in one pass over the text.
pub fn tracker_references(content: &str) -> Vec<(usize, String, String)> {
    let mut out = Vec::new();
    let mut line = 1;
    let mut counted_to = 0;
    for caps in TRACKER_RE.captures_iter(content) {
        let start = caps.get(0).expect("group 0 always matches").start();
        line += content.as_bytes()[counted_to..start]
            .iter()
            .filter(|b| **b == b'\n')
            .count();
        counted_to = start;
        out.push((line, caps[1].to_string(), caps[2].to_string()));
    }
    out
}

/// The forge-title check of `#<n> ("<title>")` references. The forge is identified once
/// per run, on the first reference that needs it.
struct TrackerTitles<'c, 'a> {
    ctx: &'c Context<'a>,
    no_network: bool,
    forge: Option<crate::forge::Forge>,
}

impl<'c, 'a> TrackerTitles<'c, 'a> {
    fn new(ctx: &'c Context<'a>, no_network: bool) -> Self {
        Self {
            ctx,
            no_network,
            forge: None,
        }
    }

    fn verify(&mut self, path: &str, content: &str, out: &mut GateOutcome) -> Result<()> {
        let settings = &self.ctx.config.gates.citation_anchors;
        for (line_num, num, expected_title) in tracker_references(content) {
            let access = match self.ctx.forge.as_ref() {
                Some(a) if !self.no_network => a,
                _ => {
                    if settings.require_online {
                        return Err(anyhow::anyhow!(
                            "forge connection required to verify tracker titles but network is disabled or forge unavailable"
                        ));
                    }
                    out.notes.push(format!(
                        "tracker reference #{num} title check skipped (offline/no network)"
                    ));
                    out.examined += 1;
                    continue;
                }
            };

            let forge = match &self.forge {
                Some(f) => f.clone(),
                None => {
                    let f = (access.identify)().map_err(|e| {
                        anyhow::anyhow!("citation-anchors cannot identify the forge: {e}")
                    })?;
                    self.forge = Some(f.clone());
                    f
                }
            };
            let api_path = match forge.kind {
                crate::forge::ForgeKind::GitLab => format!(
                    "projects/{}/issues/{num}",
                    crate::forge::gitlab_project_id(&forge.repo)
                ),
                _ => format!("repos/{}/issues/{num}", forge.repo),
            };
            out.examined += 1;
            match access.api.fetch(&forge, &api_path) {
                Ok(issue_json) => {
                    let actual_title = issue_json
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    if actual_title != expected_title {
                        out.add_violation(
                            settings.severity,
                            &crate::findings::TRACKER_TITLE_MISMATCH,
                            path,
                            line_num,
                            format!(
                                "tracker reference #{num} title mismatch: expected \"{expected_title}\", forge has \"{actual_title}\""
                            ),
                            "update the cited tracker title to match the forge issue title",
                        );
                    }
                }
                Err(e) if e.kind == crate::forge::ForgeErrorKind::NotFound => {
                    out.add_violation(
                        settings.severity,
                        &crate::findings::TRACKER_TITLE_MISMATCH,
                        path,
                        line_num,
                        format!("tracker reference #{num} does not exist on forge"),
                        "verify the issue number exists on the host forge",
                    );
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "failed to fetch issue #{num} from forge: {e}"
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CitationAnchorsGate, DisciplineConfig, Severity, UnanchoredCitationsMode};

    #[test]
    fn test_citation_anchors_defaults() {
        let gate = CitationAnchorsGate::default();
        assert!(!gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
        assert!(gate.diff_only);
        assert_eq!(gate.unanchored_citations, UnanchoredCitationsMode::Warn);
        assert!(!gate.verify_tracker_titles);
        assert!(!gate.require_online);
    }

    #[test]
    fn test_anchored_citation_lifecycle() {
        // Base commit has src/service.rs with:
        // line 1: pub fn init() {}
        // line 2: pub fn invalidate_cache() {
        // line 3: }
        let base_service = "pub fn init() {}\npub fn invalidate_cache() {\n}\n";
        let (_dir, git) = crate::gitctx::test_support::repo_with_files(
            &[("src/service.rs", base_service), ("docs/spec.md", "")],
            &[],
        );
        let commit_hex = git.head_oid().unwrap();
        let short_sha = &commit_hex[..7];

        let config_toml = r#"
[meta]
version = 1
name = "t"
[gates.citation-anchors]
enabled = true
unanchored_citations = "ignore"
"#;
        let config = DisciplineConfig::from_toml_str(config_toml).unwrap();

        // 1. Valid citation matching line 2
        let valid_doc = format!("As defined in src/service.rs:2@{short_sha} \"pub fn invalidate_cache() {{\", it works.\n");
        let doc_path = git.root().join("docs/spec.md");
        std::fs::create_dir_all(doc_path.parent().unwrap()).unwrap();
        std::fs::write(&doc_path, &valid_doc).unwrap();

        let out = evaluate_citation_anchors(&crate::guards::test_support::context(&config, &git))
            .unwrap();
        assert!(out.violations.is_empty(), "{:?}", out.violations);

        // 2. Text mismatch
        let mismatch_doc = format!(
            "As defined in src/service.rs:2@{short_sha} \"non_existent_symbol\", it fails.\n"
        );
        std::fs::write(&doc_path, &mismatch_doc).unwrap();
        let out_mismatch =
            evaluate_citation_anchors(&crate::guards::test_support::context(&config, &git))
                .unwrap();
        assert_eq!(out_mismatch.violations.len(), 1);
        assert_eq!(
            out_mismatch.violations[0].code,
            "citation-anchors/text-mismatch"
        );

        // 3. Unresolvable commit
        let unresolvable_doc =
            "src/service.rs:2@deadbeefdeadbeef \"pub fn invalidate_cache() {\"\n";
        std::fs::write(&doc_path, unresolvable_doc).unwrap();
        let out_unresolvable =
            evaluate_citation_anchors(&crate::guards::test_support::context(&config, &git))
                .unwrap();
        assert_eq!(out_unresolvable.violations.len(), 1);
        assert_eq!(
            out_unresolvable.violations[0].code,
            "citation-anchors/commit-unresolvable"
        );
    }

    #[test]
    fn test_unanchored_citation_warn_and_reject() {
        let (_dir, git) = crate::gitctx::test_support::repo_with_changed_file(
            "docs/spec.md",
            "",
            "As defined in src/service.rs:42, cache invalidation occurs immediately.\n",
        );

        // Warn by default
        let warn_toml = r#"
[meta]
version = 1
name = "t"
[gates.citation-anchors]
enabled = true
unanchored_citations = "warn"
"#;
        let warn_config = DisciplineConfig::from_toml_str(warn_toml).unwrap();
        let out_warn =
            evaluate_citation_anchors(&crate::guards::test_support::context(&warn_config, &git))
                .unwrap();
        assert_eq!(out_warn.violations.len(), 1);
        assert_eq!(
            out_warn.violations[0].code,
            "citation-anchors/unanchored-citation"
        );
        assert_eq!(out_warn.violations[0].severity, Severity::Warning);

        // Reject
        let reject_toml = r#"
[meta]
version = 1
name = "t"
[gates.citation-anchors]
enabled = true
unanchored_citations = "reject"
"#;
        let reject_config = DisciplineConfig::from_toml_str(reject_toml).unwrap();
        let out_reject =
            evaluate_citation_anchors(&crate::guards::test_support::context(&reject_config, &git))
                .unwrap();
        assert_eq!(out_reject.violations.len(), 1);
        assert_eq!(
            out_reject.violations[0].code,
            "citation-anchors/unanchored-citation"
        );
        assert_eq!(out_reject.violations[0].severity, Severity::Error);
    }

    #[test]
    fn test_malformed_anchor_tail_treated_as_unanchored() {
        let content = "Reference to src/auth.rs:10@invalid without quote\n";
        let unanchored = extract_unanchored_citations(content);
        assert_eq!(unanchored.len(), 1);
        assert_eq!(unanchored[0].path, "src/auth.rs");
        assert_eq!(unanchored[0].line, 10);
    }

    const ENABLED: &str = "[meta]\nversion = 1\nname = \"t\"\n[gates.citation-anchors]\nenabled = true\nunanchored_citations = \"ignore\"\n";

    /// A repository whose base holds `src/service.rs`, with `doc` as the changed
    /// `docs/spec.md` citing `{sha}` (replaced by the base commit's short id).
    fn eval_doc(doc: &str, config_toml: &str) -> GateOutcome {
        let base_service = "pub fn init() {}\npub fn invalidate_cache() {\n}\n";
        let (_dir, git) = crate::gitctx::test_support::repo_with_files(
            &[("src/service.rs", base_service), ("docs/spec.md", "")],
            &[],
        );
        let short = git.head_oid().unwrap()[..7].to_string();
        std::fs::write(
            git.root().join("docs/spec.md"),
            doc.replace("{sha}", &short),
        )
        .unwrap();
        let config = DisciplineConfig::from_toml_str(config_toml).unwrap();
        evaluate_with_network(&crate::guards::test_support::context(&config, &git), false).unwrap()
    }

    fn codes(out: &GateOutcome) -> Vec<&str> {
        out.violations.iter().map(|v| v.code.as_str()).collect()
    }

    #[test]
    fn a_quote_shorter_than_the_minimum_is_reported_even_when_it_matches() {
        // `{` and `()` are on line 2, so a contains-check alone would pass them.
        for short in ["{", "()", "fn in"] {
            let out = eval_doc(&format!("src/service.rs:2@{{sha}} \"{short}\"\n"), ENABLED);
            assert_eq!(
                codes(&out),
                vec!["citation-anchors/quote-too-short"],
                "`{short}`"
            );
        }
        // Control: eight non-whitespace characters that match pass.
        let out = eval_doc("src/service.rs:2@{sha} \"invalidate\"\n", ENABLED);
        assert!(out.violations.is_empty(), "{:?}", out.violations);
        // The minimum is a setting: 0 turns it off.
        let off = format!("{ENABLED}min_quote_chars = 0\n");
        let out = eval_doc("src/service.rs:2@{sha} \"{\"\n", &off);
        assert!(out.violations.is_empty(), "{:?}", out.violations);
    }

    #[test]
    fn a_commit_the_base_does_not_hold_is_reported_even_when_its_text_matches() {
        let (_dir, git) = crate::gitctx::test_support::repo_with_files(
            &[("README.md", "base\n"), ("docs/spec.md", "")],
            &[],
        );
        let base = git.head_oid().unwrap();
        // The change commits evidence, then deletes it again: the commit stays in the
        // object database and on the branch, but not on the base.
        let repo = git2::Repository::open(git.root()).unwrap();
        std::fs::write(git.root().join("evidence.rs"), "fabricated evidence line\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("evidence.rs")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@example.invalid").unwrap();
        let parent = repo
            .find_commit(git2::Oid::from_str(&base).unwrap())
            .unwrap();
        let on_branch = repo
            .commit(Some("HEAD"), &sig, &sig, "add", &tree, &[&parent])
            .unwrap()
            .to_string();
        // A second commit that no ref points at (another branch, a dropped commit).
        let dangling = repo
            .commit(None, &sig, &sig, "dangling", &tree, &[&parent])
            .unwrap()
            .to_string();
        index
            .remove_path(std::path::Path::new("evidence.rs"))
            .unwrap();
        index.write().unwrap();
        std::fs::remove_file(git.root().join("evidence.rs")).unwrap();

        let config = DisciplineConfig::from_toml_str(ENABLED).unwrap();
        for cited in [&on_branch, &dangling] {
            std::fs::write(
                git.root().join("docs/spec.md"),
                format!("evidence.rs:1@{} \"fabricated evidence\"\n", &cited[..10]),
            )
            .unwrap();
            let out =
                evaluate_with_network(&crate::guards::test_support::context(&config, &git), false)
                    .unwrap();
            assert_eq!(
                codes(&out),
                vec!["citation-anchors/commit-unresolvable"],
                "{:?}",
                out.violations
            );
            assert!(
                out.violations[0]
                    .message
                    .contains("not on the base's history"),
                "{}",
                out.violations[0].message
            );
        }
        // Control: the base commit itself, citing a file it holds, verifies.
        std::fs::write(
            git.root().join("docs/spec.md"),
            format!("README.md:1@{} \"base\"\n", &base[..10]),
        )
        .unwrap();
        let lenient =
            DisciplineConfig::from_toml_str(&format!("{ENABLED}min_quote_chars = 4\n")).unwrap();
        let out =
            evaluate_with_network(&crate::guards::test_support::context(&lenient, &git), false)
                .unwrap();
        assert!(out.violations.is_empty(), "{:?}", out.violations);
    }

    #[test]
    fn tracker_reference_lines_are_counted_from_the_start_of_each_line() {
        let content =
            "#1 (\"first\")\nprose\n#2 (\"second\") and #3 (\"third\")\n\n#4 (\"fourth\")";
        let refs = tracker_references(content);
        let lines: Vec<(usize, &str)> = refs.iter().map(|(l, n, _)| (*l, n.as_str())).collect();
        assert_eq!(lines, vec![(1, "1"), (3, "2"), (3, "3"), (5, "4")]);
        assert_eq!(refs[1].2, "second");
    }

    #[test]
    fn network_switch_reads_like_the_forge_client() {
        assert!(network_disabled(Some("1")));
        assert!(network_disabled(Some("yes")));
        assert!(network_disabled(Some(" true ")));
        for off in ["0", "false", "FALSE", "", "  "] {
            assert!(!network_disabled(Some(off)), "`{off}`");
        }
        assert!(!network_disabled(None));
    }

    fn github_forge() -> crate::forge::Forge {
        crate::forge::Forge {
            kind: crate::forge::ForgeKind::GitHub,
            url: "https://github.com".into(),
            repo: "o/r".into(),
        }
    }

    /// The gate over `doc` with tracker titles on, `require_online` as given, and the
    /// forge answering from `api` (`None`: no forge at all).
    fn eval_titles(
        doc: &str,
        require_online: bool,
        api: Option<&crate::forge::CannedApi>,
        no_network: bool,
    ) -> Result<GateOutcome> {
        let (_dir, git) =
            crate::gitctx::test_support::repo_with_files(&[("docs/spec.md", "")], &[]);
        std::fs::write(git.root().join("docs/spec.md"), doc).unwrap();
        let config = DisciplineConfig::from_toml_str(&format!(
            "{ENABLED}verify_tracker_titles = true\nrequire_online = {require_online}\n"
        ))
        .unwrap();
        let identify = || Ok::<_, String>(github_forge());
        let mut ctx = crate::guards::test_support::context(&config, &git);
        ctx.forge = api.map(|api| crate::guards::ForgeAccess {
            api,
            identify: &identify,
            pull: None,
        });
        evaluate_with_network(&ctx, no_network)
    }

    fn canned() -> crate::forge::CannedApi {
        let mut api = crate::forge::CannedApi::default();
        api.responses.insert(
            "github:repos/o/r/issues/7".into(),
            serde_json::json!({"title": "Cache eviction stalls"}),
        );
        api.responses
            .insert("github:repos/o/r/issues/8".into(), serde_json::Value::Null);
        api
    }

    #[test]
    fn tracker_title_match_mismatch_and_missing_issue() {
        let api = canned();
        let doc = "Intro\n#7 (\"Cache eviction stalls\")\n#7 (\"Something else\")\n#8 (\"Gone\")\n";
        let out = eval_titles(doc, false, Some(&api), false).unwrap();
        let found: Vec<(&str, Option<usize>)> = out
            .violations
            .iter()
            .map(|v| (v.code.as_str(), v.line))
            .collect();
        assert_eq!(
            found,
            vec![
                ("citation-anchors/tracker-title-mismatch", Some(3)),
                ("citation-anchors/tracker-title-mismatch", Some(4)),
            ],
            "{:?}",
            out.violations
        );
        assert!(out.violations[0]
            .message
            .contains("forge has \"Cache eviction stalls\""));
        assert!(out.violations[1]
            .message
            .contains("does not exist on forge"));
        assert_eq!(out.examined, 3);
    }

    #[test]
    fn tracker_titles_offline_note_or_require_online_error() {
        let api = canned();
        let doc = "#7 (\"Cache eviction stalls\")\n";
        for (forge, no_network) in [(None, false), (Some(&api), true)] {
            // Offline and not required: a note, no finding, nothing fetched.
            let out = eval_titles(doc, false, forge, no_network).unwrap();
            assert!(out.violations.is_empty(), "{:?}", out.violations);
            assert!(
                out.notes
                    .iter()
                    .any(|n| n.contains("#7 title check skipped (offline/no network)")),
                "{:?}",
                out.notes
            );
            // Offline and required: the run cannot check, which is an error.
            let err = eval_titles(doc, true, forge, no_network).unwrap_err();
            assert!(
                format!("{err:#}").contains("forge connection required"),
                "{err:#}"
            );
        }
        assert!(api.log().is_empty(), "offline runs fetched {:?}", api.log());
    }
}
