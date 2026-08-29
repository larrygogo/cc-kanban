import { describe, expect, it } from "vitest";
import { slashArgCommand, slashMatchesFor } from "./composerCompletion";

const commands = [
  { name: "/model" },
  { name: "/mode" },
  { name: "/deploy" },
  { name: "/frontend:component" },
];

describe("slashMatchesFor", () => {
  it("前缀命中照常给候选", () => {
    expect(slashMatchesFor(commands, "/mo").map((c) => c.name)).toEqual(["/model", "/mode"]);
  });

  it("子串命中:词中片段也能补全(此前只前缀匹配,打 /epl 菜单就空了)", () => {
    expect(slashMatchesFor(commands, "/epl").map((c) => c.name)).toEqual(["/deploy"]);
  });

  it("命名空间命令按全名子串命中", () => {
    expect(slashMatchesFor(commands, "/comp").map((c) => c.name)).toEqual(["/frontend:component"]);
  });

  it("大小写不敏感", () => {
    expect(slashMatchesFor(commands, "/MO").map((c) => c.name)).toEqual(["/model", "/mode"]);
  });

  it("前缀命中排在一般子串命中前面", () => {
    // "/de" 对 /deploy 是前缀、对 /cancel:delete 是词中——前缀优先。
    const withMid = [{ name: "/cancel:delete" }, { name: "/deploy" }];
    expect(slashMatchesFor(withMid, "/de").map((c) => c.name)).toEqual(["/deploy", "/cancel:delete"]);
  });

  it("已完整敲出的命令不再占位", () => {
    expect(slashMatchesFor(commands, "/model").map((c) => c.name)).toEqual([]);
  });

  it("带参数或不是斜杠输入时不给候选", () => {
    expect(slashMatchesFor(commands, "/model sonnet")).toEqual([]);
    expect(slashMatchesFor(commands, "hello")).toEqual([]);
  });
});

describe("slashArgCommand", () => {
  it("命令精确命中且带参数(或刚敲空格)时返回该命令", () => {
    expect(slashArgCommand(commands, "/model sonnet")?.name).toBe("/model");
    expect(slashArgCommand(commands, "/model ")?.name).toBe("/model");
  });

  it("未知命令、无参数、普通文本都返回 null", () => {
    expect(slashArgCommand(commands, "/nope arg")).toBeNull();
    expect(slashArgCommand(commands, "/model")).toBeNull();
    expect(slashArgCommand(commands, "hello world")).toBeNull();
  });
});
