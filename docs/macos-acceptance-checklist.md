# macOS 真机验收清单

## 安装与启动
- [ ] dmg 双击安装，拖入 Applications；首次打开无 Gatekeeper 拦截（签名公证生效）
- [ ] Dock 不出现 Meowo 图标（LSUIElement）
- [ ] 顶部状态栏出现 Meowo 图标

## 面板交互
- [ ] 左键图标弹出面板，定位在图标正下方
- [ ] 面板内容与 Windows 贴纸卡片一致（项目名/状态/标题/todo 进度/连接状态/tab）
- [ ] 面板无拖拽区/pin/resize 手柄
- [ ] 点面板外部（失焦）面板自动收起；再次左键重新弹出
- [ ] 右键图标弹出「设置 / 退出」菜单；点设置打开设置窗口并获焦；关闭后 Dock 不残留图标
- [ ] 跨 Space / 全屏下面板仍可正常弹出

## 终端跳转 / 恢复
- [ ] 连接中会话（运行在 Terminal.app）点卡片 → 切到对应 tab 并置前
- [ ] 连接中会话（运行在 iTerm2）点卡片 → 切到对应 session/tab 并置前
- [ ] 已断开会话点卡片 → 新开 Terminal.app，cd 到原目录并 claude --resume
- [ ] 运行在非 Terminal/iTerm2（如 Warp）的会话点卡片 → 回退新开 Terminal resume
- [ ] 首次跳转弹「自动化」授权，允许后生效；路径含空格/特殊字符不出错（防注入）

## 通知
- [ ] 会话待交互/出错弹系统通知；同一情形不重复弹（去重）
- [ ] 总开关关闭后不再弹
- [ ] 点击通知切到对应终端
- [ ] 弹通知时不卡顿、CPU 正常（无 100% 飙升）

## 自动更新
- [ ] 发布新版本后，应用内/后台检查到更新并能升级（latest.json 的 darwin-aarch64 / darwin-x86_64 各指向自己架构的 app.tar.gz；Apple 芯片与 Intel 机各验一台）
