//! Archive readers behind `archive-contents`.
//!
//! The format is decided by the file's magic bytes. The extension, when it names a
//! format, must agree with them: a `.tar.gz` that holds an HTML error page, or a
//! `.zip` that holds a gzip stream, is refused rather than guessed at. Formats the
//! gate cannot read (disk images, installers, bare binaries) are refused by name, so
//! an `archive_path` that points at one exits 2 instead of passing.
//!
//! Nested archives are not followed, with two exceptions that are part of the
//! package format itself: a `.gem`'s `data.tar.gz` (entries prefixed `data/`) and a
//! `.deb`'s `data.tar.*` member.

use anyhow::{anyhow, bail, Context as _, Result};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub mod fixtures;

/// Compression wrapped around a tar stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Gzip,
    Bzip2,
    Xz,
    Zstd,
}

/// A package format the gate reads entries from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Zip,
    Tar(Compression),
    Gem,
    Deb,
    Rpm,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Zip => "zip archive",
            Format::Tar(Compression::None) => "tar archive",
            Format::Tar(Compression::Gzip) => "gzip-compressed tar archive",
            Format::Tar(Compression::Bzip2) => "bzip2-compressed tar archive",
            Format::Tar(Compression::Xz) => "xz-compressed tar archive",
            Format::Tar(Compression::Zstd) => "zstd-compressed tar archive",
            Format::Gem => "RubyGems package",
            Format::Deb => "Debian package",
            Format::Rpm => "RPM package",
        }
    }
}

/// What the gate can do with an entry's bytes.
pub enum EntryData<'a> {
    /// The decoded bytes of the entry.
    Bytes(&'a mut dyn Read),
    /// A package-format member whose own entries are reported after it (a `.gem`'s
    /// `data.tar.gz`); its bytes are consumed by that read.
    Expanded,
    /// The bytes cannot be decoded by this reader, and why.
    Undecodable(String),
}

/// One entry of an archive.
pub struct Entry<'a> {
    /// Path inside the archive, `/`-separated, as stored (no stripping).
    pub name: String,
    /// Uncompressed size in bytes, as the archive declares it.
    pub size: u64,
    pub is_dir: bool,
    pub data: EntryData<'a>,
}

/// Visitor called once per entry, in archive order.
pub type Visit<'v> = dyn FnMut(Entry<'_>) -> Result<()> + 'v;

/// Extensions of the zip-based package formats.
pub const ZIP_EXTENSIONS: &[&str] = &[
    ".zip", ".whl", ".jar", ".war", ".ear", ".aar", ".apk", ".nupkg", ".snupkg", ".vsix", ".xpi",
    ".ipa",
];

/// Extensions that name a format the gate does not read. Checked before the magic
/// bytes, since a disk image has no reliable leading signature.
pub const REFUSED_EXTENSIONS: &[(&str, &str)] = &[
    (".dmg", "Apple disk image (.dmg)"),
    (".msi", "Windows Installer package (.msi)"),
    (".exe", "Windows executable (.exe)"),
    (".appimage", "AppImage executable"),
    (".snap", "snap package (squashfs image)"),
    (".iso", "ISO 9660 disk image"),
    (".7z", "7-Zip archive"),
    (".rar", "RAR archive"),
];

/// What the file name promises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    Format(Format),
    /// `.pkg`: a FreeBSD package is a compressed tar, a macOS installer is xar.
    ByMagic,
    Refused(&'static str),
}

fn expect_from_name(lower: &str) -> Option<Expect> {
    if let Some((_, label)) = REFUSED_EXTENSIONS
        .iter()
        .find(|(ext, _)| lower.ends_with(ext))
    {
        return Some(Expect::Refused(label));
    }
    if ZIP_EXTENSIONS.iter().any(|ext| lower.ends_with(ext)) {
        return Some(Expect::Format(Format::Zip));
    }
    let tar = |c| Some(Expect::Format(Format::Tar(c)));
    let ends = |exts: &[&str]| exts.iter().any(|e| lower.ends_with(e));
    if ends(&[".tar.gz", ".tgz", ".crate"]) {
        tar(Compression::Gzip)
    } else if ends(&[".tar.bz2", ".tbz2", ".tbz"]) {
        tar(Compression::Bzip2)
    } else if ends(&[".tar.xz", ".txz"]) {
        tar(Compression::Xz)
    } else if ends(&[".tar.zst", ".tar.zstd", ".tzst"]) {
        tar(Compression::Zstd)
    } else if lower.ends_with(".tar") {
        tar(Compression::None)
    } else if lower.ends_with(".gem") {
        Some(Expect::Format(Format::Gem))
    } else if ends(&[".deb", ".udeb"]) {
        Some(Expect::Format(Format::Deb))
    } else if lower.ends_with(".rpm") {
        Some(Expect::Format(Format::Rpm))
    } else if lower.ends_with(".pkg") {
        Some(Expect::ByMagic)
    } else {
        None
    }
}

/// What the leading bytes say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Magic {
    Zip,
    Compressed(Compression),
    Ar,
    Rpm,
    Tar,
    Unsupported(&'static str),
    Unknown,
}

