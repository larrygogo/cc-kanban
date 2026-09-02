// 设置窗口的通用 UI 组件：开关、分段选择、色板、离散滑块。
// 纯展示、无业务耦合，供各 section 复用。
// 弹层菜单（Dropdown / ActionMenu / useMenuPopup）已收敛到 ../menu——全项目只有那一份实现。
import { STICKER_COLORS, STICKER_COLOR_KEYS } from "../../appearance";
import { useT } from "../../i18n";
import { formatBackendError } from "../../i18n/errors";

/** 设置保存失败的统一错误行：patch 失败时开关会被回读「弹回去」，没有这一行用户
 *  只能看到界面自己变回去、零解释（S-3——曾只有网络分区把错误显示了出来）。
 *  常驻错误加关闭 ×：错误不会自己消失（下一次成功才清），用户看完得有办法关掉。
 *  渲染点兜底过一遍 formatBackendError（产出端 state.ts 已格式化，这里是幂等双保险）。
 *  7S-7：本来长在 About.tsx 里、只有三处在用；网络与远程两处各写各的裸 div（无 ×、
 *  无 role=alert）。抽到这里供全部分区共用。 */
export function SettingsError({ error, onDismiss }: { error: string | null; onDismiss: () => void }) {
  const t = useT();
  if (!error) return null;
  return (
    <div className="sec-hint proxy-err" role="alert">
      {formatBackendError(error, t.locale)}
      <button
        type="button"
        aria-label={t.settings.close}
        style={{ appearance: "none", background: "none", border: "none", padding: 0, marginLeft: 6, font: "inherit", color: "inherit", cursor: "pointer" }}
        onClick={onDismiss}
      >
        ×
      </button>
    </div>
  );
}

export function Switch({ checked, onChange, disabled, label }: { checked: boolean; onChange: () => void; disabled?: boolean; label: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      className={"pswitch" + (checked ? " on" : "")}
      onClick={onChange}
    >
      <span className="pswitch-knob" />
    </button>
  );
}

// 一排互斥的分段按钮（外观模式 / 界面密度）：语义上是单选，用 radiogroup/radio。
// roving tabindex + 方向键导航与 SwatchPicker 同款（radiogroup 的键盘规约：
// 只有选中项在 Tab 序里，方向键移动即选中）。
export function Segmented<T extends string | number>({
  value,
  options,
  onChange,
  label,
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  label: string;
}) {
  return (
    <div className="seg" role="radiogroup" aria-label={label}>
      {options.map((o, i) => (
        <button
          type="button"
          role="radio"
          aria-checked={o.value === value}
          tabIndex={o.value === value ? 0 : -1}
          key={String(o.value)}
          className={"seg-btn" + (o.value === value ? " on" : "")}
          onClick={() => onChange(o.value)}
          onKeyDown={(e) => {
            const handledKeys = ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", " ", "Enter"];
            if (!handledKeys.includes(e.key)) return;
            e.preventDefault();

            const next =
              e.key === "Home"
                ? 0
                : e.key === "End"
                  ? options.length - 1
                  : e.key === "ArrowLeft" || e.key === "ArrowUp"
                    ? (i - 1 + options.length) % options.length
                    : (i + 1) % options.length;

            const opt = options[next];
            if (opt) onChange(opt.value);

            const radios = Array.from(e.currentTarget.parentElement?.querySelectorAll<HTMLElement>("[role=radio]") ?? []);
            radios[next]?.focus();
          }}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

// 贴纸颜色色板：一排圆色块（鲜亮代表色），选中加高亮描边圈；点选即换。语义上单选，用 radiogroup/radio。
export function SwatchPicker({
  value,
  onChange,
  label,
  names,
}: {
  value: string;
  onChange: (v: string) => void;
  label: string;
  names: Record<string, string>;
}) {
  return (
    <div className="swatches" role="radiogroup" aria-label={label}>
      {STICKER_COLOR_KEYS.map((k) => (
        <button
          type="button"
          role="radio"
          aria-checked={k === value}
          tabIndex={k === value ? 0 : -1}
          key={k}
          className={"swatch" + (k === value ? " sel" : "") + (k === "neutral" ? " swatch-none" : "")}
          style={{ background: STICKER_COLORS[k].swatch }}
          data-tip={names[k] ?? k}
          aria-label={names[k] ?? k}
          onClick={() => onChange(k)}
          onKeyDown={(e) => {
            const handledKeys = ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", " ", "Enter"];
            if (!handledKeys.includes(e.key)) return;
            e.preventDefault();

            const cur = STICKER_COLOR_KEYS.indexOf(k);
            const next =
              e.key === "Home"
                ? 0
                : e.key === "End"
                  ? STICKER_COLOR_KEYS.length - 1
                  : e.key === "ArrowLeft" || e.key === "ArrowUp"
                    ? (cur - 1 + STICKER_COLOR_KEYS.length) % STICKER_COLOR_KEYS.length
                    : (cur + 1) % STICKER_COLOR_KEYS.length;

            const nextKey = STICKER_COLOR_KEYS[next];
            if (nextKey) onChange(nextKey);

            const radios = Array.from(e.currentTarget.parentElement?.querySelectorAll<HTMLElement>("[role=radio]") ?? []);
            radios[next]?.focus();
          }}
        />
      ))}
    </div>
  );
}

// 三等分离散滑块（字体大小 小/中/大）：轨道 + 滑钮 + 底部标签。
export function FontSizeSlider({
  value,
  options,
  onChange,
  label,
}: {
  value: number;
  options: { value: number; label: string }[];
  onChange: (v: number) => void;
  label: string;
}) {
  const index = Math.max(0, options.findIndex((o) => o.value === value));
  return (
    <div className="dslider" role="radiogroup" aria-label={label}>
      <div className="dslider-track">
        <div className="dslider-knob-wrap">
          <div className="dslider-knob" style={{ left: `${(index / (options.length - 1)) * 100}%` }} />
        </div>
        {options.map((o, i) => (
          <button
            key={o.value}
            type="button"
            role="radio"
            aria-checked={o.value === value}
            tabIndex={o.value === value ? 0 : -1}
            className="dslider-point"
            aria-label={o.label}
            onClick={() => onChange(o.value)}
            onKeyDown={(e) => {
              // 与 Segmented/SwatchPicker 同款：radiogroup 内方向键移动即选中。
              const handledKeys = ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", " ", "Enter"];
              if (!handledKeys.includes(e.key)) return;
              e.preventDefault();

              const next =
                e.key === "Home"
                  ? 0
                  : e.key === "End"
                    ? options.length - 1
                    : e.key === "ArrowLeft" || e.key === "ArrowUp"
                      ? (i - 1 + options.length) % options.length
                      : (i + 1) % options.length;

              const opt = options[next];
              if (opt) onChange(opt.value);

              const radios = Array.from(e.currentTarget.parentElement?.querySelectorAll<HTMLElement>("[role=radio]") ?? []);
              radios[next]?.focus();
            }}
          />
        ))}
      </div>
      <div className="dslider-labels">
        {options.map((o) => (
          <span key={o.value} className="dslider-label">{o.label}</span>
        ))}
      </div>
    </div>
  );
}
