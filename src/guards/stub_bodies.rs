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
//! A pack that does not supply function facts leaves its files named as not analysed.

use super::{Context, GateOutcome, PathFilter};
use crate::ast::functions::{BodyShape, FunctionFacts};
use crate::ast::{default_registry, AssertVocabulary, Fact};
use crate::config::GateSettings;
use crate::gitctx::ChangeKind;
use crate::tokens;
use anyhow::Result;
use std::collections::HashMap;

pub const GATE: &str = "stub-bodies";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub title: &'static str,
    pub name: String,
    pub line: usize,
    pub what: String,
}

fn shape_label(shape: &BodyShape) -> String {
    match shape {
        BodyShape::Stub(m) => format!("stub `{m}`"),
        BodyShape::Empty => "an empty body".to_string(),
        BodyShape::Trivial(t) => format!("bare `{t}`"),
        BodyShape::Substantive => "a body".to_string(),
    }
}

/// Judge one file's functions, head against base. Functions pair by name and order:
/// the n-th `f` on the head side is the n-th `f` on the base side.
pub fn judge(base: &[FunctionFacts], head: &[FunctionFacts]) -> Vec<Finding> {
    let mut by_name: HashMap<&str, Vec<&FunctionFacts>> = HashMap::new();
    for f in base {
        by_name.entry(f.name.as_str()).or_default().push(f);
    }
    let mut taken: HashMap<&str, usize> = HashMap::new();
    let mut found = Vec::new();
    for h in head {
        if h.is_test {
            continue;
        }
        let idx = taken.entry(h.name.as_str()).or_default();
        let b = by_name.get(h.name.as_str()).and_then(|v| v.get(*idx));
        *idx += 1;
        match b {
            None => {
                if let BodyShape::Stub(m) = &h.shape {
                    found.push(Finding {
                        title: "Stub Body Added",
                        name: h.name.clone(),
                        line: h.line,
                        what: format!("`{}` is added with the body `{m}` and nothing else", h.name),
                    });
                }
            }
            Some(b) => {
                if b.shape == BodyShape::Substantive && h.shape != BodyShape::Substantive {
                    found.push(Finding {
                        title: "Function Body Replaced By Stub",
                        name: h.name.clone(),
                        line: h.line,
                        what: format!(
                            "`{}` had a body on the base side and now has {}",
                            h.name,
                            shape_label(&h.shape)
                        ),
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
    let vocab = AssertVocabulary::default();
    let mut unsupported: Vec<String> = Vec::new();

    for file in ctx.git.changed_files()? {
        if file.kind == ChangeKind::Deleted || exempt.matches(&file.path) {
            continue;
        }
        let Some(pack) = registry.find_pack(&file.path) else {
            continue;
        };
        if !pack.supplies(Fact::Functions) {
            unsupported.push(file.path.clone());
            continue;
        }
        let Some(head_src) = ctx.git.head_content(&file.path)? else {
            continue;
        };
        let head = match pack.extract(&file.path, &head_src, &vocab) {
            Ok(f) => f,
            Err(e) => {
                out.notes.push(format!(
                    "`{}`: not analysed, the head side does not parse ({e})",
                    file.path
                ));
                continue;
            }
        };
        let base = match ctx.git.base_content(&file.old_path)? {
            Some(src) => pack
                .extract(&file.old_path, &src, &vocab)
                .map(|f| f.functions)
                .unwrap_or_default(),
            None => Vec::new(),
        };
        out.examined += head.functions.iter().filter(|f| !f.is_test).count();
        for f in judge(&base, &head.functions) {
            let lift = |subject: &str| ctx.find_override(GATE, tokens::ALLOW_STUB, subject);
            if let Some(ov) = lift(&f.name)
                .or_else(|| lift(&file.path))
                .or_else(|| file.path.rsplit('/').next().and_then(lift))
            {
                out.overrides.push(ov);
                continue;
            }
            out.push(
                ctx.overridable(settings.severity()),
                f.title,
                Some(&file.path),
                Some(f.line),
                format!("{} in `{}`.", f.what, file.path),
                &format!(
                    "Implement it, or justify the stub on its own line in the PR body or a commit message: `allow-stub: {} <reason>`.",
                    f.name
                ),
            );
        }
    }
    if !unsupported.is_empty() {
        let sample: Vec<&str> = unsupported.iter().take(3).map(String::as_str).collect();
        out.notes.push(format!(
            "{} changed file(s) are in a language whose pack supplies no function facts and were NOT analysed (e.g. {})",
            unsupported.len(),
            sample.join(", ")
        ));
    }
    if out.examined == 0 && unsupported.is_empty() {
        out.notes
            .push("no functions in changed files of an analysed language".to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str, line: usize, shape: BodyShape, is_test: bool) -> FunctionFacts {
        FunctionFacts {
            name: name.into(),
            line,
            end_line: line,
            shape,
            is_test,
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
        assert_eq!(got[0].title, "Stub Body Added");
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
            .all(|g| g.title == "Function Body Replaced By Stub"));
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
}
