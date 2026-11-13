//! `discipline replay --format html`: the adoption preview as one self-contained page.
//!
//! The page renders the [`Summary`] that `replay --json` prints and nothing else: every
//! number, row and sentence comes from a field of that JSON. The one input from outside
//! it is the web links, built from the `origin` remote as the audit page builds them (no
//! request is made). It is not the audit page: the audit lists escape hatches and re-runs
//! nothing (#319, #361); this page lists what the configuration under test would have
//! blocked. It shares the audit page's stylesheet and its escaping rules: text from
//! outside the binary (a commit subject, a file name, a gate id or finding code read from
//! a child report, an error a check stopped on) is written with `esc`, which escapes markup
//! and shows a control, bidirectional or invisible character as U+FFFD; the binary's own
//! words are written with `own`, which escapes markup only. It carries no script: every
//! section prints in sequence.
//!
//! An actor's login is never written: a refused override says that the author is not in
//! `allowed_override_actors`, not who the author is.

use crate::audit::Links;
use crate::audit_html::{plural, table, STYLE};
use crate::could_not_check::Reason;
use crate::escape::{html as own, html_author_text as esc};
use crate::replay::{last_line, Case, Summary};
use std::collections::BTreeMap;

/// What a could-not-check reason means, in the page's words.
fn reason_meaning(reason: &str) -> &'static str {
    match Reason::parse(reason) {
        Some(Reason::Configuration) => "The configuration under test could not be read for this change.",
        Some(Reason::Baseline) => "The baseline file could not be read.",
        Some(Reason::Repository) => "Git could not read the change or its parent.",
        Some(Reason::ToolMissing) => "A tool a gate runs was not found or could not start.",
        Some(Reason::ToolTimeout) => "A tool a gate runs exceeded its timeout.",
        Some(Reason::ToolchainUnavailable) => "A tool reported that its toolchain is unavailable.",
        Some(Reason::Forge) => "The merged pull request could not be read from the forge, and its body may carry the directive that lifts the finding.",
        Some(Reason::Gate) => "A gate could not run.",
        Some(Reason::Internal) | None => "The check stopped for a reason no narrower code names.",
    }
}

/// The change's label (`#N`, else the short sha), linked to its pull request on the
/// forge, else to its commit; plain text when the remote names no known forge.
fn change_link(c: &Case, links: Option<&Links>) -> String {
    let label = own(&c.label());
    match (links, c.pr) {
        (Some(l), Some(n)) => format!(
            r##"<a href="{}">{label}</a>"##,
            esc(&l.pull.replace("{n}", &n.to_string()))
        ),
        (Some(l), None) => format!(
            r##"<a href="{}">{label}</a>"##,
            esc(&l.commit.replace("{sha}", &c.sha))
        ),
        (None, _) => label,
    }
}

/// The change cell: its link, and its commit when the link went to the pull request.
fn change_cell(c: &Case, links: Option<&Links>) -> String {
    let mut out = change_link(c, links);
    if let (Some(l), Some(_)) = (links, c.pr) {
        out.push_str(&format!(
            r##" <a class="src-link" href="{}" title="The merged commit">{} ↗</a>"##,
            esc(&l.commit.replace("{sha}", &c.sha)),
            own(&c.sha.chars().take(10).collect::<String>())
        ));
    }
    if c.cached {
        out.push_str(
            r##" <span class="badge muted" title="Read from the replay cache">cached</span>"##,
        );
    }
    out
}

