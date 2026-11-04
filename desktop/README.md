# yarr desktop（非官方 macOS 版）

把 [yarr](https://github.com/nkanaev/yarr) 包成原生窗口的桌面应用。**上游一行代码都不改**：Tauri 只做壳，Go 二进制作为 sidecar 跑在本机回环地址上，WebView 直接加载它吐出的页面。

> 非官方 fork，遵循上游 MIT 许可。上游版权归 Nazar Kanaev。

## 为什么这样分层

上游已经有「桌面模式」，但 `src/platform/gui.go` 的 `gui` 构建只是起一个托盘、点 Open 调系统浏览器打开 URL。真正的缺口只有一个：把「系统浏览器」换成「内嵌 WebView」。

而前端并不是能独立跑的静态包 —— `src/assets/templates/index.html` 是 Go 的 `html/template`，服务端要注入 `settings` / `authenticated` / `requiresAuth`，`main.ts` 直接读 `window.app.*`。所以页面必须由 Go 服务提供，这也是选择「加载本地 URL」而不是「改造前端塞进 frontendDist」的硬性原因。

```
Tauri 壳 (Rust)  ──spawn──▶  yarr (Go, 上游原样)  ──▶  SQLite
  窗口/托盘/单实例              内嵌 Vue 前端 + REST API
  端口/看护/外链
        │
        └── WebView 加载 http://127.0.0.1:<随机端口>
```

职责边界：**壳负责「怎么呈现、怎么活」，Go 负责所有业务。** 壳不碰数据库、不碰 feed 解析，因此上游更新时这个 crate 基本不用改。

## 目录

```
desktop/
├── shell/
│   ├── src-tauri/          # Rust 壳
│   │   ├── src/main.rs     # 窗口、托盘、菜单、外链拦截、退出语义
│   │   ├── src/sidecar.rs  # 端口、拉起、就绪探针、日志、崩溃重启、收尸
│   │   ├── tauri.conf.json
│   │   └── binaries/       # 构建产物（gitignore），yarr-<target-triple>
│   └── ui/index.html       # 本地启动页 / 错误页（远端页面不可用时的兜底）
├── scripts/
│   ├── build-sidecar.sh    # 调上游 makefile 产 sidecar，按 triple 命名
│   ├── dev.sh              # 开发循环
│   └── smoke.sh            # M2 验收冒烟：启动/接口/自愈/退出回收
├── docs/upstream.md        # 补丁登记表 + 同步日志 + 冒烟清单
└── UPSTREAM                # 当前基线 commit / tag / 同步日期
```

## 快速开始

依赖：macOS + Xcode Command Line Tools、Node 22、Rust（rustup）、Go（`go.mod` 要求 1.26，需允许自动下载工具链）。

```bash
# 1. 构建后端 sidecar（内部会按需构建前端资源，再调用上游 make host）
./desktop/scripts/build-sidecar.sh

# 2. 打包 .app（--debug 更快；要发布就换成 --bundles app,dmg 并去掉 --debug）
cd desktop/shell && npx @tauri-apps/cli build --debug --bundles app

# 3. 运行
open desktop/shell/src-tauri/target/debug/bundle/macos/yarr.app

# 4. 验收冒烟（会启动、杀掉 sidecar 验证自愈、再正常退出验证回收）
./desktop/scripts/smoke.sh
```

开发循环：`./desktop/scripts/dev.sh`（先构建 sidecar，再 `tauri dev`）。

## 三平台构建

`.github/workflows/desktop-build.yml` 用一个 matrix 出三个平台的安装包：

| 平台 | runner | sidecar make 目标 | 安装包 |
| --- | --- | --- | --- |
| macOS arm64 | macos-14 | `host` | `.dmg` |
| Linux x64 | ubuntu-24.04 | `linux_amd64`（zig cc / musl 静态） | `.AppImage` / `.deb` |
| Windows x64 | windows-latest | `windows_amd64`（zig cc / windows-gnu） | NSIS `.exe` |

触发方式：手动 `workflow_dispatch`，或推送 `desktop-v*` 标签。产物只上传为 artifact，不自动发 Release —— 目前 macOS 未签名公证、Windows 无代码签名，自动发布等于给用户发"下载后会被系统拦"的包。

本地按平台构建时，用同一套环境变量驱动同一个脚本：

```bash
# Linux 安装包
YARR_MAKE_TARGET=linux_amd64 ./desktop/scripts/build-sidecar.sh
cd desktop/shell && npx @tauri-apps/cli@2 build --bundles appimage,deb

# Windows（在 Windows 上执行，需 zig + make）
YARR_MAKE_TARGET=windows_amd64 ./desktop/scripts/build-sidecar.sh
cd desktop/shell && npx @tauri-apps/cli@2 build --bundles nsis
```

`--bundles` 必须显式传：`tauri.conf.json` 里的 `targets` 写的是 macOS 的 `app/dmg`。

## 端到端检查

`desktop/e2e/` 是一个独立的 Node 小工程（只依赖 `playwright-core`），用真实浏览器加载真实页面，把「新建订阅 → 新建文件夹 → 输入 → 确定」整条路径点一遍，顺带验证 Esc 取消不产生副作用、`confirm()` 能返回 true：

```bash
# 先启动应用，再：
cd desktop/e2e && npm install && node dialogs.spec.mjs
```

不指定 `BASE` 时会从 `~/Library/Application Support/app.yarr.desktop/yarr.log` 里读当前端口；`CHROMIUM` 可指定浏览器，否则自动在 ms-playwright 缓存里找。

## 运行时行为

| 行为 | 说明 |
| --- | --- |
| 启动 | 窗口在 yarr 就绪、页面加载完成后才出现，**不显示启动页**；只有首启迁移数据库超过 2.5s 或启动失败时才会看到本地状态页 |
| 端口 | 每次启动由内核分配空闲端口，只监听 `127.0.0.1` |
| 数据 | `~/Library/Application Support/app.yarr.desktop/storage.db`（首次启动会尝试从上游默认位置 `Application Support/yarr/storage.db` 导入） |
| 日志 | 同目录 `yarr.log`，托盘菜单「打开日志」直达；启动失败页也会显示尾部日志 |
| 单实例 | 第二次启动只激活已有窗口，绝不启动第二个 server（两个进程写同一个 SQLite 是最难查的故障） |
| 关窗 | macOS 关窗只隐藏，应用留在托盘；Cmd+Q 才退出 |
| 外链 | 站外链接（含 `target="_blank"`）一律交给系统浏览器。上游前端用的是普通 `<a href>`，不拦截会把整个应用导航走；而 `_blank` 在 WKWebView 里会被 wry 静默丢弃，所以壳在页面层把它降级成同窗口导航再拦截 |
| 崩溃自愈 | sidecar 异常退出后按同端口自动重启（最多 3 次），页面自动重载 |
| 退出回收 | 正常退出 / SIGTERM / SIGINT 都会先给 sidecar 发 SIGTERM，1.5s 未退出再 SIGKILL |
| 导出订阅 | 上游的「导出」是普通链接指向 `/opml/export`，响应是 attachment —— WKWebView 没有下载管理器，直接放行会白屏。壳在导航层拦下，自己取一次并存到 `~/Downloads/`，然后在 Finder 里选中该文件 |
| 强杀兜底 | 被 `kill -9` 或「强制退出」后 sidecar 会成为孤儿；下次启动会先清理占用同一数据库的残留进程 |

调试用环境变量：`YARR_DATA_DIR=/tmp/xxx` 覆盖数据目录；`YARR_SIDECAR=/path/to/yarr` 指定二进制。

### 对话框为什么是自绘的

WKWebView 不实现 `alert` / `confirm` / `prompt`：wry 的 `WKUIDelegate` 只实现了文件选择、媒体权限、新窗口，没有实现这三个 JS 对话框方法。表现是 `prompt()` 直接返回 `null`、`confirm()` 返回 `false` —— 于是「新建文件夹、重命名文件夹/订阅、修改订阅链接、删除确认」全部点了没反应。

壳的做法：`desktop/shell/ui/dialogs.js` 提供等价的异步对话框，由 Rust 侧 `initialization_script` 注入；上游 2 个文件的 8 处调用点改为 `await` 调用，浏览器环境自动回退到原生 API。这是目前唯一的补丁，登记见 [`docs/upstream.md`](docs/upstream.md)。

实现上必须用 `<dialog>.showModal()` 而不是 `z-index` 遮罩：yarr 自己的弹窗就是 `<dialog>`，处于 top layer，任何 `z-index` 都盖不过它。

## 故意不开的能力

- **不开 `--auth`**：上游鉴权 cookie 是 `Secure: true`（`src/server/middleware/auth.go`），`http://127.0.0.1` 的 WebView 很可能拒收，表现为「登录成功但仍未登录」。安全由「只绑回环 + 随机端口 + 单实例」保证。
- **不做浏览器访问同一份数据**：随机端口意味着地址不可记；需要时再加固定端口开关。

## 已知限制

- 未签名、未公证：本地构建是 ad-hoc 签名，分发给别人会被 Gatekeeper 拦（M4 再处理）。
- 仅适配 macOS；Windows / Linux 的托盘与关闭语义在代码里已分支，但未实测。
- 托盘菜单、关窗隐藏这两条依赖人工点击，未纳入自动化冒烟。

## 上游同步

见 [`docs/upstream.md`](docs/upstream.md)。核心是**自有代码全部隔离在 `desktop/`**，上游文件零改动，因此同步就是 `git merge`，结构性不会冲突。当前补丁数：0。

## 许可

MIT，见仓库根目录 `license`。本项目是非官方桌面封装，与上游作者无隶属关系。

## 故障排查

### 阅读模式（书图标）提示「阅读模式加载失败」

阅读模式**不是**浏览器渲染，而是把原链接交给后端重新抓取一遍再提取正文（`/page` → `handlePageCrawl`）。所以源站挡爬虫、文章被删、或者源站自己挂了，都会失败 —— 浏览器能打开不代表后端能抓到。

排查：看日志里的 `failed to fetch ...`，它会带上状态码。

```bash
tail -20 ~/Library/Application\ Support/app.yarr.desktop/yarr.log | grep "failed to fetch"
```

实测样本（11 个真实订阅链接）：10 个成功，1 个失败且是源站返回 404（文章已删）。另见过 `status 521`（Cloudflare 说源站宕机）和偶发 `EOF`（瞬时网络抖动，重试即可）。

失败时的替代路径：点工具栏里书图标**右边**的外链图标，用系统浏览器打开原文。

### 添加订阅提示「保存订阅失败 / 500 内部服务错误」

这是上游的行为，不是壳的故障：`handleFeedList` 在抓取或解析源失败时统一返回 `500` 且**响应体为空**（`writeJSON(w, 500, nil)`），前端只能拿到状态码，于是显示成「内部服务错误」。真实原因在日志里。

排查顺序：

1. 托盘菜单「打开日志」，或直接看 `~/Library/Application Support/app.yarr.desktop/yarr.log`，搜 `Faild to discover feed`（上游原文就是这个拼写）。
2. 日志会带上具体原因和状态码，例如 `status code 403`、`status code 429`、`context deadline exceeded`。
3. 用同样的 UA 验证源本身是否可用：

   ```bash
   curl -s -o /dev/null -w "%{http_code}\n" -A "Yarr/bleeding" "https://example.com/feed"
   ```

常见原因：

| 日志里的现象 | 含义 | 处理 |
| --- | --- | --- |
| `status code 403` | 源站拒绝阅读器/爬虫 | 换源，或自建实例 |
| `status code 429` | 触发限流 | 降低刷新频率或换源 |
| `context deadline exceeded` | 抓取超时（默认 30s） | 换源或自建 |
| `no feed found` | 页面里没有可识别的 RSS/Atom | 填真正的 feed 地址 |

已知踩坑：**`rsshub.app` 官方实例已按阅读器限制访问**，任何 UA 都返回 403，响应正文明确写着「intended for testing purposes only，建议自建」。可选替代：少数派官方源 `https://sspai.com/feed`、第三方实例（如 `https://rsshub.rssforever.com/sspai/index`），长期使用建议自建 RSSHub。
