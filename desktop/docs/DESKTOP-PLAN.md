# yarr 桌面版（Tauri）总体方案与上游同步机制

> 状态：规划稿，未开始编码
> 调研基线：`zeisscai/yarr_app` @ `a87e819`（上游 `nkanaev/yarr` master，v2.9 之后）
> 目标：把 yarr 从「托盘 + 系统浏览器」升级为原生窗口的桌面应用，同时保证上游更新可低成本跟进

---

## 0. 结论先讲

| 问题 | 结论 |
| --- | --- |
| 架构怎么选 | Tauri 只做「壳」：把上游 Go 二进制**原样**作为 sidecar 拉起，WebView 直接加载 `http://127.0.0.1:<port>` |
| 上游代码要改多少 | **零行**（唯一可能的例外是动态端口，可用壳侧探测绕开，或改 5 行并 PR 回上游） |
| 上游更新怎么同步 | 所有自有代码隔离在 `desktop/`；同步 = `git merge upstream-mirror`，因为文件重叠面积为 0，**结构性不会冲突** |
| 每次同步成本 | 正常情况下 10 分钟；上游发布大版本时 30~60 分钟（跑一遍冒烟清单） |
| 最大风险 | 不是 git 冲突，而是上游 **CLI 参数 / 路由 / 构建方式** 的变化让壳静默失效 → 用每周漂移检测 + 冒烟清单兜住 |

一句话：**能不能轻松同步，取决于你和上游的文件重叠面积，而不是 git 技巧。** 把重叠面积压到 0，同步问题就消失了。

---

## 1. 事实核查（基于 fork 当前代码，不是猜测）

| 事实 | 证据 | 对桌面版的影响 |
| --- | --- | --- |
| 上游已有「桌面模式」，但只是托盘 + 调系统浏览器 | `src/platform/gui.go`（build tag `gui`），菜单 Open → `Open(s.GetAddr())` | 你要做的真实增量只有一个：把「系统浏览器」换成「内嵌 WebView」。其余都是体验增强 |
| 前端**不是**可独立跑的纯静态 SPA | `src/assets/templates/index.html` 是 Go `html/template`，注入 `.settings` / `.authenticated` / `.requiresAuth`；`src/frontend/js/main.ts` 直接读 `window.app.*` | 关键结论：前端**必须由 Go 服务提供页面**，不能把 dist 塞进 Tauri 的 `frontendDist` 单独跑。这是「加载本地 URL」方案的决定性理由 |
| 后端已有一切必要 CLI 开关 | `cmd/yarr/main.go`：`-addr` `-db` `-base` `-auth-file` `-log-file` `-open` `-client-timeout` | 壳可以 100% 通过命令行控制后端，**不需要改一行 Go** |
| `-addr` 不支持 `:0` 动态端口 | `src/server/server.go` `GetAddr()` 直接字符串拼接 `"http://" + Addr`，`Start()` 拿不到实际端口 | 端口由壳先探测空闲端口再传给 sidecar；或加 5 行补丁返回真实 listener 地址（建议 PR 回上游） |
| 鉴权 cookie 是 `Secure: true` | `src/server/middleware/auth.go` `newAuthCookie()`：`Secure: true, HttpOnly, SameSite=Lax, MaxAge 7d` | **桌面模式绝对不要开 `--auth`**。`http://127.0.0.1` 上 WKWebView 很可能拒收 Secure cookie，表现为「登录成功但仍未登录」的死循环。安全靠：只绑 127.0.0.1 + 随机端口 + 单实例 |
| 有可用的就绪探针 | `src/server/routes.go`：`/api/status`（未配置 auth 时无需鉴权） | 壳用它判断 sidecar 是否就绪，再创建窗口，避免白屏 |
| 前端资源 `go:embed`，无外部依赖 | `src/assets/assetsfs.go`；`debug` tag 下才读磁盘 | 单文件自包含，sidecar 就是「一个二进制」，打包极简单 |
| 默认数据目录与本应用无关 | `main.go`：`os.UserConfigDir()/yarr/storage.db` | 与 Tauri bundle id 目录不一致，必须显式 `-db` 统一，否则出现「两份数据」 |
| 许可证 MIT | `license` | fork 分发需保留版权与许可声明；建议在 README 明确「非官方 fork」 |

