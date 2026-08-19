// 手机远程配对卡:扫码地址必须内含 token(二维码编码的正是这串 URL),开关落盘走
// set_settings,启动失败如实红字。
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor } from "@testing-library/react";

const api = vi.hoisted(() => ({
  getSettings: vi.fn(),
  setSettings: vi.fn(),
  remoteAccessInfo: vi.fn(),
}));
vi.mock("../../api", async (o) => ({ ...(await o<typeof import("../../api")>()), ...api }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => {}),
}));

import { RemoteAccessCard } from "./RemoteAccessCard";
import { SETTINGS_DEFAULTS } from "./state";
import { zh } from "../../i18n/zh";

const TOKEN = "deadbeefdeadbeefdeadbeefdeadbeef11223344";

beforeEach(() => {
  Object.values(api).forEach((m) => m.mockReset());
  api.setSettings.mockResolvedValue(undefined);
});
afterEach(() => cleanup());

describe("RemoteAccessCard", () => {
  it("开启后二维码在场,复制拿到含 token 的完整地址;有 Tailscale 默认选它", async () => {
    api.getSettings.mockResolvedValue({
      ...SETTINGS_DEFAULTS,
      remote_access_enabled: true,
      remote_access_port: 18620,
      remote_access_token: TOKEN,
    });
    // 后端 Tailscale 优先排序;桌面猜不出手机在哪个网,Tailscale 是唯一不挑网的地址,
    // 默认必须落它,而不是局域网 IP。
    api.remoteAccessInfo.mockResolvedValue({
      enabled: true,
      port: 18620,
      token: TOKEN,
      ips: [
        { ip: "100.64.0.7", kind: "tailscale" },
        { ip: "192.168.1.5", kind: "lan" },
      ],
      lastError: null,
    });
    // 标注入参:vi.fn 零参推断会让 mock.calls[0][0] 变成空元组取值报 TS2493。
    const writeText = vi.fn((_text: string) => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    render(<RemoteAccessCard />);

    // 地址不明文上屏(含 token 的凭据串),入口只有复制按钮与二维码本体。
    fireEvent.click(await screen.findByText(zh.remote.copy));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    const copied = writeText.mock.calls[0][0];
    expect(copied).toMatch(/^http:\/\/100\.64\.0\.7:18620\/#token=/);
    expect(copied).toContain(`#token=${TOKEN}`);
    expect(screen.queryByText(copied)).toBeNull();
    // 二维码就地生成(uqr renderSVG),扫码区确有 <svg>。
    expect(document.querySelector(".remote-qr svg")).toBeTruthy();
    // 选中候选的可达性说明如实标注。
    expect(screen.getByText(zh.remote.hintTailscale)).toBeTruthy();
  });

  it("关闭态点开关:落盘 remote_access_enabled=true", async () => {
    api.getSettings.mockResolvedValue({ ...SETTINGS_DEFAULTS, remote_access_enabled: false });
    api.remoteAccessInfo.mockResolvedValue({ enabled: false, port: 18620, token: "", ips: [], lastError: null });
    render(<RemoteAccessCard />);

    // 关闭态只显开关 + 端口 + 关闭提示,不渲染二维码。
    expect(await screen.findByText(zh.remote.offHint)).toBeTruthy();
    expect(document.querySelector(".remote-qr")).toBeNull();

    fireEvent.click(screen.getByRole("switch", { name: zh.remote.enable }));
    await waitFor(() =>
      expect(api.setSettings).toHaveBeenCalledWith(
        expect.objectContaining({ remote_access_enabled: true }),
      ),
    );
  });

  it("启动失败(端口被占)红字显示原因", async () => {
    api.getSettings.mockResolvedValue({ ...SETTINGS_DEFAULTS, remote_access_enabled: true, remote_access_token: TOKEN });
    api.remoteAccessInfo.mockResolvedValue({
      enabled: true,
      port: 18620,
      token: TOKEN,
      ips: [],
      lastError: "绑定端口 18620 失败：地址已被占用",
    });
    render(<RemoteAccessCard />);

    expect(await screen.findByText(zh.remote.startError("绑定端口 18620 失败：地址已被占用"))).toBeTruthy();
    // 无可达 IP → 回退手输提示,不出二维码。
    expect(await screen.findByText(zh.remote.noIp)).toBeTruthy();
    expect(document.querySelector(".remote-qr")).toBeNull();
  });
});
