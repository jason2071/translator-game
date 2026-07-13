//! Integration coverage for the `unity-textbl` engine (Unity Mono + Addressables
//! custom `TextTable` MonoBehaviours). The engine's real work runs through a UnityPy
//! helper against gigabyte-scale bundles, so there is no synthetic fixture — the
//! meaningful checks are **env-gated** against a real game and skipped otherwise (so
//! CI stays green without the game or Python/UnityPy).
//!
//! Point `RPGTL_TEXTBL_GAME` at the game root (the folder holding `<name>_Data`) to run
//! detect + extract. Add `RPGTL_TEXTBL_WRITE=1` to also run the full in-place export
//! (this **modifies the game bundles** — originals are snapshotted under `.rpgtl/source/`).

use app_lib::engine::{self, unity_textbl, ExtractOpts};
use app_lib::model::Status;
use std::path::Path;

/// Detection + a non-trivial extract on a real game.
#[test]
fn real_game_detect_and_extract() {
    let Ok(root) = std::env::var("RPGTL_TEXTBL_GAME") else {
        eprintln!("skipped: set RPGTL_TEXTBL_GAME to a game root to run");
        return;
    };
    let root = Path::new(&root);
    let eng = engine::detect(root).expect("real game should detect");
    assert_eq!(eng.id(), "unity-textbl");

    let units = eng.extract(root, &ExtractOpts::default()).unwrap();
    assert!(units.len() > 100, "expected a full table, got {}", units.len());
    for u in &units {
        assert!(!u.source.is_empty(), "no empty source cells");
        // Pointer shape: <kind>#<file>#<pathId>#<idx>, kind ∈ {tbl (TextTable), ds (DS)}.
        assert!(
            u.pointer.starts_with("tbl#") || u.pointer.starts_with("ds#"),
            "pointer {}",
            u.pointer
        );
        assert_eq!(u.pointer.matches('#').count(), 3, "pointer {}", u.pointer);
    }
    // Both tiers should be present: TextTable UI/SFX (tbl#) + Dialogue System story (ds#).
    let tbl = units.iter().filter(|u| u.pointer.starts_with("tbl#")).count();
    let ds = units.iter().filter(|u| u.pointer.starts_with("ds#")).count();
    assert!(tbl > 0 && ds > 0, "expected both tiers, got tbl={tbl} ds={ds}");
    eprintln!("real game: extracted {} strings ({tbl} TextTable + {ds} Dialogue System)", units.len());
}

/// Full in-place export against a real game: extract, translate a few fields to Thai,
/// then `export_bundles(embed_font=true)` — repacking the edited bundles, clearing the
/// catalog CRC, and swapping the Thai font — all through the production Rust path.
/// Needs `RPGTL_TEXTBL_GAME` + `RPGTL_TEXTBL_WRITE` AND Python + UnityPy on PATH.
#[test]
fn real_game_full_export() {
    let (Ok(root), Ok(_)) = (
        std::env::var("RPGTL_TEXTBL_GAME"),
        std::env::var("RPGTL_TEXTBL_WRITE"),
    ) else {
        eprintln!("skipped: set RPGTL_TEXTBL_GAME and RPGTL_TEXTBL_WRITE to run");
        return;
    };
    let root = Path::new(&root);
    let data = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir() && p.file_name().unwrap().to_string_lossy().ends_with("_Data"))
        .expect("a _Data dir");

    let eng = engine::detect(root).unwrap();
    assert_eq!(eng.id(), "unity-textbl");
    let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();

    // Translate the first few fields (whatever they are) so a launch shows Thai.
    let mut hit = 0;
    for u in units.iter_mut().take(8) {
        u.translation = Some(format!("ไทย#{hit}"));
        u.status = Status::Translated;
        hit += 1;
    }
    assert!(hit >= 4, "expected fields to translate");

    let ex = unity_textbl::export_bundles(root, &data, &units, true).unwrap();
    eprintln!("full export: {}", ex.note);
    assert!(!ex.note.contains("failed"), "font embed should succeed: {}", ex.note);
    assert!(ex.bundles >= 1, "at least one bundle edited");

    // Re-extract and confirm the translated fields now read back as Thai (load-faithful).
    let after = eng.extract(root, &ExtractOpts::default()).unwrap();
    let thai = after.iter().filter(|u| u.source.starts_with("ไทย#")).count();
    assert!(thai >= 4, "expected Thai in the Default column, found {thai}");
}
