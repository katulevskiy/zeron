//! Incremental, parallel log scanning for [`super::UsageScanner`].
//!
//! Every source is append-only JSONL except OpenCode (SQLite). Each file
//! keeps a byte cursor, so a rescan reads only what was appended; files are
//! scanned on a few threads at once; and a `memchr` substring prefilter
//! skips the overwhelming majority of lines (message bodies, tool output)
//! before any JSON parsing. Only the handful of numeric/id fields needed are
//! deserialized — content is skipped by serde's `IgnoredAny` path.

use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use memchr::memmem;
use serde::{Deserialize, Serialize};

use crate::turn::model_token;

/// One usage-bearing log line, normalized.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Record {
    pub day: String,
    pub source: &'static str,
    pub model: String,
    pub origin: String,
    pub session: u64,
    /// Present when the source can repeat a line (Claude writes one line per
    /// content block of the same response; resumed sessions copy history).
    pub dedupe: Option<u64>,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub reasoning: u64,
    pub cost_usd: Option<f64>,
}

/// Per-file resume point (plus Codex's running context, which lives in
/// earlier lines of the same file).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct FileCursor {
    pub offset: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex: Option<CodexCursor>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct CodexCursor {
    pub model: Option<String>,
    pub origin: Option<String>,
    pub session: u64,
    pub last_total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Claude,
    Codex,
    Pi,
}

pub(super) struct FileScan {
    pub path: PathBuf,
    pub cursor: FileCursor,
    pub records: Vec<Record>,
    pub bytes: u64,
}

pub(super) fn hash64(parts: &[&str]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for part in parts {
        part.hash(&mut h);
    }
    h.finish()
}

fn token(s: &str) -> String {
    if !s.is_empty()
        && s.len() <= 40
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        s.to_string()
    } else {
        "unknown".to_string()
    }
}

/// `YYYY-MM-DD` of an RFC 3339 timestamp, if it's inside the window.
fn day_of(ts: &str, horizon: &str) -> Option<String> {
    let parsed = chrono::DateTime::parse_from_rfc3339(ts).ok()?;
    let day = parsed
        .with_timezone(&chrono::Utc)
        .format("%Y-%m-%d")
        .to_string();
    (day.as_str() >= horizon).then_some(day)
}

/// All `*.jsonl` under `root`, at most `depth` directories deep.
pub(super) fn jsonl_files(root: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() && depth > 0 {
            jsonl_files(&path, depth - 1, out);
        } else if kind.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
            out.push(path);
        }
    }
}

/// Reads complete lines appended after `cursor.offset`, feeding the ones
/// that pass the prefilter to the source's parser. A trailing partial line
/// (a writer mid-append) is left for the next scan.
pub(super) fn scan_file(
    path: &Path,
    kind: Kind,
    mut cursor: FileCursor,
    horizon: &str,
) -> Option<FileScan> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len < cursor.offset {
        // Truncated or replaced: start over (dedupe ids absorb repeats).
        cursor = FileCursor::default();
    }
    if len == cursor.offset {
        return None;
    }
    file.seek(SeekFrom::Start(cursor.offset)).ok()?;
    let mut reader = std::io::BufReader::with_capacity(1 << 20, file.take(len - cursor.offset));
    let session = hash64(&[&path.to_string_lossy()]);
    let mut records = Vec::new();
    let mut line = Vec::with_capacity(64 * 1024);
    let mut consumed = 0u64;
    let finders = Finders::new();
    loop {
        line.clear();
        let n = reader.read_until(b'\n', &mut line).ok()?;
        if n == 0 || line.last() != Some(&b'\n') {
            break;
        }
        consumed += n as u64;
        match kind {
            Kind::Claude => {
                if finders.usage.find(&line).is_some()
                    && let Some(record) = parse_claude(&line, horizon)
                {
                    records.push(record);
                }
            }
            Kind::Codex => {
                let codex = cursor.codex.get_or_insert_with(|| CodexCursor {
                    session,
                    ..CodexCursor::default()
                });
                if let Some(record) = parse_codex(&line, codex, &finders, horizon) {
                    records.push(record);
                }
            }
            Kind::Pi => {
                if finders.usage.find(&line).is_some()
                    && let Some(record) = parse_pi(&line, session, horizon)
                {
                    records.push(record);
                }
            }
        }
    }
    cursor.offset += consumed;
    Some(FileScan {
        path: path.to_path_buf(),
        cursor,
        records,
        bytes: consumed,
    })
}

struct Finders {
    usage: memmem::Finder<'static>,
    token_count: memmem::Finder<'static>,
    turn_context: memmem::Finder<'static>,
    session_meta: memmem::Finder<'static>,
}

impl Finders {
    fn new() -> Self {
        Finders {
            usage: memmem::Finder::new(b"\"usage\""),
            token_count: memmem::Finder::new(b"\"token_count\""),
            turn_context: memmem::Finder::new(b"\"turn_context\""),
            session_meta: memmem::Finder::new(b"\"session_meta\""),
        }
    }
}

