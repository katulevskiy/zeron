//! Native session portability: carry a harness's own on-disk session from one
//! device to another so a moved chat resumes natively instead of replaying
//! its transcript (docs/plans/2026-09-30-agent-mobility-and-policy.md,
//! "Harness sessions").
//!
//! Export lists the files that make up one session in the harness's store on
//! this device. Import places copies of them in the store on another device,
//! keyed by the NEW cwd, and rewrites the absolute cwd references the CLI
//! reads back on resume. Harnesses whose state is opaque (opencode's server,
//! Devin, Hermes, Antigravity) are not ported; the caller replays instead.
//!
//! Rewrites are JSON-aware and deliberately narrow: only known cwd-bearing
//! keys are touched, and only when the value is the old cwd or a path below
//! it. The original bytes around a rewritten string are kept, so untouched
//! records stay byte-identical and tool output is never edited. Moving a chat
//! back to its original cwd therefore restores the original bytes, which
//! matters to stores that index a history file by byte offset (Codex).
//!
//! Collision policy: a different file already at a destination is moved into
//! `<harness home>/.zeron-backups/<stamp>/` before the import replaces it.
//! Backups live outside the session trees because the CLIs find sessions by
//! scanning those trees for the id, and a sibling `*.bak` would be found too.

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use zeron_proto::HarnessId;

/// Where each harness keeps its sessions on one device.
#[derive(Clone, Debug)]
pub struct PortRoots {
    pub home: PathBuf,
    /// Claude config root (`$CLAUDE_CONFIG_DIR`, else `~/.claude`).
    pub claude: PathBuf,
    /// `$CODEX_HOME`, else `~/.codex`.
    pub codex: PathBuf,
    /// Pi's agent dir (`$PI_CODING_AGENT_DIR`, else `~/.pi/agent`).
    pub pi_agent: PathBuf,
    /// Zeron's Cursor store root (`$ZERON_CURSOR_STATE_DIR`, else `~/.zeron/cursor-state`).
    pub cursor_state: PathBuf,
    /// Grok's sessions dir (`$GROK_HOME/sessions`, else `~/.grok/sessions`).
    pub grok_sessions: PathBuf,
}

impl PortRoots {
    /// The roots the drivers themselves use on this device.
    pub fn from_env() -> Self {
        let home = crate::executable::home_or_current_dir();
        Self {
            claude: crate::model_context::root("CLAUDE_CONFIG_DIR", home.join(".claude")),
            codex: crate::model_context::root("CODEX_HOME", home.join(".codex")),
            pi_agent: crate::pi::agent_dir(None),
            cursor_state: crate::cursor::state_root(),
            grok_sessions: crate::model_context::root("GROK_HOME", home.join(".grok"))
                .join("sessions"),
            home,
        }
    }

    /// Default locations under `home`, ignoring environment overrides.
    pub fn under(home: &Path) -> Self {
        Self {
            home: home.to_owned(),
            claude: home.join(".claude"),
            codex: home.join(".codex"),
            pi_agent: home.join(".pi/agent"),
            cursor_state: home.join(".zeron/cursor-state"),
            grok_sessions: home.join(".grok/sessions"),
        }
    }
}

