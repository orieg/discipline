//! YAML, workflow, citation metadata, and lockfile parsers (`serde_yaml`, `lockfile`,
//! `ci_gitlab`, `citation_metadata`). Arbitrary bytes are fed as lockfile or workflow
//! documents; unparseable input must return `None` or `Err`, never panic or hang.
#![no_main]

use discipline::guards::ci_gitlab;
use discipline::guards::citation_metadata;
use discipline::guards::lockfile;
use libfuzzer_sys::fuzz_target;

const LOCK_FILES: &[&str] = &[
    "pnpm-lock.yaml",
    "yarn.lock",
    "Cargo.lock",
    "package-lock.json",
    "poetry.lock",
    "uv.lock",
    "composer.lock",
    "Gemfile.lock",
];

fuzz_target!(|data: &[u8]| {
    let Some((&sel, body)) = data.split_first() else {
        return;
    };
    let text = String::from_utf8_lossy(body);
    let mid = text.floor_char_boundary(text.len() / 2);
    let (base_text, head_text) = text.split_at(mid);

    let lock_name = LOCK_FILES[usize::from(sel) % LOCK_FILES.len()];
    if let Some(entries) = lockfile::parse_lock(lock_name, &text) {
        let entry_mid = entries.len() / 2;
        let _ = lockfile::diff_lock(&entries[..entry_mid], &entries[entry_mid..]);
    }
    if let (Some(base_entries), Some(head_entries)) = (
        lockfile::parse_lock(lock_name, base_text),
        lockfile::parse_lock(lock_name, head_text),
    ) {
        let _ = lockfile::diff_lock(&base_entries, &head_entries);
    }

    let _ = ci_gitlab::includes(&text);
    let _ = ci_gitlab::diff_gitlab_ci(base_text, head_text);
    let base_docs = [base_text.to_string()];
    let head_docs = [head_text.to_string()];
    let _ = ci_gitlab::unparsed_script_jobs(&base_docs, &head_docs);

    let _ = citation_metadata::check(Some(base_text), Some(head_text));
});
