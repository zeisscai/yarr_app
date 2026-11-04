//! yarr 进程看护：端口分配、拉起、就绪探针、日志捕获、异常退出重启。
//!
//! 设计原则：壳不碰业务。yarr 的二进制一行都不改，只通过命令行参数控制
//! （`-addr` / `-db`），所有业务能力都在 Go 侧。

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Manager};

const TAIL_LIMIT: usize = 80;
const BOOT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RESTARTS: u32 = 3;

#[derive(Clone, Serialize)]
pub struct BootState {
    pub phase: String,
    pub url: String,
    pub error: String,
    pub log_tail: Vec<String>,
    pub db_path: String,
    pub log_path: String,
    pub attempts: u32,
}

pub struct AppState {
    pub boot: Mutex<BootState>,
    pub port: AtomicU16,
    pub pid: AtomicI32,
    pub quitting: AtomicBool,
    /// 是否已经拿到可访问的 yarr 地址（决定窗口何时显示）
    pub ready: AtomicBool,
    /// 防止「点重试」和自动重启同时拉起两个进程
    pub booting: Mutex<()>,
    pub db_path: PathBuf,
    pub log_path: PathBuf,
    pub sink: Arc<Sink>,
}

impl AppState {
    pub fn new(app: &AppHandle) -> Result<Arc<AppState>, String> {
        // YARR_DATA_DIR 仅用于本地调试与冒烟测试，生产路径仍走系统应用数据目录
        let dir = match std::env::var_os("YARR_DATA_DIR") {
            Some(custom) => PathBuf::from(custom),
            None => app
                .path()
                .app_data_dir()
                .map_err(|e| format!("无法定位应用数据目录: {e}"))?,
        };
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建数据目录失败: {e}"))?;

        let db_path = dir.join("storage.db");
        let log_path = dir.join("yarr.log");
        migrate_legacy_db(&db_path);

        let sink = Arc::new(Sink::new(&log_path));
        sink.push(format!(
            "[desktop] 数据目录 {} | sidecar {}",
            dir.display(),
            resolve_binary()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|e| format!("<未找到: {e}>"))
        ));

