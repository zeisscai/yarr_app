/**
 * 桌面端外链垫片。
 *
 * 问题：yarr 的“打开原文”按钮和正文里的外链都是 <a target="_blank">。
 * WKWebView 对 target=_blank 走的是 `createWebViewWithConfiguration`
 * （开新窗口），而 wry 只有在应用注册了 new-window 回调时才处理，否则
 * 直接返回 nil —— 结果是**点击被静默丢弃**，既不导航也不开浏览器。
 * Tauri 没有暴露这个回调，所以只能在页面层把它「降级」成同窗口导航，
 * 交给 Rust 侧的 on_navigation 拦截并转给系统浏览器。
 *
 * 判定规则刻意保守：
 *   - 只接管 http/https/mailto 且跨源的链接（站内路由、锚点不动）
 *   - 保留阻止默认行为后的同窗口跳转，Rust 返回 false 时页面不会真的离开
 */
(function () {
  if (window.__yarrLinks) return;
  window.__yarrLinks = true;

  const EXTERNAL = /^(https?|mailto):/i;

  document.addEventListener(
    "click",
    function (event) {
      // 让用户自己用 ⌘/Ctrl/Shift 点开的行为保持原样
      if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      if (event.defaultPrevented) return;

      const anchor = event.target && event.target.closest
        ? event.target.closest("a[href]")
        : null;
      if (!anchor) return;

      const raw = anchor.getAttribute("href");
      if (!raw || raw.startsWith("#") || raw.startsWith("javascript:")) return;
      if (!EXTERNAL.test(raw)) return;

      let url;
      try {
        url = new URL(raw, location.href);
      } catch (_) {
        return;
      }

      // 站内链接（同源）交给应用自己处理
      if (url.origin === location.origin) return;

      event.preventDefault();
      // 同窗口导航：Rust 的 on_navigation 会拦截、交给系统浏览器并 cancel 掉
      window.location.href = url.href;
    },
    true,
  );
})();
