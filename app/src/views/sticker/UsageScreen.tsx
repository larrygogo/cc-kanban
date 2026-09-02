// 底栏「凹陷小屏」用量读数：每个开启配额的 provider 一个图标标签，点选后显示其用量泳道。
import { useState } from "react";
import { agentAssets, tintStyle } from "../../providers";
import { useAgents } from "../../useAgents";
import { useT } from "../../i18n";
import type { ProviderUsage, UsageLane } from "../../api";
import { USAGE_KEY } from "./types";

/** 底部用量：嵌在底栏左侧的「凹陷小屏读数」——标签式：一行品牌图标标签，点选后显示该
   provider 的 5h + 7d/weekly 用量条；与右侧凸起按钮组成「凹陷显示屏 + 凸起按钮」的物理设备面板。 */
// 利用率档位 → 复用应用既有状态色(绿/黄/红)，与卡片状态点同语义；越满越红即预警。
function usageSev(pct: number): string {
  return pct >= 80 ? "is-high" : pct >= 50 ? "is-warn" : "is-ok";
}

// 单条用量泳道（进度条型或余额数值型）
function LaneRow({ lane, label }: { lane: UsageLane; label: string }) {
  if (lane.used_pct != null) {
    const pct = Math.max(0, Math.min(100, lane.used_pct));
    return (
      <div className="stk-urow">
        <span className="stk-ulabel">{label}</span>
        <span className="stk-utrack" role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)}>
          <i className={"stk-ufill " + usageSev(pct)} style={{ width: `${pct}%` }} />
        </span>
        <span className="stk-uval">{Math.round(pct)}%</span>
      </div>
    );
  }
  // 余额型：显数值，不画进度条
  const valText = lane.used != null ? `${lane.used}${lane.unit ? ` ${lane.unit}` : ""}` : "—";
  return (
    <div className="stk-urow">
      <span className="stk-ulabel">{label}</span>
      <span className="stk-uval">{valText}</span>
    </div>
  );
}

/** 标签式用量屏：每个开启配额的 provider 一个图标标签，点选后显示其 5h + 7d/weekly 条。
 *  符合条件 provider 为空 → 不渲染。 */
export function UsageScreen({
  quotaProviders,
  usageMap,
  usageMeta = {},
}: {
  quotaProviders: string[];
  usageMap: Record<string, ProviderUsage>;
  /// 每个 provider 上次成功刷新的时刻与「最近一次刷新是否失败」（7B-8）。
  /// 缺省为空表：拿不到元信息时按新鲜渲染，与旧行为一致。
  usageMeta?: Record<string, { at: number; stale: boolean }>;
}) {
  const t = useT();
  // provider 图标标签的可访问名/悬停提示（P2-7）：按钮内容只有品牌 SVG，
  // 没有名字屏幕阅读器读不出、鼠标悬停无提示。展示名与卡片徽标同源（后端下发名单）。
  const { name: agentNameOf } = useAgents();
  // 用户偏好选中的 provider（持久化：折叠/展开重挂后记住；若不在当前活跃列表中则退回第一个）
  const [selectedPref, setSelectedPref] = useState<string>(() => localStorage.getItem(USAGE_KEY) ?? "");
  const pick = (p: string) => {
    setSelectedPref(p);
    localStorage.setItem(USAGE_KEY, p);
  };

  // 仅显示「在配额列表中且有用量数据」的 provider
  const activeProviders = quotaProviders.filter((p) => !!usageMap[p]);
  if (!activeProviders.length) return null;

  // 选中态：优先用户选择，其次第一个
  const selected = activeProviders.includes(selectedPref) ? selectedPref : activeProviders[0];

  const usage = usageMap[selected];
  const fiveHourLane = usage?.lanes.find((l) => l.kind === "five_hour") ?? null;
  const sevenDayLane = usage?.lanes.find((l) => l.kind === "seven_day" || l.kind === "weekly") ?? null;
  // 模型专属周限(claude 的 Fable/Opus 周配额)。它常比总量先见顶(实测 Fable 83% vs
  // 总量 44%),是真正的约束项,不显示会误判还有余量。旧后端的 opus 泳道同槽展示。
  //
  // 可能同时下发多条(Opus 与 Fable 各一条),而小屏只放得下一条:取**用得最满**的那条。
  // 曾经取数组第一条——顺序由服务端定,可能显示 44% 的那条、把 83% 的藏起来,恰好削掉
  // 这条泳道的立项理由。
  const modelLane = usage?.lanes
    .filter((l) => l.kind === "model_weekly")
    .reduce<UsageLane | null>((tightest, lane) => (
      !tightest || (lane.used_pct ?? -1) > (tightest.used_pct ?? -1) ? lane : tightest
    ), null)
    ?? usage?.lanes.find((l) => l.kind === "opus") ?? null;
  const modelLabel = modelLane?.kind === "opus"
    ? t.account.laneOpus
    : modelLane?.label ? t.account.laneModelWeekly(modelLane.label) : t.account.laneWeekly;

  // 7B-8：刷新失败时读数原样留在屏上，看着和新鲜值一模一样。降饱和 + tip 说明上次
  // 更新时刻——数字仍可用（多半只差几分钟），但「它可能过时了」必须看得出来。
  const meta = usageMeta[selected];
  const stale = meta?.stale === true;

  return (
    <div
      className={"stk-uscreen" + (stale ? " is-stale" : "")}
      role="group"
      aria-label={t.account.quota}
      data-tip={stale && meta ? t.sticker.usageStale(new Date(meta.at).toLocaleTimeString()) : undefined}
    >
      {/* 品牌图标标签行（每个 provider 一个，点选切换） */}
      <div className="stk-utabs">
        {activeProviders.map((p) => {
          const { Icon } = agentAssets(p);
          const label = agentNameOf(p);
          return (
            <button
              key={p}
              type="button"
              className={"stk-utab" + (p === selected ? " on" : "")}
              style={tintStyle(p)}
              aria-pressed={p === selected}
              aria-label={label}
              data-tip={label}
              onClick={() => pick(p)}
            >
              <Icon />
            </button>
          );
        })}
      </div>
      {/* 选中 provider 的 5h、7d/weekly 与模型专属周限用量条 */}
      {fiveHourLane && <LaneRow lane={fiveHourLane} label={t.account.laneFiveHour} />}
      {sevenDayLane && <LaneRow lane={sevenDayLane} label={sevenDayLane.kind === "weekly" ? t.account.laneWeekly : t.account.laneSevenDay} />}
      {modelLane && <LaneRow lane={modelLane} label={modelLabel} />}
    </div>
  );
}
