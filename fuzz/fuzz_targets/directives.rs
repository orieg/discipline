//! Override directives in a pull-request body or commit message. The text is
//! attacker-influenced (a pull request body is written by whoever opens the pull
//! request). Parsing compiles a pattern per call, so this target is the slowest per
//! input; the cheaper text parsers are in `pr_text`.
#![no_main]

use discipline::tokens::{self, OverrideSource};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = tokens::directive_lines(&text, OverrideSource::PrBody);
    let _ = tokens::find_directive_in_subject(&text);
    let _ = tokens::directive_reasons(&text, &["allow-stub", "allow-swallow"]);
    let _ = tokens::is_valid_rationale(&text);
    let _ = tokens::extract_citations(&text);
    let _ = tokens::workflow_marker_reason(&text, "allow-stub");
});
