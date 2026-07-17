//! SQLite persistence for a translation project: units, TM, glossary, meta.
//! All functions take a borrowed [`Connection`]; the project module owns it.

use crate::model::{Status, TransUnit, UnitKind};
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

/// Create tables + indexes if absent. Safe to call on every open.
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        -- Fold the WAL back sooner (default 1000 pages) so a long write-heavy Run
        -- doesn't let the -wal file grow large and slow every read that scans it.
        PRAGMA wal_autocheckpoint = 400;

        CREATE TABLE IF NOT EXISTS unit (
            id          INTEGER PRIMARY KEY,
            file        TEXT NOT NULL,
            pointer     TEXT NOT NULL,
            kind        TEXT NOT NULL,
            context     TEXT,
            grp         TEXT,
            source      TEXT NOT NULL,
            translation TEXT,
            status      TEXT NOT NULL DEFAULT 'Untranslated',
            UNIQUE(file, pointer)
        );

        CREATE TABLE IF NOT EXISTS tm (
            id          INTEGER PRIMARY KEY,
            source      TEXT NOT NULL UNIQUE,
            translation TEXT NOT NULL,
            hits        INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS glossary (
            id             INTEGER PRIMARY KEY,
            term           TEXT NOT NULL,
            translation    TEXT NOT NULL,
            note           TEXT,
            case_sensitive INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT
        );

        -- Speaker → gender, so the AI can pick the right gendered Thai sentence
        -- particle (ครับ / ค่ะ) and first-person pronoun (ผม / ฉัน) per line. `name`
        -- matches a unit's `context` (the speaker). `gender` is male|female|neutral.
        -- `note` is a free-text persona/register hint (who they are, how they speak,
        -- relationships) fed to the Run prompt so pronouns/politeness fit the character.
        CREATE TABLE IF NOT EXISTS character (
            name   TEXT PRIMARY KEY,
            gender TEXT NOT NULL,
            note   TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_unit_status ON unit(status);
        CREATE INDEX IF NOT EXISTS idx_unit_file   ON unit(file);
        -- apply_tm() joins units by source; without this it is O(n^2).
        CREATE INDEX IF NOT EXISTS idx_unit_source ON unit(source);
        "#,
    )?;
    // Migrations for pre-existing project DBs (there is no version framework; every
    // open re-runs init_schema, so these must be idempotent).
    add_column_if_missing(conn, "character", "note", "TEXT")?;
    Ok(())
}

/// Add `<column> <decl>` to `<table>` unless it already exists. Idempotent — safe to
/// run on every open (SQLite `ALTER TABLE ADD COLUMN` errors if the column is present).
fn add_column_if_missing(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|c| c.ok())
        .any(|c| c == column);
    if !exists {
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"), [])?;
    }
    Ok(())
}

pub fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    let v = conn
        .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
            r.get::<_, String>(0)
        })
        .ok();
    Ok(v)
}

pub fn unit_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM unit", [], |r| r.get(0))?)
}

/// Bulk-insert freshly extracted units in one transaction. Existing rows for
/// the same (file, pointer) are left untouched so a re-extract keeps edits.
pub fn insert_units(conn: &mut Connection, units: &[TransUnit]) -> Result<usize> {
    let tx = conn.transaction()?;
    let mut inserted = 0usize;
    {
        let mut stmt = tx.prepare(
            "INSERT OR IGNORE INTO unit(file, pointer, kind, context, grp, source, translation, status)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for u in units {
            inserted += stmt.execute(params![
                u.file,
                u.pointer,
                u.kind.as_str(),
                u.context,
                u.group,
                u.source,
                u.translation,
                u.status.as_str(),
            ])?;
        }
    }
    tx.commit()?;
    Ok(inserted)
}

