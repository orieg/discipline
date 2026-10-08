//! Files the diff gates could not read as source: syntax errors and NUL bytes the change
//! adds.

use super::FileFacts;
use crate::guards::{GateOutcome, PathFilter};
use crate::tokens;

pub(crate) fn report_parse_errors(
    files: &[FileFacts],
    severity: crate::config::Severity,
    out: &mut GateOutcome,
    exempt: &PathFilter,
) {
    for ff in files {
        if exempt.matches(&ff.file.path) {
            continue;
        }
        if let Some(h) = ff.head.as_ref() {
            for note in &h.notes {
                out.notes.push(note.clone());
            }
            if h.has_parse_errors {
                let is_c_like = matches!(
                    crate::ast::language_for(&ff.file.path),
                    Some(
                        crate::ast::Language::C
                            | crate::ast::Language::Cpp
                            | crate::ast::Language::CSharp
                            | crate::ast::Language::ObjectiveC
                    )
                );
                // A file with no tests on either side, outside a test path, in a language
                // that keeps its tests in test files: a parse error there hides no test. Rust
                // keeps tests inline in source files, so it stays strict.
                let holds_no_tests = !matches!(
                    crate::ast::language_for(&ff.file.path),
                    Some(crate::ast::Language::Rust)
                ) && !crate::ast::functions::test_path(&ff.file.path)
                    && h.tests.is_empty()
                    && ff.base.as_ref().is_none_or(|b| b.tests.is_empty());
                let sev = if is_c_like || holds_no_tests {
                    crate::config::Severity::Warning
                } else {
                    severity
                };
                let err_line = if is_c_like {
                    h.first_parse_error_line.or(Some(1))
                } else {
                    h.first_parse_error_line
                };
                let (title, msg) = if is_c_like {
                    let line_display = err_line.unwrap_or(1);
                    let lang_name = match crate::ast::language_for(&ff.file.path) {
                        Some(crate::ast::Language::CSharp) => "C#",
                        Some(crate::ast::Language::ObjectiveC) => "Objective-C",
                        _ => "C/C++",
                    };
                    out.notes.push(format!(
                        "file `{}` had {} skipped {} parse error region(s) (first error near line {})",
                        ff.file.path, h.skipped_error_nodes_count, lang_name, line_display
                    ));
                    (
                        &crate::findings::SOURCE_PARSED_WITH_ERRORS_PREPROCESSOR,
                        format!(
                            "{lang_name} grammar encountered preprocessor or syntax errors near line {line_display} ({} skipped AST error region(s)). Surrounding well-formed code was inspected, but some facts may be incomplete.",
                            h.skipped_error_nodes_count
                        ),
                    )
                } else {
                    (
                        &crate::findings::SOURCE_PARSED_WITH_ERRORS,
                        if holds_no_tests {
                            "The grammar reported syntax errors in a file that holds no tests on \
                             either side, so no test can be hidden by them; reported at warning."
                                .to_string()
                        } else {
                            "The grammar reported syntax errors, so assertion and unsafe facts for \
                             this file may be incomplete. A gate that cannot read its input does not pass."
                                .to_string()
                        },
                    )
                };

                out.push(
                    sev,
                    title,
                    Some(&ff.file.path),
                    err_line,
                    msg,
                    "Fix the syntax error, or list the path under `exempt_paths` for the AST gates \
                     if it uses syntax the bundled grammar does not know yet.",
                );
            }
        }
    }
}

/// Newly added NUL bytes in source files flag a violation, liftable by directive.
pub(crate) fn report_newly_added_nul_bytes(
    files: &[FileFacts],
    severity: crate::config::Severity,
    out: &mut GateOutcome,
    directives: &[crate::tokens::ParsedDirective],
    is_staged: bool,
    exempt: &PathFilter,
) {
    for ff in files {
        if exempt.matches(&ff.file.path) {
            continue;
        }
        if ff.newly_added_nul {
            if let Some(record) =
                // The finding lands in the first enabled AST gate: its own marker lifts it.
                tokens::find_override(
                    directives,
                    out.gate,
                    &crate::findings::NUL_BYTE_ADDED,
                    tokens::ALLOW_NUL,
                    &ff.file.path,
                )
                .or_else(|| {
                    let own = format!("discipline:allow({})", out.gate);
                    let short = format!("allow({})", out.gate);
                    tokens::find_override(
                        directives,
                        out.gate,
                        &crate::findings::NUL_BYTE_ADDED,
                        &[own.as_str(), short.as_str()],
                        &ff.file.path,
                    )
                })
            {
                out.overrides.push(record);
                continue;
            }
            let sev = if is_staged {
                crate::config::Severity::Warning
            } else {
                severity
            };
            out.push(
                sev,
                &crate::findings::NUL_BYTE_ADDED,
                Some(&ff.file.path),
                None,
                format!(
                    "Source file `{}` contains a newly added NUL byte; refusing corrupted or binary source without directive.",
                    ff.file.path
                ),
                &format!(
                    "Remove the NUL byte, or justify it on its own line in the PR body or a commit message: `allow-nul: {} <reason>` (or `discipline:allow({}): {} <reason>`).",
                    ff.file.path, out.gate, ff.file.path
                ),
            );
        }
    }
}
