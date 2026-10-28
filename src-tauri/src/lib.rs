mod agents;
mod session_watch;
mod usage;

pub fn run_usage_collector_if_requested() -> Option<i32> {
    usage::collector::run_if_requested()
}

#[tauri::command]
fn collector_status(service: tauri::State<usage::service::Service>) -> Result<usage::service::Status, String> {
    service.status()
}
#[tauri::command]
async fn set_collector_enabled(app: tauri::AppHandle, enabled: bool) -> Result<usage::service::Status, String> {
    run_backend(move || app.state::<usage::service::Service>().set_enabled(enabled)).await
}
#[tauri::command]
fn open_collector_settings(service: tauri::State<usage::service::Service>) -> Result<(), String> {
    service.open_settings()
}

use agents::profiles::workspace::{ChangeKind, ChangePreview, Validation, Workspace};
use agents::profiles::{CurrentConfig, ProfileSummary};
use agents::{AgentInfo, Catalog, SessionDetail};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use tauri::{Emitter, Manager};

/// 位 0 为未保存草稿，位 1 为进行中的配置操作。原生关闭与退出均需复核。
#[derive(Default)]
struct ConfigGuard(AtomicU8);

#[cfg(target_os = "macos")]
fn install_guarded_quit(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem};
    let menu = Menu::default(app)?;
    let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.into_iter().next() else {
        return Err(std::io::Error::other("Default application menu was not found").into());
    };
    let quit_text = PredefinedMenuItem::quit(app, None)?.text()?;
    let mut position = None;
    for (index, item) in app_menu.items()?.iter().enumerate() {
        if let MenuItemKind::Predefined(item) = item {
            if item.text()? == quit_text {
                position = Some(index);
                break;
            }
        }
    }
    let position =
        position.ok_or_else(|| std::io::Error::other("Default Quit menu item was not found"))?;
    // 预定义 Quit 在 macOS 上直接发 terminate:，不会先经过 ExitRequested。
    // 仅替换此项，保留系统的编辑、窗口、服务菜单和原有快捷键。
    app_menu.remove_at(position)?;
    let quit = MenuItem::with_id(
        app,
        "asm-guarded-quit",
        quit_text,
        true,
        Some("CmdOrCtrl+Q"),
    )?;
    app_menu.insert(&quit, position)?;
    app.set_menu(menu)?;
    Ok(())
}

#[tauri::command]
fn set_config_guard(state: tauri::State<'_, ConfigGuard>, dirty: bool, busy: bool) {
    state
        .0
        .store(u8::from(dirty) | (u8::from(busy) << 1), Ordering::SeqCst);
}

#[tauri::command]
fn confirm_config_exit(
    app: tauri::AppHandle,
    state: tauri::State<'_, ConfigGuard>,
) -> Result<(), String> {
    if state.0.load(Ordering::SeqCst) & 2 != 0 {
        return Err("配置操作正在处理中，请稍后再退出".into());
    }
    state.0.store(0, Ordering::SeqCst);
    app.exit(0);
    Ok(())
}

// 同步命令会在窗口线程执行。扫描、文件锁与外部进程都可能等待，
// 必须放进阻塞线程池，让慢 I/O 期间窗口仍可切页、刷新或关闭。
async fn run_backend<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| format!("后台操作失败: {error}"))?
}

#[tauri::command]
async fn list_agents() -> Result<Vec<AgentInfo>, String> {
    run_backend(|| Ok(agents::list_agents())).await
}

static USAGE_GENERATION: AtomicU64 = AtomicU64::new(0);
static EVENT_GENERATION: AtomicU64 = AtomicU64::new(0);
static SUMMARY_GENERATION: AtomicU64 = AtomicU64::new(0);

