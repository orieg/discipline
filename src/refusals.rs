//! The registry of every policy refusal a run can report.
//!
//! A refusal is a decision of the run's policy that no gate owns: an override budget
//! that is exceeded, directive overrides without the approval the policy requires, a
//! hidden directive that was not read. It is not a finding: it has no gate, no severity
//! a `[gates.<id>]` table sets, and no baseline entry. So it is not registered in
//! [`crate::findings`], whose every kind names the gate that reports it; it has this
//! table, held to the same rule: a code is frozen once released
//! (docs/ARCHITECTURE.md §3.2), the title and the wording are not.
//!
//! The JSON report is the canonical record of a refusal: `policy_failures` for one that
//! fails the run, a gate note for a hidden directive that was not read. The SARIF, JUnit
//! and GitLab code-quality reports carry a projection of it ([`project`]): the rule id
//! (`policy/<code>`), where the refusal is, and the fixed wording of this table. They
//! never carry what the canonical record quotes (a count, a reviewer's login, a commit)
//! nor anything of a directive (its name, its subject, its reason).

use crate::guards::CheckSummary;
use crate::report::gitlab::sha256_hex;
use crate::tokens::OverrideSource;
use serde::{Serialize, Serializer};

/// The first segment of every refusal's rule id. No gate may take it
/// (`tests::no_refusal_rule_id_is_a_finding_code`).
pub const NAMESPACE: &str = "policy";

/// One kind of refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalKind {
    /// More directive overrides than `directives.max_overrides` allows.
    MaxOverrides,
    /// More inline overrides than `directives.max_inline_overrides` allows.
    MaxInlineOverrides,
    /// Directive overrides without the approving review `directives.require_approval` needs.
    ApprovalRequired,
    /// A directive written where the rendered text does not show it, not read because
    /// `allow_hidden` is false.
    HiddenDirective,
}

/// What the reports say about one kind of refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefusalInfo {
    pub kind: RefusalKind,
    /// Kebab-case code, unique in this table. The rule id is `policy/<code>`.
    pub code: &'static str,
    /// The configuration key that decides it.
    pub key: &'static str,
    /// Whether the refusal fails the run (exit 1). One that does not is reported at the
    /// format's lowest level.
    pub blocks: bool,
    /// Display text. Free to change: nothing keys on it.
    pub title: &'static str,
    /// The whole message of a report entry. It quotes nothing from the run.
    pub message: &'static str,
}

/// Every kind, in the order the reports list them.
pub const REFUSALS: &[RefusalInfo] = &[
    RefusalInfo {
        kind: RefusalKind::MaxOverrides,
        code: "max-overrides-exceeded",
        key: "directives.max_overrides",
        blocks: true,
        title: "Directive Override Budget Exceeded",
        message: "The change applies more directive overrides than `directives.max_overrides` allows, so the run is refused. The counts are in the JSON report (`policy_failures`).",
    },
    RefusalInfo {
        kind: RefusalKind::MaxInlineOverrides,
        code: "max-inline-overrides-exceeded",
        key: "directives.max_inline_overrides",
        blocks: true,
        title: "Inline Override Budget Exceeded",
        message: "The change applies more inline overrides than `directives.max_inline_overrides` allows, so the run is refused. The counts are in the JSON report (`policy_failures`).",
    },
    RefusalInfo {
        kind: RefusalKind::ApprovalRequired,
        code: "approval-required",
        key: "directives.require_approval",
        blocks: true,
        title: "Directive Overrides Await Approval",
        message: "The change applies directive overrides under `directives.require_approval`, and its checked head has no approving review from a reviewer `directives.allowed_override_actors` lists (the author's own approval does not count), so the run is refused. The detail is in the JSON report (`policy_failures`).",
    },
    RefusalInfo {
        kind: RefusalKind::HiddenDirective,
        code: "hidden-directive-refused",
        key: "directives.allow_hidden",
        blocks: false,
        title: "Hidden Directive Not Read",
        message: "A directive written where the rendered text does not show it was not read (`allow_hidden` is false). It lifts no finding; a finding it was written for is reported by its gate.",
    },
];

impl RefusalKind {
    /// The kind's row of [`REFUSALS`].
    pub fn info(self) -> &'static RefusalInfo {
        REFUSALS
            .iter()
            .find(|r| r.kind == self)
            .expect("every refusal kind is registered (tests::every_kind_is_registered_once)")
    }
}

