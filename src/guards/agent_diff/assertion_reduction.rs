//! `assertion-reduction`: a test, or a helper its tests call, that checks less after the
//! change than before it.

use super::{
    call_names_helper, equality_exits_gained, helper_call_gain, leaf_name,
    leave_helpers_shown_by_tests, lines_of, moved_into_looping_helpers, note_self_comparison_scope,
    FileFacts, HelperPair, Located, TestPair, WeakenedCrateHelper, SELF_COMPARISON_SCOPE,
};
use crate::ast::TestFn;
use crate::config::GateSettings;
use crate::guards::{exempt_filter, GateOutcome, PathFilter};
use crate::tokens;
use anyhow::Result;

const ASSERTION_REDUCTION: &str = "assertion-reduction";

pub fn evaluate_assertion_reduction(
    pairs: &[TestPair],
    added: &[Located],
    helpers: &[HelperPair],
    settings: &crate::config::AssertionGate,
    directives: &[crate::tokens::ParsedDirective],
    is_staged: bool,
) -> Result<GateOutcome> {
    const GATE: &str = ASSERTION_REDUCTION;
    let exempt = exempt_filter(settings)?;
    let mut out = GateOutcome::new(GATE);
    out.examined = pairs.len() + helpers.len();
    let cx = ReductionInputs {
        settings,
        directives,
        is_staged,
    };

    report_caught_in_added_tests(added, &exempt, &cx, &mut out);
    report_weakened_helpers(pairs, added, helpers, &exempt, &cx, &mut out);

    let mut file_cases = FileCases::collect(pairs, added, &exempt);
    for p in pairs.iter().filter(|p| !exempt.matches(p.path)) {
        judge_pair(p, helpers, &mut file_cases, &cx, &mut out);
    }
    Ok(out)
}

/// What every phase of `assertion-reduction` reads besides the tests it judges.
pub struct ReductionInputs<'a> {
    pub settings: &'a crate::config::AssertionGate,
    pub directives: &'a [crate::tokens::ParsedDirective],
    pub is_staged: bool,
}

/// Reports, in each test the change adds, the assertions an enclosing handler catches.
fn report_caught_in_added_tests(
    added: &[Located],
    exempt: &PathFilter,
    cx: &ReductionInputs,
    out: &mut GateOutcome,
) {
    const GATE: &str = ASSERTION_REDUCTION;
    let (settings, directives, is_staged) = (cx.settings, cx.directives, cx.is_staged);
    for a in added.iter().filter(|a| !exempt.matches(a.path)) {
        // A test the change adds is read for swallowed assertions, so it is examined.
        out.examined += 1;
        if a.test.caught_assertions.is_empty() {
            continue;
        }
        let lift = |subject: &str| {
            tokens::find_override(
                directives,
                GATE,
                &crate::findings::ASSERTION_FAILURE_CAUGHT,
                tokens::ALLOW_ASSERTION_DROP,
                subject,
            )
        };
        if let Some(record) = lift(leaf_name(a.test)).or_else(|| lift(a.path)) {
            out.overrides.push(record);
            continue;
        }
        for c in &a.test.caught_assertions {
            out.push(
                if is_staged {
                    crate::config::Severity::Warning
                } else {
                    settings.severity()
                },
                &crate::findings::ASSERTION_FAILURE_CAUGHT,
                Some(a.path),
                Some(c.line),
                format!(
                    "Test `{}`: the assertion on line {} is caught by an enclosing handler (line {}) without failing the test; it is not an effective check.",
                    a.test.name, c.line, c.handler_line
                ),
                &format!(
                    "Restore the assertion to propagate failures, or justify the handler in the PR body or a commit message: `allow-assertion-drop: {} <reason>`.",
                    leaf_name(a.test)
                ),
            );
        }
    }
}

/// Reports each test helper that lost assertions or was deleted, naming the tests that
/// call it.
fn report_weakened_helpers(
    pairs: &[TestPair],
    added: &[Located],
    helpers: &[HelperPair],
    exempt: &PathFilter,
    cx: &ReductionInputs,
    out: &mut GateOutcome,
) {
    for hp in helpers.iter().filter(|hp| !exempt.matches(hp.path)) {
        let callers = |leaf: &str| helper_callers_clause(pairs, added, leaf, &hp.base.name);
        report_weakened_helper(hp, &callers, cx, out);
    }
}

/// Reports the helpers of a crate's test-only modules that lost assertions or were
/// deleted ([`WeakenedCrateHelper`]), as [`report_weakened_helpers`] reports a helper of
/// a test-support file: the same finding, lifted by the same directive, naming the tests
/// whose calls resolve to the helper. A helper that a dropping test of its own file
/// already shows is not reported a second time (`leave_helpers_shown_by_tests`).
/// `helpers` are the pairs that credit a test.
pub fn report_weakened_crate_helpers<'a>(
    weakened: &'a [WeakenedCrateHelper],
    pairs: &[TestPair<'a>],
    files: &'a [FileFacts],
    helpers: &[HelperPair<'a>],
    cx: &ReductionInputs,
    out: &mut GateOutcome,
) -> Result<()> {
    let exempt = exempt_filter(cx.settings)?;
    let mut judged: Vec<HelperPair<'a>> = weakened
        .iter()
        .map(|w| HelperPair {
            path: &w.path,
            base: &w.base,
            head: w.head.as_ref(),
        })
        .collect();
    leave_helpers_shown_by_tests(&mut judged, pairs, files, Some(helpers));
    out.examined += weakened.len();
    for hp in judged.iter().filter(|hp| !exempt.matches(hp.path)) {
        let callers = |_: &str| {
            let of = weakened.iter().find(|w| std::ptr::eq(&w.base, hp.base));
            callers_clause(of.map(|w| w.callers.clone()).unwrap_or_default())
        };
        report_weakened_helper(hp, &callers, cx, out);
    }
    Ok(())
}

/// Reports `hp` when its helper lost assertions or was deleted. `callers` gives the
/// clause naming the tests that call the helper, from the last segment of its name.
fn report_weakened_helper(
    hp: &HelperPair,
    callers: &dyn Fn(&str) -> String,
    cx: &ReductionInputs,
    out: &mut GateOutcome,
) {
    const GATE: &str = ASSERTION_REDUCTION;
    let (settings, directives, is_staged) = (cx.settings, cx.directives, cx.is_staged);
    let b = hp.base;
    let (total_drop, strong_drop, fatal_drop, h_eff) = match hp.head {
        Some(h) => {
            let b_eff = b.effective_asserts();
            let h_eff = h.effective_asserts();
            (
                h_eff < b_eff,
                h.strong_asserts < b.strong_asserts,
                h.fatal_asserts < b.fatal_asserts,
                h_eff,
            )
        }
        None => (
            b.effective_asserts() > 0,
            b.strong_asserts > 0,
            b.fatal_asserts > 0,
            0,
        ),
    };
    if !total_drop && !strong_drop && !fatal_drop {
        return;
    }

    let lift = |subject: &str| {
        tokens::find_override(
            directives,
            GATE,
            &crate::findings::TEST_HELPER_WEAKENED,
            tokens::ALLOW_ASSERTION_DROP,
            subject,
        )
    };
    let helper_leaf = b.name.rsplit("::").next().unwrap_or(&b.name);
    let file_leaf = hp.path.rsplit('/').next().unwrap_or(hp.path);
    let allowed = lift(helper_leaf)
        .or_else(|| lift(&b.name))
        .or_else(|| lift(hp.path))
        .or_else(|| lift(file_leaf));
    let anchored = allowed.is_none();

    let callers_str = callers(helper_leaf);

    let b_eff = b.effective_asserts();
    let msg = match hp.head {
        None => format!(
            "Helper `{}`: deleted or checks removed (previously had {} assertion(s)){}.",
            b.name, b_eff, callers_str
        ),
        Some(_) if total_drop => format!(
            "Helper `{}`: effective assertions dropped from {} to {}{}.",
            b.name, b_eff, h_eff, callers_str
        ),
        Some(h) if strong_drop => format!(
            "Helper `{}`: equality / pattern assertions dropped from {} to {} (weakened to a looser form){}.",
            b.name, b.strong_asserts, h.strong_asserts, callers_str
        ),
        Some(h) => format!(
            "Helper `{}`: fatal assertions dropped from {} to {}{}.",
            b.name, b.fatal_asserts, h.fatal_asserts, callers_str
        ),
    };

    let severity = if is_staged {
        crate::config::Severity::Warning
    } else {
        settings.severity()
    };
    let line = hp.head.map(|h| h.line).unwrap_or(b.line);
    out.lift_or_push(
        allowed,
        severity,
        &crate::findings::TEST_HELPER_WEAKENED,
        (Some(hp.path), Some(line)),
        msg,
        &format!(
            "Restore the assertions in `{helper_leaf}`, or justify the change in the PR body or a commit message: `allow-assertion-drop: {helper_leaf} <reason>`."
        ),
    );
    if anchored {
        out.anchor_last(b.name.clone());
    }
}

/// The clause of a helper finding that names the tests calling the helper: empty when
/// none does.
fn helper_callers_clause(
    pairs: &[TestPair],
    added: &[Located],
    helper_leaf: &str,
    helper_name: &str,
) -> String {
    let mut calling_tests = Vec::new();
    for p in pairs {
        if p.head.direct_calls.iter().any(|c| {
            let c_leaf = c.rsplit("::").next().unwrap_or(c);
            let c_leaf = c_leaf.rsplit('.').next().unwrap_or(c_leaf);
            c_leaf == helper_leaf || c == helper_name
        }) || p.base.direct_calls.iter().any(|c| {
            let c_leaf = c.rsplit("::").next().unwrap_or(c);
            let c_leaf = c_leaf.rsplit('.').next().unwrap_or(c_leaf);
            c_leaf == helper_leaf || c == helper_name
        }) {
            calling_tests.push(p.head.name.clone());
        }
    }
    for a in added {
        if a.test.direct_calls.iter().any(|c| {
            let c_leaf = c.rsplit("::").next().unwrap_or(c);
            let c_leaf = c_leaf.rsplit('.').next().unwrap_or(c_leaf);
            c_leaf == helper_leaf || c == helper_name
        }) {
            calling_tests.push(a.test.name.clone());
        }
    }
    callers_clause(calling_tests)
}

/// ` (called by ..)` naming `calling_tests`: up to three by name, the first and a count
/// beyond that; empty when there is none.
fn callers_clause(mut calling_tests: Vec<String>) -> String {
    calling_tests.sort();
    calling_tests.dedup();

    if calling_tests.is_empty() {
        String::new()
    } else if calling_tests.len() <= 3 {
        format!(
            " (called by {})",
            calling_tests
                .iter()
                .map(|n| format!("`{}`", n))
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        format!(
            " (called by {} tests: `{}` and others)",
            calling_tests.len(),
            calling_tests[0]
        )
    }
}

/// The literal case counts of each file's tests on each side, and the cases that arrived
/// on a test of each file: what a case dropped from another test of that file can have
/// moved to.
struct FileCases<'a> {
    base_cases: std::collections::HashMap<&'a str, usize>,
    head_cases: std::collections::HashMap<&'a str, usize>,
    arrived_rows: std::collections::HashMap<&'a str, Vec<&'a str>>,
}

impl<'a> FileCases<'a> {
    fn collect(pairs: &[TestPair<'a>], added: &[Located<'a>], exempt: &PathFilter) -> Self {
        let mut file_base_cases: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        let mut file_head_cases: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        // The cases that arrived on a test of each file: what a case dropped from another
        // test of that file can have moved to.
        let mut file_arrived_rows: std::collections::HashMap<&str, Vec<&str>> =
            std::collections::HashMap::new();

        for p in pairs.iter().filter(|p| !exempt.matches(p.path)) {
            if let Some(c) = p.base.cases {
                *file_base_cases.entry(p.path).or_default() += c;
            }
            if let Some(c) = p.head.cases {
                *file_head_cases.entry(p.path).or_default() += c;
            }
            file_arrived_rows
                .entry(p.path)
                .or_default()
                .extend(arrived_case_rows(p.base, p.head));
        }
        for a in added.iter().filter(|a| !exempt.matches(a.path)) {
            if let Some(c) = a.test.cases {
                *file_head_cases.entry(a.path).or_default() += c;
            }
            if let Some(rows) = &a.test.case_rows {
                file_arrived_rows
                    .entry(a.path)
                    .or_default()
                    .extend(rows.iter().map(String::as_str));
            }
        }
        FileCases {
            base_cases: file_base_cases,
            head_cases: file_head_cases,
            arrived_rows: file_arrived_rows,
        }
    }
}

/// Everything one paired test is reported for.
struct PairFindings<'a> {
    /// The drop in literal cases that is not read as moved to another test of the file.
    case_drop: Option<CaseDrop>,
    newly_caught: Vec<&'a crate::ast::caught_assertions::CaughtAssertion>,
    loss: AssertionLoss,
    mock_growth: bool,
    loosened: Vec<crate::ast::bounds::Loosened>,
    changed: Vec<usize>,
    widened: Vec<crate::ast::expected_exceptions::Widened>,
    self_compared: Vec<&'a crate::ast::self_comparison::SelfComparison>,
    /// Assertions, fatal assertions or cases went down, or test doubles grew alone.
    dropped: bool,
}

