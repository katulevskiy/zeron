//! Moving a chat to another device (docs/plans/2026-09-30-agent-mobility-and-
//! policy.md, Part 1): the pure presentation layer behind the desktop's
//! "Move to…" entry points, the device picker and the move banner above the
//! composer. Rendering and the RPCs live in `shell/session_move.rs`;
//! everything here is data in, strings/flags out.
//!
//! The banner renders from the chat row's `move` field, which every device
//! already syncs, so there is no feed to watch.

use zeron_proto::{Chat, ChatMove, Device, MoveCandidate, MovePhase, MoveWhen};

use crate::file_transfers::format_bytes;

/// A finished move's "Moved to …" line stays up this long.
pub const DONE_LINGER_MS: i64 = 6_000;

/// A cancelled move's line stays up this long.
pub const CANCELLED_LINGER_MS: i64 = 4_000;

/// Failures and post-move notes wait for a dismissal, but not forever: the
/// dismissal is device-local and in-memory, so a restart would otherwise bring
/// back every old failure.
pub const STALE_MS: i64 = 24 * 60 * 60 * 1000;

/// Whether "Move to…" is offered for `chat`: top-level chats only (side chats
/// and agent-spawned workers follow their parent), hosted by an engine that
/// can move chats, and not while a move is already running.
pub fn can_offer_move(chat: &Chat, host_supports: bool) -> bool {
    chat.parent_chat_id.is_none()
        && host_supports
        && !chat.move_state.as_ref().is_some_and(ChatMove::is_live)
}

/// The move a chat is in the middle of, if any.
pub fn live_move(chat: &Chat) -> Option<&ChatMove> {
    chat.move_state.as_ref().filter(|m| m.is_live())
}

/// The sidebar row's tooltip while its chat is moving.
pub fn moving_label(m: &ChatMove) -> String {
    format!("Moving to {}\u{2026}", m.to_device_name)
}

// ---- Banner ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerTone {
    /// The move is running.
    Progress,
    Success,
    Failure,
    /// Cancelled: nothing happened.
    Quiet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerAction {
    /// Skip the wait for a safe point.
    MoveNow,
    Cancel,
    /// Hide a finished move's banner on this device.
    Dismiss,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BannerProgress {
    pub fraction: f32,
    /// `214 MB of 530 MB · 40%`.
    pub label: String,
}

/// What the banner above the composer shows for one move.
#[derive(Debug, Clone, PartialEq)]
pub struct Banner {
    pub move_id: String,
    pub tone: BannerTone,
    pub title: String,
    pub subtitle: Option<String>,
    pub progress: Option<BannerProgress>,
    /// Things to know after the move (a branch landed under another name).
    pub notes: Vec<String>,
    pub actions: Vec<BannerAction>,
    /// When a lingering line hides itself (ms since the epoch), so the view
    /// can schedule a repaint.
    pub expires_at: Option<i64>,
}

pub fn action_label(action: BannerAction) -> &'static str {
    match action {
        BannerAction::MoveNow => "Move now",
        BannerAction::Cancel => "Cancel",
        BannerAction::Dismiss => "Dismiss",
    }
}

/// The phase's headline.
pub fn phase_title(m: &ChatMove) -> String {
    let to = &m.to_device_name;
    match m.phase {
        MovePhase::Preparing => format!("Preparing to move to {to}"),
        MovePhase::Copying => format!("Copying to {to}"),
        MovePhase::Waiting => "Waiting for the current step".into(),
        MovePhase::Finishing => "Finishing".into(),
        MovePhase::Handover => format!("{to} is taking over"),
        MovePhase::Done => format!("Moved to {to}"),
        MovePhase::Failed => format!("Couldn't move to {to}"),
        MovePhase::Cancelled => "Move cancelled".into(),
    }
}

