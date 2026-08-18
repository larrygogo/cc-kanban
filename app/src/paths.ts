/**
 * 工作目录（cwd）的展示与比对口径。原本各躺在 NewSessionPanel / ChatSidebar 里一份，
 * 侧栏按目录分组后两边必须同口径（同一目录不能因斜杠方向或大小写裂成两组），故收敛到此。
 */

/** 统一路径分隔符：Windows 路径用反斜杠，Unix 路径用正斜杠。
 *  用于消除 URL 参数/前端输入与后端数据库中 cwd 的斜杠方向不一致。 */
export function normalizePath(p: string): string {
  if (!p) return p;
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

/** cwd 末段目录名作展示，完整路径进 title。与贴纸 stk-repo 同款。 */
export function folderName(cwd: string | null): string {
  if (!cwd) return "";
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
