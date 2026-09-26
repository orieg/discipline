//! Why a run could not check (exit 2), as a machine-readable reason.
//!
//! An error that stops a check is tagged where it arises ([`tag`]): the configuration
//! could not be read, a tool a gate runs is missing, the forge could not be reached, ...
//! [`classify`] returns the innermost tag, which the JSON report (`could_not_check`),
//! `discipline replay` and `discipline mcp` carry. A tagged error displays exactly as the
//! error it wraps, so messages on stderr do not change.
//!
//! The set of reasons is open: a new kind of failure may get a new reason in a minor
//! release. A failure that has a reason keeps it (docs/ARCHITECTURE.md §3.2).

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    /// `discipline.toml`, `--config`, `--config-override` or a `DISCIPLINE_*` setting is
    /// missing, unreadable or invalid.
    Configuration,
    /// The baseline file named or found could not be read.
    Baseline,
    /// Not a git repository, the base does not resolve, or git could not be read.
    Repository,
    /// A tool a gate runs was not found or could not start.
    ToolMissing,
    /// A tool a gate runs exceeded its timeout.
    ToolTimeout,
    /// A tool ran and reported that its toolchain is unavailable (Miri, a sanitizer).
    ToolchainUnavailable,
    /// The forge's API could not be reached or refused a read the check needs.
    Forge,
    /// A gate could not run, for a reason no narrower tag names.
    Gate,
    /// Anything else.
    Internal,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Configuration => "configuration",
            Reason::Baseline => "baseline",
            Reason::Repository => "repository",
            Reason::ToolMissing => "tool-missing",
            Reason::ToolTimeout => "tool-timeout",
            Reason::ToolchainUnavailable => "toolchain-unavailable",
            Reason::Forge => "forge",
            Reason::Gate => "gate",
            Reason::Internal => "internal",
        }
    }

    /// The reason a report names, if this binary knows it.
    pub fn parse(s: &str) -> Option<Reason> {
        Reason::ALL.iter().copied().find(|r| r.as_str() == s)
    }

    /// Every reason, for the JSON Schemas and their tests.
    pub const ALL: &'static [Reason] = &[
        Reason::Configuration,
        Reason::Baseline,
        Reason::Repository,
        Reason::ToolMissing,
        Reason::ToolTimeout,
        Reason::ToolchainUnavailable,
        Reason::Forge,
        Reason::Gate,
        Reason::Internal,
    ];
}

/// An error tagged with why it stopped the check.
#[derive(Debug)]
pub struct Tagged {
    pub reason: Reason,
    pub gate: Option<String>,
    inner: anyhow::Error,
}

impl std::fmt::Display for Tagged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#}", self.inner)
    }
}

impl std::error::Error for Tagged {}

/// Tag `e` with the reason it stops a check.
pub fn tag(reason: Reason, e: impl Into<anyhow::Error>) -> anyhow::Error {
    anyhow::Error::new(Tagged {
        reason,
        gate: None,
        inner: e.into(),
    })
}

/// Tag `e` as the failure of gate `gate`; a narrower tag inside it keeps its reason.
pub fn tag_gate(gate: &str, e: anyhow::Error) -> anyhow::Error {
    anyhow::Error::new(Tagged {
        reason: Reason::Gate,
        gate: Some(gate.to_string()),
        inner: e,
    })
}

/// The reason of the innermost tag in `e`, with the nearest gate that encloses it.
/// Untagged is [`Reason::Internal`].
pub fn classify(e: &anyhow::Error) -> (Reason, Option<String>) {
    fn walk(e: &anyhow::Error) -> Option<(Reason, Option<String>)> {
        for layer in e.chain() {
            if let Some(t) = layer.downcast_ref::<Tagged>() {
                return Some(match walk(&t.inner) {
                    Some((reason, gate)) => (reason, gate.or_else(|| t.gate.clone())),
                    None => (t.reason, t.gate.clone()),
                });
            }
        }
        None
    }
    walk(e).unwrap_or((Reason::Internal, None))
}

/// The `could_not_check` object of the JSON report.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CouldNotCheck {
    pub reason: Reason,
    /// The gate that could not run, when one did.
    pub gate: Option<String>,
    /// The error, as printed on stderr.
    pub detail: String,
}

impl CouldNotCheck {
    pub fn from_error(e: &anyhow::Error) -> Self {
        let (reason, gate) = classify(e);
        CouldNotCheck {
            reason,
            gate,
            detail: format!("{e:#}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn the_innermost_tag_wins_and_keeps_the_enclosing_gate() {
        let tool = tag(Reason::ToolMissing, anyhow!("tool `cargo` not found"));
        let gate = tag_gate("miri", tool.context("gate `miri` could not run"));
        assert_eq!(
            classify(&gate),
            (Reason::ToolMissing, Some("miri".to_string()))
        );
        // Display is unchanged by the tags.
        assert_eq!(
            format!("{gate:#}"),
            "gate `miri` could not run: tool `cargo` not found"
        );
        // A context added outside a tag does not hide it.
        let outer = tag(Reason::Configuration, anyhow!("unknown gate `x`")).context("loading");
        assert_eq!(classify(&outer), (Reason::Configuration, None));
        assert_eq!(classify(&anyhow!("plain")), (Reason::Internal, None));
        let bare_gate = tag_gate("pii", anyhow!("broken"));
        assert_eq!(
            classify(&bare_gate),
            (Reason::Gate, Some("pii".to_string()))
        );
    }

    #[test]
    fn reason_codes_are_kebab_case_and_serialize_as_written() {
        for r in Reason::ALL {
            assert_eq!(
                serde_json::to_value(r).unwrap(),
                serde_json::json!(r.as_str())
            );
        }
    }
}
