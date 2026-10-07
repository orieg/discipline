//! Language-pack extraction on arbitrary bytes as source text.
//!
//! The first byte picks a pack (low 5 bits), whether the path is a test path (bit 5)
//! and whether the assertion vocabulary is customised (bit 6); the rest is the source.
//! `extract` may return `Err`; it must not panic or hang.
//!
//! The body runs on the deep stack the binary does its work on
//! (`discipline::deep_stack`): an input can nest to the tree depth limit, which a
//! walker descends on that stack and not on the fuzzer's own.
#![no_main]

use discipline::ast::{default_registry, AssertVocabulary};
use libfuzzer_sys::fuzz_target;

const PATHS: &[&str] = &[
    "lib.rs", "m.py", "m.js", "m.ts", "m.tsx", "M.java", "m.go", "m.php", "m.c", "m.h", "m.cpp",
    "m.hpp", "M.cs", "m.rb", "m.kt", "m.swift", "m.scala", "m.m", "m.mm", "m.phpt",
];

fuzz_target!(|data: &[u8]| {
    discipline::deep_stack::on_deep_stack(|| {
        let Some((&sel, body)) = data.split_first() else {
            return;
        };
        let name = PATHS[(sel & 0x1f) as usize % PATHS.len()];
        let path = if sel & 0x20 != 0 {
            format!("tests/{name}")
        } else {
            format!("src/{name}")
        };
        let vocab = if sel & 0x40 != 0 {
            AssertVocabulary {
                extra_macros: vec!["check".into()],
                helper_fns: vec!["helper".into()],
                test_functions: vec!["scenario".into()],
                test_paths: vec!["**/*.x".into()],
                ..AssertVocabulary::default()
            }
        } else {
            AssertVocabulary::default()
        };
        let src = String::from_utf8_lossy(body);
        let registry = default_registry();
        if let Some(pack) = registry.find_pack(&path) {
            let _ = pack.extract(&path, &src, &vocab);
        }
        // The Rust entry point every gate shares.
        let _ = discipline::ast::analyze(&src, &vocab);
    })
    .expect("the deep stack");
});
