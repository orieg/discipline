//! The rule engine: how a key path moves the bar ([`Judge`]), and what one tree loosens
//! relative to another under a rule table ([`diff_trees`]).

use serde_json::Value;

/// How a key path moves the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Judge {
    /// List: a gained entry loosens; so does the list appearing where there was none.
    Grown,
    /// List: a lost entry loosens; so does the list disappearing. Appearing does not.
    Shrunk,
    /// Number: a smaller value loosens; so does removal.
    Floor,
    /// Number: a larger value loosens; appearing counts as rising from zero.
    Cap,
    /// Boolean: `true` loosens; appearing as `true` counts.
    LooserWhenTrue,
    /// Boolean: `false` loosens; `true` disappearing counts.
    LooserWhenFalse,
    /// Lint level (`off`/`allow` < `warn` < `error`/`deny`/`forbid`): lowering loosens.
    LintLevel,
    /// Command-line flags (string or list): losing a strict flag or gaining a lax one
    /// loosens.
    Flags,
    /// A mode, ordered strict to loose, and the rank an absent key takes: moving to a later
    /// rank loosens. A value outside the order is not judged.
    Ranked(&'static [&'static str], usize),
    /// List, string of flags, or map keys: an entry containing one of these fragments that
    /// the base side did not have loosens. `--flag value` is read as `--flag=value`.
    GainedMatching(&'static [&'static str]),
}

/// One rule: files it applies to (basename or `dir/basename` glob), the key path inside
/// the tree (`*` matches one segment), and the direction.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub files: &'static [&'static str],
    pub path: &'static str,
    pub judge: Judge,
}

/// Flags whose loss loosens the run.
const STRICT_FLAGS: &[&str] = &[
    "-Dwarnings",
    "-D",
    "--deny",
    "-F",
    "--forbid",
    "-Werror",
    "--strict-markers",
    "--strict-config",
    "--strict",
    "-W",
    "--locked",
    "--frozen",
    "--frozen-lockfile",
    "--require-hashes",
    "--cov-fail-under",
    "-Wall",
    "-Wextra",
    "-Wpedantic",
];

/// Flags whose gain loosens the run.
const LAX_FLAGS: &[&str] = &[
    "--reruns",
    "--reruns-delay",
    "--ignore",
    "--ignore-glob",
    "--deselect",
    "--no-cov",
    "--continue-on-collection-errors",
    "--runxfail",
    "-Awarnings",
    "-A",
    "--allow",
    "--cap-lints",
    "-w",
    "-Wno-error",
    "--no-verify",
    "--no-strict",
];

/// Prefixes of flags whose gain loosens the run: `-Wno-<diagnostic>` and
/// `-Wno-error[=<diagnostic>]` switch a compiler warning, or its promotion to an error, off.
pub const LAX_FLAG_PREFIXES: &[&str] = &["-Wno-"];

/// Whether one pattern segment (`*`, `mypy-*`, or a literal) matches a key. A key may
/// itself contain dots (`mypy-thirdparty.*`): keys are never split.
pub(super) fn segment_matches(pattern: &str, key: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == key;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !key.starts_with(first) || !key.ends_with(last) || key.len() < first.len() + last.len() {
        return false;
    }
    let mut rest = &key[first.len()..key.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

/// All (path, value) pairs in `tree` matching a dotted pattern where `*` is one segment.
fn matching<'a>(tree: &'a Value, pattern: &str) -> Vec<(String, &'a Value)> {
    fn walk<'a>(
        v: &'a Value,
        segs: &[&str],
        prefix: Vec<String>,
        out: &mut Vec<(String, &'a Value)>,
    ) {
        let Some((first, rest)) = segs.split_first() else {
            out.push((prefix.join("."), v));
            return;
        };
        let Some(map) = v.as_object() else { return };
        for (k, child) in map {
            if segment_matches(first, k) {
                let mut p = prefix.clone();
                p.push(k.clone());
                walk(child, rest, p, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(
        tree,
        &pattern.split('.').collect::<Vec<_>>(),
        Vec::new(),
        &mut out,
    );
    out
}

fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Some(true),
            "false" | "no" | "off" | "0" => Some(false),
            _ => None,
        },
        Value::Number(n) => n.as_i64().map(|i| i != 0),
        _ => None,
    }
}

fn as_num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().trim_end_matches('%').parse().ok(),
        _ => None,
    }
}

