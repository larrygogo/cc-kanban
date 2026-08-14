# winget 收录与自动更新

无签名安装包的免费"官方渠道"：收录进 [winget-pkgs](https://github.com/microsoft/winget-pkgs) 后，
用户 `winget install larrygogo.Meowo` 安装全程无 SmartScreen 弹窗（winget 走哈希校验 +
微软侧自动化审查），同时收录本身也是 SignPath Foundation 复审时认的"公开信号"。

## 链路

- **manifest 指向 NSIS 芯 `Meowo_x.y.z_x64-setup.exe`**，不是 WebView2 壳
  （`-installer.exe`）。winget 恒静默安装，`installerType: nsis` 自动识别 `/S`；
  壳的现代 UI 只服务官网/GitHub 的人类下载。
- 发版自动化：`.github/workflows/winget.yml` 挂在 **release published** 事件上——
  draft release 由 tauri-action 生成，你人工点 Publish 即触发提交，不会把草稿发出去。
  它用微软官方 `wingetcreate update --submit` 向 winget-pkgs 发 PR，微软的自动化
  校验（安装测试、Defender 扫描）通过后由 bot/人工合并。

## 一次性准备（做完之前 workflow 会失败，属预期）

1. **PAT**：GitHub 建 classic PAT，勾 `public_repo`，存进本仓库 secrets，名字
   `WINGET_PAT`。PR 以你的账号从你的 winget-pkgs fork 发出（wingetcreate 自动建 fork）。
2. **首次提交**（`update` 只能更新已收录的包，首个版本必须 `new`）。本地：

   ```powershell
   winget install wingetcreate   # 或 https://aka.ms/wingetcreate/latest
   wingetcreate new https://github.com/larrygogo/meowo/releases/download/vX.Y.Z/Meowo_X.Y.Z_x64-setup.exe
   ```

   交互字段建议值：

   | 字段 | 值 |
   |---|---|
   | PackageIdentifier | `larrygogo.Meowo` |
   | PackageVersion | `X.Y.Z`（与 tag 一致，不带 v） |
   | Publisher | `larrygogo` |
   | PackageName | `Meowo` |
   | License | `MIT` |
   | PackageUrl / PublisherUrl | `https://meowo.io` |
   | ShortDescription | AI 会话贴纸看板：把 Claude Code 等 AI CLI 会话钉在桌面上 |

   结尾选 submit（或带 `--token` 直接交）。PR 合并后，后续版本全自动。

## 注意

- winget 的升级卸载走 NSIS 静默路径（`/S`、卸载器 `/S`），与 updater 的 `/P /UPDATE`
  语义互不相干；`ChatEnabled` 种子在静默路径不写，winget 装出来恒为默认全功能。
- 若某版 workflow 失败（PAT 过期、winget-pkgs 校验红），手动补一次
  `wingetcreate update larrygogo.Meowo --version X.Y.Z --urls <asset url> --submit` 即可，
  不影响已发布的 release 与自动更新。
