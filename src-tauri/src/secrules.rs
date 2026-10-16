//! Pure rules behind the Security check (security.rs): parsing what Windows
//! reports, building the firewall commands, and scanning model files. No
//! Tauri here, so all of it is unit-tested on any OS. Most of it is only
//! called on Windows.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};

/// Whether an OLLAMA_HOST value makes Ollama listen on loopback only. Mirrors
/// Ollama's own parsing: an optional scheme, then host[:port]; an empty host
/// (":11434") means every interface.
pub fn host_is_loopback(value: &str) -> bool {
    let v = value.trim();
    if v.is_empty() {
        return true; // unset: Ollama's default is 127.0.0.1:11434
    }
    let v = v.split_once("://").map_or(v, |(_, rest)| rest);
    let v = v.split('/').next().unwrap_or_default();
    let host = if let Some(rest) = v.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default().to_string()
    } else if v.matches(':').count() > 1 {
        v.to_string() // bare IPv6 without brackets or port
    } else {
        v.rsplit_once(':').map_or(v, |(h, _)| h).to_string()
    };
    is_loopback_host(&host)
}

pub fn is_loopback_host(host: &str) -> bool {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']');
    if h.eq_ignore_ascii_case("localhost") {
        return true;
    }
    h.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Local addresses listening on `port`, from `netstat -an` output. Listening
/// sockets are recognized by their foreign address (0.0.0.0:0 or [::]:0), so
/// this works whatever language Windows shows the state column in.
pub fn parse_netstat_listeners(text: &str, port: u16) -> Vec<String> {
    let suffix = format!(":{port}");
    let mut out = Vec::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() < 3 || !t[0].eq_ignore_ascii_case("TCP") {
            continue;
        }
        let (local, foreign) = (t[1], t[2]);
        let listening = foreign == "0.0.0.0:0" || foreign == "[::]:0";
        if listening && local.ends_with(&suffix) {
            let host = local[..local.len() - suffix.len()].to_string();
            if !out.contains(&host) {
                out.push(host);
            }
        }
    }
    out
}