/// One session's native files, listed on the exporting device.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionExport {
    pub harness: HarnessId,
    pub session_id: String,
    /// The chat's cwd on the exporting device.
    pub cwd: String,
    pub files: Vec<ExportFile>,
    /// Harness-specific facts the importer needs (file names, the cwd the
    /// CLI recorded, subagent children).
    pub meta: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ExportFile {
    /// `/`-separated logical name the importer understands.
    pub rel: String,
    /// Absolute source on the exporting device.
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ImportedSession {
    /// The id to resume with on the importing device.
    pub session_id: String,
    /// Non-fatal notes: replaced files and where their backups went.
    pub warnings: Vec<String>,
}

/// Whether `harness` has a native session port. Others replay the transcript.
pub fn portable(harness: HarnessId) -> bool {
    matches!(
        harness,
        HarnessId::ClaudeCode
            | HarnessId::Codex
            | HarnessId::Pi
            | HarnessId::Cursor
            | HarnessId::Grok
    )
}

/// List the native files of `session_id`. `None` = this harness can't be
/// ported (the caller replays the transcript). An error means the session
/// itself couldn't be found or is not portable; the caller replays too.
pub fn export_session(
    harness: HarnessId,
    roots: &PortRoots,
    session_id: &str,
    cwd: &str,
) -> anyhow::Result<Option<SessionExport>> {
    let cwd = trim_path(cwd);
    let export = match harness {
        HarnessId::ClaudeCode => crate::claude::port::export(roots, session_id, cwd)?,
        HarnessId::Codex => crate::codex::port::export(roots, session_id, cwd)?,
        HarnessId::Pi => crate::pi::port::export(roots, session_id, cwd)?,
        HarnessId::Cursor => crate::cursor::port::export(roots, session_id, cwd)?,
        HarnessId::Grok => crate::acp::grok_port::export(roots, session_id, cwd)?,
        _ => return Ok(None),
    };
    Ok(Some(export))
}

/// Place an exported session in this device's store for `new_cwd`.
/// `staged` holds the exported files at their `rel` paths.
pub fn import_session(
    export: &SessionExport,
    staged: &Path,
    roots: &PortRoots,
    new_cwd: &str,
) -> anyhow::Result<ImportedSession> {
    for file in &export.files {
        checked_rel(&file.rel)?;
    }
    let new_cwd = trim_path(new_cwd);
    if new_cwd.is_empty() {
        bail!("the new cwd is empty");
    }
    let mut import = Import {
        export,
        staged,
        new_cwd,
        warnings: Vec::new(),
    };
    match export.harness {
        HarnessId::ClaudeCode => crate::claude::port::import(&mut import, roots)?,
        HarnessId::Codex => crate::codex::port::import(&mut import, roots)?,
        HarnessId::Pi => crate::pi::port::import(&mut import, roots)?,
        HarnessId::Cursor => crate::cursor::port::import(&mut import, roots)?,
        HarnessId::Grok => crate::acp::grok_port::import(&mut import, roots)?,
        other => bail!("{other:?} sessions are not portable"),
    }
    Ok(ImportedSession {
        session_id: export.session_id.clone(),
        warnings: import.warnings,
    })
}

/// Copy the exported files into `dest_dir` at their `rel` paths and return an
/// export pointing at the copies. Freezes a live JSONL: a torn final line (a
/// write raced the copy) is dropped; a complete one without newline is kept.
pub fn snapshot_export(export: &SessionExport, dest_dir: &Path) -> anyhow::Result<SessionExport> {
    let mut frozen = export.clone();
    for file in &mut frozen.files {
        let dest = dest_dir.join(checked_rel(&file.rel)?);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&file.path, &dest)
            .with_context(|| format!("copying {}", file.path.display()))?;
        if is_jsonl(&file.rel) {
            drop_torn_tail(&dest)?;
        }
        file.path = dest;
    }
    Ok(frozen)
}

// ---------------------------------------------------------------------------
// Import context shared by the per-harness ports
// ---------------------------------------------------------------------------

pub(crate) struct Import<'a> {
    pub export: &'a SessionExport,
    pub staged: &'a Path,
    pub new_cwd: &'a str,
    pub warnings: Vec<String>,
}

impl Import<'_> {
    /// Staged files, with their checked relative path.
    pub fn files(&self) -> impl Iterator<Item = (&str, PathBuf)> {
        self.export.files.iter().map(|f| {
            let rel = checked_rel(&f.rel).expect("rels are checked before import");
            (f.rel.as_str(), self.staged.join(rel))
        })
    }

    /// Old → new path roots for cwd rewrites: the chat's cwd plus the cwd the
    /// CLI recorded (they differ when one is a symlinked or canonical form).
    pub fn cwd_maps(&self) -> Vec<(String, String)> {
        let mut olds = vec![trim_path(&self.export.cwd).to_owned()];
        if let Some(recorded) = self.export.meta["recordedCwd"].as_str() {
            olds.push(trim_path(recorded).to_owned());
        }
        olds.retain(|old| !old.is_empty());
        olds.dedup();
        let mut maps: Vec<_> = olds
            .into_iter()
            .map(|old| (old, self.new_cwd.to_owned()))
            .collect();
        sort_maps(&mut maps);
        maps
    }

    pub fn meta_str(&self, key: &str) -> Option<&str> {
        self.export.meta[key].as_str()
    }
}

pub(crate) fn sort_maps(maps: &mut [(String, String)]) {
    // Most specific root first.
    maps.sort_by_key(|(old, _)| std::cmp::Reverse(old.len()));
}

/// Writes files into one harness store: atomic replace, identical files left
/// alone, different ones moved to `<home>/.zeron-backups/<stamp>/` first.
pub(crate) struct Placer<'a> {
    home: PathBuf,
    stamp: String,
    warnings: &'a mut Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Placed {
    New,
    Unchanged,
    /// A different file was there; it was backed up. `prefix` = the old file
    /// is a byte prefix of the new one (the session only grew).
    Replaced {
        prefix: bool,
    },
}

