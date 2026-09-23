//! In-memory fixtures for every format the reader accepts, used by the unit tests,
//! the end-to-end tests and `discipline self-test`. The tar, zip, gzip, xz and zstd
//! bytes come from the same third-party encoders a release tool would use; the ar,
//! cpio, RPM and gem framing is written here from the format descriptions.

use std::io::Write;

/// An uncompressed POSIX (ustar) tar holding `entries` as regular files.
pub fn tar(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, data) in entries {
        let mut header = tar::Header::new_ustar();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, name, *data)
            .expect("in-memory tar write");
    }
    builder.into_inner().expect("in-memory tar finish")
}

pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(bytes).expect("in-memory gzip write");
    enc.finish().expect("in-memory gzip finish")
}

pub fn xz(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    lzma_rs::xz_compress(&mut std::io::Cursor::new(bytes), &mut out).expect("in-memory xz");
    out
}

pub fn zstd(bytes: &[u8]) -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(bytes, ruzstd::encoding::CompressionLevel::Fastest)
}

/// A zip holding `entries`, deflated.
pub fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::FileOptions::default();
    for (name, data) in entries {
        writer
            .start_file(*name, options)
            .expect("in-memory zip entry");
        writer.write_all(data).expect("in-memory zip write");
    }
    writer.finish().expect("in-memory zip finish").into_inner()
}

/// A System V / GNU `ar` archive, as `dpkg-deb` writes it.
pub fn ar(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = b"!<arch>\n".to_vec();
    for (name, data) in members {
        let header = format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            name,
            0,
            0,
            0,
            "100644",
            data.len()
        );
        assert_eq!(header.len(), 60);
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(b'\n');
        }
    }
    out
}

/// A `.deb` whose data member is `data_name` holding `data` (already compressed to
/// match the name).
pub fn deb(data_name: &str, data: &[u8]) -> Vec<u8> {
    let control = gzip(&tar(&[(
        "./control",
        b"Package: example\nVersion: 1.0\nArchitecture: all\n",
    )]));
    ar(&[
        ("debian-binary", b"2.0\n"),
        ("control.tar.gz", &control),
        (data_name, data),
    ])
}

/// A `.gem`: `metadata.gz`, `data.tar.gz` holding `files`, `checksums.yaml.gz`.
pub fn gem(files: &[(&str, &[u8])]) -> Vec<u8> {
    let metadata = gzip(b"--- !ruby/object:Gem::Specification\nname: example\n");
    let data = gzip(&tar(files));
    let checksums = gzip(b"---\nSHA256: {}\n");
    tar(&[
        ("metadata.gz", &metadata),
        ("data.tar.gz", &data),
        ("checksums.yaml.gz", &checksums),
    ])
}

/// A cpio archive in the SVR4 `newc` format, as RPM payloads use.
pub fn cpio_newc(entries: &[(&str, &[u8])]) -> Vec<u8> {
    fn pad4(out: &mut Vec<u8>) {
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
    }
    let mut out = Vec::new();
    let mut push = |name: &str, mode: u32, data: &[u8], ino: u32| {
        let name_size = name.len() + 1;
        let header = format!(
            "070701{ino:08X}{mode:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{name_size:08X}{:08X}",
            0, 0, 1, 0, data.len(), 0, 0, 0, 0, 0
        );
        assert_eq!(header.len(), 110);
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        pad4(&mut out);
        out.extend_from_slice(data);
        pad4(&mut out);
    };
    for (i, (name, data)) in entries.iter().enumerate() {
        push(name, 0o100644, data, i as u32 + 1);
    }
    push("TRAILER!!!", 0, &[], 0);
    out
}

/// One RPM header structure with a single index entry and `data_len` bytes of data.
fn rpm_header(tag: u32, data_len: usize) -> Vec<u8> {
    let mut out = vec![0x8e, 0xad, 0xe8, 0x01, 0, 0, 0, 0];
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(&(data_len as u32).to_be_bytes());
    // tag, type (BIN = 7), offset, count
    out.extend_from_slice(&tag.to_be_bytes());
    out.extend_from_slice(&7u32.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&(data_len as u32).to_be_bytes());
    out.extend(std::iter::repeat_n(0xab, data_len));
    out
}

/// An `.rpm`: lead, signature header (padded to 8 bytes), header, `payload` (a
/// compressed cpio archive).
pub fn rpm(payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0xed, 0xab, 0xee, 0xdb, 3, 0, 0, 0, 0, 1];
    let mut name = b"example-1.0-1".to_vec();
    name.resize(66, 0);
    out.extend_from_slice(&name);
    out.extend_from_slice(&[0, 1, 0, 5]); // os, signature type
    out.resize(96, 0);
    // A 5-byte signature data block makes the section 37 bytes long, so the
    // 8-byte padding before the main header is exercised.
    out.extend_from_slice(&rpm_header(1000, 5));
    while !out.len().is_multiple_of(8) {
        out.push(0);
    }
    out.extend_from_slice(&rpm_header(1124, 3));
    out.extend_from_slice(payload);
    out
}
