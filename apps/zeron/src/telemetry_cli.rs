//! `zeron telemetry …` — inspect and control product telemetry.
//!
//! Talks to the running engine over IPC when there is one (so a change takes
//! effect immediately and `events` shows exactly what is being sent);
//! otherwise edits the persisted settings the next engine start reads.

use std::collections::BTreeMap;
use std::path::Path;

use clap::Subcommand;
use serde_json::{Value, json};
use zeron_rpc::methods;
use zeron_telemetry::TelemetrySettings;
use zeron_telemetry::external::{DailyUsage, Roots, UsageScanner};

#[derive(Subcommand)]
pub enum TelemetryCommand {
    /// Show whether anonymous usage data is collected/uploaded, and why.
    Status,
    /// Turn anonymous usage data on.
    Enable,
    /// Turn anonymous usage data off (also discards anything not yet sent).
    Disable,
    /// Acknowledge the usage-data notice (hides it on headless start and in
    /// the desktop sidebar).
    Ack,
    /// Include daily per-model token totals from other agent CLIs' local logs
    /// (Claude Code, Codex, Pi, OpenCode) — `on` or `off`.
    Import { state: OnOff },
    /// Print the most recent events exactly as they are (or would be) sent.
    Events {
        /// How many, newest last.
        #[arg(short, default_value_t = 20)]
        n: usize,
        /// Full JSON instead of one line per event.
        #[arg(long)]
        json: bool,
    },
    /// Token usage per agent and model over the last N days.
    Usage {
        #[arg(long, default_value_t = 30)]
        days: i64,
        /// Scan the other CLIs' logs right now, locally, without an engine
        /// and without sending anything.
        #[arg(long)]
        scan: bool,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum OnOff {
    On,
    Off,
}

pub async fn run(command: TelemetryCommand, data_dir: &Path, ipc_port: u16) -> anyhow::Result<()> {
    let engine = zeron_rpc::connect_ws(&format!("ws://127.0.0.1:{ipc_port}"))
        .await
        .ok();
    match command {
        TelemetryCommand::Status => status(engine.as_ref(), data_dir).await,
        TelemetryCommand::Enable => set(engine.as_ref(), data_dir, json!({"enabled": true})).await,
        TelemetryCommand::Disable => {
            set(engine.as_ref(), data_dir, json!({"enabled": false})).await
        }
        TelemetryCommand::Ack => {
            set(
                engine.as_ref(),
                data_dir,
                json!({"noticeAcknowledged": true}),
            )
            .await
        }
        TelemetryCommand::Import { state } => {
            let on = matches!(state, OnOff::On);
            set(engine.as_ref(), data_dir, json!({"usageImport": on})).await
        }
        TelemetryCommand::Events { n, json } => {
            let engine = engine.ok_or_else(no_engine(ipc_port))?;
            let status = engine.call(methods::TELEMETRY_STATUS, Value::Null).await?;
            let recent = status["recent"].as_array().cloned().unwrap_or_default();
            if recent.is_empty() {
                println!("no events recorded yet in this engine session");
            }
            for event in recent.iter().skip(recent.len().saturating_sub(n)) {
                if json {
                    println!("{}", serde_json::to_string_pretty(event)?);
                } else {
                    println!("{}", one_line(event));
                }
            }
            Ok(())
        }
        TelemetryCommand::Usage { days, scan } => {
            let rows = if scan {
                local_scan(data_dir)
            } else {
                let engine = engine.ok_or_else(no_engine(ipc_port))?;
                let stats = engine.call(methods::USAGE_STATS, Value::Null).await?;
                let mut rows: Vec<DailyUsage> =
                    serde_json::from_value(stats["zeron"].clone()).unwrap_or_default();
                if stats["importEnabled"] == true {
                    rows.extend(
                        serde_json::from_value::<Vec<DailyUsage>>(stats["external"].clone())
                            .unwrap_or_default(),
                    );
                } else {
                    println!(
                        "(other CLIs' usage not included — `zeron telemetry import on`, or `--scan` for a one-off local scan)\n"
                    );
                }
                rows
            };
            print_usage(&rows, days);
            Ok(())
        }
    }
}

fn no_engine(port: u16) -> impl FnOnce() -> anyhow::Error {
    move || anyhow::anyhow!("no engine listening on 127.0.0.1:{port} — is zeron running?")
}

async fn status(engine: Option<&zeron_rpc::RpcClient>, data_dir: &Path) -> anyhow::Result<()> {
    let Some(engine) = engine else {
        let settings = zeron_telemetry::read_settings(data_dir);
        println!("engine:        not running (showing saved settings)");
        print_settings(&settings);
        return Ok(());
    };
    let status = engine.call(methods::TELEMETRY_STATUS, Value::Null).await?;
    let settings: TelemetrySettings =
        serde_json::from_value(status["settings"].clone()).unwrap_or_default();
    print_settings(&settings);
    println!("collecting:    {}", status["collecting"]);
    println!("uploading:     {}", status["uploading"]);
    if let Some(reason) = status["overrideReason"].as_str() {
        println!("override:      {reason}");
    }
    println!(
        "anonymous id:  {}{}",
        status["distinctId"].as_str().unwrap_or("-"),
        if status["accountLinked"] == true {
            " (hashed account)"
        } else {
            " (hashed install)"
        }
    );
    println!("sent/dropped:  {} / {}", status["sent"], status["dropped"]);
    if let Some(err) = status["lastError"].as_str() {
        println!("last error:    {err}");
    }
    println!("\n`zeron telemetry events` shows exactly what is sent. Docs: docs/telemetry.md");
    Ok(())
}

fn print_settings(settings: &TelemetrySettings) {
    println!(
        "usage data:    {}",
        if settings.enabled { "on" } else { "off" }
    );
    println!(
        "CLI import:    {}",
        if settings.usage_import { "on" } else { "off" }
    );
}

async fn set(
    engine: Option<&zeron_rpc::RpcClient>,
    data_dir: &Path,
    patch: Value,
) -> anyhow::Result<()> {
    match engine {
        Some(engine) => {
            engine.call(methods::SET_TELEMETRY_SETTINGS, patch).await?;
        }
        None => {
            let mut settings = zeron_telemetry::read_settings(data_dir);
            if let Some(v) = patch["enabled"].as_bool() {
                settings.enabled = v;
                if !v {
                    // Nothing recorded before the opt-out may go out after it.
                    let _ = std::fs::remove_file(data_dir.join("telemetry").join("spool.jsonl"));
                }
            }
            if let Some(v) = patch["usageImport"].as_bool() {
                settings.usage_import = v;
            }
            if let Some(v) = patch["noticeAcknowledged"].as_bool() {
                settings.notice_acknowledged = v;
            }
            zeron_telemetry::write_settings(data_dir, &settings)
                .map_err(|e| anyhow::anyhow!("could not save telemetry settings: {e}"))?;
        }
    }
    status(engine, data_dir).await
}

fn one_line(event: &Value) -> String {
    let props = &event["properties"];
    let mut keys: Vec<String> = props
        .as_object()
        .map(|m| {
            m.iter()
                .filter(|(k, _)| !COMMON.contains(&k.as_str()))
                .map(|(k, v)| format!("{k}={}", compact(v)))
                .collect()
        })
        .unwrap_or_default();
    keys.sort();
    format!(
        "{}  {:<24} {}",
        event["timestamp"].as_str().unwrap_or("-"),
        event["event"].as_str().unwrap_or("-"),
        keys.join(" ")
    )
}

/// Stamped on every event; hidden from the one-line view.
const COMMON: &[&str] = &[
    "$lib",
    "$lib_version",
    "$process_person_profile",
    "app_version",
    "os",
    "arch",
    "build",
    "run_mode",
    "workspace_scope",
    "device_hash",
    "install_hash",
    "app_session",
    "account_linked",
];

fn compact(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A one-off scan into a CLI-private state dir (never the engine's, never
/// reported anywhere).
fn local_scan(data_dir: &Path) -> Vec<DailyUsage> {
    let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) else {
        return Vec::new();
    };
    let state = data_dir.join("telemetry").join("external-cli");
    let mut scanner = UsageScanner::new(state, Roots::from_home(Path::new(&home)));
    let started = std::time::Instant::now();
    let report = scanner.scan();
    eprintln!(
        "scanned {} files, {:.1} MB in {:.2}s\n",
        report.files_scanned,
        report.bytes_read as f64 / 1e6,
        started.elapsed().as_secs_f64()
    );
    report.window
}

fn print_usage(rows: &[DailyUsage], days: i64) {
    let horizon = (chrono::Utc::now() - chrono::Duration::days(days))
        .format("%Y-%m-%d")
        .to_string();
    #[derive(Default)]
    struct Sum {
        requests: u64,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        cost: Option<f64>,
    }
    let mut sums: BTreeMap<(String, String, String), Sum> = BTreeMap::new();
    for row in rows.iter().filter(|r| r.day >= horizon) {
        let origin = if row.origin == "zeron" {
            "zeron".to_string()
        } else {
            row.origin.clone()
        };
        let sum = sums
            .entry((row.source.clone(), row.model.clone(), origin))
            .or_default();
        sum.requests += row.requests;
        sum.input += row.input_tokens;
        sum.output += row.output_tokens;
        sum.cache_read += row.cache_read_tokens;
        sum.cache_write += row.cache_write_tokens;
        if let Some(cost) = row.cost_usd {
            *sum.cost.get_or_insert(0.0) += cost;
        }
    }
    if sums.is_empty() {
        println!("no usage in the last {days} days");
        return;
    }
    println!(
        "{:<12} {:<30} {:<14} {:>9} {:>9} {:>9} {:>10} {:>10} {:>9}",
        "agent", "model", "origin", "requests", "input", "output", "cache rd", "cache wr", "cost"
    );
    let mut sums: Vec<_> = sums.into_iter().collect();
    sums.sort_by(|a, b| {
        let total = |s: &Sum| s.input + s.output + s.cache_read + s.cache_write;
        total(&b.1).cmp(&total(&a.1))
    });
    for ((source, model, origin), s) in sums {
        println!(
            "{:<12} {:<30} {:<14} {:>9} {:>9} {:>9} {:>10} {:>10} {:>9}",
            truncate(&source, 12),
            truncate(&model, 30),
            truncate(&origin, 14),
            s.requests,
            human(s.input),
            human(s.output),
            human(s.cache_read),
            human(s.cache_write),
            s.cost
                .map(|c| format!("${c:.2}"))
                .unwrap_or_else(|| "-".into()),
        );
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

fn human(n: u64) -> String {
    match n {
        0..=9_999 => n.to_string(),
        10_000..=999_999 => format!("{:.1}k", n as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{:.2}B", n as f64 / 1e9),
    }
}
