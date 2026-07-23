//! Ren'Py engine (text `.rpy` scripts).
//!
//! Ren'Py dialogue lives in `game/**/*.rpy` as line-based statements:
//!   - say:   `e "Hello."`  /  `"Narration."`  /  `e happy "Hi" with vpunch`
//!   - menu:  a `menu:` block whose choices are `"Choice text":`
//!
//! Unlike the JSON engines we do not re-serialize the file. Each translatable
//! string is located by the byte span of its *inner* content (between the
//! quotes), and [`inject`] splices the translation into exactly that span. So if
//! the translation equals the source, the file comes back byte-identical —
//! round-trip identity holds for free. The `source` we store is the raw literal
//! (escapes, `[interpolation]` and `{text tags}` preserved), so a translator/AI
//! must keep those intact just like control codes.
//!
//! Python/screen/style/transform blocks are skipped so their code strings are
//! not mistaken for dialogue — but screen/python bodies are still harvested for
//! *display* strings (bare `text "…"` literals, python-level quest names and
//! `renpy.notify` messages), which translate at render time via the
//! `translate <lang> strings` table; see the harvesting section below.

use super::codes::ExtractOpts;
use super::renpy_tl::{self, Say};
use super::rpa;
use super::unrpyc::{self, PyMajor};
use super::{DetectResult, GameEngine};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct RenpyEngine;

impl GameEngine for RenpyEngine {
    fn id(&self) -> &'static str {
        "renpy"
    }

    fn name(&self) -> &'static str {
        "Ren'Py"
    }

    fn detect(&self, root: &Path) -> bool {
        game_dir(root).is_some()
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        let dir = game_dir(root).ok_or_else(|| anyhow!("not a Ren'Py project"))?;
        // A packed game has no loose `.rpy` yet — count the source scripts inside
        // its `.rpa` without unpacking (this is a read-only preview; the actual
        // extraction happens at import in `extract`).
        let count = if has_rpy(&dir) {
            collect_rpy(&dir).len()
        } else {
            rpa_rpy_count(&dir)
        };
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: dir.to_string_lossy().to_string(),
            file_count: count,
            ..Default::default()
        })
    }

    fn extract(&self, root: &Path, _opts: &ExtractOpts) -> Result<Vec<TransUnit>> {
        let dir = game_dir(root).ok_or_else(|| anyhow!("not a Ren'Py project"))?;
        // Scripts packed in a `.rpa` with no loose source: pull the source `.rpy`
        // out of the archive first (many games ship the `.rpy` alongside the
        // compiled `.rpyc`), so the normal line-based flow below can read them.
        ensure_unpacked(&dir)?;
        let mut rpys = collect_rpy(&dir);
        // Compiled scripts still need decompiling when either no `.rpy` were
        // recoverable at all (a fully compiled-only game) OR the game ships *some*
        // loose `.rpy` (e.g. a `splash.rpy`) yet keeps the bulk of its story as
        // `.rpyc` packed in a `.rpa` — a single loose script must not mask hundreds
        // of undecompiled ones. The game ships its own Python + Ren'Py runtime, all
        // unrpyc needs. `needs_decompile` gates this so a fully-source game (and any
        // re-import once decompiled) skips the work and stays idempotent.
        let mut decompile_hint = None;
        if is_renpy_game_dir(&dir) && (rpys.is_empty() || needs_decompile(&dir)) {
            decompile_hint = ensure_decompiled(&dir, root)?;
            rpys = collect_rpy(&dir); // re-scan for the `.rpy` unrpyc just wrote
        }
        // Prefer translating from an existing `tl/<en|ja|zh>/` tree when the game ships
        // one: that text is usually English (easier + higher-quality to translate from
        // than the base language, which may be Russian/etc.) and it already carries the
        // game's real translation ids, so export just retags it to the target locale.
        // Falls through to the base scripts when there is no such tree.
        if let Some((src_lang, src_dir)) = preferred_tl_source(&dir) {
            let mut units = Vec::new();
            for path in rpy_files_under(&src_dir) {
                let rel = rel_path(&dir, &path);
                let content =
                    std::fs::read_to_string(&path).with_context(|| format!("reading {rel}"))?;
                extract_from_tl(&rel, &src_lang, &content, &mut units);
            }
            if !units.is_empty() {
                return Ok(units);
            }
        }
        // Still nothing translatable. Fail the import with an actionable message
        // instead of silently producing an empty project (or, worse, importing the
        // SDK's renpy/common UI strings as if they were the game). If auto-decompile
        // was attempted, say why it couldn't finish.
        if rpys.is_empty() && is_renpy_game_dir(&dir) {
            return Err(anyhow!(
                "This Ren'Py game ships only compiled scripts (found .rpa/.rpyc but no source \
                 .rpy, even inside the archives).{}",
                match decompile_hint {
                    Some(why) => format!(
                        " Automatic decompile could not run: {why}. Decompile the .rpyc to .rpy \
                         (e.g. with unrpyc), then re-import."
                    ),
                    None =>
                        " Decompile the .rpyc to .rpy (e.g. with unrpyc), then re-import — this \
                         translator edits the .rpy source."
                            .to_string(),
                }
            ));
        }
        let mut units = Vec::new();
        let mut files: Vec<(String, String)> = Vec::new();
        let mut py_seen: HashSet<String> = HashSet::new(); // dedupe python display strings project-wide
        for path in rpys {
            let rel = rel_path(&dir, &path);
            let content =
                std::fs::read_to_string(&path).with_context(|| format!("reading {rel}"))?;
            extract_rpy(&rel, &content, &mut units, &mut py_seen);
            files.push((rel, content));
        }
        // Character names — a cross-file pass, since `define c = Character(name_var)`
        // and `name_var = "…"` can live in different files.
        extract_character_names(&files, &mut units);
        Ok(units)
    }

    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> Result<()> {
        let dir = game_dir(root).ok_or_else(|| anyhow!("not a Ren'Py project"))?;

        // Group applied units by file. Character-name units (pointer `name#<char>`)
        // and python display strings (`str#…`) aren't spliceable spans — names are
        // applied via the `tl/` zzz Character re-define, python strings via the
        // strings-table (splicing the constructor arg would desync `find_quest`-style
        // lookups keyed by the English text) — so skip them here.
        let mut by_file: BTreeMap<&str, Vec<&TransUnit>> = BTreeMap::new();
        for u in units {
            if u.status.is_applied()
                && u.translation.is_some()
                && !u.pointer.starts_with("name#")
                && !u.pointer.starts_with("str#")
                && !u.pointer.starts_with("pylist#")
            {
                by_file.entry(u.file.as_str()).or_default().push(u);
            }
        }

        for (file, mut file_units) in by_file {
            let src = dir.join(file);
            let mut bytes = std::fs::read(&src).with_context(|| format!("reading {file}"))?;

            // Apply from the end of the file backwards so earlier byte offsets
            // stay valid as we splice.
            file_units.sort_by_key(|u| Reverse(parse_pointer(&u.pointer).map(|(s, _)| s).unwrap_or(0)));
            for u in file_units {
                let (start, len) = parse_pointer(&u.pointer)
                    .ok_or_else(|| anyhow!("bad Ren'Py pointer {} in {}", u.pointer, file))?;
                if start + len > bytes.len() {
                    return Err(anyhow!(
                        "stale pointer {} in {} — re-extract needed",
                        u.pointer,
                        file
                    ));
                }
                let translation = u.translation.clone().unwrap_or_default();
                bytes.splice(start..start + len, translation.into_bytes());
            }

            let out = out_dir.join(file);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out, bytes).with_context(|| format!("writing {file}"))?;
        }
        Ok(())
    }

    /// A patched `X.rpy` leaves a stale `X.rpyc`; removing it forces Ren'Py to
    /// recompile from our edited source rather than load the old bytecode.
    fn stale_companions(&self, file: &str) -> Vec<String> {
        match file.strip_suffix(".rpy") {
            Some(stem) => vec![format!("{stem}.rpyc")],
            None => Vec::new(),
        }
    }
}

/// A Ren'Py project root holds a `game/` dir with the scripts. Prefer `game/`
/// whenever it looks like a Ren'Py game folder — even when the `.rpy` are packed in
/// a `.rpa` (no loose source) — so an *archived* game isn't mis-resolved to `root`,
/// where the SDK's `renpy/common/*.rpy` would be imported as if they were the game
/// (and where the `tl/` check would then look in the wrong place). Only when there
/// is no `game/` subdir do we accept a `root` that itself holds `.rpy` (an archive
/// unpacked straight to the root).
pub fn game_dir(root: &Path) -> Option<PathBuf> {
    let game = root.join("game");
    if game.is_dir() && (has_rpy(&game) || is_renpy_game_dir(&game)) {
        return Some(game);
    }
    if has_rpy(root) {
        return Some(root.to_path_buf());
    }
    None
}

/// The `.rpa` archives directly under `dir` (Ren'Py packs them at the game-dir
/// top level), sorted for deterministic unpack order.
fn archives_in(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("rpa") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// How many distinct source `.rpy` are packed in this dir's `.rpa` archives —
/// read-only, for the import preview count. Archives with no readable index are
/// skipped silently (the real unpack in [`ensure_unpacked`] surfaces errors).
fn rpa_rpy_count(dir: &Path) -> usize {
    let mut names = HashSet::new();
    for archive in archives_in(dir) {
        if let Ok(list) = rpa::list_rpy(&archive) {
            names.extend(list);
        }
    }
    names.len()
}

/// Materialize the source `.rpy` a game ships packed. When `dir` has no loose
/// `.rpy` but does have `.rpa` archives, extract every `.rpy` out of them into
/// `dir` so the normal Ren'Py flow (and the game's own runtime, for `tl/` export)
/// can read them. No-op once loose `.rpy` are present, so it never clobbers a
/// hand-edited or already-unpacked script and re-import is idempotent.
///
/// This is the one point the engine writes into the game dir before export — but
/// it only surfaces source that already exists inside the `.rpa` (exactly what
/// `unrpa` does by hand), never modifying the archive or any existing file.
fn ensure_unpacked(dir: &Path) -> Result<usize> {
    if has_rpy(dir) {
        return Ok(0);
    }
    let mut total = 0;
    for archive in archives_in(dir) {
        // Best-effort: an archive we can't read as RPA (corrupt, or a format we
        // don't support) simply yields no source here — the caller then surfaces
        // the actionable "decompile the .rpyc" message. One odd archive must not
        // abort recovering source from the others.
        if let Ok(n) = rpa::extract_rpy(&archive, dir) {
            total += n;
        }
    }
    Ok(total)
}

/// Whether the game still has compiled scripts (`.rpyc`) — loose on disk or packed in
/// a `.rpa` — with no decompiled `.rpy` sibling yet. A game can ship a few loose
/// `.rpy` (e.g. `splash.rpy`) while keeping the bulk of its story as `.rpyc` inside
/// `src.rpa`, so the presence of *some* `.rpy` doesn't mean the source is complete.
/// [`extract`] uses this to decide whether [`ensure_decompiled`] must still run even
/// when a loose `.rpy` is already present. Cheap: loose files are a directory walk,
/// archives are read index-only (no bytes streamed) and short-circuit on the first
/// undecompiled entry.
fn needs_decompile(dir: &Path) -> bool {
    // A loose `.rpyc` whose `.rpy` sibling is missing.
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                // Skip `tl/` — its `.rpyc` are translations, not game source.
                if p.file_name().and_then(|n| n.to_str()) != Some("tl") {
                    stack.push(p);
                }
            } else if p.extension().and_then(|x| x.to_str()) == Some("rpyc")
                && !p.with_extension("rpy").exists()
            {
                return true;
            }
        }
    }
    // A `.rpyc` packed in a `.rpa` that hasn't been unpacked+decompiled to a `.rpy`.
    for archive in archives_in(dir) {
        let Ok(names) = rpa::list_rpyc(&archive) else { continue };
        for name in names {
            // archive-relative `foo/bar.rpyc` → the on-disk `.rpy` it decompiles to.
            let rpy_rel = format!("{}.rpy", &name[..name.len() - ".rpyc".len()]);
            let mut rpy = dir.to_path_buf();
            for seg in rpy_rel.split('/') {
                rpy.push(seg);
            }
            if !rpy.exists() {
                return true;
            }
        }
    }
    false
}

/// Best-effort auto-decompile of a compiled-only game: turn its `.rpyc` into source
/// `.rpy` in place so the normal Ren'Py flow can read them. Called from [`extract`]
/// only when no `.rpy` were recoverable and the dir still looks like a Ren'Py game.
///
/// A game that ships `.rpyc` also ships its own Python interpreter
/// (`<root>/lib/<platform>/python`) and Ren'Py runtime — all the bundled
/// [`unrpyc`](super::unrpyc) decompiler needs. We stage any `.rpyc` locked inside
/// `.rpa` onto disk (like [`ensure_unpacked`] does for `.rpy`), then run
/// `<python> unrpyc.py -c <dir>`, which writes a `.rpy` next to every `.rpyc`.
///
/// Returns `Ok(None)` on success (or nothing to do); `Ok(Some(reason))` when it
/// could not decompile, so the caller folds `reason` into the actionable error and
/// the import degrades exactly as before — never a silent empty project. `Err` is
/// reserved for unexpected IO, never for "no Python" or "unrpyc failed".
fn ensure_decompiled(dir: &Path, root: &Path) -> Result<Option<String>> {
    // 1. Make sure the compiled scripts are on disk. Many compiled-only games pack
    //    their `.rpyc` inside a `.rpa` (with assets), so pull them out. No-clobber, so
    //    loose `.rpyc` already on disk are kept; we do NOT gate on their presence
    //    because some games ship a mix (loose gui `.rpyc` + the story packed in
    //    scripts.rpa). Cheap — reads each archive's index + only the `.rpyc` segments,
    //    so asset archives (images, movies) contribute nothing and are never streamed.
    for archive in archives_in(dir) {
        let _ = rpa::extract_rpyc(&archive, dir); // best-effort, mirrors ensure_unpacked
    }
    if !has_rpyc(dir) {
        return Ok(Some(
            "found no .rpyc to decompile (compiled scripts not in the game dir or its archives)"
                .to_string(),
        ));
    }

    // 2. Find the game's own bundled Python; its major version picks the unrpyc
    //    branch. No interpreter → degrade to the actionable error.
    let Some((python, major)) = find_bundled_python(root) else {
        return Ok(Some(
            "no bundled Python interpreter found under the game's lib/ folder".to_string(),
        ));
    };
    let unrpyc_py = unrpyc::materialize(major)?;

    // 3. Run unrpyc over the game dir (it recurses and writes `.rpy` beside each
    //    `.rpyc`). Mirrors export_tl's three-tier subprocess handling: a spawn
    //    failure or a non-zero exit degrades to the actionable error rather than
    //    aborting the import. Retry once with --try-harder against obfuscation.
    match run_unrpyc(&python, &unrpyc_py, dir, false) {
        Ok(true) => Ok(None),
        Ok(false) => match run_unrpyc(&python, &unrpyc_py, dir, true) {
            Ok(true) => Ok(None),
            Ok(false) => Ok(Some("unrpyc could not decompile the scripts".to_string())),
            Err(e) => Ok(Some(format!("could not run the bundled Python: {e}"))),
        },
        Err(e) => Ok(Some(format!("could not run the bundled Python: {e}"))),
    }
}