impl RefusalInfo {
    /// `policy/<code>`: the SARIF rule id and the GitLab `check_name`.
    pub fn rule_id(&self) -> String {
        format!("{NAMESPACE}/{}", self.code)
    }
}

/// A refusal that fails the run: its kind, and the sentence the terminal, the step
/// summary and the JSON report print. It is written to JSON as that sentence alone, so
/// `policy_failures` stays a list of strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyFailure {
    pub kind: RefusalKind,
    pub text: String,
}

impl PolicyFailure {
    pub fn new(kind: RefusalKind, text: impl Into<String>) -> Self {
        debug_assert!(kind.info().blocks, "{kind:?} does not fail a run");
        PolicyFailure {
            kind,
            text: text.into(),
        }
    }
}

impl Serialize for PolicyFailure {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.text)
    }
}

/// One refusal as the SARIF, JUnit and GitLab reports carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    pub info: &'static RefusalInfo,
    /// Where a refused directive was written. `None` for a refusal of the run as a whole.
    pub source: Option<OverrideSource>,
    /// Which refusal of this kind at this source this is, from 1.
    pub occurrence: usize,
}

impl Projection {
    /// The file a report that needs one names: the pull request body's synthetic path
    /// (as a gate's finding there has), else none.
    pub fn file(&self) -> Option<&'static str> {
        match self.source {
            Some(OverrideSource::PrBody) => Some("<pr-body>"),
            _ => None,
        }
    }

    /// Where the refusal is, in words: the source of a refused directive, or the
    /// configuration key that refused the run. A commit is named by its object id and a
    /// merged pull request by its number; neither is text an author wrote.
    pub fn place(&self) -> String {
        let place = match &self.source {
            Some(source) => source.to_string(),
            None => self.info.key.to_string(),
        };
        if self.occurrence > 1 {
            format!("{place}, hidden directive {}", self.occurrence)
        } else {
            place
        }
    }

    /// What tells two refusals of one kind apart (docs/ARCHITECTURE.md §7.2, the anchor
    /// rule): the source, and the occurrence number past the first, as a repeated source
    /// line has. `None` when the kind and the run identify it.
    pub fn anchor(&self) -> Option<String> {
        let source = match self.source.as_ref()? {
            OverrideSource::PrBody => "pr-body".to_string(),
            OverrideSource::Commit(oid) => format!("commit:{oid}"),
            OverrideSource::MergedPrBody(n) => format!("merged-pr:{n}"),
            OverrideSource::Inline { file, line } => format!("inline:{file}:{line}"),
        };
        Some(if self.occurrence > 1 {
            format!("{source}#{}", self.occurrence)
        } else {
            source
        })
    }

    /// The version-2 fingerprint formula of a finding with no line
    /// ([`crate::baseline`]): rule id, path, and the anchor when there is one.
    pub fn fingerprint(&self) -> String {
        let content = match self.anchor() {
            Some(a) => format!("anchor:{a}"),
            None => String::new(),
        };
        let source = format!(
            "v2:{}:{}:{}",
            self.info.rule_id(),
            self.file().unwrap_or(""),
            sha256_hex(content.as_bytes())
        );
        sha256_hex(source.as_bytes())
    }
}

