//! Free RAM and VRAM, read locally (no network): the OS memory API for RAM and
//! `nvidia-smi` for NVIDIA GPUs.

use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryInfo {
    pub total_ram: u64,
    pub free_ram: u64,
    pub gpu_name: Option<String>,
    pub total_vram: Option<u64>,
    pub free_vram: Option<u64>,
}

pub fn snapshot() -> MemoryInfo {
    let (total_ram, free_ram) = ram().unwrap_or((0, 0));
    let gpu = vram();
    MemoryInfo {
        total_ram,
        free_ram,
        gpu_name: gpu.as_ref().map(|g| g.0.clone()),
        total_vram: gpu.as_ref().map(|g| g.1),
        free_vram: gpu.as_ref().map(|g| g.2),
    }
}

#[cfg(windows)]
fn ram() -> Option<(u64, u64)> {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }
    // SAFETY: the struct matches MEMORYSTATUSEX and `length` is set as the API requires.
    unsafe {
        let mut s: MemoryStatusEx = std::mem::zeroed();
        s.length = std::mem::size_of::<MemoryStatusEx>() as u32;
        (GlobalMemoryStatusEx(&mut s) != 0).then_some((s.total_phys, s.avail_phys))
    }
}

#[cfg(target_os = "linux")]
fn ram() -> Option<(u64, u64)> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = |name: &str| {
        info.lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .map(|kb| kb * 1024)
    };
    Some((field("MemTotal:")?, field("MemAvailable:")?))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn ram() -> Option<(u64, u64)> {
    None
}

/// (name, total bytes, free bytes) of the first NVIDIA GPU, if any.
fn vram() -> Option<(String, u64, u64)> {
    let mut cmd = std::process::Command::new("nvidia-smi");
    cmd.args(["--query-gpu=name,memory.total,memory.free", "--format=csv,noheader,nounits"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().ok().filter(|o| o.status.success())?;
    parse_nvidia_smi(&String::from_utf8_lossy(&out.stdout))
}

fn parse_nvidia_smi(text: &str) -> Option<(String, u64, u64)> {
    let line = text.lines().next()?;
    let mut parts = line.split(',').map(str::trim);
    let name = parts.next()?.to_string();
    let total: u64 = parts.next()?.parse().ok()?;
    let free: u64 = parts.next()?.parse().ok()?;
    Some((name, total * 1024 * 1024, free * 1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nvidia_smi_csv() {
        let (name, total, free) = parse_nvidia_smi("NVIDIA GeForce RTX 5070, 12227, 11036\n").unwrap();
        assert_eq!(name, "NVIDIA GeForce RTX 5070");
        assert_eq!(total, 12227 * 1024 * 1024);
        assert_eq!(free, 11036 * 1024 * 1024);
        assert!(parse_nvidia_smi("").is_none());
    }

    #[test]
    fn reads_real_ram() {
        let m = snapshot();
        assert!(m.total_ram > 0 && m.free_ram > 0 && m.free_ram <= m.total_ram);
    }
}
