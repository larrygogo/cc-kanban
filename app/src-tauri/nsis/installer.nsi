Unicode true
ManifestDPIAware true
; Add in `dpiAwareness` `PerMonitorV2` to manifest for Windows 10 1607+ (note this should not affect lower versions since they should be able to ignore this and pick up `dpiAware` `true` set by `ManifestDPIAware true`)
; Currently undocumented on NSIS's website but is in the Docs folder of source tree, see
; https://github.com/kichik/nsis/blob/5fc0b87b819a9eec006df4967d08e522ddd651c9/Docs/src/attributes.but#L286-L300
; https://github.com/tauri-apps/tauri/pull/10106
ManifestDPIAwareness PerMonitorV2

!if "{{compression}}" == "none"
  SetCompress off
!else
  ; Set the compression algorithm. We default to LZMA.
  SetCompressor /SOLID "{{compression}}"
!endif

!include MUI2.nsh
!include FileFunc.nsh
!include x64.nsh
!include WordFunc.nsh
!include "utils.nsh"
!include "FileAssociation.nsh"
!include "Win\COM.nsh"
!include "Win\Propkey.nsh"
!include "StrFunc.nsh"
${StrCase}
${StrLoc}

{{#if installer_hooks}}
!include "{{installer_hooks}}"
{{/if}}

!define WEBVIEW2APPGUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

!define MANUFACTURER "{{manufacturer}}"
!define PRODUCTNAME "{{product_name}}"
!define VERSION "{{version}}"
!define VERSIONWITHBUILD "{{version_with_build}}"
!define HOMEPAGE "{{homepage}}"
!define INSTALLMODE "{{install_mode}}"
!define LICENSE "{{license}}"
!define INSTALLERICON "{{installer_icon}}"
!define SIDEBARIMAGE "{{sidebar_image}}"
!define HEADERIMAGE "{{header_image}}"
!define UNINSTALLERICON "{{uninstaller_icon}}"
!define UNINSTALLERHEADERIMAGE "{{uninstaller_header_image}}"
!define MAINBINARYNAME "{{main_binary_name}}"
!define MAINBINARYSRCPATH "{{main_binary_path}}"
!define BUNDLEID "{{bundle_id}}"
!define COPYRIGHT "{{copyright}}"
!define OUTFILE "{{out_file}}"
!define ARCH "{{arch}}"
!define ADDITIONALPLUGINSPATH "{{additional_plugins_path}}"
!define ALLOWDOWNGRADES "{{allow_downgrades}}"
!define DISPLAYLANGUAGESELECTOR "{{display_language_selector}}"
!define INSTALLWEBVIEW2MODE "{{install_webview2_mode}}"
!define WEBVIEW2INSTALLERARGS "{{webview2_installer_args}}"
!define WEBVIEW2BOOTSTRAPPERPATH "{{webview2_bootstrapper_path}}"
!define WEBVIEW2INSTALLERPATH "{{webview2_installer_path}}"
!define MINIMUMWEBVIEW2VERSION "{{minimum_webview2_version}}"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}"
!define MANUKEY "Software\${MANUFACTURER}"
!define MANUPRODUCTKEY "${MANUKEY}\${PRODUCTNAME}"
!define UNINSTALLERSIGNCOMMAND "{{uninstaller_sign_cmd}}"
!define ESTIMATEDSIZE "{{estimated_size}}"
!define STARTMENUFOLDER "{{start_menu_folder}}"

Var PassiveMode
Var UpdateMode
Var NoShortcutMode
Var WixMode
Var OldMainBinaryName

Name "${PRODUCTNAME}"
BrandingText "${COPYRIGHT}"
OutFile "${OUTFILE}"

; We don't actually use this value as default install path,
; it's just for nsis to append the product name folder in the directory selector
; https://nsis.sourceforge.io/Reference/InstallDir
!define PLACEHOLDER_INSTALL_DIR "placeholder\${PRODUCTNAME}"
InstallDir "${PLACEHOLDER_INSTALL_DIR}"

VIProductVersion "${VERSIONWITHBUILD}"
VIAddVersionKey "ProductName" "${PRODUCTNAME}"
VIAddVersionKey "FileDescription" "${PRODUCTNAME}"
VIAddVersionKey "LegalCopyright" "${COPYRIGHT}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

# additional plugins
!addplugindir "${ADDITIONALPLUGINSPATH}"

; Uninstaller signing command
!if "${UNINSTALLERSIGNCOMMAND}" != ""
  !uninstfinalize '${UNINSTALLERSIGNCOMMAND}'
!endif

; Handle install mode, `perUser`, `perMachine` or `both`
!if "${INSTALLMODE}" == "perMachine"
  RequestExecutionLevel admin
!endif

!if "${INSTALLMODE}" == "currentUser"
  RequestExecutionLevel user
!endif

!if "${INSTALLMODE}" == "both"
  !define MULTIUSER_MUI
  !define MULTIUSER_INSTALLMODE_INSTDIR "${PRODUCTNAME}"
  !define MULTIUSER_INSTALLMODE_COMMANDLINE
  !if "${ARCH}" == "x64"
    !define MULTIUSER_USE_PROGRAMFILES64
  !else if "${ARCH}" == "arm64"
    !define MULTIUSER_USE_PROGRAMFILES64
  !endif
  !define MULTIUSER_INSTALLMODE_DEFAULT_REGISTRY_KEY "${UNINSTKEY}"
  !define MULTIUSER_INSTALLMODE_DEFAULT_REGISTRY_VALUENAME "CurrentUser"
  !define MULTIUSER_INSTALLMODEPAGE_SHOWUSERNAME
  !define MULTIUSER_INSTALLMODE_FUNCTION RestorePreviousInstallLocation
  !define MULTIUSER_EXECUTIONLEVEL Highest
  !include MultiUser.nsh
!endif

; Installer icon
!if "${INSTALLERICON}" != ""
  !define MUI_ICON "${INSTALLERICON}"
!endif

; Installer sidebar image
!if "${SIDEBARIMAGE}" != ""
  !define MUI_WELCOMEFINISHPAGE_BITMAP "${SIDEBARIMAGE}"
!endif

; Enable header images for installer and uninstaller pages when either image is configured.
!if "${HEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE
!else if "${UNINSTALLERHEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE
!endif

; Installer header image
!if "${HEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE_BITMAP "${HEADERIMAGE}"
!endif

; Uninstaller header image
!if "${UNINSTALLERHEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE_UNBITMAP "${UNINSTALLERHEADERIMAGE}"
!endif

; Uninstaller icon
!if "${UNINSTALLERICON}" != ""
  !define MUI_UNICON "${UNINSTALLERICON}"
!endif

; Define registry key to store installer language
!define MUI_LANGDLL_REGISTRY_ROOT "HKCU"
!define MUI_LANGDLL_REGISTRY_KEY "${MANUPRODUCTKEY}"
!define MUI_LANGDLL_REGISTRY_VALUENAME "Installer Language"

; Installer pages, must be ordered as they appear
; MEOWO-BEGIN oneclick-ui
; 一键安装自绘界面：上游的 WELCOME / 重装问询 / DIRECTORY / STARTMENU / FINISH 五页
; 替换为「自绘主页 → INSTFILES(SHOW 回调改造) → 自绘完成页」。卸载侧 MUI 页原样保留。
; 新增状态一律用 Meowo* 命名 Var，不碰 $R0-$R9（$R0-$R3 归 utils.nsh 的
; CheckIfAppIsRunning，$R4-$R9 归 nsis-hooks.nsh；重装检测沿用上游 $R0/$R6/$WixMode
; 约定——$R0 的版本比较结果从页面创建活到 leave，期间任何新代码不得写 $R0/$R6）。
;
; 色值与 icons/nsis-oneclick-bg.bmp 的分区底色一一对应（scripts/generate-nsis-bitmaps.py
; 有同名注释）：原生控件文字背景只能靠 SetCtlColors 配纯色，控件落在哪个分区就配哪个色。
!define MEOWO_COL_BASE "17171a"
!define MEOWO_COL_BAND "101012"
!define MEOWO_COL_TEXT "f5f5f7"
!define MEOWO_COL_SUB  "a0a0a8"
!define MEOWO_COL_LINK "4ec9a5"
!define /ifndef SS_CENTER 0x1
!define /ifndef SS_NOTIFY 0x100
!define /ifndef SW_HIDE 0
!define /ifndef SW_SHOW 5
; 背景位图与 headerImage 同目录：从 HEADERIMAGE（bundler 渲染时已绝对化）推导，
; 不依赖 makensis 的 CWD。重命名这些文件时必须同步这里。
; 两个变体：-btn 带烙好的胶囊按钮（主页/完成页），无后缀的给进度页（没有按钮）。
!searchreplace MEOWO_BG_PATH "${HEADERIMAGE}" "nsis-header.bmp" "nsis-oneclick-bg.bmp"
!searchreplace MEOWO_BGBTN_PATH "${HEADERIMAGE}" "nsis-header.bmp" "nsis-oneclick-bg-btn.bmp"
ReserveFile "${MEOWO_BG_PATH}"
ReserveFile "${MEOWO_BGBTN_PATH}"

Var MeowoDpi
Var MeowoW            ; 外窗 client 宽/高（px，MeowoGuiInit 填）
Var MeowoH
Var MeowoTitleFont
Var MeowoBtnFont
Var MeowoSmallFont
Var MeowoLinkFont
Var MeowoHasExisting  ; 0/1：检测到已有安装（MeowoDetectExisting 填）
Var MeowoChatEnabled  ; 1/0：「对话窗口功能」勾选，Section 里写注册表种子
Var MeowoDesktopLnk   ; 1/0：「创建桌面快捷方式」勾选
Var MeowoExpanded     ; 0/1：自定义安装区是否展开
Var MeowoBgHandle     ; 主页/完成页背景的 HBITMAP（NSD_SetStretchedImage 管理）
Var MeowoDirLabel
Var MeowoDirText
Var MeowoBrowseBtn
Var MeowoChatCheck
Var MeowoChatLabel
Var MeowoChatHint
Var MeowoDesktopCheck
Var MeowoDesktopLabel
Var MeowoCustomLink
Var MeowoNoteLabel

; 逻辑像素 → 物理像素（96dpi 基准，MeowoGuiInit 之后可用）。
!macro MeowoScale out in
  IntOp ${out} ${in} * $MeowoDpi
  IntOp ${out} ${out} / 96
!macroend
!define MeowoScale "!insertmacro MeowoScale"

; 一次性改造向导外框：resize + 居中、隐藏向导 chrome、内页占位控件(1018)撑满。
; 之后所有页面（含 MUI 的 INSTFILES）都按全屏 client 区排版。passive 也走这里
; （updater 弹出的 /P 进度窗与 GUI 同一视觉）；silent 无窗口直接返回。
!define MUI_CUSTOMFUNCTION_GUIINIT MeowoGuiInit
Function MeowoGuiInit
  ${If} ${Silent}
    Return
  ${EndIf}
  ; 输出走寄存器再转存：System::Call 对自定义 Var 的输出说明符支持不可靠。
  System::Call "user32::GetDpiForWindow(p $HWNDPARENT) i .r0"
  StrCpy $MeowoDpi $0
  ${If} $MeowoDpi < 96
    StrCpy $MeowoDpi 96
  ${EndIf}

  ; 外窗改为 780x512（含标题栏），以**原窗口中心**为锚重定位：NSIS 已把窗口按启动
  ; 显示器居中，拿主屏 GetSystemMetrics 重算会在多屏环境跑偏（实测跑到角落）。
  ; SWP_NOZORDER|SWP_NOACTIVATE = 0x14。
  ${MeowoScale} $0 780
  ${MeowoScale} $1 512
  System::Call "*(i 0, i 0, i 0, i 0) p .r4"
  System::Call "user32::GetWindowRect(p $HWNDPARENT, p r4)"
  System::Call "*$4(i .r2, i .r3, i .r5, i .r6)"
  System::Free $4
  IntOp $2 $2 + $5
  IntOp $2 $2 / 2
  IntOp $3 $3 + $6
  IntOp $3 $3 / 2
  IntOp $5 $0 / 2
  IntOp $2 $2 - $5
  IntOp $5 $1 / 2
  IntOp $3 $3 - $5
  System::Call "user32::SetWindowPos(p $HWNDPARENT, p 0, i r2, i r3, i r0, i r1, i 0x14)"

  ; 隐藏向导 chrome。「下一步」(ID 1) 只是视觉隐藏——自绘按钮靠
  ; SendMessage WM_COMMAND 1 驱动它走标准页流（leave 校验照常触发）。
  ; 1028/1256=branding，1034-1038=header 区，1045=按钮上方分隔线。
  GetDlgItem $0 $HWNDPARENT 1
  ShowWindow $0 ${SW_HIDE}
  GetDlgItem $0 $HWNDPARENT 2
  ShowWindow $0 ${SW_HIDE}
  GetDlgItem $0 $HWNDPARENT 3
  ShowWindow $0 ${SW_HIDE}
  GetDlgItem $0 $HWNDPARENT 1028
  ShowWindow $0 ${SW_HIDE}
  GetDlgItem $0 $HWNDPARENT 1034
  ShowWindow $0 ${SW_HIDE}
  GetDlgItem $0 $HWNDPARENT 1035
  ShowWindow $0 ${SW_HIDE}
  GetDlgItem $0 $HWNDPARENT 1036
  ShowWindow $0 ${SW_HIDE}
  GetDlgItem $0 $HWNDPARENT 1037
  ShowWindow $0 ${SW_HIDE}
  GetDlgItem $0 $HWNDPARENT 1038
  ShowWindow $0 ${SW_HIDE}
  GetDlgItem $0 $HWNDPARENT 1045
  ShowWindow $0 ${SW_HIDE}
  GetDlgItem $0 $HWNDPARENT 1256
  ShowWindow $0 ${SW_HIDE}

  ; 内页占位控件(1018)撑满 client 区，并记下 client 尺寸供各页排版。
  System::Call "*(i 0, i 0, i 0, i 0) p .r4"
  System::Call "user32::GetClientRect(p $HWNDPARENT, p r4)"
  System::Call "*$4(i, i, i .r5, i .r6)"
  System::Free $4
  StrCpy $MeowoW $5
  StrCpy $MeowoH $6
  GetDlgItem $0 $HWNDPARENT 1018
  System::Call "user32::MoveWindow(p r0, i 0, i 0, i r5, i r6, i 1)"

  ; 标题栏切暗色，与深色内容匹配（DWMWA_USE_IMMERSIVE_DARK_MODE=20；Win10 老构建
  ; 不认该属性，静默失败退回亮色）。
  System::Call "dwmapi::DwmSetWindowAttribute(p $HWNDPARENT, i 20, *i 1, i 4)"
  ; 隐藏 chrome + 扩大窗口后强制整窗重绘：被隐藏控件与新暴露区域不会自动擦除，
  ; 不刷会在右/下边缘留未初始化的花屏残影（实拍）。
  System::Call "user32::InvalidateRect(p $HWNDPARENT, p 0, i 1)"

  ; 字体：标题 26 / 按钮 15 / 小字 12（逻辑 px 随 DPI 缩放；DEFAULT_CHARSET=1，
  ; CLEARTYPE_QUALITY=5）。显式用微软雅黑 UI——$(^Font) 是 MS Shell Dlg，
  ; 中文会映射到宋体，衬线在暗色大字下惨不忍睹（实拍）；雅黑渲染拉丁字形同样体面。
  ${MeowoScale} $0 26
  IntOp $0 0 - $0
  System::Call "gdi32::CreateFont(i r0, i 0, i 0, i 0, i 600, i 0, i 0, i 0, i 1, i 0, i 0, i 5, i 0, t 'Microsoft YaHei UI') p .s"
  Pop $MeowoTitleFont
  ${MeowoScale} $0 15
  IntOp $0 0 - $0
  System::Call "gdi32::CreateFont(i r0, i 0, i 0, i 0, i 500, i 0, i 0, i 0, i 1, i 0, i 0, i 5, i 0, t 'Microsoft YaHei UI') p .s"
  Pop $MeowoBtnFont
  ${MeowoScale} $0 12
  IntOp $0 0 - $0
  System::Call "gdi32::CreateFont(i r0, i 0, i 0, i 0, i 400, i 0, i 0, i 0, i 1, i 0, i 0, i 5, i 0, t 'Microsoft YaHei UI') p .s"
  Pop $MeowoSmallFont
  ; 链接字体 = 小字加下划线。链接控件不能用 NSD_CreateLink：owner-draw 按钮在对话框
  ; 首次获焦时会误发一次点击（实拍：启动即自动展开自定义区），改用 SS_NOTIFY STATIC，
  ; 可点性由下划线 + 强调色表达。
  System::Call "gdi32::CreateFont(i r0, i 0, i 0, i 0, i 400, i 0, i 1, i 0, i 1, i 0, i 0, i 5, i 0, t 'Microsoft YaHei UI') p .s"
  Pop $MeowoLinkFont
FunctionEnd

; 背景图释出到 $PLUGINSDIR（主页与 INSTFILES 都要用；passive 跳过主页，故两处都调）。
Function MeowoEnsureBg
  InitPluginsDir
  ${IfNot} ${FileExists} "$PLUGINSDIR\meowo-bg.bmp"
    File "/oname=$PLUGINSDIR\meowo-bg.bmp" "${MEOWO_BG_PATH}"
    File "/oname=$PLUGINSDIR\meowo-bg-btn.bmp" "${MEOWO_BGBTN_PATH}"
  ${EndIf}
FunctionEnd

; ===== 旧版检测（上游 PageReinstall 的纯逻辑段，UI 剥离）=====
; 输出：$MeowoHasExisting(0/1)、$WixMode、$R6(WiX 卸载键)、$R0(0=同版/1=升级/-1=降级)。
; 寄存器用法与上游逐字一致；$R0 必须活到 MeowoApplyReinstall。
Function MeowoDetectExisting
  StrCpy $MeowoHasExisting 0
  ; Uninstall previous WiX installation if exists.
  ;
  ; A WiX installer stores the installation info in registry
  ; using a UUID and so we have to loop through all keys under
  ; `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall`
  ; and check if `DisplayName` and `Publisher` keys match ${PRODUCTNAME} and ${MANUFACTURER}
  StrCpy $0 0
  wix_loop:
    EnumRegKey $1 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall" $0
    StrCmp $1 "" wix_loop_done ; Exit loop if there is no more keys to loop on
    IntOp $0 $0 + 1
    ReadRegStr $R0 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1" "DisplayName"
    ReadRegStr $R1 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1" "Publisher"
    StrCmp "$R0$R1" "${PRODUCTNAME}${MANUFACTURER}" 0 wix_loop
    ReadRegStr $R0 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1" "UninstallString"
    ${StrCase} $R1 $R0 "L"
    ${StrLoc} $R0 $R1 "msiexec" ">"
    StrCmp $R0 0 0 wix_loop_done
    StrCpy $WixMode 1
    StrCpy $R6 "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1"
    Goto compare_version
  wix_loop_done:

  ; Check if there is an existing installation
  ReadRegStr $R0 SHCTX "${UNINSTKEY}" ""
  ReadRegStr $R1 SHCTX "${UNINSTKEY}" "UninstallString"
  ${IfThen} "$R0$R1" == "" ${|} Return ${|}

  compare_version:
  ${If} $WixMode = 1
    ReadRegStr $R0 HKLM "$R6" "DisplayVersion"
  ${Else}
    ReadRegStr $R0 SHCTX "${UNINSTKEY}" "DisplayVersion"
  ${EndIf}
  nsis_tauri_utils::SemverCompare "${VERSION}" $R0
  Pop $R0
  ${If} $R0 = 0
  ${OrIf} $R0 = 1
  ${OrIf} $R0 = -1
    StrCpy $MeowoHasExisting 1
  ${EndIf}
  ; 比较结果异常（如旧版 DisplayVersion 缺失）按无旧装处理——与上游 Abort 跳过
  ; 问询页、直接覆盖装的最终效果一致。
FunctionEnd

; ===== 旧版处置（上游 PageLeaveReinstall，问询单选换成固定策略）=====
; 同版/升级 → 直接覆盖装：与 updater 的 /UPDATE 路径同构，且 PREINSTALL hook 的
; 杀进程 + rename-aside 已为覆盖写加固，文件集固定无残留风险。
; 降级 → 先跑旧卸载器：新版本文件残留给旧版本才真正危险（上游降级路径的默认选项）。
; WiX 旧装 → 恒先卸载（历史迁移语义，上游一致）。出错 MessageBox 后 Abort 回主页面。
Function MeowoApplyReinstall
  ${If} $MeowoHasExisting = 0
    Return
  ${EndIf}
  ${If} $WixMode = 1
    Goto reinst_uninstall
  ${EndIf}
  ; In update mode, always proceeds without uninstalling
  ${If} $UpdateMode = 1
    Goto reinst_done
  ${EndIf}
  ${If} $R0 = 0
  ${OrIf} $R0 = 1
    Goto reinst_done
  ${EndIf}

  reinst_uninstall:
    HideWindow
    ClearErrors

    ${If} $WixMode = 1
      ReadRegStr $R1 HKLM "$R6" "UninstallString"
      ExecWait '$R1' $0
    ${Else}
      ReadRegStr $4 SHCTX "${MANUPRODUCTKEY}" ""
      ReadRegStr $R1 SHCTX "${UNINSTKEY}" "UninstallString"
      ${IfThen} $UpdateMode = 1 ${|} StrCpy $R1 "$R1 /UPDATE" ${|} ; append /UPDATE
      ${IfThen} $PassiveMode = 1 ${|} StrCpy $R1 "$R1 /P" ${|} ; append /P
      StrCpy $R1 "$R1 _?=$4" ; append uninstall directory
      ExecWait '$R1' $0
    ${EndIf}

    BringToFront

    ${IfThen} ${Errors} ${|} StrCpy $0 2 ${|} ; ExecWait failed, set fake exit code

    ${If} $0 <> 0
    ${OrIf} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
      ; User cancelled wix uninstaller? return to the main page
      ${If} $WixMode = 1
      ${AndIf} $0 = 1602
        Abort
      ${EndIf}

      ; User cancelled NSIS uninstaller? return to the main page
      ${If} $0 = 1
        Abort
      ${EndIf}

      ; Other errors? show generic error message and return to the main page
      MessageBox MB_ICONEXCLAMATION "$(unableToUninstall)"
      Abort
    ${EndIf}
  reinst_done:
FunctionEnd

; ===== 自绘主页：品牌区 + 大按钮 + 底带「自定义安装」展开区 =====
Page custom MeowoMainPage MeowoMainLeave

Function MeowoMainPage
  Call MeowoDetectExisting
  ; passive（updater 静默升级）：复刻上游行为——不建页面，直接跑重装决策后放行；
  ; 不创建对话框即自动跳过本页（上游 PageReinstall 的同款路径）。
  ${If} $PassiveMode = 1
    Call MeowoApplyReinstall
    Return
  ${EndIf}

  Call MeowoEnsureBg
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  SetCtlColors $0 "${MEOWO_COL_TEXT}" "${MEOWO_COL_BASE}"

  ; 注意：背景位图在本函数**最末**创建（nsDialogs::Show 之前）。nsDialogs 的 z 序
  ; 即创建序——先建在顶、后建垫底（Z 走查实锤；SetWindowPos(HWND_BOTTOM) 在
  ; nsDialogs 页上会被吞）。位图先建就会盖住并吞掉所有控件的点击（实拍翻车过）。

  ; tagline：y=40%，宽 60% 居中（品牌图标与字标已烙在背景图里）。
  IntOp $2 $MeowoW * 20
  IntOp $2 $2 / 100
  IntOp $3 $MeowoH * 40
  IntOp $3 $3 / 100
  IntOp $4 $MeowoW * 60
  IntOp $4 $4 / 100
  ${MeowoScale} $5 20
  nsDialogs::CreateControl STATIC "${__NSD_Label_STYLE}|${SS_CENTER}" "${__NSD_Label_EXSTYLE}" $2 $3 $4 $5 "$(meowoTagline)"
  Pop $6
  SetCtlColors $6 "${MEOWO_COL_SUB}" "${MEOWO_COL_BASE}"
  SendMessage $6 ${WM_SETFONT} $MeowoBtnFont 1

  ; 旧版提示：y=47%（透明度换信任——一键化省掉了问询页，但要说清会发生什么）。
  IntOp $3 $MeowoH * 47
  IntOp $3 $3 / 100
  ${MeowoScale} $5 18
  nsDialogs::CreateControl STATIC "${__NSD_Label_STYLE}|${SS_CENTER}" "${__NSD_Label_EXSTYLE}" $2 $3 $4 $5 ""
  Pop $MeowoNoteLabel
  SetCtlColors $MeowoNoteLabel "${MEOWO_COL_SUB}" "${MEOWO_COL_BASE}"
  SendMessage $MeowoNoteLabel ${WM_SETFONT} $MeowoSmallFont 1
  ${If} $MeowoHasExisting = 1
    ${If} $R0 = 1
      ${NSD_SetText} $MeowoNoteLabel "$(meowoNoteUpgrade)"
    ${ElseIf} $R0 = -1
      ${NSD_SetText} $MeowoNoteLabel "$(meowoNoteDowngrade)"
    ${Else}
      ${NSD_SetText} $MeowoNoteLabel "$(meowoNoteSame)"
    ${EndIf}
  ${EndIf}

  ; 立即安装：320x52 胶囊烙在 -btn 背景图里（原生按钮的白底灰边在暗色下扎眼，
  ; owner-draw 在裸 NSIS 不可行）。控件只是胶囊中带一条同色可点文字（SS_NOTIFY），
  ; 圆角由位图承担；Enter 仍走被隐藏的默认「下一步」按钮，键盘可达性不丢。
  ${MeowoScale} $4 300
  IntOp $2 $MeowoW - $4
  IntOp $2 $2 / 2
  IntOp $3 $MeowoH * 56
  IntOp $3 $3 / 100
  ${MeowoScale} $5 15
  IntOp $3 $3 + $5
  ${MeowoScale} $5 22
  nsDialogs::CreateControl STATIC "${__NSD_Label_STYLE}|${SS_CENTER}|${SS_NOTIFY}" "${__NSD_Label_EXSTYLE}" $2 $3 $4 $5 "$(meowoOneClick)"
  Pop $7
  SetCtlColors $7 "0c211b" "${MEOWO_COL_LINK}"
  SendMessage $7 ${WM_SETFONT} $MeowoBtnFont 1
  ${NSD_OnClick} $7 MeowoOnInstallClick

  ; ---- 底带（背景图 75% 以下的深色区）----
  IntOp $8 $MeowoH * 75
  IntOp $8 $8 / 100

  ; 「自定义安装 ▾」链接：底带右下角。
  ${MeowoScale} $4 150
  ${MeowoScale} $5 24
  IntOp $2 $MeowoW - $4
  IntOp $2 $2 - $5
  ${MeowoScale} $6 18
  ${MeowoScale} $5 16
  IntOp $3 $MeowoH - $6
  IntOp $3 $3 - $5
  nsDialogs::CreateControl STATIC "${__NSD_Label_STYLE}|${SS_NOTIFY}" "${__NSD_Label_EXSTYLE}" $2 $3 $4 $6 "$(meowoCustomOpen)"
  Pop $MeowoCustomLink
  SetCtlColors $MeowoCustomLink "${MEOWO_COL_LINK}" "${MEOWO_COL_BAND}"
  SendMessage $MeowoCustomLink ${WM_SETFONT} $MeowoLinkFont 1
  ${NSD_OnClick} $MeowoCustomLink MeowoOnToggleCustom

  ; ---- 自定义安装区（默认隐藏；隐藏控件的勾选值照常生效 = 产品默认）----
  ; 安装位置行：y = 底带 + 14。
  ${MeowoScale} $5 14
  IntOp $3 $8 + $5
  ${MeowoScale} $2 24
  ${MeowoScale} $4 84
  ${MeowoScale} $6 18
  ${NSD_CreateLabel} $2 $3 $4 $6 "$(meowoInstallPath)"
  Pop $MeowoDirLabel
  SetCtlColors $MeowoDirLabel "${MEOWO_COL_SUB}" "${MEOWO_COL_BAND}"
  SendMessage $MeowoDirLabel ${WM_SETFONT} $MeowoSmallFont 1

  ${MeowoScale} $2 116
  ${MeowoScale} $4 220     ; 右侧留白：24 边距 + 88 浏览钮 + 8 间距 + 100 链接列
  IntOp $4 $MeowoW - $4
  IntOp $4 $4 - $2
  ${MeowoScale} $6 22
  ${MeowoScale} $5 12
  IntOp $3 $8 + $5
  ${NSD_CreateText} $2 $3 $4 $6 "$INSTDIR"
  Pop $MeowoDirText
  SendMessage $MeowoDirText ${WM_SETFONT} $MeowoSmallFont 1

  IntOp $2 $2 + $4
  ${MeowoScale} $5 8
  IntOp $2 $2 + $5
  ${MeowoScale} $4 88
  ${NSD_CreateButton} $2 $3 $4 $6 "$(meowoBrowse)"
  Pop $MeowoBrowseBtn
  SendMessage $MeowoBrowseBtn ${WM_SETFONT} $MeowoSmallFont 1
  ${NSD_OnClick} $MeowoBrowseBtn MeowoOnBrowse

  ; 「对话窗口功能」勾选（默认勾；取消 = 轻量模式，应用只保留贴纸生态）。
  ; themed 复选框（Button 类）的文字颜色不吃 SetCtlColors（Win32 已知行为，实拍
  ; 黑字糊在暗带上）——框与文字拆开：框只留方块，文字用可点 Label 代理切换勾选。
  ${MeowoScale} $2 24
  ${MeowoScale} $5 48
  IntOp $3 $8 + $5
  ${MeowoScale} $4 16
  ${MeowoScale} $6 18
  ${NSD_CreateCheckbox} $2 $3 $4 $6 ""
  Pop $MeowoChatCheck
  ${NSD_Check} $MeowoChatCheck
  ${MeowoScale} $2 46
  ${MeowoScale} $4 290     ; 右侧给链接列留位，不与之重叠
  IntOp $4 $MeowoW - $4
  nsDialogs::CreateControl STATIC "${__NSD_Label_STYLE}|${SS_NOTIFY}" "${__NSD_Label_EXSTYLE}" $2 $3 $4 $6 "$(meowoChatFeature)"
  Pop $MeowoChatLabel
  SetCtlColors $MeowoChatLabel "${MEOWO_COL_TEXT}" "${MEOWO_COL_BAND}"
  SendMessage $MeowoChatLabel ${WM_SETFONT} $MeowoSmallFont 1
  ${NSD_OnClick} $MeowoChatLabel MeowoOnChatLabel

  ${MeowoScale} $2 46
  ${MeowoScale} $5 70
  IntOp $3 $8 + $5
  ${NSD_CreateLabel} $2 $3 $4 $6 "$(meowoChatHint)"
  Pop $MeowoChatHint
  SetCtlColors $MeowoChatHint "${MEOWO_COL_SUB}" "${MEOWO_COL_BAND}"
  SendMessage $MeowoChatHint ${WM_SETFONT} $MeowoSmallFont 1

  ; 「创建桌面快捷方式」勾选（语义 = 上游 FINISH 页那颗，建档时机挪进 Section）。
  ${MeowoScale} $2 24
  ${MeowoScale} $5 94
  IntOp $3 $8 + $5
  ${MeowoScale} $4 16
  ${NSD_CreateCheckbox} $2 $3 $4 $6 ""
  Pop $MeowoDesktopCheck
  ${NSD_Check} $MeowoDesktopCheck
  ${MeowoScale} $2 46
  ${MeowoScale} $4 290
  IntOp $4 $MeowoW - $4
  nsDialogs::CreateControl STATIC "${__NSD_Label_STYLE}|${SS_NOTIFY}" "${__NSD_Label_EXSTYLE}" $2 $3 $4 $6 "$(meowoDesktopLnk)"
  Pop $MeowoDesktopLabel
  SetCtlColors $MeowoDesktopLabel "${MEOWO_COL_TEXT}" "${MEOWO_COL_BAND}"
  SendMessage $MeowoDesktopLabel ${WM_SETFONT} $MeowoSmallFont 1
  ${NSD_OnClick} $MeowoDesktopLabel MeowoOnDesktopLabel

  StrCpy $MeowoExpanded 0
  ShowWindow $MeowoDirLabel ${SW_HIDE}
  ShowWindow $MeowoDirText ${SW_HIDE}
  ShowWindow $MeowoBrowseBtn ${SW_HIDE}
  ShowWindow $MeowoChatCheck ${SW_HIDE}
  ShowWindow $MeowoChatLabel ${SW_HIDE}
  ShowWindow $MeowoChatHint ${SW_HIDE}
  ShowWindow $MeowoDesktopCheck ${SW_HIDE}
  ShowWindow $MeowoDesktopLabel ${SW_HIDE}

  ; 背景位图最后创建 = z 序垫底（理由见函数开头注释）。单张 2x 图缩小采样。
  ${NSD_CreateBitmap} 0 0 100% 100% ""
  Pop $1
  ${NSD_SetStretchedImage} $1 "$PLUGINSDIR\meowo-bg-btn.bmp" $MeowoBgHandle

  nsDialogs::Show
FunctionEnd

Function MeowoOnInstallClick
  Pop $0
  ; 驱动被隐藏的「下一步」(ID 1)：走标准页流，MeowoMainLeave 校验照常触发。
  SendMessage $HWNDPARENT ${WM_COMMAND} 1 0
FunctionEnd

Function MeowoOnToggleCustom
  Pop $0
  ${If} $MeowoExpanded = 0
    StrCpy $MeowoExpanded 1
    StrCpy $1 ${SW_SHOW}
    ${NSD_SetText} $MeowoCustomLink "$(meowoCustomClose)"
  ${Else}
    StrCpy $MeowoExpanded 0
    StrCpy $1 ${SW_HIDE}
    ${NSD_SetText} $MeowoCustomLink "$(meowoCustomOpen)"
  ${EndIf}
  ShowWindow $MeowoDirLabel $1
  ShowWindow $MeowoDirText $1
  ShowWindow $MeowoBrowseBtn $1
  ShowWindow $MeowoChatCheck $1
  ShowWindow $MeowoChatLabel $1
  ShowWindow $MeowoChatHint $1
  ShowWindow $MeowoDesktopCheck $1
  ShowWindow $MeowoDesktopLabel $1
FunctionEnd

; 勾选项文字 Label 的点击代理（框与文字为何拆开见主页创建处注释）。
Function MeowoOnChatLabel
  Pop $0
  ${NSD_GetState} $MeowoChatCheck $1
  ${If} $1 = ${BST_CHECKED}
    ${NSD_Uncheck} $MeowoChatCheck
  ${Else}
    ${NSD_Check} $MeowoChatCheck
  ${EndIf}
FunctionEnd

Function MeowoOnDesktopLabel
  Pop $0
  ${NSD_GetState} $MeowoDesktopCheck $1
  ${If} $1 = ${BST_CHECKED}
    ${NSD_Uncheck} $MeowoDesktopCheck
  ${Else}
    ${NSD_Check} $MeowoDesktopCheck
  ${EndIf}
FunctionEnd

Function MeowoOnBrowse
  Pop $0
  ${NSD_GetText} $MeowoDirText $1
  nsDialogs::SelectFolderDialog "$(meowoInstallPath)" $1
  Pop $1
  ${If} $1 != error
    ; 末段不是产品名则追加（对齐上游 DIRECTORY 页的目录语义）。
    StrLen $2 "${PRODUCTNAME}"
    StrCpy $3 $1 "" -$2
    ${If} $3 != "${PRODUCTNAME}"
      StrCpy $1 "$1\${PRODUCTNAME}"
    ${EndIf}
    ${NSD_SetText} $MeowoDirText $1
  ${EndIf}
FunctionEnd

Function MeowoMainLeave
  ${NSD_GetState} $MeowoChatCheck $MeowoChatEnabled
  ${NSD_GetState} $MeowoDesktopCheck $MeowoDesktopLnk
  ${NSD_GetText} $MeowoDirText $0
  ${If} $0 == ""
    MessageBox MB_ICONEXCLAMATION "$(meowoBadPath)"
    Abort
  ${EndIf}
  StrCpy $INSTDIR $0
  Call MeowoApplyReinstall
FunctionEnd
; MEOWO-END oneclick-ui

; 2. License Page (if defined)
!if "${LICENSE}" != ""
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
  !insertmacro MUI_PAGE_LICENSE "${LICENSE}"
!endif

; 3. Install mode (if it is set to `both`)
!if "${INSTALLMODE}" == "both"
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
  !insertmacro MULTIUSER_PAGE_INSTALLMODE
!endif

; MEOWO-BEGIN reinstall-removed
; 4. 上游的重装问询页（PageReinstall / PageLeaveReinstall）已迁移为纯逻辑函数
;    MeowoDetectExisting / MeowoApplyReinstall（见上方 oneclick-ui 块），
;    问询单选换成固定策略，理由见彼处注释。
; MEOWO-END reinstall-removed

; MEOWO-BEGIN pages-directory-startmenu
; 5. DIRECTORY 页并入自绘主页的「自定义安装」展开区（路径输入 + 浏览）。
; 6. STARTMENU 页移除：本项目 STARTMENUFOLDER 恒空，该页在上游也恒被 Skip；
;    $AppStartMenuFolder 在 .onInit 里按配置回填，Section 直接建快捷方式。
Var AppStartMenuFolder
; MEOWO-END pages-directory-startmenu

; 7. Installation page
; MEOWO-BEGIN instfiles-restyle
!define MUI_PAGE_CUSTOMFUNCTION_SHOW MeowoInstFilesShow
!insertmacro MUI_PAGE_INSTFILES

; INSTFILES 只能由 Section 驱动（NSIS 没有「custom 页后台跑 Section」的机制），
; 故不另起炉灶，就地改造：隐藏细节列表，进度条与状态行居中，背景图压 z 序最底。
; 状态行(1006)滚动显示 DetailPrint（含 WebView2 下载文案），白捡一条动态进度文案。
Function MeowoInstFilesShow
  Call MeowoEnsureBg
  FindWindow $1 "#32770" "" $HWNDPARENT
  GetDlgItem $0 $1 1016    ; 细节列表
  ShowWindow $0 ${SW_HIDE}
  GetDlgItem $0 $1 1027    ; 「显示细节」按钮
  ShowWindow $0 ${SW_HIDE}

  ; 状态行：y=52%，宽 60% 居中，亮字配主区底色。
  IntOp $2 $MeowoW * 20
  IntOp $2 $2 / 100
  IntOp $3 $MeowoH * 52
  IntOp $3 $3 / 100
  IntOp $4 $MeowoW * 60
  IntOp $4 $4 / 100
  ${MeowoScale} $5 20
  GetDlgItem $0 $1 1006
  System::Call "user32::MoveWindow(p r0, i r2, i r3, i r4, i r5, i 1)"
  SetCtlColors $0 "${MEOWO_COL_SUB}" "${MEOWO_COL_BASE}"
  SendMessage $0 ${WM_SETFONT} $MeowoSmallFont 1

  ; 进度条：y=58%，与状态行同宽居中。
  IntOp $3 $MeowoH * 58
  IntOp $3 $3 / 100
  ${MeowoScale} $5 12
  GetDlgItem $0 $1 1004
  System::Call "user32::MoveWindow(p r0, i r2, i r3, i r4, i r5, i 1)"

  ; 背景图：SS_BITMAP static（WS_CHILD|WS_VISIBLE|SS_BITMAP = 0x5000000E），
  ; LR_LOADFROMFILE=0x10 按 client 尺寸缩放加载，SetWindowPos 压到 HWND_BOTTOM。
  System::Call "user32::LoadImage(p 0, t '$PLUGINSDIR\meowo-bg.bmp', i 0, i $MeowoW, i $MeowoH, i 0x10) p .r6"
  System::Call "user32::CreateWindowEx(i 0, t 'STATIC', t '', i 0x5000000E, i 0, i 0, i $MeowoW, i $MeowoH, p $1, p 0, p 0, p 0) p .r7"
  SendMessage $7 0x172 0 $6 ; STM_SETIMAGE(IMAGE_BITMAP)
  System::Call "user32::SetWindowPos(p r7, p 1, i 0, i 0, i 0, i 0, i 0x13)"
FunctionEnd
; MEOWO-END instfiles-restyle

; 8. Finish page
; MEOWO-BEGIN done-page
; 上游 FINISH 页换成同视觉体系的自绘完成页：桌面快捷方式勾选已挪进主页
; （建档时机随之挪进 Section），这里零决策——一颗「立即体验」+ 一个「关闭」链接。
Page custom MeowoDonePage

Function RunMainBinary
  nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" ""
FunctionEnd

Function MeowoDonePage
  ; passive 跳过（等价上游 FINISH 的 SkipIfPassive；自动关窗由 Section 的 SetAutoClose 承担）。
  ${If} $PassiveMode = 1
    Return
  ${EndIf}
  Call MeowoEnsureBg
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  SetCtlColors $0 "${MEOWO_COL_TEXT}" "${MEOWO_COL_BASE}"

  ; 同主页：背景位图在本函数最末创建（z 序 = 创建序，后建垫底）。

  ; 「安装完成」：y=42%，标题字。
  IntOp $2 $MeowoW * 20
  IntOp $2 $2 / 100
  IntOp $3 $MeowoH * 42
  IntOp $3 $3 / 100
  IntOp $4 $MeowoW * 60
  IntOp $4 $4 / 100
  ${MeowoScale} $5 34
  nsDialogs::CreateControl STATIC "${__NSD_Label_STYLE}|${SS_CENTER}" "${__NSD_Label_EXSTYLE}" $2 $3 $4 $5 "$(meowoDoneTitle)"
  Pop $6
  SetCtlColors $6 "${MEOWO_COL_TEXT}" "${MEOWO_COL_BASE}"
  SendMessage $6 ${WM_SETFONT} $MeowoTitleFont 1

  ; 「立即体验」：与主页安装按钮同几何（胶囊烙在 -btn 背景图同一位置）。
  ${MeowoScale} $4 300
  IntOp $2 $MeowoW - $4
  IntOp $2 $2 / 2
  IntOp $3 $MeowoH * 56
  IntOp $3 $3 / 100
  ${MeowoScale} $5 15
  IntOp $3 $3 + $5
  ${MeowoScale} $5 22
  nsDialogs::CreateControl STATIC "${__NSD_Label_STYLE}|${SS_CENTER}|${SS_NOTIFY}" "${__NSD_Label_EXSTYLE}" $2 $3 $4 $5 "$(meowoLaunch)"
  Pop $7
  SetCtlColors $7 "0c211b" "${MEOWO_COL_LINK}"
  SendMessage $7 ${WM_SETFONT} $MeowoBtnFont 1
  ${NSD_OnClick} $7 MeowoOnLaunchClick

  ; 「关闭」链接：底带右下角（与主页「自定义安装」同位）。
  ${MeowoScale} $4 150
  ${MeowoScale} $5 24
  IntOp $2 $MeowoW - $4
  IntOp $2 $2 - $5
  ${MeowoScale} $6 18
  ${MeowoScale} $5 16
  IntOp $3 $MeowoH - $6
  IntOp $3 $3 - $5
  ; 同主页链接：不能用 NSD_CreateLink（首次获焦误触发点击——在这页等于装完瞬间关窗）。
  nsDialogs::CreateControl STATIC "${__NSD_Label_STYLE}|${SS_NOTIFY}" "${__NSD_Label_EXSTYLE}" $2 $3 $4 $6 "$(meowoFinish)"
  Pop $8
  SetCtlColors $8 "${MEOWO_COL_SUB}" "${MEOWO_COL_BAND}"
  SendMessage $8 ${WM_SETFONT} $MeowoLinkFont 1
  ${NSD_OnClick} $8 MeowoOnFinishClick

  ; 背景位图最后创建 = z 序垫底（理由见 MeowoMainPage 开头注释）。
  ${NSD_CreateBitmap} 0 0 100% 100% ""
  Pop $1
  ${NSD_SetStretchedImage} $1 "$PLUGINSDIR\meowo-bg-btn.bmp" $MeowoBgHandle

  nsDialogs::Show
FunctionEnd

Function MeowoOnLaunchClick
  Pop $0
  Call RunMainBinary
  SendMessage $HWNDPARENT ${WM_COMMAND} 1 0 ; 最后一页的「下一步」= 关闭安装器
FunctionEnd

Function MeowoOnFinishClick
  Pop $0
  SendMessage $HWNDPARENT ${WM_COMMAND} 1 0
FunctionEnd
; MEOWO-END done-page

; Uninstaller Pages
; 1. Confirm uninstall page
Var DeleteAppDataCheckbox
Var DeleteAppDataCheckboxState
!define /ifndef WS_EX_LAYOUTRTL         0x00400000
!define MUI_PAGE_CUSTOMFUNCTION_SHOW un.ConfirmShow
Function un.ConfirmShow ; Add add a `Delete app data` check box
  ; $1 inner dialog HWND
  ; $2 window DPI
  ; $3 style
  ; $4 x
  ; $5 y
  ; $6 width
  ; $7 height
  FindWindow $1 "#32770" "" $HWNDPARENT ; Find inner dialog
  System::Call "user32::GetDpiForWindow(p r1) i .r2"
  ${If} $(^RTL) = 1
    StrCpy $3 "${__NSD_CheckBox_EXSTYLE} | ${WS_EX_LAYOUTRTL}"
    IntOp $4 50 * $2
  ${Else}
    StrCpy $3 "${__NSD_CheckBox_EXSTYLE}"
    IntOp $4 0 * $2
  ${EndIf}
  IntOp $5 100 * $2
  IntOp $6 400 * $2
  IntOp $7 25 * $2
  IntOp $4 $4 / 96
  IntOp $5 $5 / 96
  IntOp $6 $6 / 96
  IntOp $7 $7 / 96
  System::Call 'user32::CreateWindowEx(i r3, w "${__NSD_CheckBox_CLASS}", w "$(deleteAppData)", i ${__NSD_CheckBox_STYLE}, i r4, i r5, i r6, i r7, p r1, i0, i0, i0) i .s'
  Pop $DeleteAppDataCheckbox
  SendMessage $HWNDPARENT ${WM_GETFONT} 0 0 $1
  SendMessage $DeleteAppDataCheckbox ${WM_SETFONT} $1 1
FunctionEnd
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE un.ConfirmLeave
Function un.ConfirmLeave
  SendMessage $DeleteAppDataCheckbox ${BM_GETCHECK} 0 0 $DeleteAppDataCheckboxState
FunctionEnd
!define MUI_PAGE_CUSTOMFUNCTION_PRE un.SkipIfPassive
!insertmacro MUI_UNPAGE_CONFIRM

; 2. Uninstalling Page
!insertmacro MUI_UNPAGE_INSTFILES

;Languages
{{#each languages}}
!insertmacro MUI_LANGUAGE "{{this}}"
{{/each}}
!insertmacro MUI_RESERVEFILE_LANGDLL
{{#each language_files}}
  !include "{{this}}"
{{/each}}

; MEOWO-BEGIN oneclick-langstrings
; 自绘页文案。项目语言固定为 SimpChinese + English（tauri.conf.json 的
; bundle.windows.nsis.languages）；增删语言时必须同步这里，否则编译期报未定义 LANG_*。
LangString meowoTagline ${LANG_SIMPCHINESE} "AI 会话贴纸看板"
LangString meowoTagline ${LANG_ENGLISH} "Your AI session sticker board"
LangString meowoOneClick ${LANG_SIMPCHINESE} "立即安装"
LangString meowoOneClick ${LANG_ENGLISH} "Install Now"
LangString meowoCustomOpen ${LANG_SIMPCHINESE} "自定义安装 ▾"
LangString meowoCustomOpen ${LANG_ENGLISH} "Custom install ▾"
LangString meowoCustomClose ${LANG_SIMPCHINESE} "收起自定义 ▴"
LangString meowoCustomClose ${LANG_ENGLISH} "Hide custom ▴"
LangString meowoInstallPath ${LANG_SIMPCHINESE} "安装位置"
LangString meowoInstallPath ${LANG_ENGLISH} "Install to"
LangString meowoBrowse ${LANG_SIMPCHINESE} "浏览…"
LangString meowoBrowse ${LANG_ENGLISH} "Browse…"
LangString meowoChatFeature ${LANG_SIMPCHINESE} "对话窗口功能（完整模式）"
LangString meowoChatFeature ${LANG_ENGLISH} "Chat window (full mode)"
LangString meowoChatHint ${LANG_SIMPCHINESE} "取消勾选则仅保留贴纸功能，之后可在设置中随时开启"
LangString meowoChatHint ${LANG_ENGLISH} "Uncheck for sticker-only lite mode; enable it later in Settings anytime"
LangString meowoDesktopLnk ${LANG_SIMPCHINESE} "创建桌面快捷方式"
LangString meowoDesktopLnk ${LANG_ENGLISH} "Create desktop shortcut"
LangString meowoNoteSame ${LANG_SIMPCHINESE} "检测到相同版本已安装，将原地重新安装"
LangString meowoNoteSame ${LANG_ENGLISH} "This version is already installed — it will be reinstalled in place"
LangString meowoNoteUpgrade ${LANG_SIMPCHINESE} "检测到已安装的旧版本，将覆盖升级"
LangString meowoNoteUpgrade ${LANG_ENGLISH} "An older version is installed — it will be upgraded in place"
LangString meowoNoteDowngrade ${LANG_SIMPCHINESE} "检测到更新的版本，安装前会先将其卸载"
LangString meowoNoteDowngrade ${LANG_ENGLISH} "A newer version is installed — it will be uninstalled first"
LangString meowoBadPath ${LANG_SIMPCHINESE} "请填写有效的安装位置"
LangString meowoBadPath ${LANG_ENGLISH} "Please enter a valid install location"
LangString meowoDoneTitle ${LANG_SIMPCHINESE} "安装完成"
LangString meowoDoneTitle ${LANG_ENGLISH} "All set"
LangString meowoLaunch ${LANG_SIMPCHINESE} "立即体验"
LangString meowoLaunch ${LANG_ENGLISH} "Launch Meowo"
LangString meowoFinish ${LANG_SIMPCHINESE} "关闭"
LangString meowoFinish ${LANG_ENGLISH} "Close"
; MEOWO-END oneclick-langstrings

Function .onInit
  ${GetOptions} $CMDLINE "/P" $PassiveMode
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/NS" $NoShortcutMode
  ${IfNot} ${Errors}
    StrCpy $NoShortcutMode 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/UPDATE" $UpdateMode
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}

  ; MEOWO-BEGIN oneclick-defaults
  ; 勾选默认值：主页 leave 不执行（passive/silent）时 Section 仍能读到产品默认。
  StrCpy $MeowoChatEnabled 1
  StrCpy $MeowoDesktopLnk 1
  ; STARTMENU 页已移除，按配置回填其变量（本项目恒空 → 快捷方式建在 $SMPROGRAMS 根）。
  !if "${STARTMENUFOLDER}" != ""
    StrCpy $AppStartMenuFolder "${STARTMENUFOLDER}"
  !endif
  ; MEOWO-END oneclick-defaults

  !if "${DISPLAYLANGUAGESELECTOR}" == "true"
    !insertmacro MUI_LANGDLL_DISPLAY
  !endif

  !insertmacro SetContext

  ${If} $INSTDIR == "${PLACEHOLDER_INSTALL_DIR}"
    ; Set default install location
    !if "${INSTALLMODE}" == "perMachine"
      ${If} ${RunningX64}
        !if "${ARCH}" == "x64"
          StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
        !else if "${ARCH}" == "arm64"
          StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
        !else
          StrCpy $INSTDIR "$PROGRAMFILES\${PRODUCTNAME}"
        !endif
      ${Else}
        StrCpy $INSTDIR "$PROGRAMFILES\${PRODUCTNAME}"
      ${EndIf}
    !else if "${INSTALLMODE}" == "currentUser"
      StrCpy $INSTDIR "$LOCALAPPDATA\${PRODUCTNAME}"
    !endif

    Call RestorePreviousInstallLocation
  ${EndIf}


  !if "${INSTALLMODE}" == "both"
    !insertmacro MULTIUSER_INIT
  !endif
FunctionEnd


Section EarlyChecks
  ; Abort silent installer if downgrades is disabled
  !if "${ALLOWDOWNGRADES}" == "false"
  ${If} ${Silent}
    ; If downgrading
    ${If} $R0 = -1
      System::Call 'kernel32::AttachConsole(i -1)i.r0'
      ${If} $0 <> 0
        System::Call 'kernel32::GetStdHandle(i -11)i.r0'
        System::call 'kernel32::SetConsoleTextAttribute(i r0, i 0x0004)' ; set red color
        FileWrite $0 "$(silentDowngrades)"
      ${EndIf}
      Abort
    ${EndIf}
  ${EndIf}
  !endif

SectionEnd

Section WebView2
  ; Check if Webview2 is already installed and skip this section
  ${If} ${RunningX64}
    ReadRegStr $4 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${Else}
    ReadRegStr $4 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${EndIf}
  ${If} $4 == ""
    ReadRegStr $4 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${EndIf}

  ${If} $4 == ""
    ; Webview2 installation
    ;
    ; Skip if updating
    ${If} $UpdateMode <> 1
      !if "${INSTALLWEBVIEW2MODE}" == "downloadBootstrapper"
        Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        DetailPrint "$(webview2Downloading)"
        NSISdl::download "https://go.microsoft.com/fwlink/p/?LinkId=2124703" "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Pop $0
        ${If} $0 == "success"
          DetailPrint "$(webview2DownloadSuccess)"
        ${Else}
          DetailPrint "$(webview2DownloadError)"
          Abort "$(webview2AbortError)"
        ${EndIf}
        StrCpy $6 "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Goto install_webview2
      !endif

      !if "${INSTALLWEBVIEW2MODE}" == "embedBootstrapper"
        Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        File "/oname=$TEMP\MicrosoftEdgeWebview2Setup.exe" "${WEBVIEW2BOOTSTRAPPERPATH}"
        DetailPrint "$(installingWebview2)"
        StrCpy $6 "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Goto install_webview2
      !endif

      !if "${INSTALLWEBVIEW2MODE}" == "offlineInstaller"
        Delete "$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe"
        File "/oname=$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe" "${WEBVIEW2INSTALLERPATH}"
        DetailPrint "$(installingWebview2)"
        StrCpy $6 "$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe"
        Goto install_webview2
      !endif

      Goto webview2_done

      install_webview2:
        DetailPrint "$(installingWebview2)"
        ; $6 holds the path to the webview2 installer
        ExecWait "$6 ${WEBVIEW2INSTALLERARGS} /install" $1
        ${If} $1 = 0
          DetailPrint "$(webview2InstallSuccess)"
        ${Else}
          DetailPrint "$(webview2InstallError)"
          Abort "$(webview2AbortError)"
        ${EndIf}
      webview2_done:
    ${EndIf}
  ${Else}
    !if "${MINIMUMWEBVIEW2VERSION}" != ""
      ${VersionCompare} "${MINIMUMWEBVIEW2VERSION}" "$4" $R0
      ${If} $R0 = 1
        update_webview:
          DetailPrint "$(installingWebview2)"
          ${If} ${RunningX64}
            ReadRegStr $R1 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate" "path"
          ${Else}
            ReadRegStr $R1 HKLM "SOFTWARE\Microsoft\EdgeUpdate" "path"
          ${EndIf}
          ${If} $R1 == ""
            ReadRegStr $R1 HKCU "SOFTWARE\Microsoft\EdgeUpdate" "path"
          ${EndIf}
          ${If} $R1 != ""
            ; Chromium updater docs: https://source.chromium.org/chromium/chromium/src/+/main:docs/updater/user_manual.md
            ; Modified from "HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Microsoft EdgeWebView\ModifyPath"
            ExecWait `"$R1" /install appguid=${WEBVIEW2APPGUID}&needsadmin=true` $1
            ${If} $1 = 0
              DetailPrint "$(webview2InstallSuccess)"
            ${Else}
              MessageBox MB_ICONEXCLAMATION|MB_ABORTRETRYIGNORE "$(webview2InstallError)" IDIGNORE ignore IDRETRY update_webview
              Quit
              ignore:
            ${EndIf}
          ${EndIf}
      ${EndIf}
    !endif
  ${EndIf}
SectionEnd

Section Install
  SetOutPath $INSTDIR

  !ifmacrodef NSIS_HOOK_PREINSTALL
    !insertmacro NSIS_HOOK_PREINSTALL
  !endif

  !insertmacro CheckIfAppIsRunning "${MAINBINARYNAME}.exe" "${PRODUCTNAME}"

  ; Copy main executable
  File "${MAINBINARYSRCPATH}"

  ; Copy resources
  {{#each resources_dirs}}
    CreateDirectory "$INSTDIR\\{{this}}"
  {{/each}}
  {{#each resources}}
    File /a "/oname={{this.[1]}}" "{{no-escape @key}}"
  {{/each}}

  ; Copy external binaries
  {{#each binaries}}
    File /a "/oname={{this}}" "{{no-escape @key}}"
  {{/each}}

  ; Create file associations
  {{#each file_associations as |association| ~}}
    {{#each association.ext as |ext| ~}}
       !insertmacro APP_ASSOCIATE "{{ext}}" "{{or association.name ext}}" "{{association-description association.description ext}}" "$INSTDIR\${MAINBINARYNAME}.exe,0" "Open with ${PRODUCTNAME}" "$INSTDIR\${MAINBINARYNAME}.exe $\"%1$\""
    {{/each}}
  {{/each}}

  ; Register deep links
  {{#each deep_link_protocols as |protocol| ~}}
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}" "URL Protocol" ""
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}" "" "URL:${BUNDLEID} protocol"
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
  {{/each}}

  ; Create uninstaller
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Save $INSTDIR in registry for future installations
  WriteRegStr SHCTX "${MANUPRODUCTKEY}" "" $INSTDIR

  !if "${INSTALLMODE}" == "both"
    ; Save install mode to be selected by default for the next installation such as updating
    ; or when uninstalling
    WriteRegStr SHCTX "${UNINSTKEY}" $MultiUser.InstallMode 1
  !endif

  ; Remove old main binary if it doesn't match new main binary name
  ReadRegStr $OldMainBinaryName SHCTX "${UNINSTKEY}" "MainBinaryName"
  ${If} $OldMainBinaryName != ""
  ${AndIf} $OldMainBinaryName != "${MAINBINARYNAME}.exe"
    Delete "$INSTDIR\$OldMainBinaryName"
  ${EndIf}

  ; Save current MAINBINARYNAME for future updates
  WriteRegStr SHCTX "${UNINSTKEY}" "MainBinaryName" "${MAINBINARYNAME}.exe"

  ; Registry information for add/remove programs
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayName" "${PRODUCTNAME}"
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayIcon" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\""
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr SHCTX "${UNINSTKEY}" "Publisher" "${MANUFACTURER}"
  WriteRegStr SHCTX "${UNINSTKEY}" "InstallLocation" "$\"$INSTDIR$\""
  WriteRegStr SHCTX "${UNINSTKEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoModify" "1"
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoRepair" "1"

  ${GetSize} "$INSTDIR" "/M=uninstall.exe /S=0K /G=0" $0 $1 $2
  IntOp $0 $0 + ${ESTIMATEDSIZE}
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD SHCTX "${UNINSTKEY}" "EstimatedSize" "$0"

  !if "${HOMEPAGE}" != ""
    WriteRegStr SHCTX "${UNINSTKEY}" "URLInfoAbout" "${HOMEPAGE}"
    WriteRegStr SHCTX "${UNINSTKEY}" "URLUpdateInfo" "${HOMEPAGE}"
    WriteRegStr SHCTX "${UNINSTKEY}" "HelpLink" "${HOMEPAGE}"
  !endif

  ; MEOWO-BEGIN chat-enabled-seed
  ; 「对话窗口功能」勾选写成注册表种子，应用首启读取合并进 settings.json 后即焚
  ; （app/src-tauri/src/seed.rs）。只在 GUI 交互安装写：updater 静默升级(/P /UPDATE)
  ; 与 /S 静默装不代表用户做了新选择，绝不覆写存量用户的偏好。
  ${If} $UpdateMode <> 1
  ${AndIf} $PassiveMode <> 1
  ${AndIfNot} ${Silent}
    WriteRegDWORD SHCTX "${MANUPRODUCTKEY}" "ChatEnabled" $MeowoChatEnabled
  ${EndIf}
  ; MEOWO-END chat-enabled-seed

  ; Create start menu shortcut
  ; MEOWO-BEGIN startmenu-direct
  ; STARTMENU 页已移除（MUI_STARTMENU_WRITE_* 依赖页面声明）；直接建快捷方式，
  ; 与上游「页面恒 Skip + 宏退化为直调」的实效一致。
  Call CreateOrUpdateStartMenuShortcut
  ; MEOWO-END startmenu-direct

  ; Create desktop shortcut for silent and passive installers
  ; because finish page will be skipped
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    Call CreateOrUpdateDesktopShortcut
  ; MEOWO-BEGIN desktop-shortcut-choice
  ; GUI：按主页「创建桌面快捷方式」勾选（上游放在 FINISH 页，勾选挪到主页后
  ; 建档时机随之挪进 Section）。/NS 与 /UPDATE 的拦截仍在函数内部，覆盖所有调用点。
  ${ElseIf} $MeowoDesktopLnk = 1
    Call CreateOrUpdateDesktopShortcut
  ; MEOWO-END desktop-shortcut-choice
  ${EndIf}

  !ifmacrodef NSIS_HOOK_POSTINSTALL
    !insertmacro NSIS_HOOK_POSTINSTALL
  !endif

  ; Auto close this page for passive mode
  ; MEOWO: 改为无条件——GUI 下自动前进到自绘完成页（细节列表已隐藏，停留无意义）；
  ; passive 下完成页自跳，行为与上游等价。
  SetAutoClose true
SectionEnd

Function .onInstSuccess
  ; Check for `/R` flag only in silent and passive installers because
  ; GUI installer has a toggle for the user to (re)start the app
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    ${GetOptions} $CMDLINE "/R" $R0
    ${IfNot} ${Errors}
      ${GetOptions} $CMDLINE "/ARGS" $R0
      nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" "$R0"
    ${EndIf}
  ${EndIf}
FunctionEnd

Function un.onInit
  !insertmacro SetContext

  !if "${INSTALLMODE}" == "both"
    !insertmacro MULTIUSER_UNINIT
  !endif

  !insertmacro MUI_UNGETLANGUAGE

  ${GetOptions} $CMDLINE "/P" $PassiveMode
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/UPDATE" $UpdateMode
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}
FunctionEnd

Section Uninstall

  !ifmacrodef NSIS_HOOK_PREUNINSTALL
    !insertmacro NSIS_HOOK_PREUNINSTALL
  !endif

  !insertmacro CheckIfAppIsRunning "${MAINBINARYNAME}.exe" "${PRODUCTNAME}"

  ; Delete the app directory and its content from disk
  ; Copy main executable
  Delete "$INSTDIR\${MAINBINARYNAME}.exe"

  ; Delete resources
  {{#each resources}}
    Delete "$INSTDIR\\{{this.[1]}}"
  {{/each}}

  ; Delete external binaries
  {{#each binaries}}
    Delete "$INSTDIR\\{{this}}"
  {{/each}}

  ; Delete app associations
  {{#each file_associations as |association| ~}}
    {{#each association.ext as |ext| ~}}
      !insertmacro APP_UNASSOCIATE "{{ext}}" "{{or association.name ext}}"
    {{/each}}
  {{/each}}

  ; Delete deep links
  {{#each deep_link_protocols as |protocol| ~}}
    ReadRegStr $R7 SHCTX "Software\Classes\\{{protocol}}\shell\open\command" ""
    ${If} $R7 == "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
      DeleteRegKey SHCTX "Software\Classes\\{{protocol}}"
    ${EndIf}
  {{/each}}


  ; Delete uninstaller
  Delete "$INSTDIR\uninstall.exe"

  {{#each resources_ancestors}}
  RMDir /REBOOTOK "$INSTDIR\\{{this}}"
  {{/each}}
  RMDir "$INSTDIR"

  ; Remove shortcuts if not updating
  ${If} $UpdateMode <> 1
    !insertmacro DeleteAppUserModelId

    ; Remove start menu shortcut
    ; MEOWO: STARTMENU 页已移除（GETFOLDER 宏依赖页面声明）；未配置注册表存储时
    ; 该宏本就退化为取当前变量值，按配置直接回填等价。
    !if "${STARTMENUFOLDER}" != ""
      StrCpy $AppStartMenuFolder "${STARTMENUFOLDER}"
    !endif
    !insertmacro IsShortcutTarget "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
      Delete "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
      RMDir "$SMPROGRAMS\$AppStartMenuFolder"
    ${EndIf}
    !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk"
      Delete "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    ${EndIf}

    ; Remove desktop shortcuts
    !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$DESKTOP\${PRODUCTNAME}.lnk"
      Delete "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}
  ${EndIf}

  ; Remove registry information for add/remove programs
  !if "${INSTALLMODE}" == "both"
    DeleteRegKey SHCTX "${UNINSTKEY}"
  !else if "${INSTALLMODE}" == "perMachine"
    DeleteRegKey HKLM "${UNINSTKEY}"
  !else
    DeleteRegKey HKCU "${UNINSTKEY}"
  !endif

  ; Removes the Autostart entry for ${PRODUCTNAME} from the HKCU Run key if it exists.
  ; This ensures the program does not launch automatically after uninstallation if it exists.
  ; If it doesn't exist, it does nothing.
  ; We do this when not updating (to preserve the registry value on updates)
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"
    ; MEOWO: 对话功能种子正常已被应用读后即焚，这里是「装完从未启动就卸载」的兜底。
    DeleteRegValue SHCTX "${MANUPRODUCTKEY}" "ChatEnabled"
  ${EndIf}

  ; Delete app data if the checkbox is selected
  ; and if not updating
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    ; Clear the install location $INSTDIR from registry
    DeleteRegKey SHCTX "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty SHCTX "${MANUKEY}"

    ; Clear the install language from registry
    DeleteRegValue HKCU "${MANUPRODUCTKEY}" "Installer Language"
    DeleteRegKey /ifempty HKCU "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty HKCU "${MANUKEY}"

    SetShellVarContext current
    RmDir /r "$APPDATA\${BUNDLEID}"
    RmDir /r "$LOCALAPPDATA\${BUNDLEID}"
  ${EndIf}

  !ifmacrodef NSIS_HOOK_POSTUNINSTALL
    !insertmacro NSIS_HOOK_POSTUNINSTALL
  !endif

  ; Auto close if passive mode or updating
  ${If} $PassiveMode = 1
  ${OrIf} $UpdateMode = 1
    SetAutoClose true
  ${EndIf}
SectionEnd

Function RestorePreviousInstallLocation
  ReadRegStr $4 SHCTX "${MANUPRODUCTKEY}" ""
  StrCmp $4 "" +2 0
    StrCpy $INSTDIR $4
FunctionEnd

Function Skip
  Abort
FunctionEnd

Function SkipIfPassive
  ${IfThen} $PassiveMode = 1  ${|} Abort ${|}
FunctionEnd
Function un.SkipIfPassive
  ${IfThen} $PassiveMode = 1  ${|} Abort ${|}
FunctionEnd

Function CreateOrUpdateStartMenuShortcut
  ; We used to use product name as MAINBINARYNAME
  ; migrate old shortcuts to target the new MAINBINARYNAME
  StrCpy $R0 0

  !insertmacro IsShortcutTarget "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    StrCpy $R0 1
  ${EndIf}

  !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    StrCpy $R0 1
  ${EndIf}

  ${If} $R0 = 1
    Return
  ${EndIf}

  ; Skip creating shortcut if in update mode or no shortcut mode
  ; but always create if migrating from wix
  ${If} $WixMode = 0
    ${If} $UpdateMode = 1
    ${OrIf} $NoShortcutMode = 1
      Return
    ${EndIf}
  ${EndIf}

  !if "${STARTMENUFOLDER}" != ""
    CreateDirectory "$SMPROGRAMS\$AppStartMenuFolder"
    CreateShortcut "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
  !else
    CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
  !endif
FunctionEnd

Function CreateOrUpdateDesktopShortcut
  ; We used to use product name as MAINBINARYNAME
  ; migrate old shortcuts to target the new MAINBINARYNAME
  !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Return
  ${EndIf}

  ; Skip creating shortcut if in update mode or no shortcut mode
  ; but always create if migrating from wix
  ${If} $WixMode = 0
    ${If} $UpdateMode = 1
    ${OrIf} $NoShortcutMode = 1
      Return
    ${EndIf}
  ${EndIf}

  CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
FunctionEnd
