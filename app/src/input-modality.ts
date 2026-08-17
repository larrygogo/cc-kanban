// 输入模态门控：只有「键盘导航」时才显示焦点框（配合 styles.css 里的 `:root:not([data-im="kbd"]) :focus`）。
//
// 起因：macOS 上贴纸是无边框 NSPanel，托盘点开→面板变 key→WKWebView 成为 first responder，
// WebKit 会自动把焦点给 DOM 里第一个可聚焦元素（本应用里 tab 是 <span>，故落到首张卡片的 `>_` 钮），
// 于是每次打开贴纸都无端亮起一圈 UA 焦点框。
// WebKit 下**程序化 focus 也算 `:focus-visible`**，所以纯 CSS 的 `:focus-visible` 门控压不住；
// 改用「最后一次输入是键盘还是指针」这一模态来门控（业界通行做法）。
//
// 规则：默认（无 data-im）= 指针模态 → 不画焦点框；仅当用户按下导航键才切到键盘模态。
// 面板每次获焦（托盘点开是指针动作）都复位成指针模态，确保自动聚焦不亮环；随后用户一按 Tab 即恢复焦点框。

const NAV_KEYS = new Set(["Tab", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End", "PageUp", "PageDown"]);

function useKeyboard(): void {
  document.documentElement.setAttribute("data-im", "kbd");
}

function usePointer(): void {
  document.documentElement.removeAttribute("data-im");
}

/** 安装输入模态监听（全窗口通用；在 main.tsx 启动时调用一次）。 */
export function installInputModality(): void {
  window.addEventListener(
    "keydown",
    (e) => {
      if (!NAV_KEYS.has(e.key)) return;
      // 可编辑元素内的方向键/Home/End 是**光标移动**（或输入框自己的历史翻阅），不是
      // 「在控件间导航」——据此切键盘模态会让正在打字的输入框平白亮起焦点环
      // （实拍：对话输入框按 ↑ 取回历史时冒出绿框）。Tab 不豁免：它正是离开输入框的导航。
      if (e.key !== "Tab") {
        const el = document.activeElement;
        if (el instanceof HTMLElement && (el.tagName === "TEXTAREA" || el.tagName === "INPUT" || el.isContentEditable)) return;
      }
      useKeyboard();
    },
    true,
  );
  window.addEventListener("mousedown", usePointer, true);
  window.addEventListener("pointerdown", usePointer, true);
  // 面板/窗口获焦（托盘点开）即回到指针模态，避免 first-responder 自动聚焦亮起焦点框。
  window.addEventListener("focus", usePointer);
}
