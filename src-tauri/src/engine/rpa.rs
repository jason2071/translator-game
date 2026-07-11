//! Minimal reader for Ren'Py `.rpa` archives, enough to recover the source
//! `.rpy` a game ships packed.
//!
//! A `.rpa` is a flat blob whose **first line** is a header pointing at a
//! zlib-compressed [pickle] index near the end of the file:
//!   - `RPA-3.0 <hex index offset> <hex key>\n` — offsets/lengths in the index
//!     are XORed with the key.
//!   - `RPA-2.0 <hex index offset>\n` — no key.
//!
//! The index is a Python dict `{ path: [ (offset, length, prefix), … ] }`. Each
//! file's bytes are `prefix` followed by `length - prefix.len()` bytes read at
//! `offset` (a file is almost always a single segment). We only ever pull the
//! small `.rpy` sources out — never the multi-hundred-MB asset blobs — so we
//! read just the index plus each script's own span, not the whole archive.
//!
//! [pickle]: https://docs.python.org/3/library/pickle.html

use anyhow::{anyhow, bail, Context, Result};
use flate2::read::ZlibDecoder;
use serde_pickle::{HashableValue, Value};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// One contiguous span of a file inside the archive.
#[derive(Debug, Clone)]
pub struct Segment {
    /// Byte offset of the span within the archive (already de-obfuscated).
    pub offset: u64,
    /// Total logical length of the span, counting the in-index `prefix`.
    pub length: u64,
    /// Bytes stored inline in the index and prepended to the file data (usually
    /// empty).
    pub prefix: Vec<u8>,
}

/// The archive's file table: archive-relative path → its segments.
pub type Index = BTreeMap<String, Vec<Segment>>;

/// Every `.rpy` path in `archive` (read-only — index only, no file bytes).
pub fn list_rpy(archive: &Path) -> Result<Vec<String>> {
    Ok(read_index(archive)?
        .into_keys()
        .filter(|name| is_rpy_name(name))
        .collect())
}

/// Extract every `.rpy` in `archive` into `out_dir`, re-creating the archive's
/// internal directory structure. Existing files are left untouched (so a partial
/// prior run or a hand-edited script is never clobbered). Returns how many files
/// were written.
pub fn extract_rpy(archive: &Path, out_dir: &Path) -> Result<usize> {
    extract_matching(archive, out_dir, is_rpy_name)
}

/// Like [`extract_rpy`] but for compiled `.rpyc` — used to stage the bytecode of a
/// compiled-only game on disk before handing it to unrpyc for decompilation (see
/// [`ensure_decompiled`](super::renpy)). Same no-clobber / safe-path contract.
pub fn extract_rpyc(archive: &Path, out_dir: &Path) -> Result<usize> {
    extract_matching(archive, out_dir, is_rpyc_name)
}

/// Extract every archive entry whose name satisfies `keep` into `out_dir`,
/// re-creating the internal directory structure and never clobbering an existing
/// file. Backs [`extract_rpy`] / [`extract_rpyc`]. Returns how many were written.
fn extract_matching(
    archive: &Path,
    out_dir: &Path,
    keep: impl Fn(&str) -> bool,
) -> Result<usize> {
    let index = read_index(archive)?;
    let mut f = File::open(archive).with_context(|| format!("opening {}", archive.display()))?;
    let mut written = 0usize;
    for (name, segments) in &index {
        if !keep(name) {
            continue;
        }
        let rel = safe_relative(name)
            .ok_or_else(|| anyhow!("unsafe path in archive: {name}"))?;
        let dest = out_dir.join(&rel);
        if dest.exists() {
            continue;
        }
        let data = read_segments(&mut f, segments)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, data).with_context(|| format!("writing {}", dest.display()))?;
        written += 1;
    }
    Ok(written)
}

fn is_rpy_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".rpy")
}

fn is_rpyc_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".rpyc")
}

/// Read + de-obfuscate the archive index.
pub fn read_index(archive: &Path) -> Result<Index> {
    let mut f = File::open(archive).with_context(|| format!("opening {}", archive.display()))?;

    // The header is the first line; it's short, so a small peek covers it.
    let mut head = [0u8; 128];
    let n = f.read(&mut head)?;
    let nl = head[..n]
        .iter()
        .position(|&b| b == b'\n')
        .ok_or_else(|| anyhow!("{}: no archive header", archive.display()))?;
    let header = std::str::from_utf8(&head[..nl])
        .map_err(|_| anyhow!("{}: non-UTF-8 archive header", archive.display()))?
        .trim();
    let parts: Vec<&str> = header.split_whitespace().collect();

    let (index_offset, key) = match parts.first().copied() {
        Some("RPA-3.0") => {
            let off = parse_hex(parts.get(1), "RPA-3.0 index offset")?;
            let key = parse_hex(parts.get(2), "RPA-3.0 key")?;
            (off, key)
        }
        Some("RPA-2.0") => (parse_hex(parts.get(1), "RPA-2.0 index offset")?, 0),
        other => bail!(
            "{}: unsupported archive format {:?} (only RPA-2.0 / RPA-3.0)",
            archive.display(),
            other.unwrap_or("")
        ),
    };

    f.seek(SeekFrom::Start(index_offset))?;
    let mut compressed = Vec::new();
    f.read_to_end(&mut compressed)?;
    let mut raw = Vec::new();
    ZlibDecoder::new(&compressed[..])
        .read_to_end(&mut raw)
        .with_context(|| format!("{}: inflating archive index", archive.display()))?;

    let value = serde_pickle::value_from_slice(&raw, serde_pickle::DeOptions::new())
        .with_context(|| format!("{}: unpickling archive index", archive.display()))?;
    parse_index(value, key)
}