// ── Claude Code ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ClaudeLine<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow)]
    timestamp: Option<Cow<'a, str>>,
    #[serde(rename = "requestId", borrow)]
    request_id: Option<Cow<'a, str>>,
    #[serde(rename = "sessionId", borrow)]
    session_id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    entrypoint: Option<Cow<'a, str>>,
    #[serde(borrow)]
    message: Option<ClaudeMessage<'a>>,
}

#[derive(Deserialize)]
struct ClaudeMessage<'a> {
    #[serde(borrow)]
    id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    model: Option<Cow<'a, str>>,
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ClaudeUsage {
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
    output_tokens_details: Option<ThinkingDetails>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ThinkingDetails {
    thinking_tokens: u64,
}

fn parse_claude(line: &[u8], horizon: &str) -> Option<Record> {
    let parsed: ClaudeLine = serde_json::from_slice(line).ok()?;
    if parsed.kind.as_deref() != Some("assistant") {
        return None;
    }
    let message = parsed.message?;
    let usage = message.usage?;
    let model = message.model.as_deref().unwrap_or("unknown");
    if model == "<synthetic>" {
        return None;
    }
    let day = day_of(parsed.timestamp.as_deref()?, horizon)?;
    let dedupe = match (message.id.as_deref(), parsed.request_id.as_deref()) {
        (None, None) => None,
        (id, req) => Some(hash64(&[
            "claude",
            id.unwrap_or_default(),
            req.unwrap_or_default(),
        ])),
    };
    Some(Record {
        day,
        source: "claude-code",
        model: model_token(model).unwrap_or_else(|| "unknown".into()),
        origin: parsed
            .entrypoint
            .as_deref()
            .map(token)
            .unwrap_or_else(|| "unknown".into()),
        session: hash64(&[parsed.session_id.as_deref().unwrap_or_default()]),
        dedupe,
        input: usage.input_tokens,
        output: usage.output_tokens,
        cache_read: usage.cache_read_input_tokens,
        cache_write: usage.cache_creation_input_tokens,
        reasoning: usage
            .output_tokens_details
            .map(|d| d.thinking_tokens)
            .unwrap_or(0),
        cost_usd: None,
    })
}

// ── Codex ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CodexLine<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow)]
    timestamp: Option<Cow<'a, str>>,
    payload: Option<serde_json::Value>,
}

fn parse_codex(
    line: &[u8],
    cursor: &mut CodexCursor,
    finders: &Finders,
    horizon: &str,
) -> Option<Record> {
    let is_count = finders.token_count.find(line).is_some();
    let is_context = !is_count && finders.turn_context.find(line).is_some();
    let is_meta = !is_count && !is_context && finders.session_meta.find(line).is_some();
    if !(is_count || is_context || is_meta) {
        return None;
    }
    let parsed: CodexLine = serde_json::from_slice(line).ok()?;
    let payload = parsed.payload?;
    match parsed.kind.as_deref()? {
        "session_meta" => {
            if let Some(origin) = payload.get("originator").and_then(|v| v.as_str()) {
                cursor.origin = Some(token(origin));
            }
            if let Some(id) = payload.get("id").and_then(|v| v.as_str()) {
                cursor.session = hash64(&[id]);
            }
            None
        }
        "turn_context" => {
            if let Some(model) = payload.get("model").and_then(|v| v.as_str()) {
                cursor.model = model_token(model);
            }
            None
        }
        "event_msg" if payload.get("type").and_then(|v| v.as_str()) == Some("token_count") => {
            let info = payload.get("info")?;
            let total = info
                .get("total_token_usage")
                .and_then(|t| t.get("total_tokens"))
                .and_then(|v| v.as_u64())?;
            // Codex re-emits an unchanged count (rate-limit refreshes).
            if total == cursor.last_total {
                return None;
            }
            cursor.last_total = total;
            let last = info.get("last_token_usage")?;
            let n = |k: &str| last.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
            let cached = n("cached_input_tokens");
            Some(Record {
                day: day_of(parsed.timestamp.as_deref()?, horizon)?,
                source: "codex",
                model: cursor.model.clone().unwrap_or_else(|| "unknown".into()),
                origin: cursor.origin.clone().unwrap_or_else(|| "unknown".into()),
                session: cursor.session,
                dedupe: None,
                input: n("input_tokens").saturating_sub(cached),
                output: n("output_tokens"),
                cache_read: cached,
                cache_write: n("cache_write_input_tokens"),
                reasoning: n("reasoning_output_tokens"),
                cost_usd: None,
            })
        }
        _ => None,
    }
}

// ── Pi ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct PiLine<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow)]
    timestamp: Option<Cow<'a, str>>,
    message: Option<PiMessage<'a>>,
}

