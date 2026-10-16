//! Democritus's memory: plain Markdown files in <home>\memory\. The files are
//! the source of truth (the operator may edit them in any editor); everything
//! else (search index, proposals) is derived. Every write goes through here:
//! paths are validated to stay inside memory\, and the previous version of a
//! file is snapshotted to memory\.history\ first so any change can be undone.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::localtime;

pub const HISTORY_DIR: &str = ".history";
/// Files the operator owns. The model can never propose changes to them.
pub const OPERATOR_ONLY: &[&str] = &["identity.md", "index.md"];
/// Files that always exist and can't be deleted.
pub const CORE_FILES: &[&str] = &["identity.md", "operator.md", "current_state.md", "index.md"];
const MAX_OP_CHARS: usize = 8_000;
const MAX_FILE_BYTES: usize = 256 * 1024;

fn template(name: &str) -> &'static str {
    match name {
        "identity.md" => {
            "# Identity\n\nWho Democritus is and who the operator is. Only the operator edits this file.\n\n\
## Democritus\nDemocritus is a private AI assistant that runs entirely on the operator's computer. \
Its brain is a local Qwen model that the operator can switch at any time; its identity, memory and project knowledge live in this folder.\n\n\
## Operator\n(Who you are, in your own words.)\n"
        }
        "operator.md" => "# Operator\n\nPreferences and facts about the operator.\n\n## Preferences\n\n## Facts\n",
        "current_state.md" => {
            "# Current state\n\nWhat we're doing right now.\n\n## Active project\nNone yet.\n\n## Last task\n\n## Open questions\n\n## Next step\n"
        }
        _ => "",
    }
}

pub fn project_template(title: &str) -> String {
    format!(
        "# {title}\n\n## Goal\n\n## Status\n\n## Decisions\n\n## Files involved\n\n## Open issues\n\n## Next steps\n\n## Change log\n"
    )
}

/// Creates the folder layout and any missing core files. Never overwrites.
pub fn ensure_structure(mem: &Path) -> std::io::Result<()> {
    for d in ["", "projects", "sessions", HISTORY_DIR] {
        std::fs::create_dir_all(mem.join(d))?;
    }
    for name in ["identity.md", "operator.md", "current_state.md"] {
        let p = mem.join(name);
        if !p.exists() {
            std::fs::write(p, template(name))?;
        }
    }
    if !mem.join("index.md").exists() {
        regenerate_index(mem)?;
    }
    Ok(())
}

/// A project name as a safe file stem: "My Cool App!" -> "my-cool-app".
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(60).collect()
}

/// Validates a path relative to memory\ and returns it normalized with "/".
/// Rejects absolute paths, drive letters, "..", hidden/history paths, odd
/// characters and anything that isn't a .md file.
pub fn validate_rel(raw: &str) -> Result<String, String> {
    let s = raw.trim().replace('\\', "/");
    let s = s.trim_start_matches("./");
    let s = s.strip_prefix("memory/").unwrap_or(s);
    if s.is_empty() {
        return Err("empty path".into());
    }
    if s.starts_with('/') || s.contains(':') || s.starts_with("//") {
        return Err(format!("\"{raw}\" is an absolute path; only files inside memory\\ are allowed"));
    }
    let path = Path::new(s);
    let mut parts = Vec::new();
    for c in path.components() {
        match c {
            Component::Normal(p) => {
                let p = p.to_str().ok_or("path is not valid text")?;
                if p.starts_with('.') {
                    return Err(format!("\"{raw}\" points at a hidden or history file"));
                }
                if !p.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ' ')) {
                    return Err(format!("\"{raw}\" contains characters that aren't allowed"));
                }
                parts.push(p.to_string());
            }
            _ => return Err(format!("\"{raw}\" leaves the memory folder")),
        }
    }
    if parts.len() > 2 || (parts.len() == 2 && !matches!(parts[0].as_str(), "projects" | "sessions")) {
        return Err(format!("\"{raw}\" is not in memory\\, memory\\projects\\ or memory\\sessions\\"));
    }
    let joined = parts.join("/");
    if !joined.to_lowercase().ends_with(".md") {
        return Err(format!("\"{raw}\" is not a Markdown (.md) file"));
    }
    Ok(joined)
}