fn parse_hex(tok: Option<&&str>, what: &str) -> Result<u64> {
    let s = tok.ok_or_else(|| anyhow!("missing {what}"))?;
    u64::from_str_radix(s, 16).map_err(|_| anyhow!("bad {what}: {s:?}"))
}

fn parse_index(value: Value, key: u64) -> Result<Index> {
    let Value::Dict(dict) = value else {
        bail!("archive index is not a dict");
    };
    let mut out = Index::new();
    for (k, v) in dict {
        let Some(name) = hashable_to_string(&k) else {
            continue;
        };
        let Value::List(entries) = v else { continue };
        let mut segments = Vec::with_capacity(entries.len());
        for entry in entries {
            let Value::Tuple(fields) = entry else { continue };
            if fields.len() < 2 {
                continue;
            }
            let offset = as_u64(&fields[0])? ^ key;
            let length = as_u64(&fields[1])? ^ key;
            let prefix = match fields.get(2) {
                Some(Value::Bytes(b)) => b.clone(),
                Some(Value::String(s)) => s.clone().into_bytes(),
                _ => Vec::new(),
            };
            segments.push(Segment {
                offset,
                length,
                prefix,
            });
        }
        out.insert(name, segments);
    }
    Ok(out)
}

fn hashable_to_string(h: &HashableValue) -> Option<String> {
    match h {
        HashableValue::String(s) => Some(s.clone()),
        HashableValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }
}

/// A non-negative pickled integer as `u64`. Ren'Py stores offsets either as
/// machine ints (`Value::I64`) or, once XOR-obfuscation sets the high bits, as
/// Python big ints (`Value::Int`); handle both.
fn as_u64(v: &Value) -> Result<u64> {
    match v {
        Value::I64(i) => u64::try_from(*i).map_err(|_| anyhow!("negative offset {i}")),
        // BigInt has no version-stable primitive cast in scope, but its decimal
        // Display always round-trips through parse for a non-negative value.
        Value::Int(b) => b
            .to_string()
            .parse::<u64>()
            .map_err(|_| anyhow!("offset out of range: {b}")),
        other => bail!("expected an integer offset, got {other:?}"),
    }
}

fn read_segments(f: &mut File, segments: &[Segment]) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    for s in segments {
        data.extend_from_slice(&s.prefix);
        let to_read = (s.length as usize).saturating_sub(s.prefix.len());
        f.seek(SeekFrom::Start(s.offset))?;
        let mut buf = vec![0u8; to_read];
        f.read_exact(&mut buf)
            .with_context(|| format!("reading {to_read} bytes at {}", s.offset))?;
        data.extend_from_slice(&buf);
    }
    Ok(data)
}