impl<'a> Placer<'a> {
    pub fn new(home: &Path, warnings: &'a mut Vec<String>) -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        Self {
            home: home.to_owned(),
            stamp: format!(
                "{stamp}-{}",
                &uuid::Uuid::new_v4().simple().to_string()[..8]
            ),
            warnings,
        }
    }

    pub fn warn(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    /// Copy `src` to `dest`, rewriting cwd references when `rewrite` is set.
    pub fn place(
        &mut self,
        src: &Path,
        dest: &Path,
        rewrite: Option<(&Rewriter<'_>, Format)>,
    ) -> anyhow::Result<Placed> {
        let parent = dest.parent().context("destination has no parent")?;
        std::fs::create_dir_all(parent)?;
        let name = dest.file_name().context("destination has no name")?;
        let temp = parent.join(format!(
            ".{}.zeron-tmp-{}",
            name.to_string_lossy(),
            uuid::Uuid::new_v4().simple()
        ));
        let written = (|| -> anyhow::Result<()> {
            let mut out = BufWriter::new(std::fs::File::create(&temp)?);
            match rewrite {
                Some((rw, Format::Lines)) => rw.rewrite_lines(src, &mut out)?,
                Some((rw, Format::Document)) => {
                    let bytes = std::fs::read(src)?;
                    out.write_all(rw.rewrite_doc(&bytes).as_deref().unwrap_or(&bytes))?;
                }
                None => {
                    std::io::copy(&mut std::fs::File::open(src)?, &mut out)?;
                }
            }
            out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
            Ok(())
        })()
        .with_context(|| format!("writing {}", dest.display()));
        if let Err(error) = written {
            let _ = std::fs::remove_file(&temp);
            return Err(error);
        }
        let mut placed = Placed::New;
        if dest.exists() {
            if same_contents(&temp, dest)? {
                std::fs::remove_file(&temp)?;
                return Ok(Placed::Unchanged);
            }
            placed = Placed::Replaced {
                prefix: is_prefix(dest, &temp)?,
            };
            self.retire(dest)?;
        }
        if let Err(error) = std::fs::rename(&temp, dest) {
            let _ = std::fs::remove_file(&temp);
            return Err(error).with_context(|| format!("placing {}", dest.display()));
        }
        Ok(placed)
    }

    /// Move `path` (file or dir) into this import's backup folder.
    pub fn retire(&mut self, path: &Path) -> anyhow::Result<()> {
        let rel = path
            .strip_prefix(&self.home)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.file_name().map(PathBuf::from).unwrap_or_default());
        let base = self.home.join(".zeron-backups").join(&self.stamp).join(rel);
        let mut backup = base.clone();
        for n in 1.. {
            if !backup.exists() {
                break;
            }
            backup = PathBuf::from(format!("{}.{n}", base.display()));
        }
        if let Some(parent) = backup.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(path, &backup).with_context(|| format!("backing up {}", path.display()))?;
        self.warnings.push(format!(
            "replaced {}; the previous copy is at {}",
            path.display(),
            backup.display()
        ));
        Ok(())
    }
}

/// Write `contents` to `dest` atomically (small index/marker files).
pub(crate) fn write_atomic(dest: &Path, contents: &[u8]) -> anyhow::Result<()> {
    let parent = dest.parent().context("destination has no parent")?;
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".zeron-tmp-{}", uuid::Uuid::new_v4().simple()));
    let result = std::fs::write(&temp, contents).and_then(|()| std::fs::rename(&temp, dest));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.with_context(|| format!("writing {}", dest.display()))
}

fn same_contents(a: &Path, b: &Path) -> std::io::Result<bool> {
    if std::fs::metadata(a)?.len() != std::fs::metadata(b)?.len() {
        return Ok(false);
    }
    is_prefix(a, b)
}

/// Whether `a`'s bytes are a prefix of `b`'s.
fn is_prefix(a: &Path, b: &Path) -> std::io::Result<bool> {
    if std::fs::metadata(a)?.len() > std::fs::metadata(b)?.len() {
        return Ok(false);
    }
    let (mut a, mut b) = (
        BufReader::new(std::fs::File::open(a)?),
        BufReader::new(std::fs::File::open(b)?),
    );
    let (mut x, mut y) = (vec![0; 1 << 16], vec![0; 1 << 16]);
    loop {
        let n = a.read(&mut x)?;
        if n == 0 {
            return Ok(true);
        }
        b.read_exact(&mut y[..n])?;
        if x[..n] != y[..n] {
            return Ok(false);
        }
    }
}

// ---------------------------------------------------------------------------
// Small shared helpers
// ---------------------------------------------------------------------------

