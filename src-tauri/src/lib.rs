mod agents;
mod session_catalog;
mod session_watch;
mod usage;

pub fn run_usage_collector_if_requested() -> Option<i32> {
    usage::collector::run_if_requested()
}

use agents::profiles::workspace::{ChangeKind, ChangePreview, Validation, Workspace};
use agents::profiles::{CurrentConfig, ProfileSummary};
use agents::{AgentInfo, CatalogFilter, SessionDetail};
use session_catalog::{SessionCatalogs, Snapshot};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use tauri::{Emitter, Manager};

/// 位 0 表示有未保存的草稿，位 1 表示正在修改配置。
#[derive(Default)]
struct ConfigEditGuard(AtomicU8);

/// 从第一条删除前持续到整批结束；配置编辑的状态更新不会覆盖它。
#[derive(Default)]
struct BatchDeleteGuard(AtomicBool);

fn exit_blocked(app: &tauri::AppHandle) -> bool {
    if app.state::<BatchDeleteGuard>().0.load(Ordering::SeqCst) {
        return true;
    }
    if app.state::<ConfigEditGuard>().0.load(Ordering::SeqCst) != 0 {
        let _ = app.emit("config-exit-requested", ());
        return true;
    }
    false
}

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
    // macOS 的默认退出项可能绕过 ExitRequested。
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
fn set_config_guard(state: tauri::State<'_, ConfigEditGuard>, dirty: bool, busy: bool) {
    state
        .0
        .store(u8::from(dirty) | (u8::from(busy) << 1), Ordering::SeqCst);
}

#[tauri::command]
fn set_batch_delete_guard(state: tauri::State<'_, BatchDeleteGuard>, active: bool) {
    state.0.store(active, Ordering::SeqCst);
}

#[tauri::command]
fn confirm_config_exit(
    app: tauri::AppHandle,
    state: tauri::State<'_, ConfigEditGuard>,
) -> Result<(), String> {
    if app.state::<BatchDeleteGuard>().0.load(Ordering::SeqCst) {
        return Err("批量删除仍在进行，请稍后退出。".into());
    }
    if state.0.load(Ordering::SeqCst) & 2 != 0 {
        return Err("配置操作仍在进行，请稍后退出。".into());
    }
    state.0.store(0, Ordering::SeqCst);
    app.exit(0);
    Ok(())
}

// 文件 I/O 和外部命令在线程池执行，避免阻塞窗口线程。
async fn run_backend<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| format!("后台操作失败：{error}"))?
}

#[tauri::command]
async fn list_agents() -> Result<Vec<AgentInfo>, String> {
    run_backend(|| Ok(agents::list_agents())).await
}

#[tauri::command]
async fn count_sessions(
    agent: String,
    include_empty: bool,
    include_archived: bool,
) -> Result<Option<usize>, String> {
    run_backend(move || {
        let source = agents::find_source(&agent)?;
        let filter = CatalogFilter {
            include_empty,
            include_archived: include_archived && source.supports_archived_sessions(),
        };
        let catalog = source.scan_catalog(filter, &|| false)?;
        Ok(catalog.warnings.is_empty().then_some(catalog.sessions.len()))
    })
    .await
}

#[tauri::command]
async fn open_data_dir(agent: String) -> Result<(), String> {
    run_backend(move || {
        let path = agents::data_directory(&agent)?;
        tauri_plugin_opener::open_path(path, None::<&str>)
            .map_err(|error| format!("无法打开文件管理器：{error}"))
    })
    .await
}

static EVENT_GENERATION: AtomicU64 = AtomicU64::new(0);
static SUMMARY_GENERATION: AtomicU64 = AtomicU64::new(0);

#[tauri::command]
fn usage_sync_status(collector: tauri::State<'_, usage::collector::Desktop>) -> usage::collector::SyncStatus {
    collector.status()
}

#[tauri::command]
fn check_usage_now(collector: tauri::State<'_, usage::collector::Desktop>) {
    collector.check_now();
}

