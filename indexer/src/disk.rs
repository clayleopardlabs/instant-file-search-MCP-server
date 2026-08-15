//! Durable, disk-backed index storage for low-memory installations.
//!
//! The database deliberately stores only source metadata.  Search-only fields
//! (`name`, lowercase keys, extension, and excluded) are reconstructed for one
//! row at a time while a query streams the table, so they do not become a
//! second persistent representation or a resident-memory cost.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, params_from_iter, Connection, ErrorCode, OptionalExtension, Row};
use serde::Serialize;

use crate::query::{self, AggregateOptions, AggregateResult, QueryOptions, QueryResult};
use crate::types::IndexedFile;

pub struct DiskIndex {
    conn: Mutex<Connection>,
    path: PathBuf,
    recovery_reason: Option<String>,
}

pub const SCHEMA_VERSION: i64 = 4;
pub const DISK_CONTENT_BUDGET_DEFAULT: usize = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct DiskHealth {
    pub schema_version: i64,
    pub integrity: String,
    pub recovery_reason: Option<String>,
    pub wal_bytes: u64,
}

impl DiskIndex {
    pub fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create index directory {}", parent.display()))?;
        }
        let (conn, recovery_reason) = match open_and_migrate(&path) {
            Ok(conn) => (conn, None),
            Err(error) if is_confirmed_corruption(&error) => {
                let reason = format!("{error:#}");
                quarantine_database(&path)?;
                (
                    open_and_migrate(&path).with_context(|| {
                        format!("recreate disk index after quarantining {}", path.display())
                    })?,
                    Some(reason),
                )
            }
            Err(error) => return Err(error).with_context(|| format!("open disk index {}", path.display())),
        };
        Ok(Self {
            conn: Mutex::new(conn),
            path,
            recovery_reason,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }

    pub fn health(&self) -> DiskHealth {
        let conn = self.conn.lock().unwrap();
        let schema_version = conn
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap_or(0);
        let integrity = conn
            .query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))
            .unwrap_or_else(|error| format!("error: {error}"));
        let wal_bytes = sidecar_bytes(&self.path.with_extension("sqlite3-wal"));
        DiskHealth {
            schema_version,
            integrity,
            recovery_reason: self.recovery_reason.clone(),
            wal_bytes,
        }
    }

    /// Run inexpensive maintenance after a large write batch. This is safe to
    /// call while the service is running and never blocks on a full VACUUM.
    pub fn optimize(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA optimize; PRAGMA wal_checkpoint(PASSIVE);")?;
        Ok(())
    }

    /// Finish a clean service shutdown. A later startup can still recover from
    /// an interrupted write, but clean shutdowns do not leave a growing WAL.
    pub fn clean_shutdown(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA optimize; PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    /// Remove every cached file-content copy. Content search is a temporary
    /// cache, never a durable archive. VACUUM releases its pages back to the
    /// filesystem when the cache held data.
    pub fn clear_content(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute("DELETE FROM file_contents", [])?;
        if removed > 0 {
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
        }
        Ok(removed)
    }

    pub fn content_insert(&self, path: &str, data: &[u8]) -> Result<bool> {
        if data.is_empty() {
            return Ok(false);
        }
        let key = crate::platform::canonical_key(path);
        let data = data[..data.len().min(crate::content::MAX_FILE_BYTES as usize)].to_ascii_lowercase();
        let budget = disk_content_budget();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let old_bytes: usize = tx
            .query_row(
                "SELECT byte_count FROM file_contents WHERE path=?1",
                params![key],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0)
            .max(0) as usize;
        let used: usize = tx
            .query_row("SELECT COALESCE(SUM(byte_count), 0) FROM file_contents", [], |r| {
                r.get::<_, i64>(0)
            })?
            .max(0) as usize;
        if used.saturating_sub(old_bytes).saturating_add(data.len()) > budget {
            tx.commit()?;
            return Ok(false);
        }
        tx.execute("DELETE FROM file_contents WHERE path=?1", params![key])?;
        let source_modified: i64 = tx
            .query_row(
                "SELECT modified FROM files WHERE path_key=?1",
                params![key],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let indexed_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        tx.execute(
            "INSERT INTO file_contents(path,body,byte_count,indexed_at,source_modified) VALUES(?1,?2,?3,?4,?5)",
            params![key, data, data.len() as i64, indexed_at, source_modified],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn content_remove(&self, path: &str) -> Result<()> {
        let key = crate::platform::canonical_key(path);
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM file_contents WHERE path=?1", params![key])?;
        Ok(())
    }

    pub fn content_stats(&self) -> Result<(usize, usize)> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(byte_count), 0) FROM file_contents",
            [],
            |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)?.max(0) as usize)),
        )?)
    }

    pub fn content_budget(&self) -> usize {
        disk_content_budget()
    }

    pub fn content_contains(&self, path: &str, needle: &str) -> Result<bool> {
        let key = crate::platform::canonical_key(path);
        let needle = needle.to_ascii_lowercase().into_bytes();
        if needle.is_empty() {
            return Ok(false);
        }
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM file_contents WHERE path=?1 AND instr(CAST(body AS TEXT), ?2) > 0)",
                params![key, String::from_utf8_lossy(&needle).to_string()],
                |r| r.get::<_, i64>(0),
            )?
            != 0)
    }

    pub fn content_candidates(&self, after: &str, limit: usize) -> Result<Vec<(String, u64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT files.path,files.size FROM files
             LEFT JOIN file_contents fc ON fc.path=files.path_key AND fc.source_modified=files.modified
             WHERE files.path > ?1 AND files.is_dir=0 AND files.size > 0 AND files.size <= ?2 AND files.excluded=0
               AND fc.path IS NULL
             ORDER BY files.path LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![after, crate::content::MAX_FILE_BYTES as i64, limit as i64],
            |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64)),
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Replace the complete set of durable watcher checkpoints after a full
    /// scan. The scan's index transaction commits before this transaction, so
    /// a crash can at worst replay already-applied changes on the next start.
    pub fn replace_checkpoints(&self, checkpoints: &[(String, u64, i64)]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM checkpoints", [])?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO checkpoints(volume,journal_id,cursor) VALUES(?1,?2,?3)",
            )?;
            for (volume, journal_id, cursor) in checkpoints {
                insert.execute(params![volume, *journal_id as i64, cursor])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Advance one volume after its index mutations have committed.
    pub fn advance_checkpoint(&self, volume: &str, journal_id: u64, cursor: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO checkpoints(volume,journal_id,cursor) VALUES(?1,?2,?3)
             ON CONFLICT(volume) DO UPDATE SET journal_id=excluded.journal_id,cursor=excluded.cursor",
            params![volume, journal_id as i64, cursor],
        )?;
        Ok(())
    }

    pub fn checkpoints(&self) -> Result<Vec<(String, u64, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT volume,journal_id,cursor FROM checkpoints ORDER BY volume")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)? as u64,
                row.get(2)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn replace(&self, entries: Vec<IndexedFile>) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM files", [])?;
        {
            let mut insert = tx.prepare_cached(INSERT_SQL)?;
            for e in &entries {
                insert_entry(&mut insert, e)?;
            }
        }
        tx.commit()?;
        Ok(entries.len())
    }

    pub fn replace_volume(&self, prefix: &str, entries: Vec<IndexedFile>) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM files WHERE path LIKE ?1",
            params![format!("{prefix}%")],
        )?;
        {
            let mut insert = tx.prepare_cached(INSERT_SQL)?;
            for e in &entries {
                insert_entry(&mut insert, e)?;
            }
        }
        let n = tx.query_row("SELECT COUNT(*) FROM files", [], |r| r.get::<_, i64>(0))? as usize;
        tx.commit()?;
        Ok(n)
    }

    pub fn upsert(&self, mut entry: IndexedFile) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let old: Option<(u64, bool)> = tx
            .query_row(
                "SELECT size, is_dir FROM files WHERE path=?1",
                params![entry.path],
                |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? != 0)),
            )
            .optional()?;
        let old_size = old.map(|x| x.0).unwrap_or(0);
        let old_is_dir = old.map(|x| x.1).unwrap_or(false);
        let delta = if entry.is_dir {
            if old_is_dir {
                entry.size = old_size;
                0
            } else {
                -(old_size as i64)
            }
        } else {
            entry.size as i64 - old_size as i64
        };
        {
            let mut insert = tx.prepare_cached(INSERT_SQL)?;
            insert_entry(&mut insert, &entry)?;
        }
        adjust_ancestors(&tx, &entry.path, delta)?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove(&self, path: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let old: Option<(u64, bool)> = tx
            .query_row(
                "SELECT size, is_dir FROM files WHERE path=?1",
                params![path],
                |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? != 0)),
            )
            .optional()?;
        tx.execute("DELETE FROM files WHERE path=?1", params![path])?;
        if let Some((size, false)) = old {
            adjust_ancestors(&tx, path, -(size as i64))?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn remove_prefix(&self, prefix: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let total: u64 = tx
            .query_row(
                "SELECT size FROM files WHERE path=?1",
                params![prefix],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0) as u64;
        tx.execute(
            "DELETE FROM files WHERE path=?1 OR path LIKE ?2 OR path LIKE ?3",
            params![prefix, format!("{prefix}\\%"), format!("{prefix}/%")],
        )?;
        adjust_ancestors(&tx, prefix, -(total as i64))?;
        tx.commit()?;
        Ok(())
    }

    pub fn rename_prefix(&self, old_prefix: &str, new_prefix: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let total: u64 = tx
            .query_row(
                "SELECT size FROM files WHERE path=?1",
                params![old_prefix],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0) as u64;
        let changed: Vec<IndexedFile> = {
            let mut stmt = tx.prepare("SELECT path,size,created,modified,accessed,is_dir,attributes,file_ref,parent_ref,own_name,name_key,path_key,extension,excluded FROM files WHERE path=?1 OR path LIKE ?2 OR path LIKE ?3")?;
            let rows = stmt.query_map(
                params![
                    old_prefix,
                    format!("{old_prefix}\\%"),
                    format!("{old_prefix}/%")
                ],
                entry_from_row,
            )?;
            rows.filter_map(Result::ok).collect()
        };
        tx.execute(
            "DELETE FROM files WHERE path=?1 OR path LIKE ?2 OR path LIKE ?3",
            params![
                old_prefix,
                format!("{old_prefix}\\%"),
                format!("{old_prefix}/%")
            ],
        )?;
        {
            let mut insert = tx.prepare_cached(INSERT_SQL)?;
            for mut entry in changed {
                entry.set_path(format!("{new_prefix}{}", &entry.path[old_prefix.len()..]));
                insert_entry(&mut insert, &entry)?;
            }
        }
        adjust_ancestors(&tx, old_prefix, -(total as i64))?;
        adjust_ancestors(&tx, new_prefix, total as i64)?;
        tx.commit()?;
        Ok(())
    }

    pub fn path_by_ref(&self, volume: &str, file_ref: u64) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT path FROM files WHERE volume=?1 AND file_ref=?2",
            params![volume.trim_end_matches('\\'), file_ref as i64],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn search(&self, opts: &QueryOptions) -> QueryResult {
        let conn = self.conn.lock().unwrap();
        let plan = candidate_plan(opts);
        let sql = format!("SELECT path,size,created,modified,accessed,is_dir,attributes,file_ref,parent_ref,own_name,name_key,path_key,extension,excluded FROM files{}", plan.where_sql);
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => { tracing::error!("disk index query failed: {e}"); return QueryResult::default(); }
        };
        let rows = stmt.query_map(params_from_iter(plan.params.iter()), entry_from_row);
        match rows {
            Ok(rows) => query::search_iter(
                rows.filter_map(|row| row.map_err(|e| tracing::warn!("disk index row: {e}")).ok()),
                opts,
            ),
            Err(e) => {
                tracing::error!("disk index query failed: {e}");
                QueryResult::default()
            }
        }
    }

    pub fn aggregate(&self, opts: &AggregateOptions) -> AggregateResult {
        let conn = self.conn.lock().unwrap();
        let plan = candidate_plan_for_aggregate(opts);
        let sql = format!("SELECT path,size,created,modified,accessed,is_dir,attributes,file_ref,parent_ref,own_name,name_key,path_key,extension,excluded FROM files{}", plan.where_sql);
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => { tracing::error!("disk index aggregate failed: {e}"); return AggregateResult::default(); }
        };
        let result = match stmt.query_map(params_from_iter(plan.params.iter()), entry_from_row) {
            Ok(rows) => query::aggregate_iter(
                rows.filter_map(|row| row.map_err(|e| tracing::warn!("disk index row: {e}")).ok()),
                opts,
            ),
            Err(e) => {
                tracing::error!("disk index aggregate failed: {e}");
                AggregateResult::default()
            }
        };
        result
    }
}