/// Run `<python> <unrpyc_py> -c [--try-harder] <dir>`. `Ok(true)` on exit 0,
/// `Ok(false)` on a non-zero exit, `Err` only when the process could not be spawned.
///
/// `unrpyc.py` does `import decompiler` / `import deobfuscate` — sibling modules in
/// its own dir. A game's bundled Ren'Py `python` does NOT auto-add the script's
/// directory to `sys.path` the way stock CPython does (its runtime sets `sys.path`
/// itself), so those imports fail with `No module named decompiler` when we launch
/// it directly. Point `PYTHONPATH` at the unrpyc dir so the package resolves
/// regardless of the interpreter's script-path handling.
fn run_unrpyc(python: &Path, unrpyc_py: &Path, dir: &Path, try_harder: bool) -> Result<bool> {
    let pkg_dir = unrpyc_py.parent().unwrap_or(unrpyc_py);
    let mut cmd = Command::new(python);
    // Run unrpyc.py by its *bare name* from its own dir. A game's bundled Ren'Py
    // `python` sets `sys.path` itself and doesn't honor PYTHONPATH or add an absolute
    // script's dir, so `import decompiler` (a sibling module) fails. With cwd = the
    // unrpyc dir and a relative script name, `sys.path[0]` becomes "" (the cwd), so
    // the sibling package resolves.
    //
    // Deliberately no `-p`: Ren'Py's bundled interpreter (py2 *and* py3) ships
    // without the `_multiprocessing` C module, so unrpyc's `-p`/--processes choice
    // set is empty and any `-p N` is rejected outright; with it absent unrpyc already
    // falls back to single-threaded decompilation, which is what we want anyway.
    cmd.current_dir(pkg_dir).arg("unrpyc.py").arg("-c");
    if try_harder {
        cmd.arg("--try-harder");
    }
    cmd.arg(dir);
    let output = cmd
        .output()
        .with_context(|| format!("spawning {}", python.display()))?;
    Ok(output.status.success())
}

/// Whether `dir` (recursively, skipping `tl/`) holds any compiled `.rpyc`.
fn has_rpyc(dir: &Path) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !is_tl_dir(&p) {
                    stack.push(p);
                }
            } else if p.extension().and_then(|x| x.to_str()) == Some("rpyc") {
                return true;
            }
        }
    }
    false
}

/// The Python interpreter a Ren'Py game bundles under `<root>/lib/<platform>/`, plus
/// its major version (which selects the [`unrpyc`](super::unrpyc) branch). Only an
/// interpreter matching the *host* OS can be executed, so we match the platform dir
/// against [`std::env::consts::OS`] and prefer 64-bit. Ren'Py's dir naming varies —
/// `py3-windows-x86_64` / `py2-windows-x86_64` (7.4+/8) or a bare `windows-x86_64`
/// (older 6/7) — so the major version is read from the prefix when present, else
/// sniffed from the `libpython2*/3*` runtime beside the interpreter (or a
/// `lib/pythonX.Y` marker). Returns `None` when no runnable interpreter is found.
fn find_bundled_python(root: &Path) -> Option<(PathBuf, PyMajor)> {
    let lib = root.join("lib");
    let os_tok = match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "mac",
        _ => "linux",
    };
    let mut best: Option<(PathBuf, PyMajor, u8)> = None; // (exe, major, arch rank)
    for e in std::fs::read_dir(&lib).ok()?.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let lname = name.to_ascii_lowercase();
        let host = lname.contains(os_tok) || (os_tok == "mac" && lname.contains("darwin"));
        if !host {
            continue;
        }
        let Some(exe) = python_in(&p) else { continue };
        let rank = if lname.contains("x86_64")
            || lname.contains("amd64")
            || lname.contains("aarch64")
            || lname.contains("arm64")
            || lname.contains("universal")
        {
            2 // 64-bit
        } else {
            1 // 32-bit / unknown
        };
        if best.as_ref().map_or(true, |(_, _, r)| rank > *r) {
            best = Some((exe, python_major(&lname, &p, &lib), rank));
        }
    }
    best.map(|(exe, major, _)| (exe, major))
}

/// The interpreter executable inside a Ren'Py `lib/<platform>/` dir, if any.
fn python_in(dir: &Path) -> Option<PathBuf> {
    ["python.exe", "pythonw.exe", "python", "pythonw"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
}

/// Decide a bundled interpreter's Python major version: the `py3-`/`py2-` dir prefix
/// when present, else the `libpython3*`/`libpython2*` runtime beside it, else a
/// `lib/python3*`/`lib/python2*` marker dir. Defaults to Py3 (modern Ren'Py).
fn python_major(dirname_lower: &str, platform_dir: &Path, lib: &Path) -> PyMajor {
    if dirname_lower.starts_with("py3-") {
        return PyMajor::Py3;
    }
    if dirname_lower.starts_with("py2-") {
        return PyMajor::Py2;
    }
    for probe in [platform_dir, lib] {
        if let Ok(rd) = std::fs::read_dir(probe) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_ascii_lowercase();
                if n.contains("python3") {
                    return PyMajor::Py3;
                }
                if n.contains("python2") {
                    return PyMajor::Py2;
                }
            }
        }
    }
    PyMajor::Py3
}

/// True if `dir` carries a Ren'Py game fingerprint other than loose `.rpy`: a
/// packed `.rpa` archive, a compiled `.rpyc`, or the `script_version.txt` marker.
/// Used both to recognize an archived `game/` and to explain (in [`extract`]) why
/// nothing was translatable.
fn is_renpy_game_dir(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    rd.flatten().any(|e| {
        let p = e.path();
        p.is_file()
            && (matches!(p.extension().and_then(|x| x.to_str()), Some("rpa") | Some("rpyc"))
                || p.file_name().and_then(|n| n.to_str()) == Some("script_version.txt"))
    })
}

fn has_rpy(dir: &Path) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !is_tl_dir(&p) {
                    stack.push(p);
                }
            } else if is_rpy(&p) {
                return true;
            }
        }
    }
    false
}

/// The global `.rpy` this tool writes into a game on export (default language +
/// font remap — see [`setup_language`]). It is our own artifact, never game
/// source, so it must never be re-imported.
const GENERATED_RPY: &str = "zzz_translator.rpy";

fn is_rpy(p: &Path) -> bool {
    p.is_file()
        && p.extension().map(|e| e == "rpy").unwrap_or(false)
        // Skip our export artifact: its quoted font paths (`fonts/tl_font.ttf`, the
        // remapped game fonts) would otherwise be extracted as if they were dialogue.
        && p.file_name().and_then(|n| n.to_str()) != Some(GENERATED_RPY)
}

/// Ren'Py's `game/tl/<language>/` tree holds *translations* of the source
/// script (one dir per shipped language), not source text — skip it so other
/// languages don't get imported as strings to translate. The source strings all
/// live in the base `.rpy` files outside `tl/`.
fn is_tl_dir(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()) == Some("tl")
}

/// Every source `.rpy` under `dir` (excluding the `tl/` translations tree),
/// sorted for deterministic unit order.
fn collect_rpy(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !is_tl_dir(&p) {
                    stack.push(p);
                }
            } else if is_rpy(&p) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Forward-slashed path relative to the game dir (stable across platforms).
fn rel_path(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn parse_pointer(p: &str) -> Option<(usize, usize)> {
    let (a, b) = p.split_once(':')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

// ---------------------------------------------------------------------------
// Line-based extraction
// ---------------------------------------------------------------------------

/// What kind of non-dialogue block a header opens. The body is never dialogue,
/// but `Screen` and `Python` bodies still carry display strings worth harvesting
/// (bare screen literals, python-level quest names / notify messages).
#[derive(Clone, Copy, PartialEq)]
enum SkipKind {
    Screen,
    Python,
    /// style/transform/layeredimage/testcase — pure props/assets, harvest nothing.
    Other,
}

fn skip_kind_of(head: &str) -> Option<SkipKind> {
    match head {
        "python" => Some(SkipKind::Python),
        "screen" => Some(SkipKind::Screen),
        "style" | "transform" | "layeredimage" | "testcase" => Some(SkipKind::Other),
        _ => None,
    }
}

/// Blocks whose bodies are code/UI, not dialogue — skip everything indented
/// under them (returning what kind of block, so display strings can still be
/// harvested from screen/python bodies).
fn block_skip_kind(trimmed: &str) -> Option<SkipKind> {
    const HEADS: &[(&str, SkipKind)] = &[
        ("python", SkipKind::Python),
        ("screen ", SkipKind::Screen),
        ("screen:", SkipKind::Screen),
        ("style ", SkipKind::Other),
        ("style:", SkipKind::Other),
        ("transform ", SkipKind::Other),
        ("transform:", SkipKind::Other),
        ("layeredimage ", SkipKind::Other),
        ("testcase ", SkipKind::Other),
    ];
    for (h, k) in HEADS {
        if trimmed.starts_with(h) {
            return Some(*k);
        }
    }
    // `init [priority] <python|screen|style|transform|layeredimage|testcase> …:` —
    // a block whose body is raw code / screen-language / style props, regardless of
    // the optional integer priority (e.g. `init -100 python in phone.application:`,
    // `init -1 style frame:`, `init -501 screen Log_scr():`). Skip it so asset paths
    // and style/property kwargs (`background Frame("gui/frame.png")`,
    // `properties gui.text_properties("input")`, `add "main_menu"`) aren't mistaken
    // for dialogue and translated (which breaks the screen / renames the style and
    // crashes Ren'Py). The bare forms are already in HEADS; this catches the
    // `init`-prefixed forms. Any `_()`-wrapped strings inside are still harvested.
    if let Some(rest) = trimmed.strip_prefix("init") {
        if rest.starts_with(char::is_whitespace) {
            let mut toks = rest.split_whitespace();
            let mut head = toks.next();
            if head.map(|t| t.parse::<i64>().is_ok()).unwrap_or(false) {
                head = toks.next(); // consume the optional priority
            }
            if let Some(h) = head {
                return skip_kind_of(h.trim_end_matches(':'));
            }
        }
    }
    None
}

/// Statements whose leading keyword means any string on the line is not
/// dialogue (asset names, definitions, control flow, inline python).
fn is_line_skip(first: &str) -> bool {
    const KW: &[&str] = &[
        "$", "define", "default", "image", "scene", "show", "hide", "play", "stop", "queue",
        "voice", "jump", "call", "return", "label", "pass", "window", "nvl", "camera", "pause",
        "with", "init", "python", "screen", "style", "transform", "layeredimage", "testcase",
    ];
    KW.contains(&first)
}

fn first_token(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or("")
}

/// Find the first single/double-quoted string on a line. Returns
/// `(inner_start, inner_len, after_close)` as byte indices into `line`.
/// Honors `\"` escapes; a `#` reached before any quote means the rest is a
/// comment (no dialogue string).
fn first_string(line: &str) -> Option<(usize, usize, usize)> {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'#' => return None,
            q @ (b'"' | b'\'') => {
                let inner_start = i + 1;
                let mut j = inner_start;
                while j < b.len() {
                    if b[j] == b'\\' {
                        j += 2;
                        continue;
                    }
                    if b[j] == q {
                        return Some((inner_start, j - inner_start, j + 1));
                    }
                    j += 1;
                }
                return None; // unterminated
            }
            _ => i += 1,
        }
    }
    None
}

/// Net change in Python bracket depth `()[]{}` across a line, ignoring anything
/// inside string literals or after a `#` comment. Used to follow multi-line
/// define/default/$ statements so their bodies aren't mistaken for dialogue.
fn bracket_delta(line: &str) -> i32 {
    let b = line.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'#' => break,
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == q {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                i += 1;
            }
            b')' | b']' | b'}' => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    depth
}

/// Byte spans (relative to `line`) of the inner content of each `_("...")`
/// gettext call — Ren'Py's explicit "this string is translatable" marker, which
/// may appear anywhere including inside screen/python blocks.
fn gettext_spans(line: &str) -> Vec<(usize, usize)> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < b.len() {
        // A gettext call is a lone `_` (not the tail of an identifier) then `(`.
        if b[i] == b'_'
            && b[i + 1] == b'('
            && !(i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_'))
        {
            let mut j = i + 2;
            while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
                j += 1;
            }
            if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
                if let Some((inner_rel, inner_len, after_close)) = first_string(&line[j..]) {
                    if inner_len > 0 {
                        out.push((j + inner_rel, inner_len));
                    }
                    i = j + after_close;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Display-string harvesting (bare screen literals + python-level UI text)
// ---------------------------------------------------------------------------
//
// Two sources of user-visible text that neither the say/menu path nor Ren'Py's
// own `translate` scanner sees (the scanner only collects `_()`/`__()`/`_p()`):
//
//   1. Bare screen literals: `text "Quest Log"` / `textbutton "Close"` in a
//      `screen` block the game dev forgot to `_()`-wrap.
//   2. Python-level display strings: `$ q = Quest("Go to University", …)`,
//      `$ renpy.notify("Objective complete")` — stored in variables and shown
//      later via `text _qn`.
//
// Both are translatable *at display time*: every string a screen renders runs
// through `renpy.substitutions.substitute` → `translate_string`, which consults
// the `translate <lang> strings` table — even when the text came from a
// variable. So these units are exported as strings-block entries (see
// `setup_language`), which translates the display while python-side identity
// (e.g. `find_quest("Go to University")` keyed by the English name) is untouched.

/// Inner byte spans of every single/double-quoted string on a python-ish line
/// (`#` starts a comment; `\` escapes are honored).
fn python_string_spans(line: &str) -> Vec<(usize, usize)> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'#' => break,
            b'"' | b'\'' => match first_string(&line[i..]) {
                Some((rel, len, after)) => {
                    out.push((i + rel, len));
                    i += after;
                }
                None => break, // unterminated
            },
            _ => i += 1,
        }
    }
    out
}

