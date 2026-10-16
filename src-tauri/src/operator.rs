//! Operator permissions and the rules that guard everything outside the chat:
//! internet access, downloads, and automatic learning. Enforced here in the
//! backend; nothing the model writes can reach these settings.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::db;

/// Every permission is off by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OperatorSettings {
    /// Blocks all internet access and pauses learning and memory writing.
    pub lockdown: bool,
    /// Lets the operator start (approved, one-time) model downloads at all.
    pub allow_downloads: bool,
    /// URL prefixes downloads may come from.
    pub download_allowlist: Vec<String>,
    /// Apply proposed memory updates without asking first.
    pub auto_apply_memory: bool,
    /// Stop proposing and writing memory updates.
    pub memory_paused: bool,
    /// Let Democritus switch brains per task on its own.
    pub allow_auto_model_select: bool,
    /// Allow local fine-tuning runs (also needs downloads the operator approves).
    pub allow_training: bool,
}

impl Default for OperatorSettings {
    fn default() -> Self {
        Self {
            lockdown: false,
            allow_downloads: false,
            download_allowlist: vec!["https://huggingface.co/Qwen/".into()],
            auto_apply_memory: false,
            memory_paused: false,
            allow_auto_model_select: false,
            allow_training: false,
        }
    }
}

impl OperatorSettings {
    /// Memory may be written right now (not paused, not locked down).
    pub fn memory_writable(&self) -> bool {
        !self.lockdown && !self.memory_paused
    }

    /// Normalizes allowlist entries: https only, lower-case host, trailing slash
    /// (so "https://huggingface.co/Qwen" never matches ".../Qwen-evil/").
    pub fn normalized(mut self) -> Result<Self, String> {
        let mut out = Vec::new();
        for raw in &self.download_allowlist {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            let url = reqwest::Url::parse(raw).map_err(|e| format!("Allowlist entry \"{raw}\" is not a valid URL: {e}"))?;
            if url.scheme() != "https" {
                return Err(format!("Allowlist entry \"{raw}\" must use https://"));
            }
            let host = url.host_str().ok_or_else(|| format!("Allowlist entry \"{raw}\" has no host"))?.to_lowercase();
            let mut path = url.path().to_string();
            if !path.ends_with('/') {
                path.push('/');
            }
            let entry = format!("https://{host}{path}");
            if !out.contains(&entry) {
                out.push(entry);
            }
        }
        self.download_allowlist = out;
        Ok(self)
    }

}

pub fn load(conn: &rusqlite::Connection) -> OperatorSettings {
    conn.query_row("SELECT value FROM settings WHERE key = 'operator'", [], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

pub fn save(conn: &rusqlite::Connection, s: &OperatorSettings) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('operator', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![serde_json::to_string(s).expect("serialize")],
    )?;
    Ok(())
}

/// Formats that can execute code when loaded (pickle and friends).
const REFUSED_EXTENSIONS: &[&str] = &["bin", "pt", "pth", "pkl", "pickle", "ckpt", "joblib", "npy", "npz", "h5"];
const ALLOWED_EXTENSIONS: &[&str] = &["gguf", "safetensors"];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTarget {
    pub url: String,
    pub file_name: String,
}

/// Checks a download URL against the rules: https, on the allowlist, and a
/// .gguf or .safetensors file.
pub fn validate_download_url(url: &str, allowlist: &[String]) -> Result<DownloadTarget, String> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|e| format!("Not a valid URL: {e}"))?;
    if parsed.scheme() != "https" {
        return Err("Downloads must use https://".into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("URLs with embedded credentials are not allowed.".into());
    }
    let host = parsed.host_str().ok_or("The URL has no host.")?.to_lowercase();
    // Url::parse has already resolved any "." and ".." segments.
    let normalized = format!("https://{host}{}", parsed.path());
    if !allowlist.iter().any(|prefix| normalized.starts_with(prefix.as_str())) {
        return Err(format!("{normalized} is not on your download allowlist ({}).", allowlist.join(", ")));
    }
    let file_name = parsed.path_segments().and_then(|mut s| s.next_back()).unwrap_or_default().to_string();
    let ext = file_name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    if REFUSED_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!(".{ext} files are refused: pickle-based formats can run code when loaded. Use .gguf or .safetensors."));
    }
    if !ALLOWED_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!("Only .gguf and .safetensors files can be downloaded (got \"{file_name}\")."));
    }
    Ok(DownloadTarget { url: normalized, file_name })
}

/// Checks a downloaded file's first bytes really are GGUF / safetensors, and
/// not an executable, script, archive or pickle.
pub fn sniff(head: &[u8], file_name: &str) -> Result<(), String> {
    let ext = file_name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    let bad = [
        (&b"MZ"[..], "a Windows executable"),
        (&b"\x7fELF"[..], "a Linux executable"),
        (&b"#!"[..], "a script"),
        (&b"PK\x03\x04"[..], "a zip archive (PyTorch pickles are zips)"),
        (&b"\x80"[..], "a Python pickle"),
        (&b"<"[..], "an HTML page, not a model file"),
    ];
    for (magic, what) in bad {
        if head.starts_with(magic) {
            return Err(format!("Refused: {file_name} is {what}."));
        }
    }
    match ext.as_str() {
        "gguf" if head.starts_with(b"GGUF") => Ok(()),
        "gguf" => Err(format!("Refused: {file_name} doesn't start with the GGUF signature.")),
        "safetensors" if head.len() >= 9 && head[8] == b'{' => Ok(()),
        "safetensors" => Err(format!("Refused: {file_name} doesn't look like a safetensors file.")),
        _ => Err(format!("Refused: {file_name} is not an allowed model format.")),
    }
}

