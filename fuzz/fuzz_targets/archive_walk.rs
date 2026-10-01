//! `archive-contents`: the archive reader on arbitrary bytes. The first byte picks the
//! file name (and so the format the name claims: zip, tar and its compressions, gem,
//! deb, rpm); the rest is the file. A malformed or mislabelled archive is an `Err`.
#![no_main]

use discipline::guards::archive_formats::{self, EntryData};
use libfuzzer_sys::fuzz_target;
use std::io::Read;

const NAMES: &[&str] = &[
    "a.zip",
    "a.whl",
    "a.jar",
    "a.tar",
    "a.tar.gz",
    "a.tgz",
    "a.tar.bz2",
    "a.tar.xz",
    "a.tar.zst",
    "a.gem",
    "a.deb",
    "a.rpm",
    "a.bin",
];

fuzz_target!(|data: &[u8]| {
    let Some((&sel, body)) = data.split_first() else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("discipline-fuzz-{}", std::process::id()));
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(NAMES[sel as usize % NAMES.len()]);
    if std::fs::write(&path, body).is_err() {
        return;
    }
    let mut visit = |entry: archive_formats::Entry<'_>| {
        let _ = (&entry.name, entry.size, entry.is_dir);
        if let EntryData::Bytes(r) = entry.data {
            // Bounded: a decompression bomb is the gate's own size limit to stop,
            // not this harness's to wait on.
            let _ = std::io::copy(&mut r.take(1 << 20), &mut std::io::sink());
        }
        Ok(())
    };
    let _ = archive_formats::walk(&path, &mut visit);
    let _ = archive_formats::detect(&path);
    let _ = std::fs::remove_file(&path);
});
