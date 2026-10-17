//! Search index over the memory files. Derived data only: it can be dropped and
//! rebuilt from the Markdown at any time. Keyword search uses SQLite FTS5;
//! meaning search compares embeddings (stored as f32 blobs) in Rust, which is
//! instant at memory-folder scale; results are merged by reciprocal rank fusion.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::memory;

const MAX_CHUNK_CHARS: usize = 1500;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub path: String,
    pub heading: String,
    pub text: String,
    pub score: f64,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub changed: Vec<String>,
    pub removed: Vec<String>,
    /// Files whose change didn't come from the app (edited by hand).
    pub external: Vec<String>,
}

/// Splits a file into (heading, text) chunks by section, splitting long
/// sections at paragraph breaks.
pub fn chunk(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut heading = String::new();
    let mut body = String::new();
    let flush = |heading: &str, body: &mut String, out: &mut Vec<(String, String)>| {
        let t = body.trim();
        if !t.is_empty() {
            let mut cur = String::new();
            for para in t.split("\n\n") {
                if !cur.is_empty() && cur.len() + para.len() > MAX_CHUNK_CHARS {
                    out.push((heading.to_string(), cur.trim().to_string()));
                    cur.clear();
                }
                cur.push_str(para);
                cur.push_str("\n\n");
            }
            if !cur.trim().is_empty() {
                out.push((heading.to_string(), cur.trim().to_string()));
            }
        }
        body.clear();
    };
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with('#') && t.trim_start_matches('#').starts_with(' ') {
            flush(&heading, &mut body, &mut out);
            heading = t.trim_start_matches('#').trim().to_string();
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    flush(&heading, &mut body, &mut out);
    out
}

fn file_mtime(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn remove_path(conn: &Connection, rel: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM memory_fts WHERE chunk_id IN (SELECT id FROM memory_chunks WHERE path = ?1)", params![rel])?;
    conn.execute("DELETE FROM memory_chunks WHERE path = ?1", params![rel])?;
    Ok(())
}

/// Records that the app itself just wrote `rel`, so the next sync doesn't
/// report it as a hand edit.
pub fn mark_app_write(conn: &Connection, mem: &Path, rel: &str) -> rusqlite::Result<()> {
    let m = file_mtime(&memory::abs(mem, rel));
    conn.execute(
        "INSERT INTO memory_files (path, mtime, size, app_mtime) VALUES (?1, -1, 0, ?2)
         ON CONFLICT(path) DO UPDATE SET app_mtime = excluded.app_mtime",
        params![rel, m],
    )?;
    Ok(())
}

/// Brings the index up to date with the files on disk.
pub fn sync(conn: &Connection, mem: &Path) -> rusqlite::Result<SyncReport> {
    let mut report = SyncReport::default();
    let files = memory::list(mem);
    let mut known: HashMap<String, (i64, i64, Option<i64>)> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT path, mtime, size, app_mtime FROM memory_files")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))?;
        for row in rows {
            let (p, v) = row?;
            known.insert(p, v);
        }
    }
    for f in &files {
        let p = memory::abs(mem, &f.path);
        let mtime = file_mtime(&p);
        let size = f.size as i64;
        let prev = known.remove(&f.path);
        if prev.is_some_and(|(m, s, _)| m == mtime && s == size) {
            continue;
        }
        if let Some((old_m, _, app_m)) = prev {
            if old_m >= 0 && app_m != Some(mtime) {
                report.external.push(f.path.clone());
            }
        }
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        remove_path(conn, &f.path)?;
        for (heading, body) in chunk(&text) {
            conn.execute("INSERT INTO memory_chunks (path, heading, text) VALUES (?1, ?2, ?3)", params![f.path, heading, body])?;
            let id = conn.last_insert_rowid();
            conn.execute("INSERT INTO memory_fts (heading, text, chunk_id) VALUES (?1, ?2, ?3)", params![heading, body, id])?;
        }
        conn.execute(
            "INSERT INTO memory_files (path, mtime, size, app_mtime) VALUES (?1, ?2, ?3, NULL)
             ON CONFLICT(path) DO UPDATE SET mtime = excluded.mtime, size = excluded.size",
            params![f.path, mtime, size],
        )?;
        report.changed.push(f.path.clone());
    }
    for gone in known.keys() {
        remove_path(conn, gone)?;
        conn.execute("DELETE FROM memory_files WHERE path = ?1", params![gone])?;
        report.removed.push(gone.clone());
    }
    Ok(report)
}

/// Drops the whole index (it is rebuilt by the next sync).
pub fn clear(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("DELETE FROM memory_fts; DELETE FROM memory_chunks; DELETE FROM memory_files;")
}

/// FTS5 query from free text: each word quoted, any word may match.
fn fts_query(q: &str) -> Option<String> {
    let words: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.chars().count() >= 2)
        .take(16)
        .map(|w| format!("\"{}\"", w.replace('"', "")))
        .collect();
    (!words.is_empty()).then(|| words.join(" OR "))
}

