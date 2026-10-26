//! `toolchain-config`: a change cannot quietly lower the compiler, linter, type-checker,
//! test-runner or coverage bar it is judged by.
//!
//! `config-integrity` guards `discipline.toml`; this gate does the same for the files that
//! configure the rest of the toolchain. Each recognised file is loaded, on the base side
//! and the head side, into one generic tree (TOML, YAML, JSON with comments and INI all
//! land in `serde_json::Value`), and a rule table says which key paths loosen in which
//! direction. A configuration written as code (`eslint.config.js`, `jest.config.ts`) has no
//! tree to compare: a change to it is reported as not analysed, never passed in silence.

mod rules;

pub use super::config_delta::{classify_with, diff_trees, load, Classified, LAX_FLAG_PREFIXES};
pub use rules::RULES;

use super::config_delta::{
    self, segment_matches, AbsentSide, LiteralTrees, ReadOrder, Rule, TreeGate, Weakening,
};
use super::{build_flags, ci_integrity, line_allows, Context, GateOutcome};
use crate::config::Severity;
use crate::tokens;
use anyhow::Result;
use rules::{EXECUTABLE, INHERITANCE, SCRIPT_RULES, SONAR};
use serde_json::Value;

/// Whether `path` is one of the rule files (by basename, or `dir/basename` for the dotted
/// directories), or an executable configuration.
pub fn classify(path: &str) -> Option<Classified> {
    classify_with(path, RULES, EXECUTABLE)
}

/// `(key, what was gained)` for every inheritance key of `name` whose head side names an
/// entry the base side did not.
pub fn inherited_changes(name: &str, base: &Value, head: &Value) -> Vec<(String, String)> {
    config_delta::gained_entries(INHERITANCE, name, base, head)
}

/// A Jest or Vitest configuration written as code, read as a data file where the object
/// both sides export is a literal ([`crate::ast::runner_config::script_config_tree`]).
/// `None`, and the change stays not analysed, when a side is not such a literal or a key
/// the rules read holds a value that is not one. A change outside the keys the rules
/// read is still reported as not analysed (`other_changes`).
pub fn literal_code(path: &str, base: &str, head: &str) -> Option<LiteralTrees> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let rules: Vec<&'static Rule> = SCRIPT_RULES
        .iter()
        .filter(|r| r.files.iter().any(|f| segment_matches(f, name)))
        .collect();
    if rules.is_empty() {
        return None;
    }
    let base = crate::ast::runner_config::script_config_tree(name, base)?;
    let head = crate::ast::runner_config::script_config_tree(name, head)?;
    let read_keys: Vec<Vec<&str>> = rules
        .iter()
        .map(|r| r.path)
        .chain(
            INHERITANCE
                .iter()
                .filter(|(files, _)| files.contains(&name))
                .map(|(_, p)| *p),
        )
        .map(|k| k.split('.').collect())
        .collect();
    // A computed value at, above or below a key a rule reads (`test` built by a spread
    // hides `test.exclude`) leaves that key unknown.
    let hidden = base.computed.iter().chain(&head.computed).any(|(at, _)| {
        read_keys
            .iter()
            .any(|key| key.iter().zip(at).all(|(p, c)| segment_matches(p, c)))
    });
    if hidden {
        return None;
    }
    let unread = |tree: &Value| {
        let mut tree = tree.clone();
        for key in &read_keys {
            remove_matching(&mut tree, key);
        }
        tree
    };
    let other_changes = base.outside != head.outside
        || base.computed != head.computed
        || unread(&base.tree) != unread(&head.tree);
    Some(LiteralTrees {
        base: base.tree,
        head: head.tree,
        rules,
        other_changes,
    })
}

/// Removes from `tree` every key path that `pattern` matches (`*` is one segment).
fn remove_matching(tree: &mut Value, pattern: &[&str]) {
    let Some(map) = tree.as_object_mut() else {
        return;
    };
    match pattern {
        [] => {}
        [last] => map.retain(|k, _| !segment_matches(last, k)),
        [first, rest @ ..] => {
            for (k, child) in map.iter_mut() {
                if segment_matches(first, k) {
                    remove_matching(child, rest);
                }
            }
        }
    }
}

/// A build file (`Makefile`, `CMakeLists.txt`, `setup.py`, `build.rs`): compiler warning
/// flags it gained that lower the bar, and strict ones it lost
/// ([`build_flags`]). A deleted file is not judged: removing a build file is a change of
/// build system, not a lowered flag.
fn build_file(
    ctx: &Context,
    severity: Severity,
    out: &mut GateOutcome,
    file: &crate::gitctx::ChangedFile,
    kind: build_flags::BuildKind,
) -> Result<()> {
    const GATE: &str = "toolchain-config";
    let Some(head_src) = ctx.git.head_content(&file.path)? else {
        return Ok(());
    };
    let base_src = ctx.git.base_content(&file.old_path)?;
    let read = |src: &str| build_flags::extract(kind, src);
    let head = read(&head_src);
    let base = match &base_src {
        Some(src) => read(src),
        None => Ok(Vec::new()),
    };
    let (base, head) = match (base, head) {
        (Ok(base), Ok(head)) => (base, head),
        (Err(build_flags::ExtractError::GrammarAbsent), _)
        | (_, Err(build_flags::ExtractError::GrammarAbsent)) => {
            out.notes.push(format!(
                "`{}` was not read: this build of discipline lacks the grammar it needs",
                file.path
            ));
            return Ok(());
        }
        _ => {
            out.examined += 1;
            out.push(
                severity,
                &crate::findings::TOOLCHAIN_CONFIG_UNREADABLE,
                Some(&file.path),
                None,
                format!(
                    "`{}` could not be parsed on one side, so its warning flags could not be checked.",
                    file.path
                ),
                "Fix the file so it parses.",
            );
            return Ok(());
        }
    };
    out.examined += 1;
    for change in build_flags::judge(&base, &head) {
        let lift = |subject: &str| {
            ctx.find_override(
                GATE,
                &crate::findings::TOOLCHAIN_CONFIG_WEAKENED,
                tokens::ALLOW_TOOLCHAIN_WEAKENING,
                subject,
            )
        };
        let ov = lift(&change.flag)
            .or_else(|| lift(&change.context))
            .or_else(|| lift(&file.path));
        let anchored = ov.is_none();
        let (what, remedy) = if change.gained {
            (
                "gained",
                "Drop it, or justify it on its own line in the PR body or a commit message",
            )
        } else {
            (
                "lost",
                "Restore it, or justify the loss on its own line in the PR body or a commit message",
            )
        };
        out.lift_or_push(
            ov,
            ctx.overridable(severity),
            &crate::findings::TOOLCHAIN_CONFIG_WEAKENED,
            (Some(&file.path), change.line),
            format!(
                "`{}` {what} `{}` in `{}`; the compiler warning bar is lower.",
                change.context, change.flag, file.path
            ),
            &format!(
                "{remedy}: `allow-toolchain-weakening: {} <reason>`.",
                change.flag
            ),
        );
        if anchored {
            out.anchor_last(format!("{} {}", change.context, change.flag));
        }
    }
    Ok(())
}

