//! The assertion vocabulary the diff gates extract facts with, and the runner collection
//! rules of each side, built once per run.

use crate::ast::AssertVocabulary;
use crate::guards::Context;
use anyhow::Result;
// The head-side cache test below runs the gates through `super::run`.
#[cfg(test)]
use super::run;

/// The assertion vocabulary the agent-guard gates extract facts with.
pub(crate) fn assert_vocabulary(config: &crate::config::DisciplineConfig) -> AssertVocabulary {
    let gates = &config.gates;
    AssertVocabulary {
        extra_macros: [
            &gates.assertion_reduction.extra_assert_macros[..],
            &gates.vacuous_tests.extra_assert_macros[..],
        ]
        .concat(),
        helper_fns: [
            &gates.assertion_reduction.assert_helper_fns[..],
            &gates.vacuous_tests.assert_helper_fns[..],
        ]
        .concat(),
        safety_placeholders: gates.unsafe_safety_comment.placeholders.clone(),
        mock_setup_fns: [
            &gates.assertion_reduction.mock_setup_fns[..],
            &gates.vacuous_tests.mock_setup_fns[..],
        ]
        .concat(),
        mock_assert_fns: [
            &gates.assertion_reduction.mock_assert_fns[..],
            &gates.vacuous_tests.mock_assert_fns[..],
        ]
        .concat(),
        test_functions: config.tests.functions.clone(),
        test_paths: config.tests.paths.clone(),
        c_macros: config.languages.c.macros.clone(),
        c_function_macros: config.languages.c.function_macros.clone(),
        runner_rules: Default::default(),
    }
}

pub(crate) fn assert_vocabulary_for_head(ctx: &Context) -> Result<AssertVocabulary> {
    let mut vocab = assert_vocabulary(ctx.config);
    vocab.runner_rules = head_runner_rules(ctx)?;
    Ok(vocab)
}