#[tauri::command]
fn recollect_usage(
    agent: String,
    collector: tauri::State<'_, usage::collector::Desktop>,
) -> Result<usage::collector::SyncStatus, String> {
    collector.recollect(agent)
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
async fn clear_orphan_requests(
    query: usage::events::CleanupQuery,
    collector: tauri::State<'_, usage::collector::Desktop>,
) -> Result<usize, String> {
    let removed = run_backend(move || usage::events::clear_orphans(query)).await?;
    if removed > 0 {
        collector.ledger_changed();
    }
    Ok(removed)
}

#[tauri::command]
async fn get_model_prices(
    agent: Option<String>,
) -> Result<usage::pricing::ModelPricesResponse, String> {
    run_backend(move || usage::pricing::list_model_prices(agent.as_deref())).await
}

#[tauri::command]
async fn set_custom_price(
    app: tauri::AppHandle,
    input: usage::pricing::CustomPriceInput,
) -> Result<usage::pricing::PricingCatalogStatus, String> {
    run_backend(move || {
        let status = usage::pricing::set_custom_price(input)?;
        let _ = app.emit("usage-pricing-updated", status.revision);
        Ok(status)
    })
    .await
}

#[tauri::command]
async fn delete_custom_price(
    app: tauri::AppHandle,
    model: String,
) -> Result<usage::pricing::PricingCatalogStatus, String> {
    run_backend(move || {
        let status = usage::pricing::delete_custom_price(&model)?;
        let _ = app.emit("usage-pricing-updated", status.revision);
        Ok(status)
    })
    .await
}

#[tauri::command]
async fn refresh_reference_prices(
    app: tauri::AppHandle,
) -> Result<usage::pricing::PricingCatalogStatus, String> {
    run_backend(move || {
        let status = usage::pricing::refresh_reference_prices();
        let _ = app.emit("usage-pricing-updated", status.revision);
        Ok(status)
    })
    .await
}

#[tauri::command]
async fn search_reference_prices(
    query: String,
    limit: Option<usize>,
) -> Result<Vec<usage::pricing::ReferenceSearchItem>, String> {
    run_backend(move || Ok(usage::pricing::search_reference_prices(&query, limit.unwrap_or(30))))
        .await
}

#[tauri::command]
async fn catalog(
    state: tauri::State<'_, std::sync::Arc<SessionCatalogs>>,
    agent: String,
    include_empty: bool,
    include_archived: Option<bool>,
    generation: u64,
    revision: Option<u64>,
) -> Result<Option<Snapshot>, String> {
    let state = state.inner().clone();
    run_backend(move || {
        state.load(
            agents::find_source(&agent)?.as_ref(),
            CatalogFilter {
                include_empty,
                include_archived: include_archived.unwrap_or(false),
            },
            generation,
            revision,
        )
    })
    .await
}

#[tauri::command]
async fn cancel_catalog(
    state: tauri::State<'_, std::sync::Arc<SessionCatalogs>>,
    generation: u64,
) -> Result<(), String> {
    let state = state.inner().clone();
    run_backend(move || {
        state.cancel(generation);
        Ok(())
    }).await
}

#[tauri::command]
async fn refine_catalog(
    state: tauri::State<'_, std::sync::Arc<SessionCatalogs>>,
    agent: String,
    include_empty: bool,
    include_archived: Option<bool>,
    generation: u64,
    revision: Option<u64>,
) -> Result<Option<Snapshot>, String> {
    let state = state.inner().clone();
    run_backend(move || {
        state.refine(
            agents::find_source(&agent)?.as_ref(),
            CatalogFilter {
                include_empty,
                include_archived: include_archived.unwrap_or(false),
            },
            generation,
            revision,
        )
    })
    .await
}

#[tauri::command]
async fn read_tool_payload(token: String) -> Result<serde_json::Value, String> {
    run_backend(move || agents::read_tool_payload(&token)).await
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
) -> Result<agents::SessionDeletion, String> {
    run_backend(move || {
        let source = agents::find_source(&agent)?;
        if source.delete_policy() == agents::DeletePolicy::ReadOnly {
            return Err("该 Agent 的会话为只读".into());
        }
        if agent == "codex" {
            let home = source.data_dir().ok_or("缺少 Codex 数据目录")?;
            let plan = agents::codex::deletion::Plan::for_session(&home, &project_dir, &session_id)?;
            let failure_history = usage::codex_failure_database(&home);
            return plan.run(&failure_history, &|files, validate, mutate| {
                usage::preserve_codex_deletion(files, validate, mutate)
            });
        }
        let targets = source.deletion_targets(&project_dir, &session_id)?;
        usage::preserve_before_delete(&agent, targets.as_deref(), || {
            if targets.is_some() && source.deletion_targets(&project_dir, &session_id)? != targets {
                return Err("会话文件已变化，请重新确认后删除".into());
            }
            source.delete_session(&project_dir, &session_id)
        })?;
        Ok(agents::SessionDeletion {
            removed: vec![agents::search::SessionRef { project_dir, session_id }],
            error: None,
        })
    })
    .await
}

#[tauri::command]
async fn set_session_archived(
    agent: String,
    project_dir: String,
    session_id: String,
    archived: bool,
) -> Result<ArchiveOutcome, String> {
    run_backend(move || {
        let source = agents::find_source(&agent)?;
        if agent == "codex" {
            let before = agents::codex::archive_snapshot()?.into_iter().map(|change| (change.session, change.archived))
                .collect::<std::collections::HashMap<_, _>>();
            let result = source.set_session_archived(&project_dir, &session_id, archived);
            let changed = agents::codex::archive_snapshot()?.into_iter()
                .filter(|change| before.get(&change.session) == Some(&!change.archived)).collect();
            return Ok(ArchiveOutcome { changed, error: result.err() });
        }
        source.set_session_archived(&project_dir, &session_id, archived)?;
        Ok(ArchiveOutcome { changed: vec![agents::ArchiveChange {
            session: agents::search::SessionRef { project_dir, session_id },
            archived,
        }], error: None })
    })
    .await
}

#[derive(serde::Serialize)]
struct ArchiveOutcome {
    changed: Vec<agents::ArchiveChange>,
    error: Option<String>,
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
            .ok_or_else(|| "该 Agent 不支持恢复会话".to_string())?;
        let full = match cwd {
            Some(dir) => {
                if !std::path::Path::new(&dir).is_dir() {
                    return Err(format!("原工作目录不可用，无法恢复会话：{dir}"));
                }
                format!("cd -- {} && {}", agents::shell_quote(&dir), cmd)
            }
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
        .manage(ConfigEditGuard::default())
        .manage(BatchDeleteGuard::default())
        .manage(session_watch::SessionWatcher::default())
        .manage(std::sync::Arc::new(SessionCatalogs::default()))
        .setup(|app| {
            #[cfg(target_os = "macos")]
            install_guarded_quit(app.handle())?;
            let sync_app = app.handle().clone();
            app.manage(usage::collector::Desktop::start(move |status| {
                let _ = sync_app.emit("usage-sync-status", status);
            }));
            tauri::async_runtime::spawn_blocking(|| {
                if usage::service::ensure_login_service().is_err() {
                    eprintln!("Could not register the background usage collector.");
                }
            });
            tauri::async_runtime::spawn_blocking(|| {
                if agents::profiles::recover_pending_config_transactions().is_err() {
                    eprintln!("Configuration recovery failed; open the configuration workspace to inspect the conflict");
                }
            });
            let pricing_app = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Some(status) = usage::pricing::refresh_if_stale() {
                    let _ = pricing_app.emit("usage-pricing-updated", status.revision);
                }
            });
            Ok(())
        })
        .on_menu_event(|app, event| {
            if event.id().as_ref() == "asm-guarded-quit" && !exit_blocked(app) {
                app.exit(0);
            }
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(focused) = event {
                if *focused { usage::service::check_login_service(); }
                window.state::<session_watch::SessionWatcher>().set_foreground(*focused);
                let _ = window.emit("session-window-focus", focused);
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if exit_blocked(window.app_handle()) {
                    api.prevent_close();
                }
            }
        })
        // 单实例插件只负责聚焦窗口，配置库通过 flock 防止跨进程并发写入。
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
            refine_catalog, cancel_catalog, read_tool_payload,
            list_agents,
            open_data_dir,
            catalog,
            usage_sync_status, check_usage_now, recollect_usage, read_usage_summary, cancel_usage_summary, count_sessions,
            read_request_events,
            cancel_request_events,
            clear_orphan_requests,
            get_model_prices,
            set_custom_price,
            delete_custom_price,
            refresh_reference_prices,
            search_reference_prices,
            watch_sessions,
            search_conversations,
            cancel_conversation_search,
            read_session,
            delete_session,
            set_session_archived,
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
            set_batch_delete_guard,
            confirm_config_exit,
            delete_profile,
            update_config_file
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                app.state::<usage::collector::Desktop>().stop();
            }
            // 外部退出请求也要遵守与窗口和菜单相同的保护。
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                if exit_blocked(app) {
                    api.prevent_exit();
                }
            }
        });
}
