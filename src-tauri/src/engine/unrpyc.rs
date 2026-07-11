//! Bundled [unrpyc](https://github.com/CensoredUsername/unrpyc) — the Ren'Py
//! script decompiler (MIT-licensed; vendored under `resources/unrpyc/`, see the
//! README there for pinned versions and attribution).
//!
//! A game that ships only compiled `.rpyc` (no source `.rpy`) can't be translated
//! directly, but it *does* ship its own Python interpreter (`<game>/lib/py{3,2}-*`)
//! and Ren'Py runtime. [`renpy::ensure_decompiled`](super::renpy) drives that
//! interpreter over this bundled unrpyc to recover the `.rpy` source at import.
//!
//! The decompiler split when Ren'Py moved to Python 3 in Ren'Py 8, so both
//! branches are vendored and picked by the interpreter's major version:
//! [`PyMajor::Py3`] → `v2` (master), [`PyMajor::Py2`] → `v1` (legacy).
//!
//! The tool is a multi-file Python tree, so it's embedded with `include_dir!` and
//! materialized to a temp cache on first use — mirroring the single-file
//! [`TARGET_FONT`](super::TARGET_FONT) `include_bytes!` precedent, but for a dir.

use anyhow::Result;
use include_dir::{include_dir, Dir};
use std::path::PathBuf;

/// The major Python generation a Ren'Py game ships, which selects the unrpyc
/// branch. Discovered from the `lib/py{3,2}-*` directory the game bundles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PyMajor {
    /// Ren'Py 8 — bundled CPython 3.x under `lib/py3-*`; uses unrpyc `v2` (master).
    Py3,
    /// Ren'Py 7/6 — bundled CPython 2.7 under `lib/py2-*`; uses unrpyc `v1` (legacy).
    Py2,
}

static UNRPYC_V2: Dir = include_dir!("$CARGO_MANIFEST_DIR/resources/unrpyc/v2");
static UNRPYC_V1: Dir = include_dir!("$CARGO_MANIFEST_DIR/resources/unrpyc/v1");

/// Materialize the unrpyc branch for `major` into a version-stamped temp cache and
/// return the path to its `unrpyc.py` (which the caller runs as
/// `<python> <unrpyc.py> -c <game-dir>`).
///
/// The copy is reused across imports and every game, and lives *outside* any game
/// dir, so it can never dirty a game or a committed test fixture. Stamping the path
/// with the app version means it self-invalidates on upgrade (a newer bundled
/// unrpyc lands in a fresh dir instead of colliding with a stale one).
pub fn materialize(major: PyMajor) -> Result<PathBuf> {
    let (branch, src) = match major {
        PyMajor::Py3 => ("v2", &UNRPYC_V2),
        PyMajor::Py2 => ("v1", &UNRPYC_V1),
    };
    let dir = std::env::temp_dir()
        .join("rpgtl-unrpyc")
        .join(env!("CARGO_PKG_VERSION"))
        .join(branch);
    let entry = dir.join("unrpyc.py");
    // Materialize once: reuse a prior extraction (the entry file is the sentinel).
    if !entry.exists() {
        src.extract(&dir)?;
    }
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materialize_writes_the_cli_and_its_package() {
        // Extraction needs no Python: it only unpacks the embedded tree to disk.
        let entry = materialize(PyMajor::Py3).expect("materialize v2");
        assert!(entry.is_file(), "unrpyc.py should exist at {entry:?}");
        assert_eq!(entry.file_name().unwrap(), "unrpyc.py");
        let root = entry.parent().unwrap();
        // The decompiler package and deobfuscate helper must sit next to unrpyc.py
        // (unrpyc.py does `import decompiler` / `import deobfuscate`).
        assert!(root.join("decompiler").join("__init__.py").is_file());
        assert!(root.join("deobfuscate.py").is_file());
        // Idempotent: a second call reuses the same path without error.
        let again = materialize(PyMajor::Py3).expect("re-materialize v2");
        assert_eq!(entry, again);
    }

    #[test]
    fn both_branches_materialize() {
        let v1 = materialize(PyMajor::Py2).expect("materialize v1");
        assert!(v1.is_file());
        assert!(v1.parent().unwrap().join("decompiler").join("__init__.py").is_file());
    }
}
