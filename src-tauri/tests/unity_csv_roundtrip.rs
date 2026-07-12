//! Unity (CSV localization) engine: detection (the `StreamingAssets/Localization/
//! <lang>/` fingerprint), extraction from the source-locale `key;value` catalogs,
//! byte-level round-trip identity, targeted single-value injection, and the additive
//! `export_locale` parallel-locale export (a new `<lang>/` folder + `meta.txt`, source
//! locales untouched).
//!
//! The font/CRC path (`embed_font`) drives the UnityPy sidecar + a binary catalog
//! patch, so it is exercised by the inline unit tests + manual game testing, not here
//! (no Python/real bundle in CI). Every test below uses `embed_font = false`.

use app_lib::engine::{self, unity_csv, ExtractOpts};
use app_lib::model::{Status, UnitKind};
use std::path::Path;

/// CRLF `key;value` catalogs, values carrying literal `""` and rich-text tags (opaque).
const DIALOGS: &[u8] =
    b"67e9_p_ceea;Another day, another morning.\r\n82a8_p_2faa;Get dressed first.\r\nempty_line;\r\n";
const UI: &[u8] = b"menu_new_game;New Game\r\nui_tag;Added to <color=\"\"white\"\">Gallery</color>\r\n";

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// A minimal Unity game tree with the CSV-localization scheme (english + russian).
fn make_game(root: &Path) {
    let base = "Milf Plaza_Data/StreamingAssets/Localization";
    write(root, &format!("{base}/english/meta.txt"), br#"{"_visibleName":"English"}"#);
    write(root, &format!("{base}/english/dialogs.csv"), DIALOGS);
    write(root, &format!("{base}/english/ui.csv"), UI);
    write(root, &format!("{base}/russian/meta.txt"), br#"{"_visibleName":"Russian"}"#);
    write(root, &format!("{base}/russian/dialogs.csv"), "67e9_p_ceea;Другой день.\r\n".as_bytes());
    // A Unity marker file, so it looks like a real player build.
    write(root, "Milf Plaza_Data/globalgamemanagers", b"\0");
}

#[test]
fn detects_csv_localization_and_declines_plain_unity() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();

    // A bare Unity _Data dir (no Localization scheme) must NOT detect as this engine.
    write(root, "Plain_Data/globalgamemanagers", b"\0");
    write(root, "Plain_Data/resources.assets", b"\0");
    assert!(engine::detect(root).map(|e| e.id().to_string()).as_deref() != Some("unity-csvloc"));

    // Add the Localization/<lang>/ scheme → detected as unity-csvloc.
    make_game(root);
    let eng = engine::detect(root).expect("an engine should detect");
    assert_eq!(eng.id(), "unity-csvloc");
    let desc = eng.describe(root).unwrap();
    assert!(desc.data_dir.ends_with("Milf Plaza_Data"));
    assert_eq!(desc.file_count, 2); // english dialogs.csv + ui.csv
    assert!(desc.warnings.iter().any(|w| w.contains("embed font")));
}

#[test]
fn extracts_source_values_only() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    make_game(root);

    let units = engine::detect(root)
        .unwrap()
        .extract(root, &ExtractOpts::default())
        .unwrap();

    let sources: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
    assert!(sources.contains(&"Another day, another morning."));
    assert!(sources.contains(&"New Game"));
    // Empty cell skipped; russian (not the source locale) never read.
    assert!(!sources.iter().any(|s| s.is_empty()));
    assert!(!sources.iter().any(|s| s.contains("Другой")));
    assert!(units.iter().all(|u| u.file.contains("/english/")));

    // The literal `""` in the rich-text tag survives verbatim (opaque value).
    let tag = units.iter().find(|u| u.source.contains("Gallery")).unwrap();
    assert_eq!(tag.source, r#"Added to <color=""white"">Gallery</color>"#);
    // Context is the key; dialogs are tinted Dialogue.
    let line = units.iter().find(|u| u.source.starts_with("Another day")).unwrap();
    assert_eq!(line.context.as_deref(), Some("67e9_p_ceea"));
    assert_eq!(line.kind, UnitKind::Dialogue);
}

#[test]
fn roundtrip_is_byte_identical() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    make_game(root);
    let eng = engine::detect(root).unwrap();

    let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();
    for u in &mut units {
        u.translation = Some(u.source.clone());
        u.status = Status::Translated;
    }
    let out = tempfile::tempdir().unwrap();
    eng.inject(root, &units, out.path()).unwrap();

    for rel in ["english/dialogs.csv", "english/ui.csv"] {
        let base = format!("Milf Plaza_Data/StreamingAssets/Localization/{rel}");
        let orig = std::fs::read(root.join(&base)).unwrap();
        let got = std::fs::read(out.path().join(&base)).unwrap();
        assert_eq!(orig, got, "{rel} must round-trip byte-identical");
    }
}