#[cfg(test)]
thread_local! {
    /// Every path a build of the head-side rules read, in order, for the tests that
    /// count builds and reads.
    static HEAD_RULE_READS: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

thread_local! {
    /// The runner collection rules of the head side of the last run that asked for
    /// them, by the run's id.
    static HEAD_RUNNER_RULES: RulesSlot = const { std::cell::RefCell::new(None) };
}

/// The runner collection rules of the head side. They are read from the working tree
/// (the index, for a staged run) and from nothing else, and neither changes during a
/// run, so the rules built for one opened repository are the rules of every later
/// request in that run: the diff gates and `test-floor` read each manifest, and each
/// JavaScript or TypeScript file for its `node:test` import, once. The working tree has
/// no object id to name it, so the key is the run itself ([`crate::gitctx::GitCtx::run_id`]),
/// which no other opened repository shares. A build that met a read error is not kept.
fn head_runner_rules(
    ctx: &Context,
) -> Result<crate::ast::runner_collection::RunnerCollectionRules> {
    let run = ctx.git.run_id().to_string();
    rules_for_key(&HEAD_RUNNER_RULES, Some(&run), || {
        let tracked = ctx.git.tracked_files()?;
        let reads = crate::gitctx::ReadRecorder::new();
        let head = reads.head(ctx.git);
        let rules = crate::ast::runner_collection::RunnerCollectionRules::from_tree(
            |path| {
                #[cfg(test)]
                HEAD_RULE_READS.with(|reads| reads.borrow_mut().push(path.to_string()));
                head(path).or_else(|| {
                    std::fs::read_to_string(std::path::Path::new(ctx.git.root()).join(path)).ok()
                })
            },
            &tracked,
        );
        reads.finish()?;
        Ok(rules)
    })
}

/// A base configuration that does not load falls back to the head vocabulary:
/// `config-integrity` reports that case (`base-configuration-unreadable`) and cannot be
/// switched off by the change while it holds.
pub(crate) fn assert_vocabulary_for_base(ctx: &Context) -> Result<AssertVocabulary> {
    let base_cfg = ctx
        .base_config_text()?
        .and_then(|s| crate::config::DisciplineConfig::from_toml_str(&s).ok());
    let mut vocab = assert_vocabulary(base_cfg.as_ref().unwrap_or(ctx.config));
    vocab.runner_rules = base_runner_rules(ctx)?;
    Ok(vocab)
}

/// One kept set of runner collection rules, with the key it was built for.
type RulesSlot =
    std::cell::RefCell<Option<(String, crate::ast::runner_collection::RunnerCollectionRules)>>;

thread_local! {
    /// The runner collection rules of the last base tree read, by the tree's object id.
    static BASE_RUNNER_RULES: RulesSlot = const { std::cell::RefCell::new(None) };
}

/// The rules kept for the tree `id`, or those `build` returns, which are then kept for
/// that id in place of any other tree's. With no id (the empty tree) nothing is kept.
/// A build that fails keeps nothing.
fn rules_for_tree(
    id: Option<&str>,
    build: impl FnOnce() -> Result<crate::ast::runner_collection::RunnerCollectionRules>,
) -> Result<crate::ast::runner_collection::RunnerCollectionRules> {
    rules_for_key(&BASE_RUNNER_RULES, id, build)
}

/// As [`rules_for_tree`], in the slot `slot` and for any key that names what the rules
/// were read from.
fn rules_for_key(
    slot: &'static std::thread::LocalKey<RulesSlot>,
    id: Option<&str>,
    build: impl FnOnce() -> Result<crate::ast::runner_collection::RunnerCollectionRules>,
) -> Result<crate::ast::runner_collection::RunnerCollectionRules> {
    let Some(id) = id else {
        return build();
    };
    let kept = slot.with(|cache| {
        cache
            .borrow()
            .as_ref()
            .filter(|(kept_id, _)| kept_id == id)
            .map(|(_, rules)| rules.clone())
    });
    if let Some(rules) = kept {
        return Ok(rules);
    }
    let rules = build()?;
    slot.with(|cache| *cache.borrow_mut() = Some((id.to_string(), rules.clone())));
    Ok(rules)
}

/// The runner collection rules of the base side. They are read from the base tree and
/// from nothing else, and a tree's object id names its whole content, so the rules
/// built for one id are the rules of every later request for it in the run: the gates
/// that need them (the diff gates, then `test-floor`) read each manifest once. A build
/// that met a read error is not kept.
fn base_runner_rules(
    ctx: &Context,
) -> Result<crate::ast::runner_collection::RunnerCollectionRules> {
    let tree = ctx.git.base_tree_id()?;
    rules_for_tree(tree.as_deref(), || {
        let tracked = ctx.git.base_tracked_files()?;
        let reads = crate::gitctx::ReadRecorder::new();
        let rules = crate::ast::runner_collection::RunnerCollectionRules::from_tree(
            reads.base(ctx.git),
            &tracked,
        );
        reads.finish()?;
        Ok(rules)
    })
}

#[cfg(test)]
mod base_rules_cache_tests {
    use super::rules_for_tree;
    use crate::ast::runner_collection::RunnerCollectionRules;
    use std::cell::Cell;

    /// Rules that differ by the one tracked `go.mod` directory they hold.
    fn rules_with(module: &str) -> RunnerCollectionRules {
        let mut rules = RunnerCollectionRules::default();
        rules.go.modules.push(module.to_string());
        rules
    }

    #[test]
    fn rules_are_built_once_per_tree_id_and_never_served_for_another() {
        let builds = Cell::new(0);
        let build = |module: &'static str| {
            let builds = &builds;
            move || {
                builds.set(builds.get() + 1);
                Ok(rules_with(module))
            }
        };
        // The ids are unique to this test: the kept slot belongs to the thread.
        let first = rules_for_tree(Some("cache-test-tree-a"), build("a")).unwrap();
        assert_eq!(first, rules_with("a"));
        assert_eq!(builds.get(), 1);
        // The same id again: not built, and the same rules.
        let again = rules_for_tree(Some("cache-test-tree-a"), build("never")).unwrap();
        assert_eq!(again, rules_with("a"));
        assert_eq!(builds.get(), 1);
        // Another id: built, and its own rules, not the kept ones.
        let second = rules_for_tree(Some("cache-test-tree-b"), build("b")).unwrap();
        assert_eq!(second, rules_with("b"));
        assert_eq!(builds.get(), 2);
        // Back to the first id: one slot is kept, so it is built again, never served
        // from the other tree.
        let back = rules_for_tree(Some("cache-test-tree-a"), build("a")).unwrap();
        assert_eq!(back, rules_with("a"));
        assert_eq!(builds.get(), 3);
        // No id (the empty tree): built every time, and nothing kept for it.
        rules_for_tree(None, build("none")).unwrap();
        rules_for_tree(None, build("none")).unwrap();
        assert_eq!(builds.get(), 5);
        // A build that fails keeps nothing: the next request builds.
        let failed = rules_for_tree(Some("cache-test-tree-c"), || anyhow::bail!("read error"));
        assert!(failed.is_err());
        let after = rules_for_tree(Some("cache-test-tree-c"), build("c")).unwrap();
        assert_eq!(after, rules_with("c"));
        assert_eq!(builds.get(), 6);
    }
}

