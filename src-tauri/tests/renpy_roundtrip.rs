//! Ren'Py engine: detection, extraction (dialogue vs code), and the
//! extract -> inject byte-level round-trip identity.

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::{Status, UnitKind};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/renpy-sample")
}

#[test]
fn detects_renpy() {
    let eng = engine::detect(&fixture()).expect("should detect Ren'Py");
    assert_eq!(eng.id(), "renpy");
    let d = eng.describe(&fixture()).unwrap();
    assert_eq!(d.engine_id, "renpy");
    assert!(d.file_count >= 1, "found {} rpy files", d.file_count);
}

#[test]
fn extract_finds_dialogue_not_code() {
    let eng = engine::detect(&fixture()).unwrap();
    let units = eng.extract(&fixture(), &ExtractOpts::default()).unwrap();
    let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();

    // Dialogue, choices, and menu caption are extracted.
    assert!(texts.contains(&"It was a dark and stormy night."));
    assert!(texts.contains(&"Hello. I'm glad you could make it."));
    assert!(texts.contains(&"This is going to be fun!"));
    assert!(texts.contains(&"Where should we begin?"));
    assert!(texts.contains(&"Where to?"));
    assert!(texts.contains(&"The forest"));
    assert!(texts.contains(&"The village"));
    assert!(texts.contains(&"Into the woods we go."));
    assert!(texts.contains(&"She said \\\"watch out\\\" and pointed."));
    assert!(texts.contains(&"Welcome back, [player]. {i}Ready?{/i}"));

    // `_("...")` strings are extracted even inside a screen/python block.
    assert!(texts.contains(&"Start Game"));
    assert!(texts.contains(&"Options"));
    assert!(texts.contains(&"Progress saved."));

    // Asset names, unwrapped screen/UI text, and python code are NOT extracted.
    assert!(!texts.contains(&"audio/hello.ogg"));
    assert!(!texts.contains(&"HUD text, not dialogue."));
    assert!(!texts.contains(&"Menu button"));
    assert!(!texts.contains(&"python code string"));

    // A Character's display name IS extracted (so it can be translated) — as a Name
    // unit keyed to the character variable.
    let eileen = units
        .iter()
        .find(|u| u.source == "Eileen")
        .expect("Character name extracted");
    assert_eq!(eileen.kind, UnitKind::Name);
    assert_eq!(eileen.context.as_deref(), Some("e"));
    assert!(eileen.pointer.starts_with("name#"));

    // The game/tl/<lang>/ translation tree is another language, not source —
    // never extracted, so the project stays single-language.
    assert!(!texts.iter().any(|t| t.contains("Bonjour")));
    assert!(!texts.contains(&"Commencer le jeu"));

    // Speaker context + kind classification.
    let hi = units
        .iter()
        .find(|u| u.source == "Hello. I'm glad you could make it.")
        .unwrap();
    assert_eq!(hi.kind, UnitKind::Dialogue);
    assert_eq!(hi.context.as_deref(), Some("e"));

    let narr = units
        .iter()
        .find(|u| u.source == "It was a dark and stormy night.")
        .unwrap();
    assert_eq!(narr.context, None); // narrator has no speaker

    let forest = units.iter().find(|u| u.source == "The forest").unwrap();
    assert_eq!(forest.kind, UnitKind::Choice);
    assert_eq!(forest.context, None);
}

#[test]
fn tl_dir_is_excluded_from_file_count() {
    let eng = engine::detect(&fixture()).unwrap();
    let d = eng.describe(&fixture()).unwrap();
    // Only the base script.rpy counts; game/tl/french/script.rpy is skipped.
    assert_eq!(d.file_count, 1, "tl/ files must not be counted");
}

