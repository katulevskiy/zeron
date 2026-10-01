//! The provisioning sequence against a fake Cloudflare API: exact requests,
//! the upload's metadata, idempotency, and errors that say what to fix.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::Enrollment;
use crate::mock::{FakeCloudflare, FakeState, MockServer, Reply};

struct CountingEnroller(AtomicUsize);

#[async_trait]
impl Enroller for CountingEnroller {
    async fn enroll(&self, name: &str) -> Result<Enrollment, CloudError> {
        let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(Enrollment {
            device_id: format!("box-{n}"),
            credential: format!("cred-{n}-{name}"),
            wake_key: crate::sign::new_wake_key(),
        })
    }
}

fn enroller() -> CountingEnroller {
    CountingEnroller(AtomicUsize::new(0))
}

fn request(existing: Vec<ExistingBox>) -> ProvisionRequest {
    ProvisionRequest {
        name: "Cloud".into(),
        instance_type: "standard-2".into(),
        idle_minutes: 15,
        region: None,
        edge_url: "https://edge.zeron.test".into(),
        image: "docker.io/zeronsh/zeron-cloud:1.2.3".into(),
        existing,
    }
}

async fn setup(state: FakeState) -> (FakeCloudflare, MockServer, Cloudflare) {
    let fake = FakeCloudflare::new(state);
    let server = fake.serve().await;
    let provider = Cloudflare::new("good-token").with_base_url(format!("{}/client/v4", server.url));
    (fake, server, provider)
}

fn lines(server: &MockServer) -> Vec<String> {
    server
        .requests()
        .iter()
        .map(|r| r.line().replacen("/client/v4", "", 1))
        .collect()
}