/// A `/`-separated relative name from another device, as a safe local path.
pub(crate) fn checked_rel(rel: &str) -> anyhow::Result<PathBuf> {
    let mut path = PathBuf::new();
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(['\\', ':', '\0']) {
            bail!("unsafe file name in session export: {rel:?}");
        }
        path.push(part);
    }
    Ok(path)
}

/// A session id that is safe as one path component.
pub(crate) fn checked_id(id: &str) -> anyhow::Result<&str> {
    if id.is_empty()
        || id.len() > 200
        || id.starts_with('.')
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        bail!("unsupported session id {id:?}");
    }
    Ok(id)
}

/// A single file name from another device.
pub(crate) fn checked_name(name: &str) -> anyhow::Result<&str> {
    checked_rel(name)?;
    if name.contains('/') {
        bail!("unsafe file name in session export: {name:?}");
    }
    Ok(name)
}

/// All regular files under `dir` (no symlinks), as `prefix/<rel>` entries.
/// Leftover temp files of an interrupted import are never listed.
pub(crate) fn list_files(
    dir: &Path,
    prefix: &str,
    skip: impl Fn(&str) -> bool,
) -> anyhow::Result<Vec<ExportFile>> {
    let mut files = Vec::new();
    let mut pending = vec![(dir.to_owned(), prefix.to_owned())];
    while let Some((dir, rel)) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let kind = entry.file_type()?;
            let rel = format!("{rel}/{name}");
            if skip(&name) || name.contains(".zeron-tmp-") || checked_rel(&rel).is_err() {
                continue;
            }
            if kind.is_dir() {
                pending.push((entry.path(), rel));
            } else if kind.is_file() {
                files.push(ExportFile {
                    rel,
                    path: entry.path(),
                });
            }
            if files.len() > 50_000 {
                bail!("{} holds too many files to move", dir.display());
            }
        }
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(files)
}

/// The first line of a file (bounded), parsed.
pub(crate) fn first_record(path: &Path) -> Option<serde_json::Value> {
    let mut line = Vec::new();
    BufReader::new(std::fs::File::open(path).ok()?.take(4 << 20))
        .read_until(b'\n', &mut line)
        .ok()?;
    serde_json::from_slice(&line).ok()
}