#[test]
fn roundtrip_identity() {
    // Translate every unit to itself, inject, and require byte-identical output.
    let root = fixture();
    let eng = engine::detect(&root).unwrap();
    let mut units = eng.extract(&root, &ExtractOpts::default()).unwrap();
    for u in &mut units {
        u.translation = Some(u.source.clone());
        u.status = Status::Draft;
    }

    let out = tempfile::tempdir().unwrap();
    eng.inject(&root, &units, out.path()).unwrap();

    let game = root.join("game");
    let files: BTreeSet<String> = units.iter().map(|u| u.file.clone()).collect();
    for file in files {
        let orig = std::fs::read(game.join(&file)).unwrap();
        let patched = std::fs::read(out.path().join(&file)).unwrap();
        assert_eq!(orig, patched, "round-trip altered {file}");
    }
}

#[test]
fn inject_replaces_only_target_span() {
    let root = fixture();
    let eng = engine::detect(&root).unwrap();
    let units = eng.extract(&root, &ExtractOpts::default()).unwrap();
    let mut u = units
        .iter()
        .find(|u| u.source == "Into the woods we go.")
        .unwrap()
        .clone();
    u.translation = Some("เข้าป่ากันเถอะ".to_string());
    u.status = Status::Translated;

    let out = tempfile::tempdir().unwrap();
    eng.inject(&root, std::slice::from_ref(&u), out.path()).unwrap();

    let patched = std::fs::read_to_string(out.path().join(&u.file)).unwrap();
    // The target line now holds the translation, quotes intact.
    assert!(patched.contains("e \"เข้าป่ากันเถอะ\""));
    // A neighboring line is untouched.
    assert!(patched.contains("Back to town."));
    assert!(!patched.contains("Into the woods we go."));
}

#[test]
fn archived_game_resolves_to_game_dir_and_reports_packed() {
    // A game whose scripts are packed (game/*.rpa, no loose .rpy), with the SDK's
    // renpy/common/*.rpy present at the root — the exact shape that used to make
    // detection fall through to `root` and import the SDK UI strings.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("game")).unwrap();
    std::fs::write(root.join("game/archive.rpa"), b"RPA-3.0 fake").unwrap();
    std::fs::write(root.join("game/script_version.txt"), b"8.4.0").unwrap();
    std::fs::create_dir_all(root.join("renpy/common")).unwrap();
    std::fs::write(root.join("renpy/common/00action.rpy"), "old \"Quit\"\n").unwrap();
    // Our own export artifact from a prior run must not count as game source (else
    // its font-path strings get re-imported and the packed game looks translatable).
    std::fs::write(
        root.join("game/zzz_translator.rpy"),
        "translate thai python:\n    _tl_font = \"fonts/tl_font.ttf\"\n",
    )
    .unwrap();

    let eng = engine::detect(root).expect("archived Ren'Py game still detects");
    assert_eq!(eng.id(), "renpy");
    // data_dir must be game/, NOT root (so the tl/ check and font remap land right,
    // and renpy/common at the root is never treated as the game).
    let d = eng.describe(root).unwrap();
    assert!(d.data_dir.replace('\\', "/").ends_with("/game"), "data_dir = {}", d.data_dir);

    // The archive here isn't a readable RPA, so auto-unpack recovers no source and
    // extraction fails with an actionable message (decompile the .rpyc) rather than
    // importing the SDK UI.
    let err = eng.extract(root, &ExtractOpts::default()).unwrap_err().to_string();
    assert!(err.contains("compiled") && err.contains("unrpyc"), "got: {err}");
}

