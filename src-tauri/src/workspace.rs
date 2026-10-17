//! The coding agent's file access: one folder the operator picked for a task.
//! Every path the model names is resolved here and must stay inside that
//! folder (no absolute paths, no "..", no symlink escapes). Reads are free;
//! writes are only *planned* here and applied after the operator approves
//! the exact before/after text.

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;

/// Folders never walked into (build output, dependencies, version control).
const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", "dist", "build", ".venv", "venv", "__pycache__", ".next", ".e2e"];
pub const MAX_READ_CHARS: usize = 60_000;
const MAX_WRITE_BYTES: usize = 2_000_000;
const MAX_LIST: usize = 400;
const MAX_HITS: usize = 60;
const MAX_SEARCH_FILE: u64 = 1_000_000;

const ITSELF: &str = "The agent can't work on Democritus itself. Pick a project folder instead.";

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
    /// Paths refused since the last take_refusals, for the activity log.
    refused: Arc<Mutex<Vec<Refusal>>>,
}

/// A path the model asked for that was kept out of reach, and why.
#[derive(Debug, Clone, Serialize)]
pub struct Refusal {
    pub path: String,
    pub reason: String,
}

/// A change waiting for the operator: the exact text before and after.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedEdit {
    /// Relative to the workspace, with forward slashes.
    pub path: String,
    pub before: String,
    pub after: String,
    pub new_file: bool,
}

