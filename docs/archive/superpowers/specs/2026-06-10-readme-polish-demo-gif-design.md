# README 视觉升级 + 浏览器合成演示 GIF — 设计

日期:2026-06-10
状态:已与用户确认

## 背景与目标

现有 README 内容完整但视觉素:无 logo、仅 2 枚徽章、特性区是 21 条长 bullet。现有 `docs/images/demo.gif` 为全屏实录,壁纸占大半画面且 GIF 调色板严重失真。

目标:
1. 用「浏览器合成」管线生成一张干净、可复现的演示 GIF,替换现有 demo.gif。
2. README 全面视觉升级(纯中文,内容不增不减,只重新组织)。

## 一、演示 GIF:浏览器合成管线

### 架构

```
app/demo.html + app/src/demo/      ← 仅 dev 的演示入口(不进生产构建)
   │  mockIPC 喂假会话数据;渲染「渐变桌面 + 圆角阴影贴纸窗口 + 字幕」
   │  暴露 window.__demo.seek(t) 之类的时间轴控制
   ▼
scripts/record-demo.mjs (Playwright)
   │  启动 vite dev → 打开 demo.html → 按时间轴逐帧截图
   ▼
gifenc 编码 → docs/images/demo.gif
```

- **mock 层**:用 `@tauri-apps/api/mocks` 的 `mockIPC` 拦截全部命令(核心 `get_live_sessions` / `get_settings` / `host_os`,以及 `set_archived`、`rename_session`、`plugin:event|listen` 等)。会话数据由演示脚本按时间轴推进。
- **时间轴驱动**:演示页内置确定性时间轴(按帧号推进,不依赖真实时钟),Playwright 逐帧调用 seek + 截图,保证每次产出逐帧一致。
- **不污染生产**:`demo.html` 不加入 vite build 的 rollup input,生产构建只有 `index.html`;demo 源码放 `app/src/demo/`,App 本体代码不改动(若个别组件需要注入点,以最小 prop/导出调整为限)。

### 分镜(约 18s 循环,760×480,目标 < 4MB)

| # | 内容 | 字幕 |
|---|------|------|
| 1 | 4 张会话卡,2 个橙色「运行中」转圈,activity 文本与 Context % 实时变化 | 所有 Claude Code 会话,一眼看全 |
| 2 | 一个会话转黄色「待交互」,tab 计数跳动,自动切到「待交互」tab | 谁在等你回复,立刻知道 |
| 3 | 卡片上演示重命名 + 归档,卡片收进「已归档」 | 重命名、归档,即点即管 |
| 4 | 贴纸滑向右缘缩成竖状态条(CollapsedStrip),hover 偷看展开再收回 | 吸边缩成一根状态条,不占地方 |
| 5 | 收尾:logo + Meowo + slogan 淡入 | — |

### 输出规格

- 尺寸 760×480(2x 渲染后缩放,保证文字清晰),约 12 fps。
- gifenc 逐帧调色板量化;体积超标时降帧/降色阶。

## 二、README 视觉升级

1. **头部**:居中 logo(源自 `app/src-tauri/icons/icon.png`,导出副本到 `docs/images/logo.png`)+ 标题 + 一句话 slogan + 徽章组(CI / 最新 Release / 总下载量 / 平台 / MIT)+ 新 GIF 居中。
2. **下载区**:平台表格(平台 | 安装包 | 系统要求)。
3. **特性区**:按主题分组重排——实时看板 / 跳转与恢复 / 通知与提醒 / 吸边与窗口(Windows)/ 菜单栏面板(macOS)/ 外观与自定义 / 账号与用量 / 零配置接入;每组小标题 + 精简要点,次要细节进 `<details>` 折叠。
4. **其余章节**(工作原理 / 快速开始 / 接入 / 数据配置 / 测试 / 路线)结构保留,仅排版微调。

## 验收标准

- `bun scripts/record-demo.mjs` 可一键重新生成 demo.gif,逐帧确定(同输入同输出)。
- demo.gif < 4MB、文字清晰、无现有 GIF 的调色板失真。
- `bun run build` 产物不含 demo 入口;现有测试(`tsc --noEmit` + `vitest run`)不受影响。
- README 在 GitHub 渲染正常(徽章、表格、折叠、居中头部)。

## 不做什么

- 不做英文版 README。
- 不改 App 运行时行为与样式。
- 不做真实桌面录屏(吸边等系统级交互以浏览器内组件演示代替)。