pub(crate) fn read_json(path: &Path) -> serde_json::Value {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn is_jsonl(name: &str) -> bool {
    name.ends_with(".jsonl") || name.ends_with(".ndjson")
}

/// A cwd without trailing separators (a root stays a root).
pub(crate) fn trim_path(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() || trimmed.ends_with(':') {
        path
    } else {
        trimmed
    }
}

/// The cwd as the OS reports it to a child (symlinks resolved), when it
/// exists on this device. CLIs that key stores by `process.cwd()` see this.
pub(crate) fn physical_cwd(cwd: &str) -> String {
    #[cfg(unix)]
    if let Ok(path) = std::fs::canonicalize(cwd)
        && let Some(path) = path.to_str()
    {
        return path.to_owned();
    }
    cwd.to_owned()
}

fn drop_torn_tail(path: &Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(path)?;
    if bytes.is_empty() || bytes.ends_with(b"\n") {
        return Ok(());
    }
    let start = bytes
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |at| at + 1);
    if serde_json::from_slice::<serde::de::IgnoredAny>(&bytes[start..]).is_err() {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)?
            .set_len(start as u64)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Path rewriting
// ---------------------------------------------------------------------------

fn windows_style(path: &str) -> bool {
    let b = path.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') || path.starts_with("\\\\")
}

/// Map a path value from an old root to a new one: the root itself, or a
/// path below it. The remainder takes the new root's separator style, and
/// Windows roots compare case-insensitively.
pub(crate) fn rewrite_path(value: &str, maps: &[(String, String)]) -> Option<String> {
    for (old, new) in maps {
        let windows = windows_style(old);
        let Some(head) = value.get(..old.len()) else {
            continue;
        };
        let matches = if windows {
            head.eq_ignore_ascii_case(old)
        } else {
            head == old
        };
        if !matches {
            continue;
        }
        let rest = &value[old.len()..];
        if rest.is_empty() {
            return Some(new.clone());
        }
        // `/` alone as a root would claim every absolute path.
        if old.len() <= 1 || !rest.starts_with(['/', '\\']) || (!windows && rest.starts_with('\\'))
        {
            continue;
        }
        let rest = match (windows, windows_style(new)) {
            (true, false) => rest.replace('\\', "/"),
            (false, true) => rest.replace('/', "\\"),
            _ => rest.to_owned(),
        };
        return Some(format!("{new}{rest}"));
    }
    None
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Seg {
    Key(&'static str),
    /// Any object key or array element.
    Any,
}

/// A cwd-bearing location: a key path, optionally only in records whose
/// top-level `"type"` is `kind`.
pub(crate) struct Rule {
    pub kind: Option<&'static str>,
    pub path: &'static [Seg],
}

#[derive(Clone, Copy)]
pub(crate) enum Format {
    /// One JSON record per line.
    Lines,
    /// A single JSON document (possibly pretty-printed).
    Document,
}

pub(crate) struct Rewriter<'a> {
    maps: &'a [(String, String)],
    rules: &'a [Rule],
    /// The old roots as they appear inside JSON strings: a cheap pre-check
    /// before parsing a record.
    needles: Vec<(Vec<u8>, bool)>,
}

impl<'a> Rewriter<'a> {
    pub fn new(maps: &'a [(String, String)], rules: &'a [Rule]) -> Self {
        let needles = maps
            .iter()
            .map(|(old, _)| {
                let json = serde_json::to_string(old).unwrap_or_default();
                let inner = json.as_bytes()[1..json.len() - 1].to_vec();
                if windows_style(old) {
                    (inner.to_ascii_lowercase(), true)
                } else {
                    (inner, false)
                }
            })
            .collect();
        Self {
            maps,
            rules,
            needles,
        }
    }

    fn mentions_old_root(&self, bytes: &[u8]) -> bool {
        let mut lowered: Option<Vec<u8>> = None;
        self.needles.iter().any(|(needle, fold)| {
            let haystack: &[u8] = if *fold {
                lowered.get_or_insert_with(|| bytes.to_ascii_lowercase())
            } else {
                bytes
            };
            !needle.is_empty()
                && haystack
                    .windows(needle.len())
                    .any(|w| w == needle.as_slice())
        })
    }

    /// The rewritten document, or `None` when nothing changes (including
    /// anything that isn't valid JSON — torn lines are kept verbatim).
    pub fn rewrite_doc(&self, bytes: &[u8]) -> Option<Vec<u8>> {
        if !self.mentions_old_root(bytes) {
            return None;
        }
        #[derive(Deserialize)]
        struct Kind<'b> {
            #[serde(rename = "type", borrow, default)]
            kind: Option<Cow<'b, str>>,
        }
        let kind = serde_json::from_slice::<Kind>(bytes).ok()?.kind;
        let paths: Vec<&[Seg]> = self
            .rules
            .iter()
            .filter(|rule| rule.kind.is_none_or(|k| kind.as_deref() == Some(k)))
            .map(|rule| rule.path)
            .collect();
        if paths.is_empty() {
            return None;
        }
        let mut scan = Scanner {
            b: bytes,
            i: 0,
            hits: Vec::new(),
        };
        scan.ws();
        scan.value(&paths, 0)?;
        scan.ws();
        if scan.i != bytes.len() {
            return None;
        }
        let mut out = Vec::with_capacity(bytes.len() + 64);
        let mut last = 0;
        for (start, end) in scan.hits {
            let value: String = serde_json::from_slice(&bytes[start..end]).ok()?;
            if let Some(new) = rewrite_path(&value, self.maps) {
                out.extend_from_slice(&bytes[last..start]);
                out.extend_from_slice(serde_json::to_string(&new).ok()?.as_bytes());
                last = end;
            }
        }
        if last == 0 {
            return None;
        }
        out.extend_from_slice(&bytes[last..]);
        Some(out)
    }

    /// Stream a JSONL file through [`Self::rewrite_doc`], line by line.
    pub fn rewrite_lines(&self, src: &Path, out: &mut impl Write) -> anyhow::Result<()> {
        let mut reader = BufReader::with_capacity(1 << 16, std::fs::File::open(src)?);
        let mut line = Vec::new();
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 {
                return Ok(());
            }
            let body = line.strip_suffix(b"\n").unwrap_or(&line);
            match self.rewrite_doc(body) {
                Some(new) => {
                    out.write_all(&new)?;
                    if body.len() < line.len() {
                        out.write_all(b"\n")?;
                    }
                }
                None => out.write_all(&line)?,
            }
        }
    }
}

/// A minimal JSON walker that records the byte spans of string values at
/// the rule paths, so they can be replaced without re-serializing the
/// record. Input is pre-validated by serde; any surprise returns `None`.
struct Scanner<'b> {
    b: &'b [u8],
    i: usize,
    hits: Vec<(usize, usize)>,
}