impl Magic {
    fn label(self) -> &'static str {
        match self {
            Magic::Zip => "a zip archive",
            Magic::Compressed(Compression::Gzip) => "a gzip stream",
            Magic::Compressed(Compression::Bzip2) => "a bzip2 stream",
            Magic::Compressed(Compression::Xz) => "an xz stream",
            Magic::Compressed(Compression::Zstd) => "a zstd stream",
            Magic::Compressed(Compression::None) | Magic::Tar => "a tar archive",
            Magic::Ar => "an ar archive",
            Magic::Rpm => "an RPM package",
            Magic::Unsupported(label) => label,
            Magic::Unknown => "no recognised format",
        }
    }
}

fn sniff(head: &[u8]) -> Magic {
    let starts = |m: &[u8]| head.starts_with(m);
    if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") || starts(b"PK\x07\x08") {
        Magic::Zip
    } else if starts(&[0x1f, 0x8b]) {
        Magic::Compressed(Compression::Gzip)
    } else if starts(b"BZh") {
        Magic::Compressed(Compression::Bzip2)
    } else if starts(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
        Magic::Compressed(Compression::Xz)
    } else if starts(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Magic::Compressed(Compression::Zstd)
    } else if starts(b"!<arch>\n") {
        Magic::Ar
    } else if starts(&[0xed, 0xab, 0xee, 0xdb]) {
        Magic::Rpm
    } else if head.len() >= 262 && &head[257..262] == b"ustar" {
        Magic::Tar
    } else if starts(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]) {
        Magic::Unsupported("an OLE compound file (Windows Installer .msi)")
    } else if starts(b"xar!") {
        Magic::Unsupported("a xar archive (macOS installer .pkg)")
    } else if starts(b"\x7fELF") {
        Magic::Unsupported("an ELF binary")
    } else if starts(&[0xfe, 0xed, 0xfa, 0xce])
        || starts(&[0xfe, 0xed, 0xfa, 0xcf])
        || starts(&[0xce, 0xfa, 0xed, 0xfe])
        || starts(&[0xcf, 0xfa, 0xed, 0xfe])
    {
        Magic::Unsupported("a Mach-O binary")
    } else if starts(&[0xca, 0xfe, 0xba, 0xbe]) {
        Magic::Unsupported("a Mach-O universal binary or Java class file")
    } else if starts(b"MZ") {
        Magic::Unsupported("a Windows PE executable")
    } else if starts(b"hsqs") {
        Magic::Unsupported("a squashfs image")
    } else if starts(&[b'7', b'z', 0xbc, 0xaf, 0x27, 0x1c]) {
        Magic::Unsupported("a 7-Zip archive")
    } else if starts(b"Rar!\x1a\x07") {
        Magic::Unsupported("a RAR archive")
    } else if starts(b"LZIP") {
        Magic::Unsupported("an lzip stream")
    } else {
        Magic::Unknown
    }
}

/// An Apple disk image carries its `koly` block in the last 512 bytes.
fn has_dmg_trailer(file: &mut File) -> Result<bool> {
    let len = file.metadata()?.len();
    if len < 512 {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(len - 512))?;
    let mut tag = [0u8; 4];
    file.read_exact(&mut tag)?;
    Ok(&tag == b"koly")
}

/// Decides the format of `path` from its magic bytes, cross-checked against its name.
/// Fails, naming the format, for anything the gate does not read.
pub fn detect(path: &Path) -> Result<Format> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let lower = name.to_lowercase();
    let expect = expect_from_name(&lower);
    if let Some(Expect::Refused(label)) = expect {
        bail!("`{name}` is a {label}; archive-contents does not read this format (not analysed)");
    }

    let mut file = File::open(path)?;
    let mut head = Vec::with_capacity(512);
    (&mut file).take(512).read_to_end(&mut head)?;
    let magic = sniff(&head);
    if let Magic::Unsupported(label) = magic {
        bail!("`{name}` is {label}; archive-contents does not read this format (not analysed)");
    }
    if magic == Magic::Unknown && has_dmg_trailer(&mut file)? {
        bail!("`{name}` is an Apple disk image (.dmg); archive-contents does not read this format (not analysed)");
    }

    let mismatch = |want: Format| {
        anyhow!(
            "`{name}` is named as a {} but its content is {}; refusing to guess (not analysed)",
            want.label(),
            magic.label()
        )
    };
    match expect {
        Some(Expect::Format(want)) => {
            let agrees = match want {
                Format::Zip => magic == Magic::Zip,
                // A pre-POSIX tar has no `ustar` signature; the tar reader still
                // verifies every header checksum.
                Format::Tar(Compression::None) | Format::Gem => {
                    matches!(magic, Magic::Tar | Magic::Unknown)
                }
                Format::Tar(c) => magic == Magic::Compressed(c),
                Format::Deb => magic == Magic::Ar,
                Format::Rpm => magic == Magic::Rpm,
            };
            if agrees {
                Ok(want)
            } else {
                Err(mismatch(want))
            }
        }
        _ => match magic {
            Magic::Zip => Ok(Format::Zip),
            Magic::Compressed(c) => Ok(Format::Tar(c)),
            Magic::Tar => Ok(Format::Tar(Compression::None)),
            Magic::Ar => Ok(Format::Deb),
            Magic::Rpm => Ok(Format::Rpm),
            Magic::Unsupported(_) | Magic::Unknown => {
                bail!("`{name}` is not a recognised archive format (not analysed)")
            }
        },
    }
}