fn open_and_migrate(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "wal_autocheckpoint", 1000i64)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;

    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if current > SCHEMA_VERSION {
        anyhow::bail!(
            "disk index schema version {current} is newer than supported version {SCHEMA_VERSION}"
        );
    }
    if current == 0 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS files (
                path TEXT PRIMARY KEY NOT NULL,
                volume TEXT NOT NULL,
                file_ref INTEGER NOT NULL,
                parent_ref INTEGER NOT NULL,
                own_name TEXT NOT NULL,
                size INTEGER NOT NULL,
                created INTEGER NOT NULL,
                modified INTEGER NOT NULL,
                accessed INTEGER NOT NULL,
                is_dir INTEGER NOT NULL,
                attributes INTEGER NOT NULL,
                name_key TEXT NOT NULL DEFAULT '',
                path_key TEXT NOT NULL DEFAULT '',
                extension TEXT,
                excluded INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS files_ref ON files(volume, file_ref);
             CREATE TABLE IF NOT EXISTS checkpoints (
                volume TEXT PRIMARY KEY NOT NULL,
                journal_id INTEGER NOT NULL,
                cursor INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS file_contents (
                path TEXT PRIMARY KEY NOT NULL,
                body BLOB NOT NULL,
                byte_count INTEGER NOT NULL,
                indexed_at INTEGER NOT NULL,
                source_modified INTEGER NOT NULL DEFAULT 0
             );
             SELECT 1;",
        )?;
        ensure_derived_columns(&tx)?;
        create_derived_indexes(&tx)?;
        populate_derived_columns(&tx)?;
        tx.execute_batch("PRAGMA user_version = 4;")?;
        tx.commit()?;
    } else if current == 1 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(
            "ALTER TABLE files ADD COLUMN name_key TEXT NOT NULL DEFAULT '';
             ALTER TABLE files ADD COLUMN path_key TEXT NOT NULL DEFAULT '';
             ALTER TABLE files ADD COLUMN extension TEXT;
             ALTER TABLE files ADD COLUMN excluded INTEGER NOT NULL DEFAULT 0;
             CREATE INDEX IF NOT EXISTS files_path_key ON files(path_key);
             CREATE INDEX IF NOT EXISTS files_size ON files(size);
             CREATE INDEX IF NOT EXISTS files_modified ON files(modified);
             CREATE INDEX IF NOT EXISTS files_type ON files(is_dir);
             UPDATE files SET name_key=lower(own_name), path_key=lower(path),
               extension=CASE
                 WHEN is_dir != 0 OR instr(own_name, '.') = 0 THEN NULL
                 WHEN instr(substr(own_name, instr(own_name, '.') + 1), '.') != 0 THEN NULL
                 ELSE lower(substr(own_name, instr(own_name, '.') + 1))
               END;
             CREATE TABLE IF NOT EXISTS file_contents (
                path TEXT PRIMARY KEY NOT NULL,
                body BLOB NOT NULL,
                byte_count INTEGER NOT NULL,
                indexed_at INTEGER NOT NULL,
                source_modified INTEGER NOT NULL DEFAULT 0
             );
             PRAGMA user_version = 4;",
        )?;
        tx.commit()?;
    } else if current == 2 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS file_contents (
                path TEXT PRIMARY KEY NOT NULL,
                body BLOB NOT NULL,
                byte_count INTEGER NOT NULL,
                indexed_at INTEGER NOT NULL,
                source_modified INTEGER NOT NULL DEFAULT 0
             );
             PRAGMA user_version = 4;",
        )?;
        tx.commit()?;
    } else if current == 3 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(
            "ALTER TABLE file_contents ADD COLUMN source_modified INTEGER NOT NULL DEFAULT 0;
             PRAGMA user_version = 4;",
        )?;
        tx.commit()?;
    }

    let check: String = conn.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    if check != "ok" {
        anyhow::bail!("sqlite integrity check failed: {check}");
    }
    Ok(conn)
}

