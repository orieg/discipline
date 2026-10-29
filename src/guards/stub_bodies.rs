//! `stub-bodies`: a function that says it is not implemented, or a body emptied out.
//!
//! Faced with a type error or an awkward edge case, an agent will ship the signature and
//! leave `todo!()`, `raise NotImplementedError` or `throw new Error("not implemented")`
//! as the body, or replace a working body with `return null`. No test-facing gate sees
//! that: no test was deleted and no assertion dropped. This gate reads the function facts
//! the language packs supply (`Fact::Functions`) and judges two things, base against head:
//!
//! - an added non-test function whose whole body is a stub marker;
//! - an existing function whose base body did something and whose head body is a stub,
//!   empty, or a bare constant return.
//!
//! It also reports a read of whether a test runner is loaded (`"pytest" in sys.modules`,
//! `process.env.JEST_WORKER_ID`, `cfg!(test)`, `testing.Testing()`) that the change adds
//! to production code (`Fact::Functions` packs that read them, [`crate::ast::runner_probes`]):
//! a function can then return what its tests expect only while they run. A probe the base
//! side held in the same function is not reported again, nor one in a file the runner
//! loads as a harness. These are held at `warning` until a precision sample says more.
//!
//! A pack that does not supply function facts leaves its files named as not analysed.

use super::{Context, GateOutcome, PathFilter};
use crate::ast::functions::{BodyShape, FunctionFacts};
use crate::ast::runner_collection::{named_harness, ConfiguredHarness};
use crate::ast::runner_probes;
use crate::ast::{default_registry, Fact};
use crate::config::GateSettings;
use crate::tokens;
use anyhow::Result;
use std::collections::HashMap;

pub const GATE: &str = "stub-bodies";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub kind: &'static crate::findings::FindingKind,
    pub name: String,
    pub type_name: Option<String>,
    pub line: usize,
    pub what: String,
    /// Reported at `warning` at most: an equality method the change adds, or one in test
    /// code. An existing production equality method whose substantive body is made
    /// constant is reported at the gate's own severity, as any other body replaced by a
    /// stub.
    pub capped_at_warning: bool,
}

fn shape_label(shape: &BodyShape) -> String {
    match shape {
        BodyShape::Stub(m) => format!("stub `{m}`"),
        BodyShape::Empty => "an empty body".to_string(),
        BodyShape::Trivial(t) => format!("bare `{t}`"),
        BodyShape::Substantive => "a body".to_string(),
    }
}