/// Calls `visit` for every entry of the archive at `path`, and returns its format.
pub fn walk(path: &Path, visit: &mut Visit<'_>) -> Result<Format> {
    let format = detect(path)?;
    let res = match format {
        Format::Zip => walk_zip(File::open(path)?, visit),
        Format::Tar(c) => {
            let decoded = decode(Box::new(File::open(path)?), c)?;
            walk_tar_stream(decoded, "", visit)
        }
        Format::Gem => walk_gem(File::open(path)?, visit),
        Format::Deb => walk_deb(path, visit),
        Format::Rpm => walk_rpm(path, visit),
    };
    res.with_context(|| format!("reading it as a {}", format.label()))?;
    Ok(format)
}

fn walk_zip(file: File, visit: &mut Visit<'_>) -> Result<()> {
    let mut zip = zip::ZipArchive::new(file)?;
    for i in 0..zip.len() {
        let (name, size) = {
            let raw = zip.by_index_raw(i)?;
            (raw.name().replace('\\', "/"), raw.size())
        };
        let is_dir = name.ends_with('/');
        match zip.by_index(i) {
            Ok(mut f) => visit(Entry {
                name,
                size,
                is_dir,
                data: EntryData::Bytes(&mut f),
            })?,
            Err(e) => visit(Entry {
                name,
                size,
                is_dir,
                data: EntryData::Undecodable(e.to_string()),
            })?,
        }
    }
    Ok(())
}

/// Walks a tar stream to its end, then drains the decoder so the compression
/// format's own trailer and checksum are verified.
fn walk_tar_stream(decoded: Decoded, prefix: &str, visit: &mut Visit<'_>) -> Result<()> {
    let mut archive = tar::Archive::new(decoded);
    walk_tar(&mut archive, prefix, visit)?;
    archive.into_inner().finish()
}

fn walk_tar<R: Read>(
    archive: &mut tar::Archive<R>,
    prefix: &str,
    visit: &mut Visit<'_>,
) -> Result<()> {
    for entry in archive.entries().context("not a tar stream")? {
        let mut entry = entry.context("corrupt tar entry")?;
        let kind = entry.header().entry_type();
        // A pax global header (`git archive` writes one) is metadata, not a file.
        if kind.is_pax_global_extensions() {
            continue;
        }
        let name = format!(
            "{prefix}{}",
            entry.path()?.to_string_lossy().replace('\\', "/")
        );
        let size = entry.size();
        visit(Entry {
            name,
            size,
            is_dir: kind.is_dir(),
            data: EntryData::Bytes(&mut entry),
        })?;
    }
    Ok(())
}

/// A `.gem` is an uncompressed tar of `metadata.gz`, `data.tar.gz` and
/// `checksums.yaml.gz`; the packaged files are inside `data.tar.gz`.
fn walk_gem(file: File, visit: &mut Visit<'_>) -> Result<()> {
    let mut outer = tar::Archive::new(file);
    let mut saw_data = false;
    for entry in outer.entries().context("not a tar stream")? {
        let mut entry = entry.context("corrupt tar entry")?;
        let kind = entry.header().entry_type();
        if kind.is_pax_global_extensions() {
            continue;
        }
        let name = entry.path()?.to_string_lossy().replace('\\', "/");
        let size = entry.size();
        if name.trim_start_matches("./") == "data.tar.gz" {
            saw_data = true;
            visit(Entry {
                name,
                size,
                is_dir: false,
                data: EntryData::Expanded,
            })?;
            let mut gz = flate2::read::MultiGzDecoder::new(&mut entry);
            {
                let mut inner = tar::Archive::new(&mut gz);
                walk_tar(&mut inner, "data/", visit).context("reading the gem's data.tar.gz")?;
            }
            io::copy(&mut gz, &mut io::sink()).context("reading the gem's data.tar.gz")?;
        } else {
            visit(Entry {
                name,
                size,
                is_dir: kind.is_dir(),
                data: EntryData::Bytes(&mut entry),
            })?;
        }
    }
    if !saw_data {
        bail!("the gem has no data.tar.gz member");
    }
    Ok(())
}