---

## 2. 方案对比

| 维度 | A. sidecar + 加载本地 URL（推荐） | B. 前端打进 Tauri + 跨域调 API | C. Rust 重写 / Go 嵌进 Rust | D. 不 fork，直接打包上游 release 二进制 |
| --- | --- | --- | --- | --- |
| 上游代码改动 | 0 行 | 需要改 `api.ts` / `main.ts` / `index.html` + Go 加 CORS | 全部重写 | 0 行 |
| 上游同步难度 | 极低（永不冲突） | **高**：改的正好是最活跃的文件 | 不存在同步，等于放弃上游 | 最低（升版本号） |
| Cookie / 鉴权 | 同源，天然正常 | 跨 site，`SameSite=Lax` 直接失效 | 自研 | 同源正常 |
| 原生能力上限 | 中（托盘/菜单/通知/外链/更新，够用） | 高（可深度定制 UI） | 最高 | 中 |
| 大改版风险 | 上游换框架也不影响 | 上游前端一重构就冲突（v2.9 刚 vue2→vue3） | — | 上游停发二进制就断供 |
| 结论 | ✅ 主推 | ❌ | ❌ | 备选（零补丁时的更优解） |

**为什么 B 明确不选**：`src/frontend/js/api.ts` 用的是相对路径 `fetch`，改成绝对 URL 后后端没有 CORS 中间件；`tauri://localhost` 与 `http://127.0.0.1` 属于不同 site，`SameSite=Lax` 的 cookie 不会携带；模板注入的 `window.app.settings` 还得在 Tauri 侧重造。这三处恰好是上游改动最频繁的地方 —— 等于主动把冲突面积做到最大。

**D 的定位**：如果 M0 验证后确认真的一个补丁都不需要，可以升级为「更优解」——仓库里只有桌面壳，构建时下载上游 release 二进制并校验 sha256，同步退化为改一行版本号，连 Go 工具链和 CGO 都不用装。保留为 Plan B。

**submodule 变体**：把上游当 submodule，你的仓库只有 `desktop/`，diff 极致干净；代价是构建要 `--recursive`、CI 略复杂、不能在同一 commit 里同时改上游。单人项目可选，不作首选。

---

## 3. 推荐架构

### 3.1 进程与职责边界

```
Tauri 壳 (Rust, 常驻主进程)
 ├─ 单实例锁 / 端口探测 / sidecar spawn / 健康探针 / 退出回收
 ├─ 主窗口 (系统 WebView) ──加载──> http://127.0.0.1:<port>
 ├─ 托盘 + 应用菜单 + 外链拦截 + 窗口状态记忆
 └─ 自动更新
        │ spawn (--addr 127.0.0.1:<port> --db <AppData>/yarr/storage.db --log-file ...)
        ▼
yarr 二进制 (Go, 上游原样编译, 无 gui tag)
 └─ HTTP 服务 + 内嵌 Vue 前端 + /api/* + SQLite
        │
        ▼
<AppData>/yarr/storage.db + yarr.log
```

职责划分一句话：**壳负责「怎么呈现和怎么活」，Go 负责「所有业务」。壳不碰数据库、不碰 feed 解析、不碰任何业务逻辑。**

### 3.2 关键决策点