/// Turn an archive-internal path into a safe relative path: forward-or-back
/// slashes are split, `.`/empty components dropped, and anything that could
/// escape `out_dir` (a `..` hop, an absolute path, a Windows drive/`:`) rejected.
fn safe_relative(name: &str) -> Option<PathBuf> {
    let mut rel = PathBuf::new();
    for comp in name.split(['/', '\\']) {
        match comp {
            "" | "." => continue,
            ".." => return None,
            c if c.contains(':') => return None,
            c => rel.push(c),
        }
    }
    if rel.as_os_str().is_empty() {
        None
    } else {
        Some(rel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;

    /// Build a real RPA-3.0 archive in memory: `RPA-3.0 <off> <key>\n`, then each
    /// file's bytes, then the zlib-compressed pickled index. The header is a fixed
    /// 36 bytes (`RPA-3.0 ` + 16 hex + ` ` + 8 hex + `\n`), so file data starts at
    /// a known offset.
    fn build_rpa(key: u64, files: &[(&str, &[u8])]) -> Vec<u8> {
        // The header is fixed-width, so file data starts at a known offset.
        let header_len = format!("RPA-3.0 {:016x} {:08x}\n", 0u64, key).len();
        let mut body = Vec::new();
        let mut index = BTreeMap::new();
        for (name, bytes) in files {
            let offset = (header_len + body.len()) as u64;
            body.extend_from_slice(bytes);
            let seg = Value::Tuple(vec![
                Value::I64((offset ^ key) as i64),
                Value::I64((bytes.len() as u64 ^ key) as i64),
                Value::Bytes(Vec::new()),
            ]);
            index.insert(
                HashableValue::String((*name).to_string()),
                Value::List(vec![seg]),
            );
        }
        let index_offset = (header_len + body.len()) as u64;

        let pickled =
            serde_pickle::value_to_vec(&Value::Dict(index), serde_pickle::SerOptions::new())
                .unwrap();
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&pickled).unwrap();
        let compressed = enc.finish().unwrap();

        let mut archive = format!("RPA-3.0 {index_offset:016x} {key:08x}\n").into_bytes();
        assert_eq!(archive.len(), header_len);
        archive.extend_from_slice(&body);
        archive.extend_from_slice(&compressed);
        archive
    }

    #[test]
    fn extracts_rpy_and_skips_other_files() {
        let script = b"label start:\n    e \"Hi\"\n";
        let png = b"\x89PNG not really";
        let archive = build_rpa(
            0x42424242,
            &[("script.rpy", script), ("images/logo.png", png)],
        );

        let tmp = tempfile::tempdir().unwrap();
        let rpa = tmp.path().join("archive.rpa");
        std::fs::write(&rpa, &archive).unwrap();

        // Read-only listing sees only the .rpy.
        assert_eq!(list_rpy(&rpa).unwrap(), vec!["script.rpy".to_string()]);

        let out = tmp.path().join("game");
        let n = extract_rpy(&rpa, &out).unwrap();
        assert_eq!(n, 1);
        assert_eq!(std::fs::read(out.join("script.rpy")).unwrap(), script);
        // The asset was not extracted.
        assert!(!out.join("images/logo.png").exists());
    }

    #[test]
    fn extract_rpyc_takes_only_bytecode_and_preserves_rpy_contract() {
        let rpy = b"label start:\n    e \"Hi\"\n";
        let rpyc = b"RENPY RPC2 fake-bytecode";
        let png = b"\x89PNG";
        let archive = build_rpa(
            0x1234_5678,
            &[
                ("script.rpy", rpy),
                ("script.rpyc", rpyc),
                ("images/logo.png", png),
            ],
        );
        let tmp = tempfile::tempdir().unwrap();
        let rpa = tmp.path().join("archive.rpa");
        std::fs::write(&rpa, &archive).unwrap();

        // The new extractor takes only the compiled `.rpyc` (note `.rpyc` does not
        // satisfy the `.rpy` predicate, so there's no overlap).
        let out = tmp.path().join("rpyc");
        assert_eq!(extract_rpyc(&rpa, &out).unwrap(), 1);
        assert_eq!(std::fs::read(out.join("script.rpyc")).unwrap(), rpyc);
        assert!(!out.join("script.rpy").exists());
        assert!(!out.join("images/logo.png").exists());

        // The `.rpy`-only contract is untouched: extract_rpy skips the `.rpyc`, and
        // list_rpy still lists only the source.
        let src = tmp.path().join("src");
        assert_eq!(extract_rpy(&rpa, &src).unwrap(), 1);
        assert!(src.join("script.rpy").exists());
        assert!(!src.join("script.rpyc").exists());
        assert_eq!(list_rpy(&rpa).unwrap(), vec!["script.rpy".to_string()]);
    }

    #[test]
    fn nested_paths_and_reextract_is_idempotent() {
        let a = b"label a:\n    \"one\"\n";
        let b = b"label b:\n    \"two\"\n";
        let archive = build_rpa(0xdead_beef, &[("a.rpy", a), ("sub/dir/b.rpy", b)]);

        let tmp = tempfile::tempdir().unwrap();
        let rpa = tmp.path().join("archive.rpa");
        std::fs::write(&rpa, &archive).unwrap();
        let out = tmp.path().join("game");

        assert_eq!(extract_rpy(&rpa, &out).unwrap(), 2);
        assert_eq!(std::fs::read(out.join("sub/dir/b.rpy")).unwrap(), b);

        // Second run writes nothing (files already present) and doesn't corrupt them.
        assert_eq!(extract_rpy(&rpa, &out).unwrap(), 0);
        assert_eq!(std::fs::read(out.join("a.rpy")).unwrap(), a);
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(safe_relative("../evil.rpy").is_none());
        assert!(safe_relative("a/../../evil.rpy").is_none());
        assert!(safe_relative("/etc/passwd").is_some()); // leading slash → relative
        assert_eq!(safe_relative("/etc/passwd").unwrap(), PathBuf::from("etc/passwd"));
        assert!(safe_relative("C:\\windows\\x.rpy").is_none()); // drive letter
        assert_eq!(safe_relative("a/b/c.rpy").unwrap(), PathBuf::from("a/b/c.rpy"));
    }
}
