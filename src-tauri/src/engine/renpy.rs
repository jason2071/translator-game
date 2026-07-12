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
//! not mistaken for dialogue.

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
        // Compiled-only game: the archives / loose files held only `.rpyc`, no source
        // `.rpy`. Try to auto-decompile in place (the game ships its own Python +
        // Ren'Py runtime, which is all unrpyc needs) before giving up.
        let mut decompile_hint = None;
        if rpys.is_empty() && is_renpy_game_dir(&dir) {
            decompile_hint = ensure_decompiled(&dir, root)?;
            rpys = collect_rpy(&dir); // re-scan for the `.rpy` unrpyc just wrote
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
        for path in rpys {
            let rel = rel_path(&dir, &path);
            let content =
                std::fs::read_to_string(&path).with_context(|| format!("reading {rel}"))?;
            extract_rpy(&rel, &content, &mut units);
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
        // aren't byte spans — they're applied via the `tl/` zzz Character re-define,
        // not an in-place splice — so skip them here.
        let mut by_file: BTreeMap<&str, Vec<&TransUnit>> = BTreeMap::new();
        for u in units {
            if u.status.is_applied()
                && u.translation.is_some()
                && !u.pointer.starts_with("name#")
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

/// Blocks whose bodies are code/UI, not dialogue — skip everything indented
/// under them.
fn is_block_skip(trimmed: &str) -> bool {
    const HEADS: &[&str] = &[
        "python",
        "screen ",
        "screen:",
        "style ",
        "style:",
        "transform ",
        "transform:",
        "layeredimage ",
        "testcase ",
    ];
    if HEADS.iter().any(|h| trimmed.starts_with(h)) {
        return true;
    }
    // `init [priority] python [in <namespace>]:` — a Python block whose body is
    // raw code, regardless of the optional integer priority (e.g.
    // `init -100 python in phone.application:`). Skip it so code strings like a
    // `style="empty"` kwarg aren't mistaken for dialogue and translated (which
    // would rename the style and crash Ren'Py). A bare `init python …` matches
    // here too. Any `_()`-wrapped strings inside are still harvested earlier.
    if let Some(rest) = trimmed.strip_prefix("init") {
        if rest.starts_with(char::is_whitespace) {
            let mut toks = rest.split_whitespace();
            let mut head = toks.next();
            if head.map(|t| t.parse::<i64>().is_ok()).unwrap_or(false) {
                head = toks.next(); // consume the optional priority
            }
            if head.map(|t| t.trim_end_matches(':')) == Some("python") {
                return true;
            }
        }
    }
    false
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

fn extract_rpy(file: &str, content: &str, out: &mut Vec<TransUnit>) {
    let mut skip_indent: Option<usize> = None;
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
        // lines) — skip its bare strings (colour values, dict keys, asset paths).
        // Any `_()` strings on these lines were already harvested above.
        let delta = bracket_delta(raw);
        if skip_expr_depth > 0 {
            skip_expr_depth = (skip_expr_depth + delta).max(0);
            continue;
        }

        // Leaving a skipped block? A line at or below its indent ends it, and is
        // then processed normally for bare say/menu strings.
        if let Some(si) = skip_indent {
            if indent > si {
                continue;
            }
            skip_indent = None;
        }
        if is_block_skip(trimmed) {
            skip_indent = Some(indent);
            continue;
        }
        if is_line_skip(first_token(trimmed)) {
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

/// A double-quoted Python string literal for `s`, keeping Unicode verbatim (only
/// `"`, `\`, and the usual control chars are escaped). Unlike Rust's `{:?}`, it never
/// turns a Thai combining mark into `\u{…}` (invalid Python).
fn py_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
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
        if is_block_skip(trimmed) {
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
) -> Result<Option<TlExport>> {
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
    let lookup = |s: &str| map.get(s).map(|t| t.to_string());

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

    // Character names — re-`define` each translated Character under the language
    // (Ren'Py's `translate` never touches a bare `Character("…")` name).
    let names: Vec<(String, String)> = units
        .iter()
        .filter(|u| u.kind == UnitKind::Name && u.status.is_applied())
        .filter_map(|u| Some((u.context.clone()?, u.translation.clone()?)))
        .collect();

    // Make the language selectable (default to it) and remap the game's fonts to a
    // glyph-capable one so the translation isn't rendered as "NO GLYPH" boxes.
    setup_language(data_dir, &lang, &language_label(target_lang, &lang), &names)?;

    Ok(Some(TlExport { files, dir }))
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

/// Write a small global `.rpy` that (1) defaults the game to `lang` if it has no
/// language of its own and (2) — when a target-language font is available —
/// remaps every font the game uses to it, scoped to `lang` via a
/// `translate <lang> python:` block so English is unaffected.
fn setup_language(
    data_dir: &Path,
    lang: &str,
    label: &str,
    names: &[(String, String)],
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

    // Re-define each translated Character under the language, inheriting the original
    // via `kind=` so its colour/outlines/etc. carry over — only the name changes. Runs
    // when the language is selected, so the store variable the say-statements reference
    // now points at the translated character.
    if !names.is_empty() {
        s.push_str(&format!("translate {lang} python:\n"));
        for (ch, name) in names {
            // A Python string literal that keeps Unicode verbatim — Rust's `{:?}` would
            // escape Thai combining marks to `\u{…}`, which isn't valid Python.
            s.push_str(&format!("    {ch} = Character({}, kind={ch})\n", py_str(name)));
        }
        s.push('\n');
    }

    // The bundled font (Sarabun) covers Thai + Latin, so only remap fonts for a
    // Thai target — other scripts (CJK, etc.) would render as NO GLYPH in it.
    if lang == "thai" {
        let font_rel = "fonts/tl_font.ttf";
        copy_target_font(&data_dir.join(font_rel))?;
        let refs = collect_font_refs(data_dir);
        s.push_str(&format!("translate {lang} python:\n"));
        s.push_str(&format!("    _tl_font = \"{font_rel}\"\n"));
        s.push_str("    _tl_fonts = [\n");
        for r in &refs {
            s.push_str(&format!("        {r:?},\n"));
        }
        s.push_str("    ]\n");
        // Point every game font at the bundled Thai face. (A FontGroup that kept the
        // original font for non-Thai glyphs would be cleaner for untranslated CJK, but
        // Ren'Py's `config.font_replacement_map` only accepts a font *string*, not a
        // FontGroup — passing one crashes `load_face`. Untranslated Japanese names are
        // instead handled by translating the names themselves; see
        // `extract_character_names`.)
        s.push_str("    for _f in _tl_fonts:\n");
        s.push_str("        for _b in (False, True):\n");
        s.push_str("            for _i in (False, True):\n");
        s.push_str("                config.font_replacement_map[_f, _b, _i] = (_tl_font, _b, _i)\n");
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
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dst, TL_FONT).with_context(|| format!("writing font {}", dst.display()))?;
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
        extract_rpy("script.rpy", src, &mut out);
        out
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

        // Code / UI / asset strings must NOT be extracted.
        assert!(!texts.contains(&"audio/v1.ogg"));
        assert!(!texts.contains(&"This is UI, not dialogue."));
        assert!(!texts.contains(&"code string"));
        assert!(!texts.iter().any(|t| t.contains("Eileen")));
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
        // Unwrapped screen text (no _()) is still skipped.
        assert!(!texts.contains(&"Unwrapped"));
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
    fn setup_language_writes_character_name_overrides() {
        let d = tempfile::tempdir().unwrap();
        let names = vec![("rin".to_string(), "\u{e23}\u{e34}\u{e19}".to_string())];
        setup_language(d.path(), "thai", "\u{e44}\u{e17}\u{e22}", &names).unwrap();
        let zzz = std::fs::read_to_string(d.path().join(GENERATED_RPY)).unwrap();
        assert!(zzz.contains("translate thai python:"));
        // Re-defines the character inheriting the original via kind=.
        assert!(zzz.contains("rin = Character(\"\u{e23}\u{e34}\u{e19}\", kind=rin)"));
    }
}