fn is_confirmed_corruption(error: &anyhow::Error) -> bool {
    if error.to_string().contains("sqlite integrity check failed") {
        return true;
    }
    error.chain().any(|cause| {
        cause
            .downcast_ref::<rusqlite::Error>()
            .and_then(|e| e.sqlite_error_code())
            .is_some_and(|code| matches!(code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase))
    })
}

fn quarantine_database(path: &Path) -> Result<PathBuf> {
    if !path.exists() {
        return Ok(path.to_path_buf());
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let quarantine = PathBuf::from(format!("{}.corrupt-{stamp}", path.display()));
    std::fs::rename(path, &quarantine).with_context(|| {
        format!("quarantine corrupt disk index {}", path.display())
    })?;
    for suffix in ["sqlite3-wal", "sqlite3-shm"] {
        let sidecar = path.with_extension(suffix);
        if sidecar.exists() {
            let sidecar_quarantine = PathBuf::from(format!("{}.corrupt-{stamp}", sidecar.display()));
            std::fs::rename(&sidecar, sidecar_quarantine).with_context(|| {
                format!("quarantine corrupt disk index sidecar {}", sidecar.display())
            })?;
        }
    }
    Ok(quarantine)
}

fn sidecar_bytes(path: &Path) -> u64 {
    std::fs::metadata(path).map(|metadata| metadata.len()).unwrap_or(0)
}

fn disk_content_budget() -> usize {
    std::env::var("INSTANT_FS_CONTENT_DISK_BUDGET")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DISK_CONTENT_BUDGET_DEFAULT)
}