/// Bytes moved so far, while there is a known amount to move.
pub fn progress(m: &ChatMove) -> Option<BannerProgress> {
    if !matches!(m.phase, MovePhase::Copying | MovePhase::Finishing) || m.bytes_total == 0 {
        return None;
    }
    let done = m.bytes_done.min(m.bytes_total);
    let fraction = (done as f64 / m.bytes_total as f64) as f32;
    Some(BannerProgress {
        fraction,
        label: format!(
            "{} of {} \u{b7} {}%",
            format_bytes(done),
            format_bytes(m.bytes_total),
            (fraction * 100.0).floor() as u32
        ),
    })
}

/// The controls a live move offers. **Move now** only matters while the
/// source still waits for a safe point; **Cancel** works until the target
/// starts taking over.
pub fn live_actions(m: &ChatMove) -> Vec<BannerAction> {
    let mut actions = Vec::new();
    if m.when == MoveWhen::SafePoint
        && matches!(
            m.phase,
            MovePhase::Preparing | MovePhase::Copying | MovePhase::Waiting
        )
    {
        actions.push(BannerAction::MoveNow);
    }
    if matches!(
        m.phase,
        MovePhase::Preparing | MovePhase::Copying | MovePhase::Waiting | MovePhase::Finishing
    ) {
        actions.push(BannerAction::Cancel);
    }
    actions
}

fn non_empty(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// The banner for a chat's move at `now_ms`; `None` hides it. `dismissed`
/// is this device's "Dismiss" for this move id.
pub fn banner(m: &ChatMove, now_ms: i64, dismissed: bool) -> Option<Banner> {
    let detail = non_empty(m.detail.as_deref());
    let base = |tone, subtitle, actions, expires_at| Banner {
        move_id: m.id.clone(),
        tone,
        title: phase_title(m),
        subtitle,
        progress: progress(m),
        notes: Vec::new(),
        actions,
        expires_at,
    };
    if m.is_live() {
        let subtitle = detail.or_else(|| {
            (m.phase == MovePhase::Waiting)
                .then(|| "The agent keeps working until this step ends".to_string())
        });
        return Some(base(BannerTone::Progress, subtitle, live_actions(m), None));
    }
    if dismissed {
        return None;
    }
    // Engines stamp `finished_at` when a move ends; an older row without it
    // ages from its start instead.
    let finished = m.finished_at.unwrap_or(m.started_at);
    let age = now_ms - finished;
    if age > STALE_MS {
        return None;
    }
    let dismiss = vec![BannerAction::Dismiss];
    match m.phase {
        MovePhase::Done => {
            let notes: Vec<String> = m
                .notes
                .iter()
                .filter_map(|note| non_empty(Some(note)))
                .collect();
            // Notes outlive the success line: they're the only place the user
            // learns what didn't come along.
            if notes.is_empty() && age > DONE_LINGER_MS {
                return None;
            }
            let expires_at = notes.is_empty().then_some(finished + DONE_LINGER_MS);
            let mut banner = base(BannerTone::Success, None, dismiss, expires_at);
            banner.notes = notes;
            Some(banner)
        }
        MovePhase::Cancelled => (age <= CANCELLED_LINGER_MS).then(|| {
            base(
                BannerTone::Quiet,
                detail,
                dismiss,
                Some(finished + CANCELLED_LINGER_MS),
            )
        }),
        // Failed (the only other terminal phase): stays until dismissed.
        _ => {
            let error = non_empty(m.error.as_deref())
                .or(detail)
                .unwrap_or_else(|| "Something went wrong. The chat stayed where it was.".into());
            Some(base(BannerTone::Failure, Some(error), dismiss, None))
        }
    }
}

// ---- Device picker ----

/// One row of the "Move to…" picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerRow {
    pub device_id: String,
    pub name: String,
    /// From the device's registry row (`""` when unknown).
    pub platform: String,
    pub online: bool,
    /// The device can take the chat: clicking moves it.
    pub selectable: bool,
    /// Why it can't, or which agent login it will use.
    pub secondary: Option<String>,
}

