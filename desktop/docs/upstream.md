# 上游同步手册

本仓库是 [nkanaev/yarr](https://github.com/nkanaev/yarr) 的 fork，桌面版（Tauri）的**全部自有代码只新增在 `desktop/` 下**，上游文件零改动。

- 当前基线：`upstream=v2.9 (a87e819b)`，见 `desktop/UPSTREAM`
- 分支模型：`upstream-mirror`（上游镜像，只做 ff）→ `desktop`（集成分支，GitHub 默认分支）

---

## 一、同步流程

```bash
git fetch upstream --tags                                        # 拉上游与 tag，不合并
git checkout upstream-mirror && git merge --ff-only upstream/master   # 镜像分支只做快进
git checkout desktop && git merge upstream-mirror                # 合进集成分支，冲突只在这里出现
git push origin desktop upstream-mirror                          # 两个分支一起推
git describe --tags --abbrev=0                                   # 确认新基线 tag，回填 desktop/UPSTREAM
```

首次 clone 需先 `git remote add upstream https://github.com/nkanaev/yarr.git`（只需一次）。

**`upstream-mirror` 的纪律：只接受 `--ff-only`，永不在其上提交。** 它的价值是让 `git diff upstream-mirror..desktop --stat` 成为「我们到底改了什么」的完整清单；上游万一强推或 rebase，直接 `git reset --hard upstream/master` 即可，零损失（因为你从不在上面提交）。同步完成后 `desktop/UPSTREAM` 的 `commit` / `tag` / `synced` 三行必须一起更新。

桌面版的构建/发版相关命令见 `docs/DESKTOP-PLAN.md`。

## 二、补丁登记表

凡改动上游文件（`cmd/`、`src/`、`makefile`、`go.mod` 等）都必须在此登记，一行一个补丁。「能否上游化」按 `docs/DESKTOP-PLAN.md §5.7` 判断：对非桌面用户也有价值 → 提 PR。

| 日期 | 文件 | 原因 | 能否上游化 | 状态 |
| --- | --- | --- | --- | --- |
| 2026-11-02 | `src/frontend/js/pages/App.vue` | WKWebView 不实现 `prompt`/`confirm`（wry 的 WKUIDelegate 未实现这三个方法），改为 `await` 壳注入的异步对话框；涉及 6 个调用点 + 2 个 helper | 可上游化（网页端也能受益，且 helper 已做原生回退） | 已应用 |
| 2026-11-02 | `src/frontend/js/components/newfeed.vue` | 同上：新建文件夹的 `prompt` 与「未发现订阅」的 `alert` 各 1 处 | 可上游化 | 已应用 |

当前补丁数：**1 个补丁，涉及 2 个文件、8 处调用点**。改动形态固定：把同步的 `prompt()`/`confirm()`/`alert()` 换成 `await askText()`/`await askConfirm()`/`await alertText()`，两个 helper 在浏览器环境自动回退到原生 API，因此网页版行为不变。

对话框实现本身（DOM、键盘、focus、深浅色）全部在 `desktop/shell/ui/dialogs.js`，由 Rust 侧通过 `initialization_script` 注入，不占上游代码。若上游未来自己改成异步对话框，这两个文件可以直接回退成上游版本，只需保留 helper 或改用原生实现。

## 三、同步日志

| 日期 | upstream commit | 结果 | 备注 |
| --- | --- | --- | --- |
| 2026-11-02 | a87e819b | 基线 | 规划完成，建立 `upstream-mirror` / `desktop` 分支模型 |

## 四、冲突纪律

```bash
git config rerere.enabled true        # 同类冲突自动复用上次解法，长期收益很大（需手动开启一次）
git config merge.conflictStyle zdiff3 # 冲突块里显示共同祖先，判断更准
```

1. **上游语义优先**：先删掉自己的补丁，再看能否用上游的新接口重写。不要「两边都留」，那是技术债的起点。
2. 能绕开的就绕开。改上游文件的唯一合法理由，是 CLI / HTTP 层确实做不到。
3. **解决完必须登记**：在「二、补丁登记表」加一行，并在「三、同步日志」记录本次 commit 与结果。没登记的冲突解决等于没解决 —— 下次同步会再撞一次。

## 五、升级后冒烟清单

同步（或合并 drift PR）后逐条走一遍，约 10 分钟：

1. **构建通过**：`./desktop/scripts/build-sidecar.sh` + `cargo build --manifest-path desktop/shell/Cargo.toml`。
2. **首启新库**：删掉旧 `storage.db` 启动，建库成功、页面正常渲染（上游改了模板 / 静态资源，这里最先炸）。
3. **业务链路**：添加 feed → 刷新 → 标已读 → 搜索，四项都要通。
4. **OPML 导入导出**：导入一个含 10+ feed 的文件，再导出确认条目数一致。
5. **外链走系统浏览器**：点击文章内链接在默认浏览器打开，站内链接仍在应用内。
6. **退出无残留进程**：关闭窗口并退出后 `ps aux | grep '[y]arr'` 应为空（sidecar 必须被回收）。
7. **旧库升级**：用上一版 `storage.db` 副本启动（先备份），确认 schema 迁移正常。
8. **托盘与单实例**：托盘菜单可用、Cmd+Q 生效、再次启动第二实例时激活已有窗口而非起新进程。
