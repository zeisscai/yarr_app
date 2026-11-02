/**
 * 桌面端 JS 对话框垫片。
 *
 * 背景：wry 的 WKUIDelegate 只实现了文件选择、媒体权限、新窗口，
 * 没有实现 runJavaScriptAlertPanel / ConfirmPanel / TextInputPanel，
 * 于是 WKWebView 里 alert/confirm/prompt 全部静默失效（prompt 返回 null，
 * confirm 返回 false）。yarr 的「新建/重命名文件夹、改 feed 链接、删除确认」
 * 恰好全依赖这三个同步 API。
 *
 * 做法：在 document start 注入本脚本，提供等价的「异步版」实现，
 * 由壳侧把上游 7 处调用点改成 await 调用；浏览器环境下调用点回退到原生 API。
 *
 * 语义严格对齐原生：
 *   prompt(message, default) -> string（确定，可能是空串）| null（取消）
 *   confirm(message)         -> true | false
 *
 * 关键实现细节：必须用 <dialog>.showModal()，不能只靠 z-index。
 * yarr 自己的弹窗（components/modal.vue）就是 <dialog>，处于 top layer，
 * 任何正整数的 z-index 都盖不过它 —— 早先用 fixed 遮罩时，点击会被背后的
 * <select> 命中。
 */
