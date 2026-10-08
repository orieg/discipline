//! `discipline audit --format html`: the audit as one self-contained page.
//!
//! The page renders the [`Summary`] the JSON output prints and nothing else: every
//! number, row and sentence comes from a field of that JSON. It loads nothing: the
//! styles and the one script are inlined, the charts are SVG drawn here, and fonts are
//! the reader's system fonts. Without the script every view prints in sequence; with it,
//! the views become tabs reachable by `#anchor` (`#protected`, `#c-<commit>`, `#g-<gate>`).
//! Text from outside the binary (a subject, a file name, a directive name, a
//! configuration key or value, a parse error, a reference, what a forge answered, the
//! remote's address) is written with `esc`: markup is escaped and a control,
//! bidirectional or invisible character is shown as U+FFFD. The binary's own words (a
//! kind, a class, a state, a label, a commit id, its version) are written with `own`,
//! which escapes markup only.

use crate::audit::{Record, Signal, Summary, GUARD_GATES};
use crate::escape::{html as own, html_author_text as esc};
use std::collections::{BTreeMap, BTreeSet};

const STYLE: &str = include_str!("audit_html/style.css");
const SCRIPT: &str = include_str!("audit_html/app.js");

/// `YYYY-MM-DD` (UTC) for seconds since the Unix epoch.
pub fn date(secs: i64) -> String {
    // Days to civil date: H. Hinnant, "chrono-Compatible Low-Level Date Algorithms"
    // (http://howardhinnant.github.io/date_algorithms.html, `civil_from_days`).
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn label(r: &Record) -> String {
    match r.pr {
        Some(n) => format!("#{n}"),
        None => r.sha.chars().take(10).collect(),
    }
}

fn anchor(r: &Record) -> String {
    format!("c-{}", &r.sha[..r.sha.len().min(10)])
}

/// Link to a change's entry in the Changes view.
fn ch(r: &Record) -> String {
    format!(
        r##"<a href="#{}" class="chg">{}</a>"##,
        anchor(r),
        own(&label(r))
    )
}

/// Where a file sits: markers in tests and docs are expected in a repository that
/// documents its own markers; the others are the ones to read.
fn role(path: Option<&str>) -> &'static str {
    let Some(p) = path else { return "other" };
    let name = p.rsplit('/').next().unwrap_or(p);
    if p.starts_with(".github/")
        || p.starts_with(".gitea/")
        || p.starts_with(".forgejo/")
        || p == "action.yml"
        || p == ".gitlab-ci.yml"
    {
        "workflow"
    } else if matches!(name, "AGENTS.md" | "CLAUDE.md" | "GEMINI.md")
        || p.starts_with(".claude/")
        || p.starts_with(".agents/")
    {
        "agent instructions"
    } else if p.starts_with("tests/")
        || p.starts_with("test/")
        || p.starts_with("docs/")
        || p.contains("/tests/")
        || p.contains("fixtures")
        || p.ends_with(".md")
        || p.contains("selftest")
    {
        "test or doc"
    } else {
        "live code"
    }
}

fn change_str(r: &Record) -> String {
    r.change
        .and_then(|c| serde_json::to_value(c).ok())
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The removed keys a configuration comparison set aside, for the end of its line.
fn set_aside(r: &Record) -> String {
    match r.detail.as_deref() {
        Some(note) => format!(" ({})", esc(note)),
        None => String::new(),
    }
}

fn signal_sentence(s: &Signal, summary: &Summary) -> String {
    let n = s.count;
    let c = s.changes.len();
    let gates: BTreeSet<&str> = s
        .records
        .iter()
        .filter_map(|&i| {
            signal_list(s, summary)
                .get(i)
                .and_then(|r| r.gate.as_deref())
        })
        .collect();
    let gates = gates
        .iter()
        .map(|g| format!("<code>{}</code>", esc(g)))
        .collect::<Vec<_>>()
        .join(", ");
    let settings = plural(n, "setting", "settings");
    let waivers = plural(n, "waiver", "waivers");
    let were = if n == 1 { "was" } else { "were" };
    let point = if n == 1 { "points" } else { "point" };
    match s.id {
        "guard-gate-loosened" => {
            let what: Vec<String> = s
                .records
                .iter()
                .filter_map(|&i| signal_list(s, summary).get(i).and_then(|r| r.gate.as_deref()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(|g| format!("<code>{}</code> {}", esc(g), guard_role(g)))
                .collect();
            format!(
                "{settings} {were} loosened in a check that protects the others: {}",
                what.join("; ")
            )
        }
        "hidden-directive" => format!(
            "{waivers} {were} written inside an HTML comment, where someone reading the commit or pull request would not see {}",
            if n == 1 { "it" } else { "them" }
        ),
        "config-unreadable" => format!(
            "{} of <code>discipline.toml</code> could not be read, so what {} changed is unknown",
            plural(n, "past version", "past versions"),
            if n == 1 { "it" } else { "they" }
        ),
        "loosened-without-pull-request" => format!(
            "{settings} {were} loosened in {} straight to the branch, with no pull request where anyone could review {}",
            plural(c, "push", "pushes"),
            if n == 1 { "it" } else { "them" }
        ),
        "loosening-without-waiver" => format!(
            "{settings} {were} loosened with no written reason (an <code>allow-gate-weakening</code> line) in the commit message"
        ),
        "waived-then-loosened" => format!(
            "Findings of {gates} were waived one change at a time, and later the check itself was loosened ({})",
            plural(n, "time", "times")
        ),
        "loosened-not-restored" => format!("{settings} loosened in the past {were} never tightened back"),
        "baseline-grew" => format!(
            "Existing findings of {gates} were added to the baseline, so they no longer block a change"
        ),
        "waiver-cites-missing-issue" => format!(
            "{waivers} {point} to an issue that does not exist"
        ),
        "waiver-cites-issue-closed-before" => format!(
            "{waivers} {point} to an issue that was already closed when {} written; check that the issue still supports the waiver",
            if n == 1 { "it was" } else { "they were" }
        ),
        "waiver-lifted-nothing" => format!(
            "{waivers} lifted nothing when the change was re-checked with the current discipline: either the finding was never there, or the gate has changed since {}",
            if n == 1 { "it was written" } else { "they were written" }
        ),
        "waiver-cites-issue-not-planned" => format!(
            "{waivers} {point} to an issue later closed as not planned, so the promised follow-up will not happen"
        ),
        "protected-edit-unratified" => format!(
            "{} to protected files had no owner approval the check accepts",
            plural(n, "edit", "edits")
        ),
        "protected-edit-self-ratified" => format!(
            "{} to protected files {were} approved only by the account that opened the pull request, so no second person agreed",
            plural(n, "edit", "edits")
        ),
        _ => plural(n, "record", "records"),
    }
}

/// What a gate that guards the other gates protects, in plain words.
fn guard_role(gate: &str) -> &'static str {
    match gate {
        "ratified-paths" => "decides which edits to protected files, such as CI workflows and discipline's own configuration, need an owner's approval",
        "config-integrity" => "reports every change that loosens discipline's own configuration",
        "ci-integrity" => "reports every change that weakens the CI workflows that run the checks",
        "instruction-smuggling" => "reports edits to the instruction files coding agents read",
        "sandbox-config" => "reports every change that widens a coding agent's permissions or sandbox, including the hooks that run discipline",
        _ => "protects the other checks",
    }
}

/// `1 change`, `2 changes`.
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Escaped text with each `` `code` `` span set as `<code>`.
fn prose(s: &str) -> String {
    esc(s)
        .split('`')
        .enumerate()
        .map(|(i, part)| {
            if i % 2 == 1 {
                format!("<code>{part}</code>")
            } else {
                part.to_string()
            }
        })
        .collect()
}

/// The list a signal's indexes point into.
fn signal_list<'a>(s: &Signal, summary: &'a Summary) -> &'a [Record] {
    if s.list == "protected_edits" {
        &summary.protected_edits
    } else {
        &summary.records
    }
}

