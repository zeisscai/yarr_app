#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! yarr 桌面壳。
//!
//! 职责边界（刻意保持很薄）：
//!   - 窗口、托盘、菜单、单实例、自动更新
//!   - 拉起/看护 yarr 后端进程（见 sidecar.rs）
//!   - 拦截站外链接，交给系统浏览器
//!
//! 不负责：任何业务逻辑、数据库访问、前端渲染。
//! 因此上游 yarr 更新时，这个 crate 基本不需要跟着改。

mod sidecar;

use std::path::Path;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use sidecar::AppState;

// ---------------------------------------------------------------------------
// 前端可调用的命令（仅本地状态页会调用，远端页面没有 IPC 权限）
// ---------------------------------------------------------------------------

#[tauri::command]
fn boot_state(state: tauri::State<'_, Arc<AppState>>) -> sidecar::BootState {
    state.snapshot()
}

#[tauri::command]
fn retry_boot(app: AppHandle, state: tauri::State<'_, Arc<AppState>>) {
    sidecar::start_boot(app, state.inner().clone());
}

#[tauri::command]
fn open_log(app: AppHandle, state: tauri::State<'_, Arc<AppState>>) {
    open_path(&app, &state.log_path);
}

#[tauri::command]
fn open_data_dir(app: AppHandle, state: tauri::State<'_, Arc<AppState>>) {
    if let Some(dir) = state.db_path.parent() {
        open_path(&app, dir);
    }
}

// ---------------------------------------------------------------------------
// 系统集成
// ---------------------------------------------------------------------------

/// 用系统默认程序打开文件/目录（不引入额外插件，行为可预期）。
fn open_path(_app: &AppHandle, path: &Path) {
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = Command::new("xdg-open");

    let _ = cmd.arg(path).spawn();
}

fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = Command::new("xdg-open");

    let _ = cmd.arg(url).spawn();
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn is_local_url(url: &tauri::Url) -> bool {
    match url.scheme() {
        "tauri" | "about" | "data" | "blob" => true,
        "http" | "https" => matches!(url.host_str(), Some("tauri.localhost")),
        _ => false,
    }
}