/// Entries of a list value; a scalar is a one-entry list, a map is its keys.
fn as_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect(),
        Value::Null => Vec::new(),
        Value::Object(m) => m.keys().cloned().collect(),
        Value::String(s) => s
            .split([',', '\n'])
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        other => vec![other.to_string()],
    }
}

fn lint_level(v: &Value) -> Option<u8> {
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => return a.first().and_then(lint_level),
        Value::Object(m) => return m.get("level").and_then(lint_level),
        _ => return None,
    };
    match s.trim().to_ascii_lowercase().as_str() {
        "off" | "0" | "allow" => Some(0),
        "warn" | "warning" | "1" => Some(1),
        "error" | "2" | "deny" | "forbid" => Some(2),
        _ => None,
    }
}

fn flag_tokens(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => s.split_whitespace().map(str::to_string).collect(),
        Value::Array(a) => a.iter().flat_map(flag_tokens).collect(),
        _ => Vec::new(),
    }
}

/// A scalar's text (`true`, `3`, `host`); `None` for a list or map.
fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Bool(_) | Value::Number(_) => Some(v.to_string()),
        _ => None,
    }
}

/// Entries of a list, map keys, or a flag string, with `--flag value` joined to
/// `--flag=value`; a map inside a list (a long-form mount) is its values joined by `,`.
fn entry_tokens(v: &Value) -> Vec<String> {
    let raw: Vec<String> = match v {
        Value::String(s) => s.split_whitespace().map(str::to_string).collect(),
        Value::Array(a) => a
            .iter()
            .flat_map(|x| match x {
                Value::String(s) => s.split_whitespace().map(str::to_string).collect(),
                Value::Object(m) => vec![m
                    .iter()
                    .map(|(k, v)| format!("{k}={}", scalar(v).unwrap_or_default()))
                    .collect::<Vec<_>>()
                    .join(",")],
                other => vec![other.to_string()],
            })
            .collect(),
        Value::Object(m) => m.keys().cloned().collect(),
        Value::Null => Vec::new(),
        other => vec![other.to_string()],
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let t = &raw[i];
        if t.starts_with("--")
            && !t.contains('=')
            && i + 1 < raw.len()
            && !raw[i + 1].starts_with('-')
        {
            out.push(format!("{t}={}", raw[i + 1]));
            i += 2;
            continue;
        }
        out.push(t.clone());
        i += 1;
    }
    out
}

/// The strict-vocabulary flags a token list carries, joined with their argument
/// (`-D warnings`, `--cov-fail-under 80`) so a rename is not a loss.
fn strict_flags(tokens: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let t = &tokens[i];
        if STRICT_FLAGS.contains(&t.as_str()) {
            let takes_arg = matches!(
                t.as_str(),
                "-D" | "--deny" | "-F" | "--forbid" | "-W" | "--cov-fail-under"
            );
            if takes_arg && i + 1 < tokens.len() {
                out.push(format!("{t} {}", tokens[i + 1]));
                i += 2;
                continue;
            }
            out.push(t.clone());
        } else if let Some(rest) = t.strip_prefix("--cov-fail-under=") {
            out.push(format!("--cov-fail-under {rest}"));
        }
        i += 1;
    }
    out
}

fn lax_flags(tokens: &[String]) -> Vec<String> {
    tokens
        .iter()
        .filter(|t| {
            let bare = t.split_once('=').map_or(t.as_str(), |(k, _)| k);
            LAX_FLAGS.contains(&bare) || LAX_FLAG_PREFIXES.iter().any(|p| t.starts_with(p))
        })
        .cloned()
        .collect()
}

/// One weakening in a configuration file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Weakening {
    /// Key path, e.g. `compilerOptions.strict`.
    pub key: String,
    pub what: String,
}