fn change_links(s: &Signal, summary: &Summary) -> String {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for &i in &s.records {
        let Some(r) = signal_list(s, summary).get(i) else {
            continue;
        };
        if seen.insert(r.sha.clone()) {
            out.push(ch(r));
        }
    }
    let more = out.len().saturating_sub(10);
    let mut text = out.into_iter().take(10).collect::<Vec<_>>().join(", ");
    if more > 0 {
        text.push_str(&format!(" and {more} more"));
    }
    text
}

const RANK_LABEL: &[(&str, &str, &str)] = &[
    ("look-first", "Look first", "critical"),
    ("look-soon", "Look soon", "high"),
    ("review", "Review", "medium"),
];

fn rank(r: &str) -> (&'static str, &'static str) {
    RANK_LABEL
        .iter()
        .find(|(k, _, _)| *k == r)
        .map(|(_, l, c)| (*l, *c))
        .unwrap_or(("Review", "medium"))
}

const CHECK_LABEL: &[(&str, &str)] = &[
    ("guard-gate-loosened", "Guard gates loosened"),
    ("hidden-directive", "Hidden directives"),
    ("config-unreadable", "Unreadable configurations"),
    (
        "loosened-without-pull-request",
        "Loosened with no pull request",
    ),
    ("loosening-without-waiver", "Loosened with no waiver"),
    ("waived-then-loosened", "Waived, then loosened"),
    ("loosened-not-restored", "Loosenings not restored"),
    ("baseline-grew", "Baseline growth"),
    ("protected-edit-unratified", "Unratified protected edits"),
    (
        "waiver-cites-missing-issue",
        "Waivers citing a missing issue",
    ),
    (
        "waiver-cites-issue-closed-before",
        "Waivers citing an issue closed before them",
    ),
    (
        "waiver-cites-issue-not-planned",
        "Waivers citing an issue not planned",
    ),
    ("waiver-lifted-nothing", "Waivers that lifted nothing"),
    ("cited-issues", "Issues waivers cite"),
    (
        "protected-edit-self-ratified",
        "Self-ratified protected edits",
    ),
    ("directive-lifted-a-finding", "Waivers that lifted nothing"),
    (
        "pull-request-body-directives",
        "Directives in pull request bodies",
    ),
    ("owner-ratification", "Owner ratification"),
    ("independent-review", "Review by another person"),
    (
        "pull-request-body-edited",
        "Pull request bodies edited after the merge",
    ),
    ("agent-identity", "Which agent made a change"),
];

fn check_label(id: &str) -> &str {
    CHECK_LABEL
        .iter()
        .find(|(k, _)| *k == id)
        .map(|(_, l)| *l)
        .unwrap_or(id)
}

fn dot(class: &str) -> String {
    format!(r##"<span class="dot k-{}"></span>"##, own(class))
}

fn table(head: &[&str], rows: &str) -> String {
    let th: String = head.iter().map(|h| format!("<th>{h}</th>")).collect();
    format!(
        r##"<div class="tablebox"><table><thead><tr>{th}</tr></thead><tbody>{rows}</tbody></table></div>"##
    )
}

/// A restoring tightening of the same option in a newer change, if any.
fn restored_by<'a>(r: &Record, s: &'a Summary) -> Option<&'a Record> {
    s.tightenings
        .iter()
        .filter(|t| t.ord < r.ord && t.gate == r.gate && t.key == r.key)
        .max_by_key(|t| t.ord)
}

fn waived_in_change(r: &Record, s: &Summary) -> bool {
    s.records.iter().any(|x| {
        x.kind == "directive" && x.sha == r.sha && x.gate.as_deref() == Some("config-integrity")
    })
}