/// `s` with `[interpolations]` and `{text tags}` removed, so a string that is
/// *only* markup doesn't count as display text.
fn strip_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let (mut sq, mut br) = (0i32, 0i32);
    for c in s.chars() {
        match c {
            '[' => sq += 1,
            ']' => sq = (sq - 1).max(0),
            '{' => br += 1,
            '}' => br = (br - 1).max(0),
            _ if sq == 0 && br == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Baseline "is this worth showing a translator" filter shared by both harvests:
/// must contain a letter outside markup, and must not look like an asset path or
/// a `#rrggbb` colour.
fn display_text_ok(s: &str) -> bool {
    if s.contains('/') || s.starts_with('#') {
        return false;
    }
    // An asset filename ("door h.png", "817506__soft-tap.mp3") reads as prose —
    // spaces, letters — but naming it in the translation breaks the lookup.
    const ASSET_EXT: [&str; 12] = [
        ".png", ".jpg", ".jpeg", ".webp", ".gif", ".ogg", ".mp3", ".wav", ".opus", ".webm",
        ".mp4", ".ttf",
    ];
    let lower = s.to_ascii_lowercase();
    if ASSET_EXT.iter().any(|e| lower.ends_with(e)) {
        return false;
    }
    strip_markup(s).chars().any(|c| c.is_alphabetic())
}

/// Python-string filter: multi-word text is display-worthy; a single word is
/// accepted only from a `renpy.notify(…)` line (`allow_single`) and only when it
/// doesn't look like an identifier/attribute (`_`/`.`) — dict keys, flags and
/// style names stay untouched.
fn python_display_ok(s: &str, allow_single: bool) -> bool {
    if !display_text_ok(s) {
        return false;
    }
    let stripped = strip_markup(s);
    if stripped.trim().contains(char::is_whitespace) {
        return true;
    }
    allow_single && !s.contains('_') && !s.contains('.')
}

/// For a screen statement whose argument is display text (`text "…"`,
/// `textbutton "…"`, `label "…"`), the byte offset (into `trimmed`) where that
/// argument starts — and only when the argument is a *string literal directly
/// after the keyword*. `text scene["name"]:` (the string is a dict key) or
/// `text prompt style "input_prompt"` (the string is a style name) must NOT
/// match: splicing those corrupts lookups / renames styles and crashes Ren'Py.
fn screen_text_arg(trimmed: &str) -> Option<usize> {
    for k in ["text ", "textbutton ", "label "] {
        if let Some(rest) = trimmed.strip_prefix(k) {
            let arg = rest.trim_start();
            if arg.starts_with('"') || arg.starts_with('\'') {
                return Some(trimmed.len() - arg.len());
            }
            return None;
        }
    }
    None
}

/// Screen lines whose string arguments are display text rather than an asset path,
/// a style name or a variable name: Ren'Py's own `tooltip` property, and the
/// `Set*Variable` / `Notify` actions games use to feed a tooltip or status widget
/// (`hovered SetVariable("tooltip_text", "Now is not the time…")`). The value is
/// shown through a `text` widget, so it translates via the strings table like any
/// other display string — [`harvest_python_line`]'s whitespace rule already drops
/// the variable-name argument.
fn screen_text_carrier(trimmed: &str) -> bool {
    first_token(trimmed) == "tooltip"
        || trimmed.contains("SetVariable(")
        || trimmed.contains("SetScreenVariable(")
        || trimmed.contains("SetLocalVariable(")
        || trimmed.contains("Notify(")
}

/// `define days_of_week = ["Sunday", "Monday", …]` — a list literal whose elements
/// are *all* string literals. These are UI words the game interpolates
/// (`"Day [day_number] ([days_of_week[i]]), [parts_of_day[p]]"`), and interpolation
/// runs *after* `translate_string`, so a strings-table entry never reaches them:
/// the list in the store has to be replaced (see [`setup_language`]). Returns the
/// variable and each element's text.
fn string_list_define(trimmed: &str) -> Option<(String, Vec<String>)> {
    let body = trimmed
        .strip_prefix("define ")
        .or_else(|| trimmed.strip_prefix("default "))?;
    let eq = assign_eq(body)?;
    let var = body[..eq].trim();
    if !is_ident(var) {
        return None;
    }
    let rhs = body[eq + 1..].trim();
    let rhs = rhs.split('#').next().unwrap_or(rhs).trim(); // drop a trailing comment
    let inner = rhs.strip_prefix('[')?.strip_suffix(']')?;
    let mut items = Vec::new();
    let mut rest = inner.trim();
    while !rest.is_empty() {
        let (r, l, after) = first_string(rest)?; // a non-string element disqualifies the list
        if rest[..r.saturating_sub(1)].trim() != "" {
            return None;
        }
        items.push(rest[r..r + l].to_string());
        rest = rest[after..].trim_start();
        rest = rest.strip_prefix(',').unwrap_or(rest).trim_start();
    }
    if items.is_empty() || !items.iter().all(|s| display_text_ok(s)) {
        return None;
    }
    Some((var.to_string(), items))
}

/// Harvest display strings from one python-ish line (a `$`/`define`/`default`
/// statement, a `python` block body line, or a multi-line expression
/// continuation). Emitted with a `str#` pointer: matched into the translation by
/// *display string*, never spliced in place (splicing the constructor arg would
/// desync python-side lookups keyed by the English text).
fn harvest_python_line(
    file: &str,
    raw: &str,
    line_start: usize,
    seen: &mut HashSet<usize>,
    py_seen: &mut HashSet<String>,
    out: &mut Vec<TransUnit>,
) {
    // Character names have their own pass (`extract_character_names`); docstrings
    // are code commentary, not UI.
    if raw.contains("Character(") || raw.contains("\"\"\"") || raw.contains("'''") {
        return;
    }
    // A notify message or a tooltip is often one word ("Map", "2nd Floor") — still
    // display text. The identifier rule below keeps the `SetVariable("tooltip_text",
    // …)` target out.
    let allow_single = raw.contains("renpy.notify(") || screen_text_carrier(raw.trim_start());
    for (rel, len) in python_string_spans(raw) {
        if len == 0 {
            continue;
        }
        let s = &raw[rel..rel + len];
        if !python_display_ok(s, allow_single) {
            continue;
        }
        let abs = line_start + rel;
        if !seen.insert(abs) {
            continue; // already harvested as a `_()` string
        }
        // One unit per distinct string project-wide: these are matched by their
        // text (a strings-table entry), so repeats (`find_quest("Go to University")`
        // at 20 call sites) add nothing but grid noise.
        if !py_seen.insert(s.to_string()) {
            continue;
        }
        out.push(TransUnit::new(file, format!("str#{abs}:{len}"), UnitKind::Term, s));
    }
}

/// Harvest a bare screen literal (`text "Quest Log"`). Keeps a real byte-span
/// pointer: the launcher (`tl/`) export path translates it via the strings
/// table, while the in-place fallback can still splice it directly.
/// `arg` is the offset of the literal within the trimmed statement (from
/// [`screen_text_arg`] — the quote directly after the keyword).
fn harvest_screen_literal(
    file: &str,
    raw: &str,
    indent: usize,
    arg: usize,
    line_start: usize,
    seen: &mut HashSet<usize>,
    out: &mut Vec<TransUnit>,
) {
    // `text "who" id "who"` — a say-screen placeholder replaced at runtime; the
    // literal is an id, not display text.
    if raw.contains(" id ") {
        return;
    }
    let Some((rel, len, _)) = first_string(&raw[indent + arg..]) else {
        return;
    };
    if len == 0 {
        return;
    }
    let rel = indent + arg + rel;
    let s = &raw[rel..rel + len];
    if !display_text_ok(s) {
        return;
    }
    let abs = line_start + rel;
    if seen.insert(abs) {
        out.push(TransUnit::new(file, format!("{abs}:{len}"), UnitKind::Term, s));
    }
}

/// Every `.rpy` at or under `dir` (a plain recursive walk — unlike [`collect_rpy`],
/// which skips `tl/`, this is used *on* a `tl/<lang>/` subtree).
fn rpy_files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|x| x.to_str()) == Some("rpy") {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

/// The best `tl/<lang>/` folder to translate *from*, by the source-language
/// preference **English > Japanese > Chinese** — a game that ships one of those
/// localizations is easier to translate from than its (possibly Russian/…) base.
/// Only en/ja/zh folders holding loose `.rpy` qualify; every other shipped language
/// is ignored. Returns (folder name, path), or None (→ base-script extraction).
fn preferred_tl_source(dir: &Path) -> Option<(String, PathBuf)> {
    let tl = dir.join("tl");
    let mut cands: Vec<(u8, String, PathBuf)> = std::fs::read_dir(&tl)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter_map(|p| {
            let name = p.file_name()?.to_str()?.to_string();
            let rank = super::source_lang_rank(&name)?; // only en/ja/zh
            if rpy_files_under(&p).is_empty() {
                return None; // needs loose .rpy to read from
            }
            Some((rank, name, p))
        })
        .collect();
    cands.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    cands.into_iter().next().map(|(_, name, p)| (name, p))
}

/// Which part of a `translate <lang> …:` block we're inside.
enum TlBlock {
    /// A `translate <lang> <id>:` dialogue block — its say line is translatable.
    Dialogue,
    /// A `translate <lang> strings:` block — each `new "…"` is translatable.
    Strings,
    /// A `translate <lang> python:` / `style …:` block, or none — no text.
    Other,
}

/// Extract the translatable **English (etc.) source** strings from one `tl/<src>/`
/// file: the say line inside each `translate <src> <id>:` block, and each `new "…"`
/// inside a `translate <src> strings:` block. Byte-span pointers into this file, so
/// export splices the translation back in and retags the block to the target locale.
/// The commented original (`# c "…"`) and the `old "…"` key are left untouched.
fn extract_from_tl(file: &str, src_lang: &str, content: &str, out: &mut Vec<TransUnit>) {
    let header = format!("translate {src_lang} ");
    let mut block = TlBlock::Other;
    let mut offset = 0usize;
    for line in content.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let raw = line.strip_suffix('\n').unwrap_or(line);
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = raw.trim_start();
        let indent = raw.len() - trimmed.len();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // A block header sits at column 0; anything else at column 0 ends the block.
        if indent == 0 {
            block = match trimmed.strip_prefix(header.as_str()).map(str::trim) {
                Some(rest) if rest.starts_with("strings") => TlBlock::Strings,
                Some(rest) if rest.starts_with("python") || rest.starts_with("style") => {
                    TlBlock::Other
                }
                Some(_) => TlBlock::Dialogue, // translate <src> <id>:
                None => TlBlock::Other,
            };
            continue;
        }

        match block {
            TlBlock::Strings if trimmed.starts_with("new ") => {
                if let Some((rel, len, _)) = first_string(raw) {
                    if len > 0 {
                        let abs = line_start + rel;
                        out.push(TransUnit::new(
                            file,
                            format!("{abs}:{len}"),
                            UnitKind::Term,
                            &raw[rel..rel + len],
                        ));
                    }
                }
            }
            TlBlock::Dialogue => {
                // Only the say line is dialogue; skip voice/nvl/show/… inside the block.
                if is_line_skip(first_token(trimmed)) {
                    continue;
                }
                let Some((rel, len, after)) = first_string(raw) else {
                    continue;
                };
                // Two-argument say: `"Speaker" "line"` — the 2nd string is the line.
                let rest2 = raw[after..].trim_start();
                if rest2.starts_with('"') || rest2.starts_with('\'') {
                    if let Some((r2, l2, _)) = first_string(&raw[after..]) {
                        if l2 > 0 {
                            let start = after + r2;
                            let abs = line_start + start;
                            let speaker = raw[rel..rel + len].to_string();
                            out.push(
                                TransUnit::new(
                                    file,
                                    format!("{abs}:{l2}"),
                                    UnitKind::Dialogue,
                                    &raw[start..start + l2],
                                )
                                .with_context(Some(speaker)),
                            );
                        }
                    }
                } else if len > 0 {
                    let abs = line_start + rel;
                    let prefix = raw[indent..rel - 1].trim();
                    let tok = first_token(prefix);
                    let speaker = if tok.is_empty() || tok == "extend" {
                        None
                    } else {
                        Some(tok.to_string())
                    };
                    out.push(
                        TransUnit::new(file, format!("{abs}:{len}"), UnitKind::Dialogue, &raw[rel..rel + len])
                            .with_context(speaker),
                    );
                }
            }
            _ => {}
        }
    }
}

fn extract_rpy(
    file: &str,
    content: &str,
    out: &mut Vec<TransUnit>,
    py_seen: &mut HashSet<String>,
) {
    let mut skip_indent: Option<(usize, SkipKind)> = None;
    let mut skip_expr_depth: i32 = 0; // open brackets of a multi-line define/default/$
    let mut seen: HashSet<usize> = HashSet::new(); // inner-start offsets already taken
    let mut offset = 0usize; // byte offset of the current line within the file

    for line in content.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();

        let raw = line.strip_suffix('\n').unwrap_or(line);
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = raw.trim_start();
        let indent = raw.len() - trimmed.len();

        // Blank/comment lines carry no text and never close a skipped block.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // `_("...")` strings are explicit and translatable even inside skipped
        // (screen/python) blocks, so harvest them before any skip logic.
        for (rel, len) in gettext_spans(raw) {
            let abs = line_start + rel;
            if seen.insert(abs) {
                out.push(TransUnit::new(
                    file,
                    format!("{abs}:{len}"),
                    UnitKind::Term,
                    &raw[rel..rel + len],
                ));
            }
        }

        // A multi-line define/default/$ (a Python dict/Character(...) spanning
        // lines) — its bare strings are colour values, dict keys, asset paths…
        // except display text (quest names/objectives), which the python harvest
        // picks out. Any `_()` strings were already harvested above.
        let delta = bracket_delta(raw);
        if skip_expr_depth > 0 {
            skip_expr_depth = (skip_expr_depth + delta).max(0);
            harvest_python_line(file, raw, line_start, &mut seen, py_seen, out);
            continue;
        }

        // Leaving a skipped block? A line at or below its indent ends it, and is
        // then processed normally for bare say/menu strings. Inside the block,
        // screen/python bodies still yield display strings.
        if let Some((si, kind)) = skip_indent {
            if indent > si {
                match kind {
                    SkipKind::Python => {
                        harvest_python_line(file, raw, line_start, &mut seen, py_seen, out)
                    }
                    SkipKind::Screen => {
                        if trimmed.starts_with('$')
                            || first_token(trimmed) == "default"
                            || screen_text_carrier(trimmed)
                        {
                            harvest_python_line(file, raw, line_start, &mut seen, py_seen, out);
                        } else if let Some(arg) = screen_text_arg(trimmed) {
                            harvest_screen_literal(file, raw, indent, arg, line_start, &mut seen, out);
                        }
                    }
                    SkipKind::Other => {}
                }
                continue;
            }
            skip_indent = None;
        }
        if let Some(kind) = block_skip_kind(trimmed) {
            skip_indent = Some((indent, kind));
            continue;
        }
        let first = first_token(trimmed);
        if is_line_skip(first) {
            // Inline python (`$ …`) and define/default values can hold display
            // strings (quest names, notify messages) — harvest before skipping.
            if trimmed.starts_with('$') || first == "define" || first == "default" {
                // A list of display words (weekdays, times of day) is reached only by
                // replacing the store value — `pylist#<var>#<index>`, applied by the
                // zzz `translate <lang> python:` block, never spliced.
                if let Some((var, items)) = string_list_define(trimmed) {
                    for (i, s) in items.iter().enumerate() {
                        out.push(TransUnit::new(
                            file,
                            format!("pylist#{var}#{i}"),
                            UnitKind::Term,
                            s,
                        ));
                    }
                }
                harvest_python_line(file, raw, line_start, &mut seen, py_seen, out);
            }
            // If the statement opens brackets, it continues on the next lines.
            if delta > 0 {
                skip_expr_depth = delta;
            }
            continue;
        }

        let Some((inner_rel, inner_len, after_close)) = first_string(raw) else {
            continue;
        };
        if inner_len == 0 {
            continue;
        }

        // Two-argument say: `"Speaker" "dialogue"`. The first string is the
        // speaker name (not translatable), the real line is the second string.
        let rest = raw[after_close..].trim_start();
        if rest.starts_with('"') || rest.starts_with('\'') {
            if let Some((rel2, len2, _)) = first_string(&raw[after_close..]) {
                let abs2 = line_start + after_close + rel2;
                if len2 > 0 && seen.insert(abs2) {
                    let speaker = raw[inner_rel..inner_rel + inner_len].to_string();
                    let start = after_close + rel2;
                    let source = &raw[start..start + len2];
                    out.push(
                        TransUnit::new(file, format!("{abs2}:{len2}"), UnitKind::Dialogue, source)
                            .with_context(Some(speaker)),
                    );
                }
            }
            continue;
        }

        let abs = line_start + inner_rel;
        // Already harvested as a `_()` string on this line — don't double-count.
        if !seen.insert(abs) {
            continue;
        }
        let source = &raw[inner_rel..inner_rel + inner_len];

        // A trailing `:` marks a menu choice.
        let after = raw[after_close..].trim();
        let is_choice = after.trim_end().ends_with(':');

        // The text before the opening quote is the speaker, when present.
        let prefix = raw[indent..inner_rel - 1].trim();

        // A real menu choice starts with the quote (empty prefix). A trailing `:`
        // after a NON-empty prefix is a control-flow header whose string is Python
        // code, not dialogue — `if`/`elif`/`while`/`for … "code":` (e.g.
        // `elif selected in areas["House"]:`). Translating it corrupts dict keys /
        // comparisons and crashes the game at runtime, so skip it.
        if is_choice && !prefix.is_empty() {
            continue;
        }

        let speaker = if is_choice || prefix.is_empty() {
            None
        } else {
            let tok = first_token(prefix);
            if tok == "extend" || tok.is_empty() {
                None
            } else {
                Some(tok.to_string())
            }
        };

        let kind = if is_choice {
            UnitKind::Choice
        } else {
            UnitKind::Dialogue
        };
        out.push(TransUnit::new(file, format!("{abs}:{inner_len}"), kind, source).with_context(speaker));
    }
}