#[tauri::command]
async fn sync_request_events(
    app: tauri::AppHandle,
    agent: String,
    generation: u64,
) -> Result<usage::summary::Synced, String> {
    USAGE_GENERATION.fetch_max(generation, Ordering::SeqCst);
    run_backend(move || {
        let last = std::cell::RefCell::new((std::time::Instant::now(), ""));
        usage::summary::sync_with_progress(
            &agent,
            &|| USAGE_GENERATION.load(Ordering::SeqCst) != generation,
            &|progress| {
                let mut last = last.borrow_mut();
                if progress.phase != last.1
                    || last.0.elapsed().as_millis() >= 200
                    || progress.total_files == Some(progress.checked_files)
                {
                    *last = (std::time::Instant::now(), progress.phase);
                    let _ = app.emit(
                        "usage-sync-progress",
                        serde_json::json!({ "generation": generation, "progress": progress }),
                    );
                }
            },
        )
    })
    .await
}

#[tauri::command]
async fn read_usage_summary(
    query: usage::summary::SummaryQuery,
    generation: u64,
) -> Result<usage::summary::Summary, String> {
    SUMMARY_GENERATION.fetch_max(generation, Ordering::SeqCst);
    run_backend(move || {
        usage::summary::read(query, &|| {
            SUMMARY_GENERATION.load(Ordering::SeqCst) != generation
        })
    })
    .await
}

#[tauri::command]
fn cancel_usage_summary(generation: u64) {
    SUMMARY_GENERATION.fetch_max(generation, Ordering::SeqCst);
}

#[tauri::command]
async fn read_request_events(
    query: usage::events::EventQuery,
    generation: u64,
) -> Result<usage::events::EventPage, String> {
    EVENT_GENERATION.fetch_max(generation, Ordering::SeqCst);
    run_backend(move || {
        usage::events::read(query, &|| {
            EVENT_GENERATION.load(Ordering::SeqCst) != generation
        })
    })
    .await
}

#[tauri::command]
fn cancel_request_events(generation: u64) {
    EVENT_GENERATION.fetch_max(generation, Ordering::SeqCst);
}

#[tauri::command]
fn cancel_usage(generation: u64) {
    USAGE_GENERATION.fetch_max(generation, Ordering::SeqCst);
}

#[tauri::command]
async fn catalog(agent: String, include_empty: Option<bool>) -> Result<Catalog, String> {
    run_backend(move || {
        let source = agents::find_source(&agent)?;
        let catalog = source.quick_catalog(include_empty.unwrap_or(false))?;
        agents::search::register(source.as_ref(), &catalog);
        Ok(catalog)
    }).await
}

// 同一时间只运行一次精确统计。切换上下文时取消后续文件读取，首屏不等待此锁。
static REFINEMENT: AtomicU64 = AtomicU64::new(0);
#[tauri::command]
fn cancel_catalog_refinement(generation: u64) { REFINEMENT.fetch_max(generation, Ordering::SeqCst); }
#[tauri::command]
async fn refine_catalog(agent: String, include_empty: bool, generation: u64) -> Result<Catalog, String> {
    REFINEMENT.fetch_max(generation, Ordering::SeqCst);
    run_backend(move || {
        static RUNNING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        let cancelled = || REFINEMENT.load(Ordering::SeqCst) != generation;
        if cancelled() { return Err("目录更新已取消".into()); }
        let source = agents::find_source(&agent)?;
        let catalog = source.refine_catalog(include_empty, &cancelled)?;
        if cancelled() { return Err("目录更新已取消".into()); }
        agents::search::register(source.as_ref(), &catalog);
        Ok(catalog)
    }).await
}
#[tauri::command]
async fn read_tool_payload(token: String) -> Result<serde_json::Value, String> {
    run_backend(move || agents::read_tool_payload(&token)).await
}

#[tauri::command]
async fn resolve_model_names(agent: String, models: Vec<String>) -> Result<std::collections::HashMap<String, String>, String> {
    if models.len() > 256 || models.iter().any(|id| id.len() > 512 || id.chars().any(char::is_control)) {
        return Err("模型标识数量或长度超出限制".into());
    }
    run_backend(move || Ok(agents::find_source(&agent)?.resolve_model_names(&models))).await
}

#[tauri::command]
async fn watch_sessions(app: tauri::AppHandle, state: tauri::State<'_, session_watch::SessionWatcher>,
    agent: Option<String>, generation: u64) -> Result<(), String> {
    let watcher = state.inner().clone();
    run_backend(move || watcher.select(agent, generation, move |signal| {
        let _ = app.emit("session-files-changed", signal);
    })).await
}