/// Whether the model may propose changes to this (validated) path.
pub fn model_may_write(rel: &str) -> bool {
    !OPERATOR_ONLY.contains(&rel)
}

pub fn abs(mem: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(mem.to_path_buf(), |p, part| p.join(part))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub path: String,
    pub title: String,
    pub description: String,
    pub size: u64,
    pub modified_ms: i64,
    pub core: bool,
}

fn modified_ms(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Title = first heading; description = first plain line after it.
fn describe(text: &str, fallback: &str) -> (String, String) {
    let mut title = String::new();
    let mut desc = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if title.is_empty() && t.starts_with('#') {
            title = t.trim_start_matches('#').trim().to_string();
            continue;
        }
        if !t.starts_with('#') {
            desc = t.chars().take(140).collect();
            break;
        }
    }
    if title.is_empty() {
        title = fallback.to_string();
    }
    (title, desc)
}

/// All memory files (not history), core files first.
pub fn list(mem: &Path) -> Vec<FileInfo> {
    let mut out = Vec::new();
    let mut push = |rel: String| {
        let p = abs(mem, &rel);
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        let (title, description) = describe(&text, &rel);
        out.push(FileInfo {
            core: CORE_FILES.contains(&rel.as_str()),
            size: text.len() as u64,
            modified_ms: modified_ms(&p),
            path: rel,
            title,
            description,
        });
    };
    for name in CORE_FILES {
        if mem.join(name).exists() {
            push(name.to_string());
        }
    }
    let mut extra: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(mem) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if e.path().is_file() && n.to_lowercase().ends_with(".md") && !CORE_FILES.contains(&n.as_str()) && !n.starts_with('.') {
                extra.push(n);
            }
        }
    }
    for dir in ["projects", "sessions"] {
        if let Ok(rd) = std::fs::read_dir(mem.join(dir)) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if e.path().is_file() && n.to_lowercase().ends_with(".md") && !n.starts_with('.') {
                    extra.push(format!("{dir}/{n}"));
                }
            }
        }
    }
    // Projects alphabetically, sessions newest first.
    extra.sort_by(|a, b| match (a.starts_with("sessions/"), b.starts_with("sessions/")) {
        (true, true) => b.cmp(a),
        _ => a.cmp(b),
    });
    for rel in extra {
        push(rel);
    }
    out
}

pub fn read(mem: &Path, rel: &str) -> Result<String, String> {
    let rel = validate_rel(rel)?;
    std::fs::read_to_string(abs(mem, &rel)).map_err(|e| format!("Couldn't read {rel}: {e}"))
}

// ---------- sections ----------

fn heading_level(line: &str) -> Option<usize> {
    let t = line.trim_start();
    let n = t.chars().take_while(|c| *c == '#').count();
    (n > 0 && n <= 6 && t[n..].starts_with(' ')).then_some(n)
}

fn norm_heading(s: &str) -> String {
    s.trim().trim_start_matches('#').trim().to_lowercase()
}