/// 站外链接一律交给系统浏览器。
/// 这一步是桌面阅读器的核心语义：上游前端用的是普通 <a href>，不拦截会把整个应用导航走。
fn handle_navigation(app: &AppHandle, url: &tauri::Url, port: u16) -> bool {
    if is_local_url(url) {
        return true;
    }
    let is_yarr = matches!(url.host_str(), Some("127.0.0.1") | Some("localhost"))
        && url.port().map(|p| p == port).unwrap_or(false);

    if is_yarr {
        return true;
    }

    match url.scheme() {
        "http" | "https" | "mailto" => {
            // 留一条日志：外链转浏览器出问题时，这是唯一能确认「拦截确实发生了」的线索
            if let Some(st) = app.try_state::<Arc<AppState>>() {
                st.sink
                    .push(format!("[desktop] 外链交给系统浏览器: {url}"));
            }
            open_url(url.as_str())
        }
        _ => {}
    }
    false
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "打开主窗口", true, None::<&str>)?;
    let reload = MenuItem::with_id(app, "reload", "重新加载", true, None::<&str>)?;
    let log = MenuItem::with_id(app, "log", "打开日志", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出 yarr", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &reload, &log, &sep, &quit])?;

    // macOS 用模板图（纯黑 + alpha，跟随菜单栏明暗）；其他平台用彩色图，
    // 否则黑色图标在 Windows 深色任务栏上几乎看不见。
    #[cfg(target_os = "macos")]
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
    #[cfg(not(target_os = "macos"))]
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?;

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("yarr")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "reload" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.eval("window.location.reload()");
                }
            }
            "log" => {
                if let Some(st) = app.try_state::<Arc<AppState>>() {
                    open_path(app, &st.log_path);
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

#[cfg(unix)]
fn block_shutdown_signals() {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
}

/// 用 sigwait 而不是 signal handler：handler 里不能安全调用 Tauri API。
#[cfg(unix)]
fn spawn_signal_thread(app: AppHandle) {
    std::thread::spawn(move || unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        let mut sig: libc::c_int = 0;
        if libc::sigwait(&set, &mut sig) == 0 {
            app.exit(0);
        }
    });
}

fn main() {
    // 必须在创建任何线程之前屏蔽退出信号，否则信号会被其他线程接走并直接终止进程，
    // 走不到 RunEvent::Exit，sidecar 就变成孤儿进程。
    #[cfg(unix)]
    block_shutdown_signals();

    let builder = tauri::Builder::default();

    // 单实例必须最先注册：两个实例会争同一个 SQLite 文件，是最难排查的一类故障。
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
        show_main(app);
    }));

    builder
        // 关键：不恢复「可见性」。插件默认会把上次退出时的 visible 状态写回，
        // 启动时执行 window.show()，这会在 yarr 就绪前把「正在启动本地服务」的
        // 本地页显示出来 —— 窗口隐藏策略就被这一步破坏掉了。
        // 尺寸/位置/最大化仍然记忆。
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        & !tauri_plugin_window_state::StateFlags::VISIBLE,
                )
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            boot_state,
            retry_boot,
            open_log,
            open_data_dir
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let state = AppState::new(&handle).map_err(|e| Box::<dyn std::error::Error>::from(e))?;
            app.manage(state.clone());

            build_tray(app)?;

            #[cfg(target_os = "macos")]
            {
                // 标准 macOS 菜单（含 About / 隐藏 / 退出，以及编辑菜单的复制粘贴）
                if let Ok(menu) = Menu::default(app.handle()) {
                    let _ = app.set_menu(menu);
                }
            }

            let nav_handle = handle.clone();
            let win = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("yarr")
                .inner_size(1200.0, 820.0)
                .min_inner_size(760.0, 520.0)
                .on_navigation(move |url| {
                    let port = nav_handle
                        .try_state::<Arc<AppState>>()
                        .map(|s| s.port.load(Ordering::SeqCst))
                        .unwrap_or(0);
                    handle_navigation(&nav_handle, url, port)
                })
                // WKWebView 不实现 alert/confirm/prompt（wry 的 WKUIDelegate 只有文件选择、
                // 媒体权限、新窗口），上游的建/重命名/删除对话框会静默失效。这里补一层 DOM 实现。
                .initialization_script(include_str!("../../ui/dialogs.js"))
                // target=_blank 在 WKWebView 里会被 wry 静默丢弃（没有注册 new-window 回调），
                // 导致「打开原文」和正文外链点了没反应。这里把它降级成同窗口导航，
                // 再由下面的 on_navigation 转给系统浏览器。
                .initialization_script(include_str!("../../ui/links.js"))
                // 窗口先隐藏：yarr 就绪、页面加载完成后再显示。
                // 这样启动时不会先闪一下「正在启动本地服务」的本地页。
                // 本地页只在启动很慢（首启迁移 DB）或失败时才作为兜底出现。
                .visible(false)
                .on_page_load(|window, payload| {
                    if payload.event() != PageLoadEvent::Finished {
                        return;
                    }
                    let ready = window
                        .app_handle()
                        .try_state::<Arc<AppState>>()
                        .map(|s| s.ready.load(Ordering::SeqCst))
                        .unwrap_or(false);
                    // 只在隐藏状态下才显示，避免崩溃重载时把焦点从用户手里抢走
                    if ready && !window.is_visible().unwrap_or(true) {
                        if let Some(st) = window
                            .app_handle()
                            .try_state::<Arc<AppState>>()
                        {
                            st.sink
                                .push("[desktop] 页面加载完成，显示窗口".to_string());
                        }
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                })
                .build()?;

            // macOS：关窗只隐藏，应用留在托盘；Cmd+Q 才真正退出。
            let win_for_close = win.clone();
            win.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    #[cfg(target_os = "macos")]
                    {
                        api.prevent_close();
                        let _ = win_for_close.hide();
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        let _ = (&api, &win_for_close);
                    }
                }
            });

            sidecar::start_boot(handle.clone(), state.clone());

            // 兜底：首启迁移数据库可能超过 1s，2.5s 还没就绪就先显示启动页，
            // 避免"点了图标却没有任何反应"的错觉；就绪后照常切到真实界面。
            let late_handle = handle.clone();
            let late_state = state.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(2500));
                if late_state.ready.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(w) = late_handle.get_webview_window("main") {
                    if !w.is_visible().unwrap_or(true) {
                        let _ = w.show();
                    }
                }
            });

            // 收到 SIGTERM / SIGINT 时走正常退出流程（Tauri 的 exit 会触发 RunEvent::Exit）
            #[cfg(unix)]
            spawn_signal_thread(handle);

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("构建 Tauri 应用失败")
        .run(|app, event| match event {
            // 无论哪条路径退出，都必须先收尸，否则会留下占着数据库的孤儿进程。
            RunEvent::ExitRequested { .. } | RunEvent::Exit => {
                if let Some(st) = app.try_state::<Arc<AppState>>() {
                    st.shutdown();
                }
            }
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => show_main(app),
            _ => {}
        });
}
