//! The only code that talks to the internet. It is reachable solely through a
//! single-use operator approval (see operator::Approvals), is blocked by
//! Lockdown, and every request is written to the activity log.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use crate::operator::{self, Approval};

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        // Redirects are followed by hand (at most one hop) so the operator sees
        // and approves the host a file is actually served from.
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .user_agent("Democritus (local desktop app)")
        .build()
        .expect("failed to build download client")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupResult {
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
    pub served_from: Option<String>,
}

fn header(resp: &reqwest::Response, name: &str) -> Option<String> {
    resp.headers().get(name).and_then(|v| v.to_str().ok()).map(|s| s.trim_matches('"').to_string())
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn transport_error(e: &reqwest::Error) -> String {
    let text = e.to_string();
    if text.contains("scheme") || text.contains("https") || text.contains("tls") {
        format!("HTTPS isn't available in this build ({text}). The TLS support crates need the operator's approval first.")
    } else {
        format!("Network request failed: {text}")
    }
}

/// One HEAD request (plus at most one redirect hop) to learn size and checksum.
/// Hugging Face reports LFS files' SHA-256 in `x-linked-etag`.
pub async fn lookup(http: &reqwest::Client, url: &str, log: impl Fn(&str, &str, u16)) -> Result<LookupResult, String> {
    let resp = http.head(url).send().await.map_err(|e| transport_error(&e))?;
    log("HEAD", url, resp.status().as_u16());
    let mut size = header(&resp, "x-linked-size").and_then(|s| s.parse().ok());
    let mut sha = header(&resp, "x-linked-etag").filter(|s| is_sha256(s));
    let mut served_from = None;
    if resp.status().is_redirection() {
        let location = header(&resp, "location").ok_or("The server redirected without saying where.")?;
        let next = reqwest::Url::parse(url).and_then(|base| base.join(&location)).map_err(|e| format!("Bad redirect: {e}"))?;
        if next.scheme() != "https" {
            return Err("Refused: the server redirected to a non-https address.".into());
        }
        served_from = next.host_str().map(String::from);
        if size.is_none() {
            let r2 = http.head(next.as_str()).send().await.map_err(|e| transport_error(&e))?;
            log("HEAD", next.as_str(), r2.status().as_u16());
            size = header(&r2, "content-length").and_then(|s| s.parse().ok());
            sha = sha.or_else(|| header(&r2, "etag").filter(|s| is_sha256(s)));
        }
    } else if resp.status().is_success() {
        size = size.or_else(|| header(&resp, "content-length").and_then(|s| s.parse().ok()));
        sha = sha.or_else(|| header(&resp, "etag").filter(|s| is_sha256(s)));
    } else {
        return Err(format!("The server answered HTTP {} for {url}.", resp.status()));
    }
    Ok(LookupResult { size_bytes: size, sha256: sha, served_from })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadResult {
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
    /// Some(true) when it matched the published checksum, None if none was published.
    pub checksum_matches: Option<bool>,
}

/// Downloads exactly what the operator approved, into `dest_dir`.
pub async fn download(
    http: &reqwest::Client,
    approval: &Approval,
    dest_dir: &Path,
    cancel: &Notify,
    stop: &AtomicBool,
    mut progress: impl FnMut(u64, Option<u64>),
    log: impl Fn(&str, &str, u16),
) -> Result<DownloadResult, String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    let mut resp = http.get(&approval.url).send().await.map_err(|e| transport_error(&e))?;
    log("GET", &approval.url, resp.status().as_u16());
    if resp.status().is_redirection() {
        let location = header(&resp, "location").ok_or("The server redirected without saying where.")?;
        let next = reqwest::Url::parse(&approval.url).and_then(|b| b.join(&location)).map_err(|e| e.to_string())?;
        // Only the host the operator saw in the approval dialog.
        if next.scheme() != "https" || next.host_str() != approval.served_from.as_deref() {
            return Err(format!("Refused: the file is now served from {}, which you didn't approve.", next.host_str().unwrap_or("?")));
        }
        resp = http.get(next.as_str()).send().await.map_err(|e| transport_error(&e))?;
        log("GET", next.as_str(), resp.status().as_u16());
    }
    if !resp.status().is_success() {
        return Err(format!("Download failed: HTTP {}", resp.status()));
    }
    let total = resp.content_length().or(approval.size_bytes);

    let final_path: PathBuf = dest_dir.join(&approval.file_name);
    let part_path = dest_dir.join(format!("{}.part", approval.file_name));
    let result = async {
        let mut file = File::create(&part_path).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        let mut written: u64 = 0;
        let mut head: Vec<u8> = Vec::with_capacity(16);
        loop {
            if stop.load(Ordering::SeqCst) {
                return Err("Download cancelled: Lockdown is on.".to_string());
            }
            let chunk = tokio::select! {
                _ = cancel.notified() => return Err("Download cancelled.".to_string()),
                c = resp.chunk() => c.map_err(|e| format!("Download interrupted: {e}"))?,
            };
            let Some(chunk) = chunk else { break };
            if head.len() < 16 {
                head.extend_from_slice(&chunk[..chunk.len().min(16 - head.len())]);
                if head.len() >= 16 {
                    operator::sniff(&head, &approval.file_name)?; // stop early on the wrong kind of file
                }
            }
            hasher.update(&chunk);
            file.write_all(&chunk).map_err(|e| e.to_string())?;
            written += chunk.len() as u64;
            progress(written, total);
        }
        file.flush().map_err(|e| e.to_string())?;
        operator::sniff(&head, &approval.file_name)?;
        if let Some(expected) = approval.size_bytes {
            if expected != written {
                return Err(format!("Size mismatch: expected {expected} bytes, got {written}."));
            }
        }
        let sha: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        let checksum_matches = approval.sha256.as_ref().map(|s| s.eq_ignore_ascii_case(&sha));
        if checksum_matches == Some(false) {
            return Err(format!("Checksum mismatch: expected {}, got {sha}. The file was discarded.", approval.sha256.as_deref().unwrap_or("")));
        }
        Ok((written, sha, checksum_matches))
    }
    .await;

    match result {
        Ok((size, sha, checksum_matches)) => {
            std::fs::rename(&part_path, &final_path).map_err(|e| e.to_string())?;
            Ok(DownloadResult { path: final_path.display().to_string(), size_bytes: size, sha256: sha, checksum_matches })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part_path);
            Err(e)
        }
    }
}
