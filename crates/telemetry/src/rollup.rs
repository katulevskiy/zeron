//! In-process aggregation for high-frequency signals (every RPC call, every
//! relay frame). Recording is a short critical section with no allocation
//! on the hot path (keys are looked up by `&str`); the aggregate leaves the
//! process as one `rollup` event per metric per window instead of an event
//! per call.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::event::{Event, names};

#[derive(Debug, Default, Clone, Copy)]
struct Stat {
    count: u64,
    errors: u64,
    total_us: u64,
    max_us: u64,
}

pub(crate) struct Rollup {
    state: Mutex<State>,
}

struct State {
    window_start: Instant,
    metrics: HashMap<&'static str, HashMap<Box<str>, Stat>>,
}

/// Per-metric key cap: an unbounded key space (a bug, or a client spraying
/// method names) folds into `other` instead of growing without limit.
const MAX_KEYS: usize = 256;

impl Rollup {
    pub fn new() -> Self {
        Rollup {
            state: Mutex::new(State {
                window_start: Instant::now(),
                metrics: HashMap::new(),
            }),
        }
    }

    pub fn record(&self, metric: &'static str, key: &str, elapsed: Option<Duration>, ok: bool) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let keys = state.metrics.entry(metric).or_default();
        let stat = match keys.get_mut(key) {
            Some(stat) => stat,
            None => {
                let key = if keys.len() >= MAX_KEYS { "other" } else { key };
                keys.entry(key.into()).or_default()
            }
        };
        stat.count += 1;
        if !ok {
            stat.errors += 1;
        }
        if let Some(elapsed) = elapsed {
            let us = elapsed.as_micros().min(u64::MAX as u128) as u64;
            stat.total_us = stat.total_us.saturating_add(us);
            stat.max_us = stat.max_us.max(us);
        }
    }

    /// Takes the current window as events and starts a new one.
    pub fn drain(&self) -> Vec<Event> {
        let Ok(mut state) = self.state.lock() else {
            return Vec::new();
        };
        let window_secs = state.window_start.elapsed().as_secs();
        state.window_start = Instant::now();
        let metrics = std::mem::take(&mut state.metrics);
        drop(state);

        let mut out = Vec::with_capacity(metrics.len());
        let mut metrics: Vec<_> = metrics.into_iter().collect();
        metrics.sort_by_key(|(metric, _)| *metric);
        for (metric, keys) in metrics {
            let mut calls = Map::new();
            let mut errors = Map::new();
            let mut avg_ms = Map::new();
            let mut max_ms = Map::new();
            let mut total = 0u64;
            let mut total_errors = 0u64;
            for (key, stat) in keys {
                total += stat.count;
                total_errors += stat.errors;
                calls.insert(key.to_string(), stat.count.into());
                if stat.errors > 0 {
                    errors.insert(key.to_string(), stat.errors.into());
                }
                if stat.total_us > 0 {
                    let avg = stat.total_us as f64 / stat.count as f64 / 1000.0;
                    avg_ms.insert(key.to_string(), round2(avg).into());
                    max_ms.insert(key.to_string(), round2(stat.max_us as f64 / 1000.0).into());
                }
            }
            let mut ev = Event::new(names::ROLLUP)
                .with("metric", metric)
                .with("window_secs", window_secs)
                .with("total", total)
                .with("total_errors", total_errors)
                .with("calls", Value::Object(calls));
            if !errors.is_empty() {
                ev = ev.with("errors", Value::Object(errors));
            }
            if !avg_ms.is_empty() {
                ev = ev
                    .with("avg_ms", Value::Object(avg_ms))
                    .with("max_ms", Value::Object(max_ms));
            }
            out.push(ev);
        }
        out
    }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_per_metric_and_key() {
        let r = Rollup::new();
        r.record("rpc", "ListChats", Some(Duration::from_millis(2)), true);
        r.record("rpc", "ListChats", Some(Duration::from_millis(4)), false);
        r.record("rpc", "QueueMessage", None, true);
        r.record("relay", "inbound", None, true);
        let events = r.drain();
        assert_eq!(events.len(), 2);
        let rpc = events
            .iter()
            .find(|e| e.props()["metric"] == "rpc")
            .unwrap();
        assert_eq!(rpc.props()["total"], 3);
        assert_eq!(rpc.props()["calls"]["ListChats"], 2);
        assert_eq!(rpc.props()["errors"]["ListChats"], 1);
        assert_eq!(rpc.props()["avg_ms"]["ListChats"], 3.0);
        assert_eq!(rpc.props()["max_ms"]["ListChats"], 4.0);
        assert!(r.drain().is_empty(), "drain resets the window");
    }

    #[test]
    fn key_space_is_bounded() {
        let r = Rollup::new();
        for i in 0..(MAX_KEYS + 50) {
            r.record("rpc", &format!("m{i}"), None, true);
        }
        let ev = r.drain().pop().unwrap();
        let calls = ev.props()["calls"].as_object().unwrap();
        assert_eq!(calls.len(), MAX_KEYS + 1);
        assert_eq!(calls["other"], 50);
    }
}