impl<'b> Scanner<'b> {
    fn ws(&mut self) {
        while self
            .b
            .get(self.i)
            .is_some_and(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Option<()> {
        (self.b.get(self.i) == Some(&c)).then(|| self.i += 1)
    }

    /// A string token's span, quotes included.
    fn string(&mut self) -> Option<(usize, usize)> {
        let start = self.i;
        self.eat(b'"')?;
        loop {
            match self.b.get(self.i)? {
                b'"' => {
                    self.i += 1;
                    return Some((start, self.i));
                }
                b'\\' => self.i += 2,
                _ => self.i += 1,
            }
        }
    }

    fn key(&self, (start, end): (usize, usize)) -> Option<Cow<'b, str>> {
        let raw = &self.b[start..end];
        if raw.contains(&b'\\') {
            serde_json::from_slice::<String>(raw).ok().map(Cow::Owned)
        } else {
            std::str::from_utf8(&raw[1..raw.len() - 1])
                .ok()
                .map(Cow::Borrowed)
        }
    }

    /// Walk one value. `alive` = rule paths whose first `depth` segments
    /// match the path to this value.
    fn value(&mut self, alive: &[&[Seg]], depth: usize) -> Option<()> {
        let deeper: Vec<&[Seg]> = alive.iter().copied().filter(|p| p.len() > depth).collect();
        match *self.b.get(self.i)? {
            b'"' => {
                let span = self.string()?;
                if alive.iter().any(|p| p.len() == depth) {
                    self.hits.push(span);
                }
            }
            b'{' => {
                self.i += 1;
                self.ws();
                if self.eat(b'}').is_some() {
                    return Some(());
                }
                loop {
                    self.ws();
                    let span = self.string()?;
                    let next: Vec<&[Seg]> = if deeper.is_empty() {
                        Vec::new()
                    } else {
                        let key = self.key(span)?;
                        deeper
                            .iter()
                            .copied()
                            .filter(|p| match p[depth] {
                                Seg::Key(k) => k == key,
                                Seg::Any => true,
                            })
                            .collect()
                    };
                    self.ws();
                    self.eat(b':')?;
                    self.ws();
                    self.value(&next, depth + 1)?;
                    self.ws();
                    if self.eat(b',').is_none() {
                        return self.eat(b'}');
                    }
                }
            }
            b'[' => {
                self.i += 1;
                self.ws();
                if self.eat(b']').is_some() {
                    return Some(());
                }
                let next: Vec<&[Seg]> = deeper
                    .iter()
                    .copied()
                    .filter(|p| matches!(p[depth], Seg::Any))
                    .collect();
                loop {
                    self.ws();
                    self.value(&next, depth + 1)?;
                    self.ws();
                    if self.eat(b',').is_none() {
                        return self.eat(b']');
                    }
                }
            }
            _ => {
                let start = self.i;
                while self
                    .b
                    .get(self.i)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'+' | b'.'))
                {
                    self.i += 1;
                }
                if self.i == start {
                    return None;
                }
            }
        }
        Some(())
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub const OLD_CWD: &str = "/Users/alice/dev/proj";
    pub const NEW_CWD: &str = "/home/bob/src/proj";

    /// Two devices' worth of harness stores inside one temp dir.
    pub fn devices() -> (tempfile::TempDir, PortRoots, PortRoots) {
        let dir = tempfile::tempdir().unwrap();
        let a = PortRoots::under(&dir.path().join("a"));
        let b = PortRoots::under(&dir.path().join("b"));
        (dir, a, b)
    }

    /// Export on A, carry the files (via a snapshot) and import on B.
    pub fn carry(
        harness: HarnessId,
        a: &PortRoots,
        b: &PortRoots,
        id: &str,
        old_cwd: &str,
        new_cwd: &str,
    ) -> (SessionExport, anyhow::Result<ImportedSession>) {
        let export = export_session(harness, a, id, old_cwd).unwrap().unwrap();
        let staged = a
            .home
            .parent()
            .unwrap()
            .join(format!("staged-{}", uuid::Uuid::new_v4().simple()));
        let frozen = snapshot_export(&export, &staged).unwrap();
        // The wire carries the export as JSON.
        let wire: SessionExport =
            serde_json::from_str(&serde_json::to_string(&frozen).unwrap()).unwrap();
        let imported = import_session(&wire, &staged, b, new_cwd);
        (export, imported)
    }