/// A Python identifier: non-empty, `[A-Za-z_][A-Za-z0-9_]*`.
fn is_ident(s: &str) -> bool {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Position of a simple assignment `=` in `s` — the first `=` that isn't part of
/// `==`, `<=`, `>=`, `!=`, `+=`, … so `if x == y` / `a += 1` don't look like one.
fn assign_eq(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'=' && b.get(i + 1) != Some(&b'=') {
            let prev = if i == 0 { b' ' } else { b[i - 1] };
            if !matches!(prev, b'!' | b'<' | b'>' | b'=' | b'+' | b'-' | b'*' | b'/' | b'%') {
                return Some(i);
            }
        }
    }
    None
}

/// Extract the display names of `Character(...)` definitions so they can be
/// translated. Ren'Py's `translate` never touches a bare `define c = Character("…")`
/// or its `name_var = "…"` (they aren't say-statements or `_()` strings), so a
/// Japanese name stays Japanese — and turns to tofu boxes once its font is remapped to
/// a Thai-only face. Each is surfaced as a [`UnitKind::Name`] unit keyed
/// (`pointer = "name#<char>"`, `context = <char>`) to the character's variable, and
/// [`setup_language`] re-defines that character with the translation on export.
///
/// Handles both shapes seen in the wild — `Character("literal", …)` and
/// `Character(name_var, …)` with `name_var = "literal"` (possibly in another file).
/// Skips `Character(_("…"))` (already translatable via the strings path), `None`, and
/// interpolated `"[var]"` names.
fn extract_character_names(files: &[(String, String)], out: &mut Vec<TransUnit>) {
    // Pass 1: every simple `<ident> = "literal"` assignment → (file, literal). The RHS
    // must be exactly one quoted string (a plain string-valued variable).
    let mut vars: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    for (file, content) in files {
        for raw in content.lines() {
            let t = raw.trim_start();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let body = t
                .strip_prefix("define ")
                .or_else(|| t.strip_prefix("default "))
                .unwrap_or(t);
            let Some(eq) = assign_eq(body) else { continue };
            let ident = body[..eq].trim();
            if !is_ident(ident) {
                continue;
            }
            let rhs = body[eq + 1..].trim_start();
            if !(rhs.starts_with('"') || rhs.starts_with('\'')) {
                continue;
            }
            let Some((ir, il, after)) = first_string(rhs) else { continue };
            let tail = rhs[after..].trim_start();
            if !tail.is_empty() && !tail.starts_with('#') {
                continue; // RHS is more than a lone string — not a name variable
            }
            vars.entry(ident.to_string())
                .or_insert_with(|| (file.clone(), rhs[ir..ir + il].to_string()));
        }
    }

    // Pass 2: `define <char> = Character(<first arg>, …)`.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (file, content) in files {
        for raw in content.lines() {
            let t = raw.trim_start();
            let Some(after) = t.strip_prefix("define ") else { continue };
            let Some(eq) = after.find('=') else { continue };
            let cident = after[..eq].trim();
            if !is_ident(cident) || seen.contains(cident) {
                continue;
            }
            let rhs = after[eq + 1..].trim_start();
            let Some(rest) = rhs.strip_prefix("Character") else { continue };
            let rest = rest.trim_start();
            let Some(args) = rest.strip_prefix('(').map(str::trim_start) else { continue };
            if args.starts_with("_(") {
                continue; // gettext name — already translatable via the strings path
            }
            let (name_file, name) = if args.starts_with('"') || args.starts_with('\'') {
                let Some((ir, il, _)) = first_string(args) else { continue };
                if il == 0 {
                    continue;
                }
                (file.clone(), args[ir..ir + il].to_string())
            } else {
                let arg = args
                    .split([',', ')', ' ', '\t'])
                    .next()
                    .unwrap_or("")
                    .trim();
                if !is_ident(arg) || arg == "None" {
                    continue;
                }
                match vars.get(arg) {
                    Some((f, txt)) => (f.clone(), txt.clone()),
                    None => continue,
                }
            };
            if name.trim().is_empty() || name.starts_with('[') {
                continue; // empty or interpolated "[var]" — nothing to translate
            }
            seen.insert(cident.to_string());
            out.push(
                TransUnit::new(name_file, format!("name#{cident}"), UnitKind::Name, name)
                    .with_context(Some(cident.to_string())),
            );
        }
    }
}

/// One Ren'Py say statement, located and identified for `tl/<lang>/` output.
#[derive(Debug, Clone)]
pub struct DiaBlock {
    /// The translation identifier Ren'Py will look this line up by.
    pub identifier: String,
    /// Byte span `(start, len)` of the say's inner text — matches the `TransUnit`
    /// pointer so the DB translation can be found.
    pub what_start: usize,
    pub what_len: usize,
    /// Everything on the say line except the text, so the translated line can be
    /// rebuilt as `<prefix> "<translation>"<suffix>`.
    pub prefix: String,
    pub suffix: String,
    /// 1-based source line (for the tl file's location comment).
    pub line: usize,
}

/// Parse each say statement in a `.rpy`, assigning the Ren'Py translation
/// identifier (`label_<md5>` with `_N` collision suffixes). Mirrors
/// [`extract_rpy`]'s say detection exactly so the byte spans line up with the
/// extracted `TransUnit`s, then adds label tracking + `get_code` hashing.
///
/// Menu choices and `_()` strings are NOT returned here — Ren'Py translates
/// those through the string-translation (`translate <lang> strings:`) path.
pub fn dialogue_blocks(content: &str) -> Vec<DiaBlock> {
    let mut skip_indent: Option<usize> = None;
    let mut skip_expr_depth: i32 = 0;
    let mut seen: HashSet<usize> = HashSet::new();
    let mut offset = 0usize;
    let mut line_no = 0usize;

    let mut label: Option<String> = None;
    let mut ids = renpy_tl::IdGen::new();
    let mut out: Vec<DiaBlock> = Vec::new();

    for line in content.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        line_no += 1;

        let raw = line.strip_suffix('\n').unwrap_or(line);
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = raw.trim_start();
        let indent = raw.len() - trimmed.len();

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // `_()` strings go to the string-translation path — mark their offsets so
        // an `e _("...")` say isn't also emitted as an id-block (matches extract).
        for (rel, _len) in gettext_spans(raw) {
            seen.insert(line_start + rel);
        }

        let delta = bracket_delta(raw);
        if skip_expr_depth > 0 {
            skip_expr_depth = (skip_expr_depth + delta).max(0);
            continue;
        }

        if let Some(si) = skip_indent {
            if indent > si {
                continue;
            }
            skip_indent = None;
        }
        if block_skip_kind(trimmed).is_some() {
            skip_indent = Some(indent);
            continue;
        }
        if is_line_skip(first_token(trimmed)) {
            // Track the current label for identifier prefixes. A `_`-prefixed
            // label is an "alternate" and does not change the base label.
            if first_token(trimmed) == "label" {
                if let Some(name) = label_name(trimmed) {
                    if !name.starts_with('_') {
                        label = Some(name);
                    }
                }
            }
            if delta > 0 {
                skip_expr_depth = delta;
            }
            continue;
        }

        let Some((inner_rel, inner_len, after_close)) = first_string(raw) else {
            continue;
        };
        if inner_len == 0 {
            continue;
        }

        // Two-argument say `"Speaker" "dialogue"`: who is the encoded speaker
        // string, the translated text is the second string.
        let rest = raw[after_close..].trim_start();
        if rest.starts_with('"') || rest.starts_with('\'') {
            if let Some((rel2, len2, after2)) = first_string(&raw[after_close..]) {
                let abs2 = line_start + after_close + rel2;
                if len2 > 0 && seen.insert(abs2) {
                    let speaker = &raw[inner_rel..inner_rel + inner_len];
                    let start = after_close + rel2;
                    let what = &raw[start..start + len2];
                    let suffix = raw[after_close + after2..].to_string();
                    let say = build_say(renpy_tl::encode_say_string(speaker), what, &suffix);
                    let id = ids.unique(label.as_deref(), &renpy_tl::digest(&[say]));
                    out.push(DiaBlock {
                        identifier: id,
                        what_start: abs2,
                        what_len: len2,
                        prefix: raw[indent..start].to_string(),
                        suffix,
                        line: line_no,
                    });
                }
            }
            continue;
        }

        let abs = line_start + inner_rel;
        if !seen.insert(abs) {
            continue;
        }

        // A trailing `:` marks a menu choice → string-translation path, not a say.
        let after = raw[after_close..].trim();
        if after.trim_end().ends_with(':') {
            continue;
        }

        let prefix_txt = raw[indent..inner_rel - 1].trim();
        if first_token(prefix_txt) == "extend" {
            // `extend` continues the previous say; Ren'Py still gives it its own
            // id from get_code with who="extend".
        }
        let what = &raw[inner_rel..inner_rel + inner_len];
        let suffix = raw[after_close..].to_string();
        let say = build_say_from_prefix(prefix_txt, what, &suffix);
        let id = ids.unique(label.as_deref(), &renpy_tl::digest(&[say]));
        out.push(DiaBlock {
            identifier: id,
            what_start: abs,
            what_len: inner_len,
            prefix: raw[indent..inner_rel].to_string(),
            suffix,
            line: line_no,
        });
    }
    out
}

/// The label name from a `label NAME(...):` / `label NAME:` line, or None.
fn label_name(trimmed: &str) -> Option<String> {
    let after = trimmed.strip_prefix("label")?;
    let after = after.trim_start();
    let end = after.find(|c: char| c == ':' || c == '(' || c.is_whitespace()).unwrap_or(after.len());
    let name = &after[..end];
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Build a [`Say`] whose `who` is already known (e.g. a two-arg speaker string).
fn build_say(who: String, what: &str, suffix: &str) -> Say {
    let (interact, with_) = parse_suffix(suffix);
    Say {
        who,
        what: what.to_string(),
        interact,
        with_,
        ..Default::default()
    }
}

/// Build a [`Say`] from a normal say's prefix (`who [attrs] [@ temp]`).
fn build_say_from_prefix(prefix: &str, what: &str, suffix: &str) -> Say {
    let mut toks = prefix.split_whitespace();
    let who = toks.next().unwrap_or("").to_string();
    let mut attributes = Vec::new();
    let mut temporary_attributes = Vec::new();
    let mut in_temp = false;
    for t in toks {
        if t == "@" {
            in_temp = true;
        } else if let Some(rest) = t.strip_prefix('@') {
            in_temp = true;
            temporary_attributes.push(rest.to_string());
        } else if in_temp {
            temporary_attributes.push(t.to_string());
        } else {
            attributes.push(t.to_string());
        }
    }
    let (interact, with_) = parse_suffix(suffix);
    Say {
        who,
        attributes,
        temporary_attributes,
        what: what.to_string(),
        interact,
        with_,
        ..Default::default()
    }
}

/// Pull `nointeract` and a `with <expr>` clause out of a say's trailing text.
fn parse_suffix(suffix: &str) -> (bool, Option<String>) {
    let s = suffix.trim();
    let mut interact = true;
    let mut with_ = None;
    if let Some(pos) = s.find("with ") {
        // `with` must be a standalone word.
        let before_ok = pos == 0 || s.as_bytes()[pos - 1] == b' ';
        if before_ok {
            with_ = Some(s[pos + 5..].trim().to_string());
        }
    }
    for t in s.split_whitespace() {
        if t == "nointeract" {
            interact = false;
        }
    }
    (interact, with_)
}

// ---------------------------------------------------------------------------
// tl/<language>/ export (the game's own Ren'Py generates the skeleton; we fill it)
// ---------------------------------------------------------------------------

/// Outcome of a `tl/<lang>/` export.
pub struct TlExport {
    /// Number of `tl/<lang>/` files written.
    pub files: usize,
    /// The `tl/<lang>/` directory that was filled.
    pub dir: PathBuf,
}

/// Harvest the engine/UI strings the generated skeleton exposes but the project
/// never had a unit for. Ren'Py's `translate` scans `renpy/common/*.rpy` too, so
/// `tl/<lang>/common.rpy` (and friends) list every built-in string — quit/main-menu
/// confirmations, save/load prompts, page navigation — as `old "X"\n new "X"`
/// (untranslated: `new == old`). Our extractor deliberately skips `renpy/common`
/// (so the SDK's strings never masquerade as game text), so those lines stay
/// English forever. Return them as fresh `Term` units keyed by their byte span in
/// the skeleton (a `str#` pointer → display-matched, never spliced) so the caller
/// can merge them into the DB; the next Run translates them and the following
/// export's [`renpy_tl::fill_tl`] fills them (it matches `new` by the `old` source).
///
/// `have` is the set of source strings already covered by a unit — skip those, so a
/// game string that also appears in a `strings` block isn't duplicated.
pub fn harvest_tl_untranslated(dir: &Path, existing: &[TransUnit]) -> Vec<TransUnit> {
    let have: HashSet<&str> = existing.iter().map(|u| u.source.as_str()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().and_then(|x| x.to_str()) != Some("rpy") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&p) else { continue };
            // File path relative to the game dir (`tl/<lang>/common.rpy`), matching
            // how the fill loop and the rest of the engine name Ren'Py files.
            let rel = p
                .strip_prefix(dir.parent().and_then(|q| q.parent()).unwrap_or(&d))
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            harvest_untranslated_strings(&rel, &content, &have, &mut seen, &mut out);
        }
    }
    out
}

