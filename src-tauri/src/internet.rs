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

#[cfg(test)]
mod tests {
    //! Offline tests against a throwaway HTTP server on 127.0.0.1.
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    /// Serves `body` once, in 64 KiB chunks with a small pause between them.
    fn serve_once(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            for chunk in body.chunks(64 * 1024) {
                if s.write_all(chunk).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        format!("http://127.0.0.1:{port}/model.gguf")
    }

    fn gguf(len: usize) -> Vec<u8> {
        let mut v = b"GGUF\x03\x00\x00\x00".to_vec();
        v.extend((0..len).map(|i| (i % 251) as u8));
        v
    }

    fn sha_hex(data: &[u8]) -> String {
        Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
    }

    fn approval(url: String, size: usize, sha: Option<String>) -> Approval {
        Approval { id: "t".into(), kind: "download".into(), url, file_name: "model.gguf".into(), size_bytes: Some(size as u64), sha256: sha, served_from: None, created_ms: 0 }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("democritus-dl-{name}-{}", crate::db::now_ms()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn verifies_checksum_and_keeps_file() {
        let body = gguf(300_000);
        let dir = temp_dir("ok");
        let a = approval(serve_once(body.clone()), body.len(), Some(sha_hex(&body)));
        let r = download(&client(), &a, &dir, &Notify::new(), &AtomicBool::new(false), |_, _| {}, |_, _, _| {}).await.unwrap();
        assert_eq!(r.checksum_matches, Some(true));
        assert_eq!(std::fs::read(dir.join("model.gguf")).unwrap(), body);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn checksum_mismatch_discards_the_file() {
        let body = gguf(200_000);
        let dir = temp_dir("bad-sha");
        let a = approval(serve_once(body.clone()), body.len(), Some("0".repeat(64)));
        let e = download(&client(), &a, &dir, &Notify::new(), &AtomicBool::new(false), |_, _| {}, |_, _, _| {}).await.unwrap_err();
        assert!(e.contains("Checksum mismatch"), "{e}");
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none(), "nothing left behind");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn executable_disguised_as_gguf_is_refused() {
        let mut body = b"MZ\x90\x00\x03\x00\x00\x00\x04\x00\x00\x00\xff\xff\x00\x00".to_vec();
        body.extend(vec![0u8; 100_000]);
        let dir = temp_dir("exe");
        let a = approval(serve_once(body.clone()), body.len(), None);
        let e = download(&client(), &a, &dir, &Notify::new(), &AtomicBool::new(false), |_, _| {}, |_, _, _| {}).await.unwrap_err();
        assert!(e.contains("executable"), "{e}");
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn lockdown_cancels_a_running_download() {
        let body = gguf(4_000_000);
        let dir = temp_dir("lockdown");
        let a = approval(serve_once(body.clone()), body.len(), None);
        let stop = AtomicBool::new(false);
        let e = download(&client(), &a, &dir, &Notify::new(), &stop, |done, _| {
            if done > 500_000 {
                stop.store(true, Ordering::SeqCst); // the operator hits Lockdown
            }
        }, |_, _, _| {})
        .await
        .unwrap_err();
        assert!(e.contains("Lockdown"), "{e}");
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none(), "partial file removed");
        let _ = std::fs::remove_dir_all(dir);
    }
}