/// (heading line index, end-exclusive line index, level) of a section.
fn find_section(lines: &[String], heading: &str) -> Option<(usize, usize, usize)> {
    let want = norm_heading(heading);
    let start = lines.iter().position(|l| heading_level(l).is_some() && norm_heading(l) == want)?;
    let level = heading_level(&lines[start]).unwrap_or(2);
    let end = lines[start + 1..]
        .iter()
        .position(|l| heading_level(l).is_some_and(|lv| lv <= level))
        .map(|i| start + 1 + i)
        .unwrap_or(lines.len());
    Some((start, end, level))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Op {
    pub file: String,
    #[serde(default)]
    pub section: Option<String>,
    /// append | replace_section | create_file | remove_section
    pub action: String,
    #[serde(default)]
    pub content: String,
}

/// Applies one operation to a file's text and returns the new text.
pub fn apply_op(text: &str, op: &Op) -> Result<String, String> {
    if op.content.chars().count() > MAX_OP_CHARS {
        return Err(format!("the change to {} is too long ({} characters)", op.file, op.content.chars().count()));
    }
    let content = op.content.trim_end().to_string();
    let section = op.section.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let add_section = |lines: &mut Vec<String>, title: &str, body: &str| {
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(format!("## {}", title.trim_start_matches('#').trim()));
        lines.extend(body.lines().map(String::from));
    };
    match op.action.as_str() {
        "create_file" => {
            if text.trim().is_empty() {
                return Ok(format!("{content}\n"));
            }
            // Already exists: keep the operator's version and append instead.
            return apply_op(text, &Op { action: "append".into(), ..op.clone() });
        }
        "append" => match section {
            Some(h) => match find_section(&lines, h) {
                Some((_, end, _)) => {
                    let mut insert_at = end;
                    while insert_at > 0 && lines[insert_at - 1].trim().is_empty() {
                        insert_at -= 1;
                    }
                    let new: Vec<String> = content.lines().map(String::from).collect();
                    let tail_blank = end < lines.len();
                    lines.splice(insert_at..end, new.into_iter().chain(tail_blank.then(String::new)));
                }
                None => add_section(&mut lines, h, &content),
            },
            None => {
                while lines.last().is_some_and(|l| l.trim().is_empty()) {
                    lines.pop();
                }
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                lines.extend(content.lines().map(String::from));
            }
        },
        "replace_section" => {
            let h = section.ok_or("replace_section needs a section")?;
            match find_section(&lines, h) {
                Some((start, end, _)) => {
                    let mut new: Vec<String> = content.lines().map(String::from).collect();
                    if end < lines.len() {
                        new.push(String::new());
                    }
                    lines.splice(start + 1..end, new);
                }
                None => add_section(&mut lines, h, &content),
            }
        }
        "remove_section" => {
            let h = section.ok_or("remove_section needs a section")?;
            if let Some((start, end, _)) = find_section(&lines, h) {
                lines.drain(start..end);
            }
        }
        other => return Err(format!("unknown action \"{other}\"")),
    }
    let mut out = lines.join("\n");
    out.push('\n');
    if out.len() > MAX_FILE_BYTES {
        return Err(format!("{} would grow past {} KB", op.file, MAX_FILE_BYTES / 1024));
    }
    Ok(out)
}

/// Small models often put a whole file (headings and all) into one section's
/// content. This splits such content into one operation per section, drops the
/// file's own `#` title and description, and keeps only the last copy when a
/// section is repeated. Content without headings passes through unchanged.
pub fn split_op(op: Op) -> Vec<Op> {
    if op.action == "create_file" || !op.content.lines().any(|l| heading_level(l).is_some()) {
        return vec![op];
    }
    // (heading or None for text before the first heading, level, body)
    let mut parts: Vec<(Option<String>, usize, String)> = vec![(None, 0, String::new())];
    for line in op.content.lines() {
        if let Some(level) = heading_level(line) {
            parts.push((Some(line.trim().trim_start_matches('#').trim().to_string()), level, String::new()));
        } else {
            let last = parts.last_mut().expect("non-empty");
            last.2.push_str(line);
            last.2.push('\n');
        }
    }
    let mut out: Vec<Op> = Vec::new();
    for (heading, level, body) in parts {
        let body = body.trim().to_string();
        if level == 1 || body.is_empty() {
            continue; // the file title and its description, or an empty section
        }
        let (section, action) = match heading {
            None => (op.section.clone(), op.action.clone()),
            Some(h) if op.section.as_deref().is_some_and(|s| norm_heading(s) == norm_heading(&h)) => (op.section.clone(), op.action.clone()),
            // Other sections in a dump hold that section's full current text.
            Some(h) => (Some(h), if op.action == "append" { "append".into() } else { "replace_section".into() }),
        };
        let Some(section) = section else { continue };
        let key = norm_heading(&section);
        out.retain(|o| o.section.as_deref().map(norm_heading).as_deref() != Some(key.as_str()));
        out.push(Op { file: op.file.clone(), section: Some(section), action, content: body });
    }
    out
}

// ---------- history, writes, backups ----------

fn history_dir_for(mem: &Path, rel: &str) -> PathBuf {
    mem.join(HISTORY_DIR).join(rel.replace('/', "__"))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: String,
    pub time: String,
    pub size: u64,
}

/// Copies the current version of a file into the history before it changes.
pub fn snapshot(mem: &Path, rel: &str) -> std::io::Result<()> {
    let p = abs(mem, rel);
    if !p.exists() {
        return Ok(());
    }
    let dir = history_dir_for(mem, rel);
    std::fs::create_dir_all(&dir)?;
    // stamp-NNN keeps several snapshots in the same second in order.
    let stamp = localtime::now().stamp();
    let mut n = 0;
    let mut target = dir.join(format!("{stamp}-{n:03}.md"));
    while target.exists() {
        n += 1;
        target = dir.join(format!("{stamp}-{n:03}.md"));
    }
    std::fs::copy(p, target).map(|_| ())
}

pub fn history(mem: &Path, rel: &str) -> Vec<Snapshot> {
    let mut out: Vec<Snapshot> = std::fs::read_dir(history_dir_for(mem, rel))
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let id = e.file_name().to_string_lossy().to_string();
                    let stem = id.strip_suffix(".md")?;
                    let d = stem.get(0..15)?;
                    let time = format!("{}-{}-{} {}:{}:{}", &d[0..4], &d[4..6], &d[6..8], &d[9..11], &d[11..13], &d[13..15]);
                    Some(Snapshot { size: e.metadata().map(|m| m.len()).unwrap_or(0), id, time })
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

pub fn read_snapshot(mem: &Path, rel: &str, id: &str) -> Result<String, String> {
    if id.contains(['/', '\\']) || id.contains("..") {
        return Err("invalid snapshot id".into());
    }
    std::fs::read_to_string(history_dir_for(mem, rel).join(id)).map_err(|e| e.to_string())
}

/// Writes a file atomically after snapshotting its previous version.
pub fn write(mem: &Path, rel: &str, content: &str) -> Result<(), String> {
    let rel = validate_rel(rel)?;
    if content.len() > MAX_FILE_BYTES {
        return Err(format!("{rel} is larger than {} KB", MAX_FILE_BYTES / 1024));
    }
    let p = abs(mem, &rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    snapshot(mem, &rel).map_err(|e| e.to_string())?;
    let tmp = p.with_extension("md.tmp");
    std::fs::write(&tmp, content).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
}

pub fn delete(mem: &Path, rel: &str) -> Result<(), String> {
    let rel = validate_rel(rel)?;
    if CORE_FILES.contains(&rel.as_str()) {
        return Err(format!("{rel} is a core memory file and can't be deleted (you can edit it instead)."));
    }
    snapshot(mem, &rel).map_err(|e| e.to_string())?;
    std::fs::remove_file(abs(mem, &rel)).map_err(|e| e.to_string())
}

/// Rewrites index.md: every memory file with a one-line description.
pub fn regenerate_index(mem: &Path) -> std::io::Result<()> {
    let mut out = String::from("# Memory index\n\nAll memory files, kept up to date by Democritus.\n\n");
    for f in list(mem).into_iter().filter(|f| f.path != "index.md") {
        let desc = if f.description.is_empty() { f.title.clone() } else { f.description.clone() };
        out.push_str(&format!("- `{}`: {}\n", f.path, desc));
    }
    std::fs::write(mem.join("index.md"), out)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &target)?;
        } else {
            std::fs::copy(e.path(), target)?;
        }
    }
    Ok(())
}