/// A `.deb` is an ar archive of `debian-binary`, `control.tar.*` and `data.tar.*`;
/// the installed files are the entries of the data member.
fn walk_deb(path: &Path, visit: &mut Visit<'_>) -> Result<()> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)?;
    if &magic != b"!<arch>\n" {
        bail!("not an ar archive");
    }
    let mut offset = 8u64;
    let mut data_member = None;
    while offset < len {
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0u8; 60];
        file.read_exact(&mut header)
            .context("truncated ar member header")?;
        if &header[58..60] != b"`\n" {
            bail!("corrupt ar member header at byte {offset}");
        }
        let mut name = String::from_utf8_lossy(&header[0..16])
            .trim_end()
            .trim_end_matches('/')
            .to_string();
        let stored: u64 = std::str::from_utf8(&header[48..58])
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| anyhow!("corrupt ar member size for `{name}`"))?;
        let next = offset + 60 + stored + (stored % 2);
        let mut body = offset + 60;
        let mut size = stored;
        if offset + 60 + stored > len {
            bail!("ar member `{name}` runs past the end of the file");
        }
        // BSD ar (macOS) stores a long name as `#1/<len>`, the name leading the body.
        if let Some(name_len) = name.strip_prefix("#1/").and_then(|n| n.parse::<u64>().ok()) {
            if name_len > stored || name_len > 4096 {
                bail!("corrupt BSD ar name length for member at byte {offset}");
            }
            let mut raw = vec![0u8; name_len as usize];
            file.read_exact(&mut raw)?;
            name = String::from_utf8_lossy(&raw)
                .trim_end_matches('\0')
                .to_string();
            body += name_len;
            size -= name_len;
        }
        if name.starts_with("data.tar") {
            if data_member.is_some() {
                bail!("the Debian package has more than one data.tar member");
            }
            data_member = Some((name, body, size));
        }
        offset = next;
    }
    let (name, body, size) =
        data_member.ok_or_else(|| anyhow!("the Debian package has no data.tar member"))?;
    let compression = member_compression(path, body, size, &name)?;
    let mut member = File::open(path)?;
    member.seek(SeekFrom::Start(body))?;
    let decoded = decode(Box::new(member.take(size)), compression)?;
    walk_tar_stream(decoded, "", visit).with_context(|| format!("reading member `{name}`"))
}

/// Compression of a member at `offset`, by its magic bytes.
fn member_compression(path: &Path, offset: u64, size: u64, what: &str) -> Result<Compression> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut head = Vec::with_capacity(512);
    file.take(size.min(512)).read_to_end(&mut head)?;
    match sniff(&head) {
        Magic::Compressed(c) => Ok(c),
        Magic::Tar | Magic::Unknown => Ok(Compression::None),
        other => bail!("`{what}` is {} (not a tar stream)", other.label()),
    }
}

/// Parses one RPM header structure's length: magic, reserved, index count, data size.
fn rpm_header_len(file: &mut File) -> Result<u64> {
    let mut intro = [0u8; 16];
    file.read_exact(&mut intro)
        .context("truncated RPM header")?;
    if intro[0..4] != [0x8e, 0xad, 0xe8, 0x01] {
        bail!("corrupt RPM header magic");
    }
    let index = u64::from(u32::from_be_bytes([
        intro[8], intro[9], intro[10], intro[11],
    ]));
    let data = u64::from(u32::from_be_bytes([
        intro[12], intro[13], intro[14], intro[15],
    ]));
    Ok(16 + 16 * index + data)
}

/// An `.rpm` is a 96-byte lead, a signature header padded to 8 bytes, the main
/// header, then a compressed cpio (newc) payload.
fn walk_rpm(path: &Path, visit: &mut Visit<'_>) -> Result<()> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut lead = [0u8; 96];
    file.read_exact(&mut lead).context("truncated RPM lead")?;
    if lead[0..4] != [0xed, 0xab, 0xee, 0xdb] {
        bail!("not an RPM package");
    }
    let signature_end = 96 + rpm_header_len(&mut file)?;
    let header_start = signature_end.div_ceil(8) * 8;
    if header_start > len {
        bail!("RPM signature header runs past the end of the file");
    }
    file.seek(SeekFrom::Start(header_start))?;
    let payload = header_start + rpm_header_len(&mut file)?;
    if payload > len {
        bail!("RPM header runs past the end of the file");
    }
    let mut head = [0u8; 6];
    file.seek(SeekFrom::Start(payload))?;
    let n = file.read(&mut head)?;
    let compression = match sniff(&head[..n]) {
        Magic::Compressed(c) => c,
        _ if head[..n].starts_with(b"07070") => Compression::None,
        _ if head[..n].starts_with(&[0x5d, 0x00, 0x00]) => {
            bail!("the RPM payload is a legacy raw LZMA stream, which archive-contents does not read (not analysed)")
        }
        _ => bail!("the RPM payload compression is not recognised (not analysed)"),
    };
    file.seek(SeekFrom::Start(payload))?;
    let mut decoded = decode(Box::new(file), compression)?;
    walk_cpio(&mut decoded, visit).context("reading the RPM cpio payload")?;
    decoded.finish()
}