/// Turn `MoveCandidates` into picker rows. The engine's own `problem` wins;
/// the flags fill in when it gave none. Viewer-only devices (phones, the web
/// app) can never host a chat and are dropped rather than listed as "needs a
/// newer Zeron". Movable devices first, then by name.
pub fn picker_rows(
    candidates: &[MoveCandidate],
    devices: &[Device],
    harness: &str,
) -> Vec<PickerRow> {
    let mut rows: Vec<PickerRow> = candidates
        .iter()
        .filter_map(|candidate| {
            let device = devices.iter().find(|d| d.id == candidate.device_id);
            if device.is_some_and(|d| !crate::file_transfers::runs_an_engine(d)) {
                return None;
            }
            let problem = non_empty(candidate.problem.as_deref()).or_else(|| {
                if !candidate.supported {
                    Some("Needs a newer Zeron".to_string())
                } else if !candidate.online {
                    Some("Offline".to_string())
                } else if !candidate.harness_installed {
                    Some(format!("{harness} isn't installed there"))
                } else {
                    None
                }
            });
            let selectable = problem.is_none();
            let secondary = problem.or_else(|| {
                candidate.harness_signed_in.map(|signed_in| {
                    if signed_in {
                        format!("Uses its own {harness} login")
                    } else {
                        format!("Your {harness} login will be copied there")
                    }
                })
            });
            Some(PickerRow {
                device_id: candidate.device_id.clone(),
                name: candidate.device_name.clone(),
                platform: device.map(|d| d.platform.clone()).unwrap_or_default(),
                online: candidate.online,
                selectable,
                secondary,
            })
        })
        .collect();
    rows.sort_by_key(|row| {
        (
            !row.selectable,
            row.name.to_lowercase(),
            row.device_id.clone(),
        )
    });
    rows
}