/// The `translate <lang> strings:` half of [`harvest_tl_untranslated`], for one file.
fn harvest_untranslated_strings(
    file: &str,
    content: &str,
    have: &HashSet<&str>,
    seen: &mut HashSet<String>,
    out: &mut Vec<TransUnit>,
) {
    let mut in_strings = false;
    let mut pending_old: Option<(usize, usize, String)> = None; // (abs, len, text)
    let mut offset = 0usize;
    for line in content.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let raw = line.strip_suffix('\n').unwrap_or(line);
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = raw.trim_start();
        if let Some(rest) = trimmed.strip_prefix("translate ") {
            in_strings = rest.trim_end_matches(':').ends_with("strings");
            pending_old = None;
            continue;
        }
        if !in_strings || trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("old ") {
            pending_old = first_string(raw).and_then(|(rel, len, _)| {
                (len > 0).then(|| (line_start + rel, len, raw[rel..rel + len].to_string()))
            });
        } else if trimmed.starts_with("new ") {
            if let Some((abs, len, old)) = pending_old.take() {
                if let Some((r, l, _)) = first_string(raw) {
                    let new = &raw[r..r + l];
                    // Untranslated (Ren'Py wrote `new == old`), still worth translating
                    // (has a letter), and not already a unit or harvested this run.
                    if new == old
                        && display_text_ok(&old)
                        && !have.contains(old.as_str())
                        && seen.insert(old.clone())
                    {
                        out.push(TransUnit::new(
                            file,
                            format!("str#{abs}:{len}"),
                            UnitKind::Term,
                            &old,
                        ));
                    }
                }
            }
        }
    }
}

/// Export translations the Ren'Py-native way: run the game's own bundled Ren'Py
/// to generate the `game/tl/<lang>/` skeleton (so every translation identifier is
/// exactly what Ren'Py expects), then fill it from the project's translations.
/// The source `.rpy` are never touched — Ren'Py won't recompile them, so
/// version/CDS crashes never happen and `<lang>` becomes a selectable in-game
/// language.
///
/// Returns `Ok(None)` if the game has no bundled Ren'Py launcher (the caller then
/// falls back to in-place injection).
pub fn export_tl(
    root: &Path,
    data_dir: &Path,
    units: &[TransUnit],
    target_lang: &str,
    translate_names: bool,
) -> Result<Option<TlExport>> {
    // tl-source mode: the units were read from an existing `tl/<src>/` tree (their file
    // paths live under `tl/`). Produce `tl/<target>/` by retagging that tree — no
    // launcher / `renpy translate` needed, since it already carries the game's ids.
    if let Some(src_lang) = tl_source_lang(units) {
        return export_tl_from_source(data_dir, &src_lang, units, target_lang).map(Some);
    }
    let Some(exe) = find_launcher(root) else {
        return Ok(None);
    };
    let lang = normalize_lang(target_lang);

    // Compiled-only games carry no source `.rpy` on disk, so Ren'Py's `translate`
    // would emit only the SDK `common.rpy` and leave every line of dialogue
    // untranslated. When the game dir has no source (a compiled-only game whose
    // decompiled `.rpy` were never written, or were lost between import and export —
    // cleaned or re-copied), re-materialize it the same way import does: unpack the
    // `.rpa`, decompile the `.rpyc`, so `translate` sees the game's own scripts.
    // Guarded on "no source present" so a game that already has `.rpy` (shipped or
    // previously decompiled) skips the work — this must not re-decompile every export.
    if collect_rpy(data_dir).is_empty() {
        let _ = ensure_unpacked(data_dir);
        let _ = ensure_decompiled(data_dir, root);
    }

    // Generate the skeleton. The `translate` command is headless (returns without
    // launching the game window) and writes `game/tl/<lang>/`.
    let output = Command::new(&exe)
        .arg(root)
        .arg("translate")
        .arg(&lang)
        .output()
        .with_context(|| format!("running {} translate {lang}", exe.display()))?;
    if !output.status.success() {
        return Err(anyhow!(
            "the game's Ren'Py failed to generate translations: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let dir = data_dir.join("tl").join(&lang);
    if !dir.is_dir() {
        return Err(anyhow!(
            "Ren'Py did not generate {} — nothing translatable?",
            dir.display()
        ));
    }

    // Map each source string to its translation (first one wins for duplicates).
    let mut map: HashMap<&str, &str> = HashMap::new();
    for u in units {
        if u.status.is_applied() {
            if let Some(t) = &u.translation {
                map.entry(u.source.as_str()).or_insert(t.as_str());
            }
        }
    }
    // Track which sources the skeleton consumed: whatever remains of the Term
    // units afterwards has no skeleton entry (Ren'Py's scanner only collects
    // `_()` strings — bare screen literals and python display strings are
    // invisible to it) and is emitted as a strings block in zzz_translator.rpy.
    let used: std::cell::RefCell<HashSet<String>> = std::cell::RefCell::new(HashSet::new());
    let lookup = |s: &str| {
        map.get(s).map(|t| {
            used.borrow_mut().insert(s.to_string());
            t.to_string()
        })
    };

    let mut files = 0usize;
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|x| x.to_str()) == Some("rpy") {
                let content = std::fs::read_to_string(&p)?;
                let filled = renpy_tl::fill_tl(&content, &lookup);
                if filled != content {
                    std::fs::write(&p, filled).with_context(|| format!("writing {}", p.display()))?;
                }
                files += 1;
            }
        }
    }

    // Term units the skeleton never consumed (screen literals, python display
    // strings) → extra strings-block entries, deduped by source. Character names
    // ride the same block: a say-name goes through `substitute(who)` at display
    // time, which translates it like any other string. (Re-`define`ing the
    // Character in a `translate <lang> python:` block instead would work too, but
    // it *writes the store* — the Character then lands in every save file, and
    // pickling one whose `callback=` is an init-python function raises
    // `PicklingError: … not the same object as store.<fn>` when the player saves.)
    let mut extra_seen: HashSet<&str> = HashSet::new();
    let mut extra: Vec<(String, String)> = Vec::new();
    // `pylist#<var>#<index>` units: interpolated list words, applied by replacing the
    // store value rather than through the strings table.
    let mut lists: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
    {
        let used = used.borrow();
        for u in units {
            if !matches!(u.kind, UnitKind::Term | UnitKind::Name) || !u.status.is_applied() {
                continue;
            }
            // Keep character names in the source language when the toggle is off,
            // even if a prior Run had translated them.
            if !translate_names && u.kind == UnitKind::Name {
                continue;
            }
            let Some(t) = &u.translation else { continue };
            if let Some(rest) = u.pointer.strip_prefix("pylist#") {
                if let Some((var, idx)) = rest.split_once('#') {
                    if let Ok(i) = idx.parse::<usize>() {
                        lists.entry(var.to_string()).or_default().push((i, t.clone()));
                    }
                }
                continue;
            }
            if used.contains(u.source.as_str()) || !extra_seen.insert(u.source.as_str()) {
                continue;
            }
            extra.push((u.source.clone(), t.clone()));
        }
    }

    // Make the language selectable (default to it) and remap the game's fonts to a
    // glyph-capable one so the translation isn't rendered as "NO GLYPH" boxes.
    setup_language(data_dir, &lang, &language_label(target_lang, &lang), &extra, &lists)?;

    Ok(Some(TlExport { files, dir }))
}

/// The `tl/<lang>/` folder the units were extracted *from*, if any — their `file`
/// begins `tl/<lang>/…`. `None` for base-script units (the normal flow).
///
/// Only a **spliceable** unit (a byte-span pointer) counts: those come from
/// importing a game that ships a `tl/<en>/` source tree. Synthetic units with a
/// `str#`/`name#`/`pylist#` pointer are display-matched, and
/// [`harvest_tl_untranslated`] gives engine-UI units a `tl/<lang>/common.rpy` file —
/// they must NOT flip export into tl-source mode.
fn tl_source_lang(units: &[TransUnit]) -> Option<String> {
    units.iter().find_map(|u| {
        parse_pointer(&u.pointer)?; // spliceable byte span only
        let rest = u.file.strip_prefix("tl/")?;
        rest.split('/').next().map(str::to_string)
    })
}

/// Produce `tl/<target>/` from the `tl/<src>/` tree the units were read from: for each
/// source file, splice the translations into their byte spans, retag the column-0 block
/// headers `translate <src> …` → `translate <target> …`, and write it under the target
/// locale. Untranslated lines keep the source-language text (better than falling back
/// to the base language). The `tl/<src>/` tree is never modified, so re-export is
/// idempotent. No `renpy translate` / launcher is needed — the source tree already
/// carries the game's real translation ids.
fn export_tl_from_source(
    data_dir: &Path,
    src_lang: &str,
    units: &[TransUnit],
    target_lang: &str,
) -> Result<TlExport> {
    let lang = normalize_lang(target_lang);
    let src_header = format!("translate {src_lang} ");
    let dst_header = format!("translate {lang} ");
    let src_prefix = format!("tl/{src_lang}/");

    // Applied translations grouped by their source file.
    let mut by_file: BTreeMap<&str, Vec<&TransUnit>> = BTreeMap::new();
    for u in units {
        if u.status.is_applied() && u.translation.is_some() {
            by_file.entry(u.file.as_str()).or_default().push(u);
        }
    }

    let src_root = data_dir.join("tl").join(src_lang);
    let mut files = 0usize;
    for src_path in rpy_files_under(&src_root) {
        let rel = rel_path(data_dir, &src_path); // e.g. "tl/english/script.rpy"
        let mut bytes = std::fs::read(&src_path).with_context(|| format!("reading {rel}"))?;
        if let Some(us) = by_file.get(rel.as_str()) {
            let mut us = us.clone();
            // Splice from the end so earlier byte offsets stay valid.
            us.sort_by_key(|u| Reverse(parse_pointer(&u.pointer).map(|(s, _)| s).unwrap_or(0)));
            for u in us {
                let (start, len) = parse_pointer(&u.pointer)
                    .ok_or_else(|| anyhow!("bad Ren'Py tl pointer {} in {}", u.pointer, rel))?;
                if start + len > bytes.len() {
                    return Err(anyhow!("stale pointer {} in {} — re-extract needed", u.pointer, rel));
                }
                let tr = u.translation.clone().unwrap_or_default();
                // Escape literal `%` (→ `%%`) the way the source does: Ren'Py
                // `%`-substitutes say text, so a bare `%` (e.g. "50%") followed by a
                // letter crashes at runtime — but a source that itself carries a bare
                // `%` is a raw format string (strftime, screen label) the game consumes
                // as-is. `fill_tl` does the same for the non-source tl path.
                let tr = renpy_tl::escape_percent_like(&u.source, &renpy_tl::decode_escapes(&tr));
                bytes.splice(start..start + len, renpy_tl::quote_unicode(&tr).into_bytes());
            }
        }
        // Retag the column-0 `translate <src> …` block headers to the target locale.
        let content = String::from_utf8_lossy(&bytes);
        let mut retagged = String::with_capacity(content.len());
        for line in content.split_inclusive('\n') {
            if let Some(tail) = line.strip_prefix(src_header.as_str()) {
                retagged.push_str(&dst_header);
                retagged.push_str(tail);
            } else {
                retagged.push_str(line);
            }
        }
        // tl/<src>/x.rpy → tl/<target>/x.rpy
        let inner = rel.strip_prefix(src_prefix.as_str()).unwrap_or(&rel);
        let out_path = data_dir.join("tl").join(&lang).join(inner);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out_path, retagged)
            .with_context(|| format!("writing {}", out_path.display()))?;
        files += 1;
    }

    // Make the target selectable + readable (menu entry, default language, Thai font).
    // No Character re-defines: any `_()`-wrapped name translates via the strings blocks.
    // No extra strings either: tl-source units are all spliced into the retagged tree.
    setup_language(data_dir, &lang, &language_label(target_lang, &lang), &[], &BTreeMap::new())?;
    Ok(TlExport {
        files,
        dir: data_dir.join("tl").join(&lang),
    })
}

/// The button label for the language menu: the native name for known languages,
/// else the project's target-language string.
fn language_label(target_lang: &str, lang: &str) -> String {
    match lang {
        "thai" => "\u{e44}\u{e17}\u{e22}".to_string(), // ไทย
        _ => target_lang.to_string(),
    }
}

/// Add a `<label>` button to the game's language-selection screen (a
/// `textbutton "…" action Language(…)` block) so the translation can be chosen
/// from Settings. Idempotent, and touches only screen files (no version-sensitive
/// statements). No-op if the game has no such menu.
fn add_language_option(data_dir: &Path, lang: &str, label: &str) -> Result<()> {
    let already = format!("Language(\"{lang}\")");
    let mut stack = vec![data_dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().and_then(|n| n.to_str()) != Some("tl") {
                    stack.push(p);
                }
                continue;
            }
            if p.extension().and_then(|x| x.to_str()) != Some("rpy") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&p) else { continue };
            if !content.contains("action Language(") || content.contains(&already) {
                continue;
            }
            let lines: Vec<&str> = content.split_inclusive('\n').collect();
            let Some(idx) = lines.iter().rposition(|l| l.contains("action Language(")) else {
                continue;
            };
            let indent: String = lines[idx]
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let button = format!("{indent}textbutton \"{label}\" action Language(\"{lang}\")\n");

            let mut out = String::with_capacity(content.len() + button.len());
            for (i, l) in lines.iter().enumerate() {
                out.push_str(l);
                if i == idx {
                    if !l.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str(&button);
                }
            }
            std::fs::write(&p, out).with_context(|| format!("adding language button to {}", p.display()))?;
        }
    }
    Ok(())
}

