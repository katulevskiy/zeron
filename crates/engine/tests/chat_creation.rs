//! Development-scope createChat stores new config, but retries never overwrite a seeded row.
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zeron_engine::{EngineCore, EngineProfile, HarnessId, HarnessRegistry};
use zeron_proto::{Chat, ChatConfig, SandboxLevel};
use zeron_rpc::{memory_client, methods};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn development_create_chat_preserves_config_and_duplicate_creation_is_a_noop() {
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let core = EngineCore::assemble_with_profile(
        EngineProfile::development(root.path(), "creation-org", "creation-owner"),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let client = memory_client(core.rpc_service());
    let config = ChatConfig {
        harness: HarnessId::Mock,
        model: Some("mock-1".into()),
        reasoning: None,
        model_options: Default::default(),
        sandbox: SandboxLevel::WorkspaceWrite,
    };
    client
        .call(
            methods::MUTATE,
            json!({
                "op": "createSpace", "spaceId": "seed-space", "deviceId": core.device_id,
                "path": root.path(),
            }),
        )
        .await
        .unwrap();
    client
        .call(
            methods::MUTATE,
            json!({
                "op": "createChat", "chatId": "seed-chat", "spaceId": "seed-space",
            }),
        )
        .await
        .unwrap();
    let seeded = core.workspace.chat("seed-chat").unwrap().unwrap();
    assert_eq!(seeded.config, None);
    assert_eq!(seeded.space_id.as_deref(), Some("seed-space"));

    // This is the fixture ID-collision pattern: a create is an idempotent
    // retry, not SetChatConfig or a conversion to a projectless chat.
    assert_eq!(
        client
            .call(
                methods::MUTATE,
                json!({
                    "op": "createChat", "chatId": "seed-chat", "deviceId": core.device_id,
                    "config": config,
                })
            )
            .await
            .unwrap(),
        json!({"ok": true})
    );
    assert_eq!(
        core.workspace.chat("seed-chat").unwrap(),
        Some(seeded.clone())
    );

    assert_eq!(
        client
            .call(
                methods::MUTATE,
                json!({
                    "op": "createChat", "chatId": "new-chat", "deviceId": core.device_id,
                    "config": config,
                })
            )
            .await
            .unwrap(),
        json!({"ok": true})
    );
    let mut watch = client
        .subscribe(methods::WATCH_CHATS, json!({}))
        .await
        .unwrap();
    let rows = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let rows: Vec<Chat> =
                serde_json::from_value(watch.recv().await.expect("WatchChats ended")).unwrap();
            if rows.iter().any(|row| row.id == "new-chat") {
                break rows;
            }
        }
    })
    .await
    .expect("new chat was not published");
    let created = rows.iter().find(|row| row.id == "new-chat").unwrap();
    assert_eq!(created.config.as_ref(), Some(&config));
    assert_eq!(created.device_id, core.device_id);
    assert_eq!(created.cwd.as_deref(), Some("~"));
    assert_eq!(created.space_id, None);
    assert_eq!(rows.iter().find(|row| row.id == "seed-chat"), Some(&seeded));
    assert_eq!(rows.len(), 2);
    drop(watch);
    drop(client);
    core.shutdown().await;
}
