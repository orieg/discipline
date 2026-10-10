//! `harness-tampering`: a file the test runner loads must not make a failing run pass.
//!
//! No test is deleted and no assertion is dropped when a `conftest.py` rewrites every
//! report to "passed", a Go `TestMain` exits with `0` whatever `m.Run()` returned, or a
//! global set-up file ends the process before a test has run. The test-facing gates see
//! nothing, and `toolchain-config` reports such a file as changed and not analysed.
//!
//! This gate reads the forms of [`crate::ast::harness`] in the files the runner loads,
//! and reports the ones the change adds: a form the base side of the same harness file
//! already held is not reported again. Which files those are comes from how each runner
//! finds them, never from a word in the path:
//!
//! - by name ([`named_harness`]): `conftest.py`, `sitecustomize.py` /
//!   `usercustomize.py`, a `_test.go` file the go tool builds, a Jest or Vitest
//!   configuration written as code;
//! - by configuration ([`ConfiguredHarness`]): the set-up and global set-up / tear-down
//!   files a Jest or Vitest configuration names, and the root file of each Cargo test
//!   target;
//! - a Python test file (the Python pack's test-file rule, or `[tests] paths`), for the
//!   `unittest` forms only.
//!
//! A file that becomes a harness file without changing (a configuration now names it) is
//! read as if every form in it were added. A harness file that cannot be parsed is named
//! in the notes, never passed in silence. The notes also list every harness file read and
//! the version of the forms.

use super::{Context, GateOutcome, PathFilter};
use crate::ast::harness::{Form, PythonChecks, Scan, Site, PATTERN_VERSION};
use crate::ast::runner_collection::{named_harness, ConfiguredHarness, NamedHarness};
use crate::ast::{default_registry, LanguageRegistry};
use crate::gitctx::ChangeKind;
use crate::tokens;
use anyhow::Result;
use std::collections::{HashMap, HashSet};

pub const GATE: &str = "harness-tampering";

/// The scope limit every finding states.
pub const SCOPE_LIMIT: &str = "Exact form only; no value-flow analysis.";

/// What the runner loads a file as, which decides the forms read in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Roles {
    /// A Go test file: `TestMain`.
    pub go_test: bool,
    /// pytest imports it: report and collection hooks.
    pub pytest_hooks: bool,
    /// Python the runner executes before or around the tests: an exit with status zero.
    pub python_exit: bool,
    /// Python the runner loads: `unittest` result methods.
    pub unittest: bool,
    /// JavaScript or TypeScript the runner executes around the tests.
    pub js_exit: bool,
    /// The root of a Cargo test target.
    pub rust_exit: bool,
    /// How the file is known to be loaded, for the finding's message.
    pub why: String,
}

impl Roles {
    pub fn any(&self) -> bool {
        self.go_test
            || self.pytest_hooks
            || self.python_exit
            || self.unittest
            || self.js_exit
            || self.rust_exit
    }

    /// The same forms are read, whatever the reason.
    fn same_forms(&self, other: &Self) -> bool {
        let flags = |r: &Self| {
            [
                r.go_test,
                r.pytest_hooks,
                r.python_exit,
                r.unittest,
                r.js_exit,
                r.rust_exit,
            ]
        };
        flags(self) == flags(other)
    }
}

/// What the runner loads `path` as on the side `configured` was read from.
pub fn roles(
    path: &str,
    configured: &ConfiguredHarness,
    registry: &LanguageRegistry,
    test_paths: &[String],
) -> Roles {
    let mut roles = Roles::default();
    match named_harness(path) {
        Some(NamedHarness::Conftest) => {
            roles.pytest_hooks = true;
            roles.python_exit = true;
            roles.unittest = true;
            roles.why = "a pytest `conftest.py`".to_string();
        }
        Some(NamedHarness::PythonStartup) => {
            roles.python_exit = true;
            roles.unittest = true;
            roles.why = "a Python start-up file".to_string();
        }
        Some(NamedHarness::GoTest) => {
            roles.go_test = true;
            roles.why = "a Go test file".to_string();
        }
        Some(NamedHarness::JsRunnerConfig) => {
            roles.js_exit = true;
            roles.why = "a test runner configuration".to_string();
        }
        None => {}
    }
    if let Some(config) = configured.js_setup.get(path) {
        roles.js_exit = true;
        roles.why = format!("a set-up file that `{config}` names");
    }
    if let Some(conftest) = configured.pytest_plugins.get(path) {
        roles.pytest_hooks = true;
        roles.python_exit = true;
        roles.unittest = true;
        roles.why = format!("a pytest plugin that `{conftest}` names");
    }
    if let Some(own_main) = configured.rust_targets.get(path) {
        roles.rust_exit = true;
        roles.why = if *own_main {
            "the root of a Cargo test target with `harness = false`".to_string()
        } else {
            "the root of a Cargo test target".to_string()
        };
    }
    if let Some(target) = configured.rust_modules.get(path) {
        roles.rust_exit = true;
        roles.why = format!("a module of Cargo test target `{target}`");
    }
    if !roles.any()
        && crate::ast::extension(path) == Some("py")
        && (registry
            .find_pack(path)
            .is_some_and(|p| p.is_test_path(path))
            || crate::ast::functions::declared_test_path(path, test_paths))
    {
        roles.unittest = true;
        roles.why = "a Python test file".to_string();
    }
    roles
}