#[derive(Deserialize)]
struct PiMessage<'a> {
    #[serde(borrow)]
    role: Option<Cow<'a, str>>,
    #[serde(borrow)]
    model: Option<Cow<'a, str>>,
    #[serde(rename = "responseId", borrow)]
    response_id: Option<Cow<'a, str>>,
    timestamp: Option<serde_json::Value>,
    usage: Option<PiUsage>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct PiUsage {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
    cost: Option<PiCost>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PiCost {
    total: Option<f64>,
}

fn parse_pi(line: &[u8], session: u64, horizon: &str) -> Option<Record> {
    let parsed: PiLine = serde_json::from_slice(line).ok()?;
    if parsed.kind.as_deref() != Some("message") {
        return None;
    }
    let message = parsed.message?;
    if message.role.as_deref() != Some("assistant") {
        return None;
    }
    let usage = message.usage?;
    // Pi stamps epoch millis on the message, RFC 3339 on the line.
    let day = match (&message.timestamp, parsed.timestamp.as_deref()) {
        (Some(serde_json::Value::Number(ms)), _) => {
            let ms = ms.as_i64()?;
            let at = chrono::DateTime::from_timestamp_millis(ms)?;
            let day = at.format("%Y-%m-%d").to_string();
            (day.as_str() >= horizon).then_some(day)?
        }
        (_, Some(ts)) => day_of(ts, horizon)?,
        _ => return None,
    };
    Some(Record {
        day,
        source: "pi",
        model: message
            .model
            .as_deref()
            .and_then(model_token)
            .unwrap_or_else(|| "unknown".into()),
        origin: "unknown".into(),
        session,
        dedupe: message
            .response_id
            .as_deref()
            .map(|id| hash64(&["pi", id])),
        input: usage.input,
        output: usage.output,
        cache_read: usage.cache_read,
        cache_write: usage.cache_write,
        reasoning: usage.reasoning,
        cost_usd: usage.cost.and_then(|c| c.total),
    })
}

// ── OpenCode (SQLite) ───────────────────────────────────────────────────

/// Completed assistant messages updated after `after_ms`. Returns the
/// records and the new cursor. Read-only; OpenCode may hold the DB open.
pub(super) fn scan_opencode(db: &Path, after_ms: i64, horizon: &str) -> (Vec<Record>, i64) {
    let mut out = Vec::new();
    let mut cursor = after_ms;
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return (out, cursor);
    };
    let _ = conn.busy_timeout(std::time::Duration::from_millis(500));
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, session_id, time_updated, data FROM message \
         WHERE time_updated > ?1 ORDER BY time_updated",
    ) else {
        return (out, cursor);
    };
    let Ok(rows) = stmt.query_map([after_ms], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    }) else {
        return (out, cursor);
    };
    for (id, session, updated, data) in rows.flatten() {
        let Ok(data) = serde_json::from_str::<serde_json::Value>(&data) else {
            continue;
        };
        if data.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        // In-flight messages are rewritten on completion; count them then.
        let Some(completed) = data
            .get("time")
            .and_then(|t| t.get("completed"))
            .and_then(|v| v.as_i64())
        else {
            continue;
        };
        cursor = cursor.max(updated);
        let Some(day) = chrono::DateTime::from_timestamp_millis(completed)
            .map(|at| at.format("%Y-%m-%d").to_string())
            .filter(|day| day.as_str() >= horizon)
        else {
            continue;
        };
        let tokens = data.get("tokens");
        let n = |path: &[&str]| {
            let mut v = tokens;
            for key in path {
                v = v.and_then(|x| x.get(*key));
            }
            v.and_then(|x| x.as_u64()).unwrap_or(0)
        };
        out.push(Record {
            day,
            source: "opencode",
            model: data
                .get("modelID")
                .and_then(|v| v.as_str())
                .and_then(model_token)
                .unwrap_or_else(|| "unknown".into()),
            origin: "unknown".into(),
            session: hash64(&[&session]),
            dedupe: Some(hash64(&["opencode", &id])),
            input: n(&["input"]),
            output: n(&["output"]),
            cache_read: n(&["cache", "read"]),
            cache_write: n(&["cache", "write"]),
            reasoning: n(&["reasoning"]),
            cost_usd: data.get("cost").and_then(|v| v.as_f64()),
        });
    }
    (out, cursor)
}

/// Runs `scan_file` over `jobs` on up to `threads` worker threads.
pub(super) fn scan_parallel(
    jobs: Vec<(PathBuf, Kind, FileCursor)>,
    horizon: &str,
    threads: usize,
) -> Vec<FileScan> {
    if jobs.is_empty() {
        return Vec::new();
    }
    let threads = threads.clamp(1, jobs.len());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results = std::sync::Mutex::new(Vec::with_capacity(jobs.len()));
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((path, kind, cursor)) = jobs.get(i) else {
                        return;
                    };
                    if let Some(scan) = scan_file(path, *kind, cursor.clone(), horizon)
                        && let Ok(mut results) = results.lock()
                    {
                        results.push((i, scan));
                    }
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap_or_default();
    // Deterministic merge order (dedupe keeps the first occurrence).
    results.sort_by_key(|(i, _)| *i);
    results.into_iter().map(|(_, scan)| scan).collect()
}

pub(super) fn aggregate_key(r: &Record) -> String {
    format!("{}|{}|{}|{}", r.day, r.source, r.model, r.origin)
}