pub fn search_text(conn: &Connection, query: &str, limit: usize) -> rusqlite::Result<Vec<Hit>> {
    let Some(q) = fts_query(query) else { return Ok(Vec::new()) };
    let mut stmt = conn.prepare(
        "SELECT c.path, c.heading, c.text, bm25(memory_fts) AS rank
         FROM memory_fts JOIN memory_chunks c ON c.id = memory_fts.chunk_id
         WHERE memory_fts MATCH ?1 ORDER BY rank LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![q, limit as i64], |r| {
        Ok(Hit { path: r.get(0)?, heading: r.get(1)?, text: r.get(2)?, score: -r.get::<_, f64>(3)? })
    })?;
    rows.collect()
}

pub fn chunks_needing_embeddings(conn: &Connection, model: &str, limit: usize) -> rusqlite::Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        // File and section give the passage its context ("operator.md › Facts").
        "SELECT id, path || CASE WHEN heading = '' THEN '' ELSE ' › ' || heading END || char(10) || text
         FROM memory_chunks WHERE embedding IS NULL OR embed_model IS NOT ?1 LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![model, limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

pub fn store_embedding(conn: &Connection, id: i64, model: &str, v: &[f32]) -> rusqlite::Result<()> {
    let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    conn.execute("UPDATE memory_chunks SET embedding = ?2, embed_model = ?3 WHERE id = ?1", params![id, bytes, model])?;
    Ok(())
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0f64, 0f64, 0f64);
    for (x, y) in a.iter().zip(b) {
        dot += f64::from(*x) * f64::from(*y);
        na += f64::from(*x) * f64::from(*x);
        nb += f64::from(*y) * f64::from(*y);
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

pub fn search_vector(conn: &Connection, model: &str, query: &[f32], limit: usize) -> rusqlite::Result<Vec<Hit>> {
    let mut stmt = conn.prepare("SELECT path, heading, text, embedding FROM memory_chunks WHERE embedding IS NOT NULL AND embed_model = ?1")?;
    let rows = stmt.query_map(params![model], |r| {
        let blob: Vec<u8> = r.get(3)?;
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, blob))
    })?;
    let mut hits = Vec::new();
    for row in rows {
        let (path, heading, text, blob) = row?;
        let v: Vec<f32> = blob.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        hits.push(Hit { path, heading, text, score: cosine(query, &v) });
    }
    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    hits.truncate(limit);
    Ok(hits)
}

/// Rank constant for fusion. The usual 60 is meant for long result lists; the
/// memory index has a few dozen passages, and at 60 a passage that every
/// keyword list catches (common words match almost everything) beat the best
/// meaning match.
const FUSE_K: f64 = 5.0;
/// Weight of the meaning list against the keyword list. Questions are worded
/// unlike the notes that answer them; exact names and terms still win through
/// the keyword list.
pub const MEANING_WEIGHT: f64 = 2.0;