/// The value of `name` in `reg query <key> /v <name>` output.
pub fn parse_reg_query(text: &str, name: &str) -> Option<String> {
    for line in text.lines() {
        let l = line.trim();
        let Some(rest) = l.get(..name.len()).filter(|h| h.eq_ignore_ascii_case(name)).and_then(|_| l.get(name.len()..)) else { continue };
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim_start();
        let (kind, value) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        if kind.starts_with("REG_") {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// Expands %NAME% references, as Windows does for REG_EXPAND_SZ values.
/// Unknown names are left as they are.
pub fn expand_env(s: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        let Some(len) = rest[start + 1..].find('%') else { break };
        let name = &rest[start + 1..start + 1 + len];
        out.push_str(&rest[..start]);
        match lookup(name).filter(|_| !name.is_empty()) {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out.push_str(rest);
    out
}

/// Single-quoted PowerShell string.
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Every address except loopback, so a block rule can never cut Democritus off
/// from the local model server on 127.0.0.1.
const NON_LOOPBACK: &str = "'0.0.0.0-126.255.255.255','128.0.0.0-255.255.255.255','::2-ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff'";

pub fn firewall_command(label: &str, program: &Path) -> String {
    let path = program.display().to_string();
    let file = path.rsplit(['\\', '/']).next().unwrap_or_default();
    format!(
        "New-NetFirewallRule -DisplayName {} -Group 'Democritus' -Direction Outbound -Action Block -Program {} -RemoteAddress {NON_LOOPBACK} -Profile Any",
        ps_quote(&format!("Democritus: block internet for {label} ({file})")),
        ps_quote(&path),
    )
}

/// Base64 of UTF-16LE text, as `powershell -EncodedCommand` expects.
pub fn encode_command(script: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Same path, ignoring case and slash direction (Windows paths).
pub fn same_path(a: &str, b: &Path) -> bool {
    let norm = |s: &str| s.trim().trim_matches('"').replace('/', "\\").to_lowercase();
    norm(a) == norm(&b.display().to_string())
}

/// Magic bytes that mean "this can run code", whatever the file is called.
pub fn dangerous_magic(head: &[u8]) -> Option<&'static str> {
    [
        (&b"MZ"[..], "a Windows executable"),
        (&b"\x7fELF"[..], "a Linux executable"),
        (&b"#!"[..], "a script"),
        (&b"PK\x03\x04"[..], "a zip archive (PyTorch pickles are zips)"),
        (&b"\x80"[..], "a Python pickle"),
    ]
    .into_iter()
    .find(|(m, _)| head.starts_with(m))
    .map(|(_, what)| what)
}

#[derive(Debug, Default, PartialEq)]
pub struct ScanResult {
    pub scanned: usize,
    pub problems: Vec<String>,
}

fn head_of(path: &Path) -> Vec<u8> {
    use std::io::Read;
    let mut buf = [0u8; 16];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
    buf[..n].to_vec()
}

/// Democritus's downloads folder: only .gguf / .safetensors files whose first
/// bytes match their type (unfinished `.part` downloads are skipped).
pub fn scan_downloads(dir: &Path) -> ScanResult {
    let mut r = ScanResult::default();
    for path in files_in(dir, 2) {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.ends_with(".part") {
            continue;
        }
        r.scanned += 1;
        if let Err(e) = crate::operator::sniff(&head_of(&path), &name) {
            r.problems.push(format!("{}: {}", path.display(), e.trim_start_matches("Refused: ")));
        }
    }
    r
}

/// Ollama's model store keeps files as `blobs/sha256-…` without extensions, so
/// only the content can be checked: model weights must not be executables,
/// archives or pickles. (Small text blobs hold templates and licenses.)
pub fn scan_ollama_blobs(models_dir: &Path) -> ScanResult {
    let mut r = ScanResult::default();
    for path in files_in(&models_dir.join("blobs"), 1) {
        r.scanned += 1;
        if let Some(what) = dangerous_magic(&head_of(&path)) {
            r.problems.push(format!("{} is {what}", path.display()));
        }
    }
    r
}

fn files_in(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_file() => out.push(p),
            Ok(t) if t.is_dir() && depth > 1 => out.extend(files_in(&p, depth - 1)),
            _ => {}
        }
    }
    out
}

// ---------- system probes (Windows) ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_host_values() {
        for ok in ["", "127.0.0.1", "127.0.0.1:11434", "localhost:11434", "http://127.0.0.1:11434", "[::1]:11434", "::1", "127.0.0.2:9"] {
            assert!(host_is_loopback(ok), "{ok} should be loopback");
        }
        for bad in ["0.0.0.0", "0.0.0.0:11434", ":11434", "192.168.1.5:11434", "http://0.0.0.0:11434", "[::]:11434", "example.com"] {
            assert!(!host_is_loopback(bad), "{bad} should not be loopback");
        }
    }

    #[test]
    fn netstat_listeners_any_language() {
        let text = "\r\nActive Connections\r\n\r\n  Proto  Local Address          Foreign Address        State\r\n\
            \x20 TCP    0.0.0.0:135            0.0.0.0:0              LISTENING\r\n\
            \x20 TCP    127.0.0.1:11434        0.0.0.0:0              ABHÖREN\r\n\
            \x20 TCP    127.0.0.1:11434        127.0.0.1:50122        HERGESTELLT\r\n\
            \x20 TCP    [::]:11434             [::]:0                 LISTENING\r\n\
            \x20 UDP    0.0.0.0:11434          *:*\r\n";
        assert_eq!(parse_netstat_listeners(text, 11434), vec!["127.0.0.1", "[::]"]);
        assert!(parse_netstat_listeners(text, 8080).is_empty());
        assert!(!is_loopback_host("[::]") && is_loopback_host("[::1]") && is_loopback_host("127.0.0.1"));
    }

    #[test]
    fn reg_query_values() {
        let text = "\r\nHKEY_CURRENT_USER\\Environment\r\n    OLLAMA_HOST    REG_SZ    0.0.0.0:11434\r\n\r\n";
        assert_eq!(parse_reg_query(text, "OLLAMA_HOST").as_deref(), Some("0.0.0.0:11434"));
        let models = "    OLLAMA_MODELS    REG_EXPAND_SZ    D:\\My Models\\ollama\r\n";
        assert_eq!(parse_reg_query(models, "OLLAMA_MODELS").as_deref(), Some("D:\\My Models\\ollama"));
        assert_eq!(parse_reg_query("ERROR: The system was unable to find the specified registry key or value.", "OLLAMA_HOST"), None);
        assert_eq!(parse_reg_query("    OLLAMA_HOSTX    REG_SZ    x", "OLLAMA_HOST"), None);
    }

    #[test]
    fn firewall_command_is_quoted_and_spares_loopback() {
        let c = firewall_command("Democritus", Path::new(r"C:\Users\O'Neil\AppData\Local\Democritus\democritus.exe"));
        assert!(c.starts_with("New-NetFirewallRule -DisplayName 'Democritus: block internet for Democritus (democritus.exe)'"));
        assert!(c.contains(r"-Program 'C:\Users\O''Neil\AppData\Local\Democritus\democritus.exe'"));
        assert!(c.contains("-Direction Outbound -Action Block"));
        assert!(c.contains("'0.0.0.0-126.255.255.255','128.0.0.0-255.255.255.255'"));
        assert!(!c.contains("127.0.0.1"));
    }

    #[test]
    fn encoded_command_is_utf16le_base64() {
        // [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes('dir'))
        assert_eq!(encode_command("dir"), "ZABpAHIA");
        assert_eq!(encode_command("a"), "YQA=");
        assert_eq!(encode_command(""), "");
    }

    #[test]
    fn expands_env_references() {
        let env = |n: &str| (n == "USERPROFILE").then(|| r"C:\Users\op".to_string());
        assert_eq!(expand_env(r"%USERPROFILE%\.ollama\models", env), r"C:\Users\op\.ollama\models");
        assert_eq!(expand_env(r"D:\models", env), r"D:\models");
        assert_eq!(expand_env(r"%NOPE%\x %", env), r"%NOPE%\x %");
    }

    #[test]
    fn paths_compare_like_windows() {
        assert!(same_path(r"c:\program files\ollama\OLLAMA.EXE", Path::new(r"C:\Program Files\Ollama\ollama.exe")));
        assert!(same_path("C:/x/y.exe", Path::new(r"C:\x\y.exe")));
        assert!(!same_path(r"C:\x\z.exe", Path::new(r"C:\x\y.exe")));
    }

    #[test]
    fn scans_downloads_and_blobs() {
        let dir = crate::db::test_dir("security-scan");
        let downloads = dir.join("downloads");
        std::fs::create_dir_all(downloads.join("sub")).unwrap();
        std::fs::write(downloads.join("good.gguf"), b"GGUF\x03\0\0\0rest").unwrap();
        std::fs::write(downloads.join("half.gguf.part"), b"MZ").unwrap();
        std::fs::write(downloads.join("sub").join("evil.bin"), b"\x80\x02}q").unwrap();
        std::fs::write(downloads.join("fake.gguf"), b"MZ\x90\0").unwrap();
        let d = scan_downloads(&downloads);
        assert_eq!(d.scanned, 3);
        assert_eq!(d.problems.len(), 2, "{:?}", d.problems);

        let blobs = dir.join("models").join("blobs");
        std::fs::create_dir_all(&blobs).unwrap();
        std::fs::write(blobs.join("sha256-aa"), b"GGUF\x03\0\0\0").unwrap();
        std::fs::write(blobs.join("sha256-bb"), b"<|im_start|>{{ .System }}").unwrap();
        std::fs::write(blobs.join("sha256-cc"), b"{\"model_format\":\"gguf\"}").unwrap();
        let o = scan_ollama_blobs(&dir.join("models"));
        assert_eq!(o, ScanResult { scanned: 3, problems: vec![] });
        std::fs::write(blobs.join("sha256-dd"), b"PK\x03\x04").unwrap();
        assert_eq!(scan_ollama_blobs(&dir.join("models")).problems.len(), 1);
        assert_eq!(scan_downloads(&dir.join("missing")), ScanResult::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
