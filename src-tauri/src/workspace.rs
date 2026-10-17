//! The coding agent's file access: one folder the operator picked for a task.
//! Every path the model names is resolved here and must stay inside that
//! folder (no absolute paths, no "..", no symlink escapes). Reads are free;
//! writes are only *planned* here and applied after the operator approves
//! the exact before/after text.

use std::path::{Component, Path, PathBuf};

use serde::Serialize;

/// Folders never walked into (build output, dependencies, version control).
const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", "dist", "build", ".venv", "venv", "__pycache__", ".next", ".e2e"];
pub const MAX_READ_CHARS: usize = 60_000;
const MAX_WRITE_BYTES: usize = 2_000_000;
const MAX_LIST: usize = 400;
const MAX_HITS: usize = 60;
const MAX_SEARCH_FILE: u64 = 1_000_000;

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
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
    /// Opens `folder` as a workspace. `protected` are folders the agent may
    /// never work in or around (the Democritus home, app data, model stores).
    pub fn open(folder: &str, protected: &[PathBuf]) -> Result<Self, String> {
        let folder = folder.trim().trim_matches('"');
        if folder.is_empty() {
            return Err("Pick a project folder first.".into());
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
        for p in protected {
            let Ok(p) = std::fs::canonicalize(p) else { continue };
            if root.starts_with(&p) || p.starts_with(&root) {
                return Err(format!("{} holds Democritus's own data or model files, so the coding agent can't work there.", root.display()));
            }
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves a model-supplied relative path to a real path inside the root.
    /// The target need not exist (for new files), but its nearest existing
    /// ancestor must resolve inside the root.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
        let rel = rel.trim().trim_matches('"');
        let rel = if rel.is_empty() || rel == "." || rel == "/" { "." } else { rel };
        let p = Path::new(rel);
        if p.is_absolute() || rel.starts_with('/') || rel.starts_with('\\') || p.components().any(|c| matches!(c, Component::Prefix(_) | Component::RootDir)) {
            return Err(format!("\"{rel}\" is an absolute path. Use a path relative to the project folder."));
        }
        if p.components().any(|c| c == Component::ParentDir) {
            return Err(format!("\"{rel}\" leaves the project folder (\"..\" is not allowed)."));
        }
        let joined = self.root.join(p);
        // Walk up to the nearest existing ancestor and make sure symlinks
        // don't lead outside the root.
        let mut existing = joined.as_path();
        while !existing.exists() {
            existing = existing.parent().ok_or_else(|| format!("\"{rel}\" is not inside the project folder."))?;
        }
        let real = std::fs::canonicalize(existing).map_err(|e| format!("Can't resolve \"{rel}\": {e}"))?;
        if !real.starts_with(&self.root) {
            return Err(format!("\"{rel}\" points outside the project folder."));
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
            Ok(rd) => rd.flatten().map(|e| (e.path().is_dir(), e.file_name().to_string_lossy().to_string(), e.path())).collect(),
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
            let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
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
            return Err("That's the project folder itself, not a file.".into());
        }
        if r.split('/').any(|part| part == ".git") {
            return Err("Files inside .git can't be changed by the coding agent.".into());
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
        let ws = Workspace::open(proj.to_str().unwrap(), &[]).unwrap();
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
        assert!(Workspace::open(dir.to_str().unwrap(), &[home.clone()]).is_err(), "contains the home folder");
        assert!(Workspace::open(home.join("memory").to_str().unwrap(), &[home.clone()]).is_err(), "inside the home folder");
        assert!(Workspace::open("/", &[]).is_err());
        assert!(Workspace::open("", &[]).is_err());
        assert!(Workspace::open(dir.join("missing").to_str().unwrap(), &[]).is_err());
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

    #[test]
    fn crlf_files_match_lf_edits() {
        let (dir, ws) = setup("ws-crlf");
        std::fs::write(ws.root().join("win.txt"), "a\r\nb\r\nc\r\n").unwrap();
        let e = ws.plan_edit("win.txt", "a\nb", "a\nB").unwrap();
        assert_eq!(e.after, "a\r\nB\r\nc\r\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
