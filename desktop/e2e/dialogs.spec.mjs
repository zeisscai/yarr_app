/**
 * 桌面端对话框 E2E。
 *
 * 为什么需要它：WKWebView 不实现 alert/confirm/prompt（wry 的 WKUIDelegate 缺这三个方法），
 * 壳用 `desktop/shell/ui/dialogs.js` 注入了一套异步对话框。这个脚本用真实浏览器加载
 * 真实页面，把「新建订阅 → 新建文件夹」整条路径点一遍，确认不是"看起来有代码"而是真的能用。
 *
 * 前置：
 *   1. 应用已启动（或任何 yarr 实例在跑）；
 *   2. 在本目录执行过 npm install（只依赖 playwright-core）。
 *
 * 用法：
 *   cd desktop/e2e && npm install && node dialogs.spec.mjs
 *
 * 环境变量：
 *   BASE       直接指定 http://host:port/，不指定则从应用日志里读端口
 *   CHROMIUM   Chromium 可执行文件路径；不指定则自动在 ms-playwright 缓存里找
 *
 * 退出码：0 全通过，1 有失败。
 */
import { chromium } from "playwright-core";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const DIALOGS = join(HERE, "../shell/ui/dialogs.js");
const APP_LOG = join(
  homedir(),
  "Library/Application Support/app.yarr.desktop/yarr.log",
);
const FOLDER_NAME = "E2E 测试文件夹";

const results = [];
const pass = msg => {
  results.push([true, msg]);
  console.log("PASS  " + msg);
};
const fail = msg => {
  results.push([false, msg]);
  console.error("FAIL  " + msg);
};

function resolveBase() {
  if (process.env.BASE) return process.env.BASE.replace(/\/?$/, "/");
  if (!existsSync(APP_LOG)) {
    throw new Error(
      `找不到应用日志 ${APP_LOG}；请先启动 yarr.app，或用 BASE=http://127.0.0.1:<port>/ 指定`,
    );
  }
  const matches = readFileSync(APP_LOG, "utf8").match(/127\.0\.0\.1:\d+/g);
  if (!matches || !matches.length) throw new Error("日志里没有找到监听端口");
  return "http://" + matches[matches.length - 1] + "/";
}

function resolveChromium() {
  if (process.env.CHROMIUM) return process.env.CHROMIUM;
  const cache = join(homedir(), "Library/Caches/ms-playwright");
  if (!existsSync(cache)) return undefined;
  const candidates = readdirSync(cache)
    .filter(d => d.startsWith("chromium-"))
    .sort()
    .reverse()
    .map(d => join(cache, d, "chrome-mac/Chromium.app/Contents/MacOS/Chromium"))
    .filter(p => existsSync(p));
  return candidates[0];
}

async function main() {
  if (!existsSync(DIALOGS)) throw new Error("找不到注入脚本: " + DIALOGS);
  const base = resolveBase();
  const executablePath = resolveChromium();
  console.log(`目标: ${base}`);
  console.log(`Chromium: ${executablePath || "<playwright 默认>"}`);

  const browser = await chromium.launch({ executablePath, headless: true });
  const page = await browser.newPage({ viewport: { width: 1280, height: 860 } });
  await page.addInitScript({ path: DIALOGS });

  const pageErrors = [];
  page.on("pageerror", e => pageErrors.push(String(e)));

  try {
    await page.goto(base, { waitUntil: "networkidle" });

    if ((await page.evaluate(() => typeof window.__yarrAskText)) !== "function") {
      fail("注入脚本未生效（window.__yarrAskText 不存在）");
      throw new Error("abort");
    }
    pass("注入脚本已生效");

    // 打开「设置 → 新建订阅」弹窗
    await page.click('button[title="设置"]');
    await page.getByText("新建订阅").first().click();
    await page.waitForTimeout(300);

    // 场景一：取消（Esc）不应创建文件夹
    await page.getByText("新建文件夹").first().click();
    await page.waitForSelector(".__yarr_dlg", { timeout: 3000 });
    pass("点击「新建文件夹」弹出自绘对话框");

    const message = (await page.textContent(".__yarr_dlg_msg")) || "";
    if (!message.includes("文件夹")) fail(`对话框文案异常: ${message}`);
    else pass(`对话框文案来自应用 i18n: ${message.trim()}`);

    await page.keyboard.press("Escape");
    await page.waitForTimeout(400);
    if (await page.$(".__yarr_dlg")) fail("Esc 后对话框未关闭");
    else pass("Esc 关闭对话框");
    const afterCancel = await page.evaluate(async () => (await fetch("/api/folders")).json());
    if (afterCancel.some(f => f.title === FOLDER_NAME)) fail("取消后却创建了文件夹");
    else pass("取消不产生副作用");

    // 场景二：输入并确认，应创建文件夹并被选中
    await page.getByText("新建文件夹").first().click();
    await page.waitForSelector(".__yarr_dlg_input", { timeout: 3000 });
    await page.fill(".__yarr_dlg_input", FOLDER_NAME);
    await page.click(".__yarr_dlg_primary");
    await page.waitForTimeout(800);

    if (await page.$(".__yarr_dlg")) fail("确认后对话框未关闭");
    else pass("确认后对话框关闭");

    const folders = await page.evaluate(async () => (await fetch("/api/folders")).json());
    const created = folders.find(f => f.title === FOLDER_NAME);
    if (!created) fail("服务端没有创建出文件夹: " + JSON.stringify(folders));
    else pass("服务端已创建文件夹 id=" + created.id);

    if (created) {
      const selected = await page.$eval("#feed-folder", el => el.value);
      if (String(selected) !== String(created.id)) fail("下拉框未自动选中新文件夹: " + selected);
      else pass("下拉框已自动选中新文件夹");

      const code = await page.evaluate(
        async id => (await fetch("/api/folders/" + id, { method: "DELETE" })).status,
        created.id,
      );
      if (code >= 400) fail("清理测试文件夹失败，HTTP " + code);
      else pass("已清理测试文件夹");
    }

    // 删除确认走的是 confirm()，同样依赖注入；确认它能返回 true
    const confirmWorks = await page.evaluate(async () => {
      const p = window.__yarrAskConfirm("E2E: 确认返回 true？");
      await new Promise(r => setTimeout(r, 100));
      document.querySelector(".__yarr_dlg_primary").click();
      return p;
    });
    if (confirmWorks !== true) fail("confirm 未返回 true");
    else pass("confirm 返回 true（删除类操作可正常确认）");

    if (pageErrors.length) fail("页面 JS 报错: " + pageErrors.join(" | "));
    else pass("无页面 JS 报错");
  } finally {
    await browser.close();
  }
}

main()
  .then(() => {
    const failed = results.filter(([ok]) => !ok).length;
    console.log(
      `\n${failed === 0 ? "全部通过" : "有失败"}：${results.length - failed}/${results.length}`,
    );
    process.exit(failed === 0 ? 0 : 1);
  })
  .catch(err => {
    console.error("\nE2E 中止:", err.message);
    process.exit(1);
  });
