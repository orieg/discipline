//! `scripts/precision_v8.py` builds and scores `agent-pr-precision-v8`
//! (`docs/benchmarks/precision-preregistration-v8.toml`). Its self-checks run here, so a
//! change that breaks how packet items keep their v6 ids fails `cargo test`.

mod common;

use std::path::Path;

#[test]
fn the_v8_script_self_checks_pass() {
    let out = common::script_command("python3")
        .args(["-I", "scripts/precision_v8.py", "--test"])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .output()
        .expect("run python3");
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