(function () {
  if (window.__yarrDialog) return;

  // 注入时机在 document start，documentElement 有可能尚未就绪，必须做空值保护
  const rootLang = document.documentElement ? document.documentElement.lang : "";
  const ZH = /^zh/i.test(rootLang || navigator.language || "");
  const L = ZH ? { ok: "确定", cancel: "取消" } : { ok: "OK", cancel: "Cancel" };

  const CSS = `
  dialog.__yarr_dlg {
    border: 0; padding: 0; margin: auto; background: transparent; overflow: visible;
    font: 13px/1.6 -apple-system, BlinkMacSystemFont, "Helvetica Neue", "PingFang SC", sans-serif;
    color: #1c1c1a;
  }
  dialog.__yarr_dlg::backdrop { background: rgba(0, 0, 0, 0.38); }
  .__yarr_dlg_box {
    width: 360px; max-width: calc(100vw - 48px);
    padding: 18px 18px 14px;
    border-radius: 12px;
    background: #fff; color: #1c1c1a;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.28);
    border: 0.5px solid rgba(0, 0, 0, 0.12);
    text-align: left;
  }
  .__yarr_dlg_msg { margin: 0 0 12px; font-size: 13px; white-space: pre-wrap; word-break: break-word; }
  .__yarr_dlg_input {
    width: 100%; box-sizing: border-box; margin: 0 0 14px;
    padding: 7px 10px; font: inherit; color: inherit;
    background: #f6f6f4; border: 0.5px solid rgba(0, 0, 0, 0.18); border-radius: 7px;
    outline: none; -webkit-user-select: text; user-select: text;
  }
  .__yarr_dlg_input:focus { border-color: rgba(0, 0, 0, 0.45); }
  .__yarr_dlg_actions { display: flex; justify-content: flex-end; gap: 8px; }
  .__yarr_dlg_btn {
    font: inherit; font-size: 12px; padding: 6px 14px;
    border-radius: 7px; border: 0.5px solid rgba(0, 0, 0, 0.18);
    background: #fff; color: inherit; cursor: pointer;
  }
  .__yarr_dlg_btn:hover { border-color: rgba(0, 0, 0, 0.4); }
  .__yarr_dlg_primary { background: #1c1c1a; color: #fff; border-color: transparent; }
  @media (prefers-color-scheme: dark) {
    dialog.__yarr_dlg { color: #ececea; }
    .__yarr_dlg_box { background: #232322; color: #ececea; border-color: rgba(255, 255, 255, 0.14); }
    .__yarr_dlg_input { background: #161615; border-color: rgba(255, 255, 255, 0.2); }
    .__yarr_dlg_input:focus { border-color: rgba(255, 255, 255, 0.5); }
    .__yarr_dlg_btn { background: #232322; border-color: rgba(255, 255, 255, 0.2); }
    .__yarr_dlg_btn:hover { border-color: rgba(255, 255, 255, 0.5); }
    .__yarr_dlg_primary { background: #ececea; color: #171715; }
  }
  /* 老 WebKit（macOS 11/12 的 Safari 14）没有 showModal，退化成 fixed 遮罩 */
  dialog.__yarr_dlg.__yarr_dlg_fallback {
    position: fixed; inset: 0; width: 100%; height: 100%;
    display: flex; align-items: center; justify-content: center;
    background: rgba(0, 0, 0, 0.38); z-index: 2147483000;
  }
  `;

  function ensureStyle() {
    if (document.getElementById("__yarr_dlg_style")) return;
    const style = document.createElement("style");
    style.id = "__yarr_dlg_style";
    style.textContent = CSS;
    (document.head || document.documentElement).appendChild(style);
  }

  function open(kind, message, defaultValue) {
    return new Promise(resolve => {
      ensureStyle();
      const previous = document.activeElement;

      const dlg = document.createElement("dialog");
      dlg.className = "__yarr_dlg";

      const box = document.createElement("div");
      box.className = "__yarr_dlg_box";
      box.setAttribute("role", "dialog");
      box.setAttribute("aria-modal", "true");

      const msg = document.createElement("p");
      msg.className = "__yarr_dlg_msg";
      msg.textContent = message == null ? "" : String(message);
      box.appendChild(msg);

      let input = null;
      if (kind === "prompt") {
        input = document.createElement("input");
        input.className = "__yarr_dlg_input";
        input.type = "text";
        input.value = defaultValue == null ? "" : String(defaultValue);
        box.appendChild(input);
      }

      const actions = document.createElement("div");
      actions.className = "__yarr_dlg_actions";

      let settled = false;
      const cancelValue = () => (kind === "confirm" ? false : null);
      const okValue = () => (kind === "confirm" ? true : input ? input.value : undefined);

      const finish = value => {
        if (settled) return;
        settled = true;
        document.removeEventListener("keydown", onKey, true);
        try {
          if (typeof dlg.close === "function") dlg.close();
        } catch (_) {}
        dlg.remove();
        if (previous && typeof previous.focus === "function") previous.focus();
        resolve(value);
      };

      const makeButton = (label, primary, onClick) => {
        const b = document.createElement("button");
        b.type = "button";
        b.className = "__yarr_dlg_btn" + (primary ? " __yarr_dlg_primary" : "");
        b.textContent = label;
        b.addEventListener("click", onClick);
        return b;
      };

      if (kind !== "alert") {
        actions.appendChild(makeButton(L.cancel, false, () => finish(cancelValue())));
      }
      actions.appendChild(makeButton(L.ok, true, () => finish(okValue())));
      box.appendChild(actions);
      dlg.appendChild(box);

      const onKey = event => {
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          finish(cancelValue());
        } else if (event.key === "Enter") {
          event.preventDefault();
          event.stopPropagation();
          finish(okValue());
        }
      };
      document.addEventListener("keydown", onKey, true);

      // 原生 Esc / 点遮罩都会触发 cancel
      dlg.addEventListener("cancel", event => {
        event.preventDefault();
        finish(cancelValue());
      });
      dlg.addEventListener("click", event => {
        if (event.target === dlg) finish(cancelValue());
      });

      document.body.appendChild(dlg);

      if (typeof dlg.showModal === "function") {
        dlg.showModal();
      } else {
        dlg.setAttribute("open", "");
        dlg.classList.add("__yarr_dlg_fallback");
      }

      if (input) {
        input.focus();
        input.select();
      } else {
        const primary = actions.querySelector(".__yarr_dlg_primary");
        if (primary) primary.focus();
      }
    });
  }

  window.__yarrDialog = {
    prompt: (message, defaultValue) => open("prompt", message, defaultValue),
    confirm: message => open("confirm", message),
    alert: message => open("alert", message).then(() => undefined),
  };

  // 上游调用点用的便捷函数；桌面壳未注入时（普通浏览器）调用点会回退到原生同步 API。
  window.__yarrAskText = (message, defaultValue) => open("prompt", message, defaultValue);
  window.__yarrAskConfirm = message => open("confirm", message);
  window.__yarrAlert = message => open("alert", message);
})();