impl PairFindings<'_> {
    /// The finding the override lifts, as the report below would rank it: the drop
    /// (fewer assertions, weaker fatal ones, or doubles grown alone), else a loosened
    /// bound. One directive lifts every finding of the pair; the record names the first.
    fn first_kind(&self) -> &'static crate::findings::FindingKind {
        if !self.newly_caught.is_empty() {
            &crate::findings::ASSERTION_FAILURE_CAUGHT
        } else if self.case_drop.is_some() {
            &crate::findings::TEST_CASES_REDUCED
        } else if self.loss.total || self.loss.strong {
            &crate::findings::ASSERTIONS_REDUCED
        } else if self.loss.fatal {
            &crate::findings::FATAL_ASSERTIONS_WEAKENED
        } else if self.mock_growth {
            &crate::findings::MOCKING_INCREASED
        } else if !self.loosened.is_empty() {
            &crate::findings::ASSERTION_BOUND_LOOSENED
        } else if !self.widened.is_empty() {
            &crate::findings::EXPECTED_EXCEPTION_WIDENED
        } else if !self.changed.is_empty() {
            &crate::findings::EXPECTED_VALUE_CHANGED
        } else {
            &crate::findings::SELF_COMPARISON_ASSERTION_INTRODUCED
        }
    }
}

/// How a message names the pair: the head test, or both names of a forced pair.
fn pair_label(p: &TestPair) -> String {
    if p.forced {
        format!("Test `{}` -> `{}`", p.base.name, p.head.name)
    } else {
        format!("Test `{}`", p.head.name)
    }
}

/// The test a remediation names in its directive: the base test of a forced pair.
fn pair_directive_name<'a>(p: &TestPair<'a>) -> &'a str {
    if p.forced {
        leaf_name(p.base)
    } else {
        leaf_name(p.head)
    }
}

/// Judges one paired test: what it lost, the directive that lifts it, and its findings.
fn judge_pair<'a>(
    p: &TestPair<'a>,
    helpers: &[HelperPair],
    file_cases: &mut FileCases<'a>,
    cx: &ReductionInputs,
    out: &mut GateOutcome,
) {
    const GATE: &str = ASSERTION_REDUCTION;
    let directives = cx.directives;
    let (b, h) = (p.base, p.head);
    let Some(f) = pair_findings(p, helpers, file_cases, cx.settings, out) else {
        return;
    };

    // For forced pairs (unrelated names forced together), the override directive MUST name
    // the old test that was replaced/gutted. For non-forced pairs (exact name or similarity rename),
    // naming either the old test or the new test is accepted.
    let lifts = f.first_kind();
    let lift = |subject: &str| {
        tokens::find_override(
            directives,
            GATE,
            lifts,
            tokens::ALLOW_ASSERTION_DROP,
            subject,
        )
    };
    let allowed = if p.forced {
        lift(leaf_name(b)).or_else(|| lift(p.path))
    } else {
        lift(leaf_name(h))
            .or_else(|| lift(leaf_name(b)))
            .or_else(|| lift(p.path))
    };
    if let Some(record) = allowed {
        out.overrides.push(record);
        return;
    }

    let mut case_drop_allowed = false;
    if f.case_drop.is_some() {
        let lift_case = |subject: &str| {
            tokens::find_override(
                directives,
                GATE,
                &crate::findings::TEST_CASES_REDUCED,
                tokens::ALLOW_CASE_DROP,
                subject,
            )
        };
        let allowed_case = if p.forced {
            lift_case(leaf_name(b)).or_else(|| lift_case(p.path))
        } else {
            lift_case(leaf_name(h))
                .or_else(|| lift_case(leaf_name(b)))
                .or_else(|| lift_case(p.path))
        };
        if let Some(record) = allowed_case {
            out.overrides.push(record);
            case_drop_allowed = true;
        }
    }

    report_newly_caught(p, &f, cx, out);
    report_self_comparisons(p, &f, out);
    report_loosened_bounds(p, &f, cx, out);
    report_changed_expectations(p, &f, cx, out);
    report_widened_exceptions(p, &f, cx, out);
    if !f.dropped {
        return;
    }

    report_case_drop(p, &f, case_drop_allowed, cx, out);
    report_assertion_loss(p, &f, cx, out);
}

/// Everything the pair is reported for, with the notes on what was read as moved.
/// `None` when the head test weakens nothing.
fn pair_findings<'a>(
    p: &TestPair<'a>,
    helpers: &[HelperPair],
    file_cases: &mut FileCases<'a>,
    settings: &crate::config::AssertionGate,
    out: &mut GateOutcome,
) -> Option<PairFindings<'a>> {
    let (b, h) = (p.base, p.head);
    let case_drop = unmoved_case_drop(p, file_cases, out);
    let cases_drop = case_drop.is_some();
    let b_eff = b.effective_asserts();
    let h_eff = h.effective_asserts();
    let newly_caught = crate::ast::caught_assertions::newly_caught(b, h);
    let loss = assertion_loss(p, helpers, newly_caught.len(), settings, out);
    let (total_drop, strong_drop, fatal_drop) = (loss.total, loss.strong, loss.fatal);
    // More doubles in the test, and no stronger assertion on what the code produced:
    // the shape of an integration failure sidestepped by mocking it away.
    let mock_growth = h.mock_setups > b.mock_setups
        && h.strong_asserts <= b.strong_asserts
        && h_eff.saturating_sub(h.mock_asserts) <= b_eff.saturating_sub(b.mock_asserts);
    // The same assertion with its numeric bound moved the loose way: the count holds.
    let loosened = crate::ast::bounds::loosened(&b.bounds, &h.bounds);
    // The same assertion expecting a different value: the count and strength hold.
    let changed = crate::ast::expectations::changed(&b.expectations, &h.expectations);
    // The same assertion with widened expected exception or dropped matcher.
    let mut widened = crate::ast::expected_exceptions::widened_in(b, h);
    // An expectation that is gone along with a lower count is the reduction below.
    if total_drop || strong_drop {
        widened.retain(|w| !w.dropped);
    }
    // Equality assertions that compare an operand with itself and that the base
    // side did not hold (`ast::self_comparison`).
    let self_compared = h.equality_operands.introduced_since(&b.equality_operands);
    let dropped = total_drop || strong_drop || fatal_drop || mock_growth || cases_drop;
    if !dropped
        && loosened.is_empty()
        && changed.is_empty()
        && newly_caught.is_empty()
        && widened.is_empty()
        && self_compared.is_empty()
    {
        return None;
    }
    Some(PairFindings {
        case_drop,
        newly_caught,
        loss,
        mock_growth,
        loosened,
        changed,
        widened,
        self_compared,
        dropped,
    })
}

/// The drop in the pair's literal case count that is not read as moved to another test
/// of the file. Notes what was read as moved and what could not be compared.
fn unmoved_case_drop<'a>(
    p: &TestPair<'a>,
    file_cases: &mut FileCases<'a>,
    out: &mut GateOutcome,
) -> Option<CaseDrop> {
    let (b, h) = (p.base, p.head);
    if b.non_literal_cases || h.non_literal_cases {
        out.notes.push(format!(
            "{}:{}: non-literal test case source in `{}`; test case reduction cannot be statically verified",
            p.path, h.line, h.name
        ));
    }
    // A base side with a literal count is compared with whatever the head side is:
    // a head with no literal count is a reduction that cannot be measured, not an
    // unchanged test. One literal case is one run with or without its parametrization.
    let case_change = match (b.cases, h.cases) {
        (Some(b_cases), Some(h_cases)) if h_cases < b_cases => {
            Some(CaseDrop::Fewer(b_cases, h_cases))
        }
        (Some(b_cases), None) if b_cases >= 2 => Some(if h.non_literal_cases {
            CaseDrop::NotLiteral(b_cases)
        } else {
            CaseDrop::NotParametrized(b_cases)
        }),
        _ => None,
    };
    let mut cases_drop_info = None;
    if let Some(change) = case_change {
        // A dropped case is excused as moved when the same case, by its content,
        // arrived on another test of the file. The file holding as many cases as
        // before says nothing: three cases dropped beside an unrelated new test of
        // three are three cases dropped.
        let dropped = match change {
            CaseDrop::Fewer(b_cases, h_cases) => b_cases - h_cases,
            CaseDrop::NotLiteral(b_cases) | CaseDrop::NotParametrized(b_cases) => b_cases,
        };
        let left = left_case_rows(b, h, change);
        let moved = match &left {
            Some(left) => {
                let arrived = file_cases.arrived_rows.entry(p.path).or_default();
                take_moved_rows(left, arrived, dropped)
            }
            None => 0,
        };
        let file_keeps_its_total = file_cases.head_cases.get(p.path).copied().unwrap_or(0)
            >= file_cases.base_cases.get(p.path).copied().unwrap_or(0);
        if moved >= dropped {
            let what = match change {
                CaseDrop::Fewer(b_cases, h_cases) => {
                    format!("test cases {b_cases} -> {h_cases}")
                }
                CaseDrop::NotLiteral(b_cases) | CaseDrop::NotParametrized(b_cases) => {
                    format!("{b_cases} literal test cases no longer counted on it")
                }
            };
            out.notes.push(format!(
                "`{}` in `{}`: {} read as preserved across tests in same file ({} of {} dropped case(s) found moved to another test of the file, 0 not found)",
                h.name, p.path, what, moved, dropped
            ));
        } else {
            if left.is_none() && file_keeps_its_total {
                out.notes.push(format!(
                    "`{}` in `{}`: the file holds as many cases as before, but the cases dropped from this test cannot be compared by content (its case list is not literal rows on the side that had them, or is no longer a literal list), so none is read as moved",
                    h.name, p.path
                ));
            } else if moved > 0 || file_keeps_its_total {
                out.notes.push(format!(
                    "`{}` in `{}`: {} of {} dropped case(s) found moved to another test of the file, {} not found",
                    h.name,
                    p.path,
                    moved,
                    dropped,
                    dropped - moved
                ));
            }
            cases_drop_info = Some(change);
        }
    }
    cases_drop_info
}

/// What a paired test lost in assertions once the checks its helpers stand for are
/// counted.
struct AssertionLoss {
    /// Fewer effective assertions.
    total: bool,
    /// Fewer equality / pattern assertions.
    strong: bool,
    /// Fewer fatal assertions.
    fatal: bool,
    /// The checks the test's calls to paired helpers add, counted into a reported drop.
    helper_total: usize,
    helper_strong: usize,
}

