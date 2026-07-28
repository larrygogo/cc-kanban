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

/** 用于去重/分组的路径 key：斜杠方向归一，Windows 路径再忽略大小写。 */
export function pathKey(p: string): string {
  const norm = normalizePath(p);
  return /^[A-Za-z]:/.test(norm) ? norm.toLowerCase() : norm;
}

/** cwd 末段目录名作展示，完整路径进 title。与贴纸 stk-repo 同款。 */
export function folderName(cwd: string | null): string {
  if (!cwd) return "";
  return cwd.split(/[\\/]/).filter(Boolean).pop() ?? cwd;
}