#[test]
fn export_locale_writes_parallel_thai_folder() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    make_game(root);
    let data = root.join("Milf Plaza_Data");
    let eng = engine::detect(root).unwrap();

    let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();
    for u in &mut units {
        if u.source == "New Game" {
            u.translation = Some("เริ่มเกมใหม่".into());
            u.status = Status::Translated;
        }
    }

    let ex = unity_csv::export_locale(root, &data, &units, "Thai", false, false, None).unwrap();
    assert!(ex.files >= 2);

    let thai = data.join("StreamingAssets/Localization/thai");
    // meta.txt names the language for the in-game menu.
    let meta = std::fs::read_to_string(thai.join("meta.txt")).unwrap();
    assert!(meta.contains(r#""_visibleName":"Thai""#));
    // Translated catalog carries the Thai value, CRLF + other rows intact.
    let ui = std::fs::read_to_string(thai.join("ui.csv")).unwrap();
    assert!(ui.contains("menu_new_game;เริ่มเกมใหม่\r\n"));
    assert!(ui.contains(r#"ui_tag;Added to <color=""white"">Gallery</color>"#));
    // A catalog with no translations is copied verbatim so the locale is complete.
    assert!(thai.join("dialogs.csv").is_file());
    // The source english locale is never modified.
    let eng_ui =
        std::fs::read_to_string(data.join("StreamingAssets/Localization/english/ui.csv")).unwrap();
    assert!(eng_ui.contains("menu_new_game;New Game\r\n"));
}

/// Optional smoke test against a real game folder. Set `RPGTL_UNITYCSV_GAME` to the
/// game root (the folder holding `<name>_Data`) to run it; otherwise it is skipped, so
/// it stays harmless in CI. Verifies detection + a non-trivial extract on real files.
#[test]
fn real_game_detect_and_extract() {
    let Ok(root) = std::env::var("RPGTL_UNITYCSV_GAME") else {
        eprintln!("skipped: set RPGTL_UNITYCSV_GAME to a game root to run");
        return;
    };
    let root = Path::new(&root);
    let eng = engine::detect(root).expect("real game should detect");
    assert_eq!(eng.id(), "unity-csvloc");
    let units = eng.extract(root, &ExtractOpts::default()).unwrap();
    assert!(units.len() > 500, "expected a full catalog, got {}", units.len());
    // No unit is empty, and every one is addressable by its span.
    for u in &units {
        assert!(!u.source.is_empty());
        assert!(u.pointer.contains(':'));
    }
    eprintln!("real game: extracted {} units from {}", units.len(), eng.name());
}

/// Optional full end-to-end against a real game: extract, translate a few visible
/// strings to Thai, then `export_locale(embed_font=true)` — writing the `thai/` locale
/// folder, swapping the Thai font into the font bundle(s), and zeroing their
/// Addressables CRC — all through the production Rust path (which drives the UnityPy
/// sidecar). Needs `RPGTL_UNITYCSV_GAME` set AND Python + UnityPy on PATH. Skipped
/// otherwise. **Modifies the game in place** (restore from the `.rpgtl`/manual backups
/// if needed).
#[test]
fn real_game_full_export() {
    let (Ok(root), Ok(_)) = (
        std::env::var("RPGTL_UNITYCSV_GAME"),
        std::env::var("RPGTL_UNITYCSV_WRITE"), // extra opt-in: this writes to the game
    ) else {
        eprintln!("skipped: set RPGTL_UNITYCSV_GAME and RPGTL_UNITYCSV_WRITE to run");
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
    let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();

    // Translate a handful of always-visible strings so the launch shows Thai.
    let thai: std::collections::HashMap<&str, &str> = [
        ("menu_new_game", "เริ่มเกมใหม่ ◆TH◆"),
        ("menu_continue", "เล่นต่อ"),
        ("menu_settings", "ตั้งค่า"),
        ("menu_exit", "ออกจากเกม"),
        ("language", "ภาษา"),
        ("67e9_p_ceea", "อื้อ... อีกวัน อีกเช้า ◆TH◆"),
    ]
    .into_iter()
    .collect();
    let mut hit = 0;
    for u in &mut units {
        if let Some(t) = u.context.as_deref().and_then(|k| thai.get(k)) {
            u.translation = Some((*t).to_string());
            u.status = Status::Translated;
            hit += 1;
        }
    }
    assert!(hit >= 4, "expected to match sample keys, matched {hit}");

    let ex = unity_csv::export_locale(root, &data, &units, "Thai", false, true, None).unwrap();
    eprintln!("full export: {}", ex.note);
    assert!(!ex.note.contains("failed"), "font embed should succeed: {}", ex.note);

    // thai/ folder + meta written (slug of "Thai").
    let created = data.join("StreamingAssets/Localization/thai");
    assert!(created.join("meta.txt").is_file(), "meta.txt written");
    assert!(created.join("ui.csv").is_file(), "ui catalog written");

    // Every font bundle's CRC is zeroed in the catalog.
    let cat = std::fs::read(data.join("StreamingAssets/aa/catalog.bin")).unwrap();
    let sw = data.join("StreamingAssets/aa/StandaloneWindows64");
    for e in std::fs::read_dir(&sw).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with("fonts") || !name.ends_with(".bundle") {
            continue;
        }
        let hash = name
            .trim_end_matches(".bundle")
            .rsplit('_')
            .find(|t| t.len() == 32 && t.bytes().all(|b| b.is_ascii_hexdigit()))
            .unwrap();
        let raw: Vec<u8> = (0..16)
            .map(|i| u8::from_str_radix(&hash[i * 2..i * 2 + 2], 16).unwrap())
            .collect();
        let pos = cat.windows(16).position(|w| w == raw.as_slice()).unwrap();
        assert_eq!(&cat[pos + 60..pos + 64], &[0, 0, 0, 0], "CRC zeroed for {name}");
    }
    eprintln!("full export: all font-bundle CRCs zeroed ✓");
}

/// Optional real-game MOD export: `export_locale(out_base = Some(staging))` overwrites
/// every locale by key into a staging mirror + stages the font bundles/catalog there,
/// never touching the game. Needs `RPGTL_UNITYCSV_GAME` + Python/UnityPy on PATH.
/// Writes only to a temp dir, so no write opt-in is required.
#[test]
fn real_game_mod_export() {
    let Ok(root) = std::env::var("RPGTL_UNITYCSV_GAME") else {
        eprintln!("skipped: set RPGTL_UNITYCSV_GAME to run");
        return;
    };
    let root = Path::new(&root);
    let data = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir() && p.file_name().unwrap().to_string_lossy().ends_with("_Data"))
        .expect("a _Data dir");
    let cat = data.join("StreamingAssets/aa/catalog.bin");

    // Read a font-bundle CRC from a catalog: 16-byte hash from the filename → +60.
    let crc_at = |catalog: &Path, bundle_name: &str| -> u32 {
        let hash = bundle_name
            .trim_end_matches(".bundle")
            .rsplit('_')
            .find(|t| t.len() == 32 && t.bytes().all(|b| b.is_ascii_hexdigit()))
            .unwrap();
        let raw: Vec<u8> = (0..16)
            .map(|i| u8::from_str_radix(&hash[i * 2..i * 2 + 2], 16).unwrap())
            .collect();
        let bytes = std::fs::read(catalog).unwrap();
        let pos = bytes.windows(16).position(|w| w == raw.as_slice()).unwrap();
        u32::from_le_bytes(bytes[pos + 60..pos + 64].try_into().unwrap())
    };
    let bundle_names: Vec<String> = std::fs::read_dir(data.join("StreamingAssets/aa/StandaloneWindows64"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("fonts") && n.ends_with(".bundle"))
        .collect();
    let game_crc_before: Vec<u32> = bundle_names.iter().map(|n| crc_at(&cat, n)).collect();

    let eng = engine::detect(root).unwrap();
    let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();
    for u in &mut units {
        if u.context.as_deref() == Some("menu_new_game") || u.context.as_deref() == Some("67e9_p_ceea") {
            u.translation = Some("ไทย ◆TH◆".into());
            u.status = Status::Translated;
        }
    }

    let staging = tempfile::tempdir().unwrap();
    let ex = unity_csv::export_locale(root, &data, &units, "Thai", false, true, Some(staging.path()))
        .unwrap();
    eprintln!("mod export: {}", ex.note);
    assert!(!ex.note.contains("failed"), "mod font embed should succeed: {}", ex.note);

    // The mod overwrote EVERY existing locale (english + russian) by key.
    let mod_data = staging.path().join(data.file_name().unwrap());
    let loc = mod_data.join("StreamingAssets/Localization");
    let mut locales_seen = 0;
    for e in std::fs::read_dir(&loc).unwrap().flatten() {
        if e.path().is_dir() {
            assert!(e.path().join("ui.csv").is_file() || e.path().join("dialogs.csv").is_file());
            locales_seen += 1;
        }
    }
    assert!(locales_seen >= 2, "expected english + russian overwritten, saw {locales_seen}");

    // Font bundles staged + their CRC zeroed in the MOD catalog.
    let mod_cat = mod_data.join("StreamingAssets/aa/catalog.bin");
    for n in &bundle_names {
        assert!(
            mod_data.join(format!("StreamingAssets/aa/StandaloneWindows64/{n}")).is_file(),
            "{n} should be staged in the mod"
        );
        assert_eq!(crc_at(&mod_cat, n), 0, "{n} CRC zeroed in the mod catalog");
    }

    // The GAME is untouched: its catalog CRCs are unchanged (mod patched only its copy).
    let game_crc_after: Vec<u32> = bundle_names.iter().map(|n| crc_at(&cat, n)).collect();
    assert_eq!(game_crc_before, game_crc_after, "the game catalog must be untouched");
    eprintln!("mod export: {locales_seen} locales overwritten, fonts staged, game untouched ✓");
}
