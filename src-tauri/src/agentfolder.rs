//! The coding agent's sandbox folder, held by the backend: the folder picked
//! now, and the last and recent ones saved in agent-folders.json next to the
//! database. Every folder passes Workspace::open before it is used or saved,
//! and the saved one is checked again on start. No Tauri here, so it is
//! unit-tested on any OS; agent.rs wraps it in commands.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::workspace::{display_path, Refusal, Workspace};

/// File name inside the app data folder (where democritus.db lives).
pub const FILE: &str = "agent-folders.json";
pub const PICK_FIRST: &str = "Pick a folder first.";
const MAX_RECENT: usize = 6;

/// What agent-folders.json holds. Folders are in display form (D:\projects\app).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    #[serde(default)]
    pub last: Option<String>,
    /// Most recent first.
    #[serde(default)]
    pub recent: Vec<String>,
}

impl Saved {
    /// Makes `folder` the last one and moves it to the top of the recent list.
    pub fn remember(&mut self, folder: &str) {
        self.last = Some(folder.to_string());
        self.recent.retain(|f| !same_folder(f, folder));
        self.recent.insert(0, folder.to_string());
        self.recent.truncate(MAX_RECENT);
    }
}

/// On Windows, paths that differ only in case are the same folder.
fn same_folder(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.to_lowercase() == b.to_lowercase()
    } else {
        a == b
    }
}

/// The saved folders; a missing or unreadable file means none.
pub fn load(file: &Path) -> Saved {
    std::fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Writes to a temporary file first, then renames it over the old one.
pub fn save(file: &Path, saved: &Saved) -> Result<(), String> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Can't create {}: {e}", dir.display()))?;
    }
    let tmp = file.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(saved).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).map_err(|e| format!("Can't save {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, file).map_err(|e| format!("Can't save {}: {e}", file.display()))
}

/// The folder state as the Code page sees it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderState {
    /// The current sandbox folder (display form), if one is set.
    pub folder: Option<String>,
    /// Why there is no usable folder ("Pick a folder first." when none was picked).
    pub error: Option<String>,
    /// Recently used folders that can still be opened, most recent first.
    pub recent: Vec<String>,
}

/// The current sandbox folder. `root` is canonical; None until one is picked.
#[derive(Debug, Default)]
pub struct Sandbox {
    loaded: bool,
    root: Option<PathBuf>,
    error: Option<String>,
}

impl Sandbox {
    /// The first time only: loads the saved folder and checks it again. A
    /// folder that can't be used any more is forgotten in the file and
    /// returned so the caller can log it.
    pub fn load(&mut self, file: &Path, own: &[PathBuf], protected: &[PathBuf]) -> Option<Refusal> {
        if self.loaded {
            return None;
        }
        self.loaded = true;
        let mut saved = load(file);
        let last = saved.last.clone().filter(|l| !l.trim().is_empty())?;
        match Workspace::open(&last, own, protected) {
            Ok(ws) => {
                self.root = Some(ws.root().to_path_buf());
                None
            }
            Err(reason) => {
                saved.last = None;
                let _ = save(file, &saved);
                self.error = Some(format!("Your last folder ({last}) can't be used: {reason}"));
                Some(Refusal { path: last, reason })
            }
        }
    }

    /// Checks `folder` and makes it current, saved as the last and most recent
    /// one. On a refusal nothing changes. Returns the display form. (If the
    /// file can't be written the folder is still used this session.)
    pub fn set(&mut self, file: &Path, folder: &str, own: &[PathBuf], protected: &[PathBuf]) -> Result<String, String> {
        let ws = Workspace::open(folder, own, protected)?;
        let shown = ws.display_root();
        let mut saved = load(file);
        saved.remember(&shown);
        let _ = save(file, &saved);
        self.root = Some(ws.root().to_path_buf());
        self.error = None;
        Ok(shown)
    }

    /// No current folder, and none on the next start (recent ones are kept).
    pub fn clear(&mut self, file: &Path) {
        self.root = None;
        self.error = None;
        let mut saved = load(file);
        if saved.last.take().is_some() {
            let _ = save(file, &saved);
        }
    }

    /// The current folder in display form.
    pub fn folder(&self) -> Option<String> {
        self.root.as_deref().map(display_path)
    }