/// Assemble a real RPA-3.0 archive: `RPA-3.0 <hex index off> <hex key>\n`, each
/// file's bytes, then the zlib-compressed pickled index `{path: [(off^key,
/// len^key, b"")]}`. The header is fixed-width so file data starts at a known
/// offset. Mirrors what a real Ren'Py `.rpa` looks like.
fn build_rpa(key: u64, files: &[(&str, &[u8])]) -> Vec<u8> {
    use flate2::{write::ZlibEncoder, Compression};
    use serde_pickle::{HashableValue, Value};
    use std::collections::BTreeMap;
    use std::io::Write;

    let header_len = format!("RPA-3.0 {:016x} {:08x}\n", 0u64, key).len();
    let mut body = Vec::new();
    let mut index = BTreeMap::new();
    for (name, bytes) in files {
        let offset = (header_len + body.len()) as u64;
        body.extend_from_slice(bytes);
        index.insert(
            HashableValue::String((*name).to_string()),
            Value::List(vec![Value::Tuple(vec![
                Value::I64((offset ^ key) as i64),
                Value::I64((bytes.len() as u64 ^ key) as i64),
                Value::Bytes(Vec::new()),
            ])]),
        );
    }
    let index_offset = (header_len + body.len()) as u64;
    let pickled =
        serde_pickle::value_to_vec(&Value::Dict(index), serde_pickle::SerOptions::new()).unwrap();
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
fn packed_game_with_source_rpy_auto_unpacks() {
    // A packed game that ships its source .rpy *inside* the .rpa (as many do).
    // The app must recover the source automatically at import — no external tools.
    let script = "label start:\n    e \"Into the woods we go.\"\n    \"Back to town.\"\n";
    let archive = build_rpa(0x4242_4242, &[("story.rpy", script.as_bytes())]);

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("game")).unwrap();
    std::fs::write(root.join("game/archive.rpa"), &archive).unwrap();
    std::fs::write(root.join("game/script_version.txt"), b"8.4.0").unwrap();

    let eng = engine::detect(root).expect("packed Ren'Py game detects");
    assert_eq!(eng.id(), "renpy");

    // describe() peeks the archive index (read-only) and reports the packed source
    // count — without unpacking yet.
    let d = eng.describe(root).unwrap();
    assert_eq!(d.file_count, 1, "peeked .rpy count from the archive");
    assert!(!root.join("game/story.rpy").exists(), "describe must not write");

    // extract() unpacks the source out of the archive, then reads it normally.
    let units = eng.extract(root, &ExtractOpts::default()).unwrap();
    assert!(root.join("game/story.rpy").exists(), "extract unpacked the source");
    let sources: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
    assert!(sources.contains(&"Into the woods we go."));
    assert!(sources.contains(&"Back to town."));
    assert_eq!(
        units.iter().find(|u| u.source == "Into the woods we go.").unwrap().kind,
        UnitKind::Dialogue
    );

    // Re-extract is idempotent: the loose .rpy now wins, the archive isn't re-read.
    let again = eng.extract(root, &ExtractOpts::default()).unwrap();
    assert_eq!(again.len(), units.len());
}

#[test]
fn compiled_only_game_without_bundled_python_reports_actionable_error() {
    // Scripts packed as `.rpyc` inside the `.rpa` (no `.rpy` anywhere) and NO bundled
    // Python under `lib/`. Auto-decompile stages the `.rpyc` out of the archive but
    // can't run unrpyc, so import must still degrade to the actionable
    // "decompile … unrpyc" error (never a silent empty project) — now naming why the
    // automatic attempt couldn't run. This is the key no-regression guard.
    let rpyc: &[u8] = b"RENPY RPC2 fake-bytecode";
    let archive = build_rpa(0x4242_4242, &[("story.rpyc", rpyc)]);

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("game")).unwrap();
    std::fs::write(root.join("game/scripts.rpa"), &archive).unwrap();
    std::fs::write(root.join("game/script_version.txt"), b"7.4.0").unwrap();

    let eng = engine::detect(root).expect("compiled-only Ren'Py game detects");
    assert_eq!(eng.id(), "renpy");

    let err = eng
        .extract(root, &ExtractOpts::default())
        .unwrap_err()
        .to_string();
    assert!(err.contains("compiled") && err.contains("unrpyc"), "got: {err}");
    assert!(
        err.contains("Automatic decompile"),
        "should explain the auto-attempt: {err}"
    );
    // The bytecode was staged out of the archive as a side effect of the attempt.
    assert!(root.join("game/story.rpyc").exists(), "rpyc staged from archive");
}
