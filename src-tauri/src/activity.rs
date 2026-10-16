//! Local activity log: one JSON object per line in <home>\logs\activity-YYYY-MM-DD.jsonl.
//! Records network requests, downloads, approvals, memory changes, setting
//! changes and security events. Nothing here ever leaves the computer.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::{db, localtime};

pub struct ActivityLog {
    dir: Mutex<PathBuf>,
}

impl ActivityLog {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir: Mutex::new(dir) }
    }

    pub fn set_dir(&self, dir: PathBuf) {
        *self.dir.lock().unwrap_or_else(|p| p.into_inner()) = dir;
    }

    pub fn dir(&self) -> PathBuf {
        self.dir.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// kind: network | download | approval | memory | setting | security | model
    pub fn log(&self, kind: &str, action: &str, detail: Value) {
        let t = localtime::now();
        let entry = json!({ "ts": db::now_ms(), "time": t.datetime(), "kind": kind, "action": action, "detail": detail });
        let dir = self.dir();
        let result = std::fs::create_dir_all(&dir).and_then(|_| {
            let mut f = OpenOptions::new().create(true).append(true).open(dir.join(format!("activity-{}.jsonl", t.date())))?;
            writeln!(f, "{entry}")
        });
        if let Err(e) = result {
            eprintln!("activity log write failed: {e}");
        }
    }

    /// Newest entries first, optionally filtered by kind and text.
    pub fn read(&self, limit: usize, kind: Option<&str>, query: Option<&str>) -> Vec<Value> {
        let dir = self.dir();
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("activity-") && n.ends_with(".jsonl")))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        files.reverse();
        let query = query.map(|q| q.to_lowercase()).filter(|q| !q.is_empty());
        let mut out = Vec::new();
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else { continue };
            for line in text.lines().rev() {
                let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
                if kind.is_some_and(|k| v["kind"].as_str() != Some(k)) {
                    continue;
                }
                if let Some(q) = &query {
                    if !line.to_lowercase().contains(q) {
                        continue;
                    }
                }
                out.push(v);
                if out.len() >= limit {
                    return out;
                }
            }
        }
        out
    }
}

/// Top-level fields that differ between two serializable values: {field: {from, to}}.
pub fn diff<T: serde::Serialize>(before: &T, after: &T) -> serde_json::Map<String, Value> {
    let a = serde_json::to_value(before).unwrap_or_default();
    let b = serde_json::to_value(after).unwrap_or_default();
    let mut out = serde_json::Map::new();
    if let (Some(a), Some(b)) = (a.as_object(), b.as_object()) {
        for (k, va) in a {
            let vb = b.get(k).cloned().unwrap_or(Value::Null);
            if *va != vb {
                out.insert(k.clone(), json!({ "from": va, "to": vb }));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_reports_changed_fields_only() {
        let d = diff(&json!({"a": 1, "b": "x"}), &json!({"a": 2, "b": "x"}));
        assert_eq!(d.len(), 1);
        assert_eq!(d["a"], json!({"from": 1, "to": 2}));
    }

    #[test]
    fn writes_and_reads_newest_first_with_filters() {
        let dir = db::test_dir("activity");
        let log = ActivityLog::new(dir.clone());
        log.log("setting", "changed", json!({"field": "temperature", "from": 0.7, "to": 0.5}));
        log.log("network", "request", json!({"url": "http://127.0.0.1:11434/v1/chat/completions"}));
        log.log("security", "lockdown on", json!({}));
        let all = log.read(10, None, None);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0]["action"], "lockdown on");
        assert_eq!(log.read(10, Some("network"), None).len(), 1);
        assert_eq!(log.read(10, None, Some("TEMPERATURE")).len(), 1);
        assert_eq!(log.read(1, None, None).len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