#[tokio::test]
async fn first_box_runs_the_documented_sequence() {
    let (fake, server, provider) = setup(FakeState::default()).await;
    let enroller = enroller();
    let done = provider
        .provision(&request(Vec::new()), &enroller)
        .await
        .unwrap();

    assert_eq!(
        lines(&server),
        [
            "GET /user/tokens/verify",
            "GET /accounts?per_page=50",
            "GET /accounts/acc-1/containers/me",
            "POST /accounts/acc-1/r2/buckets",
            "GET /accounts/acc-1/workers/services/zeron-cloud",
            "PUT /accounts/acc-1/workers/scripts/zeron-cloud",
            "GET /accounts/acc-1/workers/durable_objects/namespaces?per_page=1000",
            "GET /accounts/acc-1/containers/applications?name=zeron-cloud",
            "POST /accounts/acc-1/containers/applications",
            "PUT /accounts/acc-1/workers/scripts/zeron-cloud/secrets",
            "GET /accounts/acc-1/workers/subdomain",
            "PUT /accounts/acc-1/workers/subdomain",
            "POST /accounts/acc-1/workers/scripts/zeron-cloud/subdomain",
        ]
    );
    let requests = server.requests();
    assert!(
        requests
            .iter()
            .all(|r| r.header("authorization") == Some("Bearer good-token"))
    );
    assert_eq!(requests[3].json(), json!({ "name": "zeron-cloud" }));

    // The Worker upload: metadata + both modules.
    let upload = &requests[5];
    assert!(
        upload
            .header("content-type")
            .unwrap()
            .starts_with("multipart/form-data; boundary=")
    );
    let state = fake.state();
    let (metadata, modules) = &state.uploads[0];
    assert_eq!(
        metadata,
        &json!({
            "main_module": "worker.js",
            "compatibility_date": worker::COMPATIBILITY_DATE,
            "bindings": [
                { "type": "durable_object_namespace", "name": "BOX", "class_name": "ZeronBox" },
                { "type": "r2_bucket", "name": "CKPT", "bucket_name": "zeron-cloud" },
                { "type": "plain_text", "name": "ZERON_EDGE_URL", "text": "https://edge.zeron.test" },
            ],
            "containers": [{ "class_name": "ZeronBox" }],
            "migrations": { "new_tag": "v1", "steps": [{ "new_sqlite_classes": ["ZeronBox"] }] },
            "keep_bindings": ["secret_text"],
        })
    );
    let names: Vec<_> = modules.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["worker.js", "core.js"]);
    for module in modules {
        assert_eq!(
            module.content_type.as_deref(),
            Some("application/javascript+module")
        );
        assert_eq!(module.filename.as_deref(), Some(module.name.as_str()));
    }
    assert_eq!(modules[0].body, worker::WORKER_JS.as_bytes());
    assert_eq!(modules[1].body, worker::CORE_JS.as_bytes());

    // The container app, bound to the Worker's namespace (not the other one).
    assert_eq!(
        requests[8].json(),
        json!({
            "name": "zeron-cloud",
            "scheduling_policy": "default",
            "configuration": { "image": "docker.io/zeronsh/zeron-cloud:1.2.3", "instance_type": "standard-2" },
            "instances": 0,
            "max_instances": 1,
            "durable_objects": { "namespace_id": "ns-1" },
            "rollout_active_grace_period": 900,
        })
    );

    // The secret holds the new box's keys.
    assert_eq!(requests[9].json()["name"], "ZERON_BOXES");
    assert_eq!(requests[9].json()["type"], "secret_text");
    let secret: Value = serde_json::from_str(&state.secrets["ZERON_BOXES"]).unwrap();
    assert_eq!(
        secret,
        json!({ "box-1": { "wakeKey": done.record.wake_key, "credential": "cred-1-Cloud", "idleMinutes": 15 } })
    );
    assert_eq!(
        requests[12].json(),
        json!({ "enabled": true, "previews_enabled": false })
    );

    let sub = state.subdomain.clone().unwrap();
    assert!(sub.starts_with("zeron-"));
    assert_eq!(
        done.record,
        CloudBox {
            id: "box-1".into(),
            provider: "cloudflare".into(),
            name: "Cloud".into(),
            account_id: "acc-1".into(),
            worker_url: format!("https://zeron-cloud.{sub}.workers.dev"),
            wake_key: done.record.wake_key.clone(),
            instance_type: "standard-2".into(),
            idle_minutes: 15,
            created_at: done.record.created_at,
            state: CloudBoxState::Asleep,
            state_at: done.record.state_at,
            error: None,
        }
    );
    assert_eq!(done.credential, "cred-1-Cloud");
}

#[tokio::test]
async fn a_second_box_reuses_everything_and_keeps_the_first_reachable() {
    let (fake, server, provider) = setup(FakeState::default()).await;
    let enroller = enroller();
    let first = provider
        .provision(&request(Vec::new()), &enroller)
        .await
        .unwrap();
    server.clear();

    let existing = vec![ExistingBox {
        record: first.record.clone(),
        credential: Some(first.credential.clone()),
    }];
    let second = provider
        .provision(&request(existing), &enroller)
        .await
        .unwrap();
    assert_eq!(second.record.id, "box-2");
    assert_eq!(second.record.worker_url, first.record.worker_url);
    assert_eq!(
        lines(&server),
        [
            "GET /user/tokens/verify",
            "GET /accounts?per_page=50",
            "GET /accounts/acc-1/containers/me",
            "POST /accounts/acc-1/r2/buckets", // 409: exists
            "GET /accounts/acc-1/workers/services/zeron-cloud",
            "PUT /accounts/acc-1/workers/scripts/zeron-cloud",
            "GET /accounts/acc-1/workers/durable_objects/namespaces?per_page=1000",
            "GET /accounts/acc-1/containers/applications?name=zeron-cloud",
            "PATCH /accounts/acc-1/containers/applications/app-1",
            "PUT /accounts/acc-1/workers/scripts/zeron-cloud/secrets",
            "GET /accounts/acc-1/workers/subdomain",
            "POST /accounts/acc-1/workers/scripts/zeron-cloud/subdomain",
        ]
    );
    let state = fake.state();
    // The script already has v1: no migration this time.
    assert!(state.uploads[1].0.get("migrations").is_none());
    assert_eq!(state.apps.len(), 1);
    assert_eq!(state.apps[0]["max_instances"], 2);
    assert!(state.rollouts.is_empty(), "same image and type: no rollout");
    let secret: Value = serde_json::from_str(&state.secrets["ZERON_BOXES"]).unwrap();
    assert_eq!(secret["box-1"]["credential"], "cred-1-Cloud");
    assert_eq!(secret["box-2"]["credential"], "cred-2-Cloud");
}

