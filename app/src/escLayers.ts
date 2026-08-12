/**
 * Esc 层栈：全局记录「当前有多少个浮层想消费 Esc」。
 *
 * 背景：对话窗有窗口级「Esc = 拒绝审批」监听，各浮层（下拉菜单/右键菜单/弹层/灯箱/
 * 补全菜单）曾靠三种互不兼容的约定让路——preventDefault 标记、捕获段 stopPropagation、
 * activeElement 白名单。每加一个浮层都要知道选哪一种，漏了就是「关个弹层顺手拒了
 * agent 审批」（RenameModal 真踩过）。
 *
 * 本栈是统一判据：浮层挂载/打开时 push，卸载/关闭时 pop；窗口级监听在栈非空时一律
 * 让位。各浮层自己的 Esc 处理（关闭 + preventDefault/stopPropagation）**保留**——
 * 它们负责「把层关掉」，栈只负责「别让全局动作趁虚而入」，双保险互不依赖。
 */
const layers = new Set<symbol>();

/** 注册一个 Esc 消费层，返回注销函数（幂等，可安全在 effect cleanup 里调用）。 */
export function pushEscLayer(): () => void {
  const token = Symbol("esc-layer");
  layers.add(token);
  return () => {
    layers.delete(token);
  };
}

/** 当前是否有浮层在消费 Esc（窗口级全局动作应让位）。 */
export function hasEscLayers(): boolean {
  return layers.size > 0;
}