/// Reads a cpio archive in the SVR4 `newc` / `crc` formats (magic `070701` / `070702`).
fn walk_cpio(reader: &mut dyn Read, visit: &mut Visit<'_>) -> Result<()> {
    loop {
        let mut header = [0u8; 110];
        reader
            .read_exact(&mut header)
            .context("truncated cpio header (no TRAILER!!! entry)")?;
        match &header[0..6] {
            b"070701" | b"070702" => {}
            b"07070X" => bail!("the cpio payload uses rpm's large-file format, which archive-contents does not read (not analysed)"),
            _ => bail!("unsupported cpio header (only the newc format is read)"),
        }
        let field = |i: usize| -> Result<u64> {
            let raw = &header[6 + 8 * i..14 + 8 * i];
            std::str::from_utf8(raw)
                .ok()
                .and_then(|s| u64::from_str_radix(s, 16).ok())
                .ok_or_else(|| anyhow!("corrupt cpio header field"))
        };
        let mode = field(1)?;
        let file_size = field(6)?;
        let name_size = field(11)?;
        if name_size == 0 || name_size > 65_536 {
            bail!("corrupt cpio name length {name_size}");
        }
        let mut raw_name = vec![0u8; name_size as usize];
        reader
            .read_exact(&mut raw_name)
            .context("truncated cpio name")?;
        let name = String::from_utf8_lossy(raw_name.strip_suffix(&[0]).unwrap_or(&raw_name))
            .replace('\\', "/");
        skip(reader, (4 - (110 + name_size) % 4) % 4)?;
        if name == "TRAILER!!!" {
            return Ok(());
        }
        let mut data = reader.take(file_size);
        visit(Entry {
            name: name.clone(),
            size: file_size,
            is_dir: mode & 0o170000 == 0o040000,
            data: EntryData::Bytes(&mut data),
        })?;
        io::copy(&mut data, &mut io::sink())?;
        if data.limit() != 0 {
            bail!("cpio entry `{name}` is truncated");
        }
        skip(reader, (4 - file_size % 4) % 4)?;
    }
}

fn skip(reader: &mut dyn Read, n: u64) -> Result<()> {
    let copied = io::copy(&mut reader.take(n), &mut io::sink())?;
    if copied != n {
        bail!("truncated archive");
    }
    Ok(())
}

/// A decompressing reader. `finish` drains it, so trailers and checksums are read,
/// and reports a decoder thread's error.
pub struct Decoded {
    reader: Box<dyn Read>,
    worker: Option<std::thread::JoinHandle<Result<()>>>,
}

impl Read for Decoded {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buf)
    }
}

impl Decoded {
    fn finish(mut self) -> Result<()> {
        let drained = io::copy(&mut self.reader, &mut io::sink());
        drop(self.reader);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| anyhow!("the xz decoder thread panicked"))??;
        }
        drained.context("corrupt or truncated compressed stream")?;
        Ok(())
    }
}

fn decode(source: Box<dyn Read + Send>, compression: Compression) -> Result<Decoded> {
    let reader: Box<dyn Read> = match compression {
        Compression::None => source,
        Compression::Gzip => Box::new(flate2::read::MultiGzDecoder::new(source)),
        Compression::Bzip2 => Box::new(bzip2_rs::DecoderReader::new(source)),
        Compression::Zstd => Box::new(ZstdFrames::new(source)),
        Compression::Xz => {
            // lzma-rs decodes xz into a writer; a thread and a pipe turn that into a
            // reader, so a large payload streams instead of being held in memory.
            let (pipe_reader, mut pipe_writer) = io::pipe()?;
            let worker = std::thread::spawn(move || -> Result<()> {
                let mut input = BufReader::new(source);
                match lzma_rs::xz_decompress(&mut input, &mut pipe_writer) {
                    Ok(()) => Ok(()),
                    // The reading side stopped early (it already has its answer or its
                    // own error); that is reported there.
                    Err(lzma_rs::error::Error::IoError(e))
                        if e.kind() == io::ErrorKind::BrokenPipe =>
                    {
                        Ok(())
                    }
                    Err(e) => Err(anyhow!("corrupt xz stream: {e}")),
                }
            });
            return Ok(Decoded {
                reader: Box::new(pipe_reader),
                worker: Some(worker),
            });
        }
    };
    Ok(Decoded {
        reader,
        worker: None,
    })
}

/// Largest zstd window accepted: 256 MiB, enough for `zstd --long` up to
/// `--long=28`. A larger window is refused (the decoder would allocate it) and the
/// archive is not analysed.
const ZSTD_MAX_WINDOW: u64 = 1 << 28;

type ZstdDecoder = ruzstd::decoding::StreamingDecoder<
    BufReader<Box<dyn Read + Send>>,
    ruzstd::decoding::FrameDecoder,
>;

/// Decodes every zstd frame in the stream, not only the first.
struct ZstdFrames {
    source: Option<BufReader<Box<dyn Read + Send>>>,
    frame: Option<ZstdDecoder>,
}

impl ZstdFrames {
    fn new(source: Box<dyn Read + Send>) -> Self {
        Self {
            source: Some(BufReader::new(source)),
            frame: None,
        }
    }
}