fn ensure_derived_columns(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    let columns: std::collections::HashSet<String> = {
        let mut stmt = tx.prepare("PRAGMA table_info(files)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for statement in [
        ("name_key", "ALTER TABLE files ADD COLUMN name_key TEXT NOT NULL DEFAULT ''"),
        ("path_key", "ALTER TABLE files ADD COLUMN path_key TEXT NOT NULL DEFAULT ''"),
        ("extension", "ALTER TABLE files ADD COLUMN extension TEXT"),
        ("excluded", "ALTER TABLE files ADD COLUMN excluded INTEGER NOT NULL DEFAULT 0"),
    ] {
        if !columns.contains(statement.0) {
            tx.execute_batch(statement.1)?;
        }
    }
    Ok(())
}

fn create_derived_indexes(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(
        "CREATE INDEX IF NOT EXISTS files_path_key ON files(path_key);
         CREATE INDEX IF NOT EXISTS files_size ON files(size);
         CREATE INDEX IF NOT EXISTS files_modified ON files(modified);
         CREATE INDEX IF NOT EXISTS files_type ON files(is_dir);",
    )
}

fn populate_derived_columns(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(
        "UPDATE files SET name_key=lower(own_name), path_key=lower(path),
           extension=CASE
             WHEN is_dir != 0 OR instr(own_name, '.') = 0 THEN NULL
             WHEN instr(substr(own_name, instr(own_name, '.') + 1), '.') != 0 THEN NULL
             ELSE lower(substr(own_name, instr(own_name, '.') + 1))
           END;",
    )
}

struct CandidatePlan {
    where_sql: String,
    params: Vec<rusqlite::types::Value>,
}

fn candidate_plan(opts: &QueryOptions) -> CandidatePlan {
    let mut clauses = Vec::new();
    let mut params = Vec::new();

    if !opts.match_case {
        if let Some(scope) = ascii_path_key(opts.path.as_deref()) {
            let escaped = escape_like(&scope);
            clauses.push("(path_key = ? OR path_key LIKE ? ESCAPE '\\')".to_string());
            params.push(rusqlite::types::Value::Text(scope));
            params.push(rusqlite::types::Value::Text(format!("{escaped}\\\\%")));
        }
        if let Some(exclude) = opts.exclude_path.as_deref() {
            let mut excluded = Vec::new();
            for part in exclude.split(';').filter_map(|part| ascii_path_key(Some(part.trim()))) {
                let escaped = escape_like(&part);
                excluded.push("(path_key = ? OR path_key LIKE ? ESCAPE '\\')".to_string());
                params.push(rusqlite::types::Value::Text(part));
                params.push(rusqlite::types::Value::Text(format!("{escaped}\\\\%")));
            }
            if !excluded.is_empty() {
                clauses.push(format!("NOT ({})", excluded.join(" OR ")));
            }
        }
    }

    if !opts.regex && !opts.match_case && !opts.match_whole_word {
        let query = opts.query.trim();
        if !query.is_empty() && !query.contains(['|', '<', '>']) {
            for token in query.split_whitespace() {
                if token.starts_with('!') || token.starts_with("not:") || token.starts_with("or:") {
                    continue;
                }
                let (kind, value) = token.split_once(':').unwrap_or(("", token));
                match kind.to_ascii_lowercase().as_str() {
                    "file" => {
                        clauses.push("is_dir = 0".to_string());
                    }
                    "folder" => {
                        clauses.push("is_dir != 0".to_string());
                    }
                    "ext" => add_extension_pattern(&mut clauses, &mut params, value),
                    "size" => add_size_constraint(&mut clauses, &mut params, value),
                    "content" | "path" | "dm" | "dc" | "da" | "attrib" | "len" | "frn" => {}
                    "" => add_like_pattern(&mut clauses, &mut params, value),
                    _ => {}
                }
            }
        }
    }

    if let Some(needles) = &opts.content_needles {
        for needle in needles.iter().filter(|needle| !needle.is_empty()) {
            clauses.push(
                "EXISTS (SELECT 1 FROM file_contents fc WHERE fc.path=files.path_key AND instr(CAST(fc.body AS TEXT), ?) > 0)"
                    .to_string(),
            );
            params.push(rusqlite::types::Value::Text(needle.to_ascii_lowercase()));
        }
    }

    CandidatePlan {
        where_sql: if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        },
        params,
    }
}