#[tokio::test]
async fn retrying_after_a_failure_finishes_the_job() {
    let mut state = FakeState::default();
    // The account has no workers.dev subdomain and can't create one yet.
    state
        .forbidden
        .push("/accounts/acc-1/workers/subdomain".into());
    let (fake, _server, provider) = setup(state).await;
    let enroller = enroller();
    let err = provider
        .provision(&request(Vec::new()), &enroller)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CloudError::MissingPermission(Permission::WorkersScripts)
    ));
    fake.state().forbidden.clear();
    // The engine replays the same enrolment; here a second one is fine.
    let done = provider
        .provision(&request(Vec::new()), &enroller)
        .await
        .unwrap();
    let state = fake.state();
    assert_eq!(state.apps.len(), 1);
    assert_eq!(state.uploads.len(), 2);
    assert!(state.uploads[1].0.get("migrations").is_none());
    assert!(done.record.worker_url.ends_with(".workers.dev"));
}

#[tokio::test]
async fn a_new_instance_type_rolls_the_app_out() {
    let (fake, _server, provider) = setup(FakeState::default()).await;
    let enroller = enroller();
    let first = provider
        .provision(&request(Vec::new()), &enroller)
        .await
        .unwrap();
    let mut req = request(vec![ExistingBox {
        record: first.record,
        credential: Some(first.credential),
    }]);
    req.instance_type = "standard-3".into();
    req.region = Some("ENAM".into());
    provider.provision(&req, &enroller).await.unwrap();
    let state = fake.state();
    assert_eq!(
        state.apps[0]["configuration"]["instance_type"],
        "standard-3"
    );
    assert_eq!(state.apps[0]["constraints"], json!({ "regions": ["ENAM"] }));
    assert_eq!(
        state.rollouts,
        [json!({
            "description": "Zeron cloud box update",
            "strategy": "rolling",
            "kind": "full_auto",
            "step_percentage": 100,
            "target_configuration": { "image": "docker.io/zeronsh/zeron-cloud:1.2.3", "instance_type": "standard-3" },
        })]
    );
}

#[tokio::test]
async fn an_account_owned_token_is_verified_against_its_account() {
    let state = FakeState {
        user_token: false,
        ..FakeState::default()
    };
    let (_fake, server, provider) = setup(state).await;
    let account = provider.validate().await.unwrap();
    assert_eq!(account.id, "acc-1");
    assert_eq!(
        lines(&server)[..3],
        [
            "GET /user/tokens/verify",
            "GET /accounts?per_page=50",
            "GET /accounts/acc-1/tokens/verify",
        ]
    );
}