1. **端口**：壳先 `bind :0` 拿到空闲端口再释放（有微小 TOCTOU 竞态，失败重试 3 次），或从 7070 起逐个探测。**不开 `--auth`**（原因见 §1 的 Secure cookie）。
2. **数据目录**：`--db` 指向 Tauri 的 app data 目录（macOS `~/Library/Application Support/<bundle-id>/yarr/storage.db`，Windows `%APPDATA%\<bundle-id>\yarr\storage.db`）。首启若检测到旧的 `~/Library/Application Support/yarr/storage.db`，提示「导入已有数据」。
3. **就绪再建窗**：因为端口是运行期决定的，窗口必须在 Rust 侧运行期创建（不能用 `tauri.conf.json` 里的静态 URL）。先显示轻量启动页，探针 200 后加载真实页面；失败则显示错误页 + sidecar stderr 尾部，而不是白屏。
4. **外链**：`on_navigation` 拦截非 `127.0.0.1` 的导航，交给系统浏览器（RSS 阅读器的核心体验，缺了就是不合格的桌面版）。
5. **退出语义**：macOS 关窗不退出（留托盘），Cmd+Q 退出；Windows 关窗即退出。任何退出路径都必须 kill sidecar，并用看护线程兜底（父进程退出 → sidecar 收尸），避免残留进程占住 SQLite。
6. **单实例**：第二个实例只激活已有窗口。两个 server 指向同一个 SQLite 会直接锁库，这是最容易踩且最难查的坑。
7. **注入（可选，M3）**：若需要原生通知/快捷键，用 `initialization_script` 注入少量 JS，标记 `window.__YARR_DESKTOP__`。原则：**不依赖上游 DOM 结构**，失效时静默降级，不能阻塞页面。

---

## 4. 仓库布局（同步策略的物理基础）

```
yarr_app/
├── cmd/  src/  etc/  doc/  makefile  package.json  go.mod ...   ← 上游文件，一行不改
├── desktop/                        ← 你的全部代码，上游永不触碰
│   ├── shell/                      ← Tauri 工程（src-tauri 内容）
│   │   ├── src/                    ← Rust: 单实例/端口/看护/窗口/托盘/菜单
│   │   ├── binaries/               ← 构建产物存放（gitignore）
│   │   ├── capabilities/
│   │   └── tauri.conf.json
│   ├── scripts/build-sidecar.sh    ← 调上游 makefile 产 sidecar 并按 triple 命名
│   ├── docs/upstream.md            ← 补丁登记表 + 每次同步日志
│   └── UPSTREAM                    ← upstream / commit / tag / synced 日期
├── .github/workflows/desktop-ci.yml       ← 新增文件，不改上游 workflow
└── .github/workflows/desktop-drift.yml    ← 每周漂移检测
```

四条硬规则：

1. **不改上游文件**（理想为 0；每多改一处，未来每次同步就多一分冲突概率）。
2. **不改 `go.mod` 的 module 路径**（`github.com/nkanaev/yarr` 保持原样，改了就永远合不回去）。
3. **不复制上游构建命令**：`desktop/scripts/build-sidecar.sh` 内部调用 `make host`，让上游改构建标签时你自动跟上。
4. **不动上游的 `.github/workflows/`**：只新增 `desktop-*.yml`。同名文件必冲突。

---

## 5. 上游同步机制（核心问题）

### 5.1 三条纪律

- 纪律一：**自有代码只在 `desktop/` 里新增**，不改上游文件。
- 纪律二：**每个改动都能一句话说明来源**，commit 前缀 `desktop:` / `fix:` / `upstream-pr:`。
- 纪律三：**改上游文件的唯一合法理由**是「CLI/HTTP 层做不到」。做不到就先想办法绕，绕不过再改，改完登记。

### 5.2 remote 与分支模型

```
origin    = git@github.com:zeisscai/yarr_app.git
upstream  = https://github.com/nkanaev/yarr.git

upstream-mirror   ← 上游镜像分支，只接受 ff-only，永不提交
desktop           ← 集成分支（GitHub 默认分支设为它）
feature/xxx       ← 短分支，PR 回 desktop
```

日常同步（五步，冲突面积 0 时全程无冲突）：