fn candidate_plan_for_aggregate(opts: &AggregateOptions) -> CandidatePlan {
    candidate_plan(&QueryOptions {
        query: opts.query.clone(),
        path: opts.path.clone(),
        exclude_path: opts.exclude_path.clone(),
        include_all: opts.include_all,
        match_path: opts.match_path,
        regex: opts.regex,
        match_case: opts.match_case,
        match_whole_word: opts.match_whole_word,
        content_paths: opts.content_paths.clone(),
        content_needles: opts.content_needles.clone(),
        ..Default::default()
    })
}

fn ascii_path_key(path: Option<&str>) -> Option<String> {
    let path = path?.trim();
    if path.is_empty() {
        return None;
    }
    let normalized = crate::platform::normalize_path(path);
    let canonical = crate::platform::canonical_key(&normalized);
    let key = crate::platform::trim_trailing_sep(&canonical);
    key.is_ascii().then(|| key.to_string())
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn add_like_pattern(clauses: &mut Vec<String>, params: &mut Vec<rusqlite::types::Value>, pattern: &str) {
    if let Some(extension) = pattern.strip_prefix("*.") {
        if !extension.is_empty() && extension.is_ascii() && !extension.contains(['*', '?', '[', '#']) {
            clauses.push("(extension = ? OR is_dir != 0 OR (extension IS NULL AND path_key LIKE ? ESCAPE '\\'))".to_string());
            params.push(rusqlite::types::Value::Text(extension.to_ascii_lowercase()));
            params.push(rusqlite::types::Value::Text(format!("%.{}", escape_like(&extension.to_ascii_lowercase()))));
            return;
        }
    }
    let Some(like) = wildcard_like(pattern) else {
        return;
    };
    clauses.push("path_key LIKE ? ESCAPE '\\'".to_string());
    params.push(rusqlite::types::Value::Text(format!("%{like}%")));
}

fn add_extension_pattern(clauses: &mut Vec<String>, params: &mut Vec<rusqlite::types::Value>, extension: &str) {
    let extension = extension.trim_start_matches('.');
    if extension.is_empty() || !extension.is_ascii() || extension.contains(['*', '?', '[', '#']) {
        return;
    }
    clauses.push("(extension = ? OR is_dir != 0 OR (extension IS NULL AND path_key LIKE ? ESCAPE '\\'))".to_string());
    params.push(rusqlite::types::Value::Text(extension.to_ascii_lowercase()));
    params.push(rusqlite::types::Value::Text(format!("%.{}", escape_like(&extension.to_ascii_lowercase()))));
}

fn wildcard_like(pattern: &str) -> Option<String> {
    if !pattern.is_ascii() || pattern.contains(['\\', '/', '[', ']', '#']) {
        return None;
    }
    let mut out = String::new();
    for ch in pattern.chars() {
        match ch {
            '*' => out.push('%'),
            '?' => out.push('_'),
            '%' | '_' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch.to_ascii_lowercase()),
        }
    }
    Some(out)
}

