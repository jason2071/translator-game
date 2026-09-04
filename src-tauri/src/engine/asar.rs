//! Minimal Electron ASAR reader/writer for packed game resources.
//!
//! ASAR stores a Chromium-pickle JSON file table followed by the packed file
//! bytes.  It is not compressed, so a rewrite can stream unchanged entries
//! directly from the original archive without expanding its assets to disk.

use anyhow::{anyhow, bail, Context, Result};
#[cfg(test)]
use serde_json::Map;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    offset: u64,
    size: u64,
    unpacked: bool,
}

/// Parsed ASAR metadata. Entries retain archive-relative paths with `/`
/// separators, regardless of the host platform.
#[derive(Debug)]
pub struct Archive {
    path: PathBuf,
    header: Value,
    data_offset: u64,
    entries: Vec<Entry>,
}

impl Archive {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file =
            File::open(path).with_context(|| format!("opening ASAR {}", path.display()))?;
        let mut prefix = [0u8; 16];
        file.read_exact(&mut prefix)
            .with_context(|| format!("reading ASAR header {}", path.display()))?;

        // Electron serializes the header size in a tiny pickle containing one
        // u32, followed by a pickle containing the JSON string.
        if u32::from_le_bytes(prefix[0..4].try_into().unwrap()) != 4 {
            bail!("{}: unsupported ASAR size pickle", path.display());
        }
        let header_size = u32::from_le_bytes(prefix[4..8].try_into().unwrap()) as u64;
        let json_size = u32::from_le_bytes(prefix[12..16].try_into().unwrap()) as u64;
        let data_offset = 8 + header_size;
        if header_size < 8 || 16 + json_size > data_offset {
            bail!("{}: invalid ASAR header lengths", path.display());
        }

        let mut json = vec![0u8; json_size as usize];
        file.read_exact(&mut json)
            .with_context(|| format!("reading ASAR index {}", path.display()))?;
        let header: Value = serde_json::from_slice(&json)
            .with_context(|| format!("parsing ASAR index {}", path.display()))?;
        let mut entries = Vec::new();
        collect_entries(&header, "", &mut entries)?;
        let archive_len = file.metadata()?.len();
        for entry in &entries {
            if entry.unpacked {
                continue;
            }
            let end = data_offset
                .checked_add(entry.offset)
                .and_then(|start| start.checked_add(entry.size))
                .ok_or_else(|| anyhow!("ASAR entry size overflow: {}", entry.path))?;
            if end > archive_len {
                bail!("ASAR entry extends beyond archive: {}", entry.path);
            }
        }
        entries.sort_by_key(|entry| entry.offset);

        Ok(Self {
            path: path.to_path_buf(),
            header,
            data_offset,
            entries,
        })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn contains(&self, path: &str) -> bool {
        self.entries.iter().any(|entry| entry.path == path)
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .ok_or_else(|| anyhow!("ASAR entry not found: {path}"))?;
        if entry.unpacked {
            bail!("ASAR entry is unpacked and cannot be read from the archive: {path}");
        }
        let mut file = File::open(&self.path)
            .with_context(|| format!("opening ASAR {}", self.path.display()))?;
        file.seek(SeekFrom::Start(self.data_offset + entry.offset))?;
        let mut bytes = vec![0u8; entry.size as usize];
        file.read_exact(&mut bytes)
            .with_context(|| format!("reading ASAR entry {path}"))?;
        Ok(bytes)
    }

    /// Rebuild the archive at `out`, replacing only named packed entries.
    /// Unchanged payloads are copied in streaming chunks; they are never held
    /// in memory or unpacked as loose files.
    pub fn rebuild(&self, out: &Path, replacements: &HashMap<String, Vec<u8>>) -> Result<()> {
        let mut by_path: BTreeMap<&str, &Entry> = BTreeMap::new();
        for entry in &self.entries {
            by_path.insert(&entry.path, entry);
        }
        for path in replacements.keys() {
            let entry = by_path
                .get(path.as_str())
                .ok_or_else(|| anyhow!("ASAR replacement is not an archive entry: {path}"))?;
            if entry.unpacked {
                bail!("cannot replace unpacked ASAR entry: {path}");
            }
        }

        let mut layout = BTreeMap::new();
        let mut next_offset = 0u64;
        for entry in &self.entries {
            if entry.unpacked {
                continue;
            }
            let size = replacements
                .get(&entry.path)
                .map(|bytes| bytes.len() as u64)
                .unwrap_or(entry.size);
            layout.insert(entry.path.clone(), (next_offset, size));
            next_offset = next_offset
                .checked_add(size)
                .ok_or_else(|| anyhow!("ASAR payload size overflow"))?;
        }

        let mut header = self.header.clone();
        apply_layout(&mut header, "", &layout)?;
        let header_bytes = encode_header(&header)?;
        let out_file = File::create(out).with_context(|| format!("creating {}", out.display()))?;
        let mut writer = BufWriter::new(out_file);
        writer.write_all(&header_bytes)?;

        let mut source = File::open(&self.path)
            .with_context(|| format!("opening ASAR {}", self.path.display()))?;
        for entry in &self.entries {
            if entry.unpacked {
                continue;
            }
            if let Some(bytes) = replacements.get(&entry.path) {
                writer.write_all(bytes)?;
            } else {
                source.seek(SeekFrom::Start(self.data_offset + entry.offset))?;
                let mut limited = (&mut source).take(entry.size);
                let copied = std::io::copy(&mut limited, &mut writer)
                    .with_context(|| format!("copying ASAR entry {}", entry.path))?;
                if copied != entry.size {
                    bail!("truncated ASAR entry while copying: {}", entry.path);
                }
            }
        }
        writer.flush()?;
        Ok(())
    }
}