#[cfg(test)]
mod head_rules_cache_tests {
    use super::{assert_vocabulary_for_head, HEAD_RULE_READS};
    use crate::gitctx::GitCtx;
    use crate::guards::Context;

    const NODE_TEST: &str = "import test from 'node:test';\ntest('one', () => {});\n";
    const PACKAGE: &str = r#"{"name": "app", "scripts": {"test": "node --test"}}"#;

    /// A repository whose one commit holds a manifest and two JavaScript test files.
    fn repository() -> (tempfile::TempDir, git2::Oid) {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let mut index = repo.index().unwrap();
        for (path, content) in [
            ("package.json", PACKAGE),
            ("test/a.test.js", NODE_TEST),
            ("test/b.test.js", NODE_TEST),
        ] {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, content).unwrap();
            index.add_path(std::path::Path::new(path)).unwrap();
        }
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@example.invalid").unwrap();
        let commit = repo
            .commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
            .unwrap();
        (dir, commit)
    }

    fn context<'a>(config: &'a crate::config::DisciplineConfig, git: &'a GitCtx) -> Context<'a> {
        Context {
            config,
            head_config: None,
            git,
            config_path: "discipline.toml",
            baseline_path: None,
            baseline: None,
            staged: false,
            pr_title: None,
            pr_body: None,
            directives: Vec::new(),
            directive_notes: Vec::new(),
            bench_provenance: None,
            allow_cross_host_bench: false,
            bench_base_file: None,
            bench_head_file: None,
            test_base_report: None,
            test_head_report: None,
            test_report: None,
            forge: None,
        }
    }

    /// How many times the builds so far read `path`.
    fn reads_of(path: &str) -> usize {
        HEAD_RULE_READS.with(|reads| reads.borrow().iter().filter(|p| *p == path).count())
    }

    /// The diff gates and `test-floor` both ask for the head-side rules. In one run the
    /// rules are built once: each manifest, and each JavaScript file for its `node:test`
    /// import, is read once. Another opened repository builds its own.
    #[test]
    fn two_gates_in_one_run_build_the_head_side_rules_once() {
        let (dir, commit) = repository();
        let config = crate::config::DisciplineConfig::default_for_repo("cache-test");
        let open = || GitCtx::for_test(git2::Repository::open(dir.path()).unwrap(), Some(commit));
        HEAD_RULE_READS.with(|reads| reads.borrow_mut().clear());
        let git = open();
        let ctx = context(&config, &git);

        // The diff gates.
        let outcomes = super::run(&ctx).unwrap();
        assert!(!outcomes.is_empty());
        assert_eq!(reads_of("test/a.test.js"), 1);
        assert_eq!(reads_of("test/b.test.js"), 1);
        let manifest_reads = reads_of("package.json");
        assert!(manifest_reads >= 1);
        // `test-floor`, in the same run: nothing is read again.
        let floor = crate::guards::test_floor::evaluate_test_floor(&ctx).unwrap();
        assert_eq!(floor.examined, 2, "{:?}", floor.notes);
        assert_eq!(reads_of("test/a.test.js"), 1);
        assert_eq!(reads_of("test/b.test.js"), 1);
        assert_eq!(reads_of("package.json"), manifest_reads);
        let kept = assert_vocabulary_for_head(&ctx).unwrap();
        assert!(kept.runner_rules.js.node_test_script);
        assert_eq!(reads_of("test/a.test.js"), 1);

        // Another run over a changed working tree reads it again, and sees the change.
        std::fs::write(dir.path().join("package.json"), r#"{"name": "app"}"#).unwrap();
        let later = open();
        let ctx = context(&config, &later);
        let rebuilt = assert_vocabulary_for_head(&ctx).unwrap();
        assert!(!rebuilt.runner_rules.js.node_test_script);
        assert_eq!(reads_of("test/a.test.js"), 2);
    }
}