```bash
git remote add upstream https://github.com/nkanaev/yarr.git   # 只需一次
git fetch upstream --tags
git checkout upstream-mirror && git merge --ff-only upstream/master
git checkout desktop && git merge upstream-mirror
git push origin desktop upstream-mirror
```

**为什么值得多一个 mirror 分支**（不是仪式感）：

- `git diff upstream-mirror..desktop --stat` 就是你「到底改了什么」的完整清单，随时可审计；上游如果提问或你想上游化，一眼就能出 patch。
- 上游若强推/rebase（极少但会发生），`upstream-mirror` 直接 `reset --hard upstream/master` 即可，因为你从不在上面提交 —— 零损失。
- 同步出问题时，你的 `desktop` 工作区始终是干净的，随时能回退。

**merge 还是 rebase？** 结论：`desktop` 用 **merge**。理由：你已经发布过 tag/别人可能 fork，rebase 会重写公开历史；且本地提交越多，rebase 每次要重放的成本越高，而 merge 只在合并点解决一次。只有在「分支从未公开」时才考虑 rebase。

### 5.3 冲突处理纪律

```bash
git config rerere.enabled true        # 同类冲突自动复用上次解法（长期收益很大）
git config merge.conflictStyle zdiff3 # 看得到共同祖先，判断更准
```

冲突时优先级：**上游语义优先**。先删掉自己的补丁，再看能否用新接口重写 —— 不要「两边都留」，那是技术债的起点。每次解决后在 `desktop/docs/upstream.md` 记一条：哪个文件 / 为什么改 / 上游是否已有等价能力 / 能否 PR 回去。

### 5.4 漂移预警（把「发版前崩」变成「每周 5 分钟」）

`desktop-drift.yml`，每周一次：

1. `git fetch upstream`；
2. 在临时分支上尝试 `git merge upstream/master`，不推送；
3. 有冲突 → 开 issue（标题带「upstream drift」，正文附冲突文件列表）；
4. 无冲突 → 直接开 PR 到 `desktop`，CI 跑构建 + 冒烟，绿了就合。

同时订阅上游 releases 的 atom（`releases.atom`），大版本手动看一眼 changelog。

### 5.5 版本可追溯

- `desktop/UPSTREAM` 固定四行：`upstream` / `commit` / `tag` / `synced`。
- 构建注入：sidecar 的 `-X main.Version` 用「桌面版号 + 上游短 sha」；About 面板同时显示「yarr 上游版本」和「桌面壳版本」。
- release tag `v0.1.0`，正文必写 `upstream: v2.9 (a87e819)`。
- 价值：用户报 bug 时，3 秒判断是上游问题还是壳的问题，不用复现。

### 5.6 上游更新后的冒烟清单（10 分钟）

1. 构建通过：sidecar（`make host`）+ `tauri build`。
2. 首启：新库建库成功、页面加载、语言与主题注入正常（模板变了这里最先炸）。
3. 业务：添加 feed / 刷新 / 标已读 / 搜索 / OPML 导入导出。
4. 外链在系统浏览器打开，站内链接仍在应用内。
5. 关窗与退出后无残留进程：`ps aux | grep yarr`。
6. **旧库升级**：拿上一版 `storage.db` 副本启动，确认 schema 迁移正常（先备份再试）。
7. 托盘菜单、Cmd+Q、第二实例激活已有窗口。
8. 重点复核：若上游动了 CLI / 路由 / 静态资源路径，先看 spawn 参数与 `/api/status` 探针。

### 5.7 什么时候把补丁还回上游

判断标准：**这个改动对「非桌面用户」也有价值吗？**
- 有 → 提 PR（例如「`-addr 127.0.0.1:0` 时暴露真实端口」、「提供 JSON 版本信息接口」）。合并后你的补丁消失，同步成本永久下降。
- 没有 → 留在自己仓库，登记在案，接受它是长期维护成本。
- 措辞建议：按维护者视角写「问题现象 + 最小改动 + 不改变现有行为」，不写「我做桌面版需要」。

---

## 6. 上游升级会「打穿」你的地方（风险表）

