/**
 * 工作目录（cwd）的展示与比对口径。原本各躺在 NewSessionPanel / ChatSidebar 里一份，
 * 侧栏按目录分组后两边必须同口径（同一目录不能因斜杠方向或大小写裂成两组），故收敛到此。
 */

/** 去掉首尾空白与**成对**的引号。资源管理器的「复制为路径」给的就是 `"C:\repo\app"`，
 *  粘进目录框后后端按字面找不到、报「目录不存在」（7T-4）。只剥成对的：单边引号在
 *  Unix 上是合法文件名字符，剥掉会造出一个不存在的路径。
 *  与 normalizePath 分开导出：调用方常常只想清掉引号，不想连带翻转斜杠方向（发给
 *  后端的 cwd 保持用户原样，斜杠归一只用于比对/分组）。 */
export function unquotePath(p: string): string {
  const trimmed = p.trim();
  if (trimmed.length >= 2
    && ((trimmed.startsWith('"') && trimmed.endsWith('"')) || (trimmed.startsWith("'") && trimmed.endsWith("'")))) {
    return trimmed.slice(1, -1).trim();
  }
  return trimmed;
}

/** 统一路径分隔符：Windows 路径用反斜杠，Unix 路径用正斜杠。
 *  用于消除 URL 参数/前端输入与后端数据库中 cwd 的斜杠方向不一致。 */
export function normalizePath(p: string): string {
  if (!p) return p;
  p = unquotePath(p);
  if (/^[A-Za-z]:/.test(p)) {
    return p.replace(/\//g, "\\");
  }
  return p.replace(/\\/g, "/");
}

/** 用于去重/分组的路径 key：斜杠方向归一，去掉尾部分隔符，Windows 路径再忽略大小写。
 *  尾斜杠必须裁掉——DB 里 cwd 有带尾斜杠的写法混存，`C:\foo\bar` 与 `C:\foo\bar\`
 *  否则得到不同 key，同一目录在分组/去重下拉里裂成两条同名项。裁到只剩盘符（`C:\`→`C:`）
 *  或空（`/`→``）无妨：那是分组键不是真实路径，且项目 cwd 不会是驱动器/文件系统根。 */
export function pathKey(p: string): string {
  const norm = normalizePath(p).replace(/[\\/]+$/, "");
  return /^[A-Za-z]:/.test(norm) ? norm.toLowerCase() : norm;
}

/** cwd 是不是用户主目录本身。判据不依赖后端下发的 home 值：各平台主目录的形状是固定的
 *  （`C:/Users/<名>`、`/home/<名>`、`/Users/<名>`），段数加倒数第二段就足以认出来。
 *  宁漏勿冤——`/home/shared` 这类恰好同形的真实目录会被误认，但代价只是标签写成 `~`
 *  （完整路径仍在 tip 里），比把「仓库」显示成用户名好（7B-6 实拍：显示成 `35122`）。 */
export function isHomeDir(cwd: string): boolean {
  const parts = normalizePath(cwd).split(/[\\/]+/).filter(Boolean);
  if (parts.length === 3) return /^[A-Za-z]:$/.test(parts[0]) && parts[1].toLowerCase() === "users";
  if (parts.length === 2) return parts[0].toLowerCase() === "users" || parts[0] === "home";
  return false;
}

/** cwd 末段目录名作展示，完整路径进 title。与贴纸 stk-repo 同款。
 *  主目录不取末段——那是用户名，当「仓库名」读毫无意义（7B-6）。 */
export function folderName(cwd: string | null): string {
  if (!cwd) return "";
  if (isHomeDir(cwd)) return "~";
  return cwd.split(/[\\/]/).filter(Boolean).pop() ?? cwd;
}

/** 同名目录消歧用的「上一级目录」段（侧栏目录下拉与 Ctrl+K 切换器共用，原先各抄一份）。
 *  上级只为消歧不必全名，超长段（uuid 一类的 agent 沙箱目录）截短——但要保**尾部**：
 *  消歧信息通常在尾部，截头会让共享长前缀的一批目录（feature-auth-login / -logout）
 *  塌成同一个标签，消歧当场失效。 */
export function parentSegment(cwd: string): string {
  const parts = cwd.split(/[\\/]+/).filter(Boolean);
  const parent = parts.length >= 2 ? parts[parts.length - 2] : "";
  return parent.length > 12 ? `…${parent.slice(-8)}` : parent;
}