/// `(key, what was gained)` for every key path of `table` that applies to the file `name`
/// and whose head side names an entry the base side did not. `table` pairs file names with
/// a key path.
pub fn gained_entries(
    table: &[(&[&str], &str)],
    name: &str,
    base: &Value,
    head: &Value,
) -> Vec<(String, String)> {
    let applies = |files: &[&str]| {
        files.iter().any(|f| {
            if let Some(prefix) = f.strip_suffix(".*.json") {
                name.starts_with(&format!("{prefix}.")) && name.ends_with(".json")
            } else {
                name == *f || name.ends_with(&format!("/{f}"))
            }
        })
    };
    let mut out = Vec::new();
    for (files, path) in table {
        if !applies(files) {
            continue;
        }
        let b_paths: std::collections::BTreeMap<String, &Value> =
            matching(base, path).into_iter().collect();
        for (key, h) in matching(head, path) {
            let bl = b_paths.get(&key).map(|v| as_list(v)).unwrap_or_default();
            let hl = as_list(h);
            let gained: Vec<&String> = hl.iter().filter(|x| !bl.contains(x)).collect();
            if !gained.is_empty() {
                out.push((key, sample(&gained)));
            }
        }
    }
    out
}

/// What `head` loosens relative to `base` under `rules`.
pub fn diff_trees(base: &Value, head: &Value, rules: &[&Rule]) -> Vec<Weakening> {
    let mut found = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for rule in rules {
        let b_paths: std::collections::BTreeMap<String, &Value> =
            matching(base, rule.path).into_iter().collect();
        let h_paths: std::collections::BTreeMap<String, &Value> =
            matching(head, rule.path).into_iter().collect();
        let keys: std::collections::BTreeSet<&String> =
            b_paths.keys().chain(h_paths.keys()).collect();
        for key in keys {
            // Two patterns can name the same key (`coverageThreshold.global.*` and
            // `coverageThreshold.*.*`); judge it once.
            if !seen.insert(key.clone()) {
                continue;
            }
            let b = b_paths.get(key).copied();
            let h = h_paths.get(key).copied();
            let what = match rule.judge {
                Judge::Grown => {
                    let bl = b.map(as_list).unwrap_or_default();
                    let hl = h.map(as_list).unwrap_or_default();
                    let gained: Vec<_> = hl.iter().filter(|x| !bl.contains(x)).collect();
                    (!gained.is_empty()).then(|| {
                        format!("gained {} entr(y/ies): {}", gained.len(), sample(&gained))
                    })
                }
                Judge::Shrunk => {
                    let Some(b) = b else { continue };
                    let bl = as_list(b);
                    let hl = h.map(as_list).unwrap_or_default();
                    let lost: Vec<_> = bl.iter().filter(|x| !hl.contains(x)).collect();
                    (!lost.is_empty()).then(|| {
                        if h.is_none() {
                            format!("removed (was {} entr(y/ies))", bl.len())
                        } else {
                            format!("lost {} entr(y/ies): {}", lost.len(), sample(&lost))
                        }
                    })
                }
                Judge::Floor => match (b.and_then(as_num), h.and_then(as_num)) {
                    (Some(bn), Some(hn)) if hn < bn => Some(format!("lowered from {bn} to {hn}")),
                    (Some(bn), None) if b.is_some() && h.is_none() => {
                        Some(format!("removed (was {bn})"))
                    }
                    _ => None,
                },
                Judge::Cap => {
                    let bn = b.and_then(as_num).unwrap_or(0.0);
                    match h.and_then(as_num) {
                        Some(hn) if hn > bn => Some(format!("raised from {bn} to {hn}")),
                        _ => None,
                    }
                }
                Judge::LooserWhenTrue => match (b.and_then(as_bool), h.and_then(as_bool)) {
                    (Some(true), _) => None,
                    (_, Some(true)) => Some("switched on".to_string()),
                    _ => None,
                },
                Judge::LooserWhenFalse => match (b.and_then(as_bool), h.and_then(as_bool)) {
                    (Some(true), Some(false)) => Some("switched off".to_string()),
                    (Some(true), None) if h.is_none() => Some("removed (was on)".to_string()),
                    _ => None,
                },
                Judge::LintLevel => match (b.and_then(lint_level), h.and_then(lint_level)) {
                    (Some(bl), Some(hl)) if hl < bl => Some(format!(
                        "lowered from {} to {}",
                        level_name(bl),
                        level_name(hl)
                    )),
                    _ => None,
                },
                Judge::Flags => {
                    let bt = b.map(flag_tokens).unwrap_or_default();
                    let ht = h.map(flag_tokens).unwrap_or_default();
                    let (bs, hs) = (strict_flags(&bt), strict_flags(&ht));
                    let lost: Vec<_> = bs.iter().filter(|f| !hs.contains(f)).collect();
                    let (bl, hl) = (lax_flags(&bt), lax_flags(&ht));
                    let gained: Vec<_> = hl.iter().filter(|f| !bl.contains(f)).collect();
                    let mut parts = Vec::new();
                    if !lost.is_empty() {
                        parts.push(format!(
                            "lost `{}`",
                            lost.iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join("`, `")
                        ));
                    }
                    if !gained.is_empty() {
                        parts.push(format!(
                            "gained `{}`",
                            gained
                                .iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join("`, `")
                        ));
                    }
                    (!parts.is_empty()).then(|| parts.join("; "))
                }
                Judge::Ranked(order, default) => {
                    let rank = |v: Option<&Value>| match v {
                        None | Some(Value::Null) => Some(default),
                        Some(v) => {
                            let s = scalar(v)?;
                            order.iter().position(|o| {
                                o.eq_ignore_ascii_case(&s) || (o.ends_with(':') && s.starts_with(o))
                            })
                        }
                    };
                    let shown = |v: Option<&Value>, r: usize| {
                        v.and_then(scalar)
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| match order[r] {
                                "" => "unset".to_string(),
                                o => o.to_string(),
                            })
                    };
                    match (rank(b), rank(h)) {
                        (Some(br), Some(hr)) if hr > br => Some(match (b, h) {
                            (_, None) => format!(
                                "removed (was `{}`; unset means `{}`)",
                                shown(b, br),
                                shown(None, hr)
                            ),
                            (None, _) => format!(
                                "set to `{}` (unset means `{}`)",
                                shown(h, hr),
                                shown(None, br)
                            ),
                            _ => format!("moved from `{}` to `{}`", shown(b, br), shown(h, hr)),
                        }),
                        _ => None,
                    }
                }
                Judge::GainedMatching(fragments) => {
                    let bt = b.map(entry_tokens).unwrap_or_default();
                    let gained: Vec<String> = h
                        .map(entry_tokens)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|t| !bt.contains(t))
                        .filter(|t| {
                            let low = t.to_ascii_lowercase();
                            fragments
                                .iter()
                                .any(|f| low.contains(&f.to_ascii_lowercase()))
                        })
                        .collect();
                    let gained: Vec<&String> = gained.iter().collect();
                    (!gained.is_empty()).then(|| format!("gained {}", sample(&gained)))
                }
            };
            if let Some(what) = what {
                found.push(Weakening {
                    key: key.clone(),
                    what,
                });
            }
        }
    }
    found
}

fn level_name(l: u8) -> &'static str {
    match l {
        0 => "off",
        1 => "warn",
        _ => "error",
    }
}

fn sample(items: &[&String]) -> String {
    let shown: Vec<&str> = items.iter().take(3).map(|s| s.as_str()).collect();
    format!(
        "`{}`{}",
        shown.join("`, `"),
        if items.len() > shown.len() {
            ", ..."
        } else {
            ""
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lax_list_names_the_warning_silencers() {
        let words = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        assert_eq!(
            lax_flags(&words(
                "-w -Wno-error -Wno-error=format -Wno-unused -Wall -Werror"
            )),
            words("-w -Wno-error -Wno-error=format -Wno-unused")
        );
        // A strict flag, or a word that merely contains `-Wno-`, is not lax.
        assert!(lax_flags(&words("-Wall -Werror=format -DX-Wno-y")).is_empty());
    }
}