impl Read for ZstdFrames {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if let Some(frame) = self.frame.as_mut() {
                let n = frame.read(buf)?;
                if n > 0 || buf.is_empty() {
                    return Ok(n);
                }
                let frame = self.frame.take().expect("frame present");
                self.source = Some(frame.into_inner());
            }
            let Some(mut source) = self.source.take() else {
                return Err(io::Error::other("the zstd stream failed earlier"));
            };
            if source.fill_buf()?.is_empty() {
                self.source = Some(source);
                return Ok(0);
            }
            let frame = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(
                source,
                ZSTD_MAX_WINDOW,
            )
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("zstd: {e}")))?;
            self.frame = Some(frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures;
    use super::*;
    use tempfile::TempDir;

    /// `pkg-1.0/README` and `pkg-1.0/tools/leak.sh` as a bzip2-compressed ustar,
    /// written by Python's `tarfile` and `bz2` modules (the crate set has no bzip2
    /// encoder).
    const BZIP2_TAR_HEX: &str = "425a6839314159265359e630e2b2000098ff80c99000044003fd80260a100062ecde4008182000721a881a68d00340d346864f5049212609a3010cd100d1bfc70a81e4cc0034892115d7e8b4a0210e24e4a210c06bb3d77303c70f8d78c0b1251c088eaf10fea34fcba5b9b9c433262472346354869da70e78f1617f3bd80a2012f29852c20a092155a0c603f177245385090e630e2b20";

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn write(dir: &TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn names(path: &Path) -> Result<(Format, Vec<String>)> {
        let mut names = Vec::new();
        let format = walk(path, &mut |e| {
            names.push(e.name);
            Ok(())
        })?;
        Ok((format, names))
    }

    fn contents(path: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        walk(path, &mut |e| {
            let mut bytes = Vec::new();
            if let EntryData::Bytes(r) = e.data {
                r.read_to_end(&mut bytes)?;
            }
            out.push((e.name, bytes));
            Ok(())
        })
        .unwrap();
        out
    }

    const FILES: &[(&str, &[u8])] = &[
        ("pkg/README", b"hello\n"),
        ("pkg/tools/leak.sh", b"echo leak\n"),
    ];

    #[test]
    fn every_zip_family_extension_is_read() {
        let dir = TempDir::new().unwrap();
        let bytes = fixtures::zip(FILES);
        for ext in ZIP_EXTENSIONS {
            let path = write(&dir, &format!("package{ext}"), &bytes);
            let (format, names) = names(&path).unwrap_or_else(|e| panic!("{ext}: {e:#}"));
            assert_eq!(format, Format::Zip, "{ext}");
            assert_eq!(names, vec!["pkg/README", "pkg/tools/leak.sh"], "{ext}");
        }
    }

    #[test]
    fn every_tar_family_extension_is_read_with_its_compression() {
        let dir = TempDir::new().unwrap();
        let tar = fixtures::tar(FILES);
        let cases: Vec<(&str, Vec<u8>, Compression)> = vec![
            ("p.tar", tar.clone(), Compression::None),
            ("p.tar.gz", fixtures::gzip(&tar), Compression::Gzip),
            ("p.tgz", fixtures::gzip(&tar), Compression::Gzip),
            ("p-1.0.crate", fixtures::gzip(&tar), Compression::Gzip),
            ("p.tar.xz", fixtures::xz(&tar), Compression::Xz),
            ("p.txz", fixtures::xz(&tar), Compression::Xz),
            ("p.tar.zst", fixtures::zstd(&tar), Compression::Zstd),
            ("p.tzst", fixtures::zstd(&tar), Compression::Zstd),
            ("p.tar.zstd", fixtures::zstd(&tar), Compression::Zstd),
        ];
        for (name, bytes, compression) in cases {
            let path = write(&dir, name, &bytes);
            let (format, names) = names(&path).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert_eq!(format, Format::Tar(compression), "{name}");
            assert_eq!(names, vec!["pkg/README", "pkg/tools/leak.sh"], "{name}");
        }
        for name in ["p.tar.bz2", "p.tbz2", "p.tbz"] {
            let path = write(&dir, name, &unhex(BZIP2_TAR_HEX));
            let (format, names) = names(&path).unwrap();
            assert_eq!(format, Format::Tar(Compression::Bzip2));
            assert_eq!(names, vec!["pkg-1.0/README", "pkg-1.0/tools/leak.sh"]);
        }
    }

    #[test]
    fn entry_bytes_are_decoded_in_every_container() {
        let dir = TempDir::new().unwrap();
        let tar = fixtures::tar(FILES);
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("a.zip", fixtures::zip(FILES)),
            ("a.tar.xz", fixtures::xz(&tar)),
            ("a.tar.zst", fixtures::zstd(&tar)),
            ("a.deb", fixtures::deb("data.tar.gz", &fixtures::gzip(&tar))),
            (
                "a.rpm",
                fixtures::rpm(&fixtures::gzip(&fixtures::cpio_newc(FILES))),
            ),
        ];
        for (name, bytes) in cases {
            let got = contents(&write(&dir, name, &bytes));
            let script = got
                .iter()
                .find(|(n, _)| n.ends_with("tools/leak.sh"))
                .unwrap_or_else(|| panic!("{name}: {got:?}"));
            assert_eq!(script.1, b"echo leak\n", "{name}");
        }
    }

    #[test]
    fn a_gem_reports_its_members_and_the_data_tar_entries_under_data() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "example-1.0.gem", &fixtures::gem(FILES));
        let (format, names) = names(&path).unwrap();
        assert_eq!(format, Format::Gem);
        assert_eq!(
            names,
            vec![
                "metadata.gz",
                "data.tar.gz",
                "data/pkg/README",
                "data/pkg/tools/leak.sh",
                "checksums.yaml.gz"
            ]
        );
        let got = contents(&path);
        assert!(got.contains(&("data/pkg/README".to_string(), b"hello\n".to_vec())));
    }

    #[test]
    fn a_gem_without_data_tar_gz_fails() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "bad.gem", &fixtures::tar(&[("metadata.gz", b"x")]));
        let err = names(&path).unwrap_err();
        assert!(format!("{err:#}").contains("no data.tar.gz"), "{err:#}");
    }

    #[test]
    fn a_deb_reports_its_data_member_whatever_the_compression() {
        let dir = TempDir::new().unwrap();
        let tar = fixtures::tar(&[
            ("./usr/bin/tool", b"bin"),
            ("./usr/share/doc/tool/README", b"r"),
        ]);
        for (file, member, data) in [
            ("tool_1.0_all.deb", "data.tar", tar.clone()),
            ("tool_1.0_all.deb", "data.tar.gz", fixtures::gzip(&tar)),
            ("tool_1.0_all.deb", "data.tar.xz", fixtures::xz(&tar)),
            ("tool_1.0_all.deb", "data.tar.zst", fixtures::zstd(&tar)),
            ("tool-udeb_1.0_all.udeb", "data.tar.xz", fixtures::xz(&tar)),
        ] {
            let path = write(&dir, file, &fixtures::deb(member, &data));
            let (format, names) = names(&path).unwrap_or_else(|e| panic!("{member}: {e:#}"));
            assert_eq!(format, Format::Deb);
            let names: Vec<String> = names
                .iter()
                .map(|n| n.trim_start_matches("./").to_string())
                .collect();
            assert_eq!(
                names,
                vec!["usr/bin/tool", "usr/share/doc/tool/README"],
                "{member}"
            );
        }
    }

    #[test]
    fn a_deb_written_by_bsd_ar_with_long_names_is_read() {
        let dir = TempDir::new().unwrap();
        let data = fixtures::gzip(&fixtures::tar(&[("./usr/bin/tool", b"bin")]));
        let mut bytes = fixtures::ar(&[("debian-binary", b"2.0\n")]);
        // BSD ar: `#1/<len>` in the name field, the name leading the body.
        let long = b"data.tar.gz\0\0\0\0\0";
        let body_len = long.len() + data.len();
        let header = format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            format!("#1/{}", long.len()),
            0,
            0,
            0,
            "100644",
            body_len
        );
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(long);
        bytes.extend_from_slice(&data);
        if body_len % 2 == 1 {
            bytes.push(b'\n');
        }
        let path = write(&dir, "bsd.deb", &bytes);
        let (_, names) = names(&path).unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(names, vec!["usr/bin/tool"]);
    }

    #[test]
    fn a_deb_without_a_data_member_fails() {
        let dir = TempDir::new().unwrap();
        let path = write(
            &dir,
            "bad.deb",
            &fixtures::ar(&[("debian-binary", b"2.0\n")]),
        );
        let err = names(&path).unwrap_err();
        assert!(format!("{err:#}").contains("no data.tar member"), "{err:#}");
    }

    #[test]
    fn an_rpm_reports_its_cpio_payload_whatever_the_compression() {
        let dir = TempDir::new().unwrap();
        let cpio =
            fixtures::cpio_newc(&[("./usr/bin/tool", b"bin"), ("./etc/tool.conf", b"c=1\n")]);
        for payload in [
            fixtures::gzip(&cpio),
            fixtures::xz(&cpio),
            fixtures::zstd(&cpio),
        ] {
            let path = write(&dir, "tool-1.0-1.noarch.rpm", &fixtures::rpm(&payload));
            let (format, names) = names(&path).unwrap_or_else(|e| panic!("{e:#}"));
            assert_eq!(format, Format::Rpm);
            assert_eq!(names, vec!["./usr/bin/tool", "./etc/tool.conf"]);
        }
    }

    #[test]
    fn an_rpm_with_a_truncated_or_large_file_payload_fails() {
        let dir = TempDir::new().unwrap();
        let cpio = fixtures::cpio_newc(&[("./usr/bin/tool", b"bin")]);
        let truncated = &cpio[..cpio.len() - 120];
        let path = write(&dir, "t.rpm", &fixtures::rpm(&fixtures::gzip(truncated)));
        assert!(names(&path).is_err());

        let mut large = cpio.clone();
        large[..6].copy_from_slice(b"07070X");
        let path = write(&dir, "l.rpm", &fixtures::rpm(&fixtures::gzip(&large)));
        let err = names(&path).unwrap_err();
        assert!(format!("{err:#}").contains("large-file"), "{err:#}");
    }

    #[test]
    fn the_format_is_taken_from_the_magic_bytes_when_the_name_has_no_extension() {
        let dir = TempDir::new().unwrap();
        let tar = fixtures::tar(FILES);
        for (name, bytes, want) in [
            (
                "artifact",
                fixtures::gzip(&tar),
                Format::Tar(Compression::Gzip),
            ),
            ("artifact.bin", fixtures::zip(FILES), Format::Zip),
            ("image", tar.clone(), Format::Tar(Compression::None)),
            (
                "payload",
                fixtures::zstd(&tar),
                Format::Tar(Compression::Zstd),
            ),
            ("tool.pkg", fixtures::xz(&tar), Format::Tar(Compression::Xz)),
        ] {
            let path = write(&dir, name, &bytes);
            assert_eq!(detect(&path).unwrap(), want, "{name}");
            assert_eq!(names(&path).unwrap().1.len(), 2, "{name}");
        }
    }

    #[test]
    fn an_extension_that_disagrees_with_the_content_fails() {
        let dir = TempDir::new().unwrap();
        let tar = fixtures::tar(FILES);
        for (name, bytes, content) in [
            ("p.zip", fixtures::gzip(&tar), "gzip stream"),
            (
                "p.tar.gz",
                b"<html>404 Not Found</html>".to_vec(),
                "no recognised format",
            ),
            ("p.tar.xz", fixtures::gzip(&tar), "gzip stream"),
            ("p.whl", tar.clone(), "tar archive"),
            ("p.deb", fixtures::zip(FILES), "zip archive"),
        ] {
            let path = write(&dir, name, &bytes);
            let err = format!("{:#}", names(&path).unwrap_err());
            assert!(
                err.contains("refusing to guess") && err.contains(content),
                "{name}: {err}"
            );
        }
    }

    #[test]
    fn formats_out_of_scope_fail_by_name() {
        let dir = TempDir::new().unwrap();
        let mut dmg = vec![0u8; 2048];
        dmg[2048 - 512..2048 - 508].copy_from_slice(b"koly");
        let mut elf = b"\x7fELF".to_vec();
        elf.resize(64, 0);
        for (name, bytes, label) in [
            ("App.dmg", vec![0u8; 16], "Apple disk image"),
            ("disk-image", dmg, "Apple disk image"),
            ("setup.msi", vec![0u8; 16], "Windows Installer"),
            ("setup.exe", b"MZ\x90\x00".to_vec(), "Windows executable"),
            ("mytool", elf, "ELF binary"),
            ("Installer.pkg", b"xar!\x00\x1c".to_vec(), "xar archive"),
            ("mytool-macos", vec![0xcf, 0xfa, 0xed, 0xfe, 0, 0], "Mach-O"),
            (
                "notes.txt",
                b"just text".to_vec(),
                "not a recognised archive format",
            ),
        ] {
            let path = write(&dir, name, &bytes);
            let err = format!("{:#}", names(&path).unwrap_err());
            assert!(
                err.contains(label) && err.contains("not analysed"),
                "{name}: {err}"
            );
        }
        // Every refused extension, even when its bytes would read as a zip.
        for (ext, label) in REFUSED_EXTENSIONS {
            let path = write(&dir, &format!("x{ext}"), &fixtures::zip(FILES));
            let err = format!("{:#}", names(&path).unwrap_err());
            assert!(err.contains(label), "{ext}: {err}");
        }
        // Signatures of formats that are not read, with no extension to go by.
        for (bytes, label) in [
            (b"MZ\x90\x00\x03".to_vec(), "PE executable"),
            (b"hsqs\x00\x00".to_vec(), "squashfs"),
            (b"7z\xbc\xaf\x27\x1c\x00".to_vec(), "7-Zip"),
            (b"Rar!\x1a\x07\x00".to_vec(), "RAR"),
            (b"LZIP\x01".to_vec(), "lzip"),
            (b"\xca\xfe\xba\xbe\x00".to_vec(), "universal binary"),
            (
                vec![0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1, 0],
                "OLE compound file",
            ),
        ] {
            let path = write(&dir, "artifact", &bytes);
            let err = format!("{:#}", names(&path).unwrap_err());
            assert!(err.contains(label) && err.contains("not analysed"), "{err}");
        }
    }

    #[test]
    fn a_corrupt_compressed_stream_fails_even_after_the_tar_end() {
        let dir = TempDir::new().unwrap();
        let tar = fixtures::tar(FILES);
        let mut gz = fixtures::gzip(&tar);
        let n = gz.len();
        gz[n - 8] ^= 0xff; // CRC32 in the gzip trailer
        let path = write(&dir, "p.tar.gz", &gz);
        assert!(names(&path).is_err());

        let xz = fixtures::xz(&tar);
        let path = write(&dir, "p.tar.xz", &xz[..xz.len() - 12]);
        assert!(names(&path).is_err());
    }

    #[test]
    fn every_zstd_frame_is_read() {
        let dir = TempDir::new().unwrap();
        let tar = fixtures::tar(FILES);
        let (first, second) = tar.split_at(512 * 2);
        let mut bytes = fixtures::zstd(first);
        bytes.extend(fixtures::zstd(second));
        let path = write(&dir, "p.tar.zst", &bytes);
        assert_eq!(names(&path).unwrap().1.len(), 2);
    }

    #[test]
    fn a_pax_global_header_is_not_an_entry() {
        let dir = TempDir::new().unwrap();
        let mut builder = tar::Builder::new(Vec::new());
        let mut global = tar::Header::new_ustar();
        global.set_entry_type(tar::EntryType::XGlobalHeader);
        let comment = b"52 comment=0123456789abcdef0123456789abcdef01234567\n";
        global.set_size(comment.len() as u64);
        global.set_cksum();
        builder
            .append_data(&mut global, "pax_global_header", &comment[..])
            .unwrap();
        let mut header = tar::Header::new_ustar();
        header.set_size(1);
        header.set_cksum();
        builder
            .append_data(&mut header, "pkg/a", &b"a"[..])
            .unwrap();
        let path = write(&dir, "p.tar", &builder.into_inner().unwrap());
        assert_eq!(names(&path).unwrap().1, vec!["pkg/a"]);
    }
}