impl Workspace {
    /// Opens `folder` as a workspace. `own` are Democritus's own folders (its
    /// home, app data, and where it runs from); `protected` are other folders
    /// the agent may never work in or around (model stores). Both are checked
    /// even if they don't exist yet.
    pub fn open(folder: &str, own: &[PathBuf], protected: &[PathBuf]) -> Result<Self, String> {
        let folder = folder.trim().trim_matches('"');
        if folder.is_empty() {
            return Err("Pick a folder first.".into());
        }
        // A relative path would be resolved against wherever the app was
        // started from, which the operator can't see.
        if !Path::new(folder).is_absolute() {
            return Err("Use the folder's full path, for example D:\\projects\\my-app.".into());
        }
        let root = std::fs::canonicalize(folder).map_err(|e| format!("Can't open the folder \"{folder}\": {e}"))?;
        if !root.is_dir() {
            return Err(format!("\"{folder}\" is not a folder."));
        }
        if root.parent().is_none() {
            return Err("A whole drive can't be a project folder. Pick the project's own folder.".into());
        }
        if let Some(home) = home_dir() {
            if root == home {
                return Err("Your whole user folder can't be a project folder. Pick the project's own folder.".into());
            }
        }
        let overlaps = |p: &PathBuf| {
            let p = canon_lossy(p);
            root.starts_with(&p) || p.starts_with(&root)
        };
        if own.iter().any(overlaps) {
            return Err(ITSELF.into());
        }
        if protected.iter().any(overlaps) {
            return Err(format!("{} holds Democritus's own data or model files, so the coding agent can't work there.", display_path(&root)));
        }
        Ok(Self { root, refused: Arc::default() })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The root as the operator would type it (no \\?\ prefix).
    pub fn display_root(&self) -> String {
        display_path(&self.root)
    }

    /// The paths refused since the last call.
    pub fn take_refusals(&self) -> Vec<Refusal> {
        std::mem::take(&mut *self.refused.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// Records a refused path and returns the reason as the error.
    fn refuse(&self, path: &str, reason: String) -> String {
        self.refused.lock().unwrap_or_else(|p| p.into_inner()).push(Refusal { path: path.to_string(), reason: reason.clone() });
        reason
    }

    /// Resolves a model-supplied relative path to a real path inside the root.
    /// The target need not exist (for new files), but its nearest existing
    /// ancestor must resolve inside the root.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
        let rel = rel.trim().trim_matches('"');
        let rel = if rel.is_empty() || rel == "." || rel == "/" { "." } else { rel };
        let p = Path::new(rel);
        if p.is_absolute() || rel.starts_with('/') || rel.starts_with('\\') || p.components().any(|c| matches!(c, Component::Prefix(_) | Component::RootDir)) {
            return Err(self.refuse(rel, format!("\"{rel}\" is an absolute path. Use a path relative to the project folder.")));
        }
        if p.components().any(|c| c == Component::ParentDir) {
            return Err(self.refuse(rel, format!("\"{rel}\" leaves the project folder (\"..\" is not allowed).")));
        }
        let joined = self.root.join(p);
        // Walk up to the nearest existing ancestor and make sure symlinks
        // don't lead outside the root. A broken link counts as existing.
        let mut existing = joined.as_path();
        while std::fs::symlink_metadata(existing).is_err() {
            match existing.parent() {
                Some(parent) => existing = parent,
                None => return Err(self.refuse(rel, format!("\"{rel}\" is not inside the project folder."))),
            }
        }
        let real = match std::fs::canonicalize(existing) {
            Ok(real) => real,
            Err(_) if std::fs::symlink_metadata(existing).is_ok_and(|m| m.file_type().is_symlink()) => {
                return Err(self.refuse(rel, format!("\"{rel}\" is a broken link or points outside the project folder.")));
            }
            Err(e) => return Err(format!("Can't resolve \"{rel}\": {e}")),
        };
        if !real.starts_with(&self.root) {
            return Err(self.refuse(rel, format!("\"{rel}\" points outside the project folder.")));
        }
        let rest = joined.strip_prefix(existing).unwrap_or(Path::new(""));
        // join("") would add a trailing separator, turning a file into "file/".
        Ok(if rest.as_os_str().is_empty() { real } else { real.join(rest) })
    }

    /// Relative display form ("src/main.rs").
    pub fn rel(&self, path: &Path) -> String {
        let r = path.strip_prefix(&self.root).unwrap_or(path).to_string_lossy().replace('\\', "/");
        if r.is_empty() {
            ".".into()
        } else {
            r
        }
    }

    /// A tree listing, directories first, skipping build and dependency folders.
    pub fn list(&self, rel: &str, depth: usize) -> Result<String, String> {
        let dir = self.resolve(rel)?;
        if !dir.is_dir() {
            return Err(format!("\"{rel}\" is not a folder."));
        }
        let mut out = Vec::new();
        let mut truncated = false;
        self.walk_list(&dir, 0, depth.clamp(1, 4), &mut out, &mut truncated);
        let mut text = format!("{}/\n{}", self.rel(&dir), out.join("\n"));
        if out.is_empty() {
            text.push_str("(empty)");
        }
        if truncated {
            text.push_str(&format!("\n… (stopped after {MAX_LIST} entries; list a subfolder for more)"));
        }
        Ok(text)
    }

    fn walk_list(&self, dir: &Path, level: usize, depth: usize, out: &mut Vec<String>, truncated: &mut bool) {
        let mut entries: Vec<(bool, String, PathBuf)> = match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .flatten()
                .filter(|e| self.walkable(e))
                .map(|e| (e.path().is_dir(), e.file_name().to_string_lossy().to_string(), e.path()))
                .collect(),
            Err(_) => return,
        };
        entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
        for (is_dir, name, path) in entries {
            if out.len() >= MAX_LIST {
                *truncated = true;
                return;
            }
            let indent = "  ".repeat(level + 1);
            if is_dir {
                if SKIP_DIRS.contains(&name.as_str()) {
                    out.push(format!("{indent}{name}/ (skipped)"));
                    continue;
                }
                out.push(format!("{indent}{name}/"));
                if level + 1 < depth {
                    self.walk_list(&path, level + 1, depth, out, truncated);
                }
            } else {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                out.push(format!("{indent}{name} ({})", human_size(size)));
            }
        }
    }

    /// Reads a text file, optionally a 1-based inclusive line range.
    pub fn read(&self, rel: &str, start: Option<usize>, end: Option<usize>) -> Result<String, String> {
        let path = self.resolve(rel)?;
        let text = read_text(&path).map_err(|e| format!("Can't read \"{rel}\": {e}"))?;
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let total = lines.len();
        let from = start.unwrap_or(1).max(1);
        let to = end.unwrap_or(total).min(total);
        if from > to.max(1) && total > 0 {
            return Err(format!("\"{rel}\" has {total} lines; line {from} is past the end."));
        }
        let mut body: String = lines.get(from.saturating_sub(1)..to).map(|l| l.concat()).unwrap_or_default();
        let mut note = String::new();
        if body.chars().count() > MAX_READ_CHARS {
            body = body.chars().take(MAX_READ_CHARS).collect();
            note = format!("\n… (cut at {MAX_READ_CHARS} characters; read a line range to see more)");
        }
        let range = if start.is_some() || end.is_some() { format!(", lines {from}-{to}") } else { String::new() };
        Ok(format!("{} ({total} lines{range}):\n{body}{note}", self.rel(&path)))
    }

    /// Case-insensitive literal search across text files.
    pub fn search(&self, query: &str, rel: &str) -> Result<String, String> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Err("The search text is empty.".into());
        }
        let start = self.resolve(rel)?;
        let mut hits = Vec::new();
        let mut files = 0usize;
        self.walk_search(&start, &q, &mut hits, &mut files);
        if hits.is_empty() {
            return Ok(format!("No matches for \"{query}\" ({files} files searched)."));
        }
        let more = if hits.len() >= MAX_HITS { format!("\n… (stopped at {MAX_HITS} matches)") } else { String::new() };
        Ok(format!("{} matches for \"{query}\":\n{}{more}", hits.len(), hits.join("\n")))
    }

    fn walk_search(&self, path: &Path, q: &str, hits: &mut Vec<String>, files: &mut usize) {
        if hits.len() >= MAX_HITS {
            return;
        }
        if path.is_dir() {
            let Ok(rd) = std::fs::read_dir(path) else { return };
            let mut entries: Vec<PathBuf> = rd.flatten().filter(|e| self.walkable(e)).map(|e| e.path()).collect();
            entries.sort();
            for p in entries {
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if p.is_dir() && SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                self.walk_search(&p, q, hits, files);
            }
            return;
        }
        if std::fs::metadata(path).map(|m| m.len() > MAX_SEARCH_FILE).unwrap_or(true) {
            return;
        }
        let Ok(text) = read_text(path) else { return };
        *files += 1;
        for (i, line) in text.lines().enumerate() {
            if line.to_lowercase().contains(q) {
                let shown: String = line.trim().chars().take(200).collect();
                hits.push(format!("{}:{}: {shown}", self.rel(path), i + 1));
                if hits.len() >= MAX_HITS {
                    return;
                }
            }
        }
    }

    /// Whether list/search may walk into or read a folder entry: a symlink or
    /// junction must resolve inside the root. Refused entries are recorded.
    fn walkable(&self, entry: &std::fs::DirEntry) -> bool {
        if entry.file_type().is_ok_and(|t| !t.is_symlink()) {
            return true;
        }
        let path = entry.path();
        match std::fs::canonicalize(&path) {
            // A link back to a folder above it would be walked forever.
            Ok(real) if real.starts_with(&self.root) => !path.starts_with(&real),
            _ => {
                let rel = self.rel(&path);
                self.refuse(&rel, format!("\"{rel}\" points outside the project folder."));
                false
            }
        }
    }

    /// Plans replacing `old` (which must occur exactly once) with `new`.
    pub fn plan_edit(&self, rel: &str, old: &str, new: &str) -> Result<PlannedEdit, String> {
        let path = self.writable(rel)?;
        let before = read_text(&path).map_err(|e| format!("Can't read \"{rel}\": {e}"))?;
        if old.is_empty() {
            return Err("old_text is empty. Quote the exact text to replace, or use write_file for a new file.".into());
        }
        // Models often send \n where the file has \r\n: match either way.
        let (old, new) = if before.contains("\r\n") && !old.contains("\r\n") {
            (old.replace('\n', "\r\n"), new.replace('\n', "\r\n"))
        } else {
            (old.to_string(), new.to_string())
        };
        match before.matches(old.as_str()).count() {
            0 => Err(format!("old_text was not found in \"{rel}\". Read the file again and copy the exact text, including spaces.")),
            1 => {
                let after = before.replacen(old.as_str(), &new, 1);
                if after.len() > MAX_WRITE_BYTES {
                    return Err("The edited file would be too large.".into());
                }
                Ok(PlannedEdit { path: self.rel(&path), before, after, new_file: false })
            }
            n => Err(format!("old_text occurs {n} times in \"{rel}\". Include more surrounding lines so it is unique.")),
        }
    }

    /// Plans writing a whole file (new or replaced).
    pub fn plan_write(&self, rel: &str, content: &str) -> Result<PlannedEdit, String> {
        let path = self.writable(rel)?;
        if content.len() > MAX_WRITE_BYTES {
            return Err("The file is too large to write.".into());
        }
        if path.is_dir() {
            return Err(format!("\"{rel}\" is a folder."));
        }
        let exists = path.exists();
        let before = if exists { read_text(&path).map_err(|e| format!("Can't read \"{rel}\": {e}"))? } else { String::new() };
        Ok(PlannedEdit { path: self.rel(&path), before, after: content.to_string(), new_file: !exists })
    }

    fn writable(&self, rel: &str) -> Result<PathBuf, String> {
        let path = self.resolve(rel)?;
        let r = self.rel(&path);
        if r == "." {
            return Err(self.refuse(rel, "That's the project folder itself, not a file.".into()));
        }
        if r.split('/').any(|part| part == ".git") {
            return Err(self.refuse(rel, "Files inside .git can't be changed by the coding agent.".into()));
        }
        Ok(path)
    }

    /// Applies an approved edit. Refuses if the file changed since the plan,
    /// and copies the old version into `backup_dir` first.
    pub fn apply(&self, edit: &PlannedEdit, backup_dir: &Path) -> Result<Option<PathBuf>, String> {
        let path = self.writable(&edit.path)?;
        let current = if path.exists() { Some(read_text(&path).map_err(|e| format!("Can't read \"{}\": {e}", edit.path))?) } else { None };
        match (&current, edit.new_file) {
            (Some(_), true) => return Err(format!("\"{}\" was created by something else in the meantime; nothing was written.", edit.path)),
            (Some(c), false) if *c != edit.before => {
                return Err(format!("\"{}\" changed on disk since this edit was proposed; nothing was written.", edit.path))
            }
            (None, false) => return Err(format!("\"{}\" was deleted in the meantime; nothing was written.", edit.path)),
            _ => {}
        }
        let backup = match current {
            Some(c) => {
                let b = backup_dir.join(edit.path.replace('/', std::path::MAIN_SEPARATOR_STR));
                if let Some(parent) = b.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't create the backup folder: {e}"))?;
                }
                // Keep the first backup of a file per task (the true original).
                if !b.exists() {
                    std::fs::write(&b, c).map_err(|e| format!("Couldn't back up \"{}\": {e}", edit.path))?;
                }
                Some(b)
            }
            None => None,
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't create the folder for \"{}\": {e}", edit.path))?;
        }
        std::fs::write(&path, &edit.after).map_err(|e| format!("Couldn't write \"{}\": {e}", edit.path))?;
        Ok(backup)
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).and_then(|h| std::fs::canonicalize(h).ok())
}