        Ok(Arc::new(AppState {
            boot: Mutex::new(BootState {
                phase: "idle".into(),
                url: String::new(),
                error: String::new(),
                log_tail: Vec::new(),
                db_path: db_path.display().to_string(),
                log_path: log_path.display().to_string(),
                attempts: 0,
            }),
            port: AtomicU16::new(0),
            pid: AtomicI32::new(0),
            quitting: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            booting: Mutex::new(()),
            db_path,
            log_path,
            sink,
        }))
    }

    pub fn snapshot(&self) -> BootState {
        let mut s = self.boot.lock().unwrap_or_else(|e| e.into_inner()).clone();
        s.log_tail = self.sink.tail();
        s
    }

    /// 退出时收尸，避免 SQLite 留下 -wal/-shm 残留或孤儿进程占库。
    /// unix 走 SIGTERM → 等待 → SIGKILL；Windows 用 taskkill /T /F（没有等价的优雅信号）。
    pub fn shutdown(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        let pid = self.pid.swap(0, Ordering::SeqCst);
        if pid <= 0 {
            return;
        }

        #[cfg(unix)]
        {
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_millis(1500);
            while Instant::now() < deadline {
                // 发 0 号信号只探测进程是否存在
                if unsafe { libc::kill(pid, 0) } != 0 {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }

        #[cfg(windows)]
        {
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .status();
        }
    }
}

/// 首次启动若发现上游 CLI 版本留下的数据库，复制一份过来，避免用户「数据不见了」。
fn migrate_legacy_db(target: &Path) {
    if target.exists() {
        return;
    }
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let legacy = PathBuf::from(home)
        .join("Library/Application Support/yarr/storage.db");
    if legacy.exists() {
        match std::fs::copy(&legacy, target) {
            Ok(_) => eprintln!(
                "[desktop] 已从上游默认位置导入数据库: {}",
                legacy.display()
            ),
            Err(e) => eprintln!("[desktop] 导入旧数据库失败: {e}"),
        }
    }
}

pub struct Sink {
    file: Mutex<Option<File>>,
    tail: Mutex<VecDeque<String>>,
}

impl Sink {
    fn new(path: &Path) -> Sink {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok();
        Sink {
            file: Mutex::new(file),
            tail: Mutex::new(VecDeque::with_capacity(TAIL_LIMIT)),
        }
    }

    pub fn push(&self, line: String) {
        if let Ok(mut t) = self.tail.lock() {
            if t.len() == TAIL_LIMIT {
                t.pop_front();
            }
            t.push_back(line.clone());
        }
        if let Ok(mut f) = self.file.lock() {
            if let Some(f) = f.as_mut() {
                let _ = writeln!(f, "{line}");
            }
        }
    }

    pub fn tail(&self) -> Vec<String> {
        self.tail
            .lock()
            .map(|t| t.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// 让内核挑一个空闲端口：先 bind :0 拿到号再释放。
/// 极小概率存在竞态（释放后被人抢），所以外层还有 3 次重试。
pub fn free_port() -> Result<u16, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("绑定临时端口失败: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    drop(listener);
    Ok(port)
}

pub fn resolve_binary() -> Result<PathBuf, String> {
    if let Ok(explicit) = std::env::var("YARR_SIDECAR") {
        let p = PathBuf::from(explicit);
        if p.exists() {
            return Ok(p);
        }
    }

    // 打包后：yarr 与主程序同目录（Tauri 把 externalBin 放在 Contents/MacOS/）
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let name = if cfg!(windows) { "yarr.exe" } else { "yarr" };
            let candidate = dir.join(name);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    // 开发态：binaries/yarr-<target-triple>
    let dev = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(format!("yarr-{}", env!("YARR_TARGET_TRIPLE")));
    if dev.exists() {
        return Ok(dev);
    }

    Err(format!(
        "未找到 yarr 二进制，请先运行 ./desktop/scripts/build-sidecar.sh（期望位置: {}）",
        dev.display()
    ))
}

fn spawn_child(bin: &Path, port: u16, db: &Path, sink: &Arc<Sink>) -> Result<Child, String> {
    let mut cmd = Command::new(bin);
    cmd.arg("-addr")
        .arg(format!("127.0.0.1:{port}"))
        .arg("-db")
        .arg(db)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("拉起 yarr 失败: {e}（{}）", bin.display()))?;

    if let Some(stdout) = child.stdout.take() {
        pump(stdout, sink.clone(), "[out]");
    }
    if let Some(stderr) = child.stderr.take() {
        pump(stderr, sink.clone(), "[err]");
    }
    Ok(child)
}

fn pump<R: std::io::Read + Send + 'static>(reader: R, sink: Arc<Sink>, tag: &'static str) {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            match line {
                Ok(l) => sink.push(format!("{tag} {l}")),
                Err(_) => break,
            }
        }
    });
}

fn wait_ready(child: &mut Child, port: u16, timeout: Duration) -> Result<(), String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "yarr 进程启动后立即退出（{status}）。常见原因：端口被占用、数据库无法访问。"
            ));
        }
        if TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    Err(format!(
        "等待 yarr 就绪超时（{}s），127.0.0.1:{port} 未监听。",
        timeout.as_secs()
    ))
}

