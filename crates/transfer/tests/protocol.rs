//! Two transfer services wired back to back over in-memory lanes: folders,
//! resume after a severed tunnel, confirmation, cancellation, and a sender
//! that lies in its manifest or its blocks.

mod common;

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::*;
use zeron_proto::{FileTransferState, FileTransferTransport};
use zeron_transfer::manifest::{Entry, EntryKind, Manifest};
use zeron_transfer::wire::{self, Block, Lane, Msg};
use zeron_transfer::{BoxIo, SendRequest, Transfers, TransportPolicy};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_folder_and_a_file_arrive_intact_in_the_inbox() {
    let pair = pair(0, 0);
    let project = project(pair._dir.path());
    let apk = pair._dir.path().join("app-debug.apk");
    std::fs::write(&apk, pattern(2 * 1024 * 1024, 7)).unwrap();
    let reply = pair
        .sender
        .send(SendRequest {
            to: "phone".into(),
            paths: vec![project.clone(), apk.clone()],
            destination: None,
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap();
    assert_eq!(reply.to_device_name, "phone name");
    let sent = wait_for(
        &pair.sender,
        &reply.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    assert_eq!(sent.done_bytes, sent.total_bytes);
    let received = wait_for(
        &pair.receiver,
        &reply.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    let inbox = pair.home.join("Zeron Transfers").join("laptop name");
    assert_eq!(
        received.destination.as_deref(),
        Some(inbox.to_str().unwrap())
    );
    assert_eq!(
        received.items[0].path.as_deref(),
        Some(inbox.join("src-project").to_str().unwrap())
    );
    assert_same_tree(&project, &inbox.join("src-project"));
    assert!(inbox.join("src-project/empty-dir").is_dir());
    assert_eq!(
        std::fs::read(inbox.join("app-debug.apk")).unwrap(),
        std::fs::read(&apk).unwrap()
    );
    no_parts(&inbox);

    // The same items again land beside the first copy, never over it.
    let again = pair
        .sender
        .send(SendRequest {
            to: "phone".into(),
            paths: vec![project.clone(), apk.clone()],
            destination: None,
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap();
    wait_for(
        &pair.receiver,
        &again.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    assert!(inbox.join("src-project (2)/README.md").is_file());
    assert!(inbox.join("app-debug (2).apk").is_file());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_severed_tunnel_resumes_from_verified_blocks() {
    // The first connection dies after ~20 MiB of a 48 MiB file.
    let pair = pair(1, 20 * 1024 * 1024);
    let big = pair._dir.path().join("big.bin");
    let bytes = pattern(48 * 1024 * 1024 + 5, 3);
    std::fs::write(&big, &bytes).unwrap();
    let reply = pair
        .sender
        .send(SendRequest {
            to: "phone".into(),
            paths: vec![big.clone()],
            destination: None,
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap();
    let interrupted = wait_for(
        &pair.sender,
        &reply.transfer_id,
        state_is(FileTransferState::Reconnecting),
    )
    .await;
    assert!(interrupted.done_bytes < interrupted.total_bytes);
    let receiving = wait_for(
        &pair.receiver,
        &reply.transfer_id,
        state_is(FileTransferState::Reconnecting),
    )
    .await;
    let before = receiving.done_bytes;
    assert!(before > 0, "some blocks were verified before the cut");
    let done = wait_for(
        &pair.receiver,
        &reply.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    assert_eq!(done.done_bytes, bytes.len() as u64);
    let landed = pair.home.join("Zeron Transfers/laptop name/big.bin");
    assert!(std::fs::read(&landed).unwrap() == bytes);
    assert_eq!(
        pair.sender_net.connects.load(Ordering::SeqCst),
        2,
        "one resume"
    );
    wait_for(
        &pair.sender,
        &reply.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn confirmation_decline_and_cancel_reach_both_sides() {
    let pair = pair(0, 0);
    let mut settings = pair.receiver.settings();
    settings.require_confirmation = true;
    pair.receiver.set_settings(settings).unwrap();
    let file = pair._dir.path().join("notes.txt");
    std::fs::write(&file, b"notes").unwrap();
    let send = |paths: Vec<PathBuf>| {
        pair.sender.send(SendRequest {
            to: "phone".into(),
            paths,
            destination: None,
            policy: TransportPolicy::Auto,
        })
    };

    // Declined.
    let declined = send(vec![file.clone()]).await.unwrap();
    wait_for(
        &pair.receiver,
        &declined.transfer_id,
        state_is(FileTransferState::AwaitingAcceptance),
    )
    .await;
    wait_for(
        &pair.sender,
        &declined.transfer_id,
        state_is(FileTransferState::AwaitingAcceptance),
    )
    .await;
    pair.receiver.decline(&declined.transfer_id).unwrap();
    wait_for(
        &pair.sender,
        &declined.transfer_id,
        state_is(FileTransferState::Declined),
    )
    .await;
    assert!(
        !pair
            .home
            .join("Zeron Transfers/laptop name/notes.txt")
            .exists()
    );

    // Accepted.
    let accepted = send(vec![file.clone()]).await.unwrap();
    wait_for(
        &pair.receiver,
        &accepted.transfer_id,
        state_is(FileTransferState::AwaitingAcceptance),
    )
    .await;
    pair.receiver.accept(&accepted.transfer_id).unwrap();
    wait_for(
        &pair.sender,
        &accepted.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    assert_eq!(
        std::fs::read(pair.home.join("Zeron Transfers/laptop name/notes.txt")).unwrap(),
        b"notes"
    );

    // Cancelled by the sender while waiting.
    let cancelled = send(vec![file.clone()]).await.unwrap();
    wait_for(
        &pair.receiver,
        &cancelled.transfer_id,
        state_is(FileTransferState::AwaitingAcceptance),
    )
    .await;
    pair.sender.cancel(&cancelled.transfer_id, true).unwrap();
    wait_for(
        &pair.receiver,
        &cancelled.transfer_id,
        state_is(FileTransferState::Cancelled),
    )
    .await;
    wait_for(
        &pair.sender,
        &cancelled.transfer_id,
        state_is(FileTransferState::Cancelled),
    )
    .await;

    // Cancelled by the receiver mid-transfer: partial data is removed.
    let mut settings = pair.receiver.settings();
    settings.require_confirmation = false;
    pair.receiver.set_settings(settings).unwrap();
    let big = pair._dir.path().join("large.bin");
    std::fs::write(&big, pattern(256 * 1024 * 1024, 9)).unwrap();
    pair.sender_net.slow.store(true, Ordering::SeqCst);
    let midway = send(vec![big]).await.unwrap();
    wait_for(&pair.receiver, &midway.transfer_id, |r| r.done_bytes > 0).await;
    pair.receiver.cancel(&midway.transfer_id, true).unwrap();
    wait_for(
        &pair.sender,
        &midway.transfer_id,
        state_is(FileTransferState::Cancelled),
    )
    .await;
    wait_for(
        &pair.receiver,
        &midway.transfer_id,
        state_is(FileTransferState::Cancelled),
    )
    .await;
    no_parts(&pair.home.join("Zeron Transfers/laptop name"));
    assert!(
        !pair
            .home
            .join("Zeron Transfers/laptop name/large.bin")
            .exists()
    );
}

/// A sender that cancels and hangs up before the receiver even asked its
/// user: the queued `Cancel` wins over the broken lane.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_racing_the_confirmation_prompt_is_not_lost() {
    let pair = pair(0, 0);
    let mut settings = pair.receiver.settings();
    settings.require_confirmation = true;
    pair.receiver.set_settings(settings).unwrap();
    for _ in 0..20 {
        let (mut ours, theirs) = tokio::io::duplex(1024 * 1024);
        pair.receiver.accept_lane(
            "laptop".into(),
            FileTransferTransport::P2p,
            Box::new(theirs),
        );
        let manifest = Manifest {
            entries: vec![Entry {
                path: "f.txt".into(),
                kind: EntryKind::File,
                size: 4,
                mode: 0o644,
                target: None,
                sha256: None,
                blocks: None,
                mtime_ms: None,
            }],
        };
        let transfer_id = uuid::Uuid::new_v4().to_string();
        for msg in [
            Msg::Hello {
                v: wire::PROTOCOL_VERSION,
                transfer_id: transfer_id.clone(),
                session_id: "s1".into(),
                lane: Lane::Control,
                sender_name: "laptop".into(),
            },
            Msg::Offer {
                entry_count: 1,
                file_count: 1,
                total_bytes: 4,
                digest: manifest.digest(),
                destination: None,
                skipped: 0,
                ticket: None,
            },
            Msg::Manifest {
                entries: manifest.entries.clone(),
            },
            Msg::ManifestEnd,
            Msg::Cancel { reason: None },
        ] {
            wire::write_msg(&mut ours, &msg).await.unwrap();
        }
        drop(ours);
        wait_for(
            &pair.receiver,
            &transfer_id,
            state_is(FileTransferState::Cancelled),
        )
        .await;
    }
}

/// Speak the wire protocol by hand, as a hostile or buggy sender would.
async fn raw_offer(receiver: &Transfers, entries: Vec<Entry>) -> (BoxIo, Msg) {
    let (mut ours, theirs) = tokio::io::duplex(1024 * 1024);
    receiver.accept_lane(
        "laptop".into(),
        FileTransferTransport::Relay,
        Box::new(theirs),
    );
    let manifest = Manifest { entries };
    let transfer_id = uuid::Uuid::new_v4().to_string();
    for msg in [
        Msg::Hello {
            v: wire::PROTOCOL_VERSION,
            transfer_id,
            session_id: "s1".into(),
            lane: Lane::Control,
            sender_name: "laptop".into(),
        },
        Msg::Offer {
            entry_count: manifest.entries.len() as u64,
            file_count: manifest.file_count(),
            total_bytes: manifest.total_bytes(),
            digest: manifest.digest(),
            destination: None,
            skipped: 0,
            ticket: None,
        },
        Msg::Manifest {
            entries: manifest.entries.clone(),
        },
        Msg::ManifestEnd,
    ] {
        wire::write_msg(&mut ours, &msg).await.unwrap();
    }
    let reply = tokio::time::timeout(Duration::from_secs(10), wire::read_msg(&mut ours))
        .await
        .unwrap()
        .unwrap();
    (Box::new(ours), reply)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn traversal_in_a_manifest_is_refused_and_nothing_is_written() {
    let pair = pair(0, 0);
    let evil = |path: &str, kind| Entry {
        path: path.into(),
        kind,
        size: 4,
        mode: 0,
        target: (kind == EntryKind::Symlink).then(|| "/".into()),
        sha256: None,
        blocks: None,
        mtime_ms: None,
    };
    for entries in [
        vec![evil("../../escape.txt", EntryKind::File)],
        vec![evil("/abs.txt", EntryKind::File)],
        vec![
            evil("ok", EntryKind::Dir),
            evil("ok/../../x", EntryKind::File),
        ],
        vec![
            evil("link", EntryKind::Symlink),
            evil("link/etc-passwd", EntryKind::File),
        ],
    ] {
        let (_io, reply) = raw_offer(&pair.receiver, entries).await;
        match reply {
            Msg::Error { message } => assert!(message.starts_with("Refused"), "{message}"),
            other => panic!("a traversal was answered with {other:?}"),
        }
    }
    assert!(!pair._dir.path().join("escape.txt").exists());
    assert!(
        !pair.home.join("Zeron Transfers").exists()
            || std::fs::read_dir(pair.home.join("Zeron Transfers"))
                .unwrap()
                .next()
                .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn corrupt_blocks_are_rejected_and_requested_again() {
    let pair = pair(0, 0);
    let data = b"four".to_vec();
    let (mut control, reply) = raw_offer(
        &pair.receiver,
        vec![Entry {
            path: "f.txt".into(),
            kind: EntryKind::File,
            size: 4,
            mode: 0o644,
            target: None,
            sha256: None,
            blocks: None,
            mtime_ms: None,
        }],
    )
    .await;
    assert!(
        matches!(reply, Msg::Accept { ref have, sync: false } if have.is_empty()),
        "{reply:?}"
    );
    let id = pair.receiver.list()[0].id.clone();
    let (mut lane, theirs) = tokio::io::duplex(1024 * 1024);
    pair.receiver.accept_lane(
        "laptop".into(),
        FileTransferTransport::Relay,
        Box::new(theirs),
    );
    wire::write_msg(
        &mut lane,
        &Msg::Hello {
            v: wire::PROTOCOL_VERSION,
            transfer_id: id.clone(),
            session_id: "s1".into(),
            lane: Lane::Data,
            sender_name: "laptop".into(),
        },
    )
    .await
    .unwrap();
    // A block whose bytes don't match its hash.
    let mut bad = Block::new(0, 0, data.clone());
    bad.data[0] ^= 0xff;
    wire::write_block(&mut lane, &bad).await.unwrap();
    loop {
        match wire::read_msg(&mut control).await.unwrap() {
            Msg::Resend {
                file: 0,
                blocks: Some(blocks),
            } => {
                assert_eq!(blocks, vec![0]);
                break;
            }
            Msg::Progress { done_bytes } => assert_eq!(done_bytes, 0),
            other => panic!("unexpected {other:?}"),
        }
    }
    // The good block plus a digest that doesn't match: the whole file is
    // requested again rather than renamed into place.
    wire::write_block(&mut lane, &Block::new(0, 0, data.clone()))
        .await
        .unwrap();
    wire::write_msg(
        &mut control,
        &Msg::Digest {
            file: 0,
            sha256: "0".repeat(64),
        },
    )
    .await
    .unwrap();
    loop {
        match wire::read_msg(&mut control).await.unwrap() {
            Msg::Resend {
                file: 0,
                blocks: None,
            } => break,
            Msg::Progress { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(!pair.home.join("Zeron Transfers/laptop name/f.txt").exists());
    // Correct bytes and digest complete it.
    wire::write_block(&mut lane, &Block::new(0, 0, data.clone()))
        .await
        .unwrap();
    let digest = zeron_transfer::manifest::hex(&<sha2::Sha256 as sha2::Digest>::digest(&data));
    wire::write_msg(
        &mut control,
        &Msg::Digest {
            file: 0,
            sha256: digest,
        },
    )
    .await
    .unwrap();
    loop {
        match wire::read_msg(&mut control).await.unwrap() {
            Msg::Complete => break,
            Msg::Progress { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(
        std::fs::read(pair.home.join("Zeron Transfers/laptop name/f.txt")).unwrap(),
        data
    );
    wait_for(&pair.receiver, &id, state_is(FileTransferState::Completed)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_explicit_destination_must_be_inside_home() {
    let pair = pair(0, 0);
    let file = pair._dir.path().join("a.txt");
    std::fs::write(&file, b"a").unwrap();
    let project = pair.home.join("projects/app");
    std::fs::create_dir_all(&project).unwrap();
    let inside = pair
        .sender
        .send(SendRequest {
            to: "phone".into(),
            paths: vec![file.clone()],
            destination: Some(project.to_string_lossy().into_owned()),
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap();
    wait_for(
        &pair.sender,
        &inside.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    assert_eq!(std::fs::read(project.join("a.txt")).unwrap(), b"a");
    let outside = pair
        .sender
        .send(SendRequest {
            to: "phone".into(),
            paths: vec![file],
            destination: Some(pair._dir.path().to_string_lossy().into_owned()),
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap();
    let failed = wait_for(
        &pair.sender,
        &outside.transfer_id,
        state_is(FileTransferState::Failed),
    )
    .await;
    assert!(failed.error.unwrap().contains("home folder"));
}

/// `~` means the sending engine's home for sources and the receiving
/// engine's home for a destination (a remote viewer or an agent in `~`
/// can't know either absolute path).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tilde_paths_resolve_against_each_engines_home() {
    let pair = pair(0, 0);
    // The phone engine has a home; `~/notes.txt` is its file.
    std::fs::write(pair.home.join("notes.txt"), b"from home").unwrap();
    let sent = pair
        .receiver
        .send(SendRequest {
            to: "laptop".into(),
            paths: vec!["~/notes.txt".into()],
            destination: None,
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap();
    let done = wait_for(
        &pair.sender,
        &sent.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    let landed = PathBuf::from(done.items[0].path.as_deref().unwrap());
    assert_eq!(std::fs::read(landed).unwrap(), b"from home");

    // A `~/…` destination is the receiving engine's home.
    let project = pair.home.join("projects/app");
    std::fs::create_dir_all(&project).unwrap();
    let file = pair._dir.path().join("b.txt");
    std::fs::write(&file, b"b").unwrap();
    let into = pair
        .sender
        .send(SendRequest {
            to: "phone".into(),
            paths: vec![file],
            destination: Some("~/projects/app".into()),
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap();
    wait_for(
        &pair.receiver,
        &into.transfer_id,
        state_is(FileTransferState::Completed),
    )
    .await;
    assert_eq!(std::fs::read(project.join("b.txt")).unwrap(), b"b");

    // Without a home, `~` is a clear error rather than a relative path.
    let error = pair
        .sender
        .send(SendRequest {
            to: "phone".into(),
            paths: vec!["~/x".into()],
            destination: None,
            policy: TransportPolicy::Auto,
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("home folder"), "{error}");
}