fn add_size_constraint(clauses: &mut Vec<String>, params: &mut Vec<rusqlite::types::Value>, value: &str) {
    let (operator, number) = if let Some(value) = value.strip_prefix(">=") {
        (">=", value)
    } else if let Some(value) = value.strip_prefix("<=") {
        ("<=", value)
    } else if let Some(value) = value.strip_prefix('>') {
        (">", value)
    } else if let Some(value) = value.strip_prefix('<') {
        ("<", value)
    } else if let Some(value) = value.strip_prefix('=') {
        ("=", value)
    } else {
        return;
    };
    let (number, multiplier) = if let Some(value) = number.strip_suffix("kb") {
        (value, 1024u64)
    } else if let Some(value) = number.strip_suffix("mb") {
        (value, 1024u64 * 1024)
    } else if let Some(value) = number.strip_suffix("gb") {
        (value, 1024u64 * 1024 * 1024)
    } else if let Some(value) = number.strip_suffix('b') {
        (value, 1)
    } else {
        return;
    };
    let Ok(number) = number.parse::<u64>() else { return };
    let Some(bytes) = number.checked_mul(multiplier) else { return };
    clauses.push(format!("size {operator} ?"));
    params.push(rusqlite::types::Value::Integer(bytes.min(i64::MAX as u64) as i64));
}

const INSERT_SQL: &str = "INSERT INTO files(path,volume,file_ref,parent_ref,own_name,size,created,modified,accessed,is_dir,attributes,name_key,path_key,extension,excluded)
 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
 ON CONFLICT(path) DO UPDATE SET volume=excluded.volume,file_ref=excluded.file_ref,parent_ref=excluded.parent_ref,own_name=excluded.own_name,size=excluded.size,created=excluded.created,modified=excluded.modified,accessed=excluded.accessed,is_dir=excluded.is_dir,attributes=excluded.attributes,name_key=excluded.name_key,path_key=excluded.path_key,extension=excluded.extension,excluded=excluded.excluded";

fn insert_entry(stmt: &mut rusqlite::CachedStatement<'_>, e: &IndexedFile) -> rusqlite::Result<()> {
    stmt.execute(params![
        e.path,
        crate::platform::volume_of(&e.path),
        e.file_ref as i64,
        e.parent_ref as i64,
        e.own_name,
        e.size as i64,
        e.created,
        e.modified,
        e.accessed,
        e.is_dir as i64,
        e.attributes as i64,
        e.lower_name,
        e.lower_path,
        e.extension,
        e.excluded as i64
    ])?;
    Ok(())
}

fn entry_from_row(r: &Row<'_>) -> rusqlite::Result<IndexedFile> {
    let mut e = IndexedFile::new(
        r.get(0)?,
        r.get::<_, i64>(1)? as u64,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get::<_, i64>(5)? != 0,
        r.get::<_, i64>(7)? as u64,
    );
    e.attributes = r.get::<_, i64>(6)? as u32;
    e.parent_ref = r.get::<_, i64>(8)? as u64;
    e.own_name = r.get(9)?;
    e.lower_name = r.get(10)?;
    e.lower_path = r.get(11)?;
    e.extension = r.get(12)?;
    e.excluded = r.get::<_, i64>(13)? != 0;
    Ok(e)
}