/// Judge one file's functions, head against base. Functions pair by enclosing type and
/// name. Within each, a head function pairs first with a base function of the same body,
/// in source order (an unchanged function, wherever the change put others of its name);
/// the rest pair by order within the gap between the same two such pairs, so a function
/// added in one gap does not take the base function of another (#910). A file of many
/// object-literal `get()` methods keeps its pairs when one is added.
pub fn judge(base: &[FunctionFacts], head: &[FunctionFacts]) -> Vec<Finding> {
    type Key<'a> = (Option<&'a str>, &'a str);
    fn key(f: &FunctionFacts) -> Key<'_> {
        (f.type_name.as_deref(), f.name.as_str())
    }
    let mut base_by_key: HashMap<Key, Vec<&FunctionFacts>> = HashMap::new();
    for b in base {
        base_by_key.entry(key(b)).or_default().push(b);
    }
    let judged: Vec<&FunctionFacts> = head
        .iter()
        .filter(|h| !h.is_test || crate::ast::functions::is_equality_name(&h.name))
        .collect();
    let mut head_by_key: HashMap<Key, Vec<usize>> = HashMap::new();
    for (i, h) in judged.iter().enumerate() {
        head_by_key.entry(key(h)).or_default().push(i);
    }
    let mut paired: Vec<Option<&FunctionFacts>> = vec![None; judged.len()];
    for (k, heads) in &head_by_key {
        let bases = base_by_key.get(k).map(Vec::as_slice).unwrap_or_default();
        // Anchors: same-body pairs, increasing on both sides.
        let mut anchors: Vec<(usize, usize)> = Vec::new();
        let mut next_base = 0;
        for (hi, &h) in heads.iter().enumerate() {
            if let Some(off) = bases[next_base..]
                .iter()
                .position(|b| b.body_digest == judged[h].body_digest)
            {
                anchors.push((hi, next_base + off));
                next_base += off + 1;
            }
        }
        // Each gap between anchors (and before the first, after the last) pairs by order.
        let mut from = (0, 0);
        for &(hi, bi) in anchors
            .iter()
            .chain(std::iter::once(&(heads.len(), bases.len())))
        {
            for (h, b) in heads[from.0..hi].iter().zip(&bases[from.1..bi]) {
                paired[*h] = Some(b);
            }
            if hi < heads.len() {
                paired[heads[hi]] = Some(bases[bi]);
            }
            from = (hi + 1, bi + 1);
        }
    }
    let mut found = Vec::new();
    for (h, b) in judged.into_iter().zip(paired) {
        let is_eq = crate::ast::functions::is_equality_name(&h.name);
        match b {
            None => {
                if is_eq
                    && crate::ast::functions::is_constant_true_shape(&h.shape)
                    && !h.equality_carved_out
                {
                    found.push(Finding {
                        kind: &crate::findings::EQUALITY_MADE_CONSTANT,
                        name: h.name.clone(),
                        type_name: h.type_name.clone(),
                        line: h.line,
                        what: format!(
                            "`{}` is an equality method added returning constant `true`",
                            h.name
                        ),
                        capped_at_warning: true,
                    });
                } else if let BodyShape::Stub(m) = &h.shape {
                    if !h.is_test {
                        found.push(Finding {
                            kind: &crate::findings::STUB_BODY_ADDED,
                            name: h.name.clone(),
                            type_name: h.type_name.clone(),
                            line: h.line,
                            what: format!(
                                "`{}` is added with the body `{m}` and nothing else",
                                h.name
                            ),
                            capped_at_warning: false,
                        });
                    }
                }
            }
            Some(b) => {
                if is_eq
                    && crate::ast::functions::is_constant_true_shape(&h.shape)
                    && !h.equality_carved_out
                {
                    if !crate::ast::functions::is_constant_true_shape(&b.shape) {
                        found.push(Finding {
                            kind: &crate::findings::EQUALITY_MADE_CONSTANT,
                            name: h.name.clone(),
                            type_name: h.type_name.clone(),
                            line: h.line,
                            what: format!(
                                "`{}` is an equality method that now returns constant `true`",
                                h.name
                            ),
                            capped_at_warning: h.is_test || b.shape != BodyShape::Substantive,
                        });
                    }
                } else if b.shape == BodyShape::Substantive
                    && h.shape != BodyShape::Substantive
                    && !h.is_test
                {
                    found.push(Finding {
                        kind: &crate::findings::BODY_REPLACED_BY_STUB,
                        name: h.name.clone(),
                        type_name: h.type_name.clone(),
                        line: h.line,
                        what: format!(
                            "`{}` had a body on the base side and now has {}",
                            h.name,
                            shape_label(&h.shape)
                        ),
                        capped_at_warning: false,
                    });
                }
            }
        }
    }
    found
}