| 风险 | 触发点 | 表现 | 缓解 |
| --- | --- | --- | --- |
| CLI 参数改名/删除 | `main.go` flag 调整 | sidecar 启动即退出，应用打不开 | 启动失败读 stderr 显示到错误页；CI 冒烟必测 |
| 就绪探针失效 | `/api/status` 改路径或加鉴权 | 永远停在「启动中」 | 探针接受任意 2xx（含首页）；超时后展示日志而非转圈 |
| 鉴权中间件变化 | 误开 `--auth` 或 cookie 策略调整 | 登录循环 | 桌面模式硬编码不开 auth，并把原因写进代码注释 |
| 静态资源/模板变化 | 资源路径或注入字段改名 | 白屏 | 加载 URL 而非本地文件，天然免疫（这也是选 A 的隐性收益） |
| 注入脚本失效 | 前端 DOM/类名重构 | 原生增强静默失效 | 注入逻辑只依赖 `window`，不依赖 DOM；失败降级 |
| Go 版本/CGO/构建标签变化 | 上游升级 Go 或改 tag | sidecar 编不出来 | 脚本调 `make host`，不复制命令；CI 用上游同款 Go 版本 |
| DB schema 迁移 | 上游改表 | 回滚旧版打不开库 | 每次启动/升级前自动备份 `storage.db.bak-<ver>`，保留 3 份 |
| 签名与公证 | macOS 嵌套二进制 | 公证失败/被 Gatekeeper 拦 | sidecar 必须在签名 app 之前单独签名（hardened runtime）；Windows 未签名会触发 SmartScreen |
| Tauri 自身安全公告 | 框架漏洞 | 例如 2026-05 的 origin 混淆 CVE（影响 ≤2.11.0） | 锁 `tauri >= 2.11.1`，开 dependabot |

---

## 7. 打包与分发

- **CI 矩阵**：`macos-14`（arm64，另出 universal）、`windows-latest`、`ubuntu-22.04`。全部**原生编译**，不做交叉编译（CGO + sqlite 交叉是自找麻烦）。
- **sidecar 命名**：Tauri `externalBin` 强制要求 `yarr-<target-triple>` 后缀（`yarr-aarch64-apple-darwin`、`yarr-x86_64-pc-windows-msvc.exe`）。macOS universal 需要先 `lipo` 合并两个架构，再按 universal triple 命名。
- **产物**：`.dmg` / `.msi`(nsis) / `.AppImage`；Linux 需 `webkit2gtk-4.1`，README 写清。
- **体积**：Go 二进制约 12~18 MB（CGO sqlite） + Tauri 约 5 MB，可接受。
- **自动更新**：`tauri-plugin-updater` + 签名密钥；建议 M4 再上，先手动发版验证流程。
- **合规**：MIT，保留 `license` 与上游版权；命名建议「yarr desktop」并在 README 注明非官方 fork，避免和上游混淆。

---

## 8. 里程碑

| 阶段 | 内容 | 验收标准 |
| --- | --- | --- |
| **M0 原型验证**（0.5~1 天） | 手动 `make host` 起服务，最小 Tauri 窗口加载 URL；验证 cookie、外链、退出回收 | 连续开关 10 次无残留进程、无白屏 |
| **M1 壳骨架**（2~3 天） | 单实例、端口探测、spawn、探针、错误页、日志捕获、`--db` 指向 AppData | 双击图标到可用 < 1.5s；异常时能看懂原因 |
| **M2 桌面体验**（3~5 天） | 托盘、应用菜单、窗口状态记忆、外链、关闭语义、崩溃重启 | macOS 关窗留托盘、Cmd+Q 全退；崩溃后自动拉起一次 |
| **M3 产品化**（3~5 天） | About/双版本号、数据迁移与备份、首启引导、设置（代理/开机自启） | 老 yarr 用户能一键导入旧库 |
| **M4 分发**（3~5 天） | CI 三平台、签名公证、自动更新、发布页 | 陌生机器下载即用，无安全警告 |
| **M5 可选** | 原生通知（轮询 `/api/status` 未读数变化）、deep link `yarr://`、远程模式（连自建服务器） | 每项独立可开关 |