/// Compares the pair's assertion counts, crediting the checks moved into helpers and
/// attributing to a weakened helper the drop that is the helper's.
fn assertion_loss(
    p: &TestPair,
    helpers: &[HelperPair],
    newly_caught: usize,
    settings: &crate::config::AssertionGate,
    out: &mut GateOutcome,
) -> AssertionLoss {
    let (b, h) = (p.base, p.head);
    let b_eff = b.effective_asserts();
    let h_eff = h.effective_asserts();
    let mut loss = AssertionLoss {
        total: h_eff < b_eff,
        strong: h.strong_asserts < b.strong_asserts,
        fatal: false,
        helper_total: 0,
        helper_strong: 0,
    };
    if loss.total && h_eff + newly_caught >= b_eff {
        loss.total = false;
    }
    // Checks moved into same-file helpers that fail (assert, raise, throw, panic) in a
    // loop: one `raise` in a helper's loop stands for many inline assertions, so the
    // count drops while the test calls more such helpers than before. A helper that
    // checks in a straight line stands for what it holds, which the pack counted into
    // the test. Deleting a helper call lowers `helper_checks` and is still a drop.
    if (loss.total || loss.strong) && moved_into_looping_helpers(b, h) {
        out.notes.push(format!(
            "`{}` in `{}`: assertions {} -> {} read as moved into same-file helpers that fail ({} -> {} calls)",
            h.name, p.path, b_eff, h_eff, b.helper_checks, h.helper_checks
        ));
        loss.total = false;
        loss.strong = false;
    }

    let helper_fatal = credit_helper_calls(p, helpers, newly_caught, settings, &mut loss, out);

    let attributed_fatal = attribute_to_weakened_helpers(p, helpers, &mut loss, out);
    loss.fatal = h.fatal_asserts + helper_fatal + attributed_fatal < b.fatal_asserts;
    loss
}

/// Credits the pair with the checks its calls to helpers add. Returns the fatal checks
/// among them.
fn credit_helper_calls(
    p: &TestPair,
    helpers: &[HelperPair],
    newly_caught: usize,
    settings: &crate::config::AssertionGate,
    loss: &mut AssertionLoss,
    out: &mut GateOutcome,
) -> usize {
    let (b, h) = (p.base, p.head);
    let b_eff = b.effective_asserts();
    let h_eff = h.effective_asserts();
    // Checks moved into a helper the test's own count does not hold (one in another
    // file, a same-file method called on a receiver, a same-file helper reached deeper
    // than the pack follows): the helper stands for the checks its calls add to this
    // test, and no more. A helper the test newly calls adds its head checks per call;
    // one the base already called adds what it gained. What that does not cover is
    // still a drop, reported with the helper's share counted in.
    let mut helper_fatal = 0;
    if loss.total || loss.strong {
        let moved = helper_call_gain(b, h, p.path, helpers, &settings.assert_helper_fns);
        let mut note_the_move = true;
        helper_fatal = moved.fatal;
        if loss.total && h_eff + newly_caught + moved.total >= b_eff {
            loss.total = false;
        }
        if loss.strong && h.strong_asserts + moved.strong >= b.strong_asserts {
            loss.strong = false;
        }
        // The count is covered and only strength falls short: an equality assertion
        // rewritten in a same-file helper as a failure exit under an equality
        // comparison (`if a != b { panic!() }`) is still an equality check.
        if loss.strong && !loss.total {
            let by_hand = equality_exits_gained(b, h) + moved.equality;
            if by_hand > 0 && h.strong_asserts + moved.strong + by_hand >= b.strong_asserts {
                loss.strong = false;
                note_the_move = false;
                let whose = if moved.equality == 0 {
                    "same-file "
                } else {
                    ""
                };
                out.notes.push(format!(
                    "`{}` in `{}`: equality assertions {} -> {} read as moved into {whose}helpers that fail on an equality comparison ({} exit(s))",
                    h.name, p.path, b.strong_asserts, h.strong_asserts, by_hand
                ));
            }
        }
        if loss.total || loss.strong {
            loss.helper_total = moved.total;
            loss.helper_strong = moved.strong;
        } else if note_the_move {
            out.notes.push(format!(
                "`{}` in `{}`: assertions {} -> {} read as moved into helper `{}` ({} check(s))",
                h.name,
                p.path,
                b_eff,
                h_eff,
                moved.names.join("`, `"),
                moved.total
            ));
        }
    }
    helper_fatal
}

/// Clears a drop that is entirely what the test's same-file helpers lost. Returns the
/// fatal checks those helpers lost.
fn attribute_to_weakened_helpers(
    p: &TestPair,
    helpers: &[HelperPair],
    loss: &mut AssertionLoss,
    out: &mut GateOutcome,
) -> usize {
    let (b, h) = (p.base, p.head);
    let b_eff = b.effective_asserts();
    let h_eff = h.effective_asserts();
    // A helper in the test's own file is counted in the test, so a helper that lost
    // checks lowers the count of every test that calls it. That drop is the helper's,
    // reported once on the helper: the test is not reported for it when everything the
    // test lost, in count and in strength, is what its calls to those helpers lost. A
    // helper in another file is not counted in the test, so a drop in the test beside
    // one is the test's own and stays reported.
    let mut attributed_fatal = 0;
    if loss.total || loss.strong {
        let (mut lost_total, mut lost_strong, mut lost_fatal) = (0, 0, 0);
        let mut weakened_helper_names = Vec::new();
        for call in &h.direct_calls {
            let weakened = helpers.iter().find_map(|hp| {
                let head_helper = hp.head?;
                let lost = hp
                    .base
                    .effective_asserts()
                    .saturating_sub(head_helper.effective_asserts());
                let lost_strength = hp
                    .base
                    .strong_asserts
                    .saturating_sub(head_helper.strong_asserts);
                (hp.path == p.path
                    && call_names_helper(call, &hp.base.name)
                    && (lost > 0 || lost_strength > 0))
                    .then(|| {
                        (
                            hp.base.name.as_str(),
                            lost,
                            lost_strength,
                            hp.base
                                .fatal_asserts
                                .saturating_sub(head_helper.fatal_asserts),
                        )
                    })
            });
            if let Some((name, lost, lost_strength, lost_fatality)) = weakened {
                lost_total += lost;
                lost_strong += lost_strength;
                lost_fatal += lost_fatality;
                weakened_helper_names.push(name);
            }
        }
        let delta_test = b_eff.saturating_sub(h_eff);
        let delta_strong = b.strong_asserts.saturating_sub(h.strong_asserts);
        if !weakened_helper_names.is_empty()
            && delta_test <= lost_total
            && delta_strong <= lost_strong
        {
            weakened_helper_names.dedup();
            out.notes.push(format!(
                "`{}` in `{}`: assertion drop {} -> {} attributed to weakened helper `{}`",
                h.name,
                p.path,
                b_eff,
                h_eff,
                weakened_helper_names.join("`, `")
            ));
            loss.total = false;
            loss.strong = false;
            attributed_fatal = lost_fatal;
        }
    }
    attributed_fatal
}

/// Reports each assertion an enclosing handler newly catches.
fn report_newly_caught(
    p: &TestPair,
    f: &PairFindings,
    cx: &ReductionInputs,
    out: &mut GateOutcome,
) {
    let (settings, is_staged) = (cx.settings, cx.is_staged);
    let test_label = pair_label(p);
    let directive_name = pair_directive_name(p);
    for c in &f.newly_caught {
        out.push(
            if is_staged {
                crate::config::Severity::Warning
            } else {
                settings.severity()
            },
            &crate::findings::ASSERTION_FAILURE_CAUGHT,
            Some(p.path),
            Some(c.line),
            format!(
                "{test_label}: the assertion on line {} is caught by an enclosing handler (line {}) without failing the test; it is not an effective check.",
                c.line, c.handler_line
            ),
            &format!(
                "Restore the assertion to propagate failures, or justify the handler in the PR body or a commit message: `allow-assertion-drop: {} <reason>`.",
                directive_name
            ),
        );
    }
}

/// Reports each equality assertion that newly compares an expression with itself, while
/// the assertion count holds.
fn report_self_comparisons(p: &TestPair, f: &PairFindings, out: &mut GateOutcome) {
    let h = p.head;
    let test_label = pair_label(p);
    let directive_name = pair_directive_name(p);
    // One finding for one act: a rewrite that also lowers the count is the
    // `assertions-reduced` finding below, at the gate's severity, whose message names
    // these lines. Reported here only while the count holds, as a warning: the form
    // is exact, and a deliberate reflexivity check is a legitimate test.
    let folded_into_reduction = f.loss.total || f.loss.strong;
    for s in f.self_compared.iter().filter(|_| !folded_into_reduction) {
        out.push(
            crate::config::Severity::Warning,
            &crate::findings::SELF_COMPARISON_ASSERTION_INTRODUCED,
            Some(p.path),
            Some(s.line),
            format!(
                "{test_label}: the equality assertion on line {} now compares an expression with itself, so it holds whatever the code does. {SELF_COMPARISON_SCOPE}",
                s.line
            ),
            &format!(
                "Compare the value with what is expected of it. A deliberate reflexivity check takes, on its own line in the PR body or a commit message: `allow-assertion-drop: {} <reason>`.",
                directive_name
            ),
        );
        out.anchor_last(h.name.clone());
        note_self_comparison_scope(out);
    }
}

/// Reports each assertion whose numeric bound moved the loose way.
fn report_loosened_bounds(
    p: &TestPair,
    f: &PairFindings,
    cx: &ReductionInputs,
    out: &mut GateOutcome,
) {
    let (settings, is_staged) = (cx.settings, cx.is_staged);
    let test_label = pair_label(p);
    let directive_name = pair_directive_name(p);
    for l in &f.loosened {
        out.push(
            if is_staged {
                crate::config::Severity::Warning
            } else {
                settings.severity()
            },
            &crate::findings::ASSERTION_BOUND_LOOSENED,
            Some(p.path),
            Some(l.line),
            // The line and the two literals only: the assertion's text is the change's
            // own and is not echoed into a report agents read.
            format!(
                "{test_label}: the assertion on line {} moved its bound from {} to {}, which accepts more results.",
                l.line, l.from, l.to
            ),
            &format!(
                "Restore the bound, or justify the change on its own line in the PR body or a commit message: `allow-assertion-drop: {} <reason>`.",
                directive_name
            ),
        );
    }
}

/// Reports each assertion that now expects a different value.
fn report_changed_expectations(
    p: &TestPair,
    f: &PairFindings,
    cx: &ReductionInputs,
    out: &mut GateOutcome,
) {
    let h = p.head;
    let (settings, is_staged) = (cx.settings, cx.is_staged);
    let test_label = pair_label(p);
    let directive_name = pair_directive_name(p);
    for &line in &f.changed {
        out.push(
            if is_staged {
                crate::config::Severity::Warning
            } else {
                settings.severity()
            },
            &crate::findings::EXPECTED_VALUE_CHANGED,
            Some(p.path),
            Some(line),
            // The line only: an expected value is the change's own text (a string can
            // carry anything) and is not echoed into a report agents read.
            format!(
                "{test_label}: the assertion on line {line} now expects a different value; the assertion is otherwise unchanged."
            ),
            &format!(
                "Restore the expected value, or justify the new one on its own line in the PR body or a commit message: `allow-assertion-drop: {} <reason>`.",
                directive_name
            ),
        );
        out.anchor_last(h.name.clone());
    }
}