/// A finding's location, linked to the file and line at the change's commit.
fn location(c: &Case, file: Option<&str>, line: Option<u64>, links: Option<&Links>) -> String {
    let Some(path) = file else {
        return String::new();
    };
    let text = match line {
        Some(n) => format!("{path}:{n}"),
        None => path.to_string(),
    };
    match links {
        Some(l) => {
            // Each segment percent-encoded, as the audit page does: a `%2e%2e` segment
            // would read as `..`, and `#` or `?` would cut the link short.
            let encoded = path
                .split('/')
                .map(crate::forge::encode_segment)
                .collect::<Vec<_>>()
                .join("/");
            let href = l.file.replace("{sha}", &c.sha).replace("{path}", &encoded);
            let href = match line {
                Some(n) => href.replace("{line}", &n.to_string()),
                None => href.replace("#L{line}", ""),
            };
            format!(r##"<a href="{}">{}</a>"##, esc(&href), esc(&text))
        }
        None => esc(&text),
    }
}

/// The error findings of `gate` in a blocked change, by code and location.
fn gate_findings(c: &Case, gate: &str, links: Option<&Links>) -> String {
    let prefix = format!("{gate}/");
    let items: Vec<String> = c
        .findings
        .iter()
        .filter(|f| f.severity == "error" && f.code.starts_with(&prefix))
        .map(|f| {
            let at = location(c, f.file.as_deref(), f.line, links);
            if at.is_empty() {
                format!("<code>{}</code>", esc(&f.code))
            } else {
                format!("<code>{}</code> {at}", esc(&f.code))
            }
        })
        .collect();
    items.join("<br>")
}

/// Gate id → the blocked changes it blocked, by an error finding or a refused override,
/// in history order. The same changes `errors_by_gate` and `refused_overrides_by_gate`
/// name.
fn blocked_by_gate(s: &Summary) -> BTreeMap<&str, Vec<&Case>> {
    let mut by: BTreeMap<&str, Vec<&Case>> = BTreeMap::new();
    for c in s.cases_detail.iter().filter(|c| c.verdict == "blocked") {
        let mut gates: Vec<&str> = c
            .blocking_gates
            .iter()
            .chain(&c.refused_overrides)
            .map(String::as_str)
            .collect();
        gates.sort_unstable();
        gates.dedup();
        for g in gates {
            by.entry(g).or_default().push(c);
        }
    }
    by
}

fn blocked_section(s: &Summary, links: Option<&Links>) -> String {
    let by = blocked_by_gate(s);
    if by.is_empty() {
        return r##"<div class="panel"><p>No change would have been blocked.</p></div>"##
            .to_string();
    }
    let mut out = String::new();
    for (gate, cases) in &by {
        let rows: String = cases
            .iter()
            .map(|c| {
                let mut why = gate_findings(c, gate, links);
                if c.refused_overrides.iter().any(|g| g == gate) {
                    if !why.is_empty() {
                        why.push_str("<br>");
                    }
                    why.push_str(r##"<span class="badge bad">override refused</span> the change's author is not in <code>allowed_override_actors</code>"##);
                }
                format!(
                    "<tr><td>{}</td><td>{}</td><td>{why}</td></tr>",
                    change_cell(c, links),
                    esc(&c.subject)
                )
            })
            .collect();
        out.push_str(&format!(
            r##"<div class="panel" id="g-{id}"><h3><code>{gate}</code> <span class="muted">{n}</span></h3>{table}</div>"##,
            id = esc(gate),
            gate = esc(gate),
            n = plural(cases.len(), "change", "changes"),
            table = table(&["Change", "Subject", "What blocked it"], &rows),
        ));
    }
    out
}

fn could_not_check_section(s: &Summary, links: Option<&Links>) -> String {
    if s.could_not_check_by_reason.is_empty() {
        return r##"<div class="panel"><p>Every change was checked.</p></div>"##.to_string();
    }
    let mut out = String::new();
    for reason in s.could_not_check_by_reason.keys() {
        let rows: String = s
            .cases_detail
            .iter()
            .filter(|c| c.verdict == "could_not_check")
            .filter(|c| c.reason.unwrap_or(Reason::Internal).as_str() == reason)
            .map(|c| {
                format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                    change_cell(c, links),
                    esc(&c.subject),
                    esc(&last_line(&c.detail))
                )
            })
            .collect();
        let n = s.could_not_check_by_reason[reason].len();
        out.push_str(&format!(
            r##"<div class="panel" id="r-{id}"><h3><code>{reason}</code> <span class="muted">{n}</span></h3><p>{meaning}</p>{table}</div>"##,
            id = esc(reason),
            reason = esc(reason),
            n = plural(n, "change", "changes"),
            meaning = reason_meaning(reason),
            table = table(&["Change", "Subject", "Why"], &rows),
        ));
    }
    out
}

/// Gate id → changes, as a two-column table; empty when the map is.
fn gate_changes_panel(title: &str, note: &str, map: &BTreeMap<String, Vec<String>>) -> String {
    if map.is_empty() {
        return String::new();
    }
    let rows: String = map
        .iter()
        .map(|(g, changes)| {
            format!(
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td></tr>",
                esc(g),
                changes.len(),
                esc(&changes.join(" "))
            )
        })
        .collect();
    format!(
        r##"<div class="panel"><h3>{title}</h3><p>{note}</p>{}</div>"##,
        table(&["Gate", "Changes", "Which"], &rows)
    )
}

fn warnings_panel(s: &Summary) -> String {
    if s.warnings_by_gate.is_empty() {
        return String::new();
    }
    let rows: String = s
        .warnings_by_gate
        .iter()
        .map(|(g, n)| format!("<tr><td><code>{}</code></td><td>{n}</td></tr>", esc(g)))
        .collect();
    format!(
        r##"<div class="panel"><h3>Warnings</h3><p>Findings that would have been reported without blocking.</p>{}</div>"##,
        table(&["Gate", "Changes"], &rows)
    )
}

fn verdict_badge(v: &str) -> String {
    let (class, text) = match v {
        "passed" => ("ok", "passed"),
        "blocked" => ("bad", "blocked"),
        "could_not_check" => ("warn", "could not check"),
        other => ("muted", other),
    };
    format!(r##"<span class="badge {class}">{}</span>"##, esc(text))
}

fn every_change(s: &Summary, links: Option<&Links>) -> String {
    let rows: String = s
        .cases_detail
        .iter()
        .map(|c| {
            let mut gates: Vec<String> = c
                .blocking_gates
                .iter()
                .map(|g| format!("<code>{}</code>", esc(g)))
                .collect();
            gates.extend(
                c.refused_overrides
                    .iter()
                    .map(|g| format!("<code>{}</code> (override refused)", esc(g))),
            );
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                change_cell(c, links),
                verdict_badge(&c.verdict),
                esc(&c.subject),
                gates.join(", ")
            )
        })
        .collect();
    table(&["Change", "Verdict", "Subject", "Blocking gates"], &rows)
}

/// The replayed repository as `owner/name`, from its forge link.
fn repository_name(links: Option<&Links>) -> String {
    links
        .map(|l| {
            let mut parts: Vec<&str> = l.repository.rsplit('/').take(2).collect();
            parts.reverse();
            parts.join("/")
        })
        .unwrap_or_else(|| "this repository".to_string())
}

fn lede(s: &Summary) -> String {
    let n = s.cases;
    let changes = plural(n, "merged change", "merged changes");
    match s.blocked {
        0 => format!("Nothing in the last {changes} would have been blocked."),
        b => format!("{b} of the last {changes} would have been blocked."),
    }
}

fn figure(n: usize, label: &str) -> String {
    format!(r##"<div class="fig"><b>{n}</b><span>{label}</span></div>"##)
}

/// The page for a replay `summary`. `links` are the forge's web links from the `origin`
/// remote, or `None` when it names no known forge: the page is then unlinked.
pub fn render(s: &Summary, links: Option<&Links>) -> String {
    let repo = repository_name(links);
    let from_body = s
        .cases_detail
        .iter()
        .filter(|c| c.directives_from == "pull request body")
        .count();
    let links_line = match links {
        Some(l) => format!(
            r##"<a href="{0}">{0}</a>, from the <code>origin</code> remote"##,
            esc(&l.repository)
        ),
        None => "none: the <code>origin</code> remote names no known forge".to_string(),
    };
    let cached = if s.cached > 0 {
        format!(
            " {} of {} verdicts were read from the replay cache.",
            s.cached, s.cases
        )
    } else {
        String::new()
    };
    let skipped = gate_changes_panel(
        "Skipped checks",
        "Parts of gates a change skipped because the configuration names a file the change does not have yet.",
        &s.skipped_by_gate,
    );
    let overrides = gate_changes_panel(
        "Overrides applied",
        "Changes whose check applied an override of the gate: a directive or marker lifted a finding.",
        &s.overrides_by_gate,
    );
    let mut other = format!("{}{overrides}{skipped}", warnings_panel(s));
    if other.is_empty() {
        other = r##"<div class="panel"><p>None.</p></div>"##.to_string();
    }
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta name="generator" content="discipline {version}">
<title>Adoption Preview · {repo}</title>
<style>
{STYLE}</style>
</head>
<body>
<div class="wrap">
  <header>
    <div class="eyebrow">discipline replay · {repo} · {n} merged changes</div>
    <h1>{lede}</h1>
    <p class="sub">{passed} passed, {blocked} blocked, {cnc} could not be checked. A replay re-checks history through the configuration under test and blocks nothing.{cached}</p>
    <div class="trust" aria-label="What this report rests on">
      <dl>
        <dt>Links</dt><dd>{links_line}</dd>
        <dt>Read</dt><dd>Directives from the pull request body for {from_body} of {n} changes; the rest used the commit message only.</dd>
        <dt>Written by</dt><dd>discipline {version}</dd>
      </dl>
      <p class="warnline">A change that could not be checked is not a pass: it is listed below with why.</p>
    </div>
  </header>
  <div class="figures" aria-label="Figures">{f_cases}{f_passed}{f_blocked}{f_cnc}</div>

  <section class="view" id="blocked">
    <div class="viewhead"><h2>Blocked, by gate</h2><p>Each change the configuration under test would have blocked, under every gate that blocked it, with the findings by code and location.</p></div>
    {blocked_html}
  </section>

  <section class="view" id="could-not-check">
    <div class="viewhead"><h2>Could not check</h2><p>Changes the replay could not judge, grouped by the reason their check gave.</p></div>
    {cnc_html}
  </section>

  <section class="view" id="other">
    <div class="viewhead"><h2>Warnings, overrides and skipped checks</h2></div>
    {other}
  </section>

  <section class="view" id="changes">
    <div class="viewhead"><h2>Every change</h2><p>Newest first.</p></div>
    <div class="panel">{every}</div>
  </section>
</div>
</body>
</html>
"##,
        version = own(env!("CARGO_PKG_VERSION")),
        repo = esc(&repo),
        n = s.cases,
        lede = lede(s),
        passed = s.passed,
        blocked = s.blocked,
        cnc = s.could_not_check,
        f_cases = figure(s.cases, "merged changes"),
        f_passed = figure(s.passed, "passed"),
        f_blocked = figure(s.blocked, "blocked"),
        f_cnc = figure(s.could_not_check, "could not be checked"),
        blocked_html = blocked_section(s, links),
        cnc_html = could_not_check_section(s, links),
        every = every_change(s, links),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::{CaseFinding, CaseOverride};

    fn case(sha: &str, pr: Option<u64>, subject: &str, verdict: &'static str) -> Case {
        Case {
            sha: sha.to_string(),
            pr,
            subject: subject.to_string(),
            verdict: verdict.into(),
            blocking_gates: Vec::new(),
            refused_overrides: Vec::new(),
            overrides: Vec::new(),
            actor: None,
            warning_gates: Vec::new(),
            findings: Vec::new(),
            directives_from: String::new(),
            skipped_checks: Vec::new(),
            reason: None,
            detail: String::new(),
            cached: false,
        }
    }

    fn links() -> Links {
        Links::for_forge(&crate::forge::Forge {
            kind: crate::forge::ForgeKind::GitHub,
            url: "https://github.com".into(),
            repo: "acme/widgets".into(),
        })
    }

    fn summary() -> Summary {
        let mut blocked = case(
            "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
            Some(12),
            "test: drop an assertion",
            "blocked",
        );
        blocked.blocking_gates = vec!["assertion-reduction".into()];
        blocked.findings = vec![CaseFinding {
            code: "assertion-reduction/assertion-removed".into(),
            severity: "error".into(),
            file: Some("tests/test_x.py".into()),
            line: Some(7),
        }];
        let mut refused = case(
            "b1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
            None,
            "chore: waive it",
            "blocked",
        );
        refused.refused_overrides = vec!["test-floor".into()];
        refused.actor = Some("someone-private".into());
        refused.overrides = vec![CaseOverride {
            gate: "test-floor".into(),
            code: None,
            directive: "allow-test-floor-drop".into(),
            subject: String::new(),
            source: "commit b1b2c3d4e5".into(),
            hidden: false,
        }];
        let mut cnc = case(
            "c1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
            Some(14),
            "feat: something",
            "could_not_check",
        );
        cnc.reason = Some(Reason::Forge);
        cnc.detail = "its merged pull request could not be read: HTTP 403".into();
        let passed = case(
            "d1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
            Some(15),
            "docs: fix a typo",
            "passed",
        );
        Summary::from_cases(vec![passed, blocked, refused, cnc])
    }

    #[test]
    fn blocked_changes_are_grouped_by_gate_and_linked_to_the_forge() {
        let page = render(&summary(), Some(&links()));
        assert!(page.starts_with("<!doctype html>"));
        assert!(page.contains("2 of the last 4 merged changes would have been blocked."));
        // One panel per blocking gate, a refused override counted as its gate.
        assert!(page.contains(r#"id="g-assertion-reduction""#));
        assert!(page.contains(r#"id="g-test-floor""#));
        // The pull request, the commit, and the finding's file and line.
        assert!(page.contains(r#"<a href="https://github.com/acme/widgets/pull/12">#12</a>"#));
        assert!(page.contains(
            r#"href="https://github.com/acme/widgets/commit/b1b2c3d4e5f60718293a4b5c6d7e8f9012345678">b1b2c3d4e5</a>"#
        ));
        assert!(page.contains(
            r#"<a href="https://github.com/acme/widgets/blob/a1b2c3d4e5f60718293a4b5c6d7e8f9012345678/tests/test_x.py#L7">tests/test_x.py:7</a>"#
        ));
        assert!(page.contains("<code>assertion-reduction/assertion-removed</code>"));
        // The could-not-check case, its reason and why.
        assert!(page.contains(r#"id="r-forge""#));
        assert!(page.contains("HTTP 403"));
        // Self-contained: nothing is loaded.
        assert!(!page.contains("<script"));
        assert!(!page.contains(" src=\""));
        assert!(!page.contains("<link"));
        // An actor's login is never written.
        assert!(!page.contains("someone-private"));
    }

    #[test]
    fn without_a_known_forge_the_page_is_unlinked() {
        let page = render(&summary(), None);
        assert!(!page.contains("<a href=\"https://"));
        assert!(page.contains("none: the <code>origin</code> remote names no known forge"));
        assert!(page.contains("tests/test_x.py:7"));
    }

    #[test]
    fn hostile_subject_path_and_detail_are_escaped() {
        let mut s = summary();
        let hostile = concat!(
            r#"<script>alert(1)</script>"x' & <img src=x onerror=y> "#,
            "\u{202E}evil"
        );
        for c in &mut s.cases_detail {
            c.subject = hostile.to_string();
            c.detail = format!("discipline check: error: {hostile}");
            for f in &mut c.findings {
                f.file = Some(format!("tests/\"><script>x</script>/{hostile}.py"));
            }
        }
        let page = render(&s, Some(&links()));
        assert!(
            !page.contains("<script"),
            "a raw script tag reached the page"
        );
        assert!(!page.contains("<img"), "a raw img tag reached the page");
        assert!(
            !page.contains('\u{202E}'),
            "a bidi override reached the page"
        );
        assert!(page.contains("&lt;script&gt;alert(1)&lt;/script&gt;&quot;x&#39; &amp; &lt;img"));
        assert!(page.contains('\u{FFFD}'));
        // The link's path is percent-encoded segment by segment.
        assert!(page.contains("/tests/%22%3E%3Cscript%3Ex%3C/script%3E/"));
    }

    #[test]
    fn nothing_blocked_and_nothing_unchecked_say_so() {
        let s = Summary::from_cases(vec![case(
            "d1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
            Some(15),
            "docs: fix a typo",
            "passed",
        )]);
        let page = render(&s, None);
        assert!(page.contains("Nothing in the last 1 merged change would have been blocked."));
        assert!(page.contains("No change would have been blocked."));
        assert!(page.contains("Every change was checked."));
        assert!(page.contains("<p>None.</p>"));
    }
}
