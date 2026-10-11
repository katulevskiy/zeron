//! Benchmarks the external usage scanner against this machine's real logs:
//! `cargo run --release -p zeron-telemetry --example usage_scan`.
//! Scans into a throwaway state dir twice (cold, then incremental) and
//! prints per-source totals. Reads only; sends nothing.

use std::time::Instant;

use zeron_telemetry::external::{Roots, UsageScanner};

fn main() {
    let home = std::env::var_os("HOME").expect("HOME");
    let state = std::env::temp_dir().join(format!("zeron-usage-scan-{}", std::process::id()));
    let mut scanner = UsageScanner::new(state.clone(), Roots::from_home(home.as_ref()));
    for pass in ["cold", "incremental"] {
        let started = Instant::now();
        let report = scanner.scan();
        let secs = started.elapsed().as_secs_f64();
        println!(
            "{pass:>11}: {} files, {:.1} MB in {:.2}s ({:.0} MB/s)",
            report.files_scanned,
            report.bytes_read as f64 / 1e6,
            secs,
            report.bytes_read as f64 / 1e6 / secs.max(1e-9),
        );
        if pass == "cold" {
            let mut by_source = std::collections::BTreeMap::<&str, (u64, u64)>::new();
            for d in &report.window {
                let e = by_source.entry(d.source.as_str()).or_default();
                e.0 += d.requests;
                e.1 +=
                    d.input_tokens + d.output_tokens + d.cache_read_tokens + d.cache_write_tokens;
            }
            for (source, (requests, tokens)) in by_source {
                println!("             {source:<12} {requests:>8} requests {tokens:>15} tokens");
            }
        }
    }
    let _ = std::fs::remove_dir_all(state);
}
