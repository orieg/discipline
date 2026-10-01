//! Other text read from a pull-request body or an owner's comment: closing-keyword
//! issue references (four forges) and owner-ratification blocks.
#![no_main]

use discipline::forge::ForgeKind;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    for kind in [
        ForgeKind::GitHub,
        ForgeKind::GitLab,
        ForgeKind::Gitea,
        ForgeKind::Forgejo,
    ] {
        let _ = discipline::references::parse(&text, kind, &[]);
    }
    let _ = discipline::ratification::parse_blocks(&text, "Owner-ratified-paths:");
    let _ = discipline::ratification::refuse_entry(&text);
});