pub fn stub_bodies(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.stub_bodies;
    let mut out = GateOutcome::new(GATE);
    let exempt = PathFilter::new(&settings.exempt_paths)?;
    let registry = default_registry();
    let vocab = super::agent_diff::assert_vocabulary(ctx.config);
    let mut unsupported: Vec<String> = Vec::new();
    // The files a runner configuration loads around the tests, read once a probe needs it.
    let mut configured: Option<ConfiguredHarness> = None;

    let changed = ctx.git.changed_files()?;
    for (file, pack) in super::ast_changes(
        &changed,
        &exempt,
        &registry,
        Fact::Functions,
        &mut unsupported,
    ) {
        let Some(head_src) = ctx.git.head_content(&file.path)? else {
            out.notes.push(super::unread_note(&file.path));
            continue;
        };
        // Base-anchored classification, shared with `error-swallowing`: a renamed
        // file is judged by its base path. The move itself is reported once by
        // `error-swallowing` (`test-path-reclassification`), not here.
        let anchored = super::base_anchored_classification(&file, &registry, &vocab.test_paths);
        out.notes.extend(
            anchored
                .language_changed_note
                .into_iter()
                .chain(anchored.declared_scope_note),
        );
        let mut head = match pack.extract(&anchored.classify_path, &head_src, &vocab) {
            Ok(f) => f,
            Err(e) => {
                out.notes.push(format!(
                    "`{}`: not analysed, the head side does not parse ({e})",
                    file.path
                ));
                continue;
            }
        };
        let (base, base_probes) = match ctx.git.base_content(&file.old_path)? {
            Some(src) => match pack.extract(&file.old_path, &src, &vocab) {
                Ok(f) => (f.functions, f.runner_probes),
                Err(_) => {
                    out.notes.push(format!(
                        "`{}`: the base side does not parse; every head-side function is judged as new",
                        file.path
                    ));
                    (Vec::new(), Vec::new())
                }
            },
            None => (Vec::new(), Vec::new()),
        };
        // Read before a file in test scope by its name only is judged as production code
        // below: that is about stubs, and a test path's probes are not production code.
        let head_probes = std::mem::take(&mut head.runner_probes);
        // A file added in test scope by its name only, with no test that checks.
        let by_name_only =
            super::name_only_scope(&file, &changed, pack, &registry, &vocab.test_paths, &head);
        if let Some(by_name) = by_name_only {
            match &by_name.classify_path {
                None => out.notes.push(by_name.note),
                Some(classify_path) => {
                    if let Some(mut production) = pack
                        .extract(classify_path, &head_src, &vocab)
                        .ok()
                        .filter(|facts| !judge(&base, &facts.functions).is_empty())
                    {
                        // A test double (`FakeClock` in `tests/fakes.py`) keeps the
                        // carve-out its real path gives it: judging the file as production
                        // code is about stubs hidden under a test name, and a class named
                        // and placed as a test double may compare equal to anything.
                        for f in &mut production.functions {
                            f.equality_carved_out |= head.functions.iter().any(|h| {
                                h.equality_carved_out
                                    && h.line == f.line
                                    && h.name == f.name
                                    && h.type_name == f.type_name
                            });
                        }
                        out.notes.push(by_name.note);
                        head = production;
                    }
                }
            }
        }
        out.examined += head
            .functions
            .iter()
            .filter(|f| !f.is_test || crate::ast::functions::is_equality_name(&f.name))
            .count();
        for f in judge(&base, &head.functions) {
            let lift = |subject: &str| ctx.find_override(GATE, f.kind, tokens::ALLOW_STUB, subject);
            let ov = lift(&f.name)
                .or_else(|| f.type_name.as_deref().and_then(lift))
                .or_else(|| lift(&file.path))
                .or_else(|| file.path.rsplit('/').next().and_then(lift));
            let severity = if f.capped_at_warning {
                settings.severity().capped_at_warning()
            } else {
                settings.severity()
            };
            let fix = if f.kind == &crate::findings::EQUALITY_MADE_CONSTANT {
                format!(
                    "Implement a real equality comparison, or justify the marker type on its own line in the PR body or a commit message: `allow-stub: {} <reason>`.",
                    f.type_name.as_deref().unwrap_or(&f.name)
                )
            } else {
                format!(
                    "Implement it, or justify the stub on its own line in the PR body or a commit message: `allow-stub: {} <reason>`.",
                    f.name
                )
            };
            out.lift_or_push(
                ov,
                ctx.overridable(severity),
                f.kind,
                (Some(&file.path), Some(f.line)),
                format!("{} in `{}`.", f.what, file.path),
                &fix,
            );
        }
        let probes = runner_probes::new_probes(&base_probes, &head_probes);
        if probes.is_empty() || named_harness(&file.path).is_some() {
            continue;
        }
        let configured = match &mut configured {
            Some(c) => c,
            None => {
                let tracked = ctx.git.tracked_files()?;
                let reads = crate::gitctx::ReadRecorder::new();
                let c = ConfiguredHarness::from_tree(reads.head(ctx.git), &tracked);
                reads.finish()?;
                configured.insert(c)
            }
        };
        if configured.js_setup.contains_key(&file.path)
            || configured.pytest_plugins.contains_key(&file.path)
            || configured.rust_targets.contains_key(&file.path)
            || configured.rust_modules.contains_key(&file.path)
        {
            continue;
        }
        let file_name = file.path.rsplit('/').next().unwrap_or(&file.path);
        let kind = &crate::findings::TEST_RUNNER_PROBE_ADDED;
        for probe in probes {
            // The probe's own `path:line` first, then the file by its path or its name.
            let whole_file = |subject: &str| {
                ctx.find_whole_file_override(GATE, kind, tokens::ALLOW_STUB, subject)
            };
            let lifted = [
                format!("{}:{}", file.path, probe.line),
                format!("{file_name}:{}", probe.line),
            ]
            .iter()
            .find_map(|own| ctx.find_override(GATE, kind, tokens::ALLOW_STUB, own))
            .or_else(|| whole_file(&file.path))
            .or_else(|| whole_file(file_name));
            out.lift_or_push(
                lifted,
                ctx.overridable(settings.severity().capped_at_warning()),
                kind,
                (Some(&file.path), Some(probe.line)),
                format!(
                    "{} is added to production code at `{}:{}`: the code can behave one way under its tests and another in use.",
                    probe.signal.label(),
                    file.path,
                    probe.line
                ),
                &format!(
                    "Remove the probe, or justify it on its own line in the PR body or a commit message: `allow-stub: {}:{} <reason>`.",
                    file.path, probe.line
                ),
            );
        }
    }
    out.notes
        .extend(super::unsupported_fact_note(&unsupported, "function"));
    if out.examined == 0 && unsupported.is_empty() {
        out.notes
            .push("no functions in changed files of an analysed language".to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A function whose body is unlike any other's: each call gets its own digest, so
    /// these functions pair by order, as two bodies that differ do.
    fn f(name: &str, line: usize, shape: BodyShape, is_test: bool) -> FunctionFacts {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        FunctionFacts {
            name: name.into(),
            line,
            end_line: line,
            shape,
            is_test,
            type_name: None,
            equality_carved_out: false,
            body_digest: NEXT.fetch_add(1, Ordering::Relaxed),
        }
    }

    #[test]
    fn added_stub_is_reported_and_added_noop_or_test_is_not() {
        let head = vec![
            f("a", 1, BodyShape::Stub("todo!()".into()), false),
            f("b", 2, BodyShape::Empty, false),
            f("c", 3, BodyShape::Trivial("None".into()), false),
            f("t", 4, BodyShape::Stub("todo!()".into()), true),
        ];
        let got = judge(&[], &head);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].kind.title, "Stub Body Added");
        assert_eq!(got[0].name, "a");
    }

    #[test]
    fn a_body_replaced_by_anything_less_is_reported_and_the_reverse_is_not() {
        let base = vec![
            f("a", 1, BodyShape::Substantive, false),
            f("b", 5, BodyShape::Substantive, false),
            f("c", 9, BodyShape::Substantive, false),
            f("d", 12, BodyShape::Stub("todo!()".into()), false),
        ];
        let head = vec![
            f("a", 1, BodyShape::Stub("unimplemented!()".into()), false),
            f("b", 5, BodyShape::Empty, false),
            f("c", 9, BodyShape::Trivial("return None".into()), false),
            f("d", 12, BodyShape::Substantive, false),
        ];
        let got = judge(&base, &head);
        let names: Vec<&str> = got.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        assert!(got
            .iter()
            .all(|g| g.kind.title == "Function Body Replaced By Stub"));
        assert!(judge(&head, &base).iter().all(|g| g.name == "d"));
    }

    #[test]
    fn overloads_pair_by_order_and_a_moved_function_is_unchanged() {
        let base = vec![
            f("m", 1, BodyShape::Substantive, false),
            f("m", 8, BodyShape::Trivial("return null".into()), false),
        ];
        let head = vec![
            f("m", 20, BodyShape::Substantive, false),
            f("m", 30, BodyShape::Trivial("return null".into()), false),
        ];
        assert!(judge(&base, &head).is_empty());
        let swapped = vec![
            f("m", 20, BodyShape::Trivial("return null".into()), false),
            f("m", 30, BodyShape::Substantive, false),
        ];
        assert_eq!(judge(&base, &swapped).len(), 1);
    }

    #[test]
    fn added_equality_returning_true_is_reported_and_carved_out_or_other_trivial_is_not() {
        let mut carved = f("__eq__", 1, BodyShape::Trivial("return True".into()), false);
        carved.equality_carved_out = true;

        let head = vec![
            f("__eq__", 2, BodyShape::Trivial("return True".into()), false),
            carved,
            f(
                "__eq__",
                3,
                BodyShape::Trivial("return False".into()),
                false,
            ),
            f("__ne__", 4, BodyShape::Trivial("return True".into()), false),
            f("other", 5, BodyShape::Trivial("return True".into()), false),
        ];
        let got = judge(&[], &head);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].kind.title, "Equality Method Made Constant");
        assert_eq!(got[0].line, 2);
    }

    #[test]
    fn changed_equality_made_constant_is_reported() {
        let base = vec![
            f("__eq__", 1, BodyShape::Substantive, false),
            f("eq", 5, BodyShape::Trivial("true".into()), false),
        ];
        let head = vec![
            f("__eq__", 1, BodyShape::Trivial("return True".into()), false),
            f("eq", 5, BodyShape::Trivial("true".into()), false),
        ];
        let got = judge(&base, &head);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].kind.title, "Equality Method Made Constant");
        assert_eq!(got[0].name, "__eq__");
        assert!(
            !got[0].capped_at_warning,
            "a substantive production body made constant keeps the gate's severity"
        );
    }

    #[test]
    fn only_a_changed_production_equality_keeps_the_gate_severity() {
        let added = judge(
            &[],
            &[f(
                "__eq__",
                1,
                BodyShape::Trivial("return True".into()),
                false,
            )],
        );
        assert!(added[0].capped_at_warning, "added: {added:?}");
        let in_test = judge(
            &[f("__eq__", 1, BodyShape::Substantive, true)],
            &[f(
                "__eq__",
                1,
                BodyShape::Trivial("return True".into()),
                true,
            )],
        );
        assert!(in_test[0].capped_at_warning, "test code: {in_test:?}");
        let from_trivial = judge(
            &[f(
                "__eq__",
                1,
                BodyShape::Trivial("return False".into()),
                false,
            )],
            &[f(
                "__eq__",
                1,
                BodyShape::Trivial("return True".into()),
                false,
            )],
        );
        assert!(from_trivial[0].capped_at_warning, "{from_trivial:?}");
    }

    /// A function added before others of its name does not shift their pairs: a head
    /// function pairs first with a base function of the same body (#910). The shape of
    /// `denoland/deno#33403`: a new `get()` added above a `get() { return true; }` and a
    /// substantive `get()`, all unchanged. The controls are still reported: a body
    /// replaced in place beside an unchanged one, and the same with an addition before
    /// them.
    #[test]
    fn an_added_function_does_not_shift_the_pairs_of_unchanged_ones() {
        let with = |mut g: FunctionFacts, digest: u64| {
            g.body_digest = digest;
            g
        };
        let trivial = || BodyShape::Trivial("return true".into());
        let base = [
            with(f("get", 10, trivial(), false), 1),
            with(f("get", 20, BodyShape::Substantive, false), 2),
        ];
        let head = [
            with(f("get", 5, BodyShape::Substantive, false), 3),
            with(f("get", 15, trivial(), false), 1),
            with(f("get", 25, BodyShape::Substantive, false), 2),
        ];
        assert_eq!(judge(&base, &head), vec![]);

        // Controls: the substantive `get` is replaced in place, with and without a `get`
        // added before it; the unchanged one keeps its pair either way.
        let replaced = [
            with(f("get", 15, trivial(), false), 1),
            with(f("get", 25, trivial(), false), 4),
        ];
        let found = judge(&base, &replaced);
        assert_eq!(
            found
                .iter()
                .map(|x| (x.kind.code, x.line))
                .collect::<Vec<_>>(),
            vec![("body-replaced-by-stub", 25)]
        );
        let added_and_replaced = [
            with(f("get", 5, BodyShape::Substantive, false), 3),
            with(f("get", 15, trivial(), false), 1),
            with(f("get", 25, trivial(), false), 4),
        ];
        let found = judge(&base, &added_and_replaced);
        assert_eq!(
            found
                .iter()
                .map(|x| (x.kind.code, x.line))
                .collect::<Vec<_>>(),
            vec![("body-replaced-by-stub", 25)]
        );

        // A trivial `get` added just before an unchanged substantive one is an added
        // function: it does not take the base function of the pair after it.
        let base = [
            with(f("get", 10, BodyShape::Substantive, false), 9),
            with(f("get", 20, BodyShape::Substantive, false), 2),
        ];
        let head = [
            with(f("get", 5, BodyShape::Substantive, false), 3),
            with(f("get", 8, trivial(), false), 4),
            with(f("get", 20, BodyShape::Substantive, false), 2),
        ];
        assert_eq!(judge(&base, &head), vec![]);

        // An unchanged function whose body is already a stub is not added: a `get` added
        // above it does not make it `stub-body-added`.
        let todo = || BodyShape::Stub("todo!()".into());
        let base = [with(f("get", 10, todo(), false), 5)];
        let head = [
            with(f("get", 5, BodyShape::Substantive, false), 3),
            with(f("get", 15, todo(), false), 5),
        ];
        assert_eq!(judge(&base, &head), vec![]);
    }

    #[test]
    fn methods_on_different_types_do_not_cross_pair() {
        let mut base_user = f("__eq__", 1, BodyShape::Substantive, false);
        base_user.type_name = Some("User".into());
        let base = vec![base_user];

        let mut head_user = f("__eq__", 1, BodyShape::Substantive, false);
        head_user.type_name = Some("User".into());

        let mut head_account = f(
            "__eq__",
            10,
            BodyShape::Trivial("return True".into()),
            false,
        );
        head_account.type_name = Some("Account".into());

        let head = vec![head_user, head_account];
        let got = judge(&base, &head);
        assert_eq!(
            got.len(),
            1,
            "Account::__eq__ must be detected as added equality constant: {got:?}"
        );
        assert_eq!(got[0].type_name.as_deref(), Some("Account"));
    }
}