#[tokio::test]
async fn errors_say_what_to_fix() {
    // Wrong token.
    let (_f, server, _) = setup(FakeState::default()).await;
    let bad = Cloudflare::new("nope").with_base_url(format!("{}/client/v4", server.url));
    assert!(matches!(
        bad.validate().await.unwrap_err(),
        CloudError::InvalidToken
    ));

    // Several accounts, unless one is chosen.
    let state = FakeState {
        accounts: vec![("a".into(), "A".into()), ("b".into(), "B".into())],
        ..FakeState::default()
    };
    let (_f, server, provider) = setup(state).await;
    assert!(matches!(
        provider.validate().await.unwrap_err(),
        CloudError::MultipleAccounts(2)
    ));
    let chosen = Cloudflare::new("good-token")
        .with_base_url(format!("{}/client/v4", server.url))
        .with_account(Some("b".into()));
    assert_eq!(chosen.validate().await.unwrap().name, "B");

    // Not on Workers Paid.
    let state = FakeState {
        containers_me: Some(Reply::error(
            403,
            1001,
            "Containers are only available on the Workers Paid plan",
        )),
        ..FakeState::default()
    };
    let (_f, _s, provider) = setup(state).await;
    assert!(matches!(
        provider.validate().await.unwrap_err(),
        CloudError::NotOnWorkersPaid
    ));
    let state = FakeState {
        containers_me: Some(Reply::error(
            400,
            1001,
            "Containers are only available on the Workers Paid plan",
        )),
        ..FakeState::default()
    };
    let (_f, _s, provider) = setup(state).await;
    assert!(matches!(
        provider.validate().await.unwrap_err(),
        CloudError::NotOnWorkersPaid
    ));
    let state = FakeState {
        containers_me: Some(Reply::error(403, 10000, "Authentication error")),
        ..FakeState::default()
    };
    let (_f, _s, provider) = setup(state).await;
    assert!(matches!(
        provider.validate().await.unwrap_err(),
        CloudError::MissingPermission(Permission::Containers)
    ));
    let state = FakeState {
        containers_me: Some(Reply::error(404, 1002, "account not onboarded")),
        ..FakeState::default()
    };
    let (_f, _s, provider) = setup(state).await;
    let err = provider.validate().await.unwrap_err();
    assert!(matches!(err, CloudError::ContainersNotEnabled(_)), "{err}");

    // A missing permission names it.
    let mut state = FakeState::default();
    state.forbidden.push("/accounts/acc-1/r2/buckets".into());
    let (_f, _s, provider) = setup(state).await;
    let err = provider
        .provision(&request(Vec::new()), &enroller())
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CloudError::MissingPermission(Permission::R2Storage)
    ));
    assert!(err.to_string().contains("Workers R2 Storage: Edit"));

    // R2 never enabled on the account.
    let (_f, server, _) = setup(FakeState::default()).await;
    let r2_off = MockServer::start(|r| {
        if r.path.ends_with("/r2/buckets") {
            Reply::error(
                403,
                10042,
                "Please enable R2 through the Cloudflare Dashboard.",
            )
        } else {
            Reply::ok(json!({}))
        }
    })
    .await;
    let provider = Cloudflare::new("t").with_base_url(&r2_off.url);
    assert!(matches!(
        provider.ensure_bucket("acc").await.unwrap_err(),
        CloudError::R2NotEnabled
    ));
    drop(server);
}

#[tokio::test]
async fn a_box_without_its_credential_here_stops_before_enrolling() {
    let (_fake, server, provider) = setup(FakeState::default()).await;
    let enroller = enroller();
    let first = provider
        .provision(&request(Vec::new()), &enroller)
        .await
        .unwrap();
    server.clear();
    let err = provider
        .provision(
            &request(vec![ExistingBox {
                record: first.record,
                credential: None,
            }]),
            &enroller,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("“Cloud”"), "{err}");
    assert_eq!(enroller.0.load(Ordering::SeqCst), 1);
    assert!(
        !lines(&server)
            .iter()
            .any(|l| l.contains("r2/buckets") || l.contains("scripts"))
    );
}

/// The box record as the fake Worker serves it (same server).
fn local(record: &CloudBox, server: &MockServer) -> CloudBox {
    CloudBox {
        worker_url: server.url.clone(),
        ..record.clone()
    }
}