/// Merge a fresh extraction into an existing project (a "rescan"): insert any new
/// `(file, pointer)` units — e.g. a tier the engine gained since the project was created —
/// and **backfill `context`** on existing units that have none but the fresh scan now
/// resolves one (e.g. a speaker name), all WITHOUT touching an existing unit's
/// translation or status. Returns `(inserted, context_filled)`.
pub fn merge_units(conn: &mut Connection, units: &[TransUnit]) -> Result<(usize, usize)> {
    let tx = conn.transaction()?;
    let mut inserted = 0usize;
    let mut filled = 0usize;
    {
        let mut ins = tx.prepare(
            "INSERT OR IGNORE INTO unit(file, pointer, kind, context, grp, source, translation, status)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        // Only fills an empty context — never overwrites one the user (or a prior scan) set.
        let mut upd = tx.prepare(
            "UPDATE unit SET context = ?3
             WHERE file = ?1 AND pointer = ?2 AND (context IS NULL OR context = '')",
        )?;
        for u in units {
            let n = ins.execute(params![
                u.file, u.pointer, u.kind.as_str(), u.context, u.group, u.source, u.translation, u.status.as_str()
            ])?;
            inserted += n;
            if n == 0 {
                // Existing row: backfill its context if the fresh scan resolved one.
                if let Some(ctx) = u.context.as_deref().filter(|c| !c.trim().is_empty()) {
                    filled += upd.execute(params![u.file, u.pointer, ctx])?;
                }
            }
        }
    }
    tx.commit()?;
    Ok((inserted, filled))
}


/// Filter/paginate the unit grid. All fields optional.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnitFilter {
    pub file: Option<String>,
    pub status: Option<String>,
    pub search: Option<String>,
    /// Which columns the `search` substring runs against. Any of "source",
    /// "translation", "context"; unknown names ignored. None/empty ⇒ the legacy
    /// default of source+translation.
    pub search_fields: Option<Vec<String>>,
    /// Exact speaker/character filter (`context = ?`) — the sidebar's character
    /// dropdown; the value comes from the known cast, not free text.
    pub context: Option<String>,
    pub untranslated_only: Option<bool>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

fn row_to_unit(r: &rusqlite::Row) -> rusqlite::Result<TransUnit> {
    Ok(TransUnit {
        id: r.get("id")?,
        file: r.get("file")?,
        pointer: r.get("pointer")?,
        kind: UnitKind::from_str(&r.get::<_, String>("kind")?),
        context: r.get("context")?,
        group: r.get("grp")?,
        source: r.get("source")?,
        translation: r.get("translation")?,
        status: Status::from_str(&r.get::<_, String>("status")?),
    })
}

/// Build the shared `WHERE …` clause + bound args for the unit-grid filters.
/// Reused by `list_units` and `count_units` so they always agree.
fn unit_where(filter: &UnitFilter) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let mut sql = String::from(" WHERE 1=1");
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(f) = &filter.file {
        sql.push_str(" AND file = ?");
        args.push(Box::new(f.clone()));
    }
    if let Some(s) = &filter.status {
        sql.push_str(" AND status = ?");
        args.push(Box::new(s.clone()));
    }
    if filter.untranslated_only.unwrap_or(false) {
        sql.push_str(" AND status = 'Untranslated'");
    }
    // Exact character (speaker) filter — value picked from the known cast, not typed.
    if let Some(c) = &filter.context {
        if !c.is_empty() {
            sql.push_str(" AND context = ?");
            args.push(Box::new(c.clone()));
        }
    }
    if let Some(q) = &filter.search {
        if !q.is_empty() {
            // Escape LIKE metacharacters so a literal % or _ isn't a wildcard.
            let esc = q
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let like = format!("%{esc}%");
            // Map each requested field to a FIXED column literal (never interpolate
            // the raw string → no injection surface). None/empty/all-unknown ⇒ the
            // legacy source+translation default.
            let cols: Vec<&str> = match &filter.search_fields {
                Some(fields) if !fields.is_empty() => fields
                    .iter()
                    .filter_map(|f| match f.as_str() {
                        "source" => Some("source"),
                        "translation" => Some("translation"),
                        "context" => Some("context"),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let cols = if cols.is_empty() {
                vec!["source", "translation"]
            } else {
                cols
            };
            let ors: Vec<String> = cols
                .iter()
                .map(|c| format!("{c} LIKE ? ESCAPE '\\'"))
                .collect();
            sql.push_str(" AND (");
            sql.push_str(&ors.join(" OR "));
            sql.push(')');
            for _ in &cols {
                args.push(Box::new(like.clone()));
            }
        }
    }
    (sql, args)
}

pub fn list_units(conn: &Connection, filter: &UnitFilter) -> Result<Vec<TransUnit>> {
    let (where_sql, mut args) = unit_where(filter);
    let mut sql = String::from(
        "SELECT id, file, pointer, kind, context, grp, source, translation, status FROM unit",
    );
    sql.push_str(&where_sql);
    sql.push_str(" ORDER BY id");
    // Grid pages are windowed; the ceiling is high so a whole-project translate
    // chunk that passes an explicit large limit is never silently truncated.
    let limit = filter.limit.unwrap_or(500).clamp(1, 200_000);
    let offset = filter.offset.unwrap_or(0).max(0);
    sql.push_str(" LIMIT ? OFFSET ?");
    args.push(Box::new(limit));
    args.push(Box::new(offset));

    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(params.as_slice(), row_to_unit)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Count units matching a filter — the windowed grid's total (scrollbar) size.
/// Same WHERE as `list_units`, no ORDER/LIMIT/OFFSET.
pub fn count_units(conn: &Connection, filter: &UnitFilter) -> Result<i64> {
    let (where_sql, args) = unit_where(filter);
    let sql = format!("SELECT COUNT(*) FROM unit{where_sql}");
    let params: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    Ok(conn.query_row(&sql, params.as_slice(), |r| r.get(0))?)
}

/// Bulk-fill every filter-matching **Untranslated/Failed** unit with its own source
/// text (status → Draft), so a human can hand-edit from the source — useful when
/// heavy inline markup makes an AI pass unreliable (keep the codes, change only the
/// words). Units that already carry a translation (Draft/Translated/Reviewed/Locked)
/// are left untouched. Reuses the shared `unit_where`. Returns the row count changed.
pub fn copy_source_to_translation(conn: &Connection, filter: &UnitFilter) -> Result<usize> {
    let (where_sql, args) = unit_where(filter);
    let sql = format!(
        "UPDATE unit SET translation = source, status = 'Draft'{where_sql} \
         AND status IN ('Untranslated', 'Failed')"
    );
    let params: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    Ok(conn.execute(&sql, params.as_slice())?)
}

/// Load specific units by id (used to translate a selection).
pub fn units_by_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<TransUnit>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let placeholders = std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT id, file, pointer, kind, context, grp, source, translation, status
           FROM unit WHERE id IN ({placeholders}) ORDER BY id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::ToSql> =
        ids.iter().map(|i| i as &dyn rusqlite::ToSql).collect();
    let rows = stmt.query_map(params.as_slice(), row_to_unit)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Load every unit (used by export/inject).
pub fn all_units(conn: &Connection) -> Result<Vec<TransUnit>> {
    let mut stmt = conn.prepare(
        "SELECT id, file, pointer, kind, context, grp, source, translation, status FROM unit ORDER BY id",
    )?;
    let rows = stmt.query_map([], row_to_unit)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Set only a unit's status, leaving its translation untouched. Used to flag a
/// unit `Failed` after an AI attempt without clobbering any existing text.
pub fn set_status(conn: &Connection, id: i64, status: &str) -> Result<()> {
    let status = Status::from_str(status).as_str();
    conn.execute("UPDATE unit SET status = ?1 WHERE id = ?2", params![status, id])?;
    Ok(())
}

pub fn update_unit(conn: &Connection, id: i64, translation: Option<&str>, status: &str) -> Result<()> {
    // Normalize the status so an unknown string can never poison stats()/export.
    let status = Status::from_str(status).as_str();
    conn.execute(
        "UPDATE unit SET translation = ?1, status = ?2 WHERE id = ?3",
        params![translation, status, id],
    )?;
    Ok(())
}

/// Fold the WAL back into the main DB file. Called periodically during a long
/// Run so the `-wal` file doesn't bloat over thousands of continuous writes
/// (which slows every read that must scan it). PASSIVE never blocks on a reader,
/// so it's safe to call mid-Run; it simply does as much as it can and returns.
pub fn wal_checkpoint(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
    Ok(())
}

/// Counts per status plus total, for the dashboard.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub total: i64,
    pub untranslated: i64,
    pub failed: i64,
    pub draft: i64,
    pub translated: i64,
    pub reviewed: i64,
    pub locked: i64,
}

pub fn stats(conn: &Connection) -> Result<Stats> {
    let mut s = Stats::default();
    let mut stmt = conn.prepare("SELECT status, COUNT(*) FROM unit GROUP BY status")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (status, n) = row?;
        s.total += n;
        match status.as_str() {
            "Untranslated" => s.untranslated = n,
            "Failed" => s.failed = n,
            "Draft" => s.draft = n,
            "Translated" => s.translated = n,
            "Reviewed" => s.reviewed = n,
            "Locked" => s.locked = n,
            _ => {}
        }
    }
    Ok(s)
}

/// Distinct files with their unit counts, for the sidebar filter.
#[derive(Debug, Serialize)]
pub struct FileCount {
    pub file: String,
    pub count: i64,
}

pub fn files_with_counts(conn: &Connection) -> Result<Vec<FileCount>> {
    let mut stmt =
        conn.prepare("SELECT file, COUNT(*) FROM unit GROUP BY file ORDER BY file")?;
    let rows = stmt.query_map([], |r| {
        Ok(FileCount {
            file: r.get(0)?,
            count: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

// ---------------------------------------------------------------------------
// Translation memory
// ---------------------------------------------------------------------------

/// Remember a confirmed translation so identical source strings can reuse it.
pub fn tm_upsert(conn: &Connection, source: &str, translation: &str) -> Result<()> {
    if source.is_empty() || translation.is_empty() {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO tm(source, translation, hits) VALUES(?1, ?2, 0)
         ON CONFLICT(source) DO UPDATE SET translation = excluded.translation",
        params![source, translation],
    )?;
    Ok(())
}

pub fn tm_lookup(conn: &Connection, source: &str) -> Result<Option<String>> {
    let v = conn
        .query_row(
            "SELECT translation FROM tm WHERE source = ?1",
            params![source],
            |r| r.get::<_, String>(0),
        )
        .ok();
    Ok(v)
}

/// Fill every untranslated unit whose source exactly matches a TM entry (or a
/// sibling unit that is already translated). Fills as `Draft`. Returns the
/// number of units filled.
pub fn apply_tm(conn: &mut Connection) -> Result<usize> {
    let tx = conn.transaction()?;
    // 1) From the persisted TM table.
    let n1 = tx.execute(
        "UPDATE unit
            SET translation = (SELECT translation FROM tm WHERE tm.source = unit.source),
                status = 'Draft'
          WHERE status = 'Untranslated'
            AND EXISTS (SELECT 1 FROM tm WHERE tm.source = unit.source)",
        [],
    )?;
    // 2) From sibling units already translated in this project (duplicates).
    let n2 = tx.execute(
        "UPDATE unit
            SET translation = (
                    SELECT s.translation FROM unit s
                     WHERE s.source = unit.source
                       AND s.translation IS NOT NULL
                       AND s.status <> 'Untranslated'
                     LIMIT 1
                ),
                status = 'Draft'
          WHERE status = 'Untranslated'
            AND EXISTS (
                    SELECT 1 FROM unit s
                     WHERE s.source = unit.source
                       AND s.translation IS NOT NULL
                       AND s.status <> 'Untranslated'
                )",
        [],
    )?;
    tx.commit()?;
    Ok(n1 + n2)
}

// ---------------------------------------------------------------------------
// Glossary
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryEntry {
    pub id: i64,
    pub term: String,
    pub translation: String,
    pub note: Option<String>,
    pub case_sensitive: bool,
}

pub fn glossary_list(conn: &Connection) -> Result<Vec<GlossaryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, term, translation, note, case_sensitive FROM glossary ORDER BY term",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(GlossaryEntry {
            id: r.get(0)?,
            term: r.get(1)?,
            translation: r.get(2)?,
            note: r.get(3)?,
            case_sensitive: r.get::<_, i64>(4)? != 0,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn glossary_add(
    conn: &Connection,
    term: &str,
    translation: &str,
    note: Option<&str>,
    case_sensitive: bool,
) -> Result<i64> {
    if term.trim().is_empty() || translation.trim().is_empty() {
        return Err(anyhow!("glossary term and translation must not be empty"));
    }
    conn.execute(
        "INSERT INTO glossary(term, translation, note, case_sensitive) VALUES(?1, ?2, ?3, ?4)",
        params![term, translation, note, case_sensitive as i64],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn glossary_update(
    conn: &Connection,
    id: i64,
    term: &str,
    translation: &str,
    note: Option<&str>,
    case_sensitive: bool,
) -> Result<()> {
    conn.execute(
        "UPDATE glossary SET term=?1, translation=?2, note=?3, case_sensitive=?4 WHERE id=?5",
        params![term, translation, note, case_sensitive as i64, id],
    )?;
    Ok(())
}

pub fn glossary_delete(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM glossary WHERE id=?1", params![id])?;
    Ok(())
}

// --- characters (speaker → gender) ------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Character {
    pub name: String,
    /// "male" | "female" | "neutral".
    pub gender: String,
    /// Free-text persona/register hint (who they are, how they speak, relationships).
    /// Empty when unset. Fed to the Run prompt via `persona_directive`.
    pub note: String,
}

pub fn characters_list(conn: &Connection) -> Result<Vec<Character>> {
    let mut stmt =
        conn.prepare("SELECT name, gender, COALESCE(note, '') FROM character ORDER BY name")?;
    let rows = stmt.query_map([], |r| {
        Ok(Character { name: r.get(0)?, gender: r.get(1)?, note: r.get(2)? })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Upsert one speaker's gender (empty gender clears the row instead).
pub fn character_set(conn: &Connection, name: &str, gender: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(());
    }
    if gender.trim().is_empty() {
        conn.execute("DELETE FROM character WHERE name=?1", params![name])?;
    } else {
        conn.execute(
            "INSERT INTO character(name, gender) VALUES(?1, ?2)
             ON CONFLICT(name) DO UPDATE SET gender = excluded.gender",
            params![name, gender.trim()],
        )?;
    }
    Ok(())
}

/// Upsert one speaker's persona/register `note`, preserving any existing gender (or an
/// empty gender for a brand-new speaker row). Clears the note when empty, and deletes the
/// row entirely if that leaves both gender and note empty (matches `character_set` cleanup).
pub fn character_set_note(conn: &Connection, name: &str, note: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(());
    }
    let note = note.trim();
    conn.execute(
        "INSERT INTO character(name, gender, note) VALUES(?1, '', ?2)
         ON CONFLICT(name) DO UPDATE SET note = excluded.note",
        params![name, note],
    )?;
    // Drop a row that now carries no information at all.
    conn.execute(
        "DELETE FROM character WHERE name=?1 AND (gender IS NULL OR gender='')
           AND (note IS NULL OR note='')",
        params![name],
    )?;
    Ok(())
}

/// Upsert several (name, gender, note) rows in one transaction (auto-classify output).
pub fn characters_set_bulk(
    conn: &mut Connection,
    items: &[(String, String, String)],
) -> Result<usize> {
    let tx = conn.transaction()?;
    let mut n = 0usize;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO character(name, gender, note) VALUES(?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET gender = excluded.gender, note = excluded.note",
        )?;
        for (name, gender, note) in items {
            let (name, gender, note) = (name.trim(), gender.trim(), note.trim());
            if name.is_empty() || gender.is_empty() {
                continue;
            }
            stmt.execute(params![name, gender, note])?;
            n += 1;
        }
    }
    tx.commit()?;
    Ok(n)
}

/// Upsert several (name, note) rows in one transaction, setting ONLY `note` and
/// preserving each existing row's gender (a brand-new speaker row gets an empty gender).
/// Used by the AI persona-fill pass so it never overwrites a manually chosen gender.
/// Skips blank names and blank notes.
pub fn characters_set_notes_bulk(conn: &mut Connection, items: &[(String, String)]) -> Result<usize> {
    let tx = conn.transaction()?;
    let mut n = 0usize;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO character(name, gender, note) VALUES(?1, '', ?2)
             ON CONFLICT(name) DO UPDATE SET note = excluded.note",
        )?;
        for (name, note) in items {
            let (name, note) = (name.trim(), note.trim());
            if name.is_empty() || note.is_empty() {
                continue;
            }
            stmt.execute(params![name, note])?;
            n += 1;
        }
    }
    tx.commit()?;
    Ok(n)
}

/// Delete every stored character→gender row (a fresh start before re-classifying).
pub fn characters_clear(conn: &Connection) -> Result<usize> {
    Ok(conn.execute("DELETE FROM character", [])? )
}

/// Distinct speaker names seen on Dialogue units (the `context` field), so the UI
/// and the auto-classify pass know who speaks. Skips empties.
pub fn distinct_speakers(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT context FROM unit
         WHERE context IS NOT NULL AND context <> '' AND kind = 'Dialogue'
         ORDER BY context",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// For each distinct dialogue speaker, up to `per` of their source lines joined by
/// newlines — the corpus the gender-classify pass reads to judge each speaker.
pub fn speaker_samples(conn: &Connection, per: usize) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT context, source FROM unit
         WHERE context IS NOT NULL AND context <> '' AND kind = 'Dialogue'
           AND source IS NOT NULL AND source <> ''
         ORDER BY context, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    // Group in order; keep the first `per` lines per speaker.
    let mut out: Vec<(String, String)> = Vec::new();
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for row in rows {
        let (name, source) = row?;
        let c = counts.entry(name.clone()).or_insert(0);
        if *c >= per {
            continue;
        }
        *c += 1;
        match out.last_mut() {
            Some((n, s)) if *n == name => {
                s.push('\n');
                s.push_str(source.trim());
            }
            _ => out.push((name, source.trim().to_string())),
        }
    }
    // Rows arrive grouped by context, but a speaker seen again after another would
    // start a second entry; merge any splits so each speaker appears once.
    let mut merged: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for (n, s) in out {
        merged.entry(n).and_modify(|e| { e.push('\n'); e.push_str(&s); }).or_insert(s);
    }
    Ok(merged.into_iter().collect())
}

/// Insert several glossary entries in one transaction. Skips empties and any
/// term already in the glossary (case-insensitive), so a re-add — or an
/// accidental double-click of "Add selected" — never creates duplicates. Also
/// dedups within the batch itself.
pub fn glossary_add_bulk(conn: &mut Connection, items: &[(String, String)]) -> Result<usize> {
    let mut seen: std::collections::HashSet<String> = {
        let mut stmt = conn.prepare("SELECT term FROM glossary")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.filter_map(|r| r.ok()).map(|t| t.trim().to_lowercase()).collect()
    };
    let tx = conn.transaction()?;
    let mut added = 0usize;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO glossary(term, translation, note, case_sensitive) VALUES(?1, ?2, NULL, 0)",
        )?;
        for (term, translation) in items {
            let key = term.trim().to_lowercase();
            if key.is_empty() || translation.trim().is_empty() || !seen.insert(key) {
                continue;
            }
            added += stmt.execute(params![term, translation])?;
        }
    }
    tx.commit()?;
    Ok(added)
}

/// A proposed glossary entry mined from the game: a proper noun (character /
/// enemy name) or a System term, with any translation the game already has.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossCandidate {
    pub term: String,
    pub translation: Option<String>,
    pub kind: String,
    pub count: i64,
}

/// Mine glossary candidates from the extracted units: actor names/nicknames,
/// enemy names, and System terms. Excludes anything already in the glossary and
/// pre-fills the translation from an already-translated instance when present.
pub fn suggest_glossary(conn: &Connection) -> Result<Vec<GlossCandidate>> {
    // Prefill each candidate's translation from a translated unit if one exists,
    // else from TM — that is where glossary auto-translate persists its results
    // (via remember_texts), so previously-translated terms come back filled and
    // are never re-billed.
    let mut stmt = conn.prepare(
        "SELECT u.source,
                COALESCE(MAX(u.translation), t.translation) AS tr,
                MIN(u.kind) AS k,
                COUNT(*) AS c
           FROM unit u
           LEFT JOIN tm t ON t.source = u.source
          WHERE u.source <> ''
            AND ( (u.file = 'Actors.json'  AND u.kind IN ('Name','Nickname'))
               OR (u.file = 'Enemies.json' AND u.kind = 'Name')
               OR (u.file = 'Classes.json' AND u.kind = 'Name')
               OR u.kind = 'Term' )
            AND u.source NOT IN (SELECT term FROM glossary)
          GROUP BY u.source
          ORDER BY c DESC, u.source
          LIMIT 500",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(GlossCandidate {
            term: r.get(0)?,
            translation: r.get::<_, Option<String>>(1)?,
            kind: r.get(2)?,
            count: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Sample distinct narrative source lines for AI glossary mining: the most
/// frequent dialogue / choice / description / map-name text, capped at
/// `max_lines`. Frequency-ranked so recurring proper nouns surface, with longer
/// lines breaking ties for richer context. The structured Name/Term fields are
/// already covered by [`suggest_glossary`]; this feeds the model the free text
/// where that heuristic is blind (names spoken in dialogue, place names, …).
pub fn sample_text_for_mining(conn: &Connection, max_lines: i64) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT source, COUNT(*) AS c
           FROM unit
          WHERE source <> ''
            AND kind IN ('Dialogue','ScrollText','Choice','Description','Profile','MapName')
          GROUP BY source
          ORDER BY c DESC, LENGTH(source) DESC
          LIMIT ?",
    )?;
    let rows = stmt.query_map([max_lines], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The unit kinds that carry free narrative text worth sampling for AI context /
/// glossary work (as a SQL `IN (...)` list).
const NARRATIVE_KINDS: &str =
    "'Dialogue','ScrollText','Choice','Description','Profile','MapName'";

/// Build a diverse, code-stripped text sample for AI game-context drafting. Unlike
/// [`sample_text_for_mining`] (frequency-ranked, which over-weights repeated UI
/// strings), this mixes three buckets so the brief sees real narrative: the opening
/// passages (setting / character intros), the longest lines (substantive prose, not
/// menu labels), and an even spread across the whole game (mid / late plot). Lines
/// are code-stripped ([`crate::engine::protect::strip_codes`]), de-duplicated,
/// short / UI lines (<3 words) dropped, and interleaved round-robin so all three
/// buckets survive the `char_budget` cap that keeps the sample inside a small
/// context window.
pub fn sample_corpus(conn: &Connection, engine_id: &str, char_budget: usize) -> Result<Vec<String>> {
    let per_bucket = 600usize;
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM unit WHERE source<>'' AND kind IN ({NARRATIVE_KINDS})"),
        [],
        |r| r.get(0),
    )?;
    let stride = (total as usize / per_bucket).max(1);

    let fetch = |sql: String| -> Result<Vec<String>> {
        let mut st = conn.prepare(&sql)?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    };
    let intro = fetch(format!(
        "SELECT source FROM unit WHERE source<>'' AND kind IN ({NARRATIVE_KINDS}) \
         ORDER BY id ASC LIMIT {per_bucket}"
    ))?;
    let longest = fetch(format!(
        "SELECT source FROM unit WHERE source<>'' AND kind IN ({NARRATIVE_KINDS}) \
         ORDER BY LENGTH(source) DESC LIMIT {per_bucket}"
    ))?;
    // Stratified: every `stride`-th row across the whole id order.
    let spread = fetch(format!(
        "SELECT source FROM (SELECT source, ROW_NUMBER() OVER (ORDER BY id) AS rn \
           FROM unit WHERE source<>'' AND kind IN ({NARRATIVE_KINDS})) \
         WHERE (rn - 1) % {stride} = 0 LIMIT {per_bucket}"
    ))?;

    let mut buckets = [intro.into_iter(), longest.into_iter(), spread.into_iter()];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();
    let mut used = 0usize;
    loop {
        let mut drew = false;
        for b in buckets.iter_mut() {
            let Some(raw) = b.next() else { continue };
            drew = true;
            let clean = crate::engine::protect::strip_codes(engine_id, &raw);
            let clean = clean.trim();
            // Drop UI labels / one-word choices. Count letters, not whitespace
            // words — CJK text has no spaces, so a whole sentence is one "word";
            // splitting on whitespace would wrongly discard every Chinese/Japanese
            // line as too short (→ an empty sample, "no context could be drafted").
            if clean.chars().filter(|c| c.is_alphabetic()).count() < 6 {
                continue;
            }
            if !seen.insert(clean.to_lowercase()) {
                continue;
            }
            if used + clean.len() > char_budget && !out.is_empty() {
                return Ok(out); // budget reached
            }
            used += clean.len() + 1;
            out.push(clean.to_string());
        }
        if !drew {
            break; // all buckets drained
        }
    }
    Ok(out)
}

/// A glossary candidate mined locally from the *whole* game (not a sample): a
/// proper-noun-shaped term, its total occurrences, how many are mid-sentence (a
/// strong proper-noun signal — see [`mine_glossary_candidates`]), and one short
/// example line for the classifier's context.
#[derive(Debug, Clone)]
pub struct MinedCandidate {
    pub term: String,
    pub count: i64,
    pub mid: i64,
    pub example: String,
}

/// A term must recur at least this many times across the game to be a candidate —
/// a one-off capitalized word is almost never a glossary term.
const MIN_TERM_FREQ: i64 = 3;

/// Scan every unit's source (codes stripped) for proper-noun-shaped terms and rank
/// them by how often they appear *mid-sentence* (where a capitalized word is a name,
/// not just a sentence start), then by raw frequency. Also folds in every distinct
/// **speaker name** (the `context` column) — proper nouns by construction, and the
/// primary name signal for a language without capitalization (Japanese/Chinese),
/// where the source scan alone finds nothing. Reads the whole DB cheaply (no AI) so
/// the returned shortlist covers the entire game; the caller sends it to a model
/// only to filter + classify + translate.
pub fn mine_glossary_candidates(
    conn: &Connection,
    engine_id: &str,
    limit: usize,
) -> Result<Vec<MinedCandidate>> {
    let existing: std::collections::HashSet<String> = glossary_list(conn)?
        .into_iter()
        .map(|g| g.term.trim().to_lowercase())
        .collect();

    struct Agg {
        surface: String,
        total: i64,
        mid: i64,
        example: String,
    }
    let mut agg: std::collections::HashMap<String, Agg> = std::collections::HashMap::new();

    let mut stmt = conn.prepare(
        "SELECT source, COUNT(*) AS c
           FROM unit
          WHERE source <> ''
            AND kind IN ('Dialogue','ScrollText','Choice','Description','Profile',
                         'MapName','Name','Nickname')
          GROUP BY source",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    for row in rows {
        let (source, count) = row?;
        let clean = crate::engine::protect::strip_codes(engine_id, &source);
        let example = clean.trim();
        for (term, mid) in proper_nouns(&clean) {
            let key = term.to_lowercase();
            if existing.contains(&key) {
                continue;
            }
            let e = agg.entry(key).or_insert_with(|| Agg {
                surface: term.clone(),
                total: 0,
                mid: 0,
                example: example.to_string(),
            });
            e.total += count;
            if mid {
                e.mid += count;
            }
            // Prefer a shorter example that still contains the term — clearer context.
            if example.len() < e.example.len() && example.len() >= term.len() {
                e.example = example.to_string();
            }
        }
    }

    // Speaker names (the `context` column) are proper nouns by construction — each
    // engine records the current speaker there (RPGMaker's 101 header, Hendrix's
    // Name column, Ren'Py's say prefix, Tyrano's `#name`). Add them directly: no
    // capitalization is needed, so this is the primary name signal for CJK games the
    // Latin proper-noun scan above can't read. A speaker that is only a control code
    // (e.g. RPGMaker's `\N[1]` actor reference) strips to empty / a bare code and is
    // skipped. Weighted like a mid-sentence hit so real names outrank incidental
    // frequency matches.
    let mut spk = conn.prepare(
        "SELECT context, COUNT(*) AS c
           FROM unit
          WHERE context IS NOT NULL AND context <> ''
          GROUP BY context",
    )?;
    let spk_rows = spk.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    for row in spk_rows {
        let (name, count) = row?;
        let clean = crate::engine::protect::strip_codes(engine_id, &name);
        let clean = clean.trim();
        if clean.is_empty() || clean.starts_with('\\') {
            continue; // pure control code / placeholder speaker
        }
        let key = clean.to_lowercase();
        if existing.contains(&key) {
            continue;
        }
        let e = agg.entry(key).or_insert_with(|| Agg {
            surface: clean.to_string(),
            total: 0,
            mid: 0,
            example: clean.to_string(),
        });
        e.total += count;
        e.mid += count;
    }

    let mut out: Vec<MinedCandidate> = agg
        .into_values()
        .filter(|a| a.total >= MIN_TERM_FREQ)
        .map(|a| MinedCandidate {
            term: a.surface,
            count: a.total,
            mid: a.mid,
            example: a.example,
        })
        .collect();
    // Mid-sentence hits first (names), then raw frequency, then stable by term.
    out.sort_by(|a, b| {
        b.mid
            .cmp(&a.mid)
            .then(b.count.cmp(&a.count))
            .then(a.term.cmp(&b.term))
    });
    out.truncate(limit);
    Ok(out)
}

/// Extract proper-noun-shaped phrases from one line: maximal runs of consecutive
/// capitalized words (≤4), with leading/trailing stopwords trimmed and a trailing
/// possessive (`'s`) removed. Each result is `(term, mid_sentence)` where
/// `mid_sentence` is true when the run does not begin its sentence — the signal
/// that distinguishes a name from a merely sentence-initial capital.
fn proper_nouns(line: &str) -> Vec<(String, bool)> {
    // Tokenize into (word, capitalized, sentence_start, joinable) where `joinable`
    // means only spaces separated this word from the previous one — so a comma or
    // period breaks a phrase (`Later, Karen` is two terms, not one).
    struct W {
        text: String,
        cap: bool,
        ss: bool,
        joinable: bool,
    }
    let mut words: Vec<W> = Vec::new();
    let mut cur = String::new();
    let mut sentence_start = true;
    let mut gap_clean = true; // separator since the last word held only spaces
    let mut joinable = true;
    for ch in line.chars() {
        if ch.is_alphabetic() || ((ch == '\'' || ch == '\u{2019}') && !cur.is_empty()) {
            if cur.is_empty() {
                joinable = gap_clean;
            }
            cur.push(ch);
        } else {
            if !cur.is_empty() {
                let cap = cur.chars().next().unwrap().is_uppercase();
                words.push(W { text: std::mem::take(&mut cur), cap, ss: sentence_start, joinable });
                sentence_start = false;
                gap_clean = true;
            }
            if matches!(ch, '.' | '!' | '?' | ':' | ';' | '\n' | '\r') {
                sentence_start = true;
            }
            if !matches!(ch, ' ' | '\t') {
                gap_clean = false;
            }
        }
    }
    if !cur.is_empty() {
        let cap = cur.chars().next().unwrap().is_uppercase();
        words.push(W { text: cur, cap, ss: sentence_start, joinable });
    }

    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        if !words[i].cap {
            i += 1;
            continue;
        }
        let group_start_ss = words[i].ss;
        let start = i;
        let mut phrase: Vec<String> = vec![words[i].text.clone()];
        i += 1;
        while i < words.len() && words[i].cap && words[i].joinable && (i - start) < 4 {
            phrase.push(words[i].text.clone());
            i += 1;
        }
        // Trim stopwords off both ends; a trimmed leading stopword means the real
        // term sits mid-sentence even if the sentence itself started capitalized.
        let mut trimmed_leading = false;
        while phrase.first().is_some_and(|w| is_stopword(w)) {
            phrase.remove(0);
            trimmed_leading = true;
        }
        while phrase.last().is_some_and(|w| is_stopword(w)) {
            phrase.pop();
        }
        if phrase.is_empty() {
            continue;
        }
        let mut term = phrase.join(" ");
        for suf in ["'s", "\u{2019}s", "'S", "\u{2019}S"] {
            if let Some(t) = term.strip_suffix(suf) {
                term = t.to_string();
                break;
            }
        }
        let term = term.trim().to_string();
        if term.chars().filter(|c| c.is_alphabetic()).count() < 2 {
            continue; // a lone initial isn't a term
        }
        out.push((term, !group_start_ss || trimmed_leading));
    }
    out
}

/// Common English words that are capitalized only because they start a sentence
/// (or are pronouns/particles) — never glossary terms.
const STOPWORDS: &[&str] = &[
    "a", "about", "after", "again", "against", "ah", "all", "also", "always", "am", "an", "and",
    "another", "any", "anything", "are", "as", "at", "away", "be", "because", "been", "before",
    "being", "but", "by", "came", "can", "cannot", "come", "could", "did", "do", "does", "doing",
    "don", "done", "down", "each", "even", "ever", "every", "everyone", "everything", "few", "for",
    "from", "get", "give", "go", "going", "good", "got", "had", "has", "have", "he", "hello", "her",
    "here", "hers", "herself", "hey", "hi", "him", "himself", "his", "how", "however", "i", "if",
    "in", "into", "is", "it", "its", "itself", "just", "keep", "know", "let", "like", "look",
    "made", "make", "many", "may", "maybe", "me", "might", "mine", "more", "most", "much", "must",
    "my", "myself", "never", "new", "no", "not", "nothing", "now", "of", "off", "oh", "ok", "okay",
    "on", "once", "one", "only", "onto", "or", "other", "our", "ours", "out", "over", "please",
    "really", "said", "say", "see", "she", "should", "since", "so", "some", "someone", "something",
    "sorry", "still", "such", "sure", "take", "than", "thank", "thanks", "that", "the", "their",
    "theirs", "them", "then", "there", "these", "they", "thing", "things", "this", "those",
    "though", "through", "thus", "to", "together", "too", "up", "upon", "us", "very", "want", "was",
    "way", "we", "well", "were", "what", "when", "where", "which", "while", "who", "whom", "whose",
    "why", "will", "with", "without", "would", "yeah", "yes", "yet", "you", "your", "yours",
    "yourself",
];

fn is_stopword(word: &str) -> bool {
    use std::sync::OnceLock;
    static SET: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| STOPWORDS.iter().copied().collect())
        .contains(word.to_lowercase().as_str())
}

/// A glossary violation: a translated unit whose source uses a glossary term
/// but whose translation lacks the mapped wording.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LintWarning {
    pub unit_id: i64,
    pub file: String,
    pub term: String,
    pub expected: String,
}

/// Check every translated unit against the glossary. Case-insensitive terms
/// match regardless of case; the expected translation is always required verbatim.
pub fn glossary_lint(conn: &Connection) -> Result<Vec<LintWarning>> {
    let glossary = glossary_list(conn)?;
    if glossary.is_empty() {
        return Ok(vec![]);
    }
    let mut stmt = conn.prepare(
        "SELECT id, file, source, translation FROM unit
          WHERE translation IS NOT NULL AND translation <> '' AND status <> 'Untranslated'",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;

    let mut warnings = Vec::new();
    for row in rows {
        let (id, file, source, translation) = row?;
        for g in &glossary {
            let present = if g.case_sensitive {
                source.contains(&g.term)
            } else {
                source.to_lowercase().contains(&g.term.to_lowercase())
            };
            if present && !translation.contains(&g.translation) {
                warnings.push(LintWarning {
                    unit_id: id,
                    file: file.clone(),
                    term: g.term.clone(),
                    expected: g.translation.clone(),
                });
            }
        }
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Status, TransUnit, UnitKind};

    fn mem_db(units: &[TransUnit]) -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        insert_units(&mut conn, units).unwrap();
        conn
    }

    fn unit(file: &str, ptr: &str, source: &str, status: Status) -> TransUnit {
        let mut u = TransUnit::new(file, ptr, UnitKind::Dialogue, source);
        u.status = status;
        u
    }

    #[test]
    fn count_units_matches_filters_and_list() {
        let units: Vec<_> = (0..250)
            .map(|i| {
                let status = if i % 5 == 0 { Status::Failed } else { Status::Untranslated };
                let file = if i < 100 { "A.json" } else { "B.json" };
                unit(file, &format!("/p/{i}"), &format!("line {i}"), status)
            })
            .collect();
        let conn = mem_db(&units);
        let big = |f: UnitFilter| UnitFilter { limit: Some(10_000), ..f };

        // Whole-table count.
        assert_eq!(count_units(&conn, &UnitFilter::default()).unwrap(), 250);
        // Status / file counts, and each agrees with list_units.
        for f in [
            UnitFilter { status: Some("Failed".into()), ..Default::default() },
            UnitFilter { file: Some("A.json".into()), ..Default::default() },
            UnitFilter { search: Some("line 1".into()), ..Default::default() },
        ] {
            let c = count_units(&conn, &f).unwrap();
            let n = list_units(&conn, &big(f)).unwrap().len() as i64;
            assert_eq!(c, n, "count_units must match list_units row count");
        }
        assert_eq!(
            count_units(&conn, &UnitFilter { status: Some("Failed".into()), ..Default::default() }).unwrap(),
            50
        );
        assert_eq!(
            count_units(&conn, &UnitFilter { file: Some("A.json".into()), ..Default::default() }).unwrap(),
            100
        );
    }

    #[test]
    fn search_fields_and_context_filter() {
        // Three units where "Alice" appears in a different column each time, so a
        // field-targeted search must be able to tell them apart.
        let mut in_source = unit("A.json", "/p/1", "Alice draws her sword", Status::Untranslated);
        in_source.context = Some("Narrator".into());

        let mut in_trans = unit("A.json", "/p/2", "he swings", Status::Draft);
        in_trans.context = Some("Bob".into());
        in_trans.translation = Some("Alice parries".into());

        let mut in_ctx = unit("A.json", "/p/3", "hello there", Status::Draft);
        in_ctx.context = Some("Alice".into());

        let conn = mem_db(&[in_source, in_trans, in_ctx]);
        let count = |f: UnitFilter| count_units(&conn, &f).unwrap();
        let fields = |v: &[&str]| Some(v.iter().map(|s| s.to_string()).collect::<Vec<_>>());

        // Default (no search_fields) = legacy source OR translation → the source hit
        // and the translation hit, but NOT the context-only hit.
        assert_eq!(count(UnitFilter { search: Some("Alice".into()), ..Default::default() }), 2);
        // Empty search_fields behaves like the default too.
        assert_eq!(
            count(UnitFilter { search: Some("Alice".into()), search_fields: fields(&[]), ..Default::default() }),
            2
        );
        // Source only → excludes the translation-only hit.
        assert_eq!(
            count(UnitFilter { search: Some("Alice".into()), search_fields: fields(&["source"]), ..Default::default() }),
            1
        );
        // Translation only → just the translation hit.
        assert_eq!(
            count(UnitFilter { search: Some("Alice".into()), search_fields: fields(&["translation"]), ..Default::default() }),
            1
        );
        // Context only → just the speaker hit.
        assert_eq!(
            count(UnitFilter { search: Some("Alice".into()), search_fields: fields(&["context"]), ..Default::default() }),
            1
        );
        // All three columns → every unit mentioning "Alice".
        assert_eq!(
            count(UnitFilter {
                search: Some("Alice".into()),
                search_fields: fields(&["source", "translation", "context"]),
                ..Default::default()
            }),
            3
        );
        // An all-unknown field list falls back to the default rather than erroring.
        assert_eq!(
            count(UnitFilter { search: Some("Alice".into()), search_fields: fields(&["bogus"]), ..Default::default() }),
            2
        );

        // Exact character filter → only that speaker's lines, regardless of text.
        assert_eq!(count(UnitFilter { context: Some("Alice".into()), ..Default::default() }), 1);
        assert_eq!(count(UnitFilter { context: Some("Bob".into()), ..Default::default() }), 1);
        assert_eq!(count(UnitFilter { context: Some("Nobody".into()), ..Default::default() }), 0);
        // Character filter AND a text query combine.
        assert_eq!(
            count(UnitFilter {
                context: Some("Bob".into()),
                search: Some("Alice".into()),
                ..Default::default()
            }),
            1
        );
        assert_eq!(
            count(UnitFilter {
                context: Some("Alice".into()),
                search: Some("zzz".into()),
                ..Default::default()
            }),
            0
        );

        // count_units must always agree with list_units row count.
        let f = UnitFilter { search: Some("Alice".into()), search_fields: fields(&["context"]), limit: Some(1000), ..Default::default() };
        assert_eq!(count_units(&conn, &f).unwrap(), list_units(&conn, &f).unwrap().len() as i64);
    }

    #[test]
    fn list_units_windows_are_ordered_and_cover_everything() {
        let units: Vec<_> = (0..500)
            .map(|i| unit("A.json", &format!("/p/{i}"), &format!("s{i}"), Status::Untranslated))
            .collect();
        let conn = mem_db(&units);
        assert_eq!(count_units(&conn, &UnitFilter::default()).unwrap(), 500);

        // Reassemble the list from offset/limit windows.
        let mut seen: Vec<i64> = Vec::new();
        let mut off = 0i64;
        loop {
            let f = UnitFilter { limit: Some(120), offset: Some(off), ..Default::default() };
            let page = list_units(&conn, &f).unwrap();
            if page.is_empty() {
                break;
            }
            assert!(page.windows(2).all(|w| w[0].id < w[1].id), "page must be id-ordered");
            seen.extend(page.iter().map(|u| u.id));
            off += 120;
        }
        // Every row exactly once, strictly increasing (no overlap, no gap).
        assert_eq!(seen.len(), 500);
        assert!(seen.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn glossary_add_bulk_never_duplicates() {
        let mut conn = mem_db(&[]);
        let set = [
            ("Stamina".to_string(), "พลังกาย".to_string()),
            ("EXP".to_string(), "ค่าประสบการณ์".to_string()),
        ];
        // First add takes both.
        assert_eq!(glossary_add_bulk(&mut conn, &set).unwrap(), 2);
        // Re-adding the same set (e.g. a double-clicked "Add selected") adds none.
        assert_eq!(glossary_add_bulk(&mut conn, &set).unwrap(), 0);
        // Case-insensitive vs existing, and intra-batch: "stamina" is a dup of the
        // stored "Stamina"; "HP" appears twice in one batch → inserted once.
        let batch = [
            ("stamina".to_string(), "x".to_string()),
            ("HP".to_string(), "พลังชีวิต".to_string()),
            ("HP".to_string(), "y".to_string()),
        ];
        assert_eq!(glossary_add_bulk(&mut conn, &batch).unwrap(), 1);
        assert_eq!(glossary_list(&conn).unwrap().len(), 3);
    }

    #[test]
    fn proper_nouns_names_and_stopword_trimming() {
        // A mid-sentence name is flagged mid=true; a sentence-initial one is not.
        let a = proper_nouns("I met Karen at the tower.");
        assert!(a.iter().any(|(t, mid)| t == "Karen" && *mid));
        let b = proper_nouns("Karen went home.");
        assert!(b.iter().any(|(t, mid)| t == "Karen" && !*mid));

        // A leading stopword ("The") is trimmed and never a term on its own.
        assert!(proper_nouns("The dog ran off.").is_empty());
        // Multi-word names group together; a trimmed leading stopword ⇒ mid.
        let c = proper_nouns("We saw Karen Yu today.");
        assert!(c.iter().any(|(t, mid)| t == "Karen Yu" && *mid));
        // Possessive is stripped.
        let d = proper_nouns("That is Karen's office.");
        assert!(d.iter().any(|(t, _)| t == "Karen"));
    }

    #[test]
    fn is_stopword_flags_common_words_case_insensitively() {
        assert!(is_stopword("The") && is_stopword("you") && is_stopword("AND"));
        assert!(!is_stopword("Karen") && !is_stopword("Corpo"));
    }

    #[test]
    fn mine_glossary_candidates_surfaces_names_not_stopwords() {
        // "Karen" recurs (count 3 ⇒ ≥ MIN_TERM_FREQ); the "The end." line yields none.
        let units = vec![
            unit("A.json", "/1", "I met Karen at the tower.", Status::Untranslated),
            unit("A.json", "/2", "Later, Karen smiled warmly.", Status::Untranslated),
            unit("A.json", "/3", "Everyone trusted Karen deeply.", Status::Untranslated),
            unit("A.json", "/4", "The end.", Status::Untranslated),
        ];
        let conn = mem_db(&units);
        let got = mine_glossary_candidates(&conn, "rpgmaker-mvmz", 50).unwrap();
        let karen = got.iter().find(|c| c.term == "Karen").expect("Karen mined");
        assert_eq!(karen.count, 3);
        assert!(karen.mid >= 3, "all three occurrences are mid-sentence");
        // Common words never surface as candidates.
        assert!(!got.iter().any(|c| c.term.eq_ignore_ascii_case("the")));
        assert!(!got.iter().any(|c| c.term.eq_ignore_ascii_case("everyone")));
    }

    #[test]
    fn mine_glossary_candidates_empty_for_capitalless_text() {
        // Japanese dialogue with no speaker column ⇒ the Latin scan finds nothing and
        // there are no speaker names ⇒ no candidates ⇒ caller uses AI fallback.
        let units = vec![
            unit("A.json", "/1", "カレンは笑った。", Status::Untranslated),
            unit("A.json", "/2", "カレンは強い。", Status::Untranslated),
            unit("A.json", "/3", "カレンは行く。", Status::Untranslated),
        ];
        let conn = mem_db(&units);
        assert!(mine_glossary_candidates(&conn, "rpgmaker-mvmz", 50).unwrap().is_empty());
    }

    #[test]
    fn mine_glossary_candidates_surfaces_cjk_speaker_names() {
        // A Japanese game: dialogue bodies yield no Latin proper nouns, but the
        // speaker column carries the character names. Those must surface (the primary
        // glossary signal for CJK), while a control-code-only speaker (\N[1]) is
        // skipped rather than mined as a bogus term.
        let spk = |ptr: &str, src: &str, name: &str| {
            let mut u = TransUnit::new("A.json", ptr, UnitKind::Dialogue, src)
                .with_context(Some(name.to_string()));
            u.status = Status::Untranslated;
            u
        };
        let units = vec![
            spk("/1", "こんにちは。", "ハルカ"),
            spk("/2", "元気ですか。", "ハルカ"),
            spk("/3", "また明日。", "ハルカ"),
            spk("/4", "了解した。", "部長"),
            spk("/5", "よろしい。", "部長"),
            spk("/6", "帰りたまえ。", "部長"),
            spk("/7", "…………", "\\N[1]"),
            spk("/8", "…………。", "\\N[1]"),
            spk("/9", "……。", "\\N[1]"),
        ];
        let conn = mem_db(&units);
        let got = mine_glossary_candidates(&conn, "rpgmaker-mvmz", 50).unwrap();
        let haruka = got.iter().find(|c| c.term == "ハルカ").expect("speaker name mined");
        assert_eq!(haruka.count, 3);
        assert!(haruka.mid >= 3, "a named speaker ranks like a mid-sentence hit");
        assert!(got.iter().any(|c| c.term == "部長"));
        // A speaker that is only an actor-ref code must not become a candidate.
        assert!(
            !got.iter().any(|c| c.term.contains('\\') || c.term.contains("N[1]")),
            "control-code speaker leaked: {:?}",
            got.iter().map(|c| &c.term).collect::<Vec<_>>()
        );
    }

    #[test]
    fn sample_corpus_is_diverse_clean_and_budgeted() {
        let mut units = vec![
            unit("A.json", "/s1", "OK", Status::Untranslated), // <3 words → dropped
            unit("A.json", "/s2", "Back", Status::Untranslated), // dropped
            unit("A.json", "/dup1", "The rain fell all night.", Status::Untranslated),
            unit("A.json", "/dup2", "the rain fell all night.", Status::Untranslated), // case dup
            unit(
                "A.json",
                "/long",
                "This is a much longer narrative line describing the ruined city at dawn.",
                Status::Untranslated,
            ),
            // CJK has no spaces: the whole sentence is one whitespace-"word", so it
            // must survive on letter count, not word count.
            unit("A.json", "/cjk", "拥有高透气性的亚麻制服装", Status::Untranslated),
        ];
        for i in 0..40 {
            units.push(unit("A.json", &format!("/m{i}"), &format!("Spread narrative line number {i}."), Status::Untranslated));
        }
        let conn = mem_db(&units);

        let big = sample_corpus(&conn, "rpgmaker-mvmz", 100_000).unwrap();
        // Short UI labels dropped by letter count (works for CJK, which has no
        // spaces); substantial lines kept.
        assert!(big.iter().all(|l| l.chars().filter(|c| c.is_alphabetic()).count() >= 6));
        assert!(!big.iter().any(|l| l == "OK" || l == "Back"));
        // A spaceless CJK sentence survives — the bug that produced "no sampled text".
        assert!(big.iter().any(|l| l.contains("拥有高透气性")), "CJK line must survive");
        // Case-insensitive dedup: the rain line appears once.
        assert_eq!(big.iter().filter(|l| l.to_lowercase() == "the rain fell all night.").count(), 1);
        // The longest line is present (longest bucket).
        assert!(big.iter().any(|l| l.contains("ruined city at dawn")));

        // A tight budget yields a strictly smaller sample, still valid.
        let small = sample_corpus(&conn, "rpgmaker-mvmz", 80).unwrap();
        assert!(small.len() < big.len() && !small.is_empty());
        let chars: usize = small.iter().map(|l| l.len() + 1).sum();
        assert!(chars <= 80 + 80, "budget roughly respected: {chars}");
    }
}

