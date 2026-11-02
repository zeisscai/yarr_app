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

## 运行时行为

| 行为 | 说明 |
| --- | --- |
| 端口 | 每次启动由内核分配空闲端口，只监听 `127.0.0.1` |
| 数据 | `~/Library/Application Support/app.yarr.desktop/storage.db`（首次启动会尝试从上游默认位置 `Application Support/yarr/storage.db` 导入） |
| 日志 | 同目录 `yarr.log`，托盘菜单「打开日志」直达；启动失败页也会显示尾部日志 |
| 单实例 | 第二次启动只激活已有窗口，绝不启动第二个 server（两个进程写同一个 SQLite 是最难查的故障） |
| 关窗 | macOS 关窗只隐藏，应用留在托盘；Cmd+Q 才退出 |
| 外链 | 站外链接交给系统浏览器；上游前端用的是普通 `<a href>`，不拦截会把整个应用导航走 |
| 崩溃自愈 | sidecar 异常退出后按同端口自动重启（最多 3 次），页面自动重载 |
| 退出回收 | 正常退出 / SIGTERM / SIGINT 都会先给 sidecar 发 SIGTERM，1.5s 未退出再 SIGKILL |
| 强杀兜底 | 被 `kill -9` 或「强制退出」后 sidecar 会成为孤儿；下次启动会先清理占用同一数据库的残留进程 |

调试用环境变量：`YARR_DATA_DIR=/tmp/xxx` 覆盖数据目录；`YARR_SIDECAR=/path/to/yarr` 指定二进制。

## 故意不开的能力

- **不开 `--auth`**：上游鉴权 cookie 是 `Secure: true`（`src/server/middleware/auth.go`），`http://127.0.0.1` 的 WebView 很可能拒收，表现为「登录成功但仍未登录」。安全由「只绑回环 + 随机端口 + 单实例」保证。
- **不做浏览器访问同一份数据**：随机端口意味着地址不可记；需要时再加固定端口开关。

## 已知限制

- 未签名、未公证：本地构建是 ad-hoc 签名，分发给别人会被 Gatekeeper 拦（M4 再处理）。
- OPML 导出走 `/opml/export` 下载，WebView 内未接下载管理器，行为待验证。
- 仅适配 macOS；Windows / Linux 的托盘与关闭语义在代码里已分支，但未实测。
- 托盘菜单、关窗隐藏这两条依赖人工点击，未纳入自动化冒烟。

## 上游同步

见 [`docs/upstream.md`](docs/upstream.md)。核心是**自有代码全部隔离在 `desktop/`**，上游文件零改动，因此同步就是 `git merge`，结构性不会冲突。当前补丁数：0。

## 许可

MIT，见仓库根目录 `license`。本项目是非官方桌面封装，与上游作者无隶属关系。