建议顺序：**先把 M0 做掉再谈细节** —— 原型能跑通，「加载本地 URL」这条主线就锁定了，后面全是工程化。

---

## 9. 需要你拍板的 4 件事

1. **平台优先级**：先把 macOS 跑通（你的机器），还是三平台同步推进？三平台同步会把 CI 与签名成本提前。
2. **是否需要「超出包装」的原生功能**（原生通知 / 全局快捷键 / deep link）？需要 → M3 要加注入层；不需要 → 壳可以更薄、更不容易被上游打穿。
3. **是否保留「浏览器也能访问同一份数据」**？sidecar 天然支持，但要求端口固定（随机端口用户记不住）。这会反过来影响 §3.2 第 1 条。
4. **是否愿意把通用改进 PR 回上游**？愿意 → 我后续会把每个改动标成「可上游化 / 只能私有」，帮你把长期 diff 压到最小。

---

## 10. 明确不推荐的路线

- **Rust 重写 yarr**：丢掉上游全部迭代（这个项目 2020 年至今持续维护），成本远大于收益。
- **把前端 fork 进 Tauri**：冲突集中在上游最活跃的目录，等于给未来的自己挖坑。
- **直接嵌上游的 `gui` 构建（带 systray）**：Tauri 托盘 + systray 双托盘，退出语义互相打架，还多一个进程要管。
- **开 `--auth` 做「本地安全」**：Secure cookie 在 `http://127.0.0.1` 的 WebView 里不可靠；安全应由「只绑 127.0.0.1 + 单实例」保证。

---

## 11. 实施记录（2026-11-02，M0–M2 已完成）

### 11.1 与规划的偏差（都是有原因的）

| 规划 | 实际 | 原因 |
| --- | --- | --- |
| Tauri 工程平铺在 `desktop/shell/` | 改为 `desktop/shell/src-tauri/` + `desktop/shell/ui/` | Tauri CLI 只认 `src-tauri/tauri.conf.json`；不改结构就得放弃 `tauri build` / `tauri dev` / CI 官方路径 |
| 用 `tauri-plugin-shell` 的 sidecar API | 改用 `std::process::Command` 自行托管 | 需要拿到 stdout/stderr 做错误页与日志、需要按 PID 收尸（SIGTERM→SIGKILL）、需要控制端口参数；顺带少一个插件权限面 |
| 失败时导航回本地状态页 | 改成向远端页面注入遮罩层 | 远端页面收不到 Tauri 事件；导航回去会丢当前阅读状态，遮罩 + 自动重载更平滑 |
| 规划未涉及 | 补了 SIGTERM/SIGINT 处理 | 实测发现 `kill -TERM` 不走 `RunEvent::Exit`，sidecar 会变孤儿 |
| 规划未涉及 | 补了启动前清理孤儿 sidecar | 被 `kill -9`/「强制退出」后孤儿会占着同一 SQLite；下次启动按 `-db` 路径匹配清理 |
| 规划未涉及 | 补了 `YARR_DATA_DIR` 调试开关 | 沙箱/CI 环境无法写 `~/Library/Application Support`，需要可覆盖的数据目录 |

### 11.2 实测踩到的坑

1. **沙箱环境写 `Application Support` 会让 SQLite 报 `disk I/O error: operation not permitted`**。这不是代码 bug，但说明「启动失败要能看到原因」是刚需 —— 错误页 + 日志尾部因此是 M2 的必做项，不是加分项。
2. **`open -a` 会被单实例插件拦截**：旧实例还活着时，`open` 只是激活它，不会启动新构建。验证新构建前必须先 `pkill`。
3. **`go.mod` 要求 Go 1.26，本机是 1.25**：需要 `GOTOOLCHAIN=auto` + `GOSUMDB=sum.golang.org`（关掉 sumdb 会导致工具链下载失败）。已写进构建脚本和 CI。
4. **Tauri 2.12.x 才包含 origin 混淆 CVE 的修复**：Cargo 实际解析到 `tauri 2.12.5`，符合安全基线。

