//! SQLite storage for conversations, messages and settings.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::context::Turn;

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: i64,
    pub title: String,
    pub model: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub project: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: i64,
    pub conversation_id: i64,
    pub role: String,
    pub content: String,
    pub reasoning: String,
    pub model: String,
    /// complete | streaming | stopped | error
    pub status: String,
    pub error: Option<String>,
    pub stats: Option<serde_json::Value>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub base_url: String,
    pub server_command: String,
    pub auto_start_server: bool,
    pub system_prompt: String,
    pub temperature: f64,
    pub max_tokens: u32,
    pub default_model: String,
    /// Democritus's home folder; empty means the default (D:\Democritus).
    pub home_dir: String,
    /// Small model that writes memory-update proposals in the background.
    pub memory_helper_model: String,
    /// Active project (file stem in memory\projects\), '' for none.
    pub active_project: String,
    /// Context window requested from Ollama (num_ctx), capped at each model's
    /// own limit. Bigger windows hold more history but need more RAM/VRAM.
    pub context_tokens: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:11434/v1".into(),
            server_command: "ollama serve".into(),
            auto_start_server: true,
            system_prompt: "You are a helpful assistant.".into(),
            temperature: 0.7,
            // Thinking tokens count toward this limit, so keep it generous.
            max_tokens: 8192,
            default_model: String::new(),
            home_dir: String::new(),
            memory_helper_model: "qwen3.5:4b".into(),
            active_project: String::new(),
            context_tokens: 16384,
        }
    }
}

/// Copies the database of the earlier "Local Qwen" build into the new location,
/// once. The old file is only read, never changed or deleted. `VACUUM INTO`
/// produces a consistent snapshot that includes anything still in the WAL.
pub fn migrate_legacy(new_db: &Path, legacy_db: &Path) -> rusqlite::Result<bool> {
    if new_db.exists() || !legacy_db.exists() {
        return Ok(false);
    }
    let old = Connection::open_with_flags(legacy_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    old.execute("VACUUM INTO ?1", params![new_db.to_string_lossy()])?;
    Ok(true)
}

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 3000;")?;
    migrate(&conn)?;
    // A message still marked as streaming means the app quit mid-generation.
    conn.execute("UPDATE messages SET status = 'stopped' WHERE status = 'streaming'", [])?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE conversations (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 title TEXT NOT NULL,
                 model TEXT NOT NULL DEFAULT '',
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL
             );
             CREATE TABLE messages (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                 role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
                 content TEXT NOT NULL DEFAULT '',
                 reasoning TEXT NOT NULL DEFAULT '',
                 model TEXT NOT NULL DEFAULT '',
                 status TEXT NOT NULL DEFAULT 'complete',
                 error TEXT,
                 stats TEXT,
                 created_at INTEGER NOT NULL
             );
             CREATE INDEX idx_messages_conversation ON messages(conversation_id, id);
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             PRAGMA user_version = 1;
             COMMIT;",
        )?;
    }
    if version < 2 {
        conn.execute_batch(
            "BEGIN;
             -- Measured footprint of each model the last time the server had it loaded.
             CREATE TABLE model_memory (
                 model TEXT PRIMARY KEY,
                 loaded_bytes INTEGER NOT NULL,
                 vram_bytes INTEGER NOT NULL,
                 context_length INTEGER,
                 updated_at INTEGER NOT NULL
             );
             PRAGMA user_version = 2;
             COMMIT;",
        )?;
    }
    if version < 3 {
        conn.execute_batch(
            "BEGIN;
             -- Project a conversation belongs to (memory\\projects\\<slug>.md), '' for none.
             ALTER TABLE conversations ADD COLUMN project TEXT NOT NULL DEFAULT '';
             -- Background memory-update jobs and their proposals.
             -- status: queued | running | pending (awaiting review) | applied | rejected | empty | failed
             CREATE TABLE memory_proposals (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 conversation_id INTEGER,
                 user_message_id INTEGER,
                 assistant_message_id INTEGER,
                 status TEXT NOT NULL,
                 model TEXT NOT NULL DEFAULT '',
                 ops TEXT NOT NULL DEFAULT '[]',
                 rejected TEXT NOT NULL DEFAULT '[]',
                 project TEXT NOT NULL DEFAULT '',
                 note TEXT NOT NULL DEFAULT '',
                 error TEXT,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL
             );
             CREATE INDEX idx_memory_proposals_status ON memory_proposals(status);
             -- Search index derived from the memory files (rebuildable).
             CREATE TABLE memory_files (path TEXT PRIMARY KEY, mtime INTEGER NOT NULL, size INTEGER NOT NULL, app_mtime INTEGER);
             CREATE TABLE memory_chunks (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 path TEXT NOT NULL,
                 heading TEXT NOT NULL,
                 text TEXT NOT NULL,
                 embedding BLOB,
                 embed_model TEXT
             );
             CREATE INDEX idx_memory_chunks_path ON memory_chunks(path);
             CREATE VIRTUAL TABLE memory_fts USING fts5(heading, text, chunk_id UNINDEXED);
             PRAGMA user_version = 3;
             COMMIT;",
        )?;
    }
    if version < 4 {
        conn.execute_batch(
            "BEGIN;
             -- Running summary of older messages that no longer fit the context
             -- window, and the id of the last message it covers.
             ALTER TABLE conversations ADD COLUMN summary TEXT NOT NULL DEFAULT '';
             ALTER TABLE conversations ADD COLUMN summary_upto INTEGER NOT NULL DEFAULT 0;
             PRAGMA user_version = 4;
             COMMIT;",
        )?;
    }
    Ok(())
}

