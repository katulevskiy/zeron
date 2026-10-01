//! Sync mode end to end: ticketed offers into a granted staging folder,
//! unchanged files and matching blocks that never travel, tolerance for
//! files that change mid-send, the result record, and resume.

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

use common::*;
use zeron_proto::FileTransferState;
use zeron_transfer::blocks::BLOCK_SIZE;
use zeron_transfer::{
    SyncGrant, SyncOutcome, SyncResult, SyncSend, SyncSource, Transfers, TransportPolicy,
};

const MIB: usize = 1024 * 1024;

fn source(rel: &str, path: &Path) -> SyncSource {
    SyncSource {
        rel: rel.into(),
        source: path.to_path_buf(),
    }
}

fn sync(ticket: &str, files: Vec<SyncSource>, tolerant: bool) -> SyncSend {
    SyncSend {
        to: "phone".into(),
        ticket: ticket.into(),
        files,
        tolerant,
        policy: TransportPolicy::Auto,
    }
}

fn grant(dest_root: &Path, basis: &[(&str, &Path)]) -> SyncGrant {
    SyncGrant {
        dest_root: dest_root.to_path_buf(),
        basis: basis
            .iter()
            .map(|(root, dir)| (root.to_string(), vec![dir.to_path_buf()]))
            .collect::<HashMap<_, _>>(),
    }
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn outcome(result: &SyncResult, rel: &str) -> SyncOutcome {
    result
        .files
        .iter()
        .find(|f| f.rel == rel)
        .unwrap_or_else(|| panic!("{rel} missing from {result:#?}"))
        .outcome
}

async fn completes(pair: &Pair, id: &str) {
    wait_for(&pair.sender, id, state_is(FileTransferState::Completed)).await;
    wait_for(&pair.receiver, id, state_is(FileTransferState::Completed)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ticketed_sync_lands_only_what_changed_in_staging() {
    let pair = pair(0, 0);
    let root = pair._dir.path();
    let ws = root.join("laptop/ws");
    let basis = root.join("phone/ws");
    let staging = root.join("phone/staging/round-1");

    // Unchanged on the receiver (one of them large).
    let same = pattern(6 * MIB + 3, 1);
    write(&ws.join("same.bin"), &same);
    write(&basis.join("same.bin"), &same);
    write(&ws.join("sub/deep.txt"), b"deep");
    write(&basis.join("sub/deep.txt"), b"deep");
    // Changed, new, outside the workspace.
    write(&ws.join("changed.txt"), b"new content");
    write(&basis.join("changed.txt"), b"old content");
    write(&ws.join("new.txt"), b"brand new");
    let shot = root.join("laptop/Desktop/shot.png");
    write(&shot, b"png");
    std::fs::create_dir_all(ws.join("empty")).unwrap();
    // Mode and mtime travel.
    write(&ws.join("run.sh"), b"#!/bin/sh\n");
    let mtime = SystemTime::UNIX_EPOCH + Duration::from_millis(1_700_000_000_123);
    std::fs::File::options()
        .write(true)
        .open(ws.join("run.sh"))
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(ws.join("run.sh"), std::fs::Permissions::from_mode(0o750))
            .unwrap();
        // A link travels as a link.
        std::os::unix::fs::symlink("same.bin", ws.join("link")).unwrap();
        // The basis holds the same bytes, but through a symlink: never
        // followed, so the file is sent.
        write(&ws.join("secret.txt"), b"secret");
        write(&root.join("phone/elsewhere.txt"), b"secret");
        std::os::unix::fs::symlink(root.join("phone/elsewhere.txt"), basis.join("secret.txt"))
            .unwrap();
    }
    pair.receiver_net
        .grant("round-1", grant(&staging, &[("ws", &basis)]));

    let mut files = vec![
        source("ws/same.bin", &ws.join("same.bin")),
        source("ws/sub/deep.txt", &ws.join("sub/deep.txt")),
        source("ws/changed.txt", &ws.join("changed.txt")),
        source("ws/new.txt", &ws.join("new.txt")),
        source("ws/empty", &ws.join("empty")),
        source("ws/run.sh", &ws.join("run.sh")),
        source("x0/Desktop/shot.png", &shot),
    ];
    #[cfg(unix)]
    {
        files.push(source("ws/link", &ws.join("link")));
        files.push(source("ws/secret.txt", &ws.join("secret.txt")));
    }
    let reply = pair
        .sender
        .send_sync(sync("round-1", files, false))
        .await
        .unwrap();
    completes(&pair, &reply.transfer_id).await;
    assert_eq!(pair.receiver_net.grant_calls.load(Ordering::SeqCst), 1);

    // Only the changed bytes travelled: not the 6 MiB unchanged file.
    let sent = pair.sender_net.resumed_bytes.load(Ordering::SeqCst);
    assert!(sent < 256 * 1024, "{sent} bytes on the wire");

    // Unchanged files leave nothing in staging; the rest lands exactly.
    assert!(!staging.join("ws/same.bin").exists());
    assert!(!staging.join("ws/sub/deep.txt").exists());
    assert_eq!(
        std::fs::read(staging.join("ws/changed.txt")).unwrap(),
        b"new content"
    );
    assert_eq!(
        std::fs::read(staging.join("ws/new.txt")).unwrap(),
        b"brand new"
    );
    assert_eq!(
        std::fs::read(staging.join("x0/Desktop/shot.png")).unwrap(),
        b"png"
    );
    assert!(staging.join("ws/empty").is_dir());
    let landed = std::fs::metadata(staging.join("ws/run.sh")).unwrap();
    assert_eq!(landed.modified().unwrap(), mtime);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(landed.permissions().mode() & 0o777, 0o750);
        assert_eq!(
            std::fs::read_link(staging.join("ws/link")).unwrap(),
            Path::new("same.bin")
        );
        assert_eq!(
            std::fs::read(staging.join("ws/secret.txt")).unwrap(),
            b"secret"
        );
    }
    no_parts(&staging);

    let result = Transfers::sync_result(&staging).unwrap();
    assert_eq!(result.transfer_id, reply.transfer_id);
    assert_eq!(result.ticket, "round-1");
    assert_eq!(outcome(&result, "ws/same.bin"), SyncOutcome::Unchanged);
    assert_eq!(outcome(&result, "ws/sub/deep.txt"), SyncOutcome::Unchanged);
    assert_eq!(outcome(&result, "ws/changed.txt"), SyncOutcome::Landed);
    assert_eq!(outcome(&result, "ws/new.txt"), SyncOutcome::Landed);
    assert_eq!(outcome(&result, "x0/Desktop/shot.png"), SyncOutcome::Landed);
    let run = result.files.iter().find(|f| f.rel == "ws/run.sh").unwrap();
    assert_eq!(run.mtime_ms, Some(1_700_000_000_123));
    assert_eq!(run.size, 10);
    #[cfg(unix)]
    {
        assert_eq!(run.mode, 0o750);
        assert_eq!(outcome(&result, "ws/secret.txt"), SyncOutcome::Landed);
        assert_eq!(result.symlinks.len(), 1);
        assert_eq!(result.symlinks[0].rel, "ws/link");
        assert_eq!(result.symlinks[0].target, "same.bin");
    }
    let same_entry = result
        .files
        .iter()
        .find(|f| f.rel == "ws/same.bin")
        .unwrap();
    assert_eq!(
        same_entry.sha256,
        zeron_transfer::manifest::hex(&<sha2::Sha256 as sha2::Digest>::digest(&same))
    );
    let dirs: Vec<&str> = result.dirs.iter().map(|d| d.rel.as_str()).collect();
    for dir in ["ws", "ws/empty", "ws/sub", "x0", "x0/Desktop"] {
        assert!(dirs.contains(&dir), "{dir} missing from {dirs:?}");
    }

    // Progress accounts for every byte, unchanged ones included.
    let row = wait_for(
        &pair.receiver,
        &reply.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    assert_eq!(row.done_bytes, row.total_bytes);
    assert_eq!(row.destination.as_deref(), Some(staging.to_str().unwrap()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_ticket_is_refused() {
    let pair = pair(0, 0);
    let file = pair._dir.path().join("a.txt");
    write(&file, b"a");
    let reply = pair
        .sender
        .send_sync(sync(
            "never-granted",
            vec![source("ws/a.txt", &file)],
            false,
        ))
        .await
        .unwrap();
    let failed = wait_for(
        &pair.sender,
        &reply.transfer_id,
        state_is(FileTransferState::Failed),
    )
    .await;
    assert!(failed.error.unwrap().contains("ticket"));
    assert_eq!(pair.receiver_net.grant_calls.load(Ordering::SeqCst), 1);
    assert!(
        !pair.home.join("Zeron Transfers").exists(),
        "nothing lands in the inbox"
    );
    // Invalid tickets never leave the sender.
    assert!(
        pair.sender
            .send_sync(sync("../x", vec![source("ws/a.txt", &file)], false))
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn block_deltas_send_only_the_blocks_that_differ() {
    let pair = pair(0, 0);
    let root = pair._dir.path();
    let ws = root.join("laptop/ws");
    let basis = root.join("phone/ws");
    let staging = root.join("phone/staging");

    // One changed block in a 10 MiB file.
    let new = pattern(10 * MIB + 5, 2);
    let mut old = new.clone();
    for byte in &mut old[4 * MIB + 100..4 * MIB + 200] {
        *byte ^= 0x5a;
    }
    write(&ws.join("model.bin"), &new);
    write(&basis.join("model.bin"), &old);
    // A file that grew: its old prefix is reused.
    let grown = pattern(8 * MIB, 3);
    write(&ws.join("log.bin"), &grown);
    write(&basis.join("log.bin"), &grown[..5 * MIB + 7]);
    pair.receiver_net
        .grant("t", grant(&staging, &[("ws", &basis)]));

    let reply = pair
        .sender
        .send_sync(sync(
            "t",
            vec![
                source("ws/model.bin", &ws.join("model.bin")),
                source("ws/log.bin", &ws.join("log.bin")),
            ],
            false,
        ))
        .await
        .unwrap();
    completes(&pair, &reply.transfer_id).await;
    assert_eq!(std::fs::read(staging.join("ws/model.bin")).unwrap(), new);
    assert_eq!(std::fs::read(staging.join("ws/log.bin")).unwrap(), grown);
    no_parts(&staging);
    let result = Transfers::sync_result(&staging).unwrap();
    assert_eq!(outcome(&result, "ws/model.bin"), SyncOutcome::Landed);
    assert_eq!(outcome(&result, "ws/log.bin"), SyncOutcome::Landed);

    // 1 changed block + the 3 blocks past the old prefix (block 5 differs
    // in length, so it travels too).
    let sent = pair.sender_net.resumed_bytes.load(Ordering::SeqCst) as u64;
    let expected = 4 * BLOCK_SIZE;
    assert!(
        sent >= expected && sent < expected + 256 * 1024,
        "{sent} bytes on the wire, expected about {expected}"
    );
}

/// Tolerant sync: a file rewritten while it is sent is skipped; the rest
/// completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tolerant_sync_skips_a_file_that_changes_mid_send() {
    let pair = pair(0, 0);
    let root = pair._dir.path();
    let ws = root.join("laptop/ws");
    let staging = root.join("phone/staging");
    write(&ws.join("big.bin"), &pattern(64 * MIB, 10));
    let rewrite = pattern(64 * MIB, 20);
    write(&ws.join("small.txt"), b"small");
    pair.receiver_net.grant("t", grant(&staging, &[]));
    pair.sender_net.slow.store(true, Ordering::SeqCst);

    let reply = pair
        .sender
        .send_sync(sync(
            "t",
            vec![
                source("ws/big.bin", &ws.join("big.bin")),
                source("ws/small.txt", &ws.join("small.txt")),
            ],
            true,
        ))
        .await
        .unwrap();
    wait_for(&pair.receiver, &reply.transfer_id, |r| r.done_bytes > 0).await;
    write(&ws.join("big.bin"), &rewrite);

    completes(&pair, &reply.transfer_id).await;
    let row = wait_for(
        &pair.receiver,
        &reply.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    assert_eq!(row.skipped, 1);
    let result = Transfers::sync_result(&staging).unwrap();
    assert_eq!(outcome(&result, "ws/big.bin"), SyncOutcome::Skipped);
    let big = result.files.iter().find(|f| f.rel == "ws/big.bin").unwrap();
    assert!(
        big.reason
            .as_deref()
            .unwrap_or_default()
            .contains("changed")
    );
    assert_eq!(outcome(&result, "ws/small.txt"), SyncOutcome::Landed);
    assert!(!staging.join("ws/big.bin").exists());
    assert_eq!(
        std::fs::read(staging.join("ws/small.txt")).unwrap(),
        b"small"
    );
    no_parts(&staging);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_strict_sync_fails_when_a_file_changes_mid_send() {
    let pair = pair(0, 0);
    let root = pair._dir.path();
    let ws = root.join("laptop/ws");
    let staging = root.join("phone/staging");
    write(&ws.join("big.bin"), &pattern(64 * MIB, 30));
    let rewrite = pattern(64 * MIB, 40);
    pair.receiver_net.grant("t", grant(&staging, &[]));
    pair.sender_net.slow.store(true, Ordering::SeqCst);

    let reply = pair
        .sender
        .send_sync(sync(
            "t",
            vec![source("ws/big.bin", &ws.join("big.bin"))],
            false,
        ))
        .await
        .unwrap();
    wait_for(&pair.receiver, &reply.transfer_id, |r| r.done_bytes > 0).await;
    write(&ws.join("big.bin"), &rewrite);

    let failed = wait_for(
        &pair.sender,
        &reply.transfer_id,
        state_is(FileTransferState::Failed),
    )
    .await;
    assert!(failed.error.unwrap().contains("changed"));
    wait_for(
        &pair.receiver,
        &reply.transfer_id,
        state_is(FileTransferState::Failed),
    )
    .await;
    assert!(!staging.join(zeron_transfer::SYNC_RESULT_FILE).exists());
    no_parts(&staging);
}

/// A sync cut mid-transfer resumes — even after the receiving engine
/// restarted and no longer grants the ticket. A new transfer with the same
/// ticket asks for a grant again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sync_resumes_after_a_cut_and_a_receiver_restart() {
    let pair = pair(1, 6 * MIB as i64);
    let root = pair._dir.path();
    let ws = root.join("laptop/ws");
    let basis = root.join("phone/ws");
    let staging = root.join("phone/staging");
    // The first half matches the basis (seeded), the second half travels.
    let new = pattern(24 * MIB, 8);
    let mut old = new.clone();
    for byte in &mut old[12 * MIB..] {
        *byte = byte.wrapping_add(1);
    }
    write(&ws.join("data.bin"), &new);
    write(&basis.join("data.bin"), &old);
    write(&ws.join("unchanged.txt"), b"same");
    write(&basis.join("unchanged.txt"), b"same");
    pair.receiver_net
        .grant("t", grant(&staging, &[("ws", &basis)]));
    let files = vec![
        source("ws/data.bin", &ws.join("data.bin")),
        source("ws/unchanged.txt", &ws.join("unchanged.txt")),
    ];
    let reply = pair
        .sender
        .send_sync(sync("t", files.clone(), false))
        .await
        .unwrap();
    let cut = wait_for(
        &pair.receiver,
        &reply.transfer_id,
        state_is(FileTransferState::Reconnecting),
    )
    .await;
    assert!(
        cut.done_bytes >= 12 * MIB as u64 && cut.done_bytes < cut.total_bytes,
        "seeded blocks count as done: {}",
        cut.done_bytes
    );

    // Restart the receiving engine without any grants.
    pair.receiver.shutdown();
    let restarted_net = net("phone", 0, 0);
    restarted_net.set_peer(pair.sender.clone());
    let restarted = service(
        root,
        "phone",
        Some(pair.home.clone()),
        restarted_net.clone(),
    );
    pair.sender_net.set_peer(restarted.clone());

    wait_for(
        &restarted,
        &reply.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    wait_for(
        &pair.sender,
        &reply.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    assert_eq!(restarted_net.grant_calls.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(staging.join("ws/data.bin")).unwrap(), new);
    let result = Transfers::sync_result(&staging).unwrap();
    assert_eq!(outcome(&result, "ws/data.bin"), SyncOutcome::Landed);
    assert_eq!(outcome(&result, "ws/unchanged.txt"), SyncOutcome::Unchanged);
    assert!(!staging.join("ws/unchanged.txt").exists());
    no_parts(&staging);
    // The resume never resent the seeded half.
    let resumed = pair.sender_net.resumed_bytes.load(Ordering::SeqCst) as u64;
    assert!(
        resumed < 12 * MIB as u64 + 64 * 1024,
        "{resumed} bytes resent"
    );

    // A new transfer under the same ticket needs a grant again.
    let again = pair
        .sender
        .send_sync(sync("t", files, false))
        .await
        .unwrap();
    let refused = wait_for(
        &pair.sender,
        &again.transfer_id,
        state_is(FileTransferState::Failed),
    )
    .await;
    assert!(refused.error.unwrap().contains("ticket"));
    assert_eq!(restarted_net.grant_calls.load(Ordering::SeqCst), 1);
    restarted.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_inbox_send_beside_a_sync_is_unaffected() {
    let pair = pair(0, 0);
    let file = pair._dir.path().join("notes.txt");
    write(&file, b"notes");
    // A grant on the receiver changes nothing for ticketless offers.
    let staging: PathBuf = pair._dir.path().join("phone/staging");
    pair.receiver_net.grant("t", grant(&staging, &[]));
    let reply = pair
        .sender
        .send(zeron_transfer::SendRequest {
            to: "phone".into(),
            paths: vec![file],
            destination: None,
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap();
    completes(&pair, &reply.transfer_id).await;
    assert_eq!(
        std::fs::read(pair.home.join("Zeron Transfers/laptop name/notes.txt")).unwrap(),
        b"notes"
    );
    assert_eq!(pair.receiver_net.grant_calls.load(Ordering::SeqCst), 0);
    assert!(!staging.exists());
}

/// A final round's basis lists the first round's staging before the
/// landing: files delivered in round 1 don't travel again, and the record
/// says which copy matched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_round_uses_the_first_rounds_staging_as_basis() {
    let pair = pair(0, 0);
    let root = pair._dir.path();
    let ws = root.join("laptop/ws");
    let landing = root.join("phone/landing/ws");
    let (r1, r2) = (root.join("phone/staging/r1"), root.join("phone/staging/r2"));

    write(&ws.join("edited.txt"), b"edited in round 1");
    write(&landing.join("edited.txt"), b"original");
    write(&ws.join("same.txt"), b"never edited");
    write(&landing.join("same.txt"), b"never edited");
    write(&ws.join("late.bin"), &pattern(5 * MIB, 11));
    write(&landing.join("late.bin"), &pattern(5 * MIB, 11));
    let files = || {
        vec![
            source("ws/edited.txt", &ws.join("edited.txt")),
            source("ws/same.txt", &ws.join("same.txt")),
            source("ws/late.bin", &ws.join("late.bin")),
        ]
    };
    pair.receiver_net
        .grant("r1", grant(&r1, &[("ws", &landing)]));
    let first = pair
        .sender
        .send_sync(sync("r1", files(), true))
        .await
        .unwrap();
    completes(&pair, &first.transfer_id).await;
    let result = Transfers::sync_result(&r1).unwrap();
    assert_eq!(outcome(&result, "ws/edited.txt"), SyncOutcome::Landed);
    assert_eq!(outcome(&result, "ws/same.txt"), SyncOutcome::Unchanged);

    // Between rounds the agent edits a file the landing already matched.
    let mut late = pattern(5 * MIB, 11);
    late[3 * MIB] ^= 1;
    write(&ws.join("late.bin"), &late);

    let mut round_two = grant(&r2, &[]);
    round_two
        .basis
        .insert("ws".into(), vec![r1.join("ws"), landing.clone()]);
    pair.receiver_net.grant("r2", round_two);
    let before = pair.sender_net.resumed_bytes.load(Ordering::SeqCst);
    let second = pair
        .sender
        .send_sync(sync("r2", files(), false))
        .await
        .unwrap();
    completes(&pair, &second.transfer_id).await;
    let result = Transfers::sync_result(&r2).unwrap();
    let file = |rel: &str| result.files.iter().find(|f| f.rel == rel).unwrap().clone();
    assert_eq!(file("ws/edited.txt").outcome, SyncOutcome::Unchanged);
    assert_eq!(file("ws/edited.txt").basis_index, Some(0), "round 1's copy");
    assert_eq!(file("ws/same.txt").outcome, SyncOutcome::Unchanged);
    assert_eq!(file("ws/same.txt").basis_index, Some(1), "the landing");
    assert_eq!(file("ws/late.bin").outcome, SyncOutcome::Landed);
    assert_eq!(file("ws/late.bin").basis_index, None);
    assert_eq!(std::fs::read(r2.join("ws/late.bin")).unwrap(), late);
    assert!(!r2.join("ws/edited.txt").exists());
    // Only the edited block of late.bin travelled.
    let sent = (pair.sender_net.resumed_bytes.load(Ordering::SeqCst) - before) as u64;
    assert!(
        (BLOCK_SIZE..BLOCK_SIZE + 128 * 1024).contains(&sent),
        "{sent} bytes on the wire"
    );
    let json = std::fs::read_to_string(r2.join(zeron_transfer::SYNC_RESULT_FILE)).unwrap();
    assert!(json.contains(r#""basisIndex":0"#), "{json}");
}

/// A round with nothing to carry still completes and leaves its record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_round_completes_with_an_empty_result() {
    let pair = pair(0, 0);
    let staging = pair._dir.path().join("phone/staging");
    pair.receiver_net.grant("t", grant(&staging, &[]));
    let reply = pair
        .sender
        .send_sync(sync("t", Vec::new(), false))
        .await
        .unwrap();
    completes(&pair, &reply.transfer_id).await;
    let result = Transfers::sync_result(&staging).unwrap();
    assert_eq!(result.transfer_id, reply.transfer_id);
    assert!(result.files.is_empty() && result.dirs.is_empty() && result.symlinks.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hash_files_reports_regular_files_only() {
    let pair = pair(0, 0);
    let dir = pair._dir.path();
    write(&dir.join("a.txt"), b"a");
    std::fs::create_dir_all(dir.join("folder")).unwrap();
    let mut paths = vec![dir.join("a.txt"), dir.join("folder"), dir.join("missing")];
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.join("a.txt"), dir.join("link")).unwrap();
        paths.push(dir.join("link"));
    }
    let hashes = pair.sender.hash_files(paths).await.unwrap();
    assert_eq!(
        hashes[0].as_deref(),
        Some(zeron_transfer::manifest::hex(&<sha2::Sha256 as sha2::Digest>::digest(b"a")).as_str())
    );
    assert!(hashes[1..].iter().all(Option::is_none), "{hashes:?}");
}