/// A pending request for the operator to approve. Each approval is single-use:
/// it is removed the moment it's approved or denied.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub id: String,
    /// lookup (one HEAD request for size/checksum) | download
    pub kind: String,
    pub url: String,
    pub file_name: String,
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
    /// Where the file is actually served from, if the server redirects.
    pub served_from: Option<String>,
    pub created_ms: i64,
}

#[derive(Default)]
pub struct Approvals {
    pending: Mutex<HashMap<String, Approval>>,
    counter: AtomicU64,
}

impl Approvals {
    pub fn create(&self, mut a: Approval) -> Approval {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        a.id = format!("ap-{}-{n}", db::now_ms());
        a.created_ms = db::now_ms();
        self.pending.lock().unwrap_or_else(|p| p.into_inner()).insert(a.id.clone(), a.clone());
        a
    }

    /// Removes and returns the approval: it can be used once only.
    pub fn take(&self, id: &str) -> Option<Approval> {
        self.pending.lock().unwrap_or_else(|p| p.into_inner()).remove(id)
    }

    pub fn clear(&self) {
        self.pending.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow() -> Vec<String> {
        OperatorSettings::default().normalized().unwrap().download_allowlist
    }

    #[test]
    fn defaults_are_all_off() {
        let s = OperatorSettings::default();
        assert!(!s.lockdown && !s.allow_downloads && !s.auto_apply_memory && !s.memory_paused && !s.allow_auto_model_select && !s.allow_training);
        assert_eq!(s.download_allowlist, vec!["https://huggingface.co/Qwen/"]);
    }

    #[test]
    fn accepts_allowlisted_gguf_and_safetensors() {
        let t = validate_download_url("https://huggingface.co/Qwen/Qwen3-Embedding-0.6B-GGUF/resolve/main/Qwen3-Embedding-0.6B-Q8_0.gguf", &allow()).unwrap();
        assert_eq!(t.file_name, "Qwen3-Embedding-0.6B-Q8_0.gguf");
        assert!(validate_download_url("https://HuggingFace.co/Qwen/x/resolve/main/model-00001-of-00002.safetensors", &allow()).is_ok());
    }

    #[test]
    fn refuses_everything_else() {
        let a = allow();
        let refused = [
            "http://huggingface.co/Qwen/x/resolve/main/m.gguf",            // not https
            "https://huggingface.co/Qwen-evil/x/resolve/main/m.gguf",      // lookalike org
            "https://huggingface.co/someone/x/resolve/main/m.gguf",        // other org
            "https://huggingface.co/Qwen/../someone/x/m.gguf",             // traversal
            "https://evil.example/huggingface.co/Qwen/m.gguf",             // other host
            "https://user:pw@huggingface.co/Qwen/x/m.gguf",                // credentials
            "https://huggingface.co/Qwen/x/resolve/main/pytorch_model.bin", // pickle
            "https://huggingface.co/Qwen/x/resolve/main/model.pt",
            "https://huggingface.co/Qwen/x/resolve/main/setup.exe",
            "https://huggingface.co/Qwen/x/resolve/main/install.ps1",
        ];
        for url in refused {
            assert!(validate_download_url(url, &a).is_err(), "should refuse {url}");
        }
        assert!(validate_download_url("https://huggingface.co/Qwen/x/m.bin", &a).unwrap_err().contains("pickle"));
    }

    #[test]
    fn allowlist_entries_are_normalized() {
        let s = OperatorSettings { download_allowlist: vec!["https://HuggingFace.co/Qwen".into(), " ".into()], ..Default::default() };
        assert_eq!(s.normalized().unwrap().download_allowlist, vec!["https://huggingface.co/Qwen/"]);
        let bad = OperatorSettings { download_allowlist: vec!["http://huggingface.co/Qwen".into()], ..Default::default() };
        assert!(bad.normalized().is_err());
    }

    #[test]
    fn sniffing_rejects_executables_and_pickles() {
        assert!(sniff(b"GGUF\x03\x00\x00\x00", "m.gguf").is_ok());
        let mut st = 100u64.to_le_bytes().to_vec();
        st.push(b'{');
        assert!(sniff(&st, "m.safetensors").is_ok());
        assert!(sniff(b"MZ\x90\x00", "m.gguf").unwrap_err().contains("executable"));
        assert!(sniff(b"PK\x03\x04", "m.safetensors").is_err());
        assert!(sniff(b"\x80\x02}q", "m.gguf").is_err());
        assert!(sniff(b"<!DOCTYPE html>", "m.gguf").is_err());
        assert!(sniff(b"NOTGGUF", "m.gguf").is_err());
    }

    #[test]
    fn approvals_are_single_use() {
        let ap = Approvals::default();
        let a = ap.create(Approval {
            id: String::new(),
            kind: "lookup".into(),
            url: "u".into(),
            file_name: "f".into(),
            size_bytes: None,
            sha256: None,
            served_from: None,
            created_ms: 0,
        });
        assert!(ap.take(&a.id).is_some());
        assert!(ap.take(&a.id).is_none(), "second use refused");
    }
}
