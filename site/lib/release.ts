import { cache } from "react";
import { marked } from "marked";
import sanitizeHtml from "sanitize-html";
import { get } from "node:https";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { REPO_SLUG } from "./site";

// 应用版本兜底：GitHub API 不可用（如本地匿名限流）时用于版本徽章。
// 优先环境变量 MEOWO_VERSION；否则构建期直接读 app/src-tauri/Cargo.toml——它是版本唯一真源
// （tauri.conf.json 无 version 字段，Tauri 即以它为准）。此前这里手抄版本号，实际漂移过
// 5 个版本（停在 0.5.5）而无人发觉，不再手抄。
const readAppVersion = (): string => {
  if (process.env.MEOWO_VERSION) return process.env.MEOWO_VERSION;
  try {
    // next build 的 cwd 是 site/（package.json scripts 与 deploy-pages.yml 均如此）。
    const toml = readFileSync(join(process.cwd(), "..", "app", "src-tauri", "Cargo.toml"), "utf8");
    const version = toml.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
    if (version) return version;
  } catch {
    // site 被单独 checkout 之类的场景读不到仓库文件，走最终兜底。
  }
  console.warn("[release] 读不到 Cargo.toml 的版本号，版本徽章将显示 unknown 兜底");
  return "unknown";
};
const APP_VERSION = readAppVersion();

export type Asset = { name: string; url: string; size: number };
export type Release = {
  tag: string;
  version: string;
  windows: Asset | null;
  macos: Asset | null;
};
export type ReleaseNote = {
  tag: string;
  /** release 的标题；只是「Meowo v0.5.0」这种和 tag 重复的自动命名时为 null。 */
  title: string | null;
  date: string;
  /** release 正文渲染出的 HTML；作者没写说明时为 null。 */
  bodyHtml: string | null;
};

// 安装包文件名带版本号，且随 productName 变过（cc-kanban → Meowo），
// 所以按后缀从 release 资产里挑，不去拼名字。
const pick = (assets: Asset[], ext: string) =>
  assets.find((a) => a.name.toLowerCase().endsWith(ext)) ?? null;

// Windows 资产两级匹配：`-installer.exe` 是 WebView2 壳（人类下载入口，现代 UI），
// `-setup.exe` 是 NSIS 芯（updater 的 latest.json 指向它，同时兼作老 release 的回退
// ——壳是后加的，旧版本 release 里没有）。**不能**笼统按 ".exe" 取第一个：release
// 里两个 exe 并存后，命中哪个取决于 GitHub API 返回的资产顺序。
const pickWindows = (assets: Asset[]) =>
  pick(assets, "-installer.exe") ?? pick(assets, "-setup.exe") ?? pick(assets, ".exe");

// 站点是静态导出，下面这些请求都发生在构建时。发新版后要重新部署站点内容才会更新——
// deploy-pages.yml 挂了 release: published 触发，你在 GitHub 上点 Publish 时会自动跑。
async function gh<T>(path: string): Promise<T | null> {
  const headers: Record<string, string> = {
    Accept: "application/vnd.github+json",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "meowo-site-build",
  };
  // CI 里带上 token，避开匿名请求的 60 次/小时限额。
  if (process.env.GITHUB_TOKEN) {
    headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
  }
  // 不使用 Next 增强版 fetch：force-cache 会跨构建保留旧 Release，no-store 又会让
  // output: "export" 把页面判为动态。Node HTTPS 是纯构建期 I/O，每次构建读取最新数据，
  // 最终页面仍是完全静态的；外层 React cache() 负责单次构建内去重。
  return new Promise((resolve) => {
    const req = get(`https://api.github.com/repos/${REPO_SLUG}${path}`, { headers }, (res) => {
      const chunks: Buffer[] = [];
      res.on("data", (chunk: Buffer) => chunks.push(chunk));
      res.on("end", () => {
        if (res.statusCode !== 200) {
          console.warn(`[release] GitHub API ${path} → ${res.statusCode ?? "unknown"}`);
          resolve(null);
          return;
        }
        try {
          resolve(JSON.parse(Buffer.concat(chunks).toString("utf8")) as T);
        } catch (err) {
          console.warn(`[release] GitHub API ${path} 返回了无效 JSON`, err);
          resolve(null);
        }
      });
    });
    req.setTimeout(15_000, () => req.destroy(new Error("GitHub API timeout")));
    req.on("error", (err) => {
      console.warn(`[release] 请求 GitHub API ${path} 失败`, err);
      resolve(null);
    });
  });
}