/// Reports each expected failure that was widened or dropped.
fn report_widened_exceptions(
    p: &TestPair,
    f: &PairFindings,
    cx: &ReductionInputs,
    out: &mut GateOutcome,
) {
    let h = p.head;
    let (settings, is_staged) = (cx.settings, cx.is_staged);
    let test_label = pair_label(p);
    let directive_name = pair_directive_name(p);
    for w in &f.widened {
        out.push(
            if is_staged {
                crate::config::Severity::Warning
            } else {
                settings.severity()
            },
            &crate::findings::EXPECTED_EXCEPTION_WIDENED,
            Some(p.path),
            Some(if w.dropped { h.line } else { w.line }),
            if w.dropped {
                // The base expectation has no line at head: the test is the location.
                format!(
                    "{test_label}: {}; no expectation at head stands for it, so the test passes without that failure. An expectation replaced on purpose is lifted with `allow-assertion-drop: {} <reason>`.",
                    w.detail, directive_name
                )
            } else {
                format!(
                    "{test_label}: the expected failure on line {} was widened ({}); it now accepts more failures.",
                    w.line, w.detail
                )
            },
            &format!(
                "Restore the expected failure or matcher, or justify the change on its own line in the PR body or a commit message: `allow-assertion-drop: {} <reason>`.",
                directive_name
            ),
        );
        out.anchor_last(h.name.clone());
    }
}

/// Reports a drop in literal test cases that no `allow-case-drop` directive lifted.
fn report_case_drop(
    p: &TestPair,
    f: &PairFindings,
    case_drop_allowed: bool,
    cx: &ReductionInputs,
    out: &mut GateOutcome,
) {
    let (b, h) = (p.base, p.head);
    let (settings, is_staged) = (cx.settings, cx.is_staged);
    let test_label = pair_label(p);
    let directive_name = pair_directive_name(p);
    if let Some(change) = f.case_drop {
        if !case_drop_allowed {
            let severity = if is_staged {
                crate::config::Severity::Warning
            } else {
                settings.severity()
            };
            let violation_line = if h.total_asserts > 0 { h.line } else { b.line };
            out.push(
                severity,
                &crate::findings::TEST_CASES_REDUCED,
                Some(p.path),
                Some(violation_line),
                // Counts only: a case source is the change's own text and is not echoed.
                match change {
                    CaseDrop::Fewer(b_cases, h_cases) => format!(
                        "{test_label}: test cases in parametrized / table-driven test dropped from {b_cases} to {h_cases}."
                    ),
                    CaseDrop::NotLiteral(b_cases) => format!(
                        "{test_label}: the case source is no longer a literal list and its cases cannot be counted; it had {b_cases} literal cases."
                    ),
                    CaseDrop::NotParametrized(b_cases) => format!(
                        "{test_label}: ran {b_cases} cases and is no longer read as parametrized; no case list is found on it."
                    ),
                },
                &format!(
                    "Restore the test cases, or justify the drop on its own line in the PR body or \
                     a commit message: `allow-case-drop: {} <reason>`.",
                    directive_name
                ),
            );
        }
    }
}

/// Reports what the pair lost in assertions: test doubles grown alone, fatal assertions
/// weakened alone, or fewer assertions.
fn report_assertion_loss(
    p: &TestPair,
    f: &PairFindings,
    cx: &ReductionInputs,
    out: &mut GateOutcome,
) {
    let (b, h) = (p.base, p.head);
    let (settings, is_staged) = (cx.settings, cx.is_staged);
    let test_label = pair_label(p);
    let directive_name = pair_directive_name(p);
    if !f.loss.total && !f.loss.strong && !f.loss.fatal && f.mock_growth {
        out.push(
            crate::config::Severity::Warning,
            &crate::findings::MOCKING_INCREASED,
            Some(p.path),
            Some(h.line),
            format!(
                "{test_label}: test doubles rose from {} to {} while assertions on real output did not grow (equality / pattern assertions: {} -> {}).",
                b.mock_setups, h.mock_setups, b.strong_asserts, h.strong_asserts
            ),
            &format!(
                "Assert on what the code produces alongside the new doubles, or justify the change in the PR body: `allow-assertion-drop: {} <reason>`.",
                directive_name
            ),
        );
        return;
    }

    if !f.loss.total && !f.loss.strong && f.loss.fatal {
        out.push(
            crate::config::Severity::Warning,
            &crate::findings::FATAL_ASSERTIONS_WEAKENED,
            Some(p.path),
            Some(h.line),
            format!(
                "{test_label}: fatal assertions dropped from {} to {} (weakened from abort-on-failure to non-fatal).",
                b.fatal_asserts, h.fatal_asserts
            ),
            &format!(
                "Restore fatal assertions (e.g. ASSERT_* or require.*), or justify the change in the PR body: `allow-assertion-drop: {} <reason>`.",
                directive_name
            ),
        );
        return;
    }

    if !f.loss.total && !f.loss.strong {
        return;
    }

    // The head side counts the checks the test's paired-helper calls account for, so
    // a partly moved test reads as what it still checks, not as its inline count.
    let what = if f.loss.total {
        format!(
            "effective assertions dropped from {} to {}",
            b.effective_asserts(),
            h.effective_asserts() + f.loss.helper_total
        )
    } else {
        format!(
            "equality / pattern assertions dropped from {} to {} (weakened to a looser form)",
            b.strong_asserts,
            h.strong_asserts + f.loss.helper_strong
        )
    };

    let severity = if is_staged {
        crate::config::Severity::Warning
    } else {
        settings.severity()
    };

    let violation_line = if h.total_asserts > 0 { h.line } else { b.line };

    out.push(
        severity,
        &crate::findings::ASSERTIONS_REDUCED,
        Some(p.path),
        Some(violation_line),
        if f.self_compared.is_empty() {
            format!("{test_label}: {what}.")
        } else {
            format!(
                "{test_label}: {what}; {} equality assertion(s) now compare an expression with itself (line {}). {SELF_COMPARISON_SCOPE}",
                f.self_compared.len(),
                lines_of(&f.self_compared)
            )
        },
        &format!(
            "Restore the assertions, or justify the drop on its own line in the PR body or \
             a commit message: `allow-assertion-drop: {} <reason>`.",
            directive_name
        ),
    );
    if !f.self_compared.is_empty() {
        note_self_comparison_scope(out);
    }
}

/// The cases that arrived on a paired test: on its head side and not on its base side,
/// by content, each as many times as it arrived. A side whose cases are not literal rows
/// cannot be compared, so such a test supplies none.
fn arrived_case_rows<'a>(base: &'a TestFn, head: &'a TestFn) -> Vec<&'a str> {
    let Some(head_rows) = &head.case_rows else {
        return Vec::new();
    };
    let mut before: Vec<&str> = match &base.case_rows {
        Some(rows) => rows.iter().map(String::as_str).collect(),
        // No case source on the base side: every head case arrived.
        None if base.cases.is_none() && !base.non_literal_cases => Vec::new(),
        None => return Vec::new(),
    };
    let mut arrived = Vec::new();
    for row in head_rows {
        match before.iter().position(|b| *b == row) {
            Some(i) => {
                before.swap_remove(i);
            }
            None => arrived.push(row.as_str()),
        }
    }
    arrived
}

/// The cases that left a paired test whose literal count went down: on its base side and
/// not on its head side, by content. `None` when they cannot be compared: a side whose
/// cases are not literal rows, or a head whose case source is no longer a literal list.
fn left_case_rows<'a>(
    base: &'a TestFn,
    head: &'a TestFn,
    change: CaseDrop,
) -> Option<Vec<&'a str>> {
    let base_rows = base.case_rows.as_ref()?;
    let mut still_there: Vec<&str> = match change {
        CaseDrop::NotLiteral(_) => return None,
        CaseDrop::NotParametrized(_) => Vec::new(),
        CaseDrop::Fewer(..) => head
            .case_rows
            .as_ref()?
            .iter()
            .map(String::as_str)
            .collect(),
    };
    let mut left = Vec::new();
    for row in base_rows {
        match still_there.iter().position(|h| *h == row) {
            Some(i) => {
                still_there.swap_remove(i);
            }
            None => left.push(row.as_str()),
        }
    }
    Some(left)
}

/// Takes out of `arrived` the cases of `left` found there, one arrival for one case, up
/// to `dropped` of them, and returns how many were found.
fn take_moved_rows(left: &[&str], arrived: &mut Vec<&str>, dropped: usize) -> usize {
    let mut moved = 0;
    for row in left {
        if moved == dropped {
            break;
        }
        if let Some(i) = arrived.iter().position(|a| a == row) {
            arrived.swap_remove(i);
            moved += 1;
        }
    }
    moved
}

/// How the literal case count of a paired parametrized test went down.
#[derive(Clone, Copy)]
enum CaseDrop {
    /// Both sides are literal: base count, head count.
    Fewer(usize, usize),
    /// The head's case source is an expression whose cases cannot be counted; base count.
    NotLiteral(usize),
    /// No case source is read on the head at all; base count.
    NotParametrized(usize),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::ParsedFileFacts;
    use crate::gitctx::{ChangeKind, ChangedFile};
    use crate::guards::agent_diff::test_support::*;
    use crate::guards::agent_diff::{match_tests, FileFacts};

