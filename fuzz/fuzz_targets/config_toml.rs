//! `discipline.toml` parsing: a configuration that does not parse must be an `Err`
//! (fail closed, exit 2), never a panic.
#![no_main]

use discipline::DisciplineConfig;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = DisciplineConfig::from_toml_str(&text);
});