/// Parse a `MoveCandidates` reply: the engine answers with the bare array;
/// a `{candidates: […]}` wrapper is tolerated.
pub fn parse_candidates(reply: serde_json::Value) -> Result<Vec<MoveCandidate>, String> {
    let list = match reply {
        serde_json::Value::Object(mut map) if map.contains_key("candidates") => {
            map.remove("candidates").unwrap_or_default()
        }
        other => other,
    };
    serde_json::from_value(list).map_err(|error| format!("Unexpected reply: {error}"))
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub fn chat_move(phase: MovePhase) -> ChatMove {
        ChatMove {
            id: "mv-1".into(),
            from_device_id: "laptop".into(),
            to_device_id: "desk".into(),
            to_device_name: "Desktop".into(),
            phase,
            when: MoveWhen::SafePoint,
            detail: None,
            bytes_total: 0,
            bytes_done: 0,
            files_total: 0,
            started_at: 1_000_000,
            finished_at: phase.is_terminal().then_some(2_000_000),
            error: None,
            notes: Vec::new(),
        }
    }

    pub fn candidate(id: &str, name: &str) -> MoveCandidate {
        MoveCandidate {
            device_id: id.into(),
            device_name: name.into(),
            online: true,
            supported: true,
            harness_installed: true,
            harness_signed_in: None,
            problem: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{candidate, chat_move};
    use super::*;

    fn chat(json: serde_json::Value) -> Chat {
        let mut base = serde_json::json!({
            "id": "c", "deviceId": "laptop", "archived": false,
            "createdAt": "2026-09-30T00:00:00Z",
        });
        base.as_object_mut()
            .unwrap()
            .extend(json.as_object().unwrap().clone());
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn move_is_offered_for_top_level_chats_on_capable_hosts_when_idle() {
        let top = chat(serde_json::json!({}));
        assert!(can_offer_move(&top, true));
        assert!(!can_offer_move(&top, false));
        let side = chat(serde_json::json!({ "parentChatId": "p" }));
        assert!(!can_offer_move(&side, true));

        let mut moving = top.clone();
        moving.move_state = Some(chat_move(MovePhase::Copying));
        assert!(!can_offer_move(&moving, true));
        assert_eq!(live_move(&moving).map(|m| m.id.as_str()), Some("mv-1"));
        assert_eq!(
            moving_label(live_move(&moving).unwrap()),
            "Moving to Desktop\u{2026}"
        );
        // A finished move doesn't block the next one.
        for phase in [MovePhase::Done, MovePhase::Failed, MovePhase::Cancelled] {
            moving.move_state = Some(chat_move(phase));
            assert!(can_offer_move(&moving, true), "{phase:?}");
            assert!(live_move(&moving).is_none());
        }
    }

    #[test]
    fn live_banners_name_the_phase_and_offer_the_right_controls() {
        use BannerAction::*;
        let cases = [
            (
                MovePhase::Preparing,
                "Preparing to move to Desktop",
                vec![MoveNow, Cancel],
            ),
            (
                MovePhase::Copying,
                "Copying to Desktop",
                vec![MoveNow, Cancel],
            ),
            (
                MovePhase::Waiting,
                "Waiting for the current step",
                vec![MoveNow, Cancel],
            ),
            (MovePhase::Finishing, "Finishing", vec![Cancel]),
            (MovePhase::Handover, "Desktop is taking over", vec![]),
        ];
        for (phase, title, actions) in cases {
            let m = chat_move(phase);
            let banner = banner(&m, 0, false).unwrap();
            assert_eq!(banner.title, title);
            assert_eq!(banner.tone, BannerTone::Progress);
            assert_eq!(banner.actions, actions, "{phase:?}");
            assert_eq!(banner.expires_at, None);
            // Dismissal only applies to finished moves.
            assert!(super::banner(&m, 0, true).is_some());
        }
        // "Move now" was already chosen: only Cancel remains.
        let mut now = chat_move(MovePhase::Waiting);
        now.when = MoveWhen::Now;
        assert_eq!(banner(&now, 0, false).unwrap().actions, [Cancel]);
    }

    #[test]
    fn subtitle_is_the_engine_detail_with_a_default_while_waiting() {
        let mut m = chat_move(MovePhase::Waiting);
        assert_eq!(
            banner(&m, 0, false).unwrap().subtitle.as_deref(),
            Some("The agent keeps working until this step ends")
        );
        m.detail = Some("Bash: cargo build, 4m".into());
        assert_eq!(
            banner(&m, 0, false).unwrap().subtitle.as_deref(),
            Some("Bash: cargo build, 4m")
        );
        m.phase = MovePhase::Preparing;
        m.detail = Some("  ".into());
        assert_eq!(banner(&m, 0, false).unwrap().subtitle, None);
    }

    #[test]
    fn progress_shows_while_bytes_move() {
        let mut m = chat_move(MovePhase::Copying);
        assert_eq!(progress(&m), None, "nothing known to move yet");
        m.bytes_total = 530_000_000;
        m.bytes_done = 214_000_000;
        let p = progress(&m).unwrap();
        assert!((p.fraction - 0.4037).abs() < 0.001);
        assert_eq!(p.label, "214 MB of 530 MB \u{b7} 40%");
        m.phase = MovePhase::Finishing;
        m.bytes_done = 600_000_000;
        assert_eq!(progress(&m).unwrap().fraction, 1.0, "clamped");
        m.phase = MovePhase::Waiting;
        assert_eq!(progress(&m), None);
    }

    #[test]
    fn done_lingers_briefly_unless_it_has_notes() {
        let mut m = chat_move(MovePhase::Done);
        let finished = m.finished_at.unwrap();
        let b = banner(&m, finished + 1_000, false).unwrap();
        assert_eq!(b.title, "Moved to Desktop");
        assert_eq!(b.tone, BannerTone::Success);
        assert_eq!(b.actions, [BannerAction::Dismiss]);
        assert_eq!(b.expires_at, Some(finished + DONE_LINGER_MS));
        assert_eq!(banner(&m, finished + DONE_LINGER_MS + 1, false), None);

        m.notes = vec!["Branch landed as main-2".into(), " ".into()];
        let b = banner(&m, finished + 60_000, false).unwrap();
        assert_eq!(b.notes, ["Branch landed as main-2"]);
        assert_eq!(b.expires_at, None);
        assert_eq!(banner(&m, finished + 60_000, true), None, "dismissed");
        assert_eq!(banner(&m, finished + STALE_MS + 1, false), None, "stale");

        // No finish stamp: ages from the start.
        m.notes.clear();
        m.finished_at = None;
        assert!(banner(&m, m.started_at + 1_000, false).is_some());
    }

    #[test]
    fn failures_stay_until_dismissed_and_cancels_pass_quickly() {
        let mut m = chat_move(MovePhase::Failed);
        let finished = m.finished_at.unwrap();
        let b = banner(&m, finished + 3_600_000, false).unwrap();
        assert_eq!(b.title, "Couldn't move to Desktop");
        assert_eq!(b.tone, BannerTone::Failure);
        assert_eq!(
            b.subtitle.as_deref(),
            Some("Something went wrong. The chat stayed where it was.")
        );
        m.error = Some("Desktop ran out of disk space".into());
        assert_eq!(
            banner(&m, finished, false).unwrap().subtitle.as_deref(),
            Some("Desktop ran out of disk space")
        );
        assert_eq!(banner(&m, finished, true), None);

        let c = chat_move(MovePhase::Cancelled);
        let b = banner(&c, finished + 1_000, false).unwrap();
        assert_eq!(b.title, "Move cancelled");
        assert_eq!(b.tone, BannerTone::Quiet);
        assert_eq!(b.expires_at, Some(finished + CANCELLED_LINGER_MS));
        assert_eq!(banner(&c, finished + CANCELLED_LINGER_MS + 1, false), None);
    }

    #[test]
    fn picker_rows_explain_problems_and_logins() {
        let device = |id: &str, platform: &str, caps: &[&str]| -> Device {
            serde_json::from_value(serde_json::json!({
                "id": id, "name": id, "platform": platform,
                "lastSeenAt": null, "capabilities": caps,
            }))
            .unwrap()
        };
        let mv = zeron_proto::capabilities::SESSION_MOVE_V1;
        let devices = vec![
            device("desk", "linux", &[mv]),
            device("mac", "macos", &[mv]),
            device("phone", "ios", &[]),
            device("box", "linux", &[mv]),
        ];
        let mut signed_in = candidate("desk", "Desktop");
        signed_in.harness_signed_in = Some(true);
        let mut copied = candidate("mac", "MacBook");
        copied.harness_signed_in = Some(false);
        let mut offline = candidate("box", "Build box");
        offline.online = false;
        offline.problem = Some("Offline".into());
        let mut missing = candidate("old", "Old laptop");
        missing.harness_installed = false;
        let mut phone = candidate("phone", "iPhone");
        phone.supported = false;
        let unknown_login = candidate("zz", "Attic");

        let rows = picker_rows(
            &[offline, phone, missing, copied, signed_in, unknown_login],
            &devices,
            "Codex",
        );
        let summary: Vec<_> = rows
            .iter()
            .map(|r| (r.name.as_str(), r.selectable, r.secondary.as_deref()))
            .collect();
        assert_eq!(
            summary,
            [
                ("Attic", true, None),
                ("Desktop", true, Some("Uses its own Codex login")),
                (
                    "MacBook",
                    true,
                    Some("Your Codex login will be copied there")
                ),
                ("Build box", false, Some("Offline")),
                ("Old laptop", false, Some("Codex isn't installed there")),
            ]
        );
        assert_eq!(rows[2].platform, "macos");
        assert_eq!(rows[0].platform, "", "no registry row");

        let mut old = candidate("x", "X");
        old.supported = false;
        old.online = false;
        assert_eq!(
            picker_rows(&[old], &[], "Codex")[0].secondary.as_deref(),
            Some("Needs a newer Zeron")
        );
    }

    #[test]
    fn candidates_parse_bare_or_wrapped() {
        let list = serde_json::json!([{
            "deviceId": "desk", "deviceName": "Desktop", "online": true,
            "supported": true, "harnessInstalled": true, "harnessSignedIn": false,
        }]);
        let parsed = parse_candidates(list.clone()).unwrap();
        assert_eq!(parsed[0].harness_signed_in, Some(false));
        assert_eq!(
            parse_candidates(serde_json::json!({ "candidates": list })).unwrap(),
            parsed
        );
        assert!(parse_candidates(serde_json::json!({ "nope": 1 })).is_err());
    }
}