/// Evaluate the escape sequences of a `.rpy` string literal's raw inner text
/// (the form stored as a unit's `source`), yielding the runtime string — which
/// is what a strings-block `old` line must match after Ren'Py evals it.
fn unescape_rpy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other), // \" \' \\ … → the char itself
            None => out.push('\\'),
        }
    }
    out
}

/// Write a small global `.rpy` that (1) defaults the game to `lang` if it has no
/// language of its own, (2) — when a target-language font is available —
/// remaps every font the game uses to it, scoped to `lang` via a
/// `translate <lang> python:` block so English is unaffected, and (3) emits a
/// `translate <lang> strings` block for display strings Ren'Py's own scanner
/// can't collect (bare screen literals, python-level quest names / notify text) —
/// every displayed string passes through `translate_string`, so these entries
/// translate the UI without touching python-side identity.
fn setup_language(
    data_dir: &Path,
    lang: &str,
    label: &str,
    strings: &[(String, String)],
    lists: &BTreeMap<String, Vec<(usize, String)>>,
) -> Result<()> {
    // Add the language to the game's own Settings language menu, if it has one.
    add_language_option(data_dir, lang, label)?;

    let mut s = String::new();
    s.push_str("# Added by RPGMaker Translator — makes the translation selectable + readable.\n");
    s.push_str("# Delete this file (and fonts/tl_font.ttf) to remove it.\n\n");
    // Default to the translation only if the game defines no language of its own.
    s.push_str(&format!(
        "init 1000 python:\n    if config.language is None:\n        config.language = \"{lang}\"\n\n"
    ));

    // Display strings with no skeleton entry, plus the character names (a say-name
    // is translated at display time by `substitute(who)`, so a strings entry renames
    // the speaker without touching the store — see the caller). `old` = the runtime
    // string (source unescaped, then re-escaped for the file); `new` gets the same
    // source-mirrored `%`-escaping as every other strings-block translation.
    if !strings.is_empty() {
        s.push_str(&format!("translate {lang} strings:\n"));
        for (old, new) in strings {
            let old = unescape_rpy(old);
            let new = renpy_tl::decode_escapes(new);
            s.push_str(&format!("    old \"{}\"\n", renpy_tl::quote_unicode(&old)));
            s.push_str(&format!(
                "    new \"{}\"\n\n",
                renpy_tl::quote_unicode(&renpy_tl::escape_percent_like(&old, &new))
            ));
        }

        // The same table again, as a `config.replace_text` hook. The strings block
        // above only fires on the string a statement *names*, and `translate_string`
        // runs **before** `[…]` interpolation — so a line like
        // `m "[renpy.random.choice(hesitation)]"`, whose text is picked at runtime
        // from a list built inside a label, is never translated by it. `replace_text`
        // runs on each TEXT token after interpolation, which is the only place that
        // value can still be caught. Values are unescaped here: `%`-substitution has
        // already happened by then. Installed at init (not under the language) so
        // switching languages can't chain the hook onto itself; the guard inside
        // keeps the source language untouched.
        s.push_str("init 1001 python:\n");
        s.push_str("    _tl_text = {\n");
        for (old, new) in strings {
            s.push_str(&format!(
                "        \"{}\": \"{}\",\n",
                renpy_tl::quote_unicode(&unescape_rpy(old)),
                renpy_tl::quote_unicode(&renpy_tl::decode_escapes(new))
            ));
        }
        s.push_str("    }\n");
        s.push_str("    _tl_prev_replace_text = config.replace_text\n");
        s.push_str("    def _tl_replace_text(_t):\n");
        s.push_str("        if _tl_prev_replace_text is not None:\n");
        s.push_str("            _t = _tl_prev_replace_text(_t)\n");
        s.push_str(&format!("        if config.language != \"{lang}\":\n"));
        s.push_str("            return _t\n");
        s.push_str("        return _tl_text.get(_t, _t)\n");
        s.push_str("    config.replace_text = _tl_replace_text\n\n");
    }

    // Interpolated word lists (`days_of_week`, `parts_of_day`). `"Day [n]
    // ([days_of_week[i]])"` is translated *before* interpolation, so the list value
    // itself must change — a strings entry never reaches it. Replace with a new list
    // (a copy, so the game's own `define`d object is left alone) under the language;
    // a list of strings pickles cleanly, so this is safe to leave in the store.
    if !lists.is_empty() {
        s.push_str(&format!("translate {lang} python:\n"));
        s.push_str("    _tl_lists = {\n");
        for (var, items) in lists {
            let mut items = items.clone();
            items.sort_by_key(|(i, _)| *i);
            s.push_str(&format!("        \"{var}\": {{"));
            for (i, t) in &items {
                s.push_str(&format!("{i}: \"{}\", ", renpy_tl::quote_unicode(&renpy_tl::decode_escapes(t))));
            }
            s.push_str("},\n");
        }
        s.push_str("    }\n");
        s.push_str("    for _v, _m in _tl_lists.items():\n");
        s.push_str("        try:\n");
        s.push_str("            _l = list(getattr(store, _v))\n");
        s.push_str("            for _i, _t in _m.items():\n");
        s.push_str("                if _i < len(_l):\n");
        s.push_str("                    _l[_i] = _t\n");
        s.push_str("            setattr(store, _v, _l)\n");
        s.push_str("        except Exception:\n");
        s.push_str("            pass\n\n");
    }

    // The bundled font (Sarabun) covers Thai + Latin, so only remap fonts for a
    // Thai target — other scripts (CJK, etc.) would render as NO GLYPH in it.
    if lang == "thai" {
        let font_rel = "fonts/tl_font.ttf";
        copy_target_font(&data_dir.join(font_rel))?;
        let refs = collect_font_refs(data_dir);

        // `_tl_font_group` MUST live in `init python`, not `translate python`.
        // Translate blocks re-execute on language change and save-load, creating a
        // new function object each time. When Ren'Py saves the game it pickles
        // `config.font_transforms["rpgtl_thai"]` (pointing at the old object), and
        // on load the new object fails identity check → PicklingError.  init python
        // runs once, so the function object is stable across save-roundtrips.
        s.push_str("init python:\n");
        s.push_str(&format!("    _tl_font = \"{font_rel}\"\n"));
        s.push_str("    _tl_groups = {}\n");
        s.push_str("    def _tl_font_group(_f):\n");
        s.push_str("        if not isinstance(_f, str):\n"); // ImageFont/FontGroup: leave alone
        s.push_str("            return _f\n");
        s.push_str("        _g = _tl_groups.get(_f)\n");
        s.push_str("        if _g is None:\n");
        s.push_str("            try:\n");
        s.push_str("                _g = FontGroup().add(_tl_font, 0x0e00, 0x0e7f).add(_f, None, None)\n");
        s.push_str("            except Exception:\n");
        s.push_str("                _g = _f\n");
        s.push_str("            _tl_groups[_f] = _g\n");
        s.push_str("        return _g\n");
        s.push_str("    if hasattr(config, \"font_transforms\"):\n");
        s.push_str("        config.font_transforms[\"rpgtl_thai\"] = _tl_font_group\n");
        // The bundled Thai face renders taller/heavier than the game's Latin font at
        // the same point size, so it crowds boxes laid out for English. Scale just the
        // Thai TTF down (keyed by its filename) — English glyphs keep scale 1.0, so
        // only Thai shrinks, game-wide. Set at init: English never uses this font.
        s.push_str("    if hasattr(config, \"ftfont_scale\"):\n");
        s.push_str("        config.ftfont_scale[_tl_font] = 0.9\n");
        s.push_str("\n");

        // Per-language activation + fallback stay in `translate python` where they
        // belong (only active when the language is Thai). The font-infra references
        // (_tl_font, _tl_groups, _tl_font_group) come from the init block above via
        // the Ren'Py store, which init-sets once and the translate block inherits.
        s.push_str(&format!("translate {lang} python:\n"));
        s.push_str("    _tl_fonts = [\n");
        for r in &refs {
            s.push_str(&format!("        {r:?},\n"));
        }
        s.push_str("    ]\n");
        // Preferred: a *glyph-level* fallback. `config.font_transforms` (Ren'Py 8.1+)
        // runs on the font of every text segment — style fonts and inline `{font=…}`
        // alike — and, unlike `config.font_replacement_map`, may return a FontGroup.
        // So Thai code points come from the bundled face and every other glyph keeps
        // the game's own font: symbols the Thai face lacks (⚫ U+26AB in a phone
        // typing indicator, ♡) render instead of turning into tofu boxes.
        s.push_str("    if hasattr(config, \"font_transforms\"):\n");
        s.push_str("        preferences.font_transform = \"rpgtl_thai\"\n");
        // Fallback for older Ren'Py, which has no font transform: swap the font file
        // outright. Whole-face, so glyphs missing from the Thai face become tofu.
        s.push_str("    else:\n");
        s.push_str("        for _f in _tl_fonts:\n");
        s.push_str("            for _b in (False, True):\n");
        s.push_str("                for _i in (False, True):\n");
        s.push_str("                    config.font_replacement_map[_f, _b, _i] = (_tl_font, _b, _i)\n");
        // Thai has no spaces, so Ren'Py's default (space-based) line breaking can't
        // wrap a Thai run — a long line overflows its box / screen edge instead of
        // wrapping (quest objectives ran off the phone screen). `"anywhere"` lets a
        // break fall between any two characters, so every Thai text fits its width.
        // Set on the base style, under the language, so all screens + dialogue inherit
        // it only while Thai is active. (No dictionary word-breaking in this Ren'Py;
        // "anywhere" is the widely-used Thai fallback.)
        s.push_str("    style.default.language = \"anywhere\"\n");
        // Thai dialogue crowds a say box sized for Latin even after the 0.9 font scale,
        // so drop the dialogue point size ~12%. Derive it from the game's own
        // `gui.text_size` (the stable base, never mutated) rather than the live style
        // size, so re-running this block on a language switch recomputes the same value
        // instead of compounding. Guarded: not every game defines gui.text_size.
        s.push_str("    if getattr(getattr(store, \"gui\", None), \"text_size\", None):\n");
        s.push_str("        style.say_dialogue.size = int(round(gui.text_size * 0.88))\n");
    }

    std::fs::write(data_dir.join(GENERATED_RPY), s)
        .with_context(|| "writing zzz_translator.rpy")?;
    Ok(())
}

/// The bundled target-language font (Sarabun Regular), shared with the other
/// engines' font embedding — see [`super::TARGET_FONT`].
const TL_FONT: &[u8] = super::TARGET_FONT;

/// Write the bundled font into the game.
fn copy_target_font(dst: &Path) -> Result<()> {
    // Already in place with the same bytes (a re-export) — skip the write. Besides
    // saving work, this keeps a re-export from failing when the game is *running* and
    // holding the font open (a Windows sharing violation): the font is already there,
    // so there's nothing to do.
    if std::fs::read(dst).is_ok_and(|b| b == TL_FONT) {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dst, TL_FONT).with_context(|| {
        format!(
            "writing the Thai font {} — if the game is open, close it and export again",
            dst.display()
        )
    })?;
    Ok(())
}