#[tauri::command]
async fn search_conversations(agent: String, targets: Vec<agents::search::SessionRef>, query: String) -> Result<agents::search::SearchResult, String> {
    let ticket = agents::search::start();
    run_backend(move || agents::search::search(agents::find_source(&agent)?.as_ref(), &targets, &query, ticket)).await
}

#[tauri::command]
fn cancel_conversation_search() { agents::search::cancel(); }

#[tauri::command]
async fn read_session(
    agent: String,
    project_dir: String,
    session_id: String,
    view: Option<bool>,
) -> Result<SessionDetail, String> {
    run_backend(move || {
        let source = agents::find_source(&agent)?;
        if view.unwrap_or(false) { source.read_session_view(&project_dir, &session_id) }
        else { source.read_session(&project_dir, &session_id) }
    }).await
}

#[tauri::command]
async fn delete_session(
    agent: String,
    project_dir: String,
    session_id: String,
) -> Result<(), String> {
    run_backend(move || usage::preserve_before_delete(&agent, || {
        agents::find_source(&agent)?.delete_session(&project_dir, &session_id)
    }))
        .await
}

#[tauri::command]
async fn resume_command(
    agent: String,
    session_id: String,
    fork: Option<bool>,
) -> Result<Option<String>, String> {
    run_backend(move || {
        Ok(agents::find_source(&agent)
            .ok()
            .and_then(|source| source.resume_command(&session_id, fork.unwrap_or(false))))
    })
    .await
}

#[tauri::command]
async fn resume_in_terminal(
    agent: String,
    session_id: String,
    cwd: Option<String>,
    fork: Option<bool>,
) -> Result<(), String> {
    run_backend(move || {
        let source = agents::find_source(&agent)?;
        let cmd = source
            .resume_command(&session_id, fork.unwrap_or(false))
            .ok_or_else(|| "该 agent 不支持恢复会话".to_string())?;
        let full = match cwd {
            Some(dir) => format!("cd {} && {}", agents::shell_quote(&dir), cmd),
            None => cmd,
        };
        agents::run_in_terminal(&full)
    })
    .await
}

#[tauri::command]
async fn get_current_config(agent: String) -> Result<CurrentConfig, String> {
    run_backend(move || agents::profiles::get_current_config(&agent)).await
}

#[tauri::command]
async fn list_profiles(agent: String) -> Result<Vec<ProfileSummary>, String> {
    run_backend(move || agents::profiles::list_profiles(&agent)).await
}

#[tauri::command]
async fn save_profile(
    agent: String,
    name: String,
    expected_revision: String,
) -> Result<ProfileSummary, String> {
    run_backend(move || agents::profiles::save_profile(&agent, name, &expected_revision)).await
}

#[tauri::command]
async fn get_config_workspace(agent: String) -> Result<Workspace, String> {
    run_backend(move || agents::profiles::workspace::get_workspace(&agent)).await
}

#[tauri::command]
async fn validate_config_content(
    agent: String,
    key: String,
    content: String,
) -> Result<Validation, String> {
    run_backend(move || agents::profiles::workspace::validate_content(&agent, &key, &content)).await
}

#[tauri::command]
async fn preview_config_change(
    agent: String,
    kind: ChangeKind,
    target: String,
) -> Result<ChangePreview, String> {
    run_backend(move || agents::profiles::workspace::preview_change(&agent, kind, &target)).await
}

#[tauri::command]
async fn apply_config_change(
    agent: String,
    kind: ChangeKind,
    target: String,
    expected_current: String,
    expected_target: String,
) -> Result<(), String> {
    run_backend(move || {
        agents::profiles::workspace::apply_change(
            &agent,
            kind,
            &target,
            &expected_current,
            &expected_target,
        )
    })
    .await
}

#[tauri::command]
async fn delete_profile(agent: String, name: String) -> Result<(), String> {
    run_backend(move || agents::profiles::delete_profile(&agent, name)).await
}

#[tauri::command]
async fn update_config_file(
    agent: String,
    key: String,
    content: String,
    expected_revision: String,
) -> Result<(), String> {
    run_backend(move || {
        agents::profiles::workspace::update_file(&agent, &key, &content, &expected_revision)
    })
    .await
}

