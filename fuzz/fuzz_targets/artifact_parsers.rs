//! Artifacts a gate reads from a pull request's tree: benchmark output (Criterion,
//! Google Benchmark, pytest-benchmark, `go test -bench`, iai-callgrind, neutral
//! fuel JSON) and source maps. Unparseable input is an `Err` or an empty list.
#![no_main]

use discipline::guards::perf;
use discipline::guards::source_maps::{self, MapRef};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&sel, body)) = data.split_first() else {
        return;
    };
    let text = String::from_utf8_lossy(body);
    let path = if sel & 1 == 0 {
        "bench.json"
    } else {
        "bench.txt"
    };
    let _ = perf::parse_metrics(path, &text);
    let _ = perf::parse_iai_callgrind_console_both(&text);
    let _ = perf::extract_provenance(&text);
    let _ = source_maps::analyse_map(body);
    for r in source_maps::map_refs(&text) {
        if let MapRef::External(url) = r {
            let _ = source_maps::resolve_external("dist/app.js", &url);
        }
    }
});
