//! Offline screenshots of Settings → Devices → Cloud on the real shell, over a
//! fake Cloudflare (nothing leaves the machine). The first argument is the
//! output directory, the second the state to show:
//!
//!   connect  no token yet
//!   create   a token is connected, no box yet
//!   boxes    one sleeping box and one running box
//!
//!   cargo run -p zeron-ui --features appshots-fixture --example cloud-fixture -- /tmp/shots boxes
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zeron_cloud::mock::{FakeCloudflare, FakeState};
use zeron_proto::{CloudBox, CloudBoxState};
use zeron_rpc::methods;
use zeron_ui::*;

async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    directory: &std::path::Path,
    name: &str,
) -> anyhow::Result<()> {
    window.update(cx, |_, w, cx| {
        w.draw(cx).clear();
        w.render_to_image()?
            .save(directory.join(format!("{name}.png")))?;
        Ok(())
    })?
}

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn cloud_box(id: &str, name: &str, state: CloudBoxState, error: Option<&str>) -> CloudBox {
    CloudBox {
        id: id.into(),
        provider: "cloudflare".into(),
        name: name.into(),
        account_id: "acc-1".into(),
        worker_url: "https://zeron-cloud.example.workers.dev".into(),
        wake_key: "a2V5".into(),
        instance_type: "standard-2".into(),
        idle_minutes: 15,
        created_at: 1,
        state,
        state_at: 1,
        error: error.map(str::to_owned),
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    let shot = std::env::args().nth(2).unwrap_or_else(|| "boxes".into());
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let core = runtime.block_on(async {
        zeron_engine::EngineCore::assemble(
            &temp.path().join("engine"),
            Arc::new(zeron_engine::default_registry()),
            zeron_proto::HarnessId::ClaudeCode,
            None,
        )
    })?;
    // A fake Cloudflare, so connecting a token works offline.
    let cloudflare = runtime.block_on(FakeCloudflare::new(FakeState::default()).serve());
    core.cloud
        .set_token_store(zeron_engine::cloud_boxes::CloudTokenStore::file(
            temp.path().join("cloud-credentials.json"),
        ));
    core.cloud
        .set_api_base(&format!("{}/client/v4", cloudflare.url));
    let ipc_port = port();
    let _ipc = runtime.block_on(zeron_engine::serve_ipc(ipc_port, core.rpc_service()))?;
    if shot != "connect" {
        runtime.block_on(async {
            let client = zeron_rpc::memory_client(core.rpc_service());
            client
                .call(
                    methods::CLOUD_CONNECT,
                    serde_json::json!({ "apiToken": "good-token" }),
                )
                .await
        })?;
    }
    if shot == "boxes" {
        core.workspace.upsert_cloud_box(&cloud_box(
            "box-1",
            "Cloud",
            CloudBoxState::Asleep,
            None,
        ))?;
        core.workspace.upsert_cloud_box(&cloud_box(
            "box-2",
            "Cloud (GPU builds)",
            CloudBoxState::Running,
            None,
        ))?;
    }
    let data = temp.path().join("ui");
    std::fs::create_dir(&data)?;
    let boot = EngineBootConfig {
        data_dir: data.clone(),
        ipc_port,
        edge_url: String::new(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: zeron_proto::HarnessId::ClaudeCode,
    };
    let handle = runtime.block_on(state::EngineHandle::bootstrap(boot.clone()))?;
    let device = core.device_id.clone();
    let failure = Arc::new(std::sync::Mutex::new(None));
    let result = failure.clone();
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx);
        gpui_base::init(cx);
        let settings = settings::UiSettings::default();
        settings::init(settings.clone(), data.clone(), cx);
        let fonts = typography::register_fonts(cx);
        typography::init(
            settings.ui_font_family.clone(),
            settings.ui_font_size,
            settings.terminal_font_family.clone(),
            settings.terminal_font_size,
            settings.code_font_family.clone(),
            settings.code_font_size,
            fonts,
            cx,
        );
        theme_library::init(data.clone(), cx);
        appearance::init(
            appearance::AppearanceMode::Dark,
            settings.theme_selection,
            settings.accent,
            settings.surface,
            cx,
        );
        history::init(
            settings.git_history_columns,
            settings.git_history_column_widths,
            settings.git_history_column_order,
            settings.git_history_author_display,
            cx,
        );
        composer::init(cx, settings.composer_send_behavior);
        terminal::panel::init(cx);
        app_menus::init(cx);
        let state = cx.new(|_| {
            let mut s = state::AppState::new();
            s.fixture_attachment_engine(handle);
            s.connection = zeron_proto::view::ConnectionStatus::Ready;
            s.workspace_scope = Some(zeron_proto::WorkspaceScope::Development);
            s.local_device_id = Some(device.clone());
            s.devices = vec![serde_json::from_value(serde_json::json!({
                "id": device, "name": "This device",
                "platform": std::env::consts::OS, "lastSeenAt": null
            }))
            .unwrap()];
            s.chats_synced = true;
            s.spaces_synced = true;
            s.auto_selected = true;
            s
        });
        let window = cx
            .open_window(
                WindowOptions {
                    window_background: theme::Theme::of(cx).window_background_appearance(),
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        gpui::point(px(20.), px(40.)),
                        size(px(1100.), px(900.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
            )
            .unwrap();
        state.update(cx, |_, cx| cx.notify());
        cx.activate(true);
        cx.spawn(async move |cx| {
            let run: anyhow::Result<()> = async {
                pause(cx, 2000).await;
                window.update(cx, |s, _, cx| s.fixture_open_devices_settings(cx))?;
                pause(cx, 2500).await;
                capture(window.into(), cx, &output, &format!("cloud-{shot}"))?;
                Ok(())
            }
            .await;
            if let Err(error) = run {
                *result.lock().unwrap() = Some(error.to_string());
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    if let Some(error) = failure.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