fn kill_child(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn set_starting(st: &AppState, attempts: u32) {
    let mut b = st.boot.lock().unwrap_or_else(|e| e.into_inner());
    b.phase = "starting".into();
    b.error.clear();
    b.attempts = attempts;
}

fn set_failed(app: &AppHandle, st: &AppState, err: &str) {
    st.sink.push(format!("[desktop] 启动失败: {err}"));
    {
        let mut b = st.boot.lock().unwrap_or_else(|e| e.into_inner());
        b.phase = "failed".into();
        b.error = err.to_string();
    }
    notify_failure(app, err);

    // 启动阶段窗口是隐藏的（为了不显示本地启动页），失败时必须让它露面，
    // 否则用户只会看到 Dock 图标在跳、什么也没发生。
    if let Some(w) = app.get_webview_window("main") {
        if !w.is_visible().unwrap_or(true) {
            st.sink.push("[desktop] 启动失败，显示窗口".to_string());
            let _ = w.show();
            let _ = w.set_focus();
        }
    }
}

/// 远端页面在 yarr 挂掉后已经不可用，用注入的遮罩层告知用户；
/// 本地状态页则自己轮询 boot_state，不需要重复提示。
fn notify_failure(app: &AppHandle, message: &str) {
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    let on_local_page = w
        .url()
        .map(|u| matches!(u.scheme(), "tauri" | "about"))
        .unwrap_or(true);
    if on_local_page {
        return;
    }
    let text = serde_json::to_string(&format!(
        "yarr 后端异常退出，正在自动恢复……\n\n{message}\n\n如果长时间没有恢复，请从托盘菜单「打开日志」查看原因。"
    ))
    .unwrap_or_else(|_| "\"yarr 后端异常退出\"".to_string());

    let js = "(function(){if(document.getElementById('__yarr_overlay'))return;\
var d=document.createElement('div');d.id='__yarr_overlay';\
d.style.cssText='position:fixed;inset:0;z-index:2147483647;background:rgba(18,18,18,.95);color:#e8e8e8;font:14px/1.7 -apple-system,system-ui,sans-serif;padding:48px;overflow:auto;white-space:pre-wrap';\
d.textContent="
        .to_string()
        + &text
        + ";document.body.appendChild(d);})()";
    let _ = w.eval(&js);
}

fn navigate_to_app(app: &AppHandle, url: &str) {
    // 标记为已就绪：窗口的 on_page_load 回调据此决定"加载完成后才显示"，
    // 这样启动时不会先闪一下「正在启动本地服务」的本地页。
    if let Some(st) = app.try_state::<Arc<AppState>>() {
        st.ready.store(true, Ordering::SeqCst);
    }
    if let Some(w) = app.get_webview_window("main") {
        match url.parse() {
            Ok(parsed) => {
                let _ = w.navigate(parsed);
            }
            Err(e) => st_log(app, format!("[desktop] URL 解析失败: {e}")),
        }
    }
}

fn st_log(app: &AppHandle, line: String) {
    if let Some(st) = app.try_state::<Arc<AppState>>() {
        st.sink.push(line);
    }
}

/// 上一次被强杀（SIGKILL / 强制退出）可能留下占着同一个数据库的孤儿进程。
/// 启动前先清理，避免两个 server 同时写一个 SQLite —— 这类问题用户几乎无法自查。
#[cfg(unix)]
fn kill_stale_sidecars(db: &Path) {
    let pattern = db.display().to_string();
    let Ok(out) = Command::new("/usr/bin/pgrep").arg("-f").arg(&pattern).output() else {
        return;
    };
    let me = std::process::id() as i32;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Ok(pid) = line.trim().parse::<i32>() {
            if pid <= 0 || pid == me {
                continue;
            }
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
    }
}

#[cfg(not(unix))]
fn kill_stale_sidecars(_db: &Path) {}

pub fn start_boot(app: AppHandle, st: Arc<AppState>) {
    std::thread::spawn(move || run_boot(app, st));
}

fn run_boot(app: AppHandle, st: Arc<AppState>) {
    let guard = st.booting.lock().unwrap_or_else(|e| e.into_inner());

    if st.quitting.load(Ordering::SeqCst) {
        return;
    }

    kill_stale_sidecars(&st.db_path);

    let bin = match resolve_binary() {
        Ok(b) => b,
        Err(e) => return set_failed(&app, &st, &e),
    };

    let mut last_err = String::new();
    for attempt in 1..=3u32 {
        set_starting(&st, attempt);
        let port = match free_port() {
            Ok(p) => p,
            Err(e) => return set_failed(&app, &st, &e),
        };
        st.port.store(port, Ordering::SeqCst);

        let mut child = match spawn_child(&bin, port, &st.db_path, &st.sink) {
            Ok(c) => c,
            Err(e) => return set_failed(&app, &st, &e),
        };
        st.pid.store(child.id() as i32, Ordering::SeqCst);
        st.sink
            .push(format!("[desktop] 已拉起 yarr pid={} port={port}", child.id()));

        match wait_ready(&mut child, port, BOOT_TIMEOUT) {
            Ok(()) => {
                let url = format!("http://127.0.0.1:{port}");
                st.sink.push(format!("[desktop] yarr 就绪: {url}"));
                {
                    let mut b = st.boot.lock().unwrap_or_else(|e| e.into_inner());
                    b.phase = "ready".into();
                    b.url = url.clone();
                    b.error.clear();
                    b.attempts = attempt;
                }
                navigate_to_app(&app, &url);
                // 进入看护阶段后释放启动锁，允许用户/托盘触发重启
                drop(guard);
                supervise(app, st, child);
                return;
            }
            Err(e) => {
                st.sink.push(format!("[desktop] 第 {attempt} 次尝试失败: {e}"));
                last_err = e;
                kill_child(&mut child);
                st.pid.store(0, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(350));
            }
        }
    }

    set_failed(&app, &st, &last_err);
}

/// 阻塞看护：进程退出则按退避重启（同端口，页面无感），超过上限就停在错误页。
fn supervise(app: AppHandle, st: Arc<AppState>, mut child: Child) {
    let mut restarts = 0u32;
    loop {
        let status = child.wait();
        st.pid.store(0, Ordering::SeqCst);

        if st.quitting.load(Ordering::SeqCst) {
            return;
        }

        let code = status.as_ref().map(|s| s.to_string()).unwrap_or_else(|e| e.to_string());
        st.sink.push(format!("[desktop] yarr 意外退出: {code}"));

        if restarts >= MAX_RESTARTS {
            set_failed(&app, &st, "yarr 反复异常退出，已停止自动重启。请查看日志。");
            return;
        }
        restarts += 1;
        std::thread::sleep(Duration::from_millis(700));

        let port = st.port.load(Ordering::SeqCst);
        if port == 0 {
            set_failed(&app, &st, "端口状态丢失，无法重启 yarr。");
            return;
        }
        let bin = match resolve_binary() {
            Ok(b) => b,
            Err(e) => return set_failed(&app, &st, &e),
        };

        match spawn_child(&bin, port, &st.db_path, &st.sink) {
            Ok(mut c) => {
                st.pid.store(c.id() as i32, Ordering::SeqCst);
                st.sink
                    .push(format!("[desktop] 重启 yarr（第 {restarts} 次）pid={}", c.id()));
                match wait_ready(&mut c, port, Duration::from_secs(15)) {
                    Ok(()) => {
                        let url = format!("http://127.0.0.1:{port}");
                        {
                            let mut b = st.boot.lock().unwrap_or_else(|e| e.into_inner());
                            b.phase = "ready".into();
                            b.url = url.clone();
                            b.error.clear();
                        }
                        navigate_to_app(&app, &url);
                        child = c;
                        continue;
                    }
                    Err(e) => {
                        kill_child(&mut c);
                        st.pid.store(0, Ordering::SeqCst);
                        set_failed(&app, &st, &format!("重启失败: {e}"));
                        return;
                    }
                }
            }
            Err(e) => {
                set_failed(&app, &st, &e);
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// OPML 导出
//
// 桌面端没有下载管理器：上游的「导出订阅」是一个普通链接（href="./opml/export"），
// 点下去 WKWebView 会导航到这个地址，而响应带 Content-Disposition: attachment，
// WebKit 无法渲染，于是整个窗口变成白屏，只能重启应用。
// 所以在 on_navigation 里拦下这个路径，自己取一次 HTTP 响应存到下载目录。
// ---------------------------------------------------------------------------

/// 直接对本地 sidecar 发一个 GET。刻意不引 HTTP 客户端依赖：目标是 127.0.0.1
/// 的已知路径、响应很小，用 std 的 TcpStream 足够，也少一个要跟着升级的 crate。
fn http_get_local(port: u16, path: &str) -> Result<(String, Vec<u8>), String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .map_err(|e| format!("连接本地服务失败: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| format!("设置超时失败: {e}"))?;

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: yarr-desktop\r\nAccept: application/xml\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("发送请求失败: {e}"))?;

    let mut raw = Vec::new();
    std::io::Read::read_to_end(&mut stream, &mut raw)
        .map_err(|e| format!("读取响应失败: {e}"))?;

    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| "响应格式异常（找不到头部结束）".to_string())?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut body = raw[split + 4..].to_vec();

    let status_line = head.lines().next().unwrap_or("");
    if !status_line.contains(" 200") {
        return Err(format!("服务端返回异常: {status_line}"));
    }

    let lower = head.to_ascii_lowercase();
    if lower.contains("transfer-encoding: chunked") {
        body = decode_chunked(&body).ok_or_else(|| "chunked 解码失败".to_string())?;
    }

    Ok((head, body))
}

/// Go 的 net/http 在响应超过内部缓冲时会用 chunked 编码，必须解码后再落盘。
fn decode_chunked(mut data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = data.windows(2).position(|w| w == b"\r\n")?;
        let line = std::str::from_utf8(&data[..line_end]).ok()?;
        let size = usize::from_str_radix(line.split(';').next()?.trim(), 16).ok()?;
        data = &data[line_end + 2..];
        if size == 0 {
            break;
        }
        if data.len() < size {
            return None;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size..];
        if data.starts_with(b"\r\n") {
            data = &data[2..];
        }
    }
    Some(out)
}

fn filename_from_headers(head: &str) -> Option<String> {
    for line in head.lines() {
        if !line.to_ascii_lowercase().starts_with("content-disposition:") {
            continue;
        }
        let idx = line.find("filename=\"")?;
        let rest = &line[idx + "filename=\"".len()..];
        let end = rest.find('"')?;
        return Some(rest[..end].to_string());
    }
    None
}

fn sanitize_filename(name: &str) -> String {
    // 先取最后一段再清洗：`../../x` 这种直接变成 `x`，避免路径穿越
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':' | '\0') { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').to_string();
    if trimmed.is_empty() {
        "subscriptions.opml".to_string()
    } else {
        trimmed
    }
}

fn downloads_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("Downloads");
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(profile).join("Downloads");
    }
    std::env::temp_dir()
}

/// 导出订阅到 `~/Downloads/<服务端给的文件名>`，返回落盘路径。
pub fn save_opml(port: u16) -> Result<PathBuf, String> {
    if port == 0 {
        return Err("本地服务尚未就绪".to_string());
    }
    let (head, body) = http_get_local(port, "/opml/export")?;
    if body.is_empty() {
        return Err("导出内容为空".to_string());
    }
    let name = filename_from_headers(&head).unwrap_or_else(|| "subscriptions.opml".to_string());
    let dir = downloads_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(sanitize_filename(&name));
    std::fs::write(&path, &body).map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_chunked_body() {
        let raw = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        assert_eq!(decode_chunked(raw).unwrap(), b"hello world");
    }

    #[test]
    fn parses_filename() {
        let head = "HTTP/1.1 200 OK\r\nContent-Disposition: attachment; filename=\"subscriptions_2026-11-04.opml\"\r\n";
        assert_eq!(
            filename_from_headers(head).unwrap(),
            "subscriptions_2026-11-04.opml"
        );
    }

    #[test]
    fn sanitizes_path_traversal() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
    }
}
