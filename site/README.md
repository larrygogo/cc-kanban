# Meowo 官网

Meowo / 喵呜 的产品官网。Next.js 15（App Router，静态导出）+ React 19 + TypeScript，包管理用 Bun。

## 本地开发

```bash
cd site
bun install
bun run dev      # http://localhost:3000
```

## 构建与预览

```bash
bun run build    # 全站静态导出到 out/
bun run start    # 本地静态服务预览产物（bun x serve out）
```

## 部署到 GitHub Pages

工作流：`.github/workflows/deploy-pages.yml`（触发条件：`main` 分支上 `site/**` 变更；发版后由 `site-redeploy.yml` dispatch 重建，以刷新下载直链与更新日志）。

1. 构建 `out/` 后通过 `actions/upload-pages-artifact` + `actions/deploy-pages` 发布，无需第三方服务。
2. 自定义域名 `meowo.io` 由 `public/CNAME` 声明；`public/.nojekyll` 让 Pages 原样提供 `_next/` 下的资源。
3. 构建期环境变量：
   - `NEXT_PUBLIC_SITE_URL`（工作流里固定为 `https://meowo.io`）：OG/Twitter 图与 sitemap 的绝对地址。
   - `GITHUB_TOKEN`：构建时读 GitHub Releases API，避开匿名限额。
   - `MEOWO_VERSION`（可选）：GitHub API 不可用时的版本徽章兜底，默认取 `lib/release.ts` 里的常量；发新版记得同步。

> ⚠️ 下载直链、更新日志、版本徽章都在构建时从 GitHub API 取数。发了新 release 后站点必须重新构建部署才会更新（`site-redeploy.yml` 已自动挂上）。

## 页面

双语站点：中文在根路径，英文在 `/en` 前缀下，共 12 个页面。

| 中文路由 | 英文路由 | 内容 |
|------|------|------|
| `/` | `/en/` | 首页：主张、产品图、特性、工作原理、CTA |
| `/features` | `/en/features/` | 功能详情：深入讲解 + CSS 示意面板 + 平台差异 |
| `/download` | `/en/download/` | 下载：平台卡、安装步骤、环境要求 |
| `/docs` | `/en/docs/` | 文档：工作原理、自动接入、手动接入 Claude Code、数据与配置 |
| `/changelog` | `/en/changelog/` | 更新日志（构建时取自 GitHub Releases） |
| `/faq` | `/en/faq/` | 常见问题（折叠面板） |

## 结构

```
site/
├── app/
│   ├── (zh)/                 # 中文路由组：根 layout 输出 <html lang="zh-CN">
│   │   ├── layout.tsx        #   中文 metadata/OG、Nav/Footer/LangHint
│   │   └── <route>/page.tsx  #   六个中文页面
│   ├── en/                   # 英文路由组：根 layout 输出 <html lang="en">
│   │   ├── layout.tsx        #   英文 metadata/OG
│   │   └── <route>/page.tsx  #   六个英文页面
│   ├── global-not-found.tsx  # 全局 404（experimental.globalNotFound，整文档组件）
│   ├── sitemap.ts · robots.ts
│   └── globals.css           # 设计系统（浅色高级风：白底 + 翡翠绿点缀）
├── assets/                   # 从 app/ 复制的代理 logo（kimi.png、gemini.svg），避免跨目录引用
├── components/
│   ├── Nav.tsx · Footer.tsx · LangHint.tsx   # 全站导航、页脚与语言提示（按路径识别语言）
│   ├── pages/                      # 六个页面的正文组件（HomeContent / FeaturesContent / DownloadContent / DocsContent / ChangelogContent / FaqContent）
│   ├── screenshots/                # CSS 还原的产品示意（StickerWindow / ChatWindowMock / MenuButtonsMock）
│   ├── FeatureGrid.tsx · CtaBand.tsx · DemoFrame.tsx · DownloadButton.tsx
│   ├── AccountCard.tsx · CheckItem.tsx · SupportedAgents.tsx · ThemeShowcase.tsx
│   ├── Reveal.tsx                  # 滚动进场动画
│   └── icons.tsx                   # 内联 SVG 图标
├── lib/
│   ├── site.ts               # 仓库 / Releases 链接常量
│   ├── i18n.ts               # 双语文案与路径助手
│   └── release.ts            # 构建期读取 GitHub Releases（版本/直链/更新日志）
└── public/                   # CNAME、.nojekyll、favicon.ico、logo.png、demo/（首页演示 iframe）
```

改文案：页面正文直接在对应 `app/**/page.tsx` 与 `components/pages/` 里改；导航/页脚等共享字符串集中在 `lib/i18n.ts`；仓库与下载地址集中在 `lib/site.ts`。

> 设计为浅色高级风（白底 + 细腻分割线 + 翡翠绿点缀）；产品截图为深色贴纸，在浅色页面上形成明暗对比。

> 语言路由说明：`(zh)` 与 `en` 两个路由组各自持有根 layout（多根 layout 模式），静态产物里中英文页面的 `<html lang>` 各自正确。不做整页自动跳转（避免闪现中文 HTML、与 hreflang 声明冲突）；英文浏览器访问中文页时由 `LangHint` 显示一条可关闭的切换提示，选择与 Nav 语言开关共用 `meowo-lang` 记忆。
