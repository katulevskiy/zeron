//! Isolated native review fixture for moving a chat to another device: the
//! "Move to…" picker and the move banner above the composer, over fake rows
//! (no engine round trip, nothing moves). ZERON_MOVE_SHOT = copying (default)
//! | waiting | done | failed | picker; ZERON_PALETTE_LIGHT selects the light
//! appearance.
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use zeron_proto::{ChatMove, MoveCandidate, MovePhase, MoveWhen};
use zeron_ui::*;

fn chat_move(shot: &str) -> Option<ChatMove> {
    let now = chrono::Utc::now().timestamp_millis();
    let mut m = ChatMove {
        id: "mv-fixture".into(),
        from_device_id: "mac".into(),
        to_device_id: "desk".into(),
        to_device_name: "Desktop".into(),
        phase: MovePhase::Copying,
        when: MoveWhen::SafePoint,
        detail: Some("Workspace, 2 extra folders and the Claude session".into()),
        bytes_total: 530_000_000,
        bytes_done: 214_000_000,
        files_total: 18_402,
        started_at: now - 40_000,
        finished_at: None,
        error: None,
        notes: Vec::new(),
    };
    match shot {
        "picker" => return None,
        "waiting" => {
            m.phase = MovePhase::Waiting;
            m.detail = Some("Bash: cargo build --release, 4m".into());
        }
        "done" => {
            m.phase = MovePhase::Done;
            m.finished_at = Some(now - 1_000);
            m.notes = vec![
                "The branch landed as feature/release-2: feature/release was taken there".into(),
                "A dev server (npm run dev) was running here and didn't come along".into(),
            ];
        }
        "failed" => {
            m.phase = MovePhase::Failed;
            m.finished_at = Some(now - 5_000);
            m.error = Some(
                "Desktop ran out of disk space (needs 530 MB, has 210 MB). The chat stayed \
                 on MacBook Pro."
                    .into(),
            );
        }
        _ => {}
    }
    Some(m)
}

fn candidates() -> Vec<MoveCandidate> {
    let candidate = |id: &str, name: &str| MoveCandidate {
        device_id: id.into(),
        device_name: name.into(),
        online: true,
        supported: true,
        harness_installed: true,
        harness_signed_in: None,
        problem: None,
    };
    let mut desk = candidate("desk", "Desktop");
    desk.harness_signed_in = Some(true);
    let mut gpu = candidate("gpu", "GPU box");
    gpu.harness_signed_in = Some(false);
    let mut nas = candidate("nas", "Attic NAS");
    nas.online = false;
    nas.problem = Some("Offline".into());
    let mut pi = candidate("pi", "Raspberry Pi");
    pi.harness_installed = false;
    pi.problem = Some("Claude Code isn't installed there".into());
    let mut old = candidate("old", "Old ThinkPad");
    old.supported = false;
    old.problem = Some("Needs a newer Zeron".into());
    vec![desk, gpu, nas, pi, old]
}

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let shot = std::env::var("ZERON_MOVE_SHOT").unwrap_or_else(|_| "copying".into());
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx);
        gpui_base::init(cx);
        let mut settings = settings::UiSettings::default();
        settings.sidebar_width = 280.0;
        settings.surface = zeron_theme::SurfacePreference::Frosted;
        settings.save(&data).unwrap();
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
            if std::env::var_os("ZERON_PALETTE_LIGHT").is_some() {
                appearance::AppearanceMode::Light
            } else {
                appearance::AppearanceMode::Dark
            },
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
        let mv = zeron_proto::capabilities::SESSION_MOVE_V1;
        let now = chrono::Utc::now();
        let picker = shot == "picker";
        let move_state = chat_move(&shot);
        let state = cx.new(|_| {
            let mut s = state::AppState::new();
            s.connection = zeron_proto::view::ConnectionStatus::Ready;
            s.workspace_scope = Some(zeron_proto::WorkspaceScope::Synced);
            s.auth = Some(zeron_proto::AuthState::SignedIn {
                user: zeron_proto::UserProfile {
                    id: "fixture-user".into(),
                    email: "alex@example.test".into(),
                    name: Some("Alex".into()),
                },
                org_id: Some("fixture-org".into()),
            });
            s.local_device_id = Some("desk".into());
            s.devices = [
                ("desk", "Desktop", "linux"),
                ("mac", "MacBook Pro", "macos"),
                ("gpu", "GPU box", "linux"),
                ("nas", "Attic NAS", "linux"),
                ("pi", "Raspberry Pi", "linux"),
            ]
            .into_iter()
            .map(|(id, name, platform)| {
                serde_json::from_value(serde_json::json!({
                    "id": id, "name": name, "platform": platform,
                    "lastSeenAt": now, "capabilities": [mv],
                }))
                .unwrap()
            })
            .collect();
            s.selected_chat = Some("fixture".into());
            s.selected_space = Some("project".into());
            s.auto_selected = true;
            s.chats_synced = true;
            s.spaces_synced = true;
            s.spaces = vec![serde_json::from_value(serde_json::json!({"id":"project","deviceId":"mac","path":"/Users/alex/fieldnotes","createdAt":"2026-09-08T00:00:00Z"})).unwrap()];
            let chat = |id: &str, title: &str| -> zeron_proto::Chat {
                serde_json::from_value(serde_json::json!({"id":id,"deviceId":"mac","spaceId":"project","cwd":"/Users/alex/fieldnotes","title":title,"archived":false,"createdAt":"2026-09-08T00:00:00Z","config":{"harness":"claude-code","model":"claude-sonnet-4-6","reasoning":null,"sandbox":"workspace-write"}})).unwrap()
            };
            let mut selected = chat("fixture", "Ship the release build");
            selected.move_state = move_state.clone();
            let mut other = chat("other", "Profile the importer");
            other.move_state = chat_move("waiting").map(|mut m| {
                m.id = "mv-other".into();
                m.to_device_name = "GPU box".into();
                m
            });
            s.chats = vec![selected, other, chat("third", "Fix flaky sync test")];
            s
        });
        let boot = EngineBootConfig {
            data_dir: data,
            ipc_port: 0,
            edge_url: String::new(),
            edge_token: None,
            org_id: None,
            workos_client_id: None,
            default_harness: HarnessId::ClaudeCode,
        };
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        gpui::point(px(0.), px(0.)),
                        size(px(1100.), px(760.)),
                    ))),
                    app_owns_titlebar_drag: true,
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
            )
            .unwrap();
        state.update(cx, |_, cx| cx.notify());
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(700))
                .await;
            if picker {
                window
                    .update(cx, |shell, _, cx| {
                        shell.fixture_move_picker(
                            "fixture",
                            candidates(),
                            None,
                            gpui::point(px(720.), px(44.)),
                            cx,
                        )
                    })
                    .ok();
            }
        })
        .detach();
    });
    Ok(())
}
