//! Temporary probe: re-run `fill_tl` over a game's existing `tl/<lang>/` files using
//! the project DB, and report what changed. Usage:
//!   fill_probe <game root> <lang>
use app_lib::engine::renpy_tl;
use std::collections::HashMap;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = std::path::PathBuf::from(args.next().expect("usage: fill_probe <root> <lang>"));
    let lang = args.next().unwrap_or_else(|| "thai".into());

    let conn = rusqlite::Connection::open(root.join(".rpgtl/project.db")).unwrap();
    let mut stmt = conn
        .prepare("SELECT source, translation FROM unit WHERE translation IS NOT NULL AND translation <> ''")
        .unwrap();
    let map: HashMap<String, String> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    println!("translations in db: {}", map.len());
    let lookup = |s: &str| map.get(s).cloned();

    let dir = root.join("game").join("tl").join(&lang);
    let (mut files, mut changed_lines) = (0usize, 0usize);
    for e in std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()) {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("rpy") {
            continue;
        }
        let before = std::fs::read_to_string(&p).unwrap();
        let after = renpy_tl::fill_tl(&before, &lookup);
        let n = before
            .lines()
            .zip(after.lines())
            .filter(|(a, b)| a != b)
            .count();
        if n > 0 {
            files += 1;
            changed_lines += n;
            println!("{:>6}  {}", n, p.file_name().unwrap().to_string_lossy());
        }
        // `fill_probe <root> <lang> <outdir>` also writes the result, so it can be
        // eyeballed without touching the game.
        if let Some(out) = std::env::args().nth(3) {
            let out = std::path::PathBuf::from(out);
            std::fs::create_dir_all(&out).unwrap();
            std::fs::write(out.join(p.file_name().unwrap()), &after).unwrap();
        }
    }
    println!("files touched: {files}, lines changed: {changed_lines}");
}
