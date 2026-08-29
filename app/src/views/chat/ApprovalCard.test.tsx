import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ApprovalCard, ApprovalCommandDetail, isRiskyCommand } from "./ApprovalCard";

afterEach(cleanup);

describe("isRiskyCommand 风险分级(C-9)", () => {
  it("破坏性命令判危险:rm -rf / sudo / git reset --hard / dd of=", () => {
    expect(isRiskyCommand("rm -rf build")).toBe(true);
    expect(isRiskyCommand("sudo apt install foo")).toBe(true);
    expect(isRiskyCommand("git reset --hard HEAD~1")).toBe(true);
    expect(isRiskyCommand("git clean -fd")).toBe(true);
    expect(isRiskyCommand("dd if=/dev/zero of=/dev/sda")).toBe(true);
    expect(isRiskyCommand("{\"command\":\"rm -rf node_modules\"}")).toBe(true);
  });

  it("安全命令不染色(宁漏勿冤):ls / cargo test / git status", () => {
    expect(isRiskyCommand("ls -la")).toBe(false);
    expect(isRiskyCommand("cargo test")).toBe(false);
    expect(isRiskyCommand("git status")).toBe(false);
    expect(isRiskyCommand("git push origin main")).toBe(false);
    expect(isRiskyCommand("{\"command\":\"cargo test\"}")).toBe(false);
  });
});

describe("ApprovalCard 外壳(G-16)", () => {
  it("role=alertdialog 且出现时把焦点交给第一个动作按钮", () => {
    render(
      <ApprovalCard
        title="Agent 请求权限"
        sideActions={<button type="button">收起</button>}
        actions={<button type="button">允许一次</button>}
      />,
    );
    const dialog = screen.getByRole("alertdialog");
    expect(dialog.getAttribute("aria-label")).toBe("Agent 请求权限");
    expect(dialog.getAttribute("aria-modal")).toBe("false");
    // 动作区第一个按钮获焦——role=alert 时代焦点进不来，用户听不到也摸不着。
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "允许一次" }));
  });
});

describe("ApprovalCommandDetail 长命令出口(C-10)", () => {
  const labels = {
    label: "完整参数",
    expandLabel: "展开全部",
    collapseLabel: "收起",
    copyLabel: "复制命令",
    copiedLabel: "已复制",
  };

  it("「展开全部」给详情挂上 is-expanded,再点收回", () => {
    const { container } = render(
      <ApprovalCard title="Agent 请求权限">
        <ApprovalCommandDetail command="very long command" {...labels} />
      </ApprovalCard>,
    );
    const detail = container.querySelector(".chat-approval-detail")!;
    expect(detail.className).not.toContain("is-expanded");
    fireEvent.click(screen.getByRole("button", { name: "展开全部" }));
    expect(detail.className).toContain("is-expanded");
    // 展开后按钮文案换成「收起」。
    fireEvent.click(screen.getByRole("button", { name: "收起" }));
    expect(detail.className).not.toContain("is-expanded");
  });

  it("「复制命令」整段进剪贴板并给已复制反馈", async () => {
    const writeText = vi.fn((_text: string) => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText } });
    render(
      <ApprovalCard title="Agent 请求权限">
        <ApprovalCommandDetail command="echo hello" {...labels} />
      </ApprovalCard>,
    );
    fireEvent.click(screen.getByRole("button", { name: "复制命令" }));
    expect(writeText).toHaveBeenCalledWith("echo hello");
    expect(await screen.findByRole("button", { name: "已复制" })).toBeTruthy();
  });
});