### 11.3 验证证据（可复现）

```
$ ./desktop/scripts/smoke.sh
PASS  启动就绪，端口 61497
PASS  首页由内嵌前端正常返回
PASS  API /api/status 可用
PASS  崩溃自愈：新 pid 80159，端口不变
PASS  退出回收（无残留进程）
```

补充验证：

- WebView 确实加载了页面：`lsof -iTCP:<port>` 可见 WebKit 网络进程与 yarr 的 ESTABLISHED 连接。
- 打包产物 `yarr.app` 43 MB，`Contents/MacOS/` 内同时有 `yarr-desktop`（32 MB）与 `yarr`（13 MB）。
- `Info.plist` 已合并 `NSAppTransportSecurity.NSAllowsLocalNetworking`（放行回环明文 HTTP）。
- 退出后生成 `.window-state.json`，窗口尺寸/位置持久化生效。
- 单实例：连续 `open` 两次，进程数仍为 1 个壳 + 1 个 sidecar。

### 11.4 尚未验证（需要人工点击）

- 托盘图标与菜单项的实际点击（`打开主窗口` / `重新加载` / `打开日志` / `退出`）。
- macOS 关窗「隐藏而非退出」。
- OPML 导出在 WebView 内的下载行为。

### 11.5 下一步

- **M3**：About 面板显示双版本号（壳版本 + 上游 `UPSTREAM`）、首启导入引导、代理与开机自启。
- **M4**：三平台 CI、Developer ID 签名 + 公证、`tauri-plugin-updater` 自动更新。

### 11.6 补丁 001：桌面端 JS 对话框（2026-11-02）

**症状**：新增订阅弹窗里的「新建文件夹」点击无反应。

**根因**（两层，第二层是实测才发现的）：

1. wry 0.57.1 的 `WryWebViewUIDelegate` 只实现了 `runOpenPanel`、`requestMediaCapturePermission`、`createWebViewWithConfiguration`、`windowWillClose`，**没有实现** `runJavaScriptAlertPanel/ConfirmPanel/TextInputPanel`。WKWebView 在缺少 delegate 方法时直接把 `alert/confirm/prompt` 当成无效调用：`prompt()` 返回 `null`，`confirm()` 返回 `false`。
2. 受影响的不是一个按钮，而是全部 7 处：新建文件夹（弹窗内 / 移动订阅到新文件夹）、重命名文件夹、改订阅链接、重命名订阅、删除文件夹确认、删除订阅确认。

**修法**：壳注入 `desktop/shell/ui/dialogs.js`（Rust `initialization_script`），提供异步的 `__yarrAskText` / `__yarrAskConfirm` / `__yarrAlert`；上游 2 个文件 8 处调用点改成 `await`，helper 在浏览器环境回退原生 API。登记见 `docs/upstream.md`。

**第二个坑（写进代码注释了）**：第一版用 `position: fixed; z-index: 2147483000` 的遮罩，实测点击穿透到了背后的 `<select>`。原因是 yarr 自己的弹窗是 `<dialog>` + `showModal()`，处于 **top layer**，任何 `z-index` 都盖不过它。改为同样用 `<dialog>.showModal()`，并在老 WebKit（Safari < 15.4）上降级为 fixed 遮罩。

**验证**：用 playwright-core + 缓存里的 Chromium 加载真实页面（`http://127.0.0.1:<port>/`），注入同一个 `dialogs.js`，走完整路径：设置 → 新建订阅 → 新建文件夹 → 输入 → 确定。8 项断言全 PASS，包括服务端确实建出文件夹、下拉框自动选中新文件夹、清理后无残留、无页面 JS 报错。