/// Whether `path` is a GitHub, Gitea or Forgejo Actions workflow file.
fn is_workflow_path(path: &str) -> bool {
    (path.ends_with(".yml") || path.ends_with(".yaml"))
        && crate::doctor::WORKFLOW_DIRS
            .iter()
            .any(|dir| path.starts_with(&format!("{dir}/")))
}

/// Split a shell line's words into sub-commands at `&&`, `||`, `|`, and `;`.
fn split_shell_commands(words: &[String]) -> Vec<Vec<String>> {
    let mut cmds = Vec::new();
    let mut cur = Vec::new();
    for w in words {
        if matches!(w.as_str(), "&&" | "||" | "|" | ";") {
            if !cur.is_empty() {
                cmds.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if let Some(stripped) = w.strip_suffix(';') {
            if !stripped.is_empty() {
                cur.push(stripped.to_string());
            }
            if !cur.is_empty() {
                cmds.push(std::mem::take(&mut cur));
            }
            continue;
        }
        cur.push(w.clone());
    }
    if !cur.is_empty() {
        cmds.push(cur);
    }
    cmds
}

/// Whether `cmd` invokes `sonar-scanner`, `mvn ... sonar:sonar`, or `gradle ... sonar`.
fn is_sonar_scanner_cmd(cmd: &[String]) -> bool {
    let args: Vec<&str> = cmd
        .iter()
        .map(String::as_str)
        .skip_while(|w| w.contains('=') && !w.starts_with('-'))
        .collect();
    if matches!(args.first().copied(), Some("echo" | "printf") | None) {
        return false;
    }
    let non_flags: Vec<String> = args
        .iter()
        .copied()
        .filter(|w| !w.starts_with('-'))
        .map(|w| w.to_ascii_lowercase())
        .collect();
    let has_bin = |names: &[&str]| {
        non_flags.iter().any(|w| {
            let base = w.rsplit(['/', '\\']).next().unwrap_or(w.as_str());
            names.contains(&base)
        })
    };
    if has_bin(&["sonar-scanner", "sonar-scanner.bat", "sonarscanner"]) {
        return true;
    }
    if has_bin(&["mvn", "mvnw", "mvn.cmd", "mvnw.cmd"])
        && non_flags
            .iter()
            .any(|w| w == "sonar:sonar" || (w.contains("sonar") && w.ends_with(":sonar")))
    {
        return true;
    }
    if has_bin(&["gradle", "gradlew", "gradle.bat", "gradlew.bat"])
        && non_flags.iter().any(|w| {
            matches!(w.as_str(), "sonar" | "sonarqube")
                || w.ends_with(":sonar")
                || w.ends_with(":sonarqube")
        })
    {
        return true;
    }
    false
}

/// Collect `-Dsonar.<key>=<value>` and `--define[=]sonar.<key>=<value>` arguments from
/// `words` into `map`.
fn extract_sonar_defines(words: &[String], map: &mut serde_json::Map<String, Value>) {
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        let spec = if let Some(rest) = w.strip_prefix("-D") {
            if !rest.is_empty() {
                Some(rest)
            } else if i + 1 < words.len() {
                i += 1;
                Some(words[i].as_str())
            } else {
                None
            }
        } else if let Some(rest) = w.strip_prefix("--define=") {
            Some(rest)
        } else if w == "--define" && i + 1 < words.len() {
            i += 1;
            Some(words[i].as_str())
        } else {
            None
        };
        if let Some(spec) = spec {
            let (k, v) = match spec.split_once('=') {
                Some((k, v)) => (k.trim(), v.trim()),
                None => (spec.trim(), "true"),
            };
            if k.starts_with("sonar.") {
                map.insert(k.to_string(), Value::String(v.to_string()));
            }
        }
        i += 1;
    }
}

/// Whether `uses` names `SonarSource/sonarqube-scan-action` or
/// `SonarSource/sonarcloud-github-action`.
fn is_sonar_scan_action(uses: &str) -> bool {
    let action = uses
        .split('@')
        .next()
        .unwrap_or(uses)
        .trim()
        .to_ascii_lowercase();
    action == "sonarsource/sonarqube-scan-action"
        || action == "sonarsource/sonarcloud-github-action"
        || action.ends_with("/sonarqube-scan-action")
        || action.ends_with("/sonarcloud-github-action")
}

/// If `step` invokes a Sonar scanner (in `run:` or via a SonarSource scan action), returns
/// the `sonar.*` property tree extracted from its command-line arguments.
fn extract_sonar_step(step: &serde_yaml::Value) -> Option<Value> {
    let mut map = serde_json::Map::new();
    if let Some(uses) = step.get("uses").and_then(|u| u.as_str()) {
        if is_sonar_scan_action(uses) {
            if let Some(args) = step
                .get("with")
                .and_then(|w| w.get("args"))
                .and_then(|a| a.as_str())
            {
                for (_, line) in build_flags::make_lines(args) {
                    if line.trim_start().starts_with('#') {
                        continue;
                    }
                    extract_sonar_defines(&build_flags::split_words(&line, true), &mut map);
                }
            }
            return Some(Value::Object(map));
        }
    }
    if let Some(run) = step.get("run").and_then(|r| r.as_str()) {
        let mut matched = false;
        for (_, line) in build_flags::make_lines(run) {
            if line.trim_start().starts_with('#') {
                continue;
            }
            let words = build_flags::split_words(&line, true);
            for cmd in split_shell_commands(&words) {
                if is_sonar_scanner_cmd(&cmd) {
                    matched = true;
                    extract_sonar_defines(&cmd, &mut map);
                }
            }
        }
        if matched {
            return Some(Value::Object(map));
        }
    }
    None
}

#[derive(Debug)]
struct SonarStep {
    step_idx: usize,
    id: String,
    name: String,
    uses: Option<String>,
    run: Option<String>,
    props: Value,
}

fn collect_job_sonar_steps(job: &serde_yaml::Value) -> Vec<SonarStep> {
    let Some(steps) = job.get("steps").and_then(|s| s.as_sequence()) else {
        return Vec::new();
    };
    steps
        .iter()
        .enumerate()
        .filter_map(|(step_idx, step)| {
            let props = extract_sonar_step(step)?;
            Some(SonarStep {
                step_idx,
                id: step
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                name: step
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                uses: step
                    .get("uses")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                run: step.get("run").and_then(|v| v.as_str()).map(str::to_string),
                props,
            })
        })
        .collect()
}

/// Compare Sonar scanner arguments in `head_src` against `base_src`. Returns `None` when
/// neither side of the workflow has a Sonar scanner step.
pub fn diff_sonar_workflow(
    base_src: Option<&str>,
    head_src: &str,
) -> Option<Vec<(Weakening, Option<usize>)>> {
    let head_doc: serde_yaml::Value = serde_yaml::from_str(head_src).ok()?;
    let base_doc: Option<serde_yaml::Value> = base_src.and_then(|b| serde_yaml::from_str(b).ok());
    let head_jobs = head_doc.get("jobs").and_then(|j| j.as_mapping())?;
    let base_jobs = base_doc
        .as_ref()
        .and_then(|b| b.get("jobs"))
        .and_then(|j| j.as_mapping());

    let sonar_rules: Vec<&Rule> = RULES.iter().filter(|r| r.files == SONAR).collect();
    let mut any_sonar_step = base_jobs.is_some_and(|m| {
        m.values()
            .any(|job| !collect_job_sonar_steps(job).is_empty())
    });
    let mut findings = Vec::new();

    for (job_k, head_job) in head_jobs {
        let head_steps = collect_job_sonar_steps(head_job);
        if head_steps.is_empty() {
            continue;
        }
        any_sonar_step = true;
        let base_steps = base_jobs
            .and_then(|m| m.get(job_k))
            .map(collect_job_sonar_steps)
            .unwrap_or_default();

        let mut base_taken = vec![false; base_steps.len()];
        let mut paired_base: Vec<Option<usize>> = vec![None; head_steps.len()];
        // Pass 1: pair by step `id` or `name`.
        for (hi, h) in head_steps.iter().enumerate() {
            if let Some((bi, _)) = base_steps.iter().enumerate().find(|(bi, b)| {
                !base_taken[*bi]
                    && ((!h.id.is_empty() && h.id == b.id)
                        || (!h.name.is_empty() && h.name == b.name))
            }) {
                base_taken[bi] = true;
                paired_base[hi] = Some(bi);
            }
        }
        // Pass 2: pair remaining Sonar steps in order within the job.
        for (hi, slot) in paired_base.iter_mut().enumerate() {
            if slot.is_some() {
                continue;
            }
            if let Some((bi, _)) = base_steps
                .iter()
                .enumerate()
                .find(|(bi, _)| !base_taken[*bi])
            {
                base_taken[bi] = true;
                *slot = Some(bi);
                let _ = hi;
            }
        }

        let job_id = job_k.as_str().unwrap_or("");
        let job_line = ci_integrity::job_line(head_src, job_id);
        let total_steps = head_job
            .get("steps")
            .and_then(|s| s.as_sequence())
            .map(|s| s.len())
            .unwrap_or(0);
        let spans = job_line.and_then(|l| ci_integrity::step_spans(head_src, l, total_steps));
        let empty = Value::Object(Default::default());

        for (hi, h) in head_steps.iter().enumerate() {
            let base_tree = paired_base[hi]
                .map(|bi| &base_steps[bi].props)
                .unwrap_or(&empty);
            let span = spans.as_ref().and_then(|s| s.get(h.step_idx).copied());
            let approx_line = span
                .map(|s| {
                    ci_integrity::step_line(
                        head_src,
                        s,
                        &h.name,
                        &h.id,
                        h.uses.as_deref(),
                        h.run.as_deref(),
                    )
                })
                .or(job_line);
            for w in diff_trees(base_tree, &h.props, &sonar_rules) {
                let line = span
                    .and_then(|s| s.line_with(head_src, s.start, &w.key))
                    .or(approx_line);
                findings.push((w, line));
            }
        }
    }

    any_sonar_step.then_some(findings)
}

fn sonar_workflow_file(
    ctx: &Context,
    severity: Severity,
    out: &mut GateOutcome,
    file: &crate::gitctx::ChangedFile,
) -> Result<()> {
    const GATE: &str = "toolchain-config";
    let Some(head_src) = ctx.git.head_content(&file.path)? else {
        return Ok(());
    };
    let base_src = ctx.git.base_content(&file.old_path)?;
    let Some(findings) = diff_sonar_workflow(base_src.as_deref(), &head_src) else {
        return Ok(());
    };
    out.examined += 1;
    let lines: Vec<&str> = head_src.lines().collect();
    for (w, line) in findings {
        if line
            .and_then(|l| lines.get(l.checked_sub(1)?))
            .is_some_and(|l| line_allows(l, GATE))
        {
            out.inline_exemptions += 1;
            continue;
        }
        let lift = |subject: &str| {
            ctx.find_override(
                GATE,
                &crate::findings::TOOLCHAIN_CONFIG_WEAKENED,
                tokens::ALLOW_TOOLCHAIN_WEAKENING,
                subject,
            )
        };
        let ov = lift(&w.key)
            .or_else(|| w.key.rsplit('.').next().and_then(lift))
            .or_else(|| lift(&file.path));
        let anchored = ov.is_none();
        out.lift_or_push(
            ov,
            ctx.overridable(severity),
            &crate::findings::TOOLCHAIN_CONFIG_WEAKENED,
            (Some(&file.path), line),
            format!("`{}` {} in `{}`.", w.key, w.what, file.path),
            &format!(
                "Revert it, or justify it on its own line in the PR body or a commit message: `allow-toolchain-weakening: {} <reason>`.",
                w.key
            ),
        );
        if anchored {
            out.anchor_last(w.key);
        }
    }
    Ok(())
}

/// A changed file the rule table does not read: a build file is judged on its warning
/// flags ([`build_file`]), and a CI workflow with a Sonar scanner step is judged on its
/// `-Dsonar.*` arguments ([`sonar_workflow_file`]).
fn other_file(
    ctx: &Context,
    severity: Severity,
    out: &mut GateOutcome,
    file: &crate::gitctx::ChangedFile,
) -> Result<()> {
    if let Some(kind) = build_flags::classify_build(&file.path) {
        build_file(ctx, severity, out, file, kind)?;
    } else if is_workflow_path(&file.path) {
        sonar_workflow_file(ctx, severity, out, file)?;
    }
    Ok(())
}

fn executable_message(path: &str) -> String {
    format!(
        "`{path}` is configuration written as code; whether the change loosens it cannot be read from a diff."
    )
}

fn unreadable_message(path: &str, _side: &str) -> String {
    format!("`{path}` could not be parsed on one side, so its weakening could not be checked.")
}

/// What this gate hands the shared loop ([`config_delta::run`]).
const TREE_GATE: TreeGate = TreeGate {
    id: "toolchain-config",
    directive: tokens::ALLOW_TOOLCHAIN_WEAKENING,
    classify,
    changed: &crate::findings::TOOLCHAIN_CONFIG_WEAKENED,
    not_analysed: &crate::findings::TOOLCHAIN_CHANGE_NOT_ANALYSED,
    unreadable: &crate::findings::TOOLCHAIN_CONFIG_UNREADABLE,
    executable_message,
    unreadable_message,
    // A file that appears has no bar to lower; one that disappears is reported: its
    // settings no longer apply.
    absent_side: AbsentSide::NotCompared {
        deleted: &crate::findings::TOOLCHAIN_CONFIG_DELETED,
    },
    read_order: ReadOrder::HeadThenBase,
    inherited: Some(inherited_changes),
    literal_code: Some(literal_code),
    other_file: Some(other_file),
    nothing_examined: "no toolchain configuration files modified in this diff",
};

pub fn toolchain_config(ctx: &Context) -> Result<GateOutcome> {
    config_delta::run(ctx, &ctx.config.gates.toolchain_config, &TREE_GATE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(name: &str, base: &str, head: &str) -> Vec<(String, String)> {
        let Some(Classified::Data { name, rules }) = classify(name) else {
            panic!("{name} is not a data config");
        };
        let b = load(&name, base).expect("base parses");
        let h = load(&name, head).expect("head parses");
        diff_trees(&b, &h, &rules)
            .into_iter()
            .map(|w| (w.key, w.what))
            .collect()
    }

    #[test]
    fn tsconfig_with_comments_strict_off_and_exclude_grown() {
        let base = "{\n  // strict everywhere\n  \"compilerOptions\": { \"strict\": true, \"skipLibCheck\": false, },\n  \"exclude\": [\"dist\"],\n}\n";
        let head = "{\"compilerOptions\": {\"strict\": false, \"skipLibCheck\": true}, \"exclude\": [\"dist\", \"src/legacy\"]}";
        assert_eq!(
            diff("tsconfig.json", base, head),
            vec![
                (
                    "compilerOptions.strict".to_string(),
                    "switched off".to_string()
                ),
                (
                    "compilerOptions.skipLibCheck".to_string(),
                    "switched on".to_string()
                ),
                (
                    "exclude".to_string(),
                    "gained 1 entr(y/ies): `src/legacy`".to_string()
                ),
            ]
        );
        // The reverse tightens; removing `strict: true` loosens.
        assert!(diff("tsconfig.build.json", head, base).is_empty());
        assert_eq!(
            diff("tsconfig.json", base, "{\"compilerOptions\": {}}")[0].1,
            "removed (was on)"
        );
    }

    #[test]
    fn pyproject_ruff_mypy_pytest_and_coverage() {
        let base = "[tool.ruff.lint]\nselect = [\"E\", \"F\", \"B\"]\nignore = []\n\
            [tool.mypy]\nstrict = true\n\
            [tool.pytest.ini_options]\naddopts = \"--strict-markers -p no:cacheprovider\"\nxfail_strict = true\n\
            [tool.coverage.report]\nfail_under = 90\n";
        let head = "[tool.ruff.lint]\nselect = [\"E\", \"F\"]\nignore = [\"E501\"]\n\
            [tool.ruff.lint.per-file-ignores]\n\"tests/*\" = [\"S101\"]\n\
            [tool.mypy]\nstrict = false\nignore_missing_imports = true\n\
            [tool.pytest.ini_options]\naddopts = \"-p no:cacheprovider --reruns 3\"\nxfail_strict = false\n\
            [tool.coverage.report]\nfail_under = 60\n";
        let keys: Vec<String> = diff("pyproject.toml", base, head)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            keys,
            vec![
                "tool.ruff.lint.select",
                "tool.ruff.lint.ignore",
                "tool.ruff.lint.per-file-ignores.tests/*",
                "tool.mypy.strict",
                "tool.mypy.ignore_missing_imports",
                "tool.pytest.ini_options.addopts",
                "tool.pytest.ini_options.xfail_strict",
                "tool.coverage.report.fail_under",
            ]
        );
        let found = diff("pyproject.toml", base, head);
        let addopts = &found
            .iter()
            .find(|(k, _)| k.ends_with("addopts"))
            .unwrap()
            .1;
        assert_eq!(addopts, "lost `--strict-markers`; gained `--reruns`");
        // A version bump elsewhere in the file is silent.
        assert!(diff(
            "pyproject.toml",
            base,
            &format!("{base}[project]\nversion = \"2\"\n")
        )
        .is_empty());
    }

    #[test]
    fn ini_files_mypy_pytest_coveragerc_and_setup_cfg() {
        let base = "[mypy]\nstrict = True\nexclude = build/\n\n[tool:pytest]\naddopts = --strict-config\n\n[flake8]\nmax-line-length = 88\nignore =\n    E203\n";
        let head = "[mypy]\nstrict = False\nexclude = build/, vendor/\n\n[mypy-thirdparty.*]\nignore_errors = True\n\n[tool:pytest]\naddopts = -q\n\n[flake8]\nmax-line-length = 120\nignore =\n    E203\n    W503\n";
        let keys: Vec<String> = diff("setup.cfg", base, head)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            keys,
            vec![
                "mypy.strict",
                "mypy.exclude",
                "mypy-thirdparty.*.ignore_errors",
                "tool:pytest.addopts",
                "flake8.ignore",
                "flake8.max-line-length",
            ]
        );
        assert_eq!(
            diff(
                ".coveragerc",
                "[report]\nfail_under = 85\n",
                "[report]\nfail_under = 70\n"
            ),
            vec![(
                "report.fail_under".to_string(),
                "lowered from 85 to 70".to_string()
            )]
        );
    }

    #[test]
    fn cargo_lints_rustflags_nextest_and_eslint_levels() {
        assert_eq!(
            diff(
                "Cargo.toml",
                "[lints.rust]\nunsafe_code = \"forbid\"\n[lints.clippy]\nunwrap_used = { level = \"deny\", priority = 1 }\nall = \"warn\"\n",
                "[lints.rust]\nunsafe_code = \"warn\"\n[lints.clippy]\nunwrap_used = \"allow\"\nall = \"deny\"\n",
            ),
            vec![
                ("lints.clippy.unwrap_used".to_string(), "lowered from error to off".to_string()),
                ("lints.rust.unsafe_code".to_string(), "lowered from error to warn".to_string()),
            ]
        );
        assert_eq!(
            diff(
                ".cargo/config.toml",
                "[build]\nrustflags = [\"-D\", \"warnings\", \"-C\", \"target-cpu=native\"]\n",
                "[build]\nrustflags = [\"-C\", \"target-cpu=native\", \"-A\", \"dead_code\"]\n",
            ),
            vec![(
                "build.rustflags".to_string(),
                "lost `-D warnings`; gained `-A`".to_string()
            )]
        );
        assert_eq!(
            diff(
                ".config/nextest.toml",
                "[profile.ci]\nfail-fast = true\n",
                "[profile.ci]\nretries = 3\nfail-fast = false\n"
            ),
            vec![
                (
                    "profile.ci.retries".to_string(),
                    "raised from 0 to 3".to_string()
                ),
                (
                    "profile.ci.fail-fast".to_string(),
                    "switched off".to_string()
                ),
            ]
        );
        assert_eq!(
            diff(
                ".eslintrc.json",
                "{\"rules\": {\"no-unused-vars\": \"error\", \"eqeqeq\": [2, \"always\"], \"semi\": 1}}",
                "{\"rules\": {\"no-unused-vars\": \"warn\", \"eqeqeq\": [\"off\"], \"semi\": 2}, \"ignorePatterns\": [\"legacy/\"]}",
            ),
            vec![
                ("rules.eqeqeq".to_string(), "lowered from error to off".to_string()),
                ("rules.no-unused-vars".to_string(), "lowered from error to warn".to_string()),
                ("ignorePatterns".to_string(), "gained 1 entr(y/ies): `legacy/`".to_string()),
            ]
        );
    }

    #[test]
    fn golangci_codecov_jest_phpstan_and_phpunit() {
        assert_eq!(
            diff(
                ".golangci.yml",
                "linters:\n  enable: [govet, errcheck]\nissues:\n  exclude-rules: []\n",
                "linters:\n  enable: [govet]\n  disable: [errcheck]\nissues:\n  exclude-rules:\n    - path: _test\\.go\n      linters: [gosec]\n",
            )
            .into_iter()
            .map(|(k, _)| k)
            .collect::<Vec<_>>(),
            vec!["linters.enable", "linters.disable", "issues.exclude-rules"]
        );
        assert_eq!(
            diff(
                "codecov.yml",
                "coverage:\n  status:\n    project:\n      default:\n        target: 80%\n        threshold: 1%\n",
                "coverage:\n  status:\n    project:\n      default:\n        target: 60%\n        threshold: 5%\n",
            ),
            vec![
                ("coverage.status.project.default.target".to_string(), "lowered from 80 to 60".to_string()),
                ("coverage.status.project.default.threshold".to_string(), "raised from 1 to 5".to_string()),
            ]
        );
        assert_eq!(
            diff(
                "package.json",
                "{\"name\": \"a\", \"jest\": {\"coverageThreshold\": {\"global\": {\"lines\": 90}}}}",
                "{\"name\": \"a\", \"version\": \"2\", \"jest\": {\"coverageThreshold\": {\"global\": {\"lines\": 50}}}}",
            ),
            vec![("jest.coverageThreshold.global.lines".to_string(), "lowered from 90 to 50".to_string())]
        );
        assert_eq!(
            diff(
                "phpstan.neon",
                "parameters:\n  level: 8\n  ignoreErrors: []\n",
                "parameters:\n  level: 5\n  ignoreErrors:\n    - '#Call to undefined#'\n"
            )
            .into_iter()
            .map(|(k, _)| k)
            .collect::<Vec<_>>(),
            vec!["parameters.level", "parameters.ignoreErrors"]
        );
        assert_eq!(
            diff(
                "phpunit.xml.dist",
                "<?xml version=\"1.0\"?>\n<phpunit bootstrap=\"vendor/autoload.php\" failOnWarning=\"true\">\n</phpunit>\n",
                "<?xml version=\"1.0\"?>\n<phpunit bootstrap=\"vendor/autoload.php\" failOnWarning=\"false\">\n</phpunit>\n",
            ),
            vec![("phpunit.failOnWarning".to_string(), "switched off".to_string())]
        );
    }

    #[test]
    fn classification_covers_executables_nested_paths_and_ignores_the_rest() {
        assert!(matches!(
            classify("eslint.config.js"),
            Some(Classified::Executable)
        ));
        assert!(matches!(
            classify("web/jest.config.ts"),
            Some(Classified::Executable)
        ));
        assert!(matches!(
            classify("crates/a/.cargo/config.toml"),
            Some(Classified::Data { .. })
        ));
        assert!(matches!(
            classify("tsconfig.app.json"),
            Some(Classified::Data { .. })
        ));
        assert!(classify("src/main.rs").is_none());
        assert!(classify("docs/config.toml").is_none());
        assert!(classify("tsconfig.json.bak").is_none());
        assert!(load("tsconfig.json", "{ nope").is_none());
    }

    #[test]
    fn clippy_toml_thresholds_are_caps_and_lists_go_their_way() {
        let got = diff(
            "clippy.toml",
            "too-many-arguments-threshold = 7\ncognitive-complexity-threshold = 25\ndisallowed-methods = [\"std::env::set_var\"]\nallowed-scripts = [\"Latin\"]\nallow-unwrap-in-tests = false\n",
            "too-many-arguments-threshold = 12\ncognitive-complexity-threshold = 25\ndisallowed-methods = []\nallowed-scripts = [\"Latin\", \"Cyrillic\"]\nallow-unwrap-in-tests = true\n",
        );
        let mut keys: Vec<&str> = got.iter().map(|(k, _)| k.as_str()).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "allow-unwrap-in-tests",
                "allowed-scripts",
                "disallowed-methods",
                "too-many-arguments-threshold",
            ],
            "{got:?}"
        );
        // Lowering a threshold is a tightening.
        assert!(diff(
            "clippy.toml",
            "too-many-lines-threshold = 100\n",
            "too-many-lines-threshold = 50\n"
        )
        .is_empty());
    }

    #[test]
    fn a_gained_or_swapped_inheritance_is_named_not_passed() {
        let b = load(
            "tsconfig.json",
            "{\"extends\": \"@tsconfig/strictest/tsconfig.json\"}",
        )
        .unwrap();
        let h = load(
            "tsconfig.json",
            "{\"extends\": \"@tsconfig/recommended/tsconfig.json\"}",
        )
        .unwrap();
        assert_eq!(
            inherited_changes("tsconfig.json", &b, &h),
            vec![(
                "extends".to_string(),
                "`@tsconfig/recommended/tsconfig.json`".to_string()
            )]
        );
        assert!(inherited_changes("tsconfig.json", &h, &h).is_empty());
        let b = load(
            ".eslintrc.json",
            "{\"extends\": [\"eslint:recommended\"], \"plugins\": []}",
        )
        .unwrap();
        let h = load(
            ".eslintrc.json",
            "{\"extends\": [\"eslint:recommended\", \"plugin:x/lax\"], \"plugins\": [\"x\"]}",
        )
        .unwrap();
        let got = inherited_changes(".eslintrc.json", &b, &h);
        assert_eq!(got.len(), 2, "{got:?}");
        // Losing an entry is `Shrunk`'s finding, not this one.
        assert!(inherited_changes(".eslintrc.json", &h, &b).is_empty());
    }

    #[test]
    fn package_json_is_strictly_parsed_as_json() {
        assert!(load("package.json", "{\"name\": \"a\",}\n").is_none());
        assert!(load("package.json", "{\n  // comment\n  \"name\": \"a\"\n}\n").is_none());
        assert!(load("package.json", "{\"name\": \"a\"}\n").is_some());
        // Relative / nested paths are also strictly parsed.
        assert!(load("packages/sub/package.json", "{\"name\": \"a\",}\n").is_none());
        assert!(load("packages/sub/package.json", "{\"name\": \"a\"}\n").is_some());
        // Control: tsconfig.json allows comments and trailing commas.
        assert!(load("tsconfig.json", "{\"name\": \"a\",}\n").is_some());
    }

    #[test]
    fn properties_reader_handles_separators_comments_continuations_escapes_and_rejects_malformed() {
        let src = "\
# hash comment
! bang comment
   # indented comment
key_eq=val1
key_colon : val2
key_space val3
escaped\\=key\\:name\\ here = a\\=b\\:c\\n\\t\\u0041
continued = first,\\
    second,\\
    third
";
        let tree = load("sonar-project.properties", src).expect("valid .properties");
        let Value::Object(m) = tree else {
            panic!("expected Object");
        };
        assert_eq!(m.get("key_eq"), Some(&Value::String("val1".to_string())));
        assert_eq!(m.get("key_colon"), Some(&Value::String("val2".to_string())));
        assert_eq!(m.get("key_space"), Some(&Value::String("val3".to_string())));
        assert_eq!(
            m.get("escaped=key:name here"),
            Some(&Value::String("a=b:c\n\tA".to_string()))
        );
        assert_eq!(
            m.get("continued"),
            Some(&Value::String("first,second,third".to_string()))
        );

        // Malformed lines (missing separator/value or invalid unicode escape) return None.
        assert!(load("sonar-project.properties", "bare_key_without_value\n").is_none());
        assert!(load("sonar-project.properties", "=empty_key\n").is_none());
        assert!(load("sonar-project.properties", "k=\\u12G4\n").is_none());
    }

    #[test]
    fn sonar_properties_rules_cover_all_keys_and_directions() {
        // Reproduction from #809: exclusions grown, inclusions narrowed (appeared),
        // qualitygate.wait switched off, multicriteria grown.
        let base = "\
sonar.projectKey=acme
sonar.sources=src/core,src/payments
sonar.exclusions=**/generated/**
sonar.coverage.exclusions=**/migrations/**
sonar.cpd.exclusions=**/proto/**
sonar.test.exclusions=**/fixtures/**
sonar.test.inclusions=tests/unit/**,tests/integration/**
sonar.qualitygate.wait=true
sonar.issue.ignore.multicriteria=e1
sonar.issue.ignore.block=b1
sonar.issue.ignore.allfile=a1
";
        let head = "\
sonar.projectKey=acme
sonar.sources=src/core
sonar.exclusions=**/generated/**,src/legacy/**,src/payments/**
sonar.coverage.exclusions=**/migrations/**,src/**
sonar.cpd.exclusions=**/proto/**,src/dup/**
sonar.test.exclusions=**/fixtures/**,tests/slow/**
sonar.inclusions=src/safe/**
sonar.test.inclusions=tests/unit/**
sonar.qualitygate.wait=false
sonar.issue.ignore.multicriteria=e1,e2
sonar.issue.ignore.multicriteria.e2.ruleKey=*
sonar.issue.ignore.multicriteria.e2.resourceKey=**/*.java
sonar.issue.ignore.block=b1,b2
sonar.issue.ignore.allfile=a1,a2
";
        let got = diff("sonar-project.properties", base, head);
        let keys: Vec<&str> = got.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "sonar.exclusions",
                "sonar.coverage.exclusions",
                "sonar.cpd.exclusions",
                "sonar.test.exclusions",
                "sonar.inclusions",
                "sonar.test.inclusions",
                "sonar.sources",
                "sonar.qualitygate.wait",
                "sonar.issue.ignore.multicriteria",
                "sonar.issue.ignore.block",
                "sonar.issue.ignore.allfile",
            ],
            "{got:?}"
        );

        // Also works for .sonarcloud.properties and when exclusions appear from nothing
        // or qualitygate.wait is removed.
        let cloud_got = diff(
            ".sonarcloud.properties",
            "sonar.qualitygate.wait=true\n",
            "sonar.exclusions=src/legacy/**\n",
        );
        assert_eq!(
            cloud_got,
            vec![
                (
                    "sonar.exclusions".to_string(),
                    "gained 1 entr(y/ies): `src/legacy/**`".to_string()
                ),
                (
                    "sonar.qualitygate.wait".to_string(),
                    "removed (was on)".to_string()
                ),
            ]
        );

        // Negative control: shrinking exclusions, widening inclusions/sources, or
        // enabling qualitygate.wait is a tightening and produces no findings.
        assert!(diff("sonar-project.properties", head, base).is_empty());
    }

    #[test]
    fn sonar_workflow_scanner_arguments_detect_loosening_across_invocations() {
        let base_wf = "\
name: sonar
on: push
jobs:
  scan:
    runs-on: ubuntu-latest
    steps:
      - run: sonar-scanner -Dsonar.qualitygate.wait=true -Dsonar.exclusions=**/gen/**
      - name: maven scan
        run: mvn -B verify sonar:sonar -Dsonar.coverage.exclusions=**/migrations/** --define=sonar.sources=src,app
      - name: gradle scan
        run: ./gradlew sonarqube --define sonar.inclusions=src/a/**,src/b/**
      - uses: SonarSource/sonarqube-scan-action@v3
        with:
          args: >
            -Dsonar.qualitygate.wait=true
            -Dsonar.issue.ignore.multicriteria=e1
";
        let head_wf = "\
name: sonar
on: push
jobs:
  scan:
    runs-on: ubuntu-latest
    steps:
      - run: sonar-scanner -Dsonar.qualitygate.wait=false -Dsonar.exclusions=**/gen/**,src/payments/**
      - name: maven scan
        run: mvn -B verify sonar:sonar -Dsonar.coverage.exclusions=**/migrations/**,src/** --define=sonar.sources=src
      - name: gradle scan
        run: ./gradlew sonarqube --define sonar.inclusions=src/a/**
      - uses: SonarSource/sonarqube-scan-action@v3
        with:
          args: >
            -Dsonar.qualitygate.wait=false
            -Dsonar.issue.ignore.multicriteria=e1,e2
            -Dsonar.cpd.exclusions=src/dup/**
";
        let got = diff_sonar_workflow(Some(base_wf), head_wf).expect("workflow with Sonar steps");
        let keys: Vec<&str> = got.iter().map(|(w, _)| w.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "sonar.exclusions",
                "sonar.qualitygate.wait",
                "sonar.coverage.exclusions",
                "sonar.sources",
                "sonar.inclusions",
                "sonar.cpd.exclusions",
                "sonar.qualitygate.wait",
                "sonar.issue.ignore.multicriteria",
            ],
            "{got:?}"
        );

        // Negative control: tightening produces no findings.
        let rev = diff_sonar_workflow(Some(head_wf), base_wf).expect("workflow with Sonar steps");
        assert!(rev.is_empty(), "{rev:?}");

        // Non-Sonar workflow returns None so examined count stays untouched.
        let non_sonar = "name: ci\non: push\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test\n";
        assert!(diff_sonar_workflow(Some(non_sonar), non_sonar).is_none());
    }

    #[test]
    fn pytest_keyword_and_marker_selection_gained_or_narrowed_in_addopts() {
        // Every file pytest reads `addopts` from.
        for (file, base, head) in [
            (
                "pytest.ini",
                "[pytest]\ntestpaths = tests\n",
                "[pytest]\ntestpaths = tests\naddopts = -k 'not test_f'\n",
            ),
            (
                "pytest.ini",
                "[pytest]\naddopts = -q\n",
                "[pytest]\naddopts = -q -m 'not slow'\n",
            ),
            (
                "tox.ini",
                "[pytest]\naddopts = -m 'unit or integration'\n",
                "[pytest]\naddopts = -m unit\n",
            ),
            (
                "setup.cfg",
                "[tool:pytest]\naddopts = -k test_\n",
                "[tool:pytest]\naddopts =\n    -k\n    test_ and not test_f\n",
            ),
            (
                "pyproject.toml",
                "[tool.pytest.ini_options]\ntestpaths = ['tests']\n",
                "[tool.pytest.ini_options]\ntestpaths = ['tests']\naddopts = \"-k 'not test_f'\"\n",
            ),
            (
                "pyproject.toml",
                "[tool.pytest.ini_options]\naddopts = [\"-q\"]\n",
                "[tool.pytest.ini_options]\naddopts = [\"-q\", \"-m\", \"not slow\"]\n",
            ),
        ] {
            let found = diff(file, base, head);
            assert_eq!(found.len(), 1, "{file}: {head:?} -> {found:?}");
            assert!(found[0].0.ends_with("addopts"), "{found:?}");
        }
        assert_eq!(
            diff(
                "pytest.ini",
                "[pytest]\naddopts = --strict-markers\n",
                "[pytest]\naddopts = -k 'not test_f'\n"
            ),
            vec![(
                "pytest.addopts".to_string(),
                "lost `--strict-markers`; gained `-k not test_f` (a test its keyword expression does not match is deselected)".to_string()
            )]
        );
        // Selecting more tests, or the same ones, is not a weakening.
        for (base, head) in [
            ("-k 'a or b'", "-k 'a or b or c'"),
            ("-m 'not slow'", "-q"),
            ("-m 'unit and not slow'", "-m unit"),
            ("-k 'not   test_f'", "-k \"not test_f\""),
            ("-k test_f", "-k test"),
        ] {
            let found = diff(
                "pytest.ini",
                &format!("[pytest]\naddopts = {base}\n"),
                &format!("[pytest]\naddopts = {head}\n"),
            );
            assert!(found.is_empty(), "{base:?} -> {head:?}: {found:?}");
        }
    }

    #[test]
    fn a_package_test_script_that_masks_its_exit_code_or_passes_with_no_tests() {
        let pkg = |name: &str, script: &str| {
            serde_json::json!({ "name": "x", "scripts": { name: script } }).to_string()
        };
        for (name, base, head, what) in [
            (
                "test",
                "jest",
                "jest --passWithNoTests || true",
                "`scripts.test` masks the test command's exit code (`jest --passWithNoTests || true`: the right side of `||` cannot fail); gained `--passWithNoTests`",
            ),
            (
                "test",
                "jest",
                "jest || exit 0",
                "`scripts.test` masks the test command's exit code (`jest || exit 0`: the right side of `||` cannot fail)",
            ),
            (
                "test:unit",
                "vitest run",
                "vitest run; exit 0",
                "`scripts.test:unit` masks the test command's exit code (`exit 0`: the last command cannot fail)",
            ),
            (
                "test:ci",
                "mocha",
                "mocha || :",
                "`scripts.test:ci` masks the test command's exit code (`mocha || :`: the right side of `||` cannot fail)",
            ),
            (
                "test",
                "vitest run",
                "vitest run --passWithNoTests",
                "`scripts.test` gained `--passWithNoTests`",
            ),
        ] {
            let found = diff("package.json", &pkg(name, base), &pkg(name, head));
            let shown: Vec<String> = found.iter().map(|(k, w)| format!("`{k}` {w}")).collect();
            assert_eq!(shown, [what], "{head:?}");
        }
        // A workspace package is read the same way.
        assert_eq!(
            diff(
                "packages/core/package.json",
                &pkg("test", "jest"),
                &pkg("test", "jest || true")
            )
            .len(),
            1
        );
        // A script that is not a test script, a reformatted test script, a mask the base
        // already had, a flag switched off, and a mask inside a string are not reported.
        for (name, base, head) in [
            ("lint", "eslint .", "eslint . || true"),
            ("pretest", "tsc", "tsc || true"),
            ("test", "jest  --ci", "jest --ci"),
            ("test", "jest || true", "jest --ci || true"),
            ("test", "jest", "jest --passWithNoTests=false"),
            ("test", "jest", "jest --testNamePattern '|| true'"),
        ] {
            let found = diff("package.json", &pkg(name, base), &pkg(name, head));
            let lax = found.iter().any(|(_, w)| w.contains("masks"));
            assert!(!lax, "{name}: {head:?}: {found:?}");
        }
        assert!(diff(
            "package.json",
            &pkg("lint", "eslint ."),
            &pkg("lint", "eslint . || true")
        )
        .is_empty());
        assert!(diff(
            "package.json",
            &pkg("test", "jest  --ci"),
            &pkg("test", "jest --ci")
        )
        .is_empty());
    }

    #[test]
    fn a_literal_jest_or_vitest_configuration_is_read_like_a_data_file() {
        let judged = |path: &str, base: &str, head: &str| -> Option<Vec<(String, String)>> {
            let trees = literal_code(path, base, head)?;
            Some(
                diff_trees(&trees.base, &trees.head, &trees.rules)
                    .into_iter()
                    .map(|w| (w.key, w.what))
                    .collect(),
            )
        };
        let base = "module.exports = { testEnvironment: 'node' };\n";
        assert_eq!(
            judged(
                "jest.config.js",
                base,
                "module.exports = { testEnvironment: 'node', testPathIgnorePatterns: ['calc'] };\n"
            ),
            Some(vec![(
                "testPathIgnorePatterns".to_string(),
                "gained 1 entr(y/ies): `calc`".to_string()
            )])
        );
        let ts_base = "import type { Config } from 'jest';\n\nconst config: Config = {\n  testMatch: ['**/*.test.ts'],\n};\n\nexport default config;\n";
        let ts_head = "import type { Config } from 'jest';\n\nconst config: Config = {\n  testMatch: ['**/core/*.test.ts'],\n  modulePathIgnorePatterns: ['<rootDir>/src/calc'],\n};\n\nexport default config;\n";
        let keys: Vec<String> = judged("web/jest.config.ts", ts_base, ts_head)
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, ["modulePathIgnorePatterns", "testMatch"]);
        let vitest = |test: &str| {
            format!("import {{ defineConfig }} from 'vitest/config';\n\nexport default defineConfig({{ test: {test} }});\n")
        };
        let keys: Vec<String> = judged(
            "vitest.config.ts",
            &vitest("{ include: ['src/**/*.test.ts'] }"),
            &vitest("{ include: ['src/core/**/*.test.ts'], exclude: ['src/calc.test.ts'] }"),
        )
        .unwrap()
        .into_iter()
        .map(|(k, _)| k)
        .collect();
        assert_eq!(keys, ["test.exclude", "test.include"]);
        // Only the keys the rules read are judged: the rest of a change to such a file is
        // still not analysed.
        let other = |base: &str, head: &str| {
            literal_code("jest.config.js", base, head).map(|t| t.other_changes)
        };
        assert_eq!(
            other(
                base,
                "module.exports = { testEnvironment: 'node', testPathIgnorePatterns: ['calc'] };\n"
            ),
            Some(false)
        );
        assert_eq!(
            other(base, "// shared settings\nmodule.exports = {\n  testEnvironment: 'node', // the default runner\n};\n"),
            Some(true)
        );
        assert_eq!(
            other(
                base,
                "module.exports = {\n  testEnvironment: 'node', // the default runner\n};\n"
            ),
            Some(false)
        );
        assert_eq!(
            other(base, "module.exports = { testEnvironment: 'jsdom' };\n"),
            Some(true)
        );
        // A key no rule reads, and a tightening, are not weakenings.
        assert_eq!(
            judged(
                "jest.config.js",
                base,
                "module.exports = { testEnvironment: 'node', collectCoverageFrom: ['src/**'] };\n"
            ),
            Some(vec![])
        );
        assert_eq!(
            judged(
                "jest.config.js",
                "module.exports = { testPathIgnorePatterns: ['calc', 'dist'] };\n",
                "module.exports = { testPathIgnorePatterns: [/* built */ 'dist'] };\n"
            ),
            Some(vec![])
        );
        // What is not a literal is not read: the change stays not analysed.
        for head in [
            "const base = require('./jest.base');\nmodule.exports = { ...base, testPathIgnorePatterns: ['calc'] };\n",
            "module.exports = { testPathIgnorePatterns: ignored() };\n",
            "module.exports = { testPathIgnorePatterns: [...shared, 'calc'] };\n",
            "module.exports = { testEnvironment: 'node' };\nmodule.exports.testPathIgnorePatterns = ['calc'];\n",
            "module.exports = makeConfig();\n",
        ] {
            assert!(judged("jest.config.js", base, head).is_none(), "{head}");
        }
        assert!(judged(
            "vitest.config.mts",
            &vitest("{ include: ['a'] }"),
            &vitest("{ ...configDefaults, include: ['a'] }")
        )
        .is_none());
        // A computed value under a key no rule reads does not stop the read, and a change
        // to it is a change no rule reads.
        let transform = |module: &str| {
            format!("module.exports = {{ testPathIgnorePatterns: ['dist'], transform: {{ '^.+\\\\.ts$': require.resolve('{module}') }} }};\n")
        };
        assert_eq!(
            judged(
                "jest.config.js",
                &transform("ts-jest"),
                &transform("babel-jest")
            ),
            Some(vec![])
        );
        assert_eq!(
            other(&transform("ts-jest"), &transform("babel-jest")),
            Some(true)
        );
        assert_eq!(
            other(&transform("ts-jest"), &transform("ts-jest")),
            Some(false)
        );
        // An eslint configuration is not read as one.
        assert!(judged(
            "eslint.config.js",
            "export default [];\n",
            "export default [{}];\n"
        )
        .is_none());
        // A gained preset is inherited, as in `jest.config.json`.
        let b = crate::ast::runner_config::script_config_tree("jest.config.js", base).unwrap();
        let h = crate::ast::runner_config::script_config_tree(
            "jest.config.js",
            "module.exports = { preset: 'ts-jest', testEnvironment: 'node' };\n",
        )
        .unwrap();
        assert_eq!(
            inherited_changes("jest.config.js", &b.tree, &h.tree),
            vec![("preset".to_string(), "`ts-jest`".to_string())]
        );
    }
}