/// Weighted reciprocal rank fusion of keyword and meaning results.
pub fn fuse(lists: &[(Vec<Hit>, f64)], limit: usize) -> Vec<Hit> {
    let mut scores: HashMap<(String, String, String), (f64, Hit)> = HashMap::new();
    for (list, weight) in lists {
        // Each list counts a passage once, at its best rank (duplicated text in
        // a file must not add up).
        let mut seen = std::collections::HashSet::new();
        let mut rank = 0usize;
        for h in list {
            let key = (h.path.clone(), h.heading.clone(), h.text.clone());
            if !seen.insert(key.clone()) {
                continue;
            }
            let add = weight / (FUSE_K + rank as f64 + 1.0);
            rank += 1;
            scores.entry(key).and_modify(|e| e.0 += add).or_insert((add, h.clone()));
        }
    }
    let mut out: Vec<Hit> = scores.into_values().map(|(s, mut h)| {
        h.score = s;
        h
    }).collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Connection, std::path::PathBuf) {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate_for_tests(&conn);
        let mem = crate::db::test_dir("idx");
        memory::ensure_structure(&mem).unwrap();
        (conn, mem)
    }

    #[test]
    fn chunks_by_section() {
        let c = chunk("# Title\nintro\n\n## A\nalpha\n\n## B\nbeta\n");
        assert_eq!(c, vec![("Title".into(), "intro".into()), ("A".into(), "alpha".into()), ("B".into(), "beta".into())]);
        let long = format!("## Big\n{}\n\n{}\n", "a".repeat(1000), "b".repeat(1000));
        assert_eq!(chunk(&long).len(), 2);
    }

    #[test]
    fn keyword_search_and_hand_edit_detection() {
        let (conn, mem) = setup();
        memory::write(&mem, "projects/garden.md", "# Garden planner\n\n## Status\nTomato beds use drip irrigation.\n").unwrap();
        let first = sync(&conn, &mem).unwrap();
        assert!(first.changed.contains(&"projects/garden.md".to_string()));
        assert!(first.external.is_empty());
        let hits = search_text(&conn, "how are the tomatoes watered? irrigation", 5).unwrap();
        assert_eq!(hits[0].path, "projects/garden.md");
        assert_eq!(hits[0].heading, "Status");

        // App write: not reported as external.
        std::thread::sleep(std::time::Duration::from_millis(20));
        memory::write(&mem, "projects/garden.md", "# Garden planner\n\n## Status\nSwitched to soaker hoses.\n").unwrap();
        mark_app_write(&conn, &mem, "projects/garden.md").unwrap();
        assert!(sync(&conn, &mem).unwrap().external.is_empty());
        assert!(search_text(&conn, "irrigation", 5).unwrap().is_empty(), "old text dropped from the index");

        // Hand edit: reported.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(mem.join("operator.md"), "# Operator\nPrefers metric units.\n").unwrap();
        let r = sync(&conn, &mem).unwrap();
        assert_eq!(r.external, vec!["operator.md".to_string()]);
        assert_eq!(search_text(&conn, "metric", 5).unwrap()[0].path, "operator.md");

        std::fs::remove_file(mem.join("projects").join("garden.md")).unwrap();
        assert_eq!(sync(&conn, &mem).unwrap().removed, vec!["projects/garden.md".to_string()]);
        let _ = std::fs::remove_dir_all(mem);
    }

    #[test]
    fn vector_search_and_fusion() {
        let (conn, mem) = setup();
        memory::write(&mem, "projects/a.md", "# A\n\n## One\nfirst\n").unwrap();
        sync(&conn, &mem).unwrap();
        let todo = chunks_needing_embeddings(&conn, "m", 100).unwrap();
        assert!(!todo.is_empty());
        for (i, (id, _)) in todo.iter().enumerate() {
            store_embedding(&conn, *id, "m", &[i as f32, 1.0, 0.0]).unwrap();
        }
        assert!(chunks_needing_embeddings(&conn, "m", 100).unwrap().is_empty());
        let hits = search_vector(&conn, "m", &[0.0, 1.0, 0.0], 3).unwrap();
        assert!((hits[0].score - 1.0).abs() < 1e-6);
        let h = |p: &str| Hit { path: p.into(), heading: String::new(), text: p.into(), score: 0.0 };
        let fused = fuse(&[(vec![h("x"), h("y")], 1.0), (vec![h("y"), h("z")], 1.0)], 3);
        assert_eq!(fused[0].path, "y", "found by both lists ranks first");
        // The best meaning match beats a passage that every keyword search
        // catches low down and meaning ranks low too (seen on Windows: "What
        // database did we choose…" put the Decisions section 11th).
        let keyword = (vec![h("a"), h("b"), h("c"), h("d"), h("noise")], 1.0);
        let meaning = (vec![h("answer"), h("e"), h("f"), h("g"), h("h"), h("noise")], MEANING_WEIGHT);
        assert_eq!(fuse(&[keyword, meaning], 3)[0].path, "answer");
        // An exact keyword match that meaning also ranks first stays first.
        let keyword = (vec![h("exact"), h("a")], 1.0);
        let meaning = (vec![h("exact"), h("b")], MEANING_WEIGHT);
        assert_eq!(fuse(&[keyword, meaning], 3)[0].path, "exact");
        let _ = std::fs::remove_dir_all(mem);
    }
}