    /// Opens the current folder again, checking it as if it were just picked.
    /// If it can't be used any more it stops being current.
    pub fn open(&mut self, own: &[PathBuf], protected: &[PathBuf]) -> Result<Workspace, String> {
        let shown = self.folder().ok_or_else(|| PICK_FIRST.to_string())?;
        Workspace::open(&shown, own, protected).map_err(|reason| {
            self.root = None;
            self.error = Some(format!("Your last folder ({shown}) can't be used: {reason}"));
            reason
        })
    }

    pub fn state(&self, file: &Path, own: &[PathBuf], protected: &[PathBuf]) -> FolderState {
        let folder = self.folder();
        let error = self.error.clone().or_else(|| folder.is_none().then(|| PICK_FIRST.to_string()));
        let recent = load(file).recent.into_iter().filter(|f| Workspace::open(f, own, protected).is_ok()).collect();
        FolderState { folder, error, recent }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITSELF: &str = "The agent can't work on Democritus itself. Pick a project folder instead.";

    fn dirs(root: &Path, names: &[&str]) -> Vec<String> {
        names
            .iter()
            .map(|n| {
                let d = root.join(n);
                std::fs::create_dir_all(&d).unwrap();
                display_path(&std::fs::canonicalize(&d).unwrap())
            })
            .collect()
    }

    #[test]
    fn recent_is_deduplicated_newest_first_and_capped() {
        let mut s = Saved::default();
        for f in ["D:\\a", "D:\\b", "D:\\c", "D:\\d", "D:\\e", "D:\\f", "D:\\g"] {
            s.remember(f);
        }
        assert_eq!(s.last.as_deref(), Some("D:\\g"));
        assert_eq!(s.recent, ["D:\\g", "D:\\f", "D:\\e", "D:\\d", "D:\\c", "D:\\b"]);
        s.remember("D:\\d");
        assert_eq!(s.recent, ["D:\\d", "D:\\g", "D:\\f", "D:\\e", "D:\\c", "D:\\b"]);
        if cfg!(windows) {
            s.remember("d:\\E");
            assert_eq!(s.recent, ["d:\\E", "D:\\d", "D:\\g", "D:\\f", "D:\\c", "D:\\b"], "case-insensitive on Windows");
        }
    }

    #[test]
    fn saved_folders_round_trip_and_bad_files_mean_none() {
        let dir = crate::db::test_dir("agentfolder-file");
        let file = dir.join("data").join(FILE);
        assert_eq!(load(&file), Saved::default(), "no file yet");
        let s = Saved { last: Some("D:\\p".into()), recent: vec!["D:\\p".into(), "D:\\q".into()] };
        save(&file, &s).unwrap();
        assert_eq!(load(&file), s);
        assert!(!file.with_extension("json.tmp").exists());
        std::fs::write(&file, "{ not json").unwrap();
        assert_eq!(load(&file), Saved::default());
        std::fs::write(&file, r#"{"recent":["D:\\x"]}"#).unwrap();
        assert_eq!(load(&file), Saved { last: None, recent: vec!["D:\\x".into()] });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_folders_that_pass_the_check_are_saved() {
        let dir = crate::db::test_dir("agentfolder-set");
        let file = dir.join(FILE);
        let own = [dir.join("app")];
        let ok = dirs(&dir, &["p1", "p2", "app"]);
        let mut sb = Sandbox::default();
        assert_eq!(sb.set(&file, &ok[0], &own, &[]).unwrap(), ok[0]);
        assert_eq!(sb.set(&file, &ok[1], &own, &[]).unwrap(), ok[1]);
        // Refused: Democritus's own folder, a missing one, a relative one.
        assert_eq!(sb.set(&file, &ok[2], &own, &[]).unwrap_err(), ITSELF);
        assert!(sb.set(&file, dir.join("missing").to_str().unwrap(), &own, &[]).is_err());
        assert!(sb.set(&file, "p1", &own, &[]).is_err());
        assert_eq!(sb.folder().as_deref(), Some(ok[1].as_str()), "a refusal keeps the current folder");
        assert_eq!(load(&file), Saved { last: Some(ok[1].clone()), recent: vec![ok[1].clone(), ok[0].clone()] });
        // The page sees only folders that still open.
        std::fs::remove_dir(dir.join("p1")).unwrap();
        let st = sb.state(&file, &own, &[]);
        assert_eq!(st, FolderState { folder: Some(ok[1].clone()), error: None, recent: vec![ok[1].clone()] });
        // Clearing forgets the last folder but keeps the recent ones.
        sb.clear(&file);
        assert_eq!(sb.state(&file, &own, &[]).error.as_deref(), Some(PICK_FIRST));
        assert_eq!(load(&file), Saved { last: None, recent: vec![ok[1].clone(), ok[0].clone()] });
        assert_eq!(sb.open(&own, &[]).unwrap_err(), PICK_FIRST);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_folder_loads_and_none_means_pick_first() {
        let dir = crate::db::test_dir("agentfolder-load");
        let file = dir.join(FILE);
        let ok = dirs(&dir, &["proj"]);
        // Nothing saved, or an empty last folder: nothing current.
        for content in [None, Some(r#"{"last":"  ","recent":[]}"#)] {
            if let Some(c) = content {
                std::fs::write(&file, c).unwrap();
            }
            let mut sb = Sandbox::default();
            assert!(sb.load(&file, &[], &[]).is_none());
            assert_eq!(sb.state(&file, &[], &[]).folder, None);
            assert_eq!(sb.state(&file, &[], &[]).error.as_deref(), Some(PICK_FIRST));
        }
        Sandbox::default().set(&file, &ok[0], &[], &[]).unwrap();
        let mut sb = Sandbox::default();
        assert!(sb.load(&file, &[], &[]).is_none());
        assert_eq!(sb.state(&file, &[], &[]), FolderState { folder: Some(ok[0].clone()), error: None, recent: vec![ok[0].clone()] });
        assert_eq!(sb.open(&[], &[]).unwrap().display_root(), ok[0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A folder that was fine when saved but now leads into Democritus's own
    /// folder (a junction made afterwards) is refused on start and forgotten.
    #[cfg(windows)]
    #[test]
    fn saved_folder_is_checked_again_on_start() {
        let dir = crate::db::test_dir("agentfolder-recheck");
        let file = dir.join(FILE);
        let app = dir.join("democritus-app");
        std::fs::create_dir_all(app.join("src")).unwrap();
        let own = [app.clone()];
        let link = dirs(&dir, &["work"]).remove(0);
        let mut sb = Sandbox::default();
        sb.set(&file, &link, &own, &[]).unwrap();
        // Swap the folder for a junction into the app folder.
        std::fs::remove_dir(&link).unwrap();
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&app).output().unwrap();
        assert!(made.status.success(), "mklink /J failed: {made:?}");

        let mut sb = Sandbox::default();
        let refused = sb.load(&file, &own, &[]);
        let st = sb.state(&file, &own, &[]);
        let saved = load(&file);
        let second = sb.load(&file, &own, &[]);
        let opened = sb.open(&own, &[]).map(|w| w.display_root());
        let _ = std::fs::remove_dir(&link);
        let _ = std::fs::remove_dir_all(&dir);

        let refused = refused.expect("the saved folder must be refused");
        assert_eq!(refused.path, link);
        assert_eq!(refused.reason, ITSELF);
        assert_eq!(st.folder, None);
        assert_eq!(st.error, Some(format!("Your last folder ({link}) can't be used: {ITSELF}")));
        assert!(st.recent.is_empty(), "{st:?}");
        assert_eq!(saved.last, None, "the refused folder is forgotten");
        assert!(second.is_none(), "loaded only once");
        assert_eq!(opened.unwrap_err(), PICK_FIRST);
    }

    /// The current folder is checked again before each run.
    #[cfg(windows)]
    #[test]
    fn current_folder_is_checked_again_before_a_run() {
        let dir = crate::db::test_dir("agentfolder-reopen");
        let file = dir.join(FILE);
        let app = dir.join("democritus-app");
        std::fs::create_dir_all(&app).unwrap();
        let own = [app.clone()];
        let work = dirs(&dir, &["work"]).remove(0);
        let mut sb = Sandbox::default();
        sb.set(&file, &work, &own, &[]).unwrap();
        assert!(sb.open(&own, &[]).is_ok());
        std::fs::remove_dir(&work).unwrap();
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&work).arg(&app).output().unwrap();
        assert!(made.status.success(), "mklink /J failed: {made:?}");
        let opened = sb.open(&own, &[]).map(|w| w.display_root());
        let st = sb.state(&file, &own, &[]);
        let _ = std::fs::remove_dir(&work);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(opened.unwrap_err(), ITSELF);
        assert_eq!(st.folder, None);
        assert!(st.error.unwrap().ends_with(ITSELF));
    }
}