#[tokio::test]
async fn wake_stop_and_status_are_signed_with_the_box_key() {
    let (fake, server, provider) = setup(FakeState::default()).await;
    let done = provider
        .provision(&request(Vec::new()), &enroller())
        .await
        .unwrap();
    let record = local(&done.record, &server);

    assert_eq!(
        provider.wake(&record).await.unwrap().state,
        CloudBoxState::Waking
    );
    assert_eq!(
        provider.status(&record).await.unwrap().state,
        CloudBoxState::Waking
    );
    assert_eq!(
        provider.stop(&record).await.unwrap().state,
        CloudBoxState::Checkpointing
    );
    let woken = server
        .requests()
        .into_iter()
        .find(|r| r.path == "/v1/boxes/box-1/wake")
        .unwrap();
    assert_eq!(woken.method, "POST");
    assert_eq!(woken.body, b"{}");
    assert!(woken.header("x-zeron-timestamp").is_some());
    assert_eq!(
        fake.state().box_calls,
        [
            ("box-1".to_string(), "wake".to_string()),
            ("box-1".to_string(), "status".to_string()),
            ("box-1".to_string(), "stop".to_string()),
        ]
    );

    let forged = CloudBox {
        wake_key: crate::sign::new_wake_key(),
        ..record.clone()
    };
    assert!(matches!(
        provider.wake(&forged).await.unwrap_err(),
        CloudError::BoxUnauthorized
    ));
    let unknown = CloudBox {
        id: "box-9".into(),
        ..record
    };
    assert!(matches!(
        provider.wake(&unknown).await.unwrap_err(),
        CloudError::BoxUnknown
    ));
}

#[tokio::test]
async fn destroying_boxes_shrinks_then_removes_the_deployment() {
    let (fake, server, provider) = setup(FakeState::default()).await;
    let enroller = enroller();
    let first = provider
        .provision(&request(Vec::new()), &enroller)
        .await
        .unwrap();
    let existing = vec![ExistingBox {
        record: first.record.clone(),
        credential: Some(first.credential.clone()),
    }];
    let second = provider
        .provision(&request(existing), &enroller)
        .await
        .unwrap();
    let all = vec![
        ExistingBox {
            record: local(&first.record, &server),
            credential: Some(first.credential),
        },
        ExistingBox {
            record: local(&second.record, &server),
            credential: Some(second.credential),
        },
    ];

    // One of two: the secret loses it, the app shrinks, everything stays.
    fake.state().bucket_objects = 3;
    provider.destroy(&all[1].record, &all, true).await.unwrap();
    {
        let state = fake.state();
        let secret: Value = serde_json::from_str(&state.secrets["ZERON_BOXES"]).unwrap();
        assert!(secret.get("box-2").is_none() && secret.get("box-1").is_some());
        assert_eq!(state.apps[0]["max_instances"], 1);
        assert_eq!(state.bucket_objects, 3, "keep_data keeps checkpoints");
        assert!(!state.box_calls.iter().any(|(_, a)| a == "purge"));
    }

    // The last one, without its data: purge, then app, Worker and bucket go.
    server.clear();
    provider
        .destroy(&all[0].record, &all[..1], false)
        .await
        .unwrap();
    let after: Vec<String> = lines(&server);
    assert_eq!(
        after,
        [
            "POST /v1/boxes/box-1/purge",
            "POST /v1/boxes/box-1/stop",
            "GET /accounts/acc-1/containers/applications?name=zeron-cloud",
            "DELETE /accounts/acc-1/containers/applications/app-1",
            "DELETE /accounts/acc-1/workers/scripts/zeron-cloud?force=true",
            "DELETE /accounts/acc-1/r2/buckets/zeron-cloud",
        ]
    );
    let state = fake.state();
    assert!(state.apps.is_empty() && state.script_tag.is_none() && !state.bucket);
}

#[test]
fn metadata_migrates_only_what_the_script_lacks() {
    let fresh = Cloudflare::worker_metadata("https://e", None);
    assert_eq!(fresh["migrations"]["new_tag"], "v1");
    assert!(fresh["migrations"].get("old_tag").is_none());
    assert!(
        Cloudflare::worker_metadata("https://e", Some("v1"))
            .get("migrations")
            .is_none()
    );
    let older = Cloudflare::worker_metadata("https://e", Some("v0"));
    assert_eq!(older["migrations"]["old_tag"], "v0");
}