/// Share of changes, per window of `bin` changes, that carried a non-routine exception,
/// oldest window first, with configuration loosenings marked.
fn rate_chart(s: &Summary) -> String {
    let n = s.changes.max(1);
    let bin = 20.min(n);
    let nb = n.div_ceil(bin);
    let mut hit: BTreeSet<usize> = BTreeSet::new();
    let mut newest_time: BTreeMap<usize, i64> = BTreeMap::new();
    for r in &s.records {
        if r.class != "process" {
            hit.insert(r.ord);
        }
    }
    for r in s
        .records
        .iter()
        .chain(&s.protected_edits)
        .chain(&s.tightenings)
    {
        newest_time.entry(r.ord).or_insert(r.time);
    }
    // Window b (0 = oldest) covers change positions [lo, hi).
    let window = |b: usize| {
        let hi = n - b * bin;
        let lo = hi.saturating_sub(bin);
        (lo, hi)
    };
    let (w, h, l, bottom, top) = (760.0, 230.0, 40.0, 44.0, 14.0);
    let (pw, ph) = (w - l - 12.0, h - bottom - top);
    let x = |b: usize| l + pw * (b as f64 + 0.5) / nb as f64;
    let y = |v: f64| top + ph - ph * v;
    let mut o = String::new();
    o.push_str(&format!(r##"<svg viewBox="0 0 {w} {h}" class="chart" role="img" aria-labelledby="t-rate"><title id="t-rate">Share of changes with an exception beyond a skipped issue link, per {bin} merged changes, oldest on the left</title>"##));
    for v in [0.0, 0.25, 0.5, 0.75, 1.0] {
        o.push_str(&format!(r##"<line x1="{l}" x2="{}" y1="{:.1}" y2="{:.1}" class="grid"/><text x="{}" y="{:.1}" class="tick" text-anchor="end">{}%</text>"##,
            w - 12.0,
            y(v),
            y(v),
            l - 6.0,
            y(v) + 4.0,
            (v * 100.0) as u32));
    }
    let mut pts = Vec::new();
    for b in 0..nb {
        let (lo, hi) = window(b);
        let share = hit.range(lo..hi).count() as f64 / (hi - lo) as f64;
        pts.push((b, share, lo, hi));
    }
    let poly: Vec<String> = pts
        .iter()
        .map(|(b, v, _, _)| format!("{:.1},{:.1}", x(*b), y(*v)))
        .collect();
    o.push_str(&format!(
        r##"<polyline class="line" points="{}"/>"##,
        poly.join(" ")
    ));
    for (b, v, lo, hi) in &pts {
        let when = newest_time
            .range(lo..hi)
            .next()
            .map(|(_, t)| format!(" up to {}", date(*t)))
            .unwrap_or_default();
        o.push_str(&format!(r##"<circle cx="{:.1}" cy="{:.1}" r="3" class="pt"><title>{}% of {} changes{when}</title></circle>"##,
            x(*b),
            y(*v),
            (v * 100.0).round() as u32,
            hi - lo));
    }
    let step = (nb / 6).max(1);
    for b in (0..nb).step_by(step) {
        let (lo, hi) = window(b);
        if let Some((_, t)) = newest_time.range(lo..hi).next() {
            o.push_str(&format!(
                r##"<text x="{:.1}" y="{}" class="tick" text-anchor="middle">{}</text>"##,
                x(b),
                h - bottom + 16.0,
                &date(*t)[5..]
            ));
        }
    }
    let mut marked = BTreeSet::new();
    for r in s.records.iter().filter(|r| r.kind == "config") {
        if !marked.insert(r.sha.clone()) {
            continue;
        }
        let b = (n - 1 - r.ord.min(n - 1)) / bin;
        let guard = r.gate.as_deref().is_some_and(|g| GUARD_GATES.contains(&g));
        let cls = if guard { "mk guard" } else { "mk" };
        o.push_str(&format!(r##"<line x1="{:.1}" x2="{:.1}" y1="{top}" y2="{}" class="{cls}"><title>{}: {} loosened</title></line>"##,
            x(b),
            x(b),
            top + ph,
            own(&label(r)),
            esc(r.gate.as_deref().unwrap_or(""))));
        if guard {
            o.push_str(&format!(
                r##"<text x="{:.1}" y="{}" class="ann" text-anchor="end">{} {}</text>"##,
                x(b) - 4.0,
                top + 10.0,
                own(&label(r)),
                esc(r.gate.as_deref().unwrap_or(""))
            ));
        }
    }
    o.push_str(&format!(r##"<text x="{l}" y="{}" class="tick">oldest</text><text x="{}" y="{}" class="tick" text-anchor="end">newest · red lines: config loosenings</text></svg>"##,
        h - 8.0,
        w - 12.0,
        h - 8.0));
    o
}

/// Finding waivers and markers outside tests and docs, per gate.
fn gate_chart(s: &Summary) -> String {
    let mut waivers: BTreeMap<&str, usize> = BTreeMap::new();
    let mut markers: BTreeMap<&str, usize> = BTreeMap::new();
    for r in &s.records {
        let Some(g) = r.gate.as_deref() else { continue };
        if r.kind == "directive" && r.class == "detector" {
            *waivers.entry(g).or_default() += 1;
        } else if r.kind == "inline-marker"
            && role(r.file.as_deref()) != "test or doc"
            && r.lifted != Some(false)
        {
            *markers.entry(g).or_default() += 1;
        }
    }
    let mut gates: Vec<(&str, usize)> = waivers
        .keys()
        .chain(markers.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|g| {
            (
                *g,
                waivers.get(g).unwrap_or(&0) + markers.get(g).unwrap_or(&0),
            )
        })
        .collect();
    gates.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    gates.truncate(12);
    if gates.is_empty() {
        return r##"<p class="muted">No finding waivers or markers outside tests and docs.</p>"##
            .to_string();
    }
    let max = gates[0].1;
    let top = max.div_ceil(5).max(1) * 5;
    let (w, rh, l) = (760.0, 22.0, 180.0);
    let h = rh * gates.len() as f64 + 30.0;
    let pw = w - l - 40.0;
    let mut o = String::new();
    o.push_str(&format!(r##"<svg viewBox="0 0 {w} {h}" class="chart" role="img" aria-labelledby="t-gate"><title id="t-gate">Finding waivers and inline markers outside tests and docs, per gate</title>"##));
    for t in (0..=top).step_by(5) {
        let xx = l + pw * t as f64 / top as f64;
        o.push_str(&format!(r##"<line x1="{xx:.1}" x2="{xx:.1}" y1="4" y2="{}" class="grid"/><text x="{xx:.1}" y="{}" class="tick" text-anchor="middle">{t}</text>"##,
            h - 24.0,
            h - 8.0));
    }
    for (i, (g, total)) in gates.iter().enumerate() {
        let yy = 6.0 + i as f64 * rh;
        let cls = if GUARD_GATES.contains(g) {
            "glabel guardname"
        } else {
            "glabel"
        };
        o.push_str(&format!(r##"<a href="#g-{g}"><text x="{}" y="{}" class="{cls}" text-anchor="end">{g}</text></a>"##,
            l - 8.0,
            yy + 13.0,
            g = esc(g)));
        let mut xx = l;
        for (count, cls, one, many) in [
            (
                waivers.get(g).copied().unwrap_or(0),
                "k-detector",
                "finding waiver",
                "finding waivers",
            ),
            (
                markers.get(g).copied().unwrap_or(0),
                "k-inline",
                "marker outside tests and docs",
                "markers outside tests and docs",
            ),
        ] {
            if count > 0 {
                let ww = pw * count as f64 / top as f64;
                o.push_str(&format!(r##"<rect x="{xx:.1}" y="{yy}" width="{ww:.1}" height="{}" class="{cls}"><title>{}: {}</title></rect>"##,
                    rh - 8.0,
                    esc(g),
                    plural(count, one, many)));
                xx += ww;
            }
        }
        o.push_str(&format!(
            r##"<text x="{:.1}" y="{}" class="val">{total}</text>"##,
            xx + 5.0,
            yy + 13.0
        ));
    }
    o.push_str("</svg>");
    o
}

/// Link to where the record's evidence is: the file and line at the commit, the file's
/// diff in the commit, or the commit whose message holds the directive.
fn source(r: &Record, s: &Summary) -> String {
    let Some(l) = &s.links else {
        return String::new();
    };
    let file_link = |path: &str| {
        // Each segment percent-encoded: a browser reads a `%2e%2e` segment as `..`, which
        // would walk the link out of the repository, and `#` or `?` would cut it short.
        let path = path
            .split('/')
            .map(crate::forge::encode_segment)
            .collect::<Vec<_>>()
            .join("/");
        let mut t = l.file.replace("{sha}", &r.sha).replace("{path}", &path);
        t = match r.line {
            Some(n) => t.replace("{line}", &n.to_string()),
            None => t.replace("#L{line}", ""),
        };
        t
    };
    let (href, text) = match (r.kind, r.file.as_deref()) {
        ("directive", _) => match (r.source, r.pr) {
            (Some("pull-request-body"), Some(n)) => (
                l.pull.replace("{n}", &n.to_string()),
                "pull request body".to_string(),
            ),
            _ => (
                l.commit.replace("{sha}", &r.sha),
                "commit message".to_string(),
            ),
        },
        ("protected-edit", Some(p)) => match &l.file_diff {
            Some(d) => (
                d.replace("{sha}", &r.sha).replace(
                    "{path_sha256}",
                    &crate::report::gitlab::sha256_hex(p.as_bytes()),
                ),
                "diff".to_string(),
            ),
            None => (l.commit.replace("{sha}", &r.sha), "commit".to_string()),
        },
        (_, Some(p)) => (
            file_link(p),
            match r.line {
                Some(n) => format!("{p}:{n}"),
                None => p.to_string(),
            },
        ),
        _ => (l.commit.replace("{sha}", &r.sha), "commit".to_string()),
    };
    // A waiver read from a body edited after the merge may not be what the gates read.
    let edited = r.source == Some("pull-request-body")
        && s.pulls
            .iter()
            .any(|p| Some(p.pr) == r.pr && p.body_edited_after_merge == Some(true));
    format!(
        r##" <a class="src-link" href="{}" title="Open where this is">{} ↗</a>{}"##,
        esc(&href),
        esc(&text),
        if edited {
            r##" <span class="badge warn" title="Compare the pull request's edit history with the waiver">edited after the merge</span>"##
        } else {
            ""
        }
    )
}

/// Whether the replay applied a waiver: empty when it did not judge it.
fn lifted_badge(r: &Record) -> &'static str {
    match r.lifted {
        Some(true) => {
            r##" <span class="badge ok" title="The replayed check applied it to a finding">lifted a finding</span>"##
        }
        Some(false) => {
            r##" <span class="badge warn" title="The replayed check had no finding for it to lift">lifted nothing</span>"##
        }
        None => "",
    }
}

/// The issues a waiver cites, each linked, with its state when `--forge` read it.
fn cites_cell(r: &Record, s: &Summary) -> String {
    r.cites
        .iter()
        .map(|t| {
            let fact = s.issues.iter().find(|f| &f.reference == t);
            let href = match (&s.links, fact) {
                (Some(l), Some(f)) if !f.repo.is_empty() => Some(
                    l.issue
                        .replace("{repo}", &f.repo)
                        .replace("{n}", &f.number.to_string()),
                ),
                _ => None,
            };
            let label = match href {
                Some(h) => format!(r##"<a href="{}">{}</a>"##, esc(&h), esc(t)),
                None => format!("<code>{}</code>", esc(t)),
            };
            let state = fact
                .map(|f| match (f.state, f.state_reason.as_deref()) {
                    ("closed", Some("not_planned")) => {
                        r##" <span class="badge warn">closed, not planned</span>"##.to_string()
                    }
                    ("closed", _) => format!(
                        r##" <span class="badge muted">closed{}</span>"##,
                        f.closed_at
                            .map(|t| format!(" {}", date(t)))
                            .unwrap_or_default()
                    ),
                    ("not-found", _) => {
                        r##" <span class="badge bad">not found</span>"##.to_string()
                    }
                    ("open", _) => r##" <span class="badge ok">open</span>"##.to_string(),
                    (other, _) => format!(r##" <span class="badge muted">{}</span>"##, own(other)),
                })
                .unwrap_or_default();
            format!("{label}{state}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Each path's ratification in one change, as `--forge` judged it, linked to the comment.
fn ratification_cell(rs: &[&Record], s: &Summary) -> String {
    let mut seen: Vec<String> = Vec::new();
    for r in rs {
        let cell = match &r.ratification {
            None => r##"<span class="badge muted">Not checked</span>"##.to_string(),
            Some(f) => {
                let (cls, text) = match f.state {
                    "ratified" => ("ok", "Ratified"),
                    "self-ratified" => ("warn", "By the author's own login"),
                    "unratified" => ("bad", "Unratified"),
                    "not-required" => ("muted", "Not required"),
                    "never-ratifiable" => ("bad", "Never ratifiable"),
                    _ => ("muted", "Not checked"),
                };
                let link = match (&s.links, &f.issue_repo, f.issue, &f.comment_id) {
                    (Some(l), Some(repo), Some(n), Some(id)) => format!(
                        r##" <a class="src-link" href="{}" title="Open the ratifying comment">#{n} comment{} ↗</a>"##,
                        esc(&l
                            .issue_comment
                            .replace("{repo}", repo)
                            .replace("{n}", &n.to_string())
                            .replace("{id}", id)),
                        f.created
                            .map(|t| format!(", {}", date(t)))
                            .unwrap_or_default()
                    ),
                    _ => f
                        .why
                        .as_deref()
                        .map(|w| format!(r##"<br><span class="muted">{}</span>"##, esc(w)))
                        .unwrap_or_default(),
                };
                format!(r##"<span class="badge {cls}">{text}</span>{link}"##)
            }
        };
        if !seen.contains(&cell) {
            seen.push(cell);
        }
    }
    seen.join("<br>")
}

/// Whether another login approved the change's pull request, as `--forge` read it.
fn review_cell(r: &Record, s: &Summary) -> String {
    if s.forge.is_none() {
        return r##"<span class="badge muted">Not checked</span>"##.to_string();
    }
    match s.pulls.iter().find(|p| p.sha == r.sha) {
        Some(p) if p.approved_by_other => r##"<span class="badge ok">Yes</span>"##.to_string(),
        Some(_) => r##"<span class="badge warn">No</span>"##.to_string(),
        None if s.forge.as_ref().is_some_and(|f| f.failed > 0) => {
            r##"<span class="badge muted">Not checked</span>"##.to_string()
        }
        None => r##"<span class="badge warn">No pull request</span>"##.to_string(),
    }
}

fn external(r: &Record, s: &Summary) -> String {
    match (&s.links, r.pr) {
        (Some(l), Some(n)) => format!(
            r##"<a href="{}">pull request #{n}</a>"##,
            esc(&l.pull.replace("{n}", &n.to_string()))
        ),
        (Some(l), None) => format!(
            r##"<a href="{}">commit</a>"##,
            esc(&l.commit.replace("{sha}", &r.sha))
        ),
        (None, _) => String::new(),
    }
}

/// The span of dates the records cover, as the eyebrow states it.
fn record_span(s: &Summary) -> String {
    let times = s
        .records
        .iter()
        .chain(&s.tightenings)
        .chain(&s.protected_edits)
        .map(|r| r.time);
    match (times.clone().min(), times.max()) {
        (Some(a), Some(b)) => format!(", records from {} to {}", date(a), date(b)),
        _ => String::new(),
    }
}

/// The audited repository as `owner/name`, from its forge link.
fn repository_name(s: &Summary) -> String {
    s.links
        .as_ref()
        .map(|l| {
            l.repository
                .rsplit('/')
                .take(2)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_else(|| "this repository".to_string())
}

/// The headline: how many findings need a look.
fn lede_text(s: &Summary) -> String {
    let n = s.changes;
    if s.signals.is_empty() {
        format!("Nothing in the last {n} merged changes needs a look.")
    } else {
        format!(
            "{} in the last {n} merged changes {} a look.",
            plural(s.signals.len(), "finding", "findings"),
            if s.signals.len() == 1 {
                "needs"
            } else {
                "need"
            }
        )
    }
}

/// The paragraph that points at the first signal, when there is one.
fn start_here(s: &Summary) -> String {
    let top = s.signals.first().map(|first| {
        format!(
            "Start here: {} ({}).",
            signal_sentence(first, s),
            change_links(first, s)
        )
    });
    top.map(|t| format!(r##"<p class="top">{t}</p>"##))
        .unwrap_or_default()
}

/// How many changes carry a record other than a skipped issue link.
fn nonroutine_changes(s: &Summary) -> usize {
    let nonroutine: BTreeSet<&str> = s
        .records
        .iter()
        .filter(|r| r.class != "process")
        .map(|r| r.sha.as_str())
        .collect();
    nonroutine.len()
}

/// The sentence under the headline: how many changes used an escape hatch.
fn sublede_text(s: &Summary, nonroutine: usize) -> String {
    let n = s.changes;
    format!(
        "{} of {n} merged changes used an escape hatch, not counting pull requests that skipped linking an issue. Waivers are shown as their authors wrote them; {}",
        nonroutine,
        if s.replay.is_some() {
            "the replay says which ones lifted a finding when re-checked with the current discipline."
        } else {
            "the report does not say whether each one was needed."
        }
    )
}

/// The trust panel's line on where the links point.
fn forge_links_line(s: &Summary) -> String {
    match &s.links {
        Some(l) => format!(
            r##"<a href="{0}">{0}</a>, from the <code>origin</code> remote"##,
            esc(&l.repository)
        ),
        None => "none: the <code>origin</code> remote names no known forge".to_string(),
    }
}

/// The trust panel's line on what was read: git objects, the forge, a replay report.
fn read_line(s: &Summary) -> String {
    let read = match &s.forge {
        None => "Git objects only: commit messages, <code>discipline.toml</code>, <code>discipline-baseline.toml</code> and the changed files. Nothing was read from the network.".to_string(),
        Some(f) => format!(
            "Git objects, and from the forge each change's merged pull request and its reviews ({} of {} changes arrived through one{}).",
            f.pulls,
            f.changes,
            if f.failed > 0 { format!("; the forge could not answer for {}", f.failed) } else { String::new() }
        ),
    };
    let replay = match &s.replay {
        Some(r) => format!(
            " A <code>discipline replay</code> report of {} changes ({} checked) says which waivers lifted a finding.",
            r.cases, r.checked
        ),
        None => String::new(),
    };
    read + &replay
}

/// The strip of what was checked: protected-path edits first, then each check, found
/// ones first.
fn checks_list(s: &Summary) -> String {
    let mut checks = String::new();
    let mut protected_check = String::new();
    if !s.protected_edits.is_empty() {
        let changes: BTreeSet<&str> = s.protected_edits.iter().map(|r| r.sha.as_str()).collect();
        protected_check.push_str(&format!(r##"<li class="chk found"><span class="st">Found</span><span class="what">{} protected-path edits in {} changes</span><span class="src">from git; {}</span></li>"##,
            s.protected_edits.len(),
            changes.len(),
            if s.forge.is_some() {
                "ratification under Owner ratification"
            } else {
                "ratification not checked"
            }));
    }
    checks.push_str(&protected_check);
    let mut ordered: Vec<&crate::audit::Check> = s.checks.iter().collect();
    ordered.sort_by_key(|c| match c.state {
        "found" => 0,
        "clean" => 1,
        _ => 2,
    });
    for c in ordered {
        let (cls, st) = match c.state {
            "found" => ("found", "Found"),
            "clean" => ("clean", "Checked, none"),
            _ => ("unchecked", "Not checked"),
        };
        let src = if c.state == "clean" {
            "from git".to_string()
        } else {
            prose(&c.detail)
        };
        checks.push_str(&format!(r##"<li class="chk {cls}"><span class="st">{st}</span><span class="what">{}</span><span class="src">{src}</span></li>"##,
            own(check_label(c.id))));
    }
    checks
}

/// The ranked list of signals that need a decision.
fn decisions_list(s: &Summary) -> String {
    let mut decisions = String::new();
    for sg in &s.signals {
        let (l, cls) = rank(sg.rank);
        decisions.push_str(&format!(r##"<li class="dec {cls}"><span class="sev">{l}</span><div><p>{} ({}).</p><p class="act"><b>Next:</b> {}</p></div></li>"##,
            signal_sentence(sg, s),
            change_links(sg, s),
            prose(sg.next)));
    }
    if decisions.is_empty() {
        decisions.push_str(r##"<li class="dec low"><span class="sev">Clean</span><div><p>No signal found anything in these changes. The checks above say what was not looked at.</p></div></li>"##);
    }
    decisions
}

/// The four figures of the overview.
fn figures(s: &Summary, loosenings: &[&Record], nonroutine: usize) -> String {
    let n = s.changes;
    let open = loosenings
        .iter()
        .filter(|r| restored_by(r, s).is_none())
        .count();
    let not_checked = s.checks.iter().filter(|c| c.state == "not-checked").count();
    let figs = [
        (
            loosenings.len().to_string(),
            "config loosenings",
            format!("{open} not restored"),
        ),
        (
            format!("{}/{n}", nonroutine),
            "changes with an exception",
            "beyond a skipped issue link".to_string(),
        ),
        (
            s.protected_edits.len().to_string(),
            "protected-path edits",
            "ratification not checked".to_string(),
        ),
        (
            not_checked.to_string(),
            "questions not checked",
            "see the strip above".to_string(),
        ),
    ];
    figs.iter()
        .map(|(v, k, d)| {
            format!(r##"<div class="fig"><b>{v}</b><span>{k}</span><small>{d}</small></div>"##)
        })
        .collect()
}

/// The protected-paths view: one row per change that edited a protected path.
fn protected_view_html(s: &Summary) -> String {
    let mut by_change: Vec<(&Record, Vec<&Record>)> = Vec::new();
    for r in &s.protected_edits {
        match by_change.iter_mut().find(|(f, _)| f.sha == r.sha) {
            Some((_, v)) => v.push(r),
            None => by_change.push((r, vec![r])),
        }
    }
    let prot_rows: String = by_change
        .iter()
        .map(|(f, rs)| {
            let paths = rs.iter().map(|r| format!("<code>{}</code>{}", esc(r.file.as_deref().unwrap_or("")), source(r, s))).collect::<Vec<_>>().join("<br>");
            let gate = f.detail.as_deref().unwrap_or("");
            let gate_badge = if gate == "gate on" { r##"<span class="badge ok">on</span>"## } else { r##"<span class="badge muted">off</span>"## };
            format!(r##"<tr><td>{}</td><td>{}</td><td>{paths}</td><td>{gate_badge}</td><td>{}</td><td>{}</td><td>{}</td></tr>"##, ch(f), date(f.time), ratification_cell(rs, s), review_cell(f, s), external(f, s))
        })
        .collect();
    if by_change.is_empty() {
        r##"<p class="muted">No change edited a path protected by <code>ratified-paths</code> in its parent configuration.</p>"##.to_string()
    } else {
        table(
            &[
                "Change",
                "Date",
                "Paths",
                "Gate",
                "Ratification",
                "Approved by another login",
                "Where to check",
            ],
            &prot_rows,
        )
    }
}

/// The configuration view's table of loosenings.
fn loosenings_table(s: &Summary, loosenings: &[&Record]) -> String {
    let cfg_rows: String = loosenings
        .iter()
        .map(|r| {
            let val = match r.count {
                Some(c) => format!("{} {c}", change_str(r)),
                None => format!("{}: <code>{}</code> → <code>{}</code>", change_str(r), esc(r.before.as_deref().unwrap_or("")), esc(r.after.as_deref().unwrap_or(""))),
            };
            let mut flags = Vec::new();
            if r.gate.as_deref().is_some_and(|g| GUARD_GATES.contains(&g)) {
                flags.push(r##"<span class="badge bad">guards the gates</span>"##);
            }
            if r.pr.is_none() {
                flags.push(r##"<span class="badge warn">no pull request</span>"##);
            }
            if r.edited {
                flags.push(r##"<span class="badge muted">edited entry</span>"##);
            }
            if waived_in_change(r, s) {
                flags.push(r##"<span class="badge muted">waived in same change</span>"##);
            }
            let restored = if r.edited {
                r##"<span class="badge muted">edited in the same change</span>"##.to_string()
            } else {
                restored_by(r, s).map(|t| format!("by {}", ch(t))).unwrap_or_else(|| r##"<span class="badge warn">open</span>"##.to_string())
            };
            let g = esc(r.gate.as_deref().unwrap_or(""));
            format!(r##"<tr><td>{}</td><td>{}</td><td><a href="#g-{g}"><code>{g}</code></a></td><td><code>{}</code> {val}{}</td><td>{}</td><td>{restored}</td></tr>"##, ch(r), date(r.time), esc(r.key.as_deref().unwrap_or("")), source(r, s), flags.join(" "))
        })
        .collect();
    if loosenings.is_empty() {
        r##"<p class="muted">No loosening.</p>"##.to_string()
    } else {
        table(
            &["Change", "Date", "Gate", "What", "Flags", "Restored"],
            &cfg_rows,
        )
    }
}

/// The configuration view's table of tightenings.
fn tightenings_table(s: &Summary) -> String {
    let tight_rows: String = s
        .tightenings
        .iter()
        .map(|t| {
            let g = esc(t.gate.as_deref().unwrap_or(""));
            let val = match t.count {
                Some(c) => format!("{c} entr(y/ies)"),
                None => format!("<code>{}</code> → <code>{}</code>", esc(t.before.as_deref().unwrap_or("")), esc(t.after.as_deref().unwrap_or(""))),
            };
            format!(r##"<tr><td>{}</td><td>{}</td><td><a href="#g-{g}"><code>{g}</code></a></td><td><code>{}</code> {val}{}</td></tr>"##, ch(t), date(t.time), esc(t.key.as_deref().unwrap_or("")), source(t, s))
        })
        .collect();
    if s.tightenings.is_empty() {
        r##"<p class="muted">No tightening.</p>"##.to_string()
    } else {
        table(&["Change", "Date", "Gate", "What"], &tight_rows)
    }
}

/// The waivers view's table of waived findings, grouped by change, with the number of
/// waivers it holds.
fn waived_findings(s: &Summary) -> (usize, String) {
    let mut det: Vec<(&Record, Vec<&Record>)> = Vec::new();
    for r in s
        .records
        .iter()
        .filter(|r| r.kind == "directive" && r.class == "detector")
    {
        match det.iter_mut().find(|(f, _)| f.sha == r.sha) {
            Some((_, v)) => v.push(r),
            None => det.push((r, vec![r])),
        }
    }
    let det_n: usize = det.iter().map(|(_, v)| v.len()).sum();
    let det_rows: String = det
        .iter()
        .map(|(f, rs)| {
            let d = rs.iter().map(|r| format!("<code>{}</code>", esc(r.directive.as_deref().unwrap_or("")))).collect::<Vec<_>>().join("<br>");
            let g = rs.iter().map(|r| {
                let g = esc(r.gate.as_deref().unwrap_or("–"));
                format!(r##"<a href="#g-{g}"><code>{g}</code></a>"##)
            }).collect::<Vec<_>>().join("<br>");
            let len = rs.iter().map(|r| r.reason_len.unwrap_or(0).to_string()).collect::<Vec<_>>().join("<br>");
            format!(r##"<tr><td>{}</td><td>{}</td><td>{d}{}</td><td>{g}</td><td>{len}</td><td class="muted">not checked</td></tr>"##, ch(f), date(f.time), source(f, s))
        })
        .collect();
    let waived = if det.is_empty() {
        r##"<p class="muted">No finding waiver.</p>"##.to_string()
    } else {
        table(
            &[
                "Change",
                "Date",
                "Directive",
                "Gate",
                "Reason length",
                "Lifted a finding",
            ],
            &det_rows,
        )
    };
    (det_n, waived)
}

/// The waivers view's table of skipped-issue-link reasons reused three times or more.
fn reused_reasons_table(process: &[&Record]) -> String {
    let mut reuse: BTreeMap<&str, Vec<&Record>> = BTreeMap::new();
    for r in process {
        if let Some(h) = r.reason_sha256.as_deref() {
            reuse.entry(h).or_default().push(r);
        }
    }
    let mut reuse: Vec<(&str, Vec<&Record>)> =
        reuse.into_iter().filter(|(_, v)| v.len() >= 3).collect();
    reuse.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    let reuse_rows: String = reuse
        .iter()
        .map(|(h, rs)| {
            let changes = rs.iter().map(|r| ch(r)).collect::<Vec<_>>().join(", ");
            format!(
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td><td>{changes}</td></tr>",
                &h[..12],
                rs.len(),
                rs[0].reason_len.unwrap_or(0)
            )
        })
        .collect();
    if reuse.is_empty() {
        r##"<p class="muted">No reason reused three times.</p>"##.to_string()
    } else {
        table(&["Reason hash", "Times", "Length", "Changes"], &reuse_rows)
    }
}

/// The waivers view's line counting inline markers by the kind of file they are in.
fn marker_roles_line(s: &Summary) -> String {
    let mut roles: BTreeMap<&str, usize> = BTreeMap::new();
    for r in s.records.iter().filter(|r| r.kind == "inline-marker") {
        *roles.entry(role(r.file.as_deref())).or_default() += 1;
    }
    let mut roles_line = roles
        .iter()
        .map(|(k, v)| format!("{v} {k}"))
        .collect::<Vec<_>>()
        .join(", ");
    let judged: Vec<&Record> = s
        .records
        .iter()
        .filter(|r| r.kind == "inline-marker" && r.lifted.is_some())
        .collect();
    if !judged.is_empty() {
        let nothing = judged.iter().filter(|r| r.lifted == Some(false)).count();
        roles_line.push_str(&format!(
            "; in the replay, {} lifted a finding and {nothing} lifted nothing (left out below)",
            judged.len() - nothing
        ));
    }
    if roles_line.is_empty() {
        "none".to_string()
    } else {
        roles_line
    }
}

/// The waivers view's table of inline markers outside tests and docs.
fn markers_table(s: &Summary) -> String {
    let marker_rows: String = s
        .records
        .iter()
        .filter(|r| {
            r.kind == "inline-marker"
                && role(r.file.as_deref()) != "test or doc"
                && r.lifted != Some(false)
        })
        .map(|r| {
            let g = esc(r.gate.as_deref().unwrap_or(""));
            format!(r##"<tr><td>{}</td><td><a href="#g-{g}"><code>{g}</code></a></td><td>{}</td><td>{}</td></tr>"##, ch(r), source(r, s), role(r.file.as_deref()))
        })
        .collect();
    if marker_rows.is_empty() {
        r##"<p class="muted">None.</p>"##.to_string()
    } else {
        table(&["Change", "Gate", "Where", "Kind of file"], &marker_rows)
    }
}

/// The changes that carry a record, one entry each, in the order the changes view lists
/// them.
fn changes_in_order(s: &Summary) -> Vec<&Record> {
    let mut order: Vec<&Record> = Vec::new();
    for r in s
        .records
        .iter()
        .chain(&s.tightenings)
        .chain(&s.protected_edits)
    {
        if !order.iter().any(|x| x.sha == r.sha) {
            order.push(r);
        }
    }
    order.sort_by_key(|r| r.ord);
    order
}

/// The changes view: everything each change did.
fn changes_list(s: &Summary, order: &[&Record]) -> String {
    let mut changes_html = String::new();
    for f in order {
        let mut facts = String::new();
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for r in s
            .records
            .iter()
            .chain(&s.tightenings)
            .chain(&s.protected_edits)
            .filter(|r| r.sha == f.sha)
        {
            *counts.entry(r.class).or_default() += 1;
            let item = change_fact(r, s);
            facts.push_str(&format!("<li>{}{item}{}</li>", dot(r.class), source(r, s)));
        }
        let summary = counts
            .iter()
            .map(|(k, v)| format!("{v} {k}"))
            .collect::<Vec<_>>()
            .join(" · ");
        if let Some(p) = s.pulls.iter().find(|p| p.sha == f.sha) {
            facts.push_str(&format!(
                "<li>{}Merged through pull request #{}: {}</li>",
                dot("process"),
                p.pr,
                if p.approved_by_other {
                    r##"<span class="badge ok">approved by another login</span>"##
                } else {
                    r##"<span class="badge warn">no approval from another login</span>"##
                }
            ));
        }
        changes_html.push_str(&format!(r##"<details id="{}"><summary><span class="lbl">{}</span><span class="d">{}</span><span class="s">{}</span><span class="cnt">{summary}</span></summary><ul class="facts">{facts}</ul><p class="gh">{} <code>{}</code></p></details>"##,
            anchor(f),
            esc(&label(f)),
            date(f.time),
            esc(&f.subject),
            external(f, s),
            &f.sha[..f.sha.len().min(10)]));
    }
    changes_html
}

/// One record as the changes view states it.
fn change_fact(r: &Record, s: &Summary) -> String {
    let g = esc(r.gate.as_deref().unwrap_or(""));
    match r.kind {
        "directive" => format!(
            "Directive <code>{}</code> in the {} ({}), reason {} chars{}{}{}",
            esc(r.directive.as_deref().unwrap_or("")),
            if r.source == Some("pull-request-body") {
                "pull request body"
            } else {
                "commit message"
            },
            if g.is_empty() {
                "no gate".to_string()
            } else {
                g.clone()
            },
            r.reason_len.unwrap_or(0),
            if r.hidden == Some(true) {
                r##" <span class="badge warn">hidden</span>"##
            } else {
                ""
            },
            if r.cites.is_empty() {
                String::new()
            } else {
                format!("; cites {}", cites_cell(r, s))
            },
            lifted_badge(r)
        ),
        "config" => format!(
            "Loosened <code>{g}.{}</code>: {} {}{}",
            esc(r.key.as_deref().unwrap_or("")),
            change_str(r),
            r.count.map(|c| c.to_string()).unwrap_or_default(),
            set_aside(r)
        ),
        "config-tightening" => format!(
            "Tightened <code>{g}.{}</code>{}",
            esc(r.key.as_deref().unwrap_or("")),
            set_aside(r)
        ),
        "config-unreadable" => format!(
            "Configuration unreadable: {}",
            esc(r.detail.as_deref().unwrap_or(""))
        ),
        "baseline" => format!(
            "{} <code>{g}</code> findings grandfathered",
            r.count.unwrap_or(0)
        ),
        "inline-marker" => format!(
            "Inline marker for <code>{g}</code> at <code>{}:{}</code> ({}){}",
            esc(r.file.as_deref().unwrap_or("")),
            r.line.unwrap_or(0),
            role(r.file.as_deref()),
            lifted_badge(r)
        ),
        "protected-edit" if r.ratification.is_some() => format!(
            "Edited protected path <code>{}</code>: {}",
            esc(r.file.as_deref().unwrap_or("")),
            ratification_cell(&[r], s)
        ),
        "protected-edit" => format!(
            "Edited protected path <code>{}</code> (gate {}); ratification not checked",
            esc(r.file.as_deref().unwrap_or("")),
            if r.detail.as_deref() == Some("gate on") {
                "on"
            } else {
                "off"
            }
        ),
        _ => own(r.kind),
    }
}

/// The gates view: each gate's records as a timeline.
fn gates_list(s: &Summary) -> String {
    let mut gates: BTreeSet<&str> = BTreeSet::new();
    for r in s
        .records
        .iter()
        .chain(&s.tightenings)
        .chain(&s.protected_edits)
    {
        if let Some(g) = r.gate.as_deref() {
            gates.insert(g);
        }
    }
    let mut gates_html = String::new();
    for g in &gates {
        let mut rs: Vec<&Record> = s
            .records
            .iter()
            .chain(&s.tightenings)
            .chain(&s.protected_edits)
            .filter(|r| r.gate.as_deref() == Some(g))
            .collect();
        rs.sort_by_key(|r| std::cmp::Reverse(r.ord));
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        let mut tl = String::new();
        for r in &rs {
            *counts.entry(r.kind).or_default() += 1;
            let what = match r.kind {
                "directive" => format!(
                    "waived by <code>{}</code>",
                    esc(r.directive.as_deref().unwrap_or(""))
                ),
                "config" => format!(
                    "<b>loosened</b> <code>{}</code> {}",
                    esc(r.key.as_deref().unwrap_or("")),
                    change_str(r)
                ),
                "config-tightening" => format!(
                    "tightened <code>{}</code>",
                    esc(r.key.as_deref().unwrap_or(""))
                ),
                "inline-marker" => format!(
                    "marker in <code>{}</code>",
                    esc(r.file.as_deref().unwrap_or(""))
                ),
                "baseline" => "baseline grew".to_string(),
                "protected-edit" => format!(
                    "protected path <code>{}</code> edited",
                    esc(r.file.as_deref().unwrap_or(""))
                ),
                other => own(other),
            };
            tl.push_str(&format!(
                "<li>{}{} {} {what}{}</li>",
                dot(r.class),
                date(r.time),
                ch(r),
                source(r, s)
            ));
        }
        let badge = if GUARD_GATES.contains(g) {
            r##"<span class="badge bad">guards the gates</span>"##
        } else {
            ""
        };
        let summary = counts
            .iter()
            .map(|(k, v)| format!("{v} {k}"))
            .collect::<Vec<_>>()
            .join(" · ");
        gates_html.push_str(&format!(r##"<details id="g-{g}"><summary><code class="lbl">{g}</code>{badge}<span class="cnt">{summary}</span></summary><ol class="timeline">{tl}</ol></details>"##,
            g = esc(g)));
    }
    gates_html
}

/// The table of all records.
fn records_table(s: &Summary) -> String {
    let class_label = |c: &str| match c {
        "detector" => "Waived finding",
        "config" => "Config loosening",
        "baseline" => "Baseline growth",
        "inline" => "Inline marker",
        "process" => "Skipped issue link",
        _ => "Other",
    };
    let rec_rows: String = s
        .records
        .iter()
        .map(|r| {
            let what = match r.kind {
                "directive" => format!("<code>{}</code>", esc(r.directive.as_deref().unwrap_or(""))),
                "config" => format!("<code>{}</code> {}", esc(r.key.as_deref().unwrap_or("")), change_str(r)),
                "inline-marker" => format!("<code>{}:{}</code>", esc(r.file.as_deref().unwrap_or("")), r.line.unwrap_or(0)),
                "baseline" => format!("{} added", r.count.unwrap_or(0)),
                _ => esc(r.detail.as_deref().unwrap_or("")),
            };
            let what = format!("{what}{}", source(r, s));
            let status = if r.evidence == "claimed" { "Requested" } else { "In effect" };
            let source = if r.tier == "C" { "Author's text" } else { "Repository" };
            format!(r##"<tr data-class="{}" data-gate="{}"><td>{}</td><td>{}</td><td>{}{}</td><td><code>{}</code></td><td>{what}</td><td>{status}</td><td>{source}</td></tr>"##, own(r.class), esc(r.gate.as_deref().unwrap_or("")), ch(r), date(r.time), dot(r.class), class_label(r.class), esc(r.gate.as_deref().unwrap_or("–")))
        })
        .collect();
    table(
        &["Change", "Date", "Kind", "Gate", "What", "Status", "Source"],
        &rec_rows,
    )
}

/// The gate filter's options: every gate a record names.
fn gate_options(s: &Summary) -> String {
    s.records
        .iter()
        .filter_map(|r| r.gate.as_deref())
        .collect::<BTreeSet<_>>()
        .iter()
        .map(|g| format!(r##"<option value="{0}">{0}</option>"##, esc(g)))
        .collect()
}

/// Render the audit as one self-contained HTML page.
pub fn render(s: &Summary) -> String {
    let n = s.changes;
    let span = record_span(s);
    let repo_name = repository_name(s);

    // --- lede ---
    let lede = lede_text(s);
    let top = start_here(s);
    let nonroutine = nonroutine_changes(s);
    let sublede = sublede_text(s, nonroutine);

    // --- trust panel and checks ---
    let links_line = forge_links_line(s);
    let checks = checks_list(s);

    // --- decisions ---
    let decisions = decisions_list(s);

    // --- figures ---
    let loosenings: Vec<&Record> = s.records.iter().filter(|r| r.kind == "config").collect();
    let figs = figures(s, &loosenings, nonroutine);

    // --- protected view ---
    let protected_view = protected_view_html(s);

    // --- waivers view ---
    let (det_n, det) = waived_findings(s);
    let process: Vec<&Record> = s.records.iter().filter(|r| r.class == "process").collect();

    // --- changes view ---
    let order = changes_in_order(s);
    let changes_html = changes_list(s, &order);

    // --- gates view ---
    let gates_html = gates_list(s);

    // --- records view ---
    let gate_opts = gate_options(s);

    let mut o = String::new();
    o.push_str(&format!(r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta name="generator" content="discipline {version}">
<title>Escape-Hatch Audit · {repo}</title>
<style>
{STYLE}</style>
</head>
<body>
<div class="wrap">
  <header>
    <div class="eyebrow">discipline audit · {repo} · {n} merged changes{span}</div>
    <h1>{lede}</h1>
    {top}
    <p class="sub">{sublede}</p>
    <div class="trust" aria-label="What this report rests on">
      <dl>
        <dt>Audited</dt><dd><code>{reference}</code> at <code>{tip}</code></dd>
        <dt>Links</dt><dd>{links_line}</dd>
        <dt>Read</dt><dd>{read_line}</dd>
        <dt>Written by</dt><dd>discipline {version}</dd>
      </dl>
      <p class="warnline">A record is a prompt to look, not a finding of wrongdoing. What git cannot tell is listed as not checked below; it is not a clean result.</p>
    </div>
    <ul class="checks" aria-label="What was checked">{checks}</ul>
  </header>

  <nav class="tabs" aria-label="Report views">
    <a href="#overview" data-view="overview">Overview</a>
    <a href="#protected" data-view="protected">Protected paths <span class="n">{prot_n}</span></a>
    <a href="#configuration" data-view="configuration">Configuration</a>
    <a href="#waivers" data-view="waivers">Waivers</a>
    <a href="#changes" data-view="changes">Changes <span class="n">{change_n}</span></a>
    <a href="#gates" data-view="gates">Gates</a>
    <a href="#records" data-view="records">All records <span class="n">{rec_n}</span></a>
  </nav>

  <section class="view" id="overview">
    <div class="viewhead"><h2>Needs a decision</h2><p>Ranked by what the exception could hide. Each item is a prompt to look, not a finding of wrongdoing.</p></div>
    <ol class="decisions">{decisions}</ol>
    <div class="figures" aria-label="Figures">{figs}</div>
    <div class="panel">
      <h3>Changes with an exception, over time</h3>
      <p>Share of merged changes, per window, that carried anything beyond a skipped issue link. Red lines mark configuration loosenings; a solid one loosened a gate that guards the other gates.</p>
      <div class="chartbox">{rate}</div>
    </div>
    <div class="panel">
      <h3>Where the exceptions are</h3>
      <ul class="legend"><li>{dd}Waived finding (directive)</li><li>{di}Inline marker outside tests and docs</li></ul>
      <p>Markers in tests and docs are left out. Gate names in red guard the other gates. Select a gate to see its history.</p>
      <div class="chartbox">{gate}</div>
    </div>
  </section>

  <section class="view" id="protected">
    <div class="viewhead"><h2>Protected paths</h2><p>Changes that edited a path the parent configuration protects under <code>ratified-paths</code>. Whether each was ratified, and by whom, lives on the forge and is not checked here.</p></div>
    <div class="panel">{protected_view}<p><b>Next:</b> open each change and confirm its ratification came from someone other than the change's author.</p></div>
  </section>

  <section class="view" id="configuration">
    <div class="viewhead"><h2>Configuration</h2><p>Every loosening of <code>discipline.toml</code>, as <code>config-integrity</code> judges it, newest first, with the change that restored it.</p></div>
    <div class="panel"><h3>Loosenings</h3>{cfg}<p><b>Next:</b> tighten each open loosening back, or record why it stays.</p></div>
    <div class="panel"><h3>Tightenings</h3>{tight}</div>
  </section>

  <section class="view" id="waivers">
    <div class="viewhead"><h2>Waivers</h2><p>Directives in commit messages, grouped by change. Whether each lifted a finding is <code>discipline replay</code>'s to say.</p></div>
    <div class="panel"><h3>Waived findings <span class="muted">({det_n})</span></h3>{det}</div>
    <div class="panel"><h3>Skipped issue links <span class="muted">({proc_n})</span></h3><p>Reasons are hashed. These were reused three times or more, which suggests boilerplate.</p>{reuse}</div>
    <div class="panel"><h3>Inline markers outside tests and docs</h3><p>All markers by where they are: {roles}.</p>{markers}</div>
  </section>

  <section class="view" id="changes">
    <div class="viewhead"><h2>Changes</h2><p>Everything each change did, newest first. Only changes with a record are listed.</p></div>
    <div>{changes_html}</div>
  </section>

  <section class="view" id="gates">
    <div class="viewhead"><h2>Gates</h2><p>Each gate's history, oldest first.</p></div>
    <div>{gates_html}</div>
  </section>

  <section class="view" id="records">
    <div class="viewhead"><h2>All records</h2><p>"Author's text" rows could have been worded to pass; "Repository" rows are changes to files on the audited branch.</p></div>
    <div class="panel">
      <div class="filters">
        <label for="f-class">Kind
          <select id="f-class">
            <option value="">Everything</option>
            <option value="nonroutine" selected>Everything but skipped issue links</option>
            <option value="detector">Waived findings</option>
            <option value="config">Config loosenings</option>
            <option value="inline">Inline markers</option>
            <option value="baseline">Baseline growth</option>
            <option value="process">Skipped issue links</option>
          </select>
        </label>
        <label for="f-gate">Gate <select id="f-gate"><option value="">Any gate</option>{gate_opts}</select></label>
        <label for="f-text">Search <input id="f-text" type="search" placeholder="#12, ci.yml, allow-stub"></label>
      </div>
      <div class="count" id="count" aria-live="polite"></div>
      {records}
    </div>
  </section>
</div>
<script>
{SCRIPT}</script>
</body>
</html>
"##,
        version = own(&s.version),
        read_line = read_line(s),
        repo = esc(&repo_name),
        reference = esc(&s.reference),
        tip = own(&s.tip[..s.tip.len().min(10)]),
        prot_n = s.protected_edits.len(),
        change_n = order.len(),
        rec_n = s.records.len(),
        rate = rate_chart(s),
        gate = gate_chart(s),
        dd = dot("detector"),
        di = dot("inline"),
        cfg = loosenings_table(s, &loosenings),
        tight = tightenings_table(s),
        proc_n = process.len(),
        reuse = reused_reasons_table(&process),
        roles = marker_roles_line(s),
        markers = markers_table(s),
        records = records_table(s),));
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_match_the_civil_calendar() {
        // Reference values from `date -u -r <secs> +%F` (BSD) and Python's `datetime`.
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(1_790_710_369), "2026-09-29");
        assert_eq!(date(-86_400), "1969-12-31");
    }

    #[test]
    fn author_text_is_escaped() {
        assert_eq!(
            esc(r##"<script>"a" & 'b'</script>"##),
            "&lt;script&gt;&quot;a&quot; &amp; &#39;b&#39;&lt;/script&gt;"
        );
    }

    #[test]
    fn files_are_sorted_by_what_a_reviewer_should_read() {
        assert_eq!(role(Some(".github/workflows/ci.yml")), "workflow");
        assert_eq!(role(Some("AGENTS.md")), "agent instructions");
        assert_eq!(role(Some("tests/test_x.rs")), "test or doc");
        assert_eq!(role(Some("docs/GATES.md")), "test or doc");
        assert_eq!(role(Some("src/guards/pii.rs")), "live code");
    }

    #[test]
    fn a_closed_cited_issue_asks_for_a_check_not_a_verdict() {
        // A waiver may cite a closed issue as its approval ("owner-approved via #315"),
        // not as a follow-up, so the sentence must not say the follow-up is untracked.
        let signal = |n| Signal {
            id: "waiver-cites-issue-closed-before",
            rank: "look-soon",
            count: n,
            changes: vec!["#331".into()],
            records: vec![],
            list: "records",
            next: "",
        };
        let summary = Summary::default();
        assert_eq!(
            signal_sentence(&signal(1), &summary),
            "1 waiver points to an issue that was already closed when it was written; \
             check that the issue still supports the waiver"
        );
        assert!(signal_sentence(&signal(2), &summary).starts_with("2 waivers point to"));
    }
}
