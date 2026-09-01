import { describe, expect, it } from "vitest";
import { applySettingsFilter } from "./searchFilter";

const NAV = { general: "通用", appearance: "外观", account: "账号与用量" };

/** 模拟设置窗的 DOM 骨架：三分区，含分组标题、普通行、账号卡深埋的配额开关行。 */
function build(): HTMLElement {
  const body = document.createElement("div");
  body.innerHTML = `
    <div class="main-sec" data-sec="general">
      <div class="row-card">
        <div class="row"><div class="row-text"><div class="row-label">语言</div><div class="row-desc">界面语言</div></div></div>
        <div class="row"><div class="row-text"><div class="row-label">开机自启</div></div></div>
      </div>
      <div class="sec-hint">保存失败会显示在这里</div>
    </div>
    <div class="main-sec" data-sec="appearance">
      <div class="sec-caption">贴纸</div>
      <div class="row-card">
        <div class="row"><div class="row-text"><div class="row-label">主题</div></div></div>
        <div class="row"><div class="row-text"><div class="row-label">贴纸颜色</div></div></div>
      </div>
    </div>
    <div class="main-sec" data-sec="account">
      <div class="row-card provider-card">
        <div class="provider-name">Kimi</div>
        <div class="usage-sticker-row"><span>在贴纸显示配额</span></div>
      </div>
    </div>`;
  return body;
}

const shown = (el: Element | null) => (el as HTMLElement).style.display !== "none";
const sec = (body: HTMLElement, key: string) => body.querySelector(`[data-sec="${key}"]`);

describe("applySettingsFilter（设置搜索过滤，S-1）", () => {
  it("按行文本过滤：命中行保留，未命中行/分区隐藏", () => {
    const body = build();
    const any = applySettingsFilter(body, "语言", NAV);
    expect(any).toBe(true);
    expect(shown(sec(body, "general"))).toBe(true);
    const rows = body.querySelectorAll('[data-sec="general"] .row');
    expect(shown(rows[0])).toBe(true); // 语言
    expect(shown(rows[1])).toBe(false); // 开机自启
    expect(shown(sec(body, "appearance"))).toBe(false);
    expect(shown(sec(body, "account"))).toBe(false);
  });

  it("深埋行可达：「在贴纸显示配额」（usage-sticker-row）命中时保留账号卡", () => {
    const body = build();
    const any = applySettingsFilter(body, "配额", NAV);
    expect(any).toBe(true);
    expect(shown(sec(body, "account"))).toBe(true);
    expect(shown(body.querySelector(".provider-card"))).toBe(true);
    expect(shown(body.querySelector(".usage-sticker-row"))).toBe(true);
    expect(shown(sec(body, "general"))).toBe(false);
  });

  it("卡片自身文本命中（agent 名）整卡保留，卡内行不被单独过滤", () => {
    const body = build();
    applySettingsFilter(body, "kimi", NAV);
    expect(shown(sec(body, "account"))).toBe(true);
    expect(shown(body.querySelector(".usage-sticker-row"))).toBe(true);
  });

  it("导航名命中整区保留（含分组标题与全部行）", () => {
    const body = build();
    applySettingsFilter(body, "外观", NAV);
    expect(shown(sec(body, "appearance"))).toBe(true);
    expect(shown(body.querySelector(".sec-caption"))).toBe(true);
    for (const row of body.querySelectorAll('[data-sec="appearance"] .row')) {
      expect(shown(row)).toBe(true);
    }
    expect(shown(sec(body, "general"))).toBe(false);
  });

  it("分组标题跟随本组卡片：组内无命中时标题一并隐藏", () => {
    const body = build();
    applySettingsFilter(body, "主题", NAV);
    // 「贴纸颜色」未命中 → 行隐藏；「主题」命中 → 卡片保留 → 标题保留
    expect(shown(sec(body, "appearance"))).toBe(true);
    expect(shown(body.querySelector(".sec-caption"))).toBe(true);
    const rows = body.querySelectorAll('[data-sec="appearance"] .row');
    expect(shown(rows[0])).toBe(true);
    expect(shown(rows[1])).toBe(false);
    // 搜索时直挂的提示块让位
    expect(shown(body.querySelector(".sec-hint"))).toBe(false);
  });

  it("下拉未展开选项的隐藏语料（.dd-search）参与行匹配：搜 WezTerm 命中「恢复终端」行", () => {
    const body = document.createElement("div");
    body.innerHTML = `
      <div class="main-sec" data-sec="general">
        <div class="row-card">
          <div class="row"><div class="row-text"><div class="row-label">恢复终端</div></div>
            <div class="dd"><button class="dd-btn">Windows Terminal</button>
              <span class="dd-search" hidden aria-hidden="true">Windows Terminal WezTerm PowerShell</span>
            </div>
          </div>
        </div>
      </div>`;
    const any = applySettingsFilter(body, "wezterm", NAV);
    expect(any).toBe(true);
    expect(shown(body.querySelector(".row"))).toBe(true);
    // 语料不干扰清空复位（它不在过滤直写的选择器里）
    applySettingsFilter(body, "", NAV);
    expect((body.querySelector(".row") as HTMLElement).style.display).toBe("");
  });

  it("零命中返回 false 且全部分区隐藏（驱动「无结果」提示）", () => {
    const body = build();
    const any = applySettingsFilter(body, "不存在的设置xyz", NAV);
    expect(any).toBe(false);
    for (const key of Object.keys(NAV)) expect(shown(sec(body, key))).toBe(false);
  });

  it("清空查询：全部过滤痕迹复位", () => {
    const body = build();
    applySettingsFilter(body, "语言", NAV);
    applySettingsFilter(body, "", NAV);
    for (const el of body.querySelectorAll<HTMLElement>(".main-sec, .row, .row-card, .sec-caption, .sec-hint")) {
      expect(el.style.display).toBe("");
    }
  });
});
