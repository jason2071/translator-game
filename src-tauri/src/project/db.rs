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

        CREATE INDEX IF NOT EXISTS idx_unit_status ON unit(status);
        CREATE INDEX IF NOT EXISTS idx_unit_file   ON unit(file);
        -- apply_tm() joins units by source; without this it is O(n^2).
        CREATE INDEX IF NOT EXISTS idx_unit_source ON unit(source);
        "#,
    )?;
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

/// Filter/paginate the unit grid. All fields optional.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnitFilter {
    pub file: Option<String>,
    pub status: Option<String>,
    pub search: Option<String>,
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

pub fn list_units(conn: &Connection, filter: &UnitFilter) -> Result<Vec<TransUnit>> {
    let mut sql = String::from(
        "SELECT id, file, pointer, kind, context, grp, source, translation, status FROM unit WHERE 1=1",
    );
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
    if let Some(q) = &filter.search {
        if !q.is_empty() {
            // Escape LIKE metacharacters so a literal % or _ isn't a wildcard.
            let esc = q
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            sql.push_str(" AND (source LIKE ? ESCAPE '\\' OR translation LIKE ? ESCAPE '\\')");
            let like = format!("%{esc}%");
            args.push(Box::new(like.clone()));
            args.push(Box::new(like));
        }
    }
    sql.push_str(" ORDER BY id");
    // Grid pages are small; the ceiling is high so a whole-project translate
    // (which passes an explicit large limit) is never silently truncated.
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

pub fn update_unit(conn: &Connection, id: i64, translation: Option<&str>, status: &str) -> Result<()> {
    // Normalize the status so an unknown string can never poison stats()/export.
    let status = Status::from_str(status).as_str();
    conn.execute(
        "UPDATE unit SET translation = ?1, status = ?2 WHERE id = ?3",
        params![translation, status, id],
    )?;
    Ok(())
}

/// Counts per status plus total, for the dashboard.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub total: i64,
    pub untranslated: i64,
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

/// Insert several glossary entries in one transaction (skips empties).
pub fn glossary_add_bulk(conn: &mut Connection, items: &[(String, String)]) -> Result<usize> {
    let tx = conn.transaction()?;
    let mut added = 0usize;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO glossary(term, translation, note, case_sensitive) VALUES(?1, ?2, NULL, 0)",
        )?;
        for (term, translation) in items {
            if term.trim().is_empty() || translation.trim().is_empty() {
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
    let mut stmt = conn.prepare(
        "SELECT source, MAX(translation) AS tr, MIN(kind) AS k, COUNT(*) AS c
           FROM unit
          WHERE source <> ''
            AND ( (file = 'Actors.json'  AND kind IN ('Name','Nickname'))
               OR (file = 'Enemies.json' AND kind = 'Name')
               OR (file = 'Classes.json' AND kind = 'Name')
               OR kind = 'Term' )
            AND source NOT IN (SELECT term FROM glossary)
          GROUP BY source
          ORDER BY c DESC, source
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
