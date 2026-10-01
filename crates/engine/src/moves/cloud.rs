//! Moving to a cloud box that may be asleep (docs/cloud.md §8): the picker
//! lists boxes from their records, and a move to one that isn't present
//! wakes it and waits for it before it starts.

use std::time::Duration;

use tokio::sync::watch;
use zeron_proto::{ChatMove, CloudBox, MoveCandidate, MovePhase, capabilities};

use super::protocol::{ProbeParams, ProbeReply};
use super::service::{Control, MoveService};

pub(super) const WAKE_DETAIL: &str = "Waking the cloud box";
pub(super) const ASLEEP_NOTE: &str = "Asleep — wakes when you move";
/// How long a move waits for a woken box to show up.
const WAKE_TIMEOUT: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_secs(2);

/// List boxes that have no device row yet, or whose device is offline, as
/// asleep candidates. A box that answered its probe stays as it is.
pub(super) fn add_asleep_boxes(boxes: &[CloudBox], own_device: &str, out: &mut Vec<MoveCandidate>) {
    for record in boxes.iter().filter(|b| b.id != own_device) {
        let asleep = asleep_candidate(record);
        match out.iter_mut().find(|c| c.device_id == record.id) {
            Some(candidate) if candidate.online || !candidate.supported => {}
            Some(candidate) => *candidate = asleep,
            None => out.push(asleep),
        }
    }
}

fn asleep_candidate(record: &CloudBox) -> MoveCandidate {
    MoveCandidate {
        device_id: record.id.clone(),
        device_name: record.name.clone(),
        online: false,
        supported: true,
        // The box image ships the agent CLIs; its own probe answers once awake.
        harness_installed: true,
        harness_signed_in: None,
        problem: None,
        asleep: true,
        note: Some(ASLEEP_NOTE.into()),
    }
}

/// The box's engine is connected: its device row exists and its presence
/// is fresh.
pub(super) fn present(service: &MoveService, device_id: &str) -> bool {
    let inner = &service.0;
    row_ready(service, device_id)
        && inner.workspace.peer_liveness(device_id) == zeron_rpc::PeerLiveness::Live
}

fn row_ready(service: &MoveService, device_id: &str) -> bool {
    service
        .0
        .workspace
        .read_devices()
        .ok()
        .into_iter()
        .flatten()
        .any(|d| d.id == device_id)
}

/// Wake `record` and wait for its engine. `false` when the move ended here
/// (failed or cancelled; the row says which).
pub(super) async fn wake_for_move(
    service: &MoveService,
    chat_id: &str,
    state: &mut ChatMove,
    record: &CloudBox,
    control: &watch::Receiver<Control>,
) -> bool {
    state.detail = Some(WAKE_DETAIL.into());
    service.publish(chat_id, state);
    let outcome = wait_until_awake(service, record, control).await;
    match outcome {
        Ok(()) => {
            state.detail = None;
            service.publish(chat_id, state);
            true
        }
        Err(ending) => {
            state.detail = None;
            state.finished_at = Some(chrono::Utc::now().timestamp_millis());
            match ending {
                Ending::Cancelled => state.phase = MovePhase::Cancelled,
                Ending::Failed(message) => {
                    state.phase = MovePhase::Failed;
                    state.error = Some(message);
                }
            }
            service.publish(chat_id, state);
            false
        }
    }
}

enum Ending {
    Cancelled,
    Failed(String),
}

async fn wait_until_awake(
    service: &MoveService,
    record: &CloudBox,
    control: &watch::Receiver<Control>,
) -> Result<(), Ending> {
    let inner = &service.0;
    crate::cloud_boxes::wake_record(&inner.workspace, &inner.cloud, record)
        .await
        .map_err(|e| Ending::Failed(format!("Couldn't wake {}: {e}", record.name)))?;
    let deadline = tokio::time::Instant::now() + WAKE_TIMEOUT;
    loop {
        if *control.borrow() == Control::Cancel {
            return Err(Ending::Cancelled);
        }
        if row_ready(service, &record.id) && answers(service, record).await? {
            let _ = inner.workspace.set_cloud_box_state(
                &record.id,
                zeron_proto::CloudBoxState::Running,
                None,
            );
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Ending::Failed(format!(
                "{} didn't wake up within 2 minutes; try again in a moment",
                record.name
            )));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// The box's engine answers a move probe (and can take chats).
async fn answers(service: &MoveService, record: &CloudBox) -> Result<bool, Ending> {
    let inner = &service.0;
    let Some(device) = inner
        .workspace
        .read_devices()
        .ok()
        .and_then(|devices| devices.into_iter().find(|d| d.id == record.id))
    else {
        return Ok(false);
    };
    if inner.workspace.peer_liveness(&record.id) == zeron_rpc::PeerLiveness::Dark {
        return Ok(false);
    }
    let probe = tokio::time::timeout(
        Duration::from_secs(4),
        service.call::<ProbeReply>(
            &record.id,
            zeron_rpc::methods::MOVE_PROBE,
            &ProbeParams {
                harness: zeron_proto::HarnessId::ClaudeCode,
            },
        ),
    )
    .await;
    if !matches!(probe, Ok(Ok(_))) {
        return Ok(false);
    }
    if !device.supports(capabilities::SESSION_MOVE_V1) {
        return Err(Ending::Failed(format!(
            "{} runs a version of Zeron that can't take chats yet",
            record.name
        )));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::CloudBoxState;

    fn record(id: &str, name: &str) -> CloudBox {
        CloudBox {
            id: id.into(),
            provider: "cloudflare".into(),
            name: name.into(),
            account_id: "acc".into(),
            worker_url: "https://zeron-cloud.sub.workers.dev".into(),
            wake_key: "a2V5".into(),
            instance_type: "standard-2".into(),
            idle_minutes: 15,
            created_at: 0,
            state: CloudBoxState::Asleep,
            state_at: 0,
            error: None,
        }
    }

    fn device(id: &str, online: bool, supported: bool) -> MoveCandidate {
        MoveCandidate {
            device_id: id.into(),
            device_name: id.into(),
            online,
            supported,
            harness_installed: online,
            harness_signed_in: None,
            problem: (!online).then(|| "Offline".into()),
            asleep: false,
            note: None,
        }
    }

    #[test]
    fn boxes_without_a_live_engine_are_offered_asleep() {
        let mut out = vec![
            device("desk", true, true),
            device("box-awake", true, true),
            device("box-off", false, true),
            device("box-old", false, false),
        ];
        let boxes = [
            record("box-awake", "Awake"),
            record("box-off", "Cloud"),
            record("box-old", "Old"),
            record("box-new", "Fresh"),
            record("me", "Myself"),
        ];
        add_asleep_boxes(&boxes, "me", &mut out);
        let view: Vec<_> = out
            .iter()
            .map(|c| {
                (
                    c.device_id.as_str(),
                    c.asleep,
                    c.problem.is_none(),
                    c.note.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            view,
            [
                ("desk", false, true, None),
                ("box-awake", false, true, None),
                ("box-off", true, true, Some(ASLEEP_NOTE)),
                ("box-old", false, false, None),
                ("box-new", true, true, Some(ASLEEP_NOTE)),
            ]
        );
        assert_eq!(out[2].device_name, "Cloud");
    }
}
