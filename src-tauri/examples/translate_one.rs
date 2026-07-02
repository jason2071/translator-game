//! Translate ONE untranslated dialogue unit straight into a live project.db,
//! using the same engine/AI code the UI's Run button drives:
//!   cargo run --example translate_one -- "<game>/.rpgtl/project.db" [model]
//! Then reload the grid in the app (toggle a filter) to see it.

use app_lib::ai::{self, BatchItem, BatchReq, ProviderConfig};
use app_lib::engine::protect;
use app_lib::model::{Status, UnitKind};
use app_lib::project::db::{self, UnitFilter};
use rusqlite::Connection;
use std::time::Duration;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let db_path = args.next().expect("path to project.db");
    let model = args.next().unwrap_or_else(|| "translategemma:12b".into());

    // Open the same DB the running app holds (WAL allows concurrent access).
    let conn = Connection::open(&db_path).expect("open project.db");
    conn.busy_timeout(Duration::from_secs(10)).unwrap();

    // Grab a batch of untranslated units and pick the first real dialogue line.
    let candidates = db::list_units(
        &conn,
        &UnitFilter {
            untranslated_only: Some(true),
            limit: Some(200),
            ..Default::default()
        },
    )
    .unwrap();
    let unit = candidates
        .into_iter()
        .find(|u| u.kind == UnitKind::Dialogue && !u.source.trim().is_empty())
        .expect("no untranslated dialogue found");

    println!("Unit #{} [{}]\nJA: {}\n", unit.id, unit.file, unit.source);

    // Mask control codes, translate via Local (Ollama), restore.
    let masked = protect::mask(&unit.source);
    let cfg = ProviderConfig {
        kind: "local".into(),
        base_url: None,
        model: model.clone(),
        temperature: Some(0.0),
        max_tokens: Some(512),
        batch_size: Some(1),
        rpm: None,
        tone: Some("casual".into()),
        system_prompt: None,
        thinking: Some(false),
    };
    let provider = ai::make_provider(&cfg).unwrap();
    let req = BatchReq {
        items: vec![BatchItem { id: unit.id, text: masked.text.clone(), context: unit.context.clone() }],
        glossary: vec![],
        source_lang: "Japanese".into(),
        target_lang: "Thai".into(),
        tone: "casual".into(),
        extra_system: None,
        model,
        temperature: 0.0,
        max_tokens: 512,
        thinking: Some(false),
    };

    let client = reqwest::Client::new();
    let out = ai::translate_batch_or_split(provider.as_ref(), &client, None, &req).await;
    let translation = match out.into_iter().next().flatten() {
        Some(m) => protect::restore(&m, &masked.tokens).unwrap_or(m),
        None => {
            eprintln!("translation failed");
            return;
        }
    };

    // Persist exactly like update_unit does (+ TM), into the live DB.
    db::update_unit(&conn, unit.id, Some(&translation), Status::Translated.as_str()).unwrap();
    db::tm_upsert(&conn, &unit.source, &translation).unwrap();

    println!("TH: {translation}\n");
    println!("Saved to project.db (status=Translated).");
    println!("In the app: change the file/status filter (or search the JA text) to reload and see it.");
}