    pub fn write(path: &Path, contents: impl AsRef<[u8]>) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    pub fn lines(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    pub fn backups(home: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut pending = vec![home.join(".zeron-backups")];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                if entry.file_type().unwrap().is_dir() {
                    pending.push(entry.path());
                } else {
                    found.push(entry.path());
                }
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use serde_json::json;

    fn maps(old: &str, new: &str) -> Vec<(String, String)> {
        vec![(old.into(), new.into())]
    }

    #[test]
    fn path_rewrite_covers_the_root_and_below_but_not_lookalikes() {
        let m = maps(OLD_CWD, NEW_CWD);
        assert_eq!(rewrite_path(OLD_CWD, &m).unwrap(), NEW_CWD);
        assert_eq!(
            rewrite_path("/Users/alice/dev/proj/src/main.rs", &m).unwrap(),
            "/home/bob/src/proj/src/main.rs"
        );
        assert_eq!(rewrite_path("/Users/alice/dev/project", &m), None);
        assert_eq!(rewrite_path("/Users/alice/dev", &m), None);
        assert_eq!(rewrite_path("relative/Users/alice/dev/proj", &m), None);
        assert_eq!(rewrite_path("/x", &maps("/", "/y")), None);
    }

    #[test]
    fn path_rewrite_crosses_operating_systems() {
        let to_unix = maps(r"C:\Users\alice\proj", "/home/bob/proj");
        assert_eq!(
            rewrite_path(r"c:\users\alice\proj\src\a.rs", &to_unix).unwrap(),
            "/home/bob/proj/src/a.rs"
        );
        assert_eq!(
            rewrite_path(r"C:\Users\alice\project", &to_unix),
            None,
            "a sibling with a common prefix is not below the root"
        );
        let to_windows = maps("/Users/alice/proj", r"D:\work\proj");
        assert_eq!(
            rewrite_path("/Users/alice/proj/src/a.rs", &to_windows).unwrap(),
            r"D:\work\proj\src\a.rs"
        );
        assert_eq!(trim_path("/a/b/"), "/a/b");
        assert_eq!(trim_path("/"), "/");
        assert_eq!(trim_path(r"C:\"), r"C:\");
    }

    #[test]
    fn rewrites_only_rule_paths_and_keeps_every_other_byte() {
        const RULES: &[Rule] = &[
            Rule {
                kind: None,
                path: &[Seg::Key("cwd")],
            },
            Rule {
                kind: Some("ctx"),
                path: &[Seg::Key("roots"), Seg::Any],
            },
            Rule {
                kind: None,
                path: &[Seg::Key("ctx"), Seg::Any, Seg::Key("cwd")],
            },
        ];
        let m = maps(OLD_CWD, r"C:\Users\bob\proj");
        let rw = Rewriter::new(&m, RULES);
        let line = br#"{"z":1.50e3, "type":"ctx","cwd" : "/Users/alice/dev/proj","roots":["/Users/alice/dev/proj/sub","/opt"],"out":"cd /Users/alice/dev/proj","nested":{"cwd":"/Users/alice/dev/proj"},"ctx":{"t1":{"cwd":"/Users/alice/dev/proj"}},"u":"\u00e9\"x"}"#;
        let out = String::from_utf8(rw.rewrite_doc(line).unwrap()).unwrap();
        assert_eq!(
            out,
            r#"{"z":1.50e3, "type":"ctx","cwd" : "C:\\Users\\bob\\proj","roots":["C:\\Users\\bob\\proj\\sub","/opt"],"out":"cd /Users/alice/dev/proj","nested":{"cwd":"/Users/alice/dev/proj"},"ctx":{"t1":{"cwd":"C:\\Users\\bob\\proj"}},"u":"\u00e9\"x"}"#
        );
        // The kind filter: `roots` only in `ctx` records.
        let other = br#"{"type":"other","roots":["/Users/alice/dev/proj"]}"#;
        assert_eq!(rw.rewrite_doc(other), None);
        // Torn or foreign lines are left alone.
        assert_eq!(rw.rewrite_doc(br#"{"cwd":"/Users/alice/dev/proj""#), None);
        assert_eq!(rw.rewrite_doc(b"cwd /Users/alice/dev/proj"), None);
        // Pretty documents keep their layout.
        let pretty = serde_json::to_vec_pretty(&json!({"cwd": OLD_CWD, "n": [1, 2]})).unwrap();
        let out = String::from_utf8(rw.rewrite_doc(&pretty).unwrap()).unwrap();
        assert_eq!(
            out,
            String::from_utf8(pretty)
                .unwrap()
                .replace(OLD_CWD, r"C:\\Users\\bob\\proj")
        );
    }

    #[test]
    fn snapshot_freezes_files_and_drops_only_a_torn_tail() {
        let dir = tempfile::tempdir().unwrap();
        let torn = dir.path().join("src/torn.jsonl");
        let whole = dir.path().join("src/whole.ndjson");
        let text = dir.path().join("src/out.txt");
        write(&torn, "{\"a\":1}\n{\"b\":");
        write(&whole, "{\"a\":1}\n{\"b\":2}");
        write(&text, "partial line");
        let export = SessionExport {
            harness: HarnessId::Mock,
            session_id: "s".into(),
            cwd: OLD_CWD.into(),
            files: vec![
                ExportFile {
                    rel: "x/torn.jsonl".into(),
                    path: torn.clone(),
                },
                ExportFile {
                    rel: "whole.ndjson".into(),
                    path: whole.clone(),
                },
                ExportFile {
                    rel: "out.txt".into(),
                    path: text,
                },
            ],
            meta: json!({}),
        };
        let dest = dir.path().join("frozen");
        let frozen = snapshot_export(&export, &dest).unwrap();
        assert_eq!(frozen.files[0].path, dest.join("x/torn.jsonl"));
        assert_eq!(
            std::fs::read_to_string(&frozen.files[0].path).unwrap(),
            "{\"a\":1}\n"
        );
        assert_eq!(
            std::fs::read_to_string(&frozen.files[1].path).unwrap(),
            "{\"a\":1}\n{\"b\":2}"
        );
        assert_eq!(
            std::fs::read_to_string(&frozen.files[2].path).unwrap(),
            "partial line"
        );
        // The source keeps growing; the snapshot doesn't.
        write(&torn, "{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n");
        assert_eq!(
            std::fs::read_to_string(&frozen.files[0].path).unwrap(),
            "{\"a\":1}\n"
        );
    }

    #[test]
    fn opaque_harnesses_are_not_portable() {
        let (_dir, a, b) = devices();
        for harness in [
            HarnessId::Opencode,
            HarnessId::Devin,
            HarnessId::Hermes,
            HarnessId::Antigravity,
            HarnessId::Mock,
        ] {
            assert!(!portable(harness), "{harness:?}");
            assert!(
                export_session(harness, &a, "id", OLD_CWD)
                    .unwrap()
                    .is_none()
            );
            let export = SessionExport {
                harness,
                session_id: "id".into(),
                cwd: OLD_CWD.into(),
                files: vec![],
                meta: json!({}),
            };
            assert!(import_session(&export, &a.home, &b, NEW_CWD).is_err());
        }
        for harness in [
            HarnessId::ClaudeCode,
            HarnessId::Codex,
            HarnessId::Pi,
            HarnessId::Cursor,
            HarnessId::Grok,
        ] {
            assert!(portable(harness), "{harness:?}");
        }
    }

    #[test]
    fn imports_refuse_names_that_escape_the_store() {
        let (_dir, a, b) = devices();
        for rel in [
            "../x.jsonl",
            "/etc/passwd",
            "a//b",
            r"a\..\b",
            "C:/x",
            "a/./b",
        ] {
            let export = SessionExport {
                harness: HarnessId::ClaudeCode,
                session_id: "id".into(),
                cwd: OLD_CWD.into(),
                files: vec![ExportFile {
                    rel: rel.into(),
                    path: a.home.join("x"),
                }],
                meta: json!({}),
            };
            assert!(
                import_session(&export, &a.home, &b, NEW_CWD).is_err(),
                "{rel}"
            );
        }
        assert!(checked_id("../x").is_err());
        assert!(checked_id("..").is_err());
        assert!(checked_id("ok-ID_1.2").is_ok());
    }

    #[test]
    fn placer_backs_up_a_different_file_and_skips_an_identical_one() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let src = dir.path().join("src.txt");
        let dest = home.join("store/s.txt");
        write(&src, "new\n");
        let mut warnings = Vec::new();
        let mut placer = Placer::new(&home, &mut warnings);
        assert_eq!(placer.place(&src, &dest, None).unwrap(), Placed::New);
        assert_eq!(placer.place(&src, &dest, None).unwrap(), Placed::Unchanged);
        write(&src, "new\nmore\n");
        assert_eq!(
            placer.place(&src, &dest, None).unwrap(),
            Placed::Replaced { prefix: true }
        );
        write(&src, "other\n");
        assert_eq!(
            placer.place(&src, &dest, None).unwrap(),
            Placed::Replaced { prefix: false }
        );
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "other\n");
        let mut saved: Vec<_> = backups(&home)
            .iter()
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect();
        saved.sort();
        assert_eq!(saved, ["new\n", "new\nmore\n"]);
        assert!(backups(&home).iter().all(|p| {
            p.parent().unwrap().ends_with("store")
                && p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("s.txt")
        }));
        assert_eq!(warnings.len(), 2);
        // No temp files linger next to the destination.
        assert_eq!(std::fs::read_dir(home.join("store")).unwrap().count(), 1);
    }
}
