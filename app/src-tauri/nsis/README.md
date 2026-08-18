# NSIS 安装器模板（fork 自 tauri-bundler）

## 文件

- `installer.nsi` — **实际使用**的 handlebars 模板（`tauri.conf.json` → `bundle.windows.nsis.template`）。
  基于上游原版，加入 Meowo 的一键安装自绘界面；所有改动用 `; MEOWO-BEGIN xxx` / `; MEOWO-END xxx`
  成对标记包裹，标记外的区域与上游逐字节一致。
- `upstream-installer.nsi` — 上游原版的只读参照副本（`@tauri-apps/cli` v2.11.2，CRLF），
  **永远不要手改**。升级时用它与新上游做 diff。

## 为什么自绘代码内联在模板里而不是单独的 .nsh

bundler 渲染自定义模板时只把**渲染结果**写进 `target/release/nsis/<arch>/`，不会拷贝模板
同目录的其它文件；makensis 的 `!include` 相对路径解析不到 `src-tauri/nsis/`。已知的出路
（经 `installerHooks` 的 `${__FILEDIR__}` 转一手）要求全部代码包成宏体，可读性更差。
故自绘代码全部内联，靠 MEOWO 标记维持与上游 diff 的机械性。

## 上游来源与一致性验证

- 上游文件：tauri-apps/tauri 仓库 `crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi`，
  tag `tauri-cli-v2.11.2`（与 `app/package.json` 的 `@tauri-apps/cli` 版本对应）。
- 模板同样以明文内嵌在 `app/node_modules/@tauri-apps/cli-win32-x64-msvc/cli.win32-x64-msvc.node`
  中（v2.11.2 位于字节偏移 11497248 起、CRLF、32767 字节，起始内容 `Unicode true`）。升级后偏移
  会变：`grep -abo "Unicode true" cli.win32-x64-msvc.node` 重新定位。
- 已验证：GitHub tag 版（LF）与二进制内嵌版（CRLF）在换行符归一化后逐字节一致。vendor 的是
  CRLF 版（bundler 实际渲染用的形态）。

## 升级 @tauri-apps/cli 的同步流程（SOP）

1. 升级 `app/package.json` 的 `@tauri-apps/cli` 并安装。
2. 取新上游模板（两种取法，任选并交叉验证）：
   - GitHub：`https://raw.githubusercontent.com/tauri-apps/tauri/tauri-cli-vX.Y.Z/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi`
   - 二进制：从新 `.node` 文件按上述 grep 定位提取。
3. `diff upstream-installer.nsi <新上游>` 查看上游改了什么。
4. 把上游 diff 应用到 `installer.nsi`（MEOWO 标记块之外的区域应能干净套用；
   若上游改动落在我们替换掉的页面序列区，需人工比对语义——重点核对下方「必须保留的语义」）。
5. 用新上游覆盖 `upstream-installer.nsi`。
6. 跑验证矩阵（见下）。

## 必须保留的语义（升级时逐条核对）

- `.onInit` / `un.onInit` 的命令行解析：`/P`→`$PassiveMode`、`/NS`→`$NoShortcutMode`、
  `/UPDATE`→`$UpdateMode`。tauri-plugin-updater 的传参由 `tauri.conf.json` 的
  `plugins.updater.windows.installMode` 决定：现配 `quiet` → `/S /R /UPDATE /ARGS`
  （0.5.14→0.5.15 实测 passive 的 `/P` 会弹自绘进度窗且界面错乱，故更新链路改全静默；
  `/P` 路径仍保留，仅手动运行 `-setup.exe` 时可达）。
- `NSIS_HOOK_PREINSTALL` 在 `SetOutPath $INSTDIR` 之后、`CheckIfAppIsRunning` 之前；
  `NSIS_HOOK_POSTINSTALL` 在注册表与快捷方式写完之后（`../nsis-hooks.nsh` 依赖这两个位置）。
- 寄存器分工：`$R0-$R3` 归 `utils.nsh` 的 `CheckIfAppIsRunning`；`$R4-$R9` 归 nsis-hooks.nsh；
  重装检测逻辑用 `$R0-$R4`/`$R6` 且 `$R0`（版本比较结果）存活到 Section 阶段。
  MEOWO 自绘代码只用命名 Var（`MeowoXxx` 前缀），不碰 `$R0-$R9`。
- `$UpdateMode=1`：跳过 WebView2 段、不卸旧版、不建/不删快捷方式、卸载不删数据。
- 卸载侧 MUI 页（含「删除应用数据」勾选框）保持上游原样。

## 验证矩阵（改模板后跑）

- `bun tauri build`；产物在 `target/release/bundle/nsis/`。
- 100% / 150% / 200% DPI 下主页面、进度页、完成页截图审查（中英文两套）。
- 全新安装：默认一键装；展开自定义改路径；去勾桌面快捷方式；去勾对话功能。
- 同版本覆盖装、旧版升级装、降级装（降级会先跑旧卸载器）。
- 命令行：`/S`（静默）、`/P /R /UPDATE`（模拟 updater：不弹页面、自动关窗、快捷方式不动、
  hooks 的补建逻辑照常）、`/NS`。
- 卸载器：确认页外观如旧、「删除应用数据」勾选在。