#[cfg(test)]
pub fn migrate_for_tests(conn: &Connection) {
    migrate(conn).unwrap();
}

/// A fresh, unique temp folder per test (tests run in parallel).
#[cfg(test)]
pub fn test_dir(name: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!("democritus-test-{name}-{}-{}-{}", std::process::id(), now_ms(), N.fetch_add(1, Ordering::SeqCst)));
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn record_model_memory(conn: &Connection, model: &str, loaded_bytes: u64, vram_bytes: u64, context_length: Option<u64>) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO model_memory (model, loaded_bytes, vram_bytes, context_length, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(model) DO UPDATE SET loaded_bytes = excluded.loaded_bytes, vram_bytes = excluded.vram_bytes,
             context_length = excluded.context_length, updated_at = excluded.updated_at",
        params![model, loaded_bytes as i64, vram_bytes as i64, context_length.map(|c| c as i64), now_ms()],
    )?;
    Ok(())
}

pub fn model_memory(conn: &Connection, model: &str) -> rusqlite::Result<Option<u64>> {
    conn.query_row("SELECT loaded_bytes FROM model_memory WHERE model = ?1", params![model], |r| r.get::<_, i64>(0))
        .optional()
        .map(|o| o.map(|b| b as u64))
}

fn conversation_from_row(r: &Row) -> rusqlite::Result<Conversation> {
    Ok(Conversation { id: r.get(0)?, title: r.get(1)?, model: r.get(2)?, created_at: r.get(3)?, updated_at: r.get(4)?, project: r.get(5)? })
}

const MESSAGE_COLUMNS: &str = "id, conversation_id, role, content, reasoning, model, status, error, stats, created_at";