/// One harness file as read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileScan {
    pub scan: Scan,
    /// For a Go test file: `(has a TestMain, tests, benchmarks)`.
    pub go: Option<(bool, usize, usize)>,
}

/// Reads `src` for the forms `roles` names. `Err` when the file cannot be parsed, with
/// the reason.
pub fn scan(path: &str, src: &str, roles: &Roles) -> std::result::Result<FileScan, String> {
    let mut out = FileScan::default();
    let mut merge = |scan: Scan| {
        out.scan.sites.extend(scan.sites);
        out.scan.notes.extend(scan.notes);
        out.scan.parse_errors |= scan.parse_errors;
    };
    let mut go = None;
    if roles.go_test {
        let file = crate::ast::harness::go_test_file(src)?;
        go = Some((file.has_test_main, file.tests, file.benchmarks));
        merge(file.scan);
    }
    if roles.pytest_hooks || roles.python_exit || roles.unittest {
        merge(crate::ast::harness::python(
            src,
            PythonChecks {
                hooks: roles.pytest_hooks,
                exit: roles.python_exit,
                unittest: roles.unittest,
            },
        )?);
    }
    if roles.js_exit {
        merge(crate::ast::harness::js_exit_zero(path, src)?);
    }
    if roles.rust_exit {
        merge(crate::ast::harness::rust_exit_zero(src)?);
    }
    out.go = go;
    out.scan.sites.sort_by_key(|s| s.line);
    Ok(out)
}

/// The sites of `head` the change adds: each form in each function, counted, less what
/// the base side held. A form that moved within its function is not new.
pub fn new_sites(base: &[Site], head: Vec<Site>) -> Vec<Site> {
    let mut held: HashMap<(Form, String), usize> = HashMap::new();
    for site in base {
        *held.entry((site.form, site.subject.clone())).or_default() += 1;
    }
    head.into_iter()
        .filter(
            |site| match held.get_mut(&(site.form, site.subject.clone())) {
                Some(left) if *left > 0 => {
                    *left -= 1;
                    false
                }
                _ => true,
            },
        )
        .collect()
}

/// The finding a form is reported as, with whether it is held at `warning`, and what
/// to do about it.
fn finding(form: Form) -> (&'static crate::findings::FindingKind, bool, &'static str) {
    match form {
        Form::GoRunNeverCalled | Form::GoResultDiscarded => (
            &crate::findings::TEST_MAIN_RESULT_DISCARDED,
            false,
            "Exit with the result of the run (`os.Exit(m.Run())`)",
        ),
        Form::PytestOutcomeAssigned => (
            &crate::findings::PYTEST_HOOK_MASKS_RESULTS,
            false,
            "Leave a report's outcome as the test produced it",
        ),
        Form::PytestItemsRemoved => (
            &crate::findings::PYTEST_HOOK_MASKS_RESULTS,
            false,
            "Select tests by a marker, keyword or option the hook reads, or leave `items` whole",
        ),
        Form::UnittestMethodAssigned | Form::UnittestMethodNeutralised => (
            &crate::findings::UNITTEST_RESULT_METHOD_REPLACED,
            false,
            "Leave the result's methods as `unittest` defines them",
        ),
        // Held at `warning`: a set-up or tear-down file that ends the process with
        // status zero exists in working repositories, and no measured false-positive
        // rate supports blocking on it.
        Form::ExitZero => (
            &crate::findings::HARNESS_EXITS_ZERO,
            true,
            "Let the runner end the process with the status of the run",
        ),
    }
}