/// The folders Democritus runs from: the program's folder and, in a dev
/// build, the source folder it was built from.
pub fn app_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.to_path_buf());
        }
        if let Some(src) = exe.ancestors().skip(1).find(|a| a.join("src-tauri").join("tauri.conf.json").is_file()) {
            out.push(src.to_path_buf());
        }
    }
    #[cfg(debug_assertions)]
    if let Some(src) = Path::new(env!("CARGO_MANIFEST_DIR")).parent() {
        out.push(src.to_path_buf());
    }
    out.dedup();
    out
}

/// Canonical form of `p` even if it doesn't exist yet: the nearest existing
/// ancestor is canonicalized and the rest appended.
fn canon_lossy(p: &Path) -> PathBuf {
    let mut existing = p;
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(existing) {
            return rest.iter().rev().fold(real, |acc, name| acc.join(name));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                existing = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

/// A path as the operator would type it: no Windows \\?\ prefix.
pub fn display_path(p: &Path) -> String {
    let s = p.display().to_string();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s
    }
}

/// Reads a file as UTF-8 text; binary files are refused.
fn read_text(path: &Path) -> Result<String, String> {
    if path.is_dir() {
        return Err("it is a folder".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.iter().take(8000).any(|&b| b == 0) {
        return Err("it looks like a binary file".into());
    }
    String::from_utf8(bytes).map_err(|_| "it is not UTF-8 text".to_string())
}

fn human_size(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1} MB", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{} KB", n / 1000)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(name: &str) -> (PathBuf, Workspace) {
        let dir = crate::db::test_dir(name);
        let proj = dir.join("proj");
        std::fs::create_dir_all(proj.join("src")).unwrap();
        std::fs::create_dir_all(proj.join("node_modules/x")).unwrap();
        std::fs::create_dir_all(proj.join(".git")).unwrap();
        std::fs::write(proj.join("src/main.rs"), "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
        std::fs::write(proj.join("README.md"), "# Proj\nHello world\n").unwrap();
        std::fs::write(proj.join("node_modules/x/index.js"), "hello").unwrap();
        std::fs::write(proj.join("logo.png"), [0u8, 1, 2, 0, 5]).unwrap();
        std::fs::write(dir.join("secret.txt"), "outside").unwrap();
        let ws = Workspace::open(proj.to_str().unwrap(), &[], &[]).unwrap();
        (dir, ws)
    }

    #[test]
    fn paths_stay_inside_the_folder() {
        let (dir, ws) = setup("ws-paths");
        assert!(ws.resolve("src/main.rs").is_ok());
        assert!(ws.resolve("src/new/file.rs").is_ok());
        assert!(ws.resolve("../secret.txt").unwrap_err().contains(".."));
        assert!(ws.resolve("src/../../secret.txt").is_err());
        assert!(ws.resolve("/etc/passwd").unwrap_err().contains("absolute"));
        assert!(ws.resolve(dir.join("secret.txt").to_str().unwrap()).is_err());
        assert_eq!(ws.rel(&ws.resolve(".").unwrap()), ".");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("secret.txt"), ws.root().join("link.txt")).unwrap();
            assert!(ws.resolve("link.txt").unwrap_err().contains("outside"));
            assert!(ws.read("link.txt", None, None).is_err());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_protected_and_too_broad_folders() {
        let dir = crate::db::test_dir("ws-protected");
        let home = dir.join("Democritus");
        std::fs::create_dir_all(home.join("memory")).unwrap();
        assert!(Workspace::open(dir.to_str().unwrap(), &[], &[home.clone()]).unwrap_err().contains("model files"), "contains a protected folder");
        assert!(Workspace::open(home.join("memory").to_str().unwrap(), &[], &[home.clone()]).is_err(), "inside a protected folder");
        assert!(Workspace::open("/", &[], &[]).is_err());
        assert!(Workspace::open("", &[], &[]).is_err());
        assert!(Workspace::open(dir.join("missing").to_str().unwrap(), &[], &[]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lists_reads_and_searches() {
        let (dir, ws) = setup("ws-read");
        let list = ws.list(".", 2).unwrap();
        assert!(list.contains("src/") && list.contains("main.rs") && list.contains("node_modules/ (skipped)"), "{list}");
        assert!(!list.contains("index.js"));
        let r = ws.read("src/main.rs", None, None).unwrap();
        assert!(r.starts_with("src/main.rs (3 lines)") && r.contains("println!"), "{r}");
        let part = ws.read("src/main.rs", Some(2), Some(2)).unwrap();
        assert!(part.contains("lines 2-2") && part.contains("println") && !part.contains("fn main"), "{part}");
        assert!(ws.read("logo.png", None, None).unwrap_err().contains("binary"));
        let s = ws.search("HELLO", ".").unwrap();
        assert!(s.contains("README.md:2: Hello world") && !s.contains("node_modules"), "{s}");
        assert!(ws.search("nothing-like-this", ".").unwrap().starts_with("No matches"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edits_are_planned_then_applied_with_backup() {
        let (dir, ws) = setup("ws-edit");
        let backups = dir.join("backups");
        assert!(ws.plan_edit("src/main.rs", "nope", "x").unwrap_err().contains("not found"));
        assert!(ws.plan_edit("README.md", "l", "L").unwrap_err().contains("times"));
        let e = ws.plan_edit("src/main.rs", "\"hi\"", "\"hello\"").unwrap();
        assert_eq!(e.path, "src/main.rs");
        assert!(!e.new_file && e.after.contains("\"hello\""));
        // Planning writes nothing.
        assert!(std::fs::read_to_string(ws.root().join("src/main.rs")).unwrap().contains("\"hi\""));
        let b = ws.apply(&e, &backups).unwrap().unwrap();
        assert!(std::fs::read_to_string(ws.root().join("src/main.rs")).unwrap().contains("\"hello\""));
        assert!(std::fs::read_to_string(b).unwrap().contains("\"hi\""));
        // A stale plan is refused.
        assert!(ws.apply(&e, &backups).unwrap_err().contains("changed on disk"));

        let w = ws.plan_write("docs/notes.md", "new\n").unwrap();
        assert!(w.new_file && w.before.is_empty());
        assert_eq!(ws.apply(&w, &backups).unwrap(), None);
        assert_eq!(std::fs::read_to_string(ws.root().join("docs/notes.md")).unwrap(), "new\n");
        assert!(ws.plan_write(".git/config", "x").unwrap_err().contains(".git"));
        assert!(ws.plan_write(".", "x").is_err());
        assert!(ws.plan_write("../evil.txt", "x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Windows path forms: drive-relative, UNC and verbatim prefixes, backslash
    /// "..", junctions (no admin needed, unlike symlinks) and .git in any case.
    #[cfg(windows)]
    #[test]
    fn windows_paths_stay_inside_the_folder() {
        let (dir, ws) = setup("ws-win");
        let outside = dir.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "outside").unwrap();
        for p in [
            "C:secret.txt",
            "C:\\Windows\\win.ini",
            "D:/secret.txt",
            "\\\\localhost\\c$\\Windows\\win.ini",
            "\\\\?\\C:\\Windows\\win.ini",
            "//?/C:/Windows/win.ini",
            "\\\\.\\PhysicalDrive0",
            "src\\..\\..\\secret.txt",
            "..\\secret.txt",
            "src/..\\../secret.txt",
        ] {
            assert!(ws.resolve(p).is_err(), "{p} should be refused");
            assert!(ws.plan_write(p, "x").is_err(), "{p} should not be writable");
        }
        // Names Win32 would normalize: whatever they resolve to must stay inside.
        for p in [".. \\secret.txt", "..\u{2024}\\secret.txt", "...\\secret.txt", "src.\\main.rs", "src \\main.rs"] {
            if let Ok(r) = ws.resolve(p) {
                assert!(r.starts_with(ws.root()), "{p} resolved to {}", r.display());
            }
        }
        // A junction inside the project that points outside it.
        let junction = ws.root().join("jx");
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(&outside).output().unwrap();
        assert!(made.status.success(), "mklink /J failed: {made:?}");
        assert!(ws.resolve("jx/secret.txt").unwrap_err().contains("outside"));
        assert!(ws.read("jx/secret.txt", None, None).is_err());
        assert!(ws.plan_write("jx/new.txt", "x").is_err());
        assert!(ws.plan_edit("jx/secret.txt", "outside", "inside").is_err());
        assert!(!outside.join("new.txt").exists());
        // .git is protected whatever the case.
        std::fs::write(ws.root().join(".git/config"), "[core]\n").unwrap();
        assert!(ws.plan_write(".GIT/config", "x").unwrap_err().contains(".git"));
        assert!(ws.plan_write(".Git\\hooks\\pre-commit", "x").unwrap_err().contains(".git"));
        let _ = std::fs::remove_dir(&junction);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// list_files and search_files must not walk into a junction or symlink
    /// inside the project that leads outside it (review 2026-10-17).
    #[cfg(windows)]
    #[test]
    fn list_and_search_do_not_follow_junctions_out() {
        let (dir, ws) = setup("ws-walk");
        let outside = dir.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "topsecret
").unwrap();
        let junction = ws.root().join("jx");
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(&outside).output().unwrap();
        assert!(made.status.success(), "mklink /J failed: {made:?}");
        let found = ws.search("topsecret", ".").unwrap();
        let listed = ws.list(".", 3).unwrap();
        let _ = std::fs::remove_dir(&junction);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!found.contains("jx/"), "{found}");
        assert!(!listed.contains("secret.txt"), "{listed}");
    }

    /// Known gap (review 2026-10-17): an approved edit to a hard link inside
    /// the project changes the linked file outside it.
    #[cfg(windows)]
    #[test]
    #[ignore = "known gap: edits through hard links reach files outside the project"]
    fn edits_refuse_hard_links_to_outside_files() {
        let (dir, ws) = setup("ws-hardlink");
        std::fs::write(dir.join("victim.txt"), "original
").unwrap();
        std::fs::hard_link(dir.join("victim.txt"), ws.root().join("hl.txt")).unwrap();
        let r = ws.plan_edit("hl.txt", "original", "changed").and_then(|e| ws.apply(&e, &dir.join("b")));
        let victim = std::fs::read_to_string(dir.join("victim.txt")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(r.is_err() && victim == "original
", "{r:?} / {victim:?}");
    }

    /// Known gap (review 2026-10-17): reserved device names, trailing dots and
    /// alternate data streams are written literally (hard to delete, or hidden).
    #[cfg(windows)]
    #[test]
    #[ignore = "known gap: Windows reserved names and streams are accepted"]
    fn refuses_reserved_windows_names() {
        let (dir, ws) = setup("ws-names");
        for p in ["nul", "con.txt", "aux", "COM1.log", "name.", "README.md:hidden"] {
            assert!(ws.plan_write(p, "x").is_err(), "{p} should be refused");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crlf_files_match_lf_edits() {
        let (dir, ws) = setup("ws-crlf");
        std::fs::write(ws.root().join("win.txt"), "a\r\nb\r\nc\r\n").unwrap();
        let e = ws.plan_edit("win.txt", "a\nb", "a\nB").unwrap();
        assert_eq!(e.after, "a\r\nB\r\nc\r\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    fn junction(link: &Path, target: &Path) {
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(link).arg(target).output().unwrap();
        assert!(made.status.success(), "mklink /J failed: {made:?}");
    }

    #[test]
    fn chosen_folder_is_respected() {
        let (dir, ws) = setup("ws-chosen");
        assert_eq!(ws.root(), std::fs::canonicalize(dir.join("proj")).unwrap());
        assert!(ws.read("README.md", None, None).unwrap().contains("Hello world"));
        assert!(ws.list(".", 2).unwrap().contains("main.rs"));
        assert!(ws.search("hello", ".").unwrap().contains("README.md:2"));
        assert!(ws.take_refusals().is_empty());
        // "Not found" is an ordinary error, not a refusal.
        assert!(ws.read("missing.txt", None, None).is_err());
        assert!(ws.take_refusals().is_empty());
        // A file in another folder, by its full path.
        let secret = dir.join("secret.txt");
        let secret = secret.to_str().unwrap();
        assert!(ws.read(secret, None, None).is_err());
        let refused = ws.take_refusals();
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert_eq!(refused[0].path, secret);
        assert!(refused[0].reason.contains("absolute path"), "{refused:?}");
        assert!(ws.take_refusals().is_empty(), "refusals are taken once");
        // Relative folders are never resolved against the working directory.
        let mut relative = vec![".", "proj", "..", "src-tauri"];
        if cfg!(windows) {
            relative.extend(["D:", "C:proj", "\\proj"]);
        }
        for f in relative {
            let e = Workspace::open(f, &[], &[]).unwrap_err();
            assert_eq!(e, "Use the folder's full path, for example D:\\projects\\my-app.", "{f}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paths_in_the_app_folder_are_refused() {
        let dir = crate::db::test_dir("ws-own");
        let app = dir.join("democritus-app");
        std::fs::create_dir_all(app.join("src")).unwrap();
        std::fs::write(app.join("src/main.ts"), "app code\n").unwrap();
        std::fs::create_dir_all(dir.join("democritus-app2")).unwrap();
        std::fs::create_dir_all(dir.join("proj")).unwrap();
        let own = [app.clone()];
        for f in [app.clone(), app.join("src"), dir.clone()] {
            assert_eq!(Workspace::open(f.to_str().unwrap(), &own, &[]).unwrap_err(), ITSELF, "{}", f.display());
        }
        // A neighbour whose name merely starts the same is fine.
        assert!(Workspace::open(dir.join("democritus-app2").to_str().unwrap(), &own, &[]).is_ok());
        let ws = Workspace::open(dir.join("proj").to_str().unwrap(), &own, &[]).unwrap();
        let target = app.join("src").join("main.ts");
        assert!(ws.read(target.to_str().unwrap(), None, None).is_err());
        assert!(ws.plan_write(target.to_str().unwrap(), "x").is_err());
        assert_eq!(ws.take_refusals().len(), 2);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "app code\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_real_app_folder_is_refused() {
        let own = app_dirs();
        assert!(!own.is_empty());
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let source = manifest.parent().unwrap();
        for f in [source.to_path_buf(), source.join("src"), manifest.to_path_buf()] {
            assert_eq!(Workspace::open(f.to_str().unwrap(), &own, &[]).unwrap_err(), ITSELF, "{}", f.display());
        }
    }

    #[test]
    fn democritus_home_cannot_be_picked() {
        let dir = crate::db::test_dir("ws-home");
        let home = dir.join("Democritus");
        std::fs::create_dir_all(home.join("memory")).unwrap();
        let own = [home.clone()];
        for f in [home.clone(), home.join("memory"), dir.clone()] {
            assert_eq!(Workspace::open(f.to_str().unwrap(), &own, &[]).unwrap_err(), ITSELF, "{}", f.display());
        }
        // A home that doesn't exist yet still protects where it will be.
        let later = dir.join("later");
        std::fs::create_dir_all(&later).unwrap();
        assert_eq!(Workspace::open(later.to_str().unwrap(), &[later.join("Democritus").join("data")], &[]).unwrap_err(), ITSELF);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_folder_means_agent_off() {
        for f in ["", "   ", "\"\"", " \"\" "] {
            assert_eq!(Workspace::open(f, &[], &[]).unwrap_err(), "Pick a folder first.", "{f:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn junction_into_the_app_folder_is_refused() {
        let dir = crate::db::test_dir("ws-own-link");
        let app = dir.join("democritus-app");
        std::fs::create_dir_all(app.join("src")).unwrap();
        std::fs::write(app.join("src/main.ts"), "appsecret\n").unwrap();
        let proj = dir.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("README.md"), "hello\n").unwrap();
        let own = [app.clone()];
        // A junction outside the project that leads to the app folder.
        let outer = dir.join("shortcut");
        junction(&outer, &app);
        assert_eq!(Workspace::open(outer.to_str().unwrap(), &own, &[]).unwrap_err(), ITSELF);
        // A junction inside the project that leads to the app folder.
        let inner = proj.join("link");
        junction(&inner, &app);
        let ws = Workspace::open(proj.to_str().unwrap(), &own, &[]).unwrap();
        let read = ws.read("link/src/main.ts", None, None);
        let write = ws.plan_write("link/src/main.ts", "x");
        let listed = ws.list(".", 4).unwrap();
        let found = ws.search("appsecret", ".").unwrap();
        let refused = ws.take_refusals();
        let _ = std::fs::remove_dir(&outer);
        let _ = std::fs::remove_dir(&inner);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(read.unwrap_err().contains("outside"));
        assert!(write.is_err());
        assert!(listed.contains("README.md") && !listed.contains("main.ts") && !listed.contains("link"), "{listed}");
        assert!(found.starts_with("No matches"), "{found}");
        let paths: Vec<&str> = refused.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, ["link/src/main.ts", "link/src/main.ts", "link", "link"], "{refused:?}");
        assert!(refused.iter().all(|r| r.reason.contains("outside")), "{refused:?}");
    }

    #[cfg(windows)]
    #[test]
    fn broken_links_are_refused() {
        let (dir, ws) = setup("ws-dangling");
        let gone = dir.join("gone");
        std::fs::create_dir_all(&gone).unwrap();
        let link = dir.join("proj").join("dead");
        junction(&link, &gone);
        std::fs::remove_dir(&gone).unwrap();
        let into = ws.plan_write("dead/new.txt", "x");
        let onto = ws.plan_write("dead", "x");
        let listed = ws.list(".", 2).unwrap();
        let refused = ws.take_refusals();
        let _ = std::fs::remove_dir(&link);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(into.unwrap_err().contains("broken link"));
        assert!(onto.unwrap_err().contains("broken link"));
        assert!(!listed.contains("dead"), "{listed}");
        assert!(!gone.exists(), "nothing was created through the link");
        assert_eq!(refused.len(), 3, "{refused:?}");
    }
}