type ApiRelease = {
  tag_name: string;
  name: string | null;
  body: string | null;
  draft: boolean;
  prerelease: boolean;
  published_at: string | null;
  assets: { name: string; browser_download_url: string; size: number }[];
};

export function renderReleaseMarkdown(body: string): string {
  const raw = marked.parse(body, { async: false }) as string;
  return sanitizeHtml(raw, {
    allowedTags: [...sanitizeHtml.defaults.allowedTags, "img"],
    allowedAttributes: {
      ...sanitizeHtml.defaults.allowedAttributes,
      a: ["href", "name", "target", "rel"],
      img: ["src", "alt", "title"],
    },
    allowedSchemes: ["http", "https"],
    transformTags: {
      a: (_tagName, attribs) => ({
        tagName: "a",
        attribs: { ...attribs, rel: "noopener noreferrer" },
      }),
    },
  });
}

/**
 * 最新 release：把安装包的真实下载地址嵌进页面。
 * API 不可用时用应用自身版本生成徽章，安装包地址回退到 releases/latest 页面。
 */
export const getLatestRelease = cache(async (): Promise<Release | null> => {
  const json = await gh<ApiRelease>("/releases/latest");
  // 本地匿名请求可能遇到 GitHub API 限流。版本徽章仍以应用自身版本为准，
  // 下载按钮则因资产为 null 自动回退到 releases/latest，不复用任何旧版本链接。
  if (!json) {
    return {
      tag: `v${APP_VERSION}`,
      version: APP_VERSION,
      windows: null,
      macos: null,
    };
  }
  const assets: Asset[] = json.assets.map((a) => ({
    name: a.name,
    url: a.browser_download_url,
    size: a.size,
  }));
  return {
    tag: json.tag_name,
    version: json.tag_name.replace(/^v/, ""),
    windows: pickWindows(assets),
    macos: pick(assets, ".dmg"),
  };
});

// tauri-action 不带 releaseBody 时塞的默认正文——不是发布说明，别展示。
const PLACEHOLDER_BODY = /^see the assets to download/i;

/**
 * 全部 release，供更新日志页渲染。正文按 markdown 渲染成 HTML。
 * 草稿要过滤掉：构建时带了 token，API 会把未发布的草稿一起返回。
 */
export const getReleaseNotes = cache(async (): Promise<ReleaseNote[]> => {
  const list = await gh<ApiRelease[]>("/releases?per_page=50");
  if (!list) return [];
  return list
    .filter((r) => !r.draft)
    .map((r) => {
      const body = (r.body ?? "").trim();
      const hasNotes = body.length > 0 && !PLACEHOLDER_BODY.test(body);
      // tauri-action 把 release 命名成「Meowo v0.5.0」，和左边的 tag 重复；只有作者
      // 另起了标题（不含 tag）才值得显示。
      const name = r.name?.trim() ?? "";
      return {
        tag: r.tag_name,
        title: name && !name.includes(r.tag_name) ? name : null,
        date: (r.published_at ?? "").slice(0, 10),
        bodyHtml: hasNotes ? renderReleaseMarkdown(body) : null,
      };
    });
});

export const formatSize = (bytes: number) => `${(bytes / 1024 / 1024).toFixed(1)} MB`;