fn message_from_row(r: &Row) -> rusqlite::Result<Message> {
    let stats: Option<String> = r.get(8)?;
    Ok(Message {
        id: r.get(0)?,
        conversation_id: r.get(1)?,
        role: r.get(2)?,
        content: r.get(3)?,
        reasoning: r.get(4)?,
        model: r.get(5)?,
        status: r.get(6)?,
        error: r.get(7)?,
        stats: stats.and_then(|s| serde_json::from_str(&s).ok()),
        created_at: r.get(9)?,
    })
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// Lists conversations, newest first. A query matches titles and message text.
pub fn list_conversations(conn: &Connection, query: Option<&str>) -> rusqlite::Result<Vec<Conversation>> {
    let pattern = query.map(str::trim).filter(|q| !q.is_empty()).map(|q| format!("%{}%", escape_like(q)));
    let mut stmt = conn.prepare(
        "SELECT id, title, model, created_at, updated_at, project FROM conversations c
         WHERE ?1 IS NULL
            OR c.title LIKE ?1 ESCAPE '\\'
            OR EXISTS (SELECT 1 FROM messages m WHERE m.conversation_id = c.id AND m.content LIKE ?1 ESCAPE '\\')
         ORDER BY c.updated_at DESC, c.id DESC",
    )?;
    let rows = stmt.query_map(params![pattern], conversation_from_row)?;
    rows.collect()
}

pub fn get_conversation(conn: &Connection, id: i64) -> rusqlite::Result<Option<Conversation>> {
    conn.query_row(
        "SELECT id, title, model, created_at, updated_at, project FROM conversations WHERE id = ?1",
        params![id],
        conversation_from_row,
    )
    .optional()
}

pub fn create_conversation(conn: &Connection, title: &str, model: &str) -> rusqlite::Result<Conversation> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO conversations (title, model, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        params![title, model, now],
    )?;
    let id = conn.last_insert_rowid();
    get_conversation(conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn rename_conversation(conn: &Connection, id: i64, title: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE conversations SET title = ?2 WHERE id = ?1", params![id, title])?;
    Ok(())
}

pub fn delete_conversation(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM conversations WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn set_conversation_model(conn: &Connection, id: i64, model: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE conversations SET model = ?2 WHERE id = ?1", params![id, model])?;
    Ok(())
}

pub fn set_conversation_project(conn: &Connection, id: i64, project: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE conversations SET project = ?2 WHERE id = ?1", params![id, project])?;
    Ok(())
}

pub fn touch_conversation(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE conversations SET updated_at = ?2 WHERE id = ?1", params![id, now_ms()])?;
    Ok(())
}

pub fn get_messages(conn: &Connection, conversation_id: i64) -> rusqlite::Result<Vec<Message>> {
    let mut stmt = conn.prepare(&format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE conversation_id = ?1 ORDER BY id"))?;
    let rows = stmt.query_map(params![conversation_id], message_from_row)?;
    rows.collect()
}

pub fn get_message(conn: &Connection, id: i64) -> rusqlite::Result<Option<Message>> {
    conn.query_row(&format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE id = ?1"), params![id], message_from_row)
        .optional()
}

pub fn insert_message(
    conn: &Connection,
    conversation_id: i64,
    role: &str,
    content: &str,
    model: &str,
    status: &str,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO messages (conversation_id, role, content, model, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![conversation_id, role, content, model, status, now_ms()],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn finish_message(
    conn: &Connection,
    id: i64,
    content: &str,
    reasoning: &str,
    status: &str,
    error: Option<&str>,
    stats: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE messages SET content = ?2, reasoning = ?3, status = ?4, error = ?5, stats = ?6 WHERE id = ?1",
        params![id, content, reasoning, status, error, stats],
    )?;
    Ok(())
}

/// The turns to send back to the model, after message `after_id` (the end of
/// the stored summary). Reasoning is left out (Qwen's guidance), as are failed
/// or empty replies.
pub fn history(conn: &Connection, conversation_id: i64, after_id: i64) -> rusqlite::Result<Vec<Turn>> {
    let mut stmt = conn.prepare(
        "SELECT id, role, content, model FROM messages
         WHERE conversation_id = ?1 AND id > ?2 AND status IN ('complete', 'stopped') AND content <> ''
         ORDER BY id",
    )?;
    let rows = stmt.query_map(params![conversation_id, after_id], |r| Ok(Turn { id: r.get(0)?, role: r.get(1)?, content: r.get(2)?, model: r.get(3)? }))?;
    rows.collect()
}

/// (summary, id of the last message it covers).
pub fn get_summary(conn: &Connection, conversation_id: i64) -> rusqlite::Result<(String, i64)> {
    conn.query_row("SELECT summary, summary_upto FROM conversations WHERE id = ?1", params![conversation_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map(Option::unwrap_or_default)
}

pub fn set_summary(conn: &Connection, conversation_id: i64, summary: &str, upto: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE conversations SET summary = ?2, summary_upto = ?3 WHERE id = ?1", params![conversation_id, summary, upto])?;
    Ok(())
}

pub fn get_settings(conn: &Connection) -> Settings {
    conn.query_row("SELECT value FROM settings WHERE key = 'app'", [], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

pub fn save_settings(conn: &Connection, settings: &Settings) -> rusqlite::Result<()> {
    let json = serde_json::to_string(settings).expect("settings serialize");
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('app', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![json],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn conversation_lifecycle_and_search() {
        let conn = mem();
        let a = create_conversation(&conn, "Rust lifetimes", "qwen3.5:9b").unwrap();
        let b = create_conversation(&conn, "Dinner ideas", "qwen3.5:4b").unwrap();
        insert_message(&conn, b.id, "user", "How do I cook 100% whole_wheat pasta?", "qwen3.5:4b", "complete").unwrap();

        assert_eq!(list_conversations(&conn, None).unwrap().len(), 2);
        assert_eq!(list_conversations(&conn, Some("rust")).unwrap()[0].id, a.id);
        assert_eq!(list_conversations(&conn, Some("100%")).unwrap()[0].id, b.id);
        assert_eq!(list_conversations(&conn, Some("whole_wheat")).unwrap()[0].id, b.id);
        assert!(list_conversations(&conn, Some("nothing-matches")).unwrap().is_empty());

        rename_conversation(&conn, a.id, "Borrow checker").unwrap();
        assert_eq!(get_conversation(&conn, a.id).unwrap().unwrap().title, "Borrow checker");

        delete_conversation(&conn, b.id).unwrap();
        assert!(get_messages(&conn, b.id).unwrap().is_empty(), "messages cascade on delete");
    }

    #[test]
    fn history_skips_failed_and_empty_replies() {
        let conn = mem();
        let c = create_conversation(&conn, "t", "m").unwrap();
        insert_message(&conn, c.id, "user", "hi", "m", "complete").unwrap();
        let failed = insert_message(&conn, c.id, "assistant", "", "m", "streaming").unwrap();
        finish_message(&conn, failed, "", "", "error", Some("boom"), None).unwrap();
        insert_message(&conn, c.id, "user", "again", "m", "complete").unwrap();
        let ok = insert_message(&conn, c.id, "assistant", "", "m", "streaming").unwrap();
        finish_message(&conn, ok, "hello", "thinking...", "complete", None, Some(r#"{"completionTokens":3}"#)).unwrap();

        let h: Vec<(String, String)> = history(&conn, c.id, 0).unwrap().into_iter().map(|t| (t.role, t.content)).collect();
        assert_eq!(h, vec![("user".into(), "hi".into()), ("user".into(), "again".into()), ("assistant".into(), "hello".into())]);
        // Messages covered by a summary are skipped.
        let first = get_messages(&conn, c.id).unwrap()[0].id;
        set_summary(&conn, c.id, "Said hi.", first).unwrap();
        assert_eq!(get_summary(&conn, c.id).unwrap(), ("Said hi.".to_string(), first));
        assert_eq!(history(&conn, c.id, first).unwrap().len(), 2);
        let saved = get_message(&conn, ok).unwrap().unwrap();
        assert_eq!(saved.reasoning, "thinking...");
        assert_eq!(saved.stats.unwrap()["completionTokens"], 3);
    }

    #[test]
    fn legacy_database_is_copied_once_and_left_untouched() {
        let dir = test_dir("migrate");
        std::fs::create_dir_all(&dir).unwrap();
        let legacy = dir.join("local-qwen.db");
        let new = dir.join("democritus.db");
        {
            let old = open(&legacy).unwrap();
            let c = create_conversation(&old, "Old chat", "qwen3.5:4b").unwrap();
            insert_message(&old, c.id, "user", "hello from before", "qwen3.5:4b", "complete").unwrap();
            let mut s = Settings::default();
            s.system_prompt = "Be terse.".into();
            save_settings(&old, &s).unwrap();
        }
        let legacy_bytes = std::fs::read(&legacy).unwrap();

        assert!(migrate_legacy(&new, &legacy).unwrap());
        let conn = open(&new).unwrap();
        let convs = list_conversations(&conn, None).unwrap();
        assert_eq!(convs[0].title, "Old chat");
        assert_eq!(get_messages(&conn, convs[0].id).unwrap()[0].content, "hello from before");
        assert_eq!(get_settings(&conn).system_prompt, "Be terse.");
        assert_eq!(std::fs::read(&legacy).unwrap(), legacy_bytes, "legacy file unchanged");
        assert!(!migrate_legacy(&new, &legacy).unwrap(), "second run is a no-op");
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_round_trip_with_defaults() {
        let conn = mem();
        assert_eq!(get_settings(&conn).max_tokens, 8192);
        let mut s = Settings::default();
        s.temperature = 0.2;
        save_settings(&conn, &s).unwrap();
        assert_eq!(get_settings(&conn).temperature, 0.2);
    }
}