/// Whether the Go package of `path` declares benchmarks and no test: a `TestMain`
/// there has no test result to drop. Read from every test file of the directory on the
/// head side; `false` when one of them cannot be read or parsed.
fn go_package_has_benchmarks_only(ctx: &Context, path: &str, tracked: &[String]) -> Result<bool> {
    let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
    let (mut tests, mut benchmarks) = (0, 0);
    for sibling in tracked {
        let sibling_dir = sibling.rsplit_once('/').map_or("", |(dir, _)| dir);
        if sibling_dir != dir || named_harness(sibling) != Some(NamedHarness::GoTest) {
            continue;
        }
        let roles = Roles {
            go_test: true,
            ..Roles::default()
        };
        let read = ctx
            .git
            .head_content(sibling)?
            .and_then(|src| scan(sibling, &src, &roles).ok());
        match read {
            Some(FileScan {
                scan,
                go: Some((_, file_tests, file_benchmarks)),
            }) if !scan.parse_errors => {
                tests += file_tests;
                benchmarks += file_benchmarks;
            }
            _ => return Ok(false),
        }
    }
    Ok(tests == 0 && benchmarks > 0)
}

pub fn harness_tampering(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.harness_tampering;
    let mut out = GateOutcome::new(GATE);
    let exempt = PathFilter::new(&settings.exempt_paths)?;
    let registry = default_registry();
    let test_paths = &ctx.config.tests.paths;

    let changed = ctx.git.changed_files()?;
    let head_tracked = ctx.git.tracked_files()?;
    let base_tracked = ctx.git.base_tracked_files()?;
    let reads = crate::gitctx::ReadRecorder::new();
    let head_configured = ConfiguredHarness::from_tree(reads.head(ctx.git), &head_tracked);
    let base_configured = ConfiguredHarness::from_tree(reads.base(ctx.git), &base_tracked);
    reads.finish()?;
    let head_roles = |path: &str| roles(path, &head_configured, &registry, test_paths);
    let base_roles = |path: &str| roles(path, &base_configured, &registry, test_paths);

    // `(head path, base path)`: the harness files the change touches, then the files a
    // configuration names on the head side and did not on the base side.
    let mut candidates: Vec<(String, Option<String>)> = Vec::new();
    let mut touched: HashSet<&str> = HashSet::new();
    for file in &changed {
        touched.insert(file.path.as_str());
        if file.kind == ChangeKind::Deleted
            || exempt.matches(&file.path)
            || !head_roles(&file.path).any()
        {
            continue;
        }
        let base_path = (file.kind != ChangeKind::Added).then(|| file.old_path.clone());
        candidates.push((file.path.clone(), base_path));
    }
    for path in head_configured
        .js_setup
        .keys()
        .chain(head_configured.rust_targets.keys())
    {
        if touched.contains(path.as_str())
            || exempt.matches(path)
            || head_roles(path).same_forms(&base_roles(path))
        {
            continue;
        }
        candidates.push((path.clone(), Some(path.clone())));
    }

    let mut examined: Vec<&str> = Vec::new();
    for (path, base_path) in &candidates {
        let Some(head_src) = ctx.git.head_content(path)? else {
            out.notes.push(super::unread_note(path));
            continue;
        };
        let roles = head_roles(path);
        let mut head = match scan(path, &head_src, &roles) {
            Ok(head) => head,
            Err(why) => {
                out.notes.push(format!(
                    "`{path}` ({}): NOT analysed, it could not be parsed ({why})",
                    roles.why
                ));
                continue;
            }
        };
        out.examined += 1;
        examined.push(path);
        if head.scan.parse_errors {
            out.notes.push(format!(
                "`{path}` ({}): parsed with syntax errors, so it was read in part",
                roles.why
            ));
        }
        out.notes.extend(
            head.scan
                .notes
                .iter()
                .map(|note| format!("`{path}`: {note}")),
        );
        let test_main_read = head
            .scan
            .sites
            .iter()
            .any(|s| matches!(s.form, Form::GoRunNeverCalled | Form::GoResultDiscarded));
        if test_main_read && go_package_has_benchmarks_only(ctx, path, &head_tracked)? {
            head.scan
                .sites
                .retain(|s| !matches!(s.form, Form::GoRunNeverCalled | Form::GoResultDiscarded));
            out.notes.push(format!(
                "`{path}`: `TestMain` not judged, the test files of its package declare benchmarks and no test"
            ));
        }
        // What the same harness file held on the base side. A file that was not a
        // harness file there held nothing the runner ran.
        let mut base_sites = Vec::new();
        if let Some(old) = base_path {
            let roles = base_roles(old);
            if roles.any() {
                if let Some(base_src) = ctx.git.base_content(old)? {
                    base_sites = scan(old, &base_src, &roles)
                        .map(|base| base.scan.sites)
                        .unwrap_or_default();
                }
            }
        }
        let file_name = path.rsplit('/').next().unwrap_or(path);
        for site in new_sites(&base_sites, head.scan.sites) {
            let (kind, held_at_warning, fix) = finding(site.form);
            // The finding's own `path:line` first, written in full. Then the file, by
            // its path or its name, from a directive that names no line.
            let whole_file = |subject: &str| {
                ctx.find_whole_file_override(GATE, kind, tokens::ALLOW_HARNESS_TAMPERING, subject)
            };
            let lifted = [
                format!("{path}:{}", site.line),
                format!("{file_name}:{}", site.line),
            ]
            .iter()
            .find_map(|own| ctx.find_override(GATE, kind, tokens::ALLOW_HARNESS_TAMPERING, own))
            .or_else(|| whole_file(path))
            .or_else(|| whole_file(file_name));
            let severity = if held_at_warning {
                settings.severity.capped_at_warning()
            } else {
                settings.severity
            };
            out.lift_or_push(
                lifted,
                ctx.overridable(severity),
                kind,
                (Some(path), Some(site.line)),
                format!("`{path}` ({}): {}. {SCOPE_LIMIT}", roles.why, site.what),
                &format!(
                    "{fix}, or justify it on its own line in the PR body or a commit message: `allow-harness-tampering: {path} <reason>` (`{path}:{}` for this finding alone).",
                    site.line
                ),
            );
        }
    }

    // A configuration whose list could not be read: the files it may load were not
    // examined as harness files. Said only when the change touches such a file.
    let is_candidate: HashSet<&str> = candidates.iter().map(|(path, _)| path.as_str()).collect();
    for unread in &head_configured.unread {
        let below = changed
            .iter()
            .filter(|f| {
                f.kind != ChangeKind::Deleted
                    && !is_candidate.contains(f.path.as_str())
                    && !exempt.matches(&f.path)
                    && (unread.dir.is_empty() || f.path.starts_with(&format!("{}/", unread.dir)))
                    && crate::ast::extension(&f.path)
                        .is_some_and(|ext| unread.extensions.contains(&ext))
            })
            .count();
        if below > 0 {
            out.notes.push(format!(
                "`{}`: the files it has the runner load could not be read (it does not parse, or the list is computed); {below} changed file(s) below it were NOT examined as harness files",
                unread.config
            ));
        }
    }
    out.notes.push(if examined.is_empty() {
        format!("pattern version {PATTERN_VERSION}; no harness file among the changed files")
    } else {
        format!(
            "pattern version {PATTERN_VERSION}; examined {} harness file(s): {}",
            examined.len(),
            examined.join(", ")
        )
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn configured(js: &[(&str, &str)], rust: &[(&str, bool)]) -> ConfiguredHarness {
        ConfiguredHarness {
            js_setup: js
                .iter()
                .map(|(file, config)| (file.to_string(), config.to_string()))
                .collect(),
            rust_targets: rust
                .iter()
                .map(|(file, own_main)| (file.to_string(), *own_main))
                .collect(),
            rust_modules: BTreeMap::new(),
            pytest_plugins: BTreeMap::new(),
            unread: Vec::new(),
        }
    }

    fn roles_of(path: &str, configured: &ConfiguredHarness) -> Roles {
        roles(
            path,
            configured,
            &default_registry(),
            &["checks/**".to_string()],
        )
    }

    #[test]
    fn a_file_is_a_harness_file_by_its_name_or_by_a_configuration_never_by_a_word() {
        let none = ConfiguredHarness::default();
        let conftest = roles_of("pkg/tests/conftest.py", &none);
        assert!(conftest.pytest_hooks && conftest.python_exit && conftest.unittest);
        let startup = roles_of("sitecustomize.py", &none);
        assert!(startup.python_exit && startup.unittest && !startup.pytest_hooks);
        assert!(roles_of("pkg/a_test.go", &none).go_test);
        assert!(roles_of("web/jest.config.ts", &none).js_exit);
        assert!(roles_of("vitest.config.mts", &none).js_exit);
        // A Python test file: the `unittest` forms and nothing else.
        for path in ["tests/test_io.py", "pkg/io_test.py", "checks/smoke.py"] {
            let test = roles_of(path, &none);
            assert!(
                test.unittest && !test.python_exit && !test.pytest_hooks,
                "{path}"
            );
        }
        // Not harness files: a word in the path is not how a runner finds a file.
        for path in [
            "src/harness.py",
            "src/conftest_helpers.py",
            "tools/setup.js",
            "test/globalSetup.js",
            "tests/it.rs",
            "pkg/testdata/a_test.go",
            "pkg/_skip_test.go",
            "node_modules/x/jest.config.js",
            "src/main.go",
            "docs/conftest.py.md",
        ] {
            assert!(!roles_of(path, &none).any(), "{path}");
        }
        // The same files, once a configuration names them.
        let named = configured(
            &[("test/globalSetup.js", "jest.config.json")],
            &[("tests/it.rs", false), ("tests/own.rs", true)],
        );
        let setup = roles_of("test/globalSetup.js", &named);
        assert!(setup.js_exit && setup.why.contains("jest.config.json"));
        assert!(roles_of("tests/it.rs", &named).rust_exit);
        assert!(roles_of("tests/own.rs", &named)
            .why
            .contains("harness = false"));
        assert!(!roles_of("tools/setup.js", &named).any());

        let mut with_extensions = named;
        with_extensions.pytest_plugins.insert(
            "tests/plugins/tamper.py".to_string(),
            "conftest.py".to_string(),
        );
        with_extensions
            .rust_modules
            .insert("tests/common/mod.rs".to_string(), "tests/it.rs".to_string());
        let plugin = roles_of("tests/plugins/tamper.py", &with_extensions);
        assert!(plugin.pytest_hooks && plugin.python_exit && plugin.unittest);
        assert!(plugin.why.contains("conftest.py"));
        let rust_mod = roles_of("tests/common/mod.rs", &with_extensions);
        assert!(rust_mod.rust_exit);
        assert!(rust_mod.why.contains("tests/it.rs"));
    }

    fn site(form: Form, subject: &str, line: usize) -> Site {
        Site {
            form,
            line,
            subject: subject.to_string(),
            what: String::new(),
        }
    }

    #[test]
    fn only_the_forms_the_change_adds_are_new() {
        let base = vec![site(Form::ExitZero, "teardown", 4)];
        // The same form in the same function, moved: not new.
        assert!(new_sites(&base, vec![site(Form::ExitZero, "teardown", 9)]).is_empty());
        // A second one in the same function, the same form in another function, and
        // another form: each new.
        let head = vec![
            site(Form::ExitZero, "teardown", 4),
            site(Form::ExitZero, "teardown", 6),
            site(Form::ExitZero, "setup", 12),
            site(Form::PytestOutcomeAssigned, "teardown", 20),
        ];
        let lines: Vec<usize> = new_sites(&base, head).iter().map(|s| s.line).collect();
        assert_eq!(lines, vec![6, 12, 20]);
        assert_eq!(new_sites(&[], vec![site(Form::ExitZero, "f", 1)]).len(), 1);
    }

    #[test]
    fn a_scan_reads_only_the_forms_of_the_files_roles() {
        let src = "import os\nos._exit(0)\n\ndef pytest_runtest_logreport(report):\n    report.outcome = \"passed\"\n";
        let none = ConfiguredHarness::default();
        let conftest = scan("conftest.py", src, &roles_of("conftest.py", &none)).unwrap();
        assert_eq!(conftest.scan.sites.len(), 2);
        let startup = scan(
            "sitecustomize.py",
            src,
            &roles_of("sitecustomize.py", &none),
        )
        .unwrap();
        assert_eq!(startup.scan.sites.len(), 1);
        let test = scan("tests/test_a.py", src, &roles_of("tests/test_a.py", &none)).unwrap();
        assert!(test.scan.sites.is_empty());
        assert_eq!(
            scan("lib.py", src, &Roles::default()).unwrap(),
            FileScan::default()
        );
    }

    #[test]
    fn only_the_exit_form_is_held_at_warning_and_every_form_has_a_finding() {
        let forms = [
            Form::GoRunNeverCalled,
            Form::GoResultDiscarded,
            Form::PytestOutcomeAssigned,
            Form::PytestItemsRemoved,
            Form::UnittestMethodAssigned,
            Form::UnittestMethodNeutralised,
            Form::ExitZero,
        ];
        for form in forms {
            let (kind, held_at_warning, fix) = finding(form);
            assert_eq!(kind.gates, &[GATE]);
            assert_eq!(held_at_warning, form == Form::ExitZero, "{form:?}");
            assert!(!fix.is_empty());
        }
        let codes: HashSet<&str> = forms.iter().map(|f| finding(*f).0.code).collect();
        assert_eq!(codes.len(), 4);
    }
}