fn adjust_ancestors(
    tx: &rusqlite::Transaction<'_>,
    path: &str,
    delta: i64,
) -> rusqlite::Result<()> {
    if delta == 0 {
        return Ok(());
    }
    let mut parent = path;
    while let Some(i) = parent.rfind(['\\', '/']) {
        parent = &parent[..i];
        tx.execute(
            "UPDATE files SET size=MAX(0, size + ?1) WHERE path=?2",
            params![delta, parent],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "instant-file-search-disk-test-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn persists_and_streams_searches_without_a_hashmap() {
        let path = test_path();
        let _ = std::fs::remove_file(&path);
        let index = DiskIndex::open(path.clone()).unwrap();
        let mut a = IndexedFile::new(r"C:\work\alpha.rs".into(), 10, 1, 2, 3, false, 42);
        a.attributes = 7;
        let b = IndexedFile::new(r"C:\work\bravo.txt".into(), 20, 1, 2, 3, false, 43);
        assert_eq!(index.replace(vec![a, b]).unwrap(), 2);
        let result = index.search(&QueryOptions {
            query: "alpha".into(),
            max_results: 10,
            ..Default::default()
        });
        assert_eq!(result.total, 1);
        assert_eq!(result.entries[0].path, r"C:\work\alpha.rs");
        index
            .replace_checkpoints(&[("C:\\".into(), 77, 1_000)])
            .unwrap();
        index.advance_checkpoint("C:\\", 77, 1_100).unwrap();
        drop(index);
        let reopened = DiskIndex::open(path.clone()).unwrap();
        assert_eq!(reopened.len(), 2);
        assert_eq!(
            reopened.path_by_ref("C:", 42).as_deref(),
            Some(r"C:\work\alpha.rs")
        );
        assert_eq!(
            reopened.checkpoints().unwrap(),
            vec![("C:\\".into(), 77, 1_100)]
        );
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn disk_queries_and_aggregate_match_the_legacy_memory_engine() {
        use std::collections::HashMap;

        let mut alpha = IndexedFile::new(
            r"C:\work\src\Alpha.rs".into(),
            2_048,
            100,
            300,
            200,
            false,
            1,
        );
        alpha.attributes = 0x80;
        let mut beta =
            IndexedFile::new(r"C:\work\src\beta.TXT".into(), 512, 200, 400, 300, false, 2);
        beta.attributes = 0x02;
        let mut src = IndexedFile::new(r"C:\work\src".into(), 2_560, 50, 50, 50, true, 3);
        src.attributes = 0x10;
        let old = IndexedFile::new(r"C:\work\archive\old.tmp".into(), 1, 10, 20, 30, false, 4);
        let duplicate =
            IndexedFile::new(r"C:\other\Alpha.rs".into(), 4_096, 150, 500, 350, false, 5);
        let files = vec![alpha, beta, src, old, duplicate];
        let memory: HashMap<String, IndexedFile> =
            files.iter().cloned().map(|e| (e.path.clone(), e)).collect();
        let path = test_path();
        let disk = DiskIndex::open(path.clone()).unwrap();
        disk.replace(files.clone()).unwrap();

        let mut scoped = QueryOptions {
            query: "*".into(),
            path: Some(r"C:\work\src".into()),
            max_results: 100,
            ..Default::default()
        };
        let excluded = QueryOptions {
            query: "*".into(),
            exclude_path: Some(r"C:\work\archive".into()),
            max_results: 100,
            ..Default::default()
        };
        let options = vec![
            QueryOptions {
                query: "*.rs".into(),
                max_results: 100,
                ..Default::default()
            },
            QueryOptions {
                query: "file:*.rs".into(),
                max_results: 100,
                ..Default::default()
            },
            QueryOptions {
                query: "folder:*".into(),
                max_results: 100,
                ..Default::default()
            },
            QueryOptions {
                query: "ext:rs size:>1kb".into(),
                max_results: 100,
                ..Default::default()
            },
            QueryOptions {
                query: "attrib:a".into(),
                max_results: 100,
                ..Default::default()
            },
            QueryOptions {
                query: "is:folder".into(),
                max_results: 100,
                ..Default::default()
            },
            QueryOptions {
                query: "!*.tmp".into(),
                max_results: 100,
                ..Default::default()
            },
            QueryOptions {
                query: "dupe:filename".into(),
                max_results: 100,
                ..Default::default()
            },
            QueryOptions {
                query: "re:Alpha\\.rs".into(),
                regex: true,
                max_results: 100,
                ..Default::default()
            },
            QueryOptions {
                query: "alpha".into(),
                match_whole_word: true,
                max_results: 100,
                ..Default::default()
            },
            QueryOptions {
                query: "*.rs".into(),
                sort: Some("size".into()),
                offset: 1,
                max_results: 1,
                ..Default::default()
            },
            {
                scoped.content_paths = Some(vec![r"c:\work\src\alpha.rs".into()]);
                scoped.clone()
            },
            excluded.clone(),
        ];
        for options in options {
            let expected = query::search(&memory, &options);
            let actual = disk.search(&options);
            assert_eq!(actual.total, expected.total, "query: {}", options.query);
            let actual_entries: Vec<_> = actual
                .entries
                .iter()
                .map(|e| {
                    (
                        &e.path,
                        e.size,
                        e.created,
                        e.modified,
                        e.accessed,
                        e.is_dir,
                        e.attributes,
                        &e.extension,
                    )
                })
                .collect();
            let expected_entries: Vec<_> = expected
                .entries
                .iter()
                .map(|e| {
                    (
                        &e.path,
                        e.size,
                        e.created,
                        e.modified,
                        e.accessed,
                        e.is_dir,
                        e.attributes,
                        &e.extension,
                    )
                })
                .collect();
            assert_eq!(actual_entries, expected_entries, "query: {}", options.query);
        }
        let aggregate = AggregateOptions {
            query: "*.rs".into(),
            top: 2,
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(disk.aggregate(&aggregate)).unwrap(),
            serde_json::to_value(query::aggregate(&memory, &aggregate)).unwrap()
        );
        drop(disk);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn content_rows_can_be_cleared_and_do_not_survive_reopen() {
        let path = test_path();
        let _ = std::fs::remove_file(&path);
        let index = DiskIndex::open(path.clone()).unwrap();
        let entry = IndexedFile::new(r"C:\work\alpha.rs".into(), 42, 1, 2, 3, false, 1);
        index.replace(vec![entry]).unwrap();
        assert!(index.content_insert(r"C:\work\alpha.rs", b"Hello durable content").unwrap());
        assert_eq!(index.content_stats().unwrap(), (1, 21));
        assert!(index.content_candidates("", 10).unwrap().is_empty());
        let result = index.search(&QueryOptions {
            query: "*".into(),
            content_needles: Some(vec!["DURABLE".into()]),
            max_results: 10,
            ..Default::default()
        });
        assert_eq!(result.total, 1);
        assert_eq!(index.clear_content().unwrap(), 1);
        assert_eq!(index.content_stats().unwrap(), (0, 0));
        drop(index);
        let reopened = DiskIndex::open(path.clone()).unwrap();
        assert_eq!(reopened.content_stats().unwrap(), (0, 0));
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn restart_recovery_is_idempotent_and_checkpoint_ordered() {
        let path = test_path();
        let _ = std::fs::remove_file(&path);
        let index = DiskIndex::open(path.clone()).unwrap();
        index
            .replace(vec![IndexedFile::new(
                r"C:\work\alpha.rs".into(),
                10,
                1,
                2,
                3,
                false,
                1,
            )])
            .unwrap();
        index
            .replace_checkpoints(&[("C:\\".into(), 9, 100)])
            .unwrap();
        index
            .upsert(IndexedFile::new(
                r"C:\work\alpha.rs".into(),
                20,
                1,
                4,
                3,
                false,
                1,
            ))
            .unwrap();
        // The checkpoint is advanced only after the metadata transaction.
        index.advance_checkpoint("C:\\", 9, 200).unwrap();
        index.clean_shutdown().unwrap();
        drop(index);

        let reopened = DiskIndex::open(path.clone()).unwrap();
        assert_eq!(reopened.len(), 1);
        assert_eq!(reopened.checkpoints().unwrap(), vec![("C:\\".into(), 9, 200)]);
        assert_eq!(reopened.aggregate(&AggregateOptions::default()).total_size, 20);
        // Replaying the same update is idempotent, not a duplicate row.
        reopened
            .upsert(IndexedFile::new(
                r"C:\work\alpha.rs".into(),
                20,
                1,
                4,
                3,
                false,
                1,
            ))
            .unwrap();
        assert_eq!(reopened.len(), 1);
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn migrates_the_original_unversioned_schema_and_reports_health() {
        let path = test_path();
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE files (
                path TEXT PRIMARY KEY NOT NULL,
                volume TEXT NOT NULL,
                file_ref INTEGER NOT NULL,
                parent_ref INTEGER NOT NULL,
                own_name TEXT NOT NULL,
                size INTEGER NOT NULL,
                created INTEGER NOT NULL,
                modified INTEGER NOT NULL,
                accessed INTEGER NOT NULL,
                is_dir INTEGER NOT NULL,
                attributes INTEGER NOT NULL
             );
             CREATE TABLE checkpoints (
                volume TEXT PRIMARY KEY NOT NULL,
                journal_id INTEGER NOT NULL,
                cursor INTEGER NOT NULL
             );",
        )
        .unwrap();
        drop(conn);

        let index = DiskIndex::open(path.clone()).unwrap();
        let health = index.health();
        assert_eq!(health.schema_version, SCHEMA_VERSION);
        assert_eq!(health.integrity, "ok");
        assert!(health.recovery_reason.is_none());
        drop(index);
        remove_test_database(&path);
    }

    #[test]
    fn migrates_schema_version_one_to_the_indexed_columns() {
        let path = test_path();
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE files (
                path TEXT PRIMARY KEY NOT NULL,
                volume TEXT NOT NULL,
                file_ref INTEGER NOT NULL,
                parent_ref INTEGER NOT NULL,
                own_name TEXT NOT NULL,
                size INTEGER NOT NULL,
                created INTEGER NOT NULL,
                modified INTEGER NOT NULL,
                accessed INTEGER NOT NULL,
                is_dir INTEGER NOT NULL,
                attributes INTEGER NOT NULL
             );
             CREATE TABLE checkpoints (
                volume TEXT PRIMARY KEY NOT NULL,
                journal_id INTEGER NOT NULL,
                cursor INTEGER NOT NULL
             );
             PRAGMA user_version = 1;",
        )
        .unwrap();
        drop(conn);
        let index = DiskIndex::open(path.clone()).unwrap();
        assert_eq!(index.health().schema_version, SCHEMA_VERSION);
        index
            .replace(vec![IndexedFile::new(
                r"C:\work\one.rs".into(),
                1,
                1,
                1,
                1,
                false,
                1,
            )])
            .unwrap();
        assert_eq!(index.search(&QueryOptions { query: "*.rs".into(), max_results: 10, ..Default::default() }).total, 1);
        remove_test_database(&path);
    }

    #[test]
    fn common_extension_queries_use_a_conservative_index_candidate() {
        let plan = candidate_plan(&QueryOptions {
            query: "*.rs".into(),
            max_results: 10,
            ..Default::default()
        });
        assert!(plan.where_sql.contains("extension = ?"));
        assert_eq!(plan.params.len(), 2);
    }

    #[test]
    fn quarantines_confirmed_corruption_but_preserves_the_old_file() {
        let path = test_path();
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, b"not a sqlite database").unwrap();
        let index = DiskIndex::open(path.clone()).unwrap();
        let health = index.health();
        assert_eq!(health.integrity, "ok");
        assert!(health.recovery_reason.is_some());
        assert!(path.exists(), "a fresh replacement database was created");
        drop(index);
        let quarantined = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .find(|candidate| {
                candidate
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("instant-file-search-disk-test-") && name.contains(".corrupt-"))
            })
            .expect("corrupt database was quarantined");
        let _ = std::fs::remove_file(quarantined);
        remove_test_database(&path);
    }

    #[test]
    fn rejects_a_future_schema_without_quarantining_it() {
        let path = test_path();
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA user_version = 99;").unwrap();
        drop(conn);
        let error = match DiskIndex::open(path.clone()) {
            Ok(_) => panic!("future schema unexpectedly opened"),
            Err(error) => error,
        };
        assert!(format!("{error:#}").contains("newer than supported"));
        assert!(path.exists());
        remove_test_database(&path);
    }

    fn remove_test_database(path: &Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
    }
}