/// Copies the whole memory folder (with history) to backups\memory-<stamp>\.
pub fn backup(mem: &Path, backups: &Path) -> std::io::Result<PathBuf> {
    let target = backups.join(format!("memory-{}", localtime::now().stamp()));
    copy_dir(mem, &target)?;
    Ok(target)
}

/// Empties memory\ and recreates the starting templates. Call backup() first.
pub fn reset(mem: &Path) -> std::io::Result<()> {
    if mem.exists() {
        for e in std::fs::read_dir(mem)? {
            let e = e?;
            if e.file_type()?.is_dir() {
                std::fs::remove_dir_all(e.path())?;
            } else {
                std::fs::remove_file(e.path())?;
            }
        }
    }
    ensure_structure(mem)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_mem(name: &str) -> PathBuf {
        let d = crate::db::test_dir(name);
        ensure_structure(&d).unwrap();
        d
    }

    #[test]
    fn creates_layout_without_overwriting() {
        let mem = temp_mem("layout");
        for f in CORE_FILES {
            assert!(mem.join(f).exists(), "{f}");
        }
        assert!(mem.join("projects").is_dir() && mem.join("sessions").is_dir() && mem.join(HISTORY_DIR).is_dir());
        std::fs::write(mem.join("operator.md"), "# Operator\nMine.\n").unwrap();
        ensure_structure(&mem).unwrap();
        assert_eq!(std::fs::read_to_string(mem.join("operator.md")).unwrap(), "# Operator\nMine.\n");
        let _ = std::fs::remove_dir_all(mem);
    }

    #[test]
    fn path_validation_keeps_writes_inside_memory() {
        assert_eq!(validate_rel("operator.md").unwrap(), "operator.md");
        assert_eq!(validate_rel("projects\\my-app.md").unwrap(), "projects/my-app.md");
        assert_eq!(validate_rel("memory/sessions/2026-10-16.md").unwrap(), "sessions/2026-10-16.md");
        for bad in [
            "../secrets.md",
            "projects/../../x.md",
            "C:\\Windows\\win.ini",
            "D:/Democritus/logs/x.md",
            "/etc/passwd.md",
            "\\\\server\\share\\x.md",
            ".history/operator.md/x.md",
            "projects/.hidden.md",
            "logs/activity.md",
            "projects/sub/deep.md",
            "notes.txt",
            "evil$(cmd).md",
            "",
        ] {
            assert!(validate_rel(bad).is_err(), "should reject {bad:?}");
        }
        assert!(!model_may_write("identity.md") && !model_may_write("index.md") && model_may_write("operator.md"));
    }

    #[test]
    fn slugs_are_safe_file_names() {
        assert_eq!(slugify("My Cool App!"), "my-cool-app");
        assert_eq!(slugify("  Démo / Tests  "), "d-mo-tests");
        assert!(validate_rel(&format!("projects/{}.md", slugify("../../etc"))).is_ok());
    }

    fn op(action: &str, section: Option<&str>, content: &str) -> Op {
        Op { file: "f.md".into(), section: section.map(String::from), action: action.into(), content: content.into() }
    }

    #[test]
    fn section_operations() {
        let text = "# State\n\n## Last task\nOld task\n\n## Next step\nStep 1\n";
        let t = apply_op(text, &op("replace_section", Some("Last task"), "Wrote the parser")).unwrap();
        assert_eq!(t, "# State\n\n## Last task\nWrote the parser\n\n## Next step\nStep 1\n");
        let t = apply_op(&t, &op("append", Some("## next step"), "Step 2")).unwrap();
        assert!(t.ends_with("## Next step\nStep 1\nStep 2\n"), "{t}");
        let t = apply_op(&t, &op("append", Some("Open questions"), "- Which DB?")).unwrap();
        assert!(t.ends_with("## Open questions\n- Which DB?\n"), "{t}");
        let t = apply_op(&t, &op("remove_section", Some("Open questions"), "")).unwrap();
        assert!(!t.contains("Open questions"));
        assert_eq!(apply_op("", &op("create_file", None, "# New\nBody")).unwrap(), "# New\nBody\n");
        assert!(apply_op("# Existing\n", &op("create_file", None, "More")).unwrap().contains("# Existing\n\nMore"));
        assert!(apply_op(text, &op("delete_everything", None, "")).is_err());
        assert!(apply_op(text, &op("append", None, &"x".repeat(MAX_OP_CHARS + 1))).is_err());
    }

    #[test]
    fn whole_file_dumps_are_split_into_sections() {
        // What qwen3.5:4b actually produced: the whole file inside "Active project",
        // with sections repeated.
        let dump = "# Current state\n\nWhat we're doing right now.\n\n## Active project\nGarden Planner: planning beds.\n\n## Last task\nNone yet.\n\n## Next step\nSketch the layout.\n\n## Next step\nSketch the bed layout.\n";
        let ops = split_op(op("replace_section", Some("Active project"), dump));
        let got: Vec<(Option<&str>, &str, &str)> = ops.iter().map(|o| (o.section.as_deref(), o.action.as_str(), o.content.as_str())).collect();
        assert_eq!(
            got,
            vec![
                (Some("Active project"), "replace_section", "Garden Planner: planning beds."),
                (Some("Last task"), "replace_section", "None yet."),
                (Some("Next step"), "replace_section", "Sketch the bed layout."),
            ]
        );
        // Applying them to the template gives a clean file: every heading once.
        let mut text = template("current_state.md").to_string();
        for o in &ops {
            text = apply_op(&text, o).unwrap();
        }
        assert_eq!(text.matches("## Active project").count(), 1);
        assert_eq!(text.matches("# Current state").count(), 1);
        assert!(text.contains("## Next step\nSketch the bed layout."));
        // Plain content is untouched.
        assert_eq!(split_op(op("append", Some("Facts"), "- Likes tea")).len(), 1);
    }

    #[test]
    fn writes_keep_history_and_restore() {
        let mem = temp_mem("history");
        write(&mem, "operator.md", "# Operator\nv2\n").unwrap();
        write(&mem, "operator.md", "# Operator\nv3\n").unwrap();
        let h = history(&mem, "operator.md");
        assert_eq!(h.len(), 2);
        assert!(read_snapshot(&mem, "operator.md", &h[0].id).unwrap().contains("v2"));
        assert!(read_snapshot(&mem, "operator.md", "../../identity.md").is_err());
        assert!(delete(&mem, "operator.md").is_err(), "core files can't be deleted");
        write(&mem, "projects/x.md", "# X\n").unwrap();
        delete(&mem, "projects/x.md").unwrap();
        assert_eq!(history(&mem, "projects/x.md").len(), 1, "deletion is undoable");
        let _ = std::fs::remove_dir_all(mem);
    }

    #[test]
    fn index_lists_files_and_reset_backs_up_first() {
        let mem = temp_mem("index");
        write(&mem, "projects/garden.md", &project_template("Garden planner")).unwrap();
        regenerate_index(&mem).unwrap();
        let idx = std::fs::read_to_string(mem.join("index.md")).unwrap();
        assert!(idx.contains("`projects/garden.md`") && idx.contains("`operator.md`"));
        let backups = mem.with_extension("backups");
        let b = backup(&mem, &backups).unwrap();
        assert!(b.join("projects").join("garden.md").exists());
        reset(&mem).unwrap();
        assert!(!mem.join("projects").join("garden.md").exists());
        assert!(mem.join("identity.md").exists());
        let _ = std::fs::remove_dir_all(mem);
        let _ = std::fs::remove_dir_all(backups);
    }
}
