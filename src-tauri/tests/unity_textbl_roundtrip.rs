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

    // Translate the first few fields of EACH tier so a launch exercises both the
    // TextTable (UI/SFX) and Dialogue System (story) render paths. A recognizable
    // marker (`〔TH〕`) makes the changed strings easy to spot in-game.
    let mut tbl = 0;
    let mut ds = 0;
    for u in units.iter_mut() {
        let tier = u.pointer.split('#').next().unwrap_or("");
        if tier == "tbl" && tbl < 6 {
            u.translation = Some(format!("〔TH-ui{tbl}〕"));
            u.status = Status::Translated;
            tbl += 1;
        } else if tier == "ds" && ds < 6 {
            u.translation = Some(format!("〔TH-บทพูด{ds}〕"));
            u.status = Status::Translated;
            ds += 1;
        }
    }
    assert!(tbl >= 4 && ds >= 4, "expected both tiers to translate (tbl={tbl} ds={ds})");

    let ex = unity_textbl::export_bundles(root, &data, &units, true).unwrap();
    eprintln!("full export: {}", ex.note);
    assert!(!ex.note.contains("failed"), "font embed should succeed: {}", ex.note);
    assert!(ex.bundles >= 2, "at least one bundle + one dialogue file edited");

    // Re-extract and confirm both tiers read back as Thai (load-faithful).
    let after = eng.extract(root, &ExtractOpts::default()).unwrap();
    let th_ui = after.iter().filter(|u| u.source.contains("TH-ui")).count();
    let th_dlg = after.iter().filter(|u| u.source.contains("TH-บทพูด")).count();
    eprintln!("after export: {th_ui} TextTable + {th_dlg} Dialogue lines read back as Thai");
    assert!(th_ui >= 4, "expected Thai TextTable, found {th_ui}");
    assert!(th_dlg >= 4, "expected Thai dialogue, found {th_dlg}");
}