/// Every refusal of a run, for the reports that carry a projection: the failures of
/// `policy_failures` in their order, then the hidden directives that were not read, in
/// the order they were met.
pub fn project(summary: &CheckSummary) -> Vec<Projection> {
    let mut out: Vec<Projection> = summary
        .policy_failures
        .iter()
        .map(|f| Projection {
            info: f.kind.info(),
            source: None,
            occurrence: 1,
        })
        .collect();
    let hidden = RefusalKind::HiddenDirective.info();
    for (i, source) in summary.refused_hidden_directives.iter().enumerate() {
        let earlier = summary.refused_hidden_directives[..i]
            .iter()
            .filter(|s| *s == source)
            .count();
        out.push(Projection {
            info: hidden,
            source: Some(source.clone()),
            occurrence: earlier + 1,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_kebab(s: &str) -> bool {
        !s.is_empty()
            && !s.starts_with('-')
            && !s.ends_with('-')
            && !s.contains("--")
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    }

    #[test]
    fn every_kind_is_registered_once() {
        let kinds = [
            RefusalKind::MaxOverrides,
            RefusalKind::MaxInlineOverrides,
            RefusalKind::ApprovalRequired,
            RefusalKind::HiddenDirective,
        ];
        for kind in kinds {
            // Exhaustive: a new variant does not compile until it is listed above.
            match kind {
                RefusalKind::MaxOverrides
                | RefusalKind::MaxInlineOverrides
                | RefusalKind::ApprovalRequired
                | RefusalKind::HiddenDirective => {}
            }
            assert_eq!(REFUSALS.iter().filter(|r| r.kind == kind).count(), 1);
            assert_eq!(kind.info().kind, kind);
        }
        assert_eq!(REFUSALS.len(), kinds.len());
        let mut codes = std::collections::HashSet::new();
        for r in REFUSALS {
            assert!(is_kebab(r.code), "`{}` is not kebab-case", r.code);
            assert!(codes.insert(r.code), "`{}` is registered twice", r.code);
            assert!(r.message.contains(r.key.rsplit('.').next().unwrap()));
        }
    }

    /// A refusal's rule id is in the namespace of no gate, so it can never be a finding
    /// code: no gate, shipped or planned, is named `policy`, no suite is, and no
    /// registered finding code equals a refusal's rule id. Read from the gate table and
    /// the findings registry, so a gate added under that name fails here.
    #[test]
    fn no_refusal_rule_id_is_a_finding_code() {
        for g in crate::config::GATES {
            assert_ne!(g.id, NAMESPACE, "a gate takes the refusals' namespace");
            assert_ne!(
                g.suite.label(),
                NAMESPACE,
                "a suite takes the name of the refusals' JUnit test suite"
            );
        }
        let finding_codes: std::collections::HashSet<String> = crate::findings::FINDINGS
            .iter()
            .flat_map(|k| k.gates.iter().map(move |g| format!("{g}/{}", k.code)))
            .collect();
        for k in crate::findings::FINDINGS {
            for g in k.gates {
                assert_ne!(*g, NAMESPACE, "`{g}/{}`", k.code);
            }
        }
        for r in REFUSALS {
            assert!(
                !finding_codes.contains(&r.rule_id()),
                "`{}` is a finding code",
                r.rule_id()
            );
            assert_eq!(r.rule_id().split('/').next(), Some(NAMESPACE));
        }
    }

    #[test]
    fn a_policy_failure_is_written_to_json_as_its_sentence() {
        let f = PolicyFailure::new(RefusalKind::MaxOverrides, "2 applied; allows 1");
        assert_eq!(
            serde_json::to_value(vec![f]).unwrap(),
            serde_json::json!(["2 applied; allows 1"])
        );
    }

    fn at(source: Option<OverrideSource>, occurrence: usize) -> Projection {
        Projection {
            info: RefusalKind::HiddenDirective.info(),
            source,
            occurrence,
        }
    }

    #[test]
    fn a_fingerprint_is_keyed_by_kind_source_and_occurrence_and_nothing_else() {
        let commit = |oid: &str| Some(OverrideSource::Commit(oid.to_string()));
        let all = [
            at(Some(OverrideSource::PrBody), 1),
            at(Some(OverrideSource::PrBody), 2),
            at(commit("aaaa"), 1),
            at(commit("bbbb"), 1),
            at(Some(OverrideSource::MergedPrBody(7)), 1),
            at(Some(OverrideSource::MergedPrBody(8)), 1),
            Projection {
                info: RefusalKind::MaxOverrides.info(),
                source: None,
                occurrence: 1,
            },
            Projection {
                info: RefusalKind::MaxInlineOverrides.info(),
                source: None,
                occurrence: 1,
            },
            Projection {
                info: RefusalKind::ApprovalRequired.info(),
                source: None,
                occurrence: 1,
            },
        ];
        let prints: std::collections::HashSet<String> =
            all.iter().map(Projection::fingerprint).collect();
        assert_eq!(prints.len(), all.len());
        // Pinned: the formula is the baseline's version 2 for a finding with no line.
        assert_eq!(
            all[6].fingerprint(),
            sha256_hex(format!("v2:policy/max-overrides-exceeded::{}", sha256_hex(b"")).as_bytes())
        );
        assert_eq!(
            all[1].fingerprint(),
            sha256_hex(
                format!(
                    "v2:policy/hidden-directive-refused:<pr-body>:{}",
                    sha256_hex(b"anchor:pr-body#2")
                )
                .as_bytes()
            )
        );
        assert_eq!(all[0].place(), "PR body");
        assert_eq!(all[1].place(), "PR body, hidden directive 2");
        assert_eq!(all[2].place(), "commit aaaa");
        assert_eq!(all[6].place(), "directives.max_overrides");
    }
}