/// Every font the game references, so they can all be remapped to the target-language
/// font: both the `.ttf`/`.otf`/`.ttc` files present (path relative to the data dir)
/// and any font paths quoted in `.rpy` scripts. The `tl/` tree is skipped.
fn collect_font_refs(data_dir: &Path) -> Vec<String> {
    let mut refs = std::collections::BTreeSet::new();
    let mut stack = vec![data_dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().and_then(|n| n.to_str()) != Some("tl") {
                    stack.push(p);
                }
                continue;
            }
            match p.extension().and_then(|x| x.to_str()) {
                Some("ttf") | Some("otf") | Some("ttc") => {
                    if let Ok(rel) = p.strip_prefix(data_dir) {
                        refs.insert(rel.to_string_lossy().replace('\\', "/"));
                    }
                }
                Some("rpy") => {
                    if let Ok(txt) = std::fs::read_to_string(&p) {
                        for r in font_strings(&txt) {
                            refs.insert(r);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // Don't remap our own font onto itself.
    refs.remove("fonts/tl_font.ttf");
    // Drop build globs like "game/**.ttf" (from a `build.classify` line) — they're not
    // real font names, so remapping them is dead weight.
    refs.into_iter()
        .filter(|r| !r.contains('*') && !r.contains('?'))
        .collect()
}

/// Quoted font paths (`"…​.ttf"` / `.otf` / `.ttc`) referenced in a `.rpy` script.
fn font_strings(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' || b[i] == b'\'' {
            let q = b[i];
            let start = i + 1;
            let mut j = start;
            while j < b.len() && b[j] != q && b[j] != b'\n' {
                j += 1;
            }
            if j < b.len() && b[j] == q {
                let inner = &text[start..j];
                let low = inner.to_ascii_lowercase();
                if low.ends_with(".ttf") || low.ends_with(".otf") || low.ends_with(".ttc") {
                    // Ren'Py font paths are relative to the game dir; drop a leading `/`.
                    out.push(inner.trim_start_matches('/').to_string());
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// The game's bundled Ren'Py launcher: a `<name>.exe` at the bundle root next to a
/// matching `<name>.py`, with a `renpy/` directory present. Running
/// `<exe> <root> translate <lang>` drives Ren'Py's own translation generation.
fn find_launcher(root: &Path) -> Option<PathBuf> {
    if !root.join("renpy").is_dir() {
        return None;
    }
    for e in std::fs::read_dir(root).ok()?.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("exe") {
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                if root.join(format!("{stem}.py")).exists() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// A Ren'Py `tl/` language directory name: lowercase ASCII letters/digits/`_`.
fn normalize_lang(lang: &str) -> String {
    let s: String = lang
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect::<String>()
        .to_lowercase();
    if s.is_empty() {
        "translated".to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(src: &str) -> Vec<TransUnit> {
        let mut out = Vec::new();
        let mut py_seen = HashSet::new();
        extract_rpy("script.rpy", src, &mut out, &mut py_seen);
        out
    }

    #[test]
    fn harvests_untranslated_engine_strings_from_the_skeleton() {
        let d = tempfile::tempdir().unwrap();
        let tl = d.path().join("tl").join("thai");
        std::fs::create_dir_all(&tl).unwrap();
        std::fs::write(
            tl.join("common.rpy"),
            "translate thai strings:\n\
             \n    old \"Are you sure you want to quit?\"\n    new \"Are you sure you want to quit?\"\n\
             \n    old \"Start\"\n    new \"\u{e40}\u{e23}\u{e34}\u{e48}\u{e21}\"\n\
             \n    old \"Already have unit\"\n    new \"Already have unit\"\n",
        )
        .unwrap();

        // A game already has a unit for "Already have unit" — don't duplicate it.
        let existing = vec![TransUnit::new("script.rpy", "1:1", UnitKind::Term, "Already have unit")];
        let got = harvest_tl_untranslated(&tl, &existing);

        let sources: Vec<&str> = got.iter().map(|u| u.source.as_str()).collect();
        assert_eq!(sources, vec!["Are you sure you want to quit?"], "{got:?}");
        let u = &got[0];
        assert_eq!(u.file, "tl/thai/common.rpy");
        assert!(u.pointer.starts_with("str#"), "display-matched, not spliced: {}", u.pointer);
        // A harvested tl/ unit must never flip a re-export into tl-source mode.
        assert_eq!(tl_source_lang(&got), None);
    }

    #[test]
    fn export_tl_from_source_retags_and_splices() {
        use crate::model::Status;
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        let ten = game.join("tl").join("english");
        std::fs::create_dir_all(&ten).unwrap();
        let src = "translate english a1:\n    e \"Hello\"\n\ntranslate english strings:\n    old \"X\"\n    new \"World\"\n";
        std::fs::write(ten.join("script.rpy"), src).unwrap();

        let mut units = Vec::new();
        extract_from_tl("tl/english/script.rpy", "english", src, &mut units);
        for u in &mut units {
            u.translation = Some(format!("T:{}", u.source));
            u.status = Status::Translated;
        }
        let out = export_tl_from_source(&game, "english", &units, "Thai").unwrap();
        assert!(out.files >= 1);

        let thai = std::fs::read_to_string(game.join("tl").join("thai").join("script.rpy")).unwrap();
        assert!(thai.contains("translate thai a1:"), "retagged dialogue: {thai}");
        assert!(thai.contains("translate thai strings:"), "retagged strings");
        assert!(thai.contains("\"T:Hello\""), "dialogue spliced");
        assert!(thai.contains("\"T:World\""), "strings `new` spliced");
        assert!(thai.contains("old \"X\""), "the `old` key is preserved");
        assert!(!thai.contains("translate english"), "no source tag left");
        // The source tree is never modified (idempotent re-export).
        assert_eq!(std::fs::read_to_string(ten.join("script.rpy")).unwrap(), src);
    }

    #[test]
    fn export_tl_from_source_escapes_literal_percent() {
        use crate::model::Status;
        // A translation with a literal `%` must be written as `%%`: Ren'Py runs a say
        // string through `%`-substitution, so a bare `%` before a letter crashes at
        // runtime ("unsupported format character"). Regression: the tl-source splice
        // path used to skip the escaping that `fill_tl` applies.
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        let ten = game.join("tl").join("english");
        std::fs::create_dir_all(&ten).unwrap();
        let src = "translate english a1:\n    e \"Discount\"\n";
        std::fs::write(ten.join("script.rpy"), src).unwrap();

        let mut units = Vec::new();
        extract_from_tl("tl/english/script.rpy", "english", src, &mut units);
        for u in &mut units {
            u.translation = Some("ลด 50% วันนี้".to_string());
            u.status = Status::Translated;
        }
        export_tl_from_source(&game, "english", &units, "Thai").unwrap();

        let thai = std::fs::read_to_string(game.join("tl").join("thai").join("script.rpy")).unwrap();
        assert!(thai.contains("50%%"), "literal percent doubled: {thai}");
        assert!(!thai.contains("50% "), "no bare `% ` left that Ren'Py would misread");
    }

    #[test]
    fn extract_from_tl_reads_english_dialogue_and_strings() {
        // A tl/english/ file: dialogue blocks (say line = English, the `#` original and
        // the `<id>` are left alone) + a strings block (each `new` = English source).
        let src = r#"translate english start_1a2b:
    # e "Привет"
    e "Hello there"

translate english start_3c4d:
    "Narration line."

translate english strings:
    # game/script.rpy:10
    old "Виктория"
    new "Victoria"

translate english python:
    e = Character("x")
"#;
        let mut out = Vec::new();
        extract_from_tl("tl/english/script.rpy", "english", src, &mut out);

        let sources: Vec<&str> = out.iter().map(|u| u.source.as_str()).collect();
        assert_eq!(sources, vec!["Hello there", "Narration line.", "Victoria"]);
        // No Cyrillic (the `old`/comment original) and no python-block code leaked in.
        assert!(!sources.iter().any(|s| s.contains('В') || s.contains("Character")));
        // Speaker captured from the say prefix; kinds correct.
        assert_eq!(out[0].context.as_deref(), Some("e"));
        assert_eq!(out[0].kind, UnitKind::Dialogue);
        assert_eq!(out[1].context, None); // narration has no speaker
        assert_eq!(out[2].kind, UnitKind::Term); // a strings entry
        // Byte spans are exact: content[span] == the source string.
        for u in &out {
            let (s, l) = parse_pointer(&u.pointer).unwrap();
            assert_eq!(&src[s..s + l], u.source);
        }
    }

    #[test]
    fn extracts_say_menu_and_skips_code() {
        let src = r#"
define e = Character("Eileen")

label start:
    "Narration line."
    e "Hello there."
    e happy "With attributes." with vpunch
    voice "audio/v1.ogg"
    menu:
        "Pick one?"
        "First choice":
            e "You picked first."
        "Second choice" if points > 3:
            pass

screen hud():
    text "This is UI, not dialogue."

init python:
    x = "code string"
"#;
        let units = extract(src);
        let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();

        assert!(texts.contains(&"Narration line."));
        assert!(texts.contains(&"Hello there."));
        assert!(texts.contains(&"With attributes."));
        assert!(texts.contains(&"Pick one?"));
        assert!(texts.contains(&"First choice"));
        assert!(texts.contains(&"Second choice"));
        assert!(texts.contains(&"You picked first."));

        // Code / asset strings must NOT be extracted.
        assert!(!texts.contains(&"audio/v1.ogg"));
        assert!(!texts.iter().any(|t| t.contains("Eileen")));

        // Bare screen literals and multi-word python strings ARE harvested now
        // (as Term units, translated via the strings table at display time).
        let ui = units.iter().find(|u| u.source == "This is UI, not dialogue.").unwrap();
        assert_eq!(ui.kind, UnitKind::Term);
        let code = units.iter().find(|u| u.source == "code string").unwrap();
        assert_eq!(code.kind, UnitKind::Term);
        assert!(code.pointer.starts_with("str#"), "python strings are display-matched, not spliced");
    }

    #[test]
    fn init_prefixed_screen_and_style_blocks_are_skipped() {
        // `init [priority] screen/style/...:` bodies are screen-language / style
        // props, not dialogue. Regression: MilfyCity's screens.rpy uses this form,
        // and asset paths + property kwargs were being mis-extracted as Dialogue.
        let src = r#"
init -1 style frame:
    background Frame("gui/frame.png", gui.frame_borders, tile=gui.frame_tile)

init -1 style input:
    properties gui.text_properties("input", accent=True)

init -501 screen Log_scr():
    tag menu
    add "main_menu"
    text "who" id "who"

label start:
    e "Real dialogue."
"#;
        let units = extract(src);
        let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();

        assert!(texts.contains(&"Real dialogue."), "kept real dialogue");
        for junk in ["gui/frame.png", "input", "main_menu", "who"] {
            assert!(!texts.contains(&junk), "must skip screen/style junk: {junk}");
        }
    }

    #[test]
    fn speaker_and_kind_are_classified() {
        let units = extract("    e \"Hi.\"\n    \"Narr.\"\n    \"Choice\":\n");
        assert_eq!(units[0].kind, UnitKind::Dialogue);
        assert_eq!(units[0].context.as_deref(), Some("e"));
        assert_eq!(units[1].kind, UnitKind::Dialogue);
        assert_eq!(units[1].context, None); // narrator
        assert_eq!(units[2].kind, UnitKind::Choice);
        assert_eq!(units[2].context, None);
    }

    #[test]
    fn pointer_spans_the_inner_content() {
        let src = "    e \"Hello.\"\n";
        let units = extract(src);
        let (start, len) = parse_pointer(&units[0].pointer).unwrap();
        assert_eq!(&src[start..start + len], "Hello.");
    }

    #[test]
    fn gettext_strings_extracted_even_in_screens() {
        let src = r#"
screen main_menu():
    textbutton _("Start Game") action Start()
    textbutton "Unwrapped" action NullAction()
    text _("Options")

label x:
    $ renpy.notify(_("Progress saved."))
"#;
        let units = extract(src);
        let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
        assert!(texts.contains(&"Start Game"));
        assert!(texts.contains(&"Options"));
        assert!(texts.contains(&"Progress saved."));
        // Unwrapped screen text (no _()) is harvested too, as a spliceable Term.
        let unwrapped = units.iter().find(|u| u.source == "Unwrapped").unwrap();
        assert_eq!(unwrapped.kind, UnitKind::Term);
        assert!(!unwrapped.pointer.starts_with("str#"), "screen literals keep a byte span");
        assert_eq!(
            units.iter().find(|u| u.source == "Start Game").unwrap().kind,
            UnitKind::Term
        );
    }

    #[test]
    fn two_argument_say_extracts_dialogue_not_speaker() {
        // `"Speaker" "dialogue"` — extract the line, keep the name as context.
        let units = extract("    \"Sylvie\" \"Hi there!\"\n    \"Me\" \"Let's go.\"\n");
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].source, "Hi there!");
        assert_eq!(units[0].context.as_deref(), Some("Sylvie"));
        assert_eq!(units[0].kind, UnitKind::Dialogue);
        assert_eq!(units[1].source, "Let's go.");
        assert_eq!(units[1].context.as_deref(), Some("Me"));
        // The speaker names must not be extracted as their own units.
        assert!(!units.iter().any(|u| u.source == "Sylvie" || u.source == "Me"));
    }

    #[test]
    fn gettext_say_is_not_double_counted() {
        // `e _("Hi")` yields exactly one unit, not one for the say and one for _().
        let units = extract("    e _(\"Hi\")\n");
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].source, "Hi");
    }

    #[test]
    fn multiline_define_body_skips_bare_strings_keeps_gettext() {
        let src = "\
define ay = Character(
    _(\"Ayumi\"),
    what_color=\"#fff\",
)

default akane_data = {
    \"name\": _(\"Akane\"),
    \"relation\": _(\"step dad\"),
    \"hair_color\": \"#a83\",
    \"portrait\": \"images/akane.png\",
}

label start:
    ay \"Nice to meet you.\"
";
        let units = extract(src);
        let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
        // `_()` strings inside the multi-line data are still harvested.
        assert!(texts.contains(&"Ayumi"));
        assert!(texts.contains(&"Akane"));
        assert!(texts.contains(&"step dad"));
        // Bare colour / asset / dict-key strings in the body are not extracted.
        assert!(!texts.contains(&"#fff"));
        assert!(!texts.contains(&"#a83"));
        assert!(!texts.contains(&"images/akane.png"));
        assert!(!texts.iter().any(|t| t.contains("hair_color")));
        // Normal dialogue after the block still works.
        assert!(texts.contains(&"Nice to meet you."));
    }

    #[test]
    fn font_strings_finds_quoted_font_paths() {
        let src = "define gui.text_font = \"gui/fonts/Dialog Regular.ttf\"\n\
                   define x = \"/gui/fonts/Title.otf\"\n\
                   text \"not a font\"\n\
                   font \"phone/JetBrains.TTC\"\n";
        let mut got = font_strings(src);
        got.sort();
        assert_eq!(
            got,
            vec![
                "gui/fonts/Dialog Regular.ttf".to_string(),
                "gui/fonts/Title.otf".to_string(), // leading slash dropped
                "phone/JetBrains.TTC".to_string(), // case-insensitive extension
            ]
        );
    }

    #[test]
    fn add_language_button_after_last_and_idempotent() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::write(
            root.join("screens.rpy"),
            "    vbox:\n        textbutton \"English\" action Language(None)\n        textbutton \"Espanol\" action Language(\"spanish\")\n",
        )
        .unwrap();

        add_language_option(root, "thai", "\u{e44}\u{e17}\u{e22}").unwrap();
        let c = std::fs::read_to_string(root.join("screens.rpy")).unwrap();
        // Added after the last existing button, with the same 8-space indent.
        assert!(c.contains("        textbutton \"\u{e44}\u{e17}\u{e22}\" action Language(\"thai\")"));

        // Re-running does not duplicate it.
        add_language_option(root, "thai", "\u{e44}\u{e17}\u{e22}").unwrap();
        let c2 = std::fs::read_to_string(root.join("screens.rpy")).unwrap();
        assert_eq!(c2.matches("Language(\"thai\")").count(), 1);
    }

    #[test]
    fn normalize_lang_is_lowercase_ascii() {
        assert_eq!(normalize_lang("Thai"), "thai");
        assert_eq!(normalize_lang("thai"), "thai");
        assert_eq!(normalize_lang("Brazilian Portuguese"), "brazilianportuguese");
        assert_eq!(normalize_lang("\u{e44}\u{e17}\u{e22}"), "translated"); // non-ASCII -> fallback
    }

    #[test]
    fn stale_companions_maps_rpy_to_rpyc() {
        let eng = RenpyEngine;
        assert_eq!(eng.stale_companions("script.rpy"), vec!["script.rpyc".to_string()]);
        assert_eq!(
            eng.stale_companions("scripts/ch1.rpy"),
            vec!["scripts/ch1.rpyc".to_string()]
        );
        assert!(eng.stale_companions("notes.txt").is_empty());
    }

    #[test]
    fn find_bundled_python_prefers_host_64bit_and_reads_major() {
        // Build lib dirs for the *host* OS so this runs on any CI platform.
        let os_tok = match std::env::consts::OS {
            "windows" => "windows",
            "macos" => "mac",
            _ => "linux",
        };
        let exe = if cfg!(windows) { "python.exe" } else { "python" };

        // Ren'Py 7 layout: a bare `<os>-<arch>` dir (no py-prefix) with a
        // libpython2.7 runtime → Py2; the 64-bit build wins over the 32-bit one.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let p64 = root.join("lib").join(format!("{os_tok}-x86_64"));
        let p32 = root.join("lib").join(format!("{os_tok}-i686"));
        for d in [&p64, &p32] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join(exe), b"").unwrap();
            std::fs::write(d.join("libpython2.7.dll"), b"").unwrap();
        }
        let (found, major) = find_bundled_python(root).expect("interpreter found");
        assert_eq!(found, p64.join(exe), "prefers the 64-bit build");
        assert_eq!(major, PyMajor::Py2);

        // Ren'Py 8 layout: a `py3-<os>-<arch>` prefix → Py3.
        let tmp3 = tempfile::tempdir().unwrap();
        let d3 = tmp3.path().join("lib").join(format!("py3-{os_tok}-x86_64"));
        std::fs::create_dir_all(&d3).unwrap();
        std::fs::write(d3.join(exe), b"").unwrap();
        let (_e, m3) = find_bundled_python(tmp3.path()).expect("py3 interpreter found");
        assert_eq!(m3, PyMajor::Py3);

        // No lib/ (or no runnable interpreter) → None, so extract degrades to the
        // actionable error rather than trying to spawn nothing.
        let empty = tempfile::tempdir().unwrap();
        assert!(find_bundled_python(empty.path()).is_none());
    }

    #[test]
    fn needs_decompile_flags_loose_rpyc_without_source() {
        // Regression (Summertime Saga): a game ships a loose `splash.rpy` yet keeps
        // the bulk of its story as `.rpyc`, so gating decompile on "no .rpy at all"
        // left hundreds of scripts unread. needs_decompile must fire on a `.rpyc`
        // that has no `.rpy` sibling, even when other `.rpy` are present.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("splash.rpy"), b"\"hi\"").unwrap(); // a loose source
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/story.rpyc"), b"x").unwrap(); // compiled, no source
        assert!(needs_decompile(dir), "undecompiled .rpyc must be detected");

        // Once its `.rpy` exists, there's nothing left to decompile → idempotent.
        std::fs::write(dir.join("src/story.rpy"), b"\"hi\"").unwrap();
        assert!(!needs_decompile(dir));

        // `.rpyc` under `tl/` are translations, not game source → ignored.
        std::fs::create_dir_all(dir.join("tl/fr")).unwrap();
        std::fs::write(dir.join("tl/fr/story.rpyc"), b"x").unwrap();
        assert!(!needs_decompile(dir));
    }

    #[test]
    fn control_flow_condition_strings_are_not_choices() {
        // Regression: strings inside `if`/`elif`/`while` conditions (which end in
        // `:`) were extracted as menu choices and translated, corrupting dict keys
        // and comparisons (KeyError at runtime).
        let src = "\
label bgm:
    if selected_dest == \"office\" and current_time in [\"morning\", \"noon\"]:
        play music \"a.ogg\"
    elif selected_dest in areas[\"House\"]:
        play music \"b.ogg\"
    menu:
        \"Go home\":
            jump home
";
        let units = extract(src);
        let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
        // Code strings in conditions are NOT extracted.
        assert!(!texts.contains(&"office"));
        assert!(!texts.contains(&"House"));
        assert!(!texts.contains(&"morning"));
        // A real menu choice (string at the start of the line) still is.
        assert!(texts.contains(&"Go home"));
        assert_eq!(
            units.iter().find(|u| u.source == "Go home").unwrap().kind,
            UnitKind::Choice
        );
    }

    #[test]
    fn init_priority_python_block_skips_code_strings() {
        // Regression: a `style="empty"` kwarg inside `init -100 python in …:` was
        // extracted and translated, renaming the style and crashing Ren'Py.
        let src = "\
init -100 python in phone.application:
    def Icon(d):
        rv = Fixed(bg, d, style=\"empty\", xysize=(10, 10))
        note = _(\"Messages\")
        return rv

init 5 python:
    x = \"raw_code_string\"

label start:
    e \"Real dialogue.\"
";
        let units = extract(src);
        let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
        // Code strings inside the priority-init python blocks are NOT extracted.
        assert!(!texts.contains(&"empty"), "style name must not be translated");
        assert!(!texts.contains(&"raw_code_string"));
        // A `_()`-wrapped string inside the block is still translatable.
        assert!(texts.contains(&"Messages"));
        // Normal dialogue outside the blocks still works.
        assert!(texts.contains(&"Real dialogue."));
    }

    #[test]
    fn escaped_quotes_are_handled() {
        let src = "    e \"She said \\\"hi\\\" softly.\"\n";
        let units = extract(src);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].source, "She said \\\"hi\\\" softly.");
        let (start, len) = parse_pointer(&units[0].pointer).unwrap();
        assert_eq!(&src[start..start + len], "She said \\\"hi\\\" softly.");
    }

    #[test]
    fn extracts_character_names_literal_and_var_and_skips_the_rest() {
        let defs = "\
init python:
    rin_name = \"\u{308a}\u{3093}\"
    p_name=\"\u{7fd4}\u{592a}\"
    unused = \"nope\"
define rin = Character(rin_name, color=\"#FF69B4\")
define p = Character(p_name)
define e = Character(\"Eileen\")
define narrator = Character(None)
define g = Character(_(\"Gwen\"))
";
        let files = vec![("01_definitions.rpy".to_string(), defs.to_string())];
        let mut units = Vec::new();
        extract_character_names(&files, &mut units);

        let got: std::collections::HashMap<&str, &str> = units
            .iter()
            .map(|u| (u.context.as_deref().unwrap(), u.source.as_str()))
            .collect();
        assert_eq!(got.get("rin"), Some(&"\u{308a}\u{3093}")); // via name variable
        assert_eq!(got.get("p"), Some(&"\u{7fd4}\u{592a}"));
        assert_eq!(got.get("e"), Some(&"Eileen")); // literal
        assert!(!got.contains_key("narrator")); // Character(None)
        assert!(!got.contains_key("g")); // Character(_()) — strings path
        // A string var not used by any Character isn't emitted.
        assert!(!units.iter().any(|u| u.source == "nope"));
        assert!(units.iter().all(|u| u.kind == UnitKind::Name));
        assert!(units.iter().all(|u| u.pointer.starts_with("name#")));
    }

    #[test]
    fn setup_language_translates_character_names_as_strings() {
        let d = tempfile::tempdir().unwrap();
        // A name rides the strings block like any other string — `substitute(who)`
        // translates the say-name at display time. Never a `translate python` re-define:
        // that writes the store, so the Character lands in saves and pickling one whose
        // `callback=` is an init-python function crashes the save.
        let names = vec![("Rin".to_string(), "\u{e23}\u{e34}\u{e19}".to_string())];
        setup_language(d.path(), "thai", "\u{e44}\u{e17}\u{e22}", &names, &BTreeMap::new()).unwrap();
        let zzz = std::fs::read_to_string(d.path().join(GENERATED_RPY)).unwrap();
        assert!(zzz.contains("    old \"Rin\"\n    new \"\u{e23}\u{e34}\u{e19}\""), "{zzz}");
        assert!(!zzz.contains("kind=rin"), "no Character re-define: {zzz}");
    }

    #[test]
    fn harvests_screen_tooltip_actions_and_interpolated_word_lists() {
        // NothingWeirdHappensHere: the tooltip never reached the AI (a screen action
        // argument), and the weekday/time words render through interpolation, which
        // runs after `translate_string` — so they need the store value replaced.
        let src = concat!(
            "define days_of_week = [\"Sunday\", \"Monday\", \"Tuesday\"]\n",
            "define parts_of_day = [\"morning\", \"late night\"]\n",
            "define doors = [\"button/door.png\", \"door h.png\"]\n",
            "screen university():\n",
            "    imagebutton:\n",
            "        idle \"button/p map.png\"\n",
            "        hovered SetVariable(\"tooltip_text\", \"Now is not the time.\")\n",
            "        unhovered SetVariable(\"tooltip_text\", \"\")\n",
        );
        let mut units = Vec::new();
        extract_rpy("navigation.rpy", src, &mut units, &mut HashSet::new());

        let tip = units.iter().find(|u| u.source == "Now is not the time.");
        assert!(tip.is_some(), "screen tooltip harvested: {units:?}");
        // The variable name (no whitespace) and the empty reset string stay out.
        assert!(!units.iter().any(|u| u.source == "tooltip_text"));

        let list: Vec<(&str, &str)> = units
            .iter()
            .filter(|u| u.pointer.starts_with("pylist#"))
            .map(|u| (u.pointer.as_str(), u.source.as_str()))
            .collect();
        assert!(list.contains(&("pylist#days_of_week#1", "Monday")), "{list:?}");
        assert!(list.contains(&("pylist#parts_of_day#1", "late night")), "{list:?}");
        // Asset lists are not display text — a path (`/`) or a bare filename.
        assert!(!list.iter().any(|(p, _)| p.starts_with("pylist#doors#")), "{list:?}");
    }

    #[test]
    fn setup_language_replaces_interpolated_word_lists() {
        let d = tempfile::tempdir().unwrap();
        let mut lists = BTreeMap::new();
        lists.insert("days_of_week".to_string(), vec![(1usize, "จันทร์".to_string())]);
        setup_language(d.path(), "thai", "ไทย", &[], &lists).unwrap();
        let zzz = std::fs::read_to_string(d.path().join(GENERATED_RPY)).unwrap();
        assert!(zzz.contains("\"days_of_week\": {1: \"จันทร์\", },"), "{zzz}");
        assert!(zzz.contains("setattr(store, _v, _l)"), "{zzz}");
    }

    #[test]
    fn setup_language_installs_a_glyph_level_font_fallback() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("gui")).unwrap();
        std::fs::write(d.path().join("gui/game.ttf"), b"fake").unwrap();
        setup_language(d.path(), "thai", "ไทย", &[], &BTreeMap::new()).unwrap();
        let zzz = std::fs::read_to_string(d.path().join(GENERATED_RPY)).unwrap();
        // Thai code points come from the bundled face, every other glyph from the
        // game's own font — a whole-face swap turns symbols like ⚫ into tofu.
        assert!(zzz.contains("FontGroup().add(_tl_font, 0x0e00, 0x0e7f).add(_f, None, None)"), "{zzz}");
        assert!(zzz.contains("preferences.font_transform = \"rpgtl_thai\""), "{zzz}");
        // Older Ren'Py has no font transform — the whole-face swap stays as fallback.
        assert!(zzz.contains("config.font_replacement_map[_f, _b, _i]"), "{zzz}");
    }

    #[test]
    fn setup_language_writes_extra_strings_block() {
        let d = tempfile::tempdir().unwrap();
        let strings = vec![
            ("Go to University".to_string(), "ไปมหาวิทยาลัย".to_string()),
            // Source with an escaped quote: `old` must carry the runtime string,
            // re-escaped for the file. Translation with a literal `%` → doubled.
            ("Say \\\"hi\\\"".to_string(), "ลด 50% เลย".to_string()),
            // A source that carries a bare `%` is a raw format string the game feeds to
            // strftime — the translation must keep its `%` single.
            ("%m/%d/%Y".to_string(), "%d/%m/%Y".to_string()),
        ];
        setup_language(d.path(), "thai", "ไทย", &strings, &BTreeMap::new()).unwrap();
        let zzz = std::fs::read_to_string(d.path().join(GENERATED_RPY)).unwrap();
        assert!(zzz.contains("translate thai strings:"), "strings block present: {zzz}");
        assert!(zzz.contains("    old \"Go to University\"\n    new \"ไปมหาวิทยาลัย\""));
        assert!(zzz.contains("old \"Say \\\"hi\\\"\""), "escapes normalized: {zzz}");
        // Same table as a post-interpolation hook, so a line whose text is picked at
        // runtime (`m "[renpy.random.choice(hesitation)]"`) still translates.
        assert!(zzz.contains("config.replace_text = _tl_replace_text"), "{zzz}");
        assert!(
            zzz.contains("\"Go to University\": \"ไปมหาวิทยาลัย\","),
            "hook table carries the unescaped translation: {zzz}"
        );
        assert!(zzz.contains("50%%"), "literal percent doubled in `new`");
        assert!(
            zzz.contains("    old \"%m/%d/%Y\"\n    new \"%d/%m/%Y\""),
            "strftime format kept single-`%`: {zzz}"
        );
    }

    #[test]
    fn harvests_python_quest_strings_and_dedupes() {
        // The Nothing-Weird-Happens-Here case: quest names/objectives are bare
        // python strings, shown later via `text _qn` — extract them once each as
        // display-matched Terms; keys/paths/colors stay out.
        let src = r##"
label quests:
    $ go_to_university = Quest("Go to University", "Start your new life as a student.")
    $ go_to_university.add_objective("Wait for Monday to start studying", completed=False)
    $ uni = find_quest("Go to University")
    $ renpy.notify("Saved")
    $ renpy.notify("✓ Objective complete: Find your classroom")
    $ event_done["aunt_housework_done"] = True
    $ color = "#ffe066"
    $ portrait = "images/quests/uni.png"
"##;
        let units = extract(src);
        let terms: Vec<&TransUnit> = units.iter().filter(|u| u.kind == UnitKind::Term).collect();
        let texts: Vec<&str> = terms.iter().map(|u| u.source.as_str()).collect();

        assert!(texts.contains(&"Go to University"));
        assert!(texts.contains(&"Start your new life as a student."));
        assert!(texts.contains(&"Wait for Monday to start studying"));
        assert!(texts.contains(&"Saved"), "notify single-word is display text");
        assert!(texts.contains(&"✓ Objective complete: Find your classroom"));
        // Deduped: the find_quest() repeat adds no second unit.
        assert_eq!(texts.iter().filter(|t| **t == "Go to University").count(), 1);
        // Keys / colors / paths stay out.
        assert!(!texts.iter().any(|t| t.contains("aunt_housework")));
        assert!(!texts.contains(&"#ffe066"));
        assert!(!texts.iter().any(|t| t.contains("images/")));
        // All python strings are display-matched (`str#`), never spliced.
        assert!(terms.iter().all(|u| u.pointer.starts_with("str#")));
    }

    #[test]
    fn harvests_bare_screen_literals() {
        let src = r##"
screen quest_log():
    modal True
    add "bg phone 2.png"
    key "dismiss" action Hide("quest_log")
    text "Quest Log":
        size 30
        color "#7daad4"
    text "›":
        size 30
    textbutton "Close" action Hide("quest_log")
    label "Objectives"
    text _qn
    text scene["name"]:
        size 26
    text prompt style "input_prompt"
    $ _bg = "#2c3e6618"
"##;
        let units = extract(src);
        let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();

        assert!(texts.contains(&"Quest Log"));
        assert!(texts.contains(&"Close"));
        assert!(texts.contains(&"Objectives"));
        // Punctuation-only literal, asset path, key name, colors: all out.
        assert!(!texts.contains(&"›"));
        assert!(!texts.contains(&"bg phone 2.png"));
        assert!(!texts.contains(&"dismiss"));
        assert!(!texts.contains(&"quest_log"));
        assert!(!texts.iter().any(|t| t.starts_with('#')));
        // The string must directly follow the keyword: a dict key
        // (`text scene["name"]`) or a style name (`… style "input_prompt"`) is
        // code — splicing it breaks lookups / renames the style.
        assert!(!texts.contains(&"name"));
        assert!(!texts.contains(&"input_prompt"));
        // Screen literals keep spliceable byte spans; content matches the span.
        for u in units.iter().filter(|u| u.kind == UnitKind::Term) {
            let (s, l) = parse_pointer(&u.pointer).unwrap();
            assert_eq!(&src[s..s + l], u.source);
        }
    }
}
