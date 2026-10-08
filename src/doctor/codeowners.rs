//! `CODEOWNERS`: whether the gate configuration has an owner, read as the forge reads the
//! file (the last matching rule wins).

use super::{Finding, Status};

/// `CODEOWNERS` locations, in the order GitHub resolves them.
pub const CODEOWNERS_PATHS: &[&str] = &[".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"];

/// One `CODEOWNERS` rule.
#[derive(Debug, Clone)]
struct OwnerRule {
    matcher: globset::GlobMatcher,
    owned: bool,
}

fn owner_rules(content: &str) -> Vec<OwnerRule> {
    let mut rules = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(pattern) = parts.next() else {
            continue;
        };
        let owned = parts.next().is_some_and(|o| !o.starts_with('#'));
        // gitignore-style: a leading `/` anchors; a pattern without an inner `/` matches
        // at any depth; a trailing `/` (or a directory) owns everything below it.
        let anchored = pattern.starts_with('/');
        let trimmed = pattern.trim_start_matches('/');
        let dir = trimmed.ends_with('/');
        let body = trimmed.trim_end_matches('/');
        let mut glob = if anchored || body.contains('/') {
            body.to_string()
        } else {
            format!("**/{body}")
        };
        if dir || !body.contains('*') {
            // A literal may name a directory; own it and everything under it.
            glob = format!("{{{glob},{glob}/**}}");
        }
        if let Ok(g) = globset::GlobBuilder::new(&glob)
            .literal_separator(true)
            .build()
        {
            rules.push(OwnerRule {
                matcher: g.compile_matcher(),
                owned,
            });
        }
    }
    rules
}

/// Whether `path` has an owner under `CODEOWNERS` content (the last matching rule wins).
pub fn codeowners_covers(content: &str, path: &str) -> bool {
    owner_rules(content)
        .iter()
        .rev()
        .find(|r| r.matcher.is_match(path))
        .is_some_and(|r| r.owned)
}

/// Check `CODEOWNERS` against the files that configure the gate.
pub fn codeowners_finding(codeowners: Option<(&str, &str)>, targets: &[String]) -> Finding {
    let Some((path, content)) = codeowners else {
        return Finding::new("codeowners", Status::Warn, "no CODEOWNERS file").fix(format!(
            "Add .github/CODEOWNERS owning {} so weakening them needs a named reviewer.",
            targets.join(", ")
        ));
    };
    let missing: Vec<&String> = targets
        .iter()
        .filter(|t| !codeowners_covers(content, t))
        .collect();
    if missing.is_empty() {
        Finding::new(
            "codeowners",
            Status::Pass,
            format!("{path} owns {}", targets.join(", ")),
        )
    } else {
        Finding::new(
            "codeowners",
            Status::Warn,
            format!(
                "{path} has no owner for {}",
                missing
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
        .fix("Add CODEOWNERS rules for the gate configuration and workflow files.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::security_policy_finding;

    #[test]
    fn codeowners_last_match_wins_and_directories_are_owned() {
        let co = "* @team\n/.github/workflows/ @ci\ndiscipline.toml @gate\n/docs/\n";
        assert!(codeowners_covers(co, "discipline.toml"));
        assert!(codeowners_covers(co, ".github/workflows/ci.yml"));
        assert!(codeowners_covers(co, "src/lib.rs"));
        // A later rule without owners removes ownership.
        assert!(!codeowners_covers(co, "docs/index.md"));
        assert!(!codeowners_covers("/src/ @x\n", "discipline.toml"));
        // Unanchored names match at any depth; anchored ones only at the root.
        assert!(codeowners_covers(
            "discipline.toml @x\n",
            "sub/discipline.toml"
        ));
        assert!(!codeowners_covers(
            "/discipline.toml @x\n",
            "sub/discipline.toml"
        ));

        let targets = vec![
            "discipline.toml".to_string(),
            ".github/workflows/ci.yml".to_string(),
        ];
        assert_eq!(codeowners_finding(None, &targets).status, Status::Warn);
        assert_eq!(
            security_policy_finding(Some(".github/SECURITY.md")).status,
            Status::Pass
        );
        assert_eq!(security_policy_finding(None).status, Status::Info);
        assert_eq!(
            codeowners_finding(Some(("CODEOWNERS", "/discipline.toml @x\n")), &targets).status,
            Status::Warn
        );
        assert_eq!(
            codeowners_finding(Some(("CODEOWNERS", co)), &targets).status,
            Status::Pass
        );
    }
}
