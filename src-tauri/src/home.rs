//! Democritus's home folder (default D:\Democritus): memory, logs, training data.
//! The app writes only inside this folder (plus its own app-data database).

use std::path::{Path, PathBuf};

/// D:\Democritus when a D: drive exists (as the operator asked), otherwise
/// ~/Democritus.
pub fn default_home() -> PathBuf {
    if cfg!(windows) && Path::new("D:\\").exists() {
        return PathBuf::from("D:\\Democritus");
    }
    let user = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    user.join("Democritus")
}

/// DEMOCRITUS_HOME (used by automated tests) wins over the setting.
pub fn resolve(setting: &str) -> PathBuf {
    if let Some(p) = std::env::var_os("DEMOCRITUS_HOME") {
        return PathBuf::from(p);
    }
    let s = setting.trim();
    if s.is_empty() {
        default_home()
    } else {
        PathBuf::from(s)
    }
}

pub fn logs_dir(home: &Path) -> PathBuf {
    home.join("logs")
}

pub fn memory_dir(home: &Path) -> PathBuf {
    home.join("memory")
}

pub fn training_dir(home: &Path) -> PathBuf {
    home.join("training")
}

pub fn backups_dir(home: &Path) -> PathBuf {
    home.join("backups")
}

pub fn downloads_dir(home: &Path) -> PathBuf {
    home.join("downloads")
}