#[cfg(test)]
mod command_tests {
    use super::run_backend;
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };

    #[test]
    fn waiting_backend_does_not_block_the_command_caller() {
        let caller = std::thread::current().id();
        let (release, wait) = std::sync::mpsc::channel();
        let mut request = Box::pin(run_backend(move || {
            assert_ne!(std::thread::current().id(), caller);
            wait.recv_timeout(std::time::Duration::from_secs(3))
                .map_err(|e| e.to_string())?;
            Ok(42)
        }));
        // 后端尚未收到放行信号时，首次轮询必须归还控制权给窗口线程。
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(request.as_mut().poll(&mut context), Poll::Pending));
        release.send(()).unwrap();
        assert_eq!(tauri::async_runtime::block_on(request).unwrap(), 42);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(ConfigGuard::default())
        .manage(session_watch::SessionWatcher::default())
        .manage(usage::service::Service::default())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            install_guarded_quit(app.handle())?;
            let service_app = app.handle().clone();
            let generation = USAGE_GENERATION.load(Ordering::SeqCst);
            tauri::async_runtime::spawn_blocking(move || {
                if service_app.state::<usage::service::Service>().ensure().is_err() {
                    eprintln!("Background token collector registration failed; inspect usage service status");
                }
                // 每次启动只同步一次，不依赖可选的登录服务。
                let cancelled = || USAGE_GENERATION.load(Ordering::SeqCst) != generation;
                for agent in usage::AGENTS {
                    // 用量页接管同步后停止启动任务，避免冷启动重复读取大历史。
                    if cancelled() {
                        break;
                    }
                    if usage::summary::sync(agent, &cancelled).is_err() && !cancelled() {
                        eprintln!("Request sync failed for {agent}; open the usage page to retry");
                    }
                }
            });
            tauri::async_runtime::spawn_blocking(|| {
                if agents::profiles::recover_pending_config_transactions().is_err() {
                    eprintln!("Configuration recovery failed; open the configuration workspace to inspect the conflict");
                }
            });
            Ok(())
        })
        .on_menu_event(|app, event| {
            if event.id().as_ref() == "asm-guarded-quit" {
                if app.state::<ConfigGuard>().0.load(Ordering::SeqCst) != 0 {
                    let _ = app.emit("config-exit-requested", ());
                } else {
                    app.exit(0);
                }
            }
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(focused) = event {
                if *focused { window.state::<usage::service::Service>().check(); }
                window.state::<session_watch::SessionWatcher>().set_foreground(*focused);
                let _ = window.emit("session-window-focus", focused);
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.state::<ConfigGuard>().0.load(Ordering::SeqCst) != 0 {
                    api.prevent_close();
                    let _ = window.emit("config-exit-requested", ());
                }
            }
        })
        // 单实例：第二次启动只聚焦已有窗口（UX 兜底）。
        // 注意：该插件在 macOS 上先探测 socket 再异步建监听，存在双进程
        // 都通过探测的竞态，不能保证互斥；配置库的跨进程互斥由
        // profiles 模块的 flock 排他锁保证（见 store_lock）
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            refine_catalog, cancel_catalog_refinement, read_tool_payload,
            list_agents,
            catalog,
            sync_request_events, read_usage_summary, cancel_usage_summary,
            read_request_events,
            cancel_request_events,
            cancel_usage,
            collector_status,
            set_collector_enabled,
            open_collector_settings,
            resolve_model_names,
            watch_sessions,
            search_conversations,
            cancel_conversation_search,
            read_session,
            delete_session,
            resume_command,
            resume_in_terminal,
            get_current_config,
            list_profiles,
            save_profile,
            get_config_workspace,
            validate_config_content,
            preview_config_change,
            apply_config_change,
            set_config_guard,
            confirm_config_exit,
            delete_profile,
            update_config_file
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // 兜底处理运行时发起的退出；用户确认退出的命令已先清除 guard。
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                if app.state::<ConfigGuard>().0.load(Ordering::SeqCst) != 0 {
                    api.prevent_exit();
                    let _ = app.emit("config-exit-requested", ());
                }
            }
        });
}
