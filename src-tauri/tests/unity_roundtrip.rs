//! Unity (Naninovel) engine. The Rust side owns detection, the opaque
//! `"<file>#<pathId>#<key>"` pointer, and the manifest/patch contract; the binary
//! `.assets` work is done by the bundled Python + UnityPy helper. So this file
//! splits into two:
//!
//!   * Pure-Rust checks (always run) — detection fingerprint on a synthetic game
//!     tree. The pointer/kind/mask logic is unit-tested in `engine::unity` and
//!     `engine::protect`.
//!   * A full extract→inject round-trip (opt-in) — needs a real Naninovel game and
//!     Python + UnityPy, neither available in CI, so it runs only when
//!     `RPGTL_UNITY_GAME` points at a game root. See `docs/games/unity-naninovel.md`.

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::{Status, UnitKind};
use std::path::{Path, PathBuf};

fn touch(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b"x").unwrap();
}

/// The Unity engine detects a Naninovel player build (`<name>_Data/` with
/// `resources.assets` + a Naninovel assembly) and declines a plain Unity game.
#[test]
fn detect_only_naninovel_unity_games() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    touch(&root.join("Plain_Data").join("resources.assets"));
    touch(&root.join("Plain_Data").join("Managed").join("Assembly-CSharp.dll"));
    assert!(
        engine::detect(root).is_none(),
        "a non-Naninovel Unity game must not be claimed by any engine"
    );

    touch(
        &root
            .join("Plain_Data")
            .join("Managed")
            .join("Elringus.Naninovel.Runtime.dll"),
    );
    let eng = engine::detect(root).expect("Naninovel game should now detect");
    assert_eq!(eng.id(), "unity");
    let desc = eng.describe(root).unwrap();
    assert!(desc.data_dir.ends_with("Plain_Data"));
    assert!(!desc.warnings.is_empty(), "should advise the experimental scope");
}

/// End-to-end against a real game (opt-in via `RPGTL_UNITY_GAME`). Extracts the
/// English managed-text slot, translates every unit with a reversible marker,
/// injects into a temp out dir, and re-extracts the produced `.assets` to confirm
/// the marker round-tripped — i.e. the helper wrote a valid, re-loadable file whose
/// records changed exactly as asked.
#[test]
fn extract_inject_roundtrip_on_a_real_game() {
    let Ok(game) = std::env::var("RPGTL_UNITY_GAME") else {
        eprintln!("skipping: set RPGTL_UNITY_GAME=<game root> to run the live round-trip");
        return;
    };
    let root = PathBuf::from(&game);
    let eng = engine::detect(&root).expect("RPGTL_UNITY_GAME is not a detected game");
    assert_eq!(eng.id(), "unity");
    let data_dir = PathBuf::from(eng.describe(&root).unwrap().data_dir);

    let mut units = match eng.extract(&root, &ExtractOpts::default()) {
        Ok(u) => u,
        Err(e) => {
            // No Python / UnityPy on this machine → treat as skip, not failure.
            eprintln!("skipping live round-trip (helper unavailable): {e:#}");
            return;
        }
    };
    assert!(!units.is_empty(), "a Naninovel game should yield units");
    // Two tiers: managed text ("<file>#<pathId>#<key>", 3 parts) and dialogue
    // ("dlg#<file>#<pathId>#<idx>", the dlg# tag + 3 parts).
    assert!(
        units.iter().all(|u| {
            let core = u.pointer.strip_prefix("dlg#").unwrap_or(&u.pointer);
            core.split('#').count() == 3
        }),
        "pointers are managed-text or dlg# dialogue"
    );
    assert!(
        units.iter().any(|u| u.kind == UnitKind::Dialogue),
        "the dialogue tier should yield compiled story lines"
    );

    // Translate every unit with a reversible marker.
    for u in &mut units {
        u.translation = Some(format!("[RT]{}", u.source));
        u.status = Status::Translated;
    }
    let out = tempfile::tempdir().unwrap();
    eng.inject(&root, &units, out.path()).expect("inject");

    // The helper emits only the changed `.assets`; at least one must appear.
    let produced: Vec<_> = std::fs::read_dir(out.path())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("assets"))
        .collect();
    assert!(!produced.is_empty(), "inject should write at least one patched .assets");

    // Re-extract from a shadow game whose data dir is the produced file, and confirm
    // the marker survived the binary round-trip.
    let shadow = tempfile::tempdir().unwrap();
    let shadow_data = shadow.path().join(data_dir.file_name().unwrap());
    // Re-extract only needs the produced `.assets` plus the Naninovel fingerprint dll
    // (TextAsset scripts are inline, so the `.resS` stream file isn't required).
    touch(&shadow_data.join("Managed").join("Naninovel.Runtime.dll"));
    for p in &produced {
        std::fs::copy(p, shadow_data.join(p.file_name().unwrap())).unwrap();
    }
    // Any other .assets the game needs for a full extract are absent, but the changed
    // file is present, so re-extraction sees the marked records.
    let re = eng
        .extract(shadow.path(), &ExtractOpts::default())
        .expect("re-extract produced .assets");
    assert!(
        re.iter().any(|u| u.source.starts_with("[RT]")),
        "the injected marker must survive the binary round-trip"
    );
    assert!(
        re.iter()
            .any(|u| u.kind == UnitKind::Dialogue && u.source.starts_with("[RT]")),
        "a compiled dialogue line's translation must survive the raw byte-splice"
    );
}