    #[test]
    fn test_assertion_reduction_pure() {
        let b = TestFn {
            name: "test_something".to_string(),
            line: 1,
            total_asserts: 2,
            strong_asserts: 2,
            tautologies: 0,
            ignored: false,
            should_panic: None,
            ..Default::default()
        };
        let h_weak = TestFn {
            name: "test_something".to_string(),
            line: 1,
            total_asserts: 2,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            should_panic: None,
            ..Default::default()
        };
        let h_drop = TestFn {
            name: "test_something".to_string(),
            line: 1,
            total_asserts: 1,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            should_panic: None,
            ..Default::default()
        };
        let settings = crate::config::AssertionGate::default();

        // Weakened strong assert
        let pair_weak = [TestPair {
            path: "tests/a.rs",
            base: &b,
            head: &h_weak,
            forced: false,
        }];
        let out_weak =
            evaluate_assertion_reduction(&pair_weak, &[], &[], &settings, &[], false).unwrap();
        assert_eq!(out_weak.violations.len(), 1);
        assert_eq!(out_weak.examined, 1);

        // Dropped total assert
        let pair_drop = [TestPair {
            path: "tests/a.rs",
            base: &b,
            head: &h_drop,
            forced: false,
        }];
        let out_drop =
            evaluate_assertion_reduction(&pair_drop, &[], &[], &settings, &[], false).unwrap();
        assert_eq!(out_drop.violations.len(), 1);

        // Override justifies the drop
        let directives = [crate::tokens::ParsedDirective {
            directive: "allow-assertion-drop".to_string(),
            reason: "test_something refactored".to_string(),
            source: crate::tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        let out_override =
            evaluate_assertion_reduction(&pair_drop, &[], &[], &settings, &directives, false)
                .unwrap();
        assert_eq!(out_override.violations.len(), 0);
        assert_eq!(out_override.overrides.len(), 1);

        // Added test does NOT absorb the drop: reductions are strictly per-paired test
        let added_split = [Located {
            path: "tests/a.rs",
            file_survives: true,
            test: &b, // has 2 asserts
        }];
        let out_unabsorbed =
            evaluate_assertion_reduction(&pair_drop, &added_split, &[], &settings, &[], false)
                .unwrap();
        assert_eq!(out_unabsorbed.violations.len(), 1);
    }

    #[test]
    fn test_compile_time_assertions_drop_detected_and_overridden() {
        let mut base_facts = ParsedFileFacts {
            compile_time_asserts: 3,
            compile_time_assert_line: Some(10),
            ..Default::default()
        };
        base_facts.build_compile_time_test();

        let mut head_facts = ParsedFileFacts {
            compile_time_asserts: 1,
            compile_time_assert_line: Some(10),
            ..Default::default()
        };
        head_facts.build_compile_time_test();

        let files = vec![FileFacts {
            file: ChangedFile {
                path: "src/types.rs".into(),
                old_path: "src/types.rs".into(),
                kind: ChangeKind::Modified,
                added_lines: std::collections::BTreeSet::new(),
            },
            base: Some(base_facts),
            head: Some(head_facts),
            newly_added_nul: false,
        }];

        let (pairs, _removed, _added) = match_tests(&files);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].base.name, "compile-time-assertions");

        let settings = crate::config::AssertionGate {
            enabled: true,
            severity: crate::config::Severity::Error,
            exempt_paths: vec![],
            ..Default::default()
        };

        // Without override -> violation
        let out = evaluate_assertion_reduction(&pairs, &[], &[], &settings, &[], false).unwrap();
        assert_eq!(out.violations.len(), 1);
        assert!(out.violations[0]
            .message
            .contains("effective assertions dropped from 3 to 1"));
        assert_eq!(out.violations[0].line, Some(10));

        // With override targeting compile-time-assertions -> pass
        let directive = [crate::tokens::ParsedDirective {
            directive: "allow-assertion-drop".to_string(),
            reason: "compile-time-assertions size refactor".to_string(),
            source: crate::tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        let out_override =
            evaluate_assertion_reduction(&pairs, &[], &[], &settings, &directive, false).unwrap();
        assert_eq!(out_override.violations.len(), 0);
        assert_eq!(out_override.overrides.len(), 1);

        // With override targeting file path -> pass
        let file_directive = [crate::tokens::ParsedDirective {
            directive: "allow-assertion-drop".to_string(),
            reason: "src/types.rs size refactor".to_string(),
            source: crate::tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        let out_file_override =
            evaluate_assertion_reduction(&pairs, &[], &[], &settings, &file_directive, false)
                .unwrap();
        assert_eq!(out_file_override.violations.len(), 0);
        assert_eq!(out_file_override.overrides.len(), 1);
    }

    #[test]
    fn test_evaluate_assertion_reduction_cases_reduced_reports_finding() {
        let b = TestFn {
            name: "test_param".to_string(),
            line: 10,
            cases: Some(5),
            total_asserts: 1,
            strong_asserts: 1,
            ..Default::default()
        };
        let h = TestFn {
            name: "test_param".to_string(),
            line: 10,
            cases: Some(2),
            total_asserts: 1,
            strong_asserts: 1,
            ..Default::default()
        };
        let pairs = [TestPair {
            path: "tests/test_foo.py",
            base: &b,
            head: &h,
            forced: false,
        }];
        let settings = crate::config::AssertionGate::default();

        let out = evaluate_assertion_reduction(&pairs, &[], &[], &settings, &[], false).unwrap();
        assert_eq!(out.violations.len(), 1);
        let v = &out.violations[0];
        assert_eq!(v.code, "assertion-reduction/test-cases-reduced");
        assert_eq!(v.severity, crate::config::Severity::Error);
        assert!(v
            .message
            .contains("test cases in parametrized / table-driven test dropped from 5 to 2"));
        assert!(v
            .remediation
            .as_deref()
            .unwrap()
            .contains("allow-case-drop: test_param"));

        // Staged reports warning
        let out_staged =
            evaluate_assertion_reduction(&pairs, &[], &[], &settings, &[], true).unwrap();
        assert_eq!(
            out_staged.violations[0].severity,
            crate::config::Severity::Warning
        );
    }

    #[test]
    fn test_evaluate_assertion_reduction_cases_reduced_waived_by_directive() {
        let b = TestFn {
            name: "test_param".to_string(),
            line: 10,
            cases: Some(5),
            total_asserts: 1,
            strong_asserts: 1,
            ..Default::default()
        };
        let h = TestFn {
            name: "test_param".to_string(),
            line: 10,
            cases: Some(2),
            total_asserts: 1,
            strong_asserts: 1,
            ..Default::default()
        };
        let pairs = [TestPair {
            path: "tests/test_foo.py",
            base: &b,
            head: &h,
            forced: false,
        }];
        let settings = crate::config::AssertionGate::default();

        // 1. Waived by allow-case-drop
        let directive_case = crate::tokens::parse_directives(
            "allow-case-drop: test_param removed slow redundant test cases\n",
            crate::tokens::OverrideSource::PrBody,
        );
        let out_case =
            evaluate_assertion_reduction(&pairs, &[], &[], &settings, &directive_case, false)
                .unwrap();
        assert_eq!(out_case.violations.len(), 0);
        assert_eq!(out_case.overrides.len(), 1);
        assert_eq!(
            out_case.overrides[0].code.as_deref(),
            Some("assertion-reduction/test-cases-reduced")
        );

        // 2. Waived by allow-assertion-drop
        let directive_assert = crate::tokens::parse_directives(
            "allow-assertion-drop: test_param removed slow redundant test cases\n",
            crate::tokens::OverrideSource::PrBody,
        );
        let out_assert =
            evaluate_assertion_reduction(&pairs, &[], &[], &settings, &directive_assert, false)
                .unwrap();
        assert_eq!(out_assert.violations.len(), 0);
        assert_eq!(out_assert.overrides.len(), 1);
        assert_eq!(
            out_assert.overrides[0].code.as_deref(),
            Some("assertion-reduction/test-cases-reduced")
        );
    }

    #[test]
    fn test_evaluate_assertion_reduction_case_drop_does_not_waive_loosened_bound_or_plain_assertion_drop(
    ) {
        let b = TestFn {
            name: "test_param".to_string(),
            line: 10,
            cases: Some(5),
            total_asserts: 2,
            strong_asserts: 2,
            ..Default::default()
        };
        let h = TestFn {
            name: "test_param".to_string(),
            line: 10,
            cases: Some(2),
            total_asserts: 1,
            strong_asserts: 1,
            ..Default::default()
        };
        let pairs = [TestPair {
            path: "tests/test_foo.py",
            base: &b,
            head: &h,
            forced: false,
        }];
        let settings = crate::config::AssertionGate::default();
        let directive_case = crate::tokens::parse_directives(
            "allow-case-drop: test_param removed slow redundant test cases\n",
            crate::tokens::OverrideSource::PrBody,
        );
        let out = evaluate_assertion_reduction(&pairs, &[], &[], &settings, &directive_case, false)
            .unwrap();
        // The case drop is waived, but the assertion drop is reported
        assert_eq!(out.violations.len(), 1);
        assert_eq!(
            out.violations[0].code,
            "assertion-reduction/assertions-reduced"
        );
        assert_eq!(out.overrides.len(), 1);
        assert_eq!(
            out.overrides[0].code.as_deref(),
            Some("assertion-reduction/test-cases-reduced")
        );

        // When only plain assertions dropped (no case change), allow-case-drop lifts nothing
        let b_plain = TestFn {
            name: "test_plain".to_string(),
            line: 10,
            total_asserts: 2,
            strong_asserts: 2,
            ..Default::default()
        };
        let h_plain = TestFn {
            name: "test_plain".to_string(),
            line: 10,
            total_asserts: 1,
            strong_asserts: 1,
            ..Default::default()
        };
        let pairs_plain = [TestPair {
            path: "tests/test_foo.py",
            base: &b_plain,
            head: &h_plain,
            forced: false,
        }];
        let directive_case_plain = crate::tokens::parse_directives(
            "allow-case-drop: test_plain tried to lift assertion drop\n",
            crate::tokens::OverrideSource::PrBody,
        );
        let out_plain = evaluate_assertion_reduction(
            &pairs_plain,
            &[],
            &[],
            &settings,
            &directive_case_plain,
            false,
        )
        .unwrap();
        assert_eq!(out_plain.violations.len(), 1);
        assert_eq!(
            out_plain.violations[0].code,
            "assertion-reduction/assertions-reduced"
        );
        assert!(out_plain.overrides.is_empty());
    }

    #[test]
    fn test_evaluate_assertion_reduction_cases_preserved_across_tests_emits_note() {
        let b1 = TestFn {
            name: "test_param_1".to_string(),
            line: 10,
            cases: Some(5),
            case_rows: Some(vec![
                "1".to_string(),
                "2".to_string(),
                "3".to_string(),
                "4".to_string(),
                "5".to_string(),
            ]),
            total_asserts: 1,
            strong_asserts: 1,
            ..Default::default()
        };
        let h1 = TestFn {
            name: "test_param_1".to_string(),
            line: 10,
            cases: Some(2),
            case_rows: Some(vec!["1".to_string(), "2".to_string()]),
            total_asserts: 1,
            strong_asserts: 1,
            ..Default::default()
        };
        let b2 = TestFn {
            name: "test_param_2".to_string(),
            line: 30,
            cases: Some(2),
            case_rows: Some(vec!["6".to_string(), "7".to_string()]),
            total_asserts: 1,
            strong_asserts: 1,
            ..Default::default()
        };
        let h2 = TestFn {
            name: "test_param_2".to_string(),
            line: 30,
            cases: Some(5),
            case_rows: Some(vec![
                "6".to_string(),
                "7".to_string(),
                "3".to_string(),
                "4".to_string(),
                "5".to_string(),
            ]),
            total_asserts: 1,
            strong_asserts: 1,
            ..Default::default()
        };
        let pairs = [
            TestPair {
                path: "tests/test_foo.py",
                base: &b1,
                head: &h1,
                forced: false,
            },
            TestPair {
                path: "tests/test_foo.py",
                base: &b2,
                head: &h2,
                forced: false,
            },
        ];
        let settings = crate::config::AssertionGate::default();

        let out = evaluate_assertion_reduction(&pairs, &[], &[], &settings, &[], false).unwrap();
        assert_eq!(out.violations.len(), 0);
        assert!(out.notes.iter().any(|n| n.contains(
            "test cases 5 -> 2 read as preserved across tests in same file (3 of 3 dropped case(s) found moved to another test of the file, 0 not found)"
        )));
    }

    #[test]
    fn test_evaluate_assertion_reduction_non_literal_cases_emits_note() {
        let b = TestFn {
            name: "test_method_source".to_string(),
            line: 10,
            non_literal_cases: true,
            total_asserts: 1,
            strong_asserts: 1,
            ..Default::default()
        };
        let h = TestFn {
            name: "test_method_source".to_string(),
            line: 10,
            non_literal_cases: true,
            total_asserts: 1,
            strong_asserts: 1,
            ..Default::default()
        };
        let pairs = [TestPair {
            path: "TestExample.java",
            base: &b,
            head: &h,
            forced: false,
        }];
        let settings = crate::config::AssertionGate::default();

        let out = evaluate_assertion_reduction(&pairs, &[], &[], &settings, &[], false).unwrap();
        assert_eq!(out.violations.len(), 0);
        assert!(out
            .notes
            .iter()
            .any(|n| n.contains("non-literal test case source in `test_method_source`")));
    }

    /// A paired test with one assertion on each side and the given case facts.
    fn case_test(name: &str, cases: Option<usize>, non_literal_cases: bool) -> TestFn {
        TestFn {
            name: name.to_string(),
            line: 10,
            cases,
            non_literal_cases,
            total_asserts: 1,
            strong_asserts: 1,
            ..Default::default()
        }
    }

    fn case_outcome(b: &TestFn, h: &TestFn, added: &[Located]) -> GateOutcome {
        let pairs = [TestPair {
            path: "tests/test_foo.py",
            base: b,
            head: h,
            forced: false,
        }];
        let settings = crate::config::AssertionGate::default();
        evaluate_assertion_reduction(&pairs, added, &[], &settings, &[], false).unwrap()
    }

    #[test]
    fn a_counted_base_with_a_non_literal_head_is_a_case_reduction() {
        let b = case_test("test_param", Some(3), false);
        let h = case_test("test_param", None, true);
        let out = case_outcome(&b, &h, &[]);
        assert_eq!(out.violations.len(), 1);
        assert_eq!(
            out.violations[0].code,
            "assertion-reduction/test-cases-reduced"
        );
        assert_eq!(
            out.violations[0].message,
            "Test `test_param`: the case source is no longer a literal list and its cases cannot be counted; it had 3 literal cases."
        );
        assert!(out.violations[0]
            .remediation
            .as_deref()
            .is_some_and(|r| r.contains("allow-case-drop: test_param")));
    }

    #[test]
    fn a_counted_base_with_an_unparametrized_head_is_a_case_reduction() {
        let b = case_test("test_param", Some(2), false);
        let h = case_test("test_param", None, false);
        let out = case_outcome(&b, &h, &[]);
        assert_eq!(out.violations.len(), 1);
        assert_eq!(
            out.violations[0].code,
            "assertion-reduction/test-cases-reduced"
        );
        assert_eq!(
            out.violations[0].message,
            "Test `test_param`: ran 2 cases and is no longer read as parametrized; no case list is found on it."
        );
    }

    /// Controls: one literal case is one run either way; an uncounted base says nothing.
    #[test]
    fn an_uncounted_head_is_no_reduction_without_two_counted_base_cases() {
        for (b_cases, b_non_literal, h_cases, h_non_literal) in [
            (Some(1), false, None, false),
            (Some(1), false, None, true),
            (Some(0), false, None, false),
            (None, true, Some(3), false),
            (None, true, None, true),
            (None, false, None, true),
            (None, false, Some(3), false),
        ] {
            let b = case_test("test_param", b_cases, b_non_literal);
            let h = case_test("test_param", h_cases, h_non_literal);
            let out = case_outcome(&b, &h, &[]);
            assert!(
                out.violations.is_empty(),
                "{b_cases:?}/{b_non_literal} -> {h_cases:?}/{h_non_literal}: {:?}",
                out.violations
                    .iter()
                    .map(|v| &v.message)
                    .collect::<Vec<_>>()
            );
        }
    }

    /// The same-file excusal reads an uncounted head as the literal one: cases that
    /// reappear on another test of the file are a note.
    #[test]
    fn an_uncounted_head_whose_cases_reappear_in_the_file_is_a_note() {
        let rows = Some(vec!["1".to_string(), "2".to_string(), "3".to_string()]);
        let b = TestFn {
            case_rows: rows.clone(),
            ..case_test("test_param", Some(3), false)
        };
        let h = case_test("test_param", None, false);
        let moved = TestFn {
            case_rows: rows,
            ..case_test("test_param_table", Some(3), false)
        };
        let added = [Located {
            path: "tests/test_foo.py",
            file_survives: true,
            test: &moved,
        }];
        let out = case_outcome(&b, &h, &added);
        assert!(out.violations.is_empty());
        assert!(
            out.notes.iter().any(|n| n.contains(
                "3 literal test cases no longer counted on it read as preserved across tests in same file (3 of 3 dropped case(s) found moved to another test of the file, 0 not found)"
            )),
            "{:?}",
            out.notes
        );
    }

    /// A paired test with one assertion on each side and these literal case rows.
    fn rows_test(name: &str, rows: &[&str]) -> TestFn {
        TestFn {
            case_rows: Some(rows.iter().map(|r| r.to_string()).collect()),
            ..case_test(name, Some(rows.len()), false)
        }
    }

    fn rows_outcome(pairs: &[(&TestFn, &TestFn)], added: &[&TestFn]) -> GateOutcome {
        let pairs: Vec<TestPair> = pairs
            .iter()
            .map(|(base, head)| TestPair {
                path: "tests/test_foo.py",
                base,
                head,
                forced: false,
            })
            .collect();
        let added: Vec<Located> = added
            .iter()
            .map(|test| Located {
                path: "tests/test_foo.py",
                file_survives: true,
                test,
            })
            .collect();
        let settings = crate::config::AssertionGate::default();
        evaluate_assertion_reduction(&pairs, &added, &[], &settings, &[], false).unwrap()
    }

    #[test]
    fn cases_dropped_beside_unrelated_new_cases_are_a_reduction() {
        let b = rows_test("test_param", &["1", "2", "3", "4"]);
        let h = rows_test("test_param", &["1"]);
        let unrelated = rows_test("test_other", &["7", "8", "9"]);
        let out = rows_outcome(&[(&b, &h)], &[&unrelated]);
        assert_eq!(out.violations.len(), 1, "{:?}", out.notes);
        assert_eq!(
            out.violations[0].message,
            "Test `test_param`: test cases in parametrized / table-driven test dropped from 4 to 1."
        );
        assert_eq!(
            out.notes,
            ["`test_param` in `tests/test_foo.py`: 0 of 3 dropped case(s) found moved to another test of the file, 3 not found"]
        );
    }

    #[test]
    fn cases_that_arrive_on_another_test_of_the_file_are_moved() {
        let b = rows_test("test_param", &["1", "2", "3", "4"]);
        let h = rows_test("test_param", &["1"]);
        let moved = rows_test("test_other", &["4", "3", "2"]);
        let out = rows_outcome(&[(&b, &h)], &[&moved]);
        assert!(out.violations.is_empty(), "{:?}", out.violations);
        assert_eq!(
            out.notes,
            ["`test_param` in `tests/test_foo.py`: test cases 4 -> 1 read as preserved across tests in same file (3 of 3 dropped case(s) found moved to another test of the file, 0 not found)"]
        );

        // Two of the three arrive.
        let partly = rows_test("test_other", &["2", "3", "9"]);
        let out = rows_outcome(&[(&b, &h)], &[&partly]);
        assert_eq!(out.violations.len(), 1);
        assert_eq!(
            out.notes,
            ["`test_param` in `tests/test_foo.py`: 2 of 3 dropped case(s) found moved to another test of the file, 1 not found"]
        );
    }

    /// A case another test already ran on the base side did not arrive there, and one
    /// arrival stands for one dropped case.
    #[test]
    fn a_case_moves_only_to_where_it_was_not_before() {
        let b = rows_test("test_param", &["1", "2", "3"]);
        let h = rows_test("test_param", &["1"]);
        let other_b = rows_test("test_other", &["2", "3"]);
        let other_h = rows_test("test_other", &["2", "3", "7", "8"]);
        let out = rows_outcome(&[(&b, &h), (&other_b, &other_h)], &[]);
        assert_eq!(out.violations.len(), 1, "{:?}", out.notes);

        // Control: the other test did not have them.
        let other_b = rows_test("test_other", &["7", "8"]);
        let out = rows_outcome(&[(&b, &h), (&other_b, &other_h)], &[]);
        assert!(out.violations.is_empty(), "{:?}", out.violations);

        let b = rows_test("test_param", &["1", "1", "1"]);
        let h = rows_test("test_param", &["1"]);
        let once = rows_test("test_other", &["1"]);
        let out = rows_outcome(&[(&b, &h)], &[&once]);
        assert_eq!(out.violations.len(), 1, "{:?}", out.notes);
        let twice = rows_test("test_other", &["1", "1"]);
        let out = rows_outcome(&[(&b, &h)], &[&twice]);
        assert!(out.violations.is_empty(), "{:?}", out.violations);
    }

    /// A row edited in place is not a dropped case: only as many cases as the count
    /// went down by must be found.
    #[test]
    fn a_row_edited_beside_moved_rows_is_not_a_dropped_case() {
        let b = rows_test("test_param", &["1", "2", "3"]);
        let h = rows_test("test_param", &["1 + 0"]);
        let moved = rows_test("test_other", &["2", "3"]);
        let out = rows_outcome(&[(&b, &h)], &[&moved]);
        assert!(out.violations.is_empty(), "{:?}", out.violations);
    }

    /// Cases whose content is not held cannot be compared: a count alone excuses nothing,
    /// on the side that drops or on the side that would supply.
    #[test]
    fn cases_without_content_neither_supply_nor_receive_the_excusal() {
        let b = rows_test("test_param", &["1", "2", "3"]);
        let h = rows_test("test_param", &["1"]);
        let counted_only = case_test("test_other", Some(2), false);
        let out = rows_outcome(&[(&b, &h)], &[&counted_only]);
        assert_eq!(out.violations.len(), 1);

        let b = case_test("test_param", Some(3), false);
        let h = case_test("test_param", Some(1), false);
        let supplies = rows_test("test_other", &["2", "3"]);
        let out = rows_outcome(&[(&b, &h)], &[&supplies]);
        assert_eq!(out.violations.len(), 1);
        assert!(
            out.notes
                .iter()
                .any(|n| n.contains("cannot be compared by content")),
            "{:?}",
            out.notes
        );

        // A head that is no longer a literal list is not excused by rows elsewhere.
        let b = rows_test("test_param", &["1", "2", "3"]);
        let h = case_test("test_param", None, true);
        let all = rows_test("test_other", &["1", "2", "3"]);
        let out = rows_outcome(&[(&b, &h)], &[&all]);
        assert_eq!(out.violations.len(), 1);
        assert!(out.violations[0]
            .message
            .contains("no longer a literal list"));

        // Control: a head with no case source at all, whose rows arrived elsewhere.
        let h = case_test("test_param", None, false);
        let out = rows_outcome(&[(&b, &h)], &[&all]);
        assert!(out.violations.is_empty(), "{:?}", out.violations);
        assert!(
            out.notes.iter().any(|n| n.contains(
                "3 literal test cases no longer counted on it read as preserved across tests in same file (3 of 3 dropped case(s) found moved to another test of the file, 0 not found)"
            )),
            "{:?}",
            out.notes
        );
    }

    #[test]
    fn test_evaluate_assertion_reduction_helper_weakened_reports_violation_and_cites_calling_tests()
    {
        let b_helper = crate::ast::TestHelperFacts {
            name: "check_user".to_string(),
            line: 5,
            end_line: 10,
            total_asserts: 2,
            strong_asserts: 2,
            tautologies: 0,
            fatal_asserts: 0,
            helper_checks: 0,
            equality_exits: 0,
        };
        let h_helper = crate::ast::TestHelperFacts {
            name: "check_user".to_string(),
            line: 5,
            end_line: 9,
            total_asserts: 1,
            strong_asserts: 1,
            tautologies: 0,
            fatal_asserts: 0,
            helper_checks: 0,
            equality_exits: 0,
        };
        let helpers = [HelperPair {
            path: "tests/helpers.py",
            base: &b_helper,
            head: Some(&h_helper),
        }];
        let b_test = TestFn {
            name: "test_login".to_string(),
            line: 20,
            direct_calls: vec!["check_user".to_string()],
            total_asserts: 2,
            strong_asserts: 2,
            ..Default::default()
        };
        let h_test = TestFn {
            name: "test_login".to_string(),
            line: 20,
            direct_calls: vec!["check_user".to_string()],
            total_asserts: 2,
            strong_asserts: 2,
            ..Default::default()
        };
        let pairs = [TestPair {
            path: "tests/test_auth.py",
            base: &b_test,
            head: &h_test,
            forced: false,
        }];
        let settings = crate::config::AssertionGate::default();

        let out =
            evaluate_assertion_reduction(&pairs, &[], &helpers, &settings, &[], false).unwrap();
        assert_eq!(out.violations.len(), 1);
        let v = &out.violations[0];
        assert_eq!(v.code, "assertion-reduction/test-helper-weakened");
        assert_eq!(v.file.as_deref(), Some("tests/helpers.py"));
        assert_eq!(v.line, Some(5));
        assert!(v
            .message
            .contains("Helper `check_user`: effective assertions dropped from 2 to 1"));
        assert!(v.message.contains("(called by `test_login`)"));
        assert!(v
            .remediation
            .as_deref()
            .unwrap()
            .contains("allow-assertion-drop: check_user"));

        // Staged reports warning
        let out_staged =
            evaluate_assertion_reduction(&pairs, &[], &helpers, &settings, &[], true).unwrap();
        assert_eq!(
            out_staged.violations[0].severity,
            crate::config::Severity::Warning
        );
    }

    #[test]
    fn test_evaluate_assertion_reduction_helper_weakened_waived_by_directives() {
        let b_helper = crate::ast::TestHelperFacts {
            name: "check_user".to_string(),
            line: 5,
            end_line: 10,
            total_asserts: 2,
            strong_asserts: 2,
            tautologies: 0,
            fatal_asserts: 0,
            helper_checks: 0,
            equality_exits: 0,
        };
        let h_helper = crate::ast::TestHelperFacts {
            name: "check_user".to_string(),
            line: 5,
            end_line: 9,
            total_asserts: 1,
            strong_asserts: 1,
            tautologies: 0,
            fatal_asserts: 0,
            helper_checks: 0,
            equality_exits: 0,
        };
        let helpers = [HelperPair {
            path: "tests/helpers.py",
            base: &b_helper,
            head: Some(&h_helper),
        }];
        let settings = crate::config::AssertionGate::default();

        // 1. Waived by helper name
        let dir_helper = crate::tokens::parse_directives(
            "allow-assertion-drop: check_user consolidated into api schema\n",
            crate::tokens::OverrideSource::PrBody,
        );
        let out_helper =
            evaluate_assertion_reduction(&[], &[], &helpers, &settings, &dir_helper, false)
                .unwrap();
        assert_eq!(out_helper.violations.len(), 0);
        assert_eq!(out_helper.overrides.len(), 1);
        assert_eq!(
            out_helper.overrides[0].code.as_deref(),
            Some("assertion-reduction/test-helper-weakened")
        );

        // 2. Waived by path
        let dir_path = crate::tokens::parse_directives(
            "allow-assertion-drop: tests/helpers.py consolidated into api schema\n",
            crate::tokens::OverrideSource::PrBody,
        );
        let out_path =
            evaluate_assertion_reduction(&[], &[], &helpers, &settings, &dir_path, false).unwrap();
        assert_eq!(out_path.violations.len(), 0);
        assert_eq!(out_path.overrides.len(), 1);
        assert_eq!(
            out_path.overrides[0].code.as_deref(),
            Some("assertion-reduction/test-helper-weakened")
        );
    }

    #[test]
    fn test_evaluate_assertion_reduction_refactor_into_helper_emits_note() {
        let b = TestFn {
            name: "test_check".to_string(),
            line: 10,
            total_asserts: 2,
            strong_asserts: 2,
            ..Default::default()
        };
        let h = TestFn {
            name: "test_check".to_string(),
            line: 10,
            total_asserts: 1,
            strong_asserts: 1,
            direct_calls: vec!["custom_assert".to_string()],
            ..Default::default()
        };
        let h_helper = crate::ast::TestHelperFacts {
            name: "custom_assert".to_string(),
            line: 30,
            end_line: 35,
            total_asserts: 2,
            strong_asserts: 2,
            tautologies: 0,
            fatal_asserts: 0,
            helper_checks: 0,
            equality_exits: 0,
        };
        let helpers = [HelperPair {
            path: "tests/common.rs",
            base: &h_helper,
            head: Some(&h_helper),
        }];
        let pairs = [TestPair {
            path: "tests/test_foo.rs",
            base: &b,
            head: &h,
            forced: false,
        }];
        let settings = crate::config::AssertionGate::default();

        let out =
            evaluate_assertion_reduction(&pairs, &[], &helpers, &settings, &[], false).unwrap();
        assert_eq!(out.violations.len(), 0);
        assert!(out
            .notes
            .iter()
            .any(|n| n.contains("read as moved into helper `custom_assert`")));
    }

    /// The outcome of one test changing from `b` to `h` beside the given helper pairs.
    fn helper_move_outcome(
        b: &TestFn,
        h: &TestFn,
        helpers: &[HelperPair],
        settings: &crate::config::AssertionGate,
    ) -> GateOutcome {
        let pairs = [TestPair {
            path: "tests/test_api.py",
            base: b,
            head: h,
            forced: false,
        }];
        evaluate_assertion_reduction(&pairs, &[], helpers, settings, &[], false).unwrap()
    }

    fn moved_note(out: &GateOutcome) -> bool {
        out.notes
            .iter()
            .any(|n| n.contains("read as moved into helper"))
    }

    #[test]
    fn test_helper_excusal_newly_called_helper_accounts_for_its_head_checks_only() {
        let settings = crate::config::AssertionGate::default();
        let helper = helper_facts("check_status", 1, 1);
        let helpers = [HelperPair {
            path: "tests/helpers.py",
            base: &helper,
            head: Some(&helper),
        }];
        let b = calling_test(3, 3, &[]);
        let h = calling_test(0, 0, &["check_status"]);
        let out = helper_move_outcome(&b, &h, &helpers, &settings);
        assert_eq!(out.violations.len(), 1, "{:?}", out.notes);
        assert_eq!(
            out.violations[0].code,
            "assertion-reduction/assertions-reduced"
        );
        assert_eq!(
            out.violations[0].message,
            "Test `test_create`: effective assertions dropped from 3 to 1."
        );
        assert!(!moved_note(&out), "{:?}", out.notes);

        // Control: the helper holds as many checks as the test drops.
        let whole = helper_facts("check_status", 3, 3);
        let helpers = [HelperPair {
            path: "tests/helpers.py",
            base: &whole,
            head: Some(&whole),
        }];
        let out = helper_move_outcome(&b, &h, &helpers, &settings);
        assert_eq!(out.violations.len(), 0, "{:?}", out.violations);
        assert!(out.notes.iter().any(|n| n
            .contains("assertions 3 -> 0 read as moved into helper `check_status` (3 check(s))")));
    }

    #[test]
    fn test_helper_excusal_already_called_helper_accounts_for_its_gain_only() {
        let settings = crate::config::AssertionGate::default();
        let base_helper = helper_facts("check_status", 1, 1);
        let b = calling_test(3, 3, &["check_status"]);
        let h = calling_test(0, 0, &["check_status"]);

        let gained_one = helper_facts("check_status", 2, 2);
        let helpers = [HelperPair {
            path: "tests/helpers.py",
            base: &base_helper,
            head: Some(&gained_one),
        }];
        let out = helper_move_outcome(&b, &h, &helpers, &settings);
        assert_eq!(out.violations.len(), 1, "{:?}", out.notes);
        assert_eq!(
            out.violations[0].message,
            "Test `test_create`: effective assertions dropped from 3 to 1."
        );

        // Control: the helper gains the three checks the test drops.
        let gained_three = helper_facts("check_status", 4, 4);
        let helpers = [HelperPair {
            path: "tests/helpers.py",
            base: &base_helper,
            head: Some(&gained_three),
        }];
        let out = helper_move_outcome(&b, &h, &helpers, &settings);
        assert_eq!(out.violations.len(), 0, "{:?}", out.violations);
        assert!(moved_note(&out), "{:?}", out.notes);
    }

    #[test]
    fn test_helper_excusal_matches_the_helper_by_exact_name() {
        let settings = crate::config::AssertionGate::default();
        let check = helper_facts("check", 1, 1);
        let precheck_base = helper_facts("precheck", 1, 1);
        let precheck_head = helper_facts("precheck", 4, 4);
        // `precheck` is paired first: a suffix match would resolve the call to it.
        let helpers = [
            HelperPair {
                path: "tests/helpers.py",
                base: &precheck_base,
                head: Some(&precheck_head),
            },
            HelperPair {
                path: "tests/helpers.py",
                base: &check,
                head: Some(&check),
            },
        ];
        let b = calling_test(3, 3, &["check"]);
        let h = calling_test(0, 0, &["check"]);
        let out = helper_move_outcome(&b, &h, &helpers, &settings);
        assert_eq!(out.violations.len(), 1, "{:?}", out.notes);
        assert_eq!(
            out.violations[0].message,
            "Test `test_create`: effective assertions dropped from 3 to 0."
        );

        // A qualified call still reaches the helper named by its last segment.
        let whole = helper_facts("check", 3, 3);
        let helpers = [HelperPair {
            path: "tests/helpers.py",
            base: &whole,
            head: Some(&whole),
        }];
        let h = calling_test(0, 0, &["helpers.check"]);
        let out = helper_move_outcome(&calling_test(3, 3, &[]), &h, &helpers, &settings);
        assert_eq!(out.violations.len(), 0, "{:?}", out.violations);
        assert!(moved_note(&out), "{:?}", out.notes);
    }

    #[test]
    fn test_helper_excusal_does_not_excuse_a_strength_drop_the_helper_cannot_cover() {
        let settings = crate::config::AssertionGate::default();
        let weak = helper_facts("check_all", 3, 1);
        let helpers = [HelperPair {
            path: "tests/helpers.py",
            base: &weak,
            head: Some(&weak),
        }];
        let b = calling_test(3, 3, &[]);
        let h = calling_test(0, 0, &["check_all"]);
        let out = helper_move_outcome(&b, &h, &helpers, &settings);
        assert_eq!(out.violations.len(), 1, "{:?}", out.notes);
        assert_eq!(
            out.violations[0].message,
            "Test `test_create`: equality / pattern assertions dropped from 3 to 1 (weakened to a looser form)."
        );
    }

    #[test]
    fn test_helper_excusal_counts_each_call_site_and_a_configured_helper_call_once() {
        let settings = crate::config::AssertionGate::default();
        let helper = helper_facts("check_pair", 2, 2);
        let helpers = [HelperPair {
            path: "tests/helpers.py",
            base: &helper,
            head: Some(&helper),
        }];
        // Two calls to a two-check helper stand for four inline assertions; one does not.
        let b = calling_test(4, 4, &[]);
        let twice = calling_test(0, 0, &["check_pair", "check_pair"]);
        let out = helper_move_outcome(&b, &twice, &helpers, &settings);
        assert_eq!(out.violations.len(), 0, "{:?}", out.violations);
        let once = calling_test(0, 0, &["check_pair"]);
        let out = helper_move_outcome(&b, &once, &helpers, &settings);
        assert_eq!(out.violations.len(), 1, "{:?}", out.notes);
        assert_eq!(
            out.violations[0].message,
            "Test `test_create`: effective assertions dropped from 4 to 2."
        );

        // A call to a helper listed in `assert_helper_fns` is already one assertion of the
        // test: the helper's two checks add one more, not two.
        let configured = crate::config::AssertionGate {
            assert_helper_fns: vec!["check_pair".to_string()],
            ..Default::default()
        };
        let b = calling_test(3, 0, &[]);
        let h = calling_test(1, 0, &["check_pair"]);
        let out = helper_move_outcome(&b, &h, &helpers, &configured);
        assert_eq!(out.violations.len(), 1, "{:?}", out.notes);
        assert_eq!(
            out.violations[0].message,
            "Test `test_create`: effective assertions dropped from 3 to 2."
        );
    }

    #[test]
    fn test_evaluate_assertion_reduction_attributed_helper_drop_emits_note() {
        let b_helper = crate::ast::TestHelperFacts {
            name: "helper".to_string(),
            line: 5,
            end_line: 10,
            total_asserts: 2,
            strong_asserts: 2,
            tautologies: 0,
            fatal_asserts: 0,
            helper_checks: 0,
            equality_exits: 0,
        };
        let h_helper = crate::ast::TestHelperFacts {
            name: "helper".to_string(),
            line: 5,
            end_line: 9,
            total_asserts: 1,
            strong_asserts: 1,
            tautologies: 0,
            fatal_asserts: 0,
            helper_checks: 0,
            equality_exits: 0,
        };
        let helpers = [HelperPair {
            path: "tests/test_foo.rs",
            base: &b_helper,
            head: Some(&h_helper),
        }];
        let b_test = TestFn {
            name: "test_foo".to_string(),
            line: 20,
            total_asserts: 2,
            strong_asserts: 2,
            direct_calls: vec!["helper".to_string()],
            ..Default::default()
        };
        let h_test = TestFn {
            name: "test_foo".to_string(),
            line: 20,
            total_asserts: 1,
            strong_asserts: 1,
            direct_calls: vec!["helper".to_string()],
            ..Default::default()
        };
        let pairs = [TestPair {
            path: "tests/test_foo.rs",
            base: &b_test,
            head: &h_test,
            forced: false,
        }];
        let settings = crate::config::AssertionGate::default();

        let out =
            evaluate_assertion_reduction(&pairs, &[], &helpers, &settings, &[], false).unwrap();
        assert_eq!(out.violations.len(), 1);
        assert_eq!(
            out.violations[0].code,
            "assertion-reduction/test-helper-weakened"
        );
        assert!(out
            .notes
            .iter()
            .any(|n| n.contains("attributed to weakened helper `helper`")));
    }
    const TEST_REDUCED: &str = "assertion-reduction/assertions-reduced";

    const PY_TEST_3: &str = "def test_create():\n    r = create()\n    assert r.a == 1\n    assert r.b == 2\n    assert r.c == 3\n";
    const PY_TEST_CALL: &str = "def test_create():\n    r = create()\n    check(r)\n";

    /// #562: a helper the change adds stands for its checks in a test that starts calling
    /// it, whether its file is new or also holds a test; a helper that holds fewer than
    /// the test dropped does not cover the drop.
    #[test]
    fn a_helper_added_by_the_change_stands_for_the_checks_it_holds() {
        let three =
            "def check(r):\n    assert r.a == 1\n    assert r.b == 2\n    assert r.c == 3\n";
        let test = "tests/test_api.py";
        assert_eq!(
            reduction(
                &[
                    ("tests/helpers.py", "", three),
                    (test, PY_TEST_3, PY_TEST_CALL)
                ],
                ""
            ),
            Vec::new()
        );
        assert_eq!(
            reduction(
                &[
                    ("tests/helpers.py", "", PY_CHECK_1),
                    (test, PY_TEST_3, PY_TEST_CALL)
                ],
                ""
            ),
            vec![(TEST_REDUCED.to_string(), test.to_string())]
        );
        let beside = format!("{three}\ndef test_unrelated():\n    assert 1 + 1 == 2\n");
        assert_eq!(
            reduction(
                &[
                    (
                        "tests/test_shared.py",
                        &beside,
                        &format!("# shared\n{beside}")
                    ),
                    (test, PY_TEST_3, PY_TEST_CALL)
                ],
                ""
            ),
            Vec::new()
        );
    }

    /// #562: a helper of the test's own file is already counted in the test, so a pair
    /// for it adds nothing. The test here read three checks and now reads two, both of
    /// them its same-file helper's: counting the helper again would read four and hide
    /// the drop. The same helper in another file is not counted in the test and adds two.
    #[test]
    fn a_helper_of_the_tests_own_file_is_not_counted_twice() {
        let b = calling_test(3, 3, &[]);
        let h = calling_test(2, 2, &["check"]);
        let own = helper_facts("check", 2, 2);
        let helpers = [HelperPair {
            path: "tests/test_api.py",
            base: &own,
            head: Some(&own),
        }];
        let settings = crate::config::AssertionGate::default();
        let gain = helper_call_gain(&b, &h, "tests/test_api.py", &helpers, &[]);
        assert_eq!((gain.total, gain.strong), (0, 0));
        let gain = helper_call_gain(&b, &h, "tests/test_other.py", &helpers, &[]);
        assert_eq!((gain.total, gain.strong), (2, 2));
        let pairs = [TestPair {
            path: "tests/test_api.py",
            base: &b,
            head: &h,
            forced: false,
        }];
        let out =
            evaluate_assertion_reduction(&pairs, &[], &helpers, &settings, &[], false).unwrap();
        assert_eq!(out.violations.len(), 1, "{:?}", out.violations);
    }

    /// #562: a helper in another file is not counted in the test, so a drop in the test
    /// beside a weakened helper is the test's own: it is reported, and a directive naming
    /// the helper lifts the helper's finding only.
    #[test]
    fn a_weakened_helper_in_another_file_does_not_stand_for_the_tests_own_drop() {
        let test = |own: &str| {
            format!(
                "def test_create():\n    r = create()\n    check(r)\n    assert r.s == 200\n{own}"
            )
        };
        let (base, head) = (test("    assert r.id == 1\n"), test(""));
        let files = [
            ("tests/helpers.py", PY_CHECK_2, PY_CHECK_1),
            ("tests/test_api.py", base.as_str(), head.as_str()),
        ];
        assert_eq!(
            reduction(&files, ""),
            vec![
                (HELPER_WEAKENED.to_string(), "tests/helpers.py".to_string()),
                (TEST_REDUCED.to_string(), "tests/test_api.py".to_string()),
            ]
        );
        assert_eq!(
            reduction(&files, "allow-assertion-drop: check moved to the model\n"),
            vec![(TEST_REDUCED.to_string(), "tests/test_api.py".to_string())]
        );
        // Control: the test keeps its own assertions, and the directive lifts all there is.
        let kept = [
            ("tests/helpers.py", PY_CHECK_2, PY_CHECK_1),
            ("tests/test_api.py", base.as_str(), base.as_str()),
        ];
        assert_eq!(
            reduction(&kept, "allow-assertion-drop: check moved to the model\n"),
            Vec::new()
        );
    }

    /// #562: a same-file helper that lost a truthiness check does not stand for an
    /// equality the test lost: the count is covered, the strength is not.
    #[test]
    fn a_weakened_same_file_helper_covers_count_and_strength_or_nothing() {
        let base_helper = helper_facts("check", 2, 1);
        let head_helper = helper_facts("check", 1, 1);
        let helpers = [HelperPair {
            path: "tests/test_api.py",
            base: &base_helper,
            head: Some(&head_helper),
        }];
        let settings = crate::config::AssertionGate::default();
        let run = |b: &TestFn, h: &TestFn| {
            let pairs = [TestPair {
                path: "tests/test_api.py",
                base: b,
                head: h,
                forced: false,
            }];
            evaluate_assertion_reduction(&pairs, &[], &helpers, &settings, &[], false)
                .unwrap()
                .violations
                .iter()
                .map(|v| v.code.to_string())
                .collect::<Vec<_>>()
        };
        // The test lost one check and one equality; the helper lost one check, no equality.
        assert_eq!(
            run(
                &calling_test(4, 3, &["check"]),
                &calling_test(3, 2, &["check"])
            ),
            vec![HELPER_WEAKENED.to_string(), TEST_REDUCED.to_string()]
        );
        // Control: the test lost exactly the helper's check.
        assert_eq!(
            run(
                &calling_test(4, 3, &["check"]),
                &calling_test(3, 3, &["check"])
            ),
            vec![HELPER_WEAKENED.to_string()]
        );
    }

    /// #562: a helper beside the tests that call it, when it loses a check: the tests
    /// that drop with it carry the finding, as before, and the helper is not reported a
    /// second time. A helper no test of the file reaches is reported itself.
    #[test]
    fn a_helper_shown_by_a_dropping_test_of_its_file_is_left_to_the_test() {
        let file = |helper: &str, test_body: &str| {
            format!("{helper}\ndef test_create():\n    r = create()\n{test_body}")
        };
        let path = "tests/test_api.py";
        let (called_base, called_head) = (
            file(PY_CHECK_2, "    check(r)\n"),
            file(PY_CHECK_1, "    check(r)\n"),
        );
        assert_eq!(
            reduction(&[(path, &called_base, &called_head)], ""),
            vec![(TEST_REDUCED.to_string(), path.to_string())]
        );
        let (uncalled_base, uncalled_head) = (
            file(PY_CHECK_2, "    assert r.s == 200\n"),
            file(PY_CHECK_1, "    assert r.s == 200\n"),
        );
        assert_eq!(
            reduction(&[(path, &uncalled_base, &uncalled_head)], ""),
            vec![(HELPER_WEAKENED.to_string(), path.to_string())]
        );
    }

    /// #595: growth in calls to same-file helpers that fail excuses a drop only when
    /// more of those calls check in a loop; a straight-line helper is held to its count.
    #[test]
    fn a_drop_is_read_as_a_refactor_only_into_helpers_that_check_in_a_loop() {
        let settings = crate::config::AssertionGate::default();
        let outcome = |looped: usize| {
            let b = calling_test(3, 3, &[]);
            let mut h = calling_test(1, 1, &["check"]);
            h.helper_checks = 1;
            h.helper_reach.looped = looped;
            helper_move_outcome(&b, &h, &[], &settings)
        };
        let straight = outcome(0);
        assert_eq!(straight.violations.len(), 1, "{:?}", straight.violations);
        assert!(straight.violations[0]
            .message
            .contains("effective assertions dropped from 3 to 1"));
        let looped = outcome(1);
        assert!(looped.violations.is_empty(), "{:?}", looped.violations);
        assert!(looped.notes.iter().any(|n| n.contains(
            "assertions 3 -> 1 read as moved into same-file helpers that fail (0 -> 1 calls)"
        )));
        // A looped call the base already made is no growth.
        let mut b = calling_test(3, 3, &["check"]);
        b.helper_checks = 1;
        b.helper_reach.looped = 1;
        let mut h = calling_test(1, 1, &["check"]);
        h.helper_checks = 1;
        h.helper_reach.looped = 1;
        assert_eq!(
            helper_move_outcome(&b, &h, &[], &settings).violations.len(),
            1
        );
    }

    /// #595: what the test's same-file helpers hold beyond its own count excuses a drop
    /// up to that amount, in count and in strength, and only for what the change added.
    #[test]
    fn the_reach_of_same_file_helpers_excuses_a_drop_up_to_what_they_hold() {
        let reaching = |total: usize, strong: usize| {
            let mut h = calling_test(0, 0, &[]);
            h.helper_reach.receiver_calls = vec!["check".to_string()];
            h.helper_reach.own_file_calls = vec!["check".to_string()];
            h.helper_reach.total = total;
            h.helper_reach.strong = strong;
            h.helper_reach.names = vec!["Checker.check".to_string()];
            h
        };
        let settings = crate::config::AssertionGate::default();
        let b = calling_test(3, 3, &[]);
        let out = helper_move_outcome(&b, &reaching(3, 3), &[], &settings);
        assert!(out.violations.is_empty(), "{:?}", out.violations);
        assert!(moved_note(&out), "{:?}", out.notes);
        // Fewer checks than were dropped, and enough checks of a weaker kind.
        assert_eq!(
            helper_move_outcome(&b, &reaching(2, 2), &[], &settings)
                .violations
                .len(),
            1
        );
        assert_eq!(
            helper_move_outcome(&b, &reaching(3, 1), &[], &settings)
                .violations
                .len(),
            1
        );
        // The base test already reached them: nothing was moved.
        let mut already = reaching(3, 3);
        already.total_asserts = 3;
        already.strong_asserts = 3;
        assert_eq!(
            helper_move_outcome(&already, &reaching(3, 3), &[], &settings)
                .violations
                .len(),
            1
        );
    }

    /// #595: equality checks written by hand in same-file helpers cover a shortfall in
    /// strength when the count holds, and nothing else.
    #[test]
    fn equality_exits_cover_a_strength_only_shortfall() {
        let settings = crate::config::AssertionGate::default();
        let b = calling_test(2, 2, &[]);
        let head = |total: usize, strong: usize, exits: usize| {
            let mut h = calling_test(total, strong, &["check"]);
            h.helper_checks = 1;
            h.helper_reach.equality_exits = exits;
            h
        };
        let out = helper_move_outcome(&b, &head(2, 1, 1), &[], &settings);
        assert!(out.violations.is_empty(), "{:?}", out.violations);
        assert!(out
            .notes
            .iter()
            .any(|n| n.contains("equality assertions 2 -> 1 read as moved into same-file helpers that fail on an equality comparison (1 exit(s))")));
        // Controls: no such exit; fewer exits than equalities lost; a count that drops
        // as well; exits the base test's helpers already held.
        let weakened =
            |h: &TestFn, b: &TestFn| helper_move_outcome(b, h, &[], &settings).violations.len();
        assert_eq!(weakened(&head(2, 1, 0), &b), 1);
        assert_eq!(weakened(&head(2, 0, 1), &b), 1);
        assert_eq!(weakened(&head(1, 0, 2), &b), 1);
        // There the count dropped, so the exits are not set against the strength: the
        // finding is the count's, and no equality is noted as moved.
        let out = helper_move_outcome(&b, &head(1, 0, 2), &[], &settings);
        assert!(
            !out.notes.iter().any(|n| n.contains("equality comparison")),
            "{:?}",
            out.notes
        );
        let mut held = calling_test(2, 2, &["check"]);
        held.helper_checks = 1;
        held.helper_reach.equality_exits = 1;
        assert_eq!(weakened(&head(2, 1, 1), &held), 1);
    }
}