impl Entry {
    pub fn is_unpacked(&self) -> bool {
        self.unpacked
    }
}

fn collect_entries(value: &Value, prefix: &str, out: &mut Vec<Entry>) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("ASAR index node is not an object"))?;
    if let Some(files) = object.get("files") {
        let files = files
            .as_object()
            .ok_or_else(|| anyhow!("ASAR files node is not an object"))?;
        for (name, child) in files {
            if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
                bail!("unsafe ASAR entry name: {name}");
            }
            let path = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}/{name}")
            };
            collect_entries(child, &path, out)?;
        }
        return Ok(());
    }

    // ASAR symlinks have a `link` target but no payload, so there is no byte
    // span to extract or rewrite. They remain intact in the copied JSON index.
    if object.contains_key("link") {
        return Ok(());
    }

    let unpacked = object
        .get("unpacked")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let size = object
        .get("size")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("ASAR entry {prefix} has no numeric size"))?;
    let offset = if unpacked {
        0
    } else {
        object
            .get("offset")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("ASAR entry {prefix} has no offset"))?
            .parse()
            .with_context(|| format!("invalid ASAR offset for {prefix}"))?
    };
    out.push(Entry {
        path: prefix.to_string(),
        offset,
        size,
        unpacked,
    });
    Ok(())
}

fn apply_layout(
    value: &mut Value,
    prefix: &str,
    layout: &BTreeMap<String, (u64, u64)>,
) -> Result<()> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("ASAR index node is not an object"))?;
    if let Some(files) = object.get_mut("files") {
        let files = files
            .as_object_mut()
            .ok_or_else(|| anyhow!("ASAR files node is not an object"))?;
        for (name, child) in files {
            let path = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}/{name}")
            };
            apply_layout(child, &path, layout)?;
        }
        return Ok(());
    }
    if let Some((offset, size)) = layout.get(prefix) {
        object.insert("offset".to_string(), Value::String(offset.to_string()));
        object.insert("size".to_string(), Value::from(*size));
    }
    Ok(())
}

fn encode_header(header: &Value) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(header)?;
    let payload_len = 4 + json.len();
    let padding = (4 - payload_len % 4) % 4;
    let pickle_payload_len = payload_len + padding;
    let header_size = 4usize
        .checked_add(pickle_payload_len)
        .ok_or_else(|| anyhow!("ASAR header is too large"))?;
    let header_size =
        u32::try_from(header_size).map_err(|_| anyhow!("ASAR header is too large"))?;
    let json_len =
        u32::try_from(json.len()).map_err(|_| anyhow!("ASAR header JSON is too large"))?;
    let pickle_payload_len =
        u32::try_from(pickle_payload_len).map_err(|_| anyhow!("ASAR header is too large"))?;

    let mut out = Vec::with_capacity(8 + header_size as usize);
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(&header_size.to_le_bytes());
    out.extend_from_slice(&pickle_payload_len.to_le_bytes());
    out.extend_from_slice(&json_len.to_le_bytes());
    out.extend_from_slice(&json);
    out.resize(out.len() + padding, 0);
    Ok(out)
}

#[cfg(test)]
pub(crate) fn write_test_archive(path: &Path, entries: &[(&str, &[u8])]) {
    let mut root = Map::new();
    let mut files = Map::new();
    let mut offset = 0u64;
    for (path, bytes) in entries {
        let mut parts = path.split('/').peekable();
        let mut current = &mut files;
        while let Some(part) = parts.next() {
            if parts.peek().is_some() {
                let value = current.entry((*part).to_string()).or_insert_with(|| {
                    Value::Object(Map::from_iter([(
                        "files".to_string(),
                        Value::Object(Map::new()),
                    )]))
                });
                current = value
                    .get_mut("files")
                    .and_then(Value::as_object_mut)
                    .unwrap();
            } else {
                current.insert(
                    (*part).to_string(),
                    Value::Object(Map::from_iter([
                        ("size".to_string(), Value::from(bytes.len() as u64)),
                        ("offset".to_string(), Value::String(offset.to_string())),
                    ])),
                );
                offset += bytes.len() as u64;
            }
        }
    }
    root.insert("files".to_string(), Value::Object(files));
    let mut bytes = encode_header(&Value::Object(root)).unwrap();
    for (_, entry) in entries {
        bytes.extend_from_slice(entry);
    }
    std::fs::write(path, bytes).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_test_asar(path: &Path, entries: &[(&str, &[u8])]) {
        write_test_archive(path, entries);
    }

    #[test]
    fn reads_and_rebuilds_only_replaced_entries() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("app.asar");
        let out = dir.path().join("out.asar");
        write_test_asar(
            &src,
            &[
                ("data/scenario/start.ks", b"Hello"),
                ("data/image/a.bin", b"asset"),
            ],
        );
        let archive = Archive::open(&src).unwrap();
        assert_eq!(archive.read("data/scenario/start.ks").unwrap(), b"Hello");

        archive
            .rebuild(
                &out,
                &HashMap::from([(
                    "data/scenario/start.ks".to_string(),
                    "สวัสดี".as_bytes().to_vec(),
                )]),
            )
            .unwrap();
        let rebuilt = Archive::open(&out).unwrap();
        assert_eq!(
            rebuilt.read("data/scenario/start.ks").unwrap(),
            "สวัสดี".as_bytes()
        );
        assert_eq!(rebuilt.read("data/image/a.bin").unwrap(), b"asset");
    }
}
