//! What the published crate carries: `package.include` in `Cargo.toml` is an
//! allow-list, so a file missing from it is missing from the crate. `LICENSE` only
//! points at `LICENSE-APACHE` and `LICENSE-MIT`; both texts must ship with it.
//!
//! The manifest is read directly rather than through `cargo package`, which is slow
//! and may reach the network.

use std::path::Path;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The entries of `package.include`.
fn include() -> Vec<String> {
    let manifest = std::fs::read_to_string(root().join("Cargo.toml")).unwrap();
    let manifest: toml::Value = toml::from_str(&manifest).unwrap();
    manifest["package"]["include"]
        .as_array()
        .expect("`package.include` is an array")
        .iter()
        .map(|v| {
            v.as_str()
                .expect("an `include` entry is a string")
                .to_string()
        })
        .collect()
}

#[test]
fn both_license_texts_are_packaged() {
    let include = include();
    for name in ["LICENSE-APACHE", "LICENSE-MIT"] {
        assert!(
            include.iter().any(|e| e == name),
            "`{name}` is not in `package.include`: {include:?}"
        );
    }
}

#[test]
fn every_plain_include_entry_names_a_file_at_the_root() {
    let plain: Vec<String> = include()
        .into_iter()
        .filter(|e| !e.contains(['*', '?', '[', ']', '{', '}', '!']))
        .collect();
    assert!(
        plain.iter().any(|e| e == "Cargo.toml"),
        "no plain entry was examined: {plain:?}"
    );
    for entry in &plain {
        assert!(
            root().join(entry).is_file(),
            "`package.include` names `{entry}`, which is not a file at the repository root"
        );
    }
}

#[test]
fn every_license_file_at_the_root_is_packaged() {
    let include = include();
    let mut licenses: Vec<String> = std::fs::read_dir(root())
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.file_type().unwrap().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("LICENSE"))
        .collect();
    licenses.sort();
    assert!(
        licenses.iter().any(|n| n == "LICENSE"),
        "no license file was found at the repository root: {licenses:?}"
    );
    for name in &licenses {
        assert!(
            include.iter().any(|e| e == name),
            "`{name}` is at the repository root but not in `package.include`: {include:?}"
        );
    }
}
