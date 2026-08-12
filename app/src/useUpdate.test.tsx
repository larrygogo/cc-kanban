import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render } from "@testing-library/react";
import { useUpdate } from "./useUpdate";

const mocks = vi.hoisted(() => ({
  enabled: true,
  check: vi.fn(),
  download: vi.fn(),
  listen: vi.fn(async () => () => {}),
}));

vi.mock("./api", () => ({
  getSettings: async () => ({ auto_update_enabled: mocks.enabled }),
  checkUpdate: () => mocks.check(),
  downloadUpdate: () => mocks.download(),
  installDownloadedUpdate: async () => {},
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
}));

function Probe() {
  const { status } = useUpdate({ automatic: true, delayMs: 10 });
  return <div data-testid="status">{status}</div>;
}

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  mocks.enabled = true;
  mocks.check.mockReset();
  mocks.download.mockReset();
  mocks.listen.mockReset();
  mocks.listen.mockImplementation(async () => () => {});
});

describe("automatic updater", () => {
  it("默认开启时延迟检查并在发现版本后自动下载", async () => {
    vi.useFakeTimers();
    mocks.check.mockResolvedValue({
      version: "9.9.9",
      body: null,
      downloadState: "available",
    });
    mocks.download.mockResolvedValue("ready");
    render(<Probe />);

    await act(async () => {
      await Promise.resolve();
      await vi.advanceTimersByTimeAsync(10);
    });

    expect(mocks.check).toHaveBeenCalledTimes(1);
    expect(mocks.download).toHaveBeenCalledTimes(1);
  });

  it("关闭自动更新时不进行后台检查或下载", async () => {
    vi.useFakeTimers();
    mocks.enabled = false;
    const view = render(<Probe />);

    await act(async () => {
      await Promise.resolve();
      await vi.advanceTimersByTimeAsync(100);
    });

    expect(mocks.check).not.toHaveBeenCalled();
    expect(mocks.download).not.toHaveBeenCalled();
    // 没检查过就是 unknown，不能谎报 latest（「关于」页会据此显示「已是最新版本」）。
    expect(view.getByTestId("status").textContent).toBe("unknown");
  });

  it("组件卸载早于异步监听注册完成时仍会注销监听", async () => {
    const resolvers: Array<(unlisten: () => void) => void> = [];
    const unlisten = vi.fn();
    mocks.listen.mockImplementation(() => new Promise((resolve) => resolvers.push(resolve)));

    const view = render(<Probe />);
    expect(resolvers).toHaveLength(4);
    view.unmount();

    await act(async () => {
      resolvers.forEach((resolve) => resolve(unlisten));
      await Promise.resolve();
    });

    expect(unlisten).toHaveBeenCalledTimes(4);
  });
});
