; 安装前清理旧品牌 cc-kanban（≤ 0.4.2）的安装。
;
; 为什么需要这个：Tauri 的 NSIS 模板把卸载注册表键定成
;   Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}
; 安装目录也是 $LOCALAPPDATA\${PRODUCTNAME}。改名 cc-kanban → Meowo 之后，新安装器
; 只按新 PRODUCTNAME 去找「已装版本」，找不到旧键便认定是全新安装，于是旧版原地不动——
; 两个应用并存，各自开机自启、各自跑一份托盘。
;
; 这里在 PREINSTALL 阶段按**旧** PRODUCTNAME 再找一遍，调用旧的 uninstall.exe。参数与
; 模板自己的重装路径（PageLeaveReinstall）保持一致：
;   /P    透传 passive，让旧卸载器别弹窗（updater 静默更新时它会直接杀掉旧进程）
;   _?=   让卸载器在原目录同步执行，而不是复制到临时目录后立刻返回——否则 ExecWait 等不到它
;
; 这段是一次性的历史包袱：等 0.4.2 的存量用户升完，可以整个删掉。

!include LogicLib.nsh

!define LEGACY_NAME "cc-kanban"
!define LEGACY_UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${LEGACY_NAME}"
; 旧版的主程序与 sidecar（注意都不叫 cc-kanban，卸载器只认得前者）
!define LEGACY_MAINBINARY "cc-app.exe"
!define LEGACY_SIDECAR "cc-reporter.exe"

; 卸旧版会连它的快捷方式一起删掉（那是指向即将消失的 cc-kanban.exe 的死链，必须删）。
; 记下删之前有哪些，POSTINSTALL 再按新名字补回来——详见那里的说明。
Var LegacyHadDesktopLnk
Var LegacyHadStartMenuLnk

; ROOT：HKCU（currentUser 安装，Tauri 默认）或 HKLM（perMachine 安装）。
; 用 $R6-$R9：PREINSTALL 展开处（Section Install 开头）它们还没人占，紧随其后的
; CheckIfAppIsRunning 用的是 $R0-$R3，不冲突。
!macro UninstallLegacy ROOT
  ReadRegStr $R8 ${ROOT} "${LEGACY_UNINSTKEY}" "UninstallString"
  ${If} $R8 != ""
    ; InstallLocation 是带引号写进注册表的（"C:\..."），而 _?= 只吃裸路径
    ReadRegStr $R9 ${ROOT} "${LEGACY_UNINSTKEY}" "InstallLocation"
    StrCpy $R6 $R9 1
    ${If} $R6 == '"'
      StrCpy $R9 $R9 "" 1
      StrCpy $R9 $R9 -1
    ${EndIf}
    ${If} $R9 == ""
      StrCpy $R9 "$LOCALAPPDATA\${LEGACY_NAME}"
    ${EndIf}

    ; InstallLocation 来自注册表，不能拿它直接做 ExecWait + RMDir /r。旧版是 currentUser
    ; 安装，唯一合法目录就是这个固定位置；异常/被篡改的键只提示并跳过，绝不碰任意目录。
    ${If} $R9 != "$LOCALAPPDATA\${LEGACY_NAME}"
      DetailPrint "Skipping legacy cleanup: unexpected InstallLocation '$R9'."
    ${ElseIfNot} ${FileExists} "$R9\uninstall.exe"
      ; 目录已被手动删掉、只剩注册表：清键了事，否则每次安装都要白跑一遍
      DeleteRegKey ${ROOT} "${LEGACY_UNINSTKEY}"
    ${Else}
      DetailPrint "Uninstalling legacy ${LEGACY_NAME}..."
      ${IfThen} $PassiveMode = 1 ${|} StrCpy $R8 "$R8 /P" ${|}
      StrCpy $R8 "$R8 _?=$R9"
      ClearErrors
      ExecWait '$R8' $R7

      ${If} ${Errors}
      ${OrIf} $R7 <> 0
        ; 卸载失败不阻断安装：装上新版更重要，最坏也就是旧版残留（与现状持平）
        DetailPrint "Legacy ${LEGACY_NAME} uninstall did not complete (code=$R7), continuing."
      ${Else}
        ; 走到这儿旧卸载器已经报成功，但旧目录多半还在，得自己收尾：
        ;
        ; 1. sidecar 是 agent 的 hooks 拉起来的常驻进程，卸载器压根不知道它存在（只 kill 主程序），
        ;    它锁着自己的 exe，Delete 静默失败 → 目录非空 → RMDir 失败 → 整个旧目录留下来。
        ;    实测就是这么残留的：卸载器把注册表和快捷方式都清了，却留下一个装着两个 exe 的目录。
        ;    杀它是安全的——它指向的路径下一秒就没了，而新版启动时会把 hooks 重新指到 meowo-reporter。
        ; 2. 主程序刚被卸载器 kill 掉，映像句柄未必立刻释放，它那次 Delete 也可能没删成。
        nsis_tauri_utils::KillProcessCurrentUser "${LEGACY_SIDECAR}"
        Pop $R6
        nsis_tauri_utils::KillProcessCurrentUser "${LEGACY_MAINBINARY}"
        Pop $R6
        Sleep 1000

        ; 带 _?= 时 NSIS 不会自删卸载器，uninstall.exe 必然还在（此刻它已退出，可以删）
        Delete "$R9\uninstall.exe"
        ; 到这一步目录里只该剩旧 exe。/r 是为了别再被任何漏网文件卡住——
        ; $R9 是从旧安装自己的 InstallLocation 读出来的，不是拼出来的路径。
        RMDir /r "$R9"
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; 先记账再卸载：旧卸载器一跑，这些快捷方式就没了
  ${If} ${FileExists} "$DESKTOP\${LEGACY_NAME}.lnk"
    StrCpy $LegacyHadDesktopLnk 1
  ${EndIf}
  ${If} ${FileExists} "$SMPROGRAMS\${LEGACY_NAME}.lnk"
    StrCpy $LegacyHadStartMenuLnk 1
  ${EndIf}

  !insertmacro UninstallLegacy HKCU
  !insertmacro UninstallLegacy HKLM

  ; 开机自启项是 app 自己（tauri-plugin-autostart）写的，键名取 productName，
  ; 卸载器只管 INSTDIR，不会碰它——留着就是一条指向已删除 exe 的死自启项。
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${LEGACY_NAME}"

  ; ── 升级清场：把锁着待覆盖文件的进程先清掉，并**等到真正消失** ──
  ;
  ; 「安装到一半提示无法复制 meowo-app.exe」（实拍）的三个锁点：
  ;  1. 第二个 Meowo 实例（自启版 + 手动再开的并存）。模板内置的 CheckIfAppIsRunning
  ;     虽会按名杀进程，但 kill 之后只盲睡 500ms 就开始复制、不复查进程是否真死——
  ;     退出慢（shutdown 要杀子孙树、清 ConPTY）或根本杀不死（线程卡死在 ConPTY 内核
  ;     I/O 时 TerminateProcess 静默无效，与「结束会话失效」同一根因）时照样撞锁。
  ;  2. meowo-reporter.exe：外部同步终端里的 attach 客户端是长驻进程，锁着 sidecar。
  ;  3. OpenConsole.exe（0.5.14 起随应用打包）：孤儿化的 ConPTY 宿主从安装目录加载，
  ;     锁着 OpenConsole.exe 与 conpty.dll。必须按**路径**精确杀——按映像名全杀会把
  ;     Windows Terminal 自己的宿主一起带走。
  ; 杀完复查而不是盲睡：最多等 10s；等不到也放行，交给内置宏再试一轮并给出它的标准
  ; 失败提示（不比现状差）。寄存器用 $R4/$R5：避开本文件的 $R6-$R9 与内置宏的 $R0-$R3。
  DetailPrint "Closing running ${PRODUCTNAME} processes..."
  nsis_tauri_utils::KillProcessCurrentUser "${MAINBINARYNAME}.exe"
  Pop $R4
  nsis_tauri_utils::KillProcessCurrentUser "meowo-reporter.exe"
  Pop $R4
  ; NSIS 里字面 $ 须写作 $$（$$_ 即 PowerShell 的 $_），\" 是传给 PS 的字面引号。
  nsExec::ExecToStack 'powershell -NoProfile -Command "Get-Process OpenConsole -ErrorAction SilentlyContinue | Where-Object { $$_.Path -eq \"$INSTDIR\OpenConsole.exe\" } | Stop-Process -Force"'
  Pop $R4
  StrCpy $R5 20
  meowo_wait_dead:
    nsis_tauri_utils::FindProcessCurrentUser "${MAINBINARYNAME}.exe"
    Pop $R4
    ${If} $R4 != 0
      Goto meowo_wait_done     ; 非 0 = 已经找不到，锁已释放
    ${EndIf}
    Sleep 500
    IntOp $R5 $R5 - 1
    ${If} $R5 > 0
      Goto meowo_wait_dead
    ${EndIf}
  meowo_wait_done:

  ; ── 终极兜底：把待覆盖文件**改名挪开**，杀不死也照装 ──
  ;
  ; 上面的清场对两类锁点无能为力（0.5.14 升级实拍仍弹「无法复制」）：
  ;  1. 内核僵尸——线程卡死在 ConPTY 内核 I/O 的 meowo-app，TerminateProcess 静默
  ;     无效，等多久都不死；
  ;  2. 复活竞态——CC 状态行每 ~300ms 拉起一次 meowo-reporter，杀掉的下一秒又有新
  ;     进程锁住 exe，File 复制的瞬间碰撞概率极高。
  ; Windows 锁运行中 exe 锁的是**文件内容，不是目录项**：Rename 对运行中的映像合法
  ; （Chrome/Firefox 更新器的标准做法）。挪开后 File 写全新文件零冲突；老进程继续跑
  ; 在改名后的映像上自然消亡。临时名用 GetTempFileName 保证唯一（固定 .old 会撞上
  ; 上一次升级还活着的老进程）；随手 Delete /REBOOTOK——没人占着就当场删，占着就登记
  ; PendingFileRenameOperations 等重启清理，不留累积残留。
  ; Rename 自身失败（目录 ACL 等极端）静默放过：File 那步照旧弹重试，不比现状差。
  !insertmacro MeowoRenameAside "${MAINBINARYNAME}.exe"
  !insertmacro MeowoRenameAside "meowo-reporter.exe"
  !insertmacro MeowoRenameAside "OpenConsole.exe"
  !insertmacro MeowoRenameAside "conpty.dll"
!macroend

; 把 $INSTDIR 里的一个文件挪到唯一临时名并尽力删除（被锁则重启后删）。
; 寄存器只用 $R4（PREINSTALL 段内 $R0-$R3 归内置宏、$R6-$R9 归旧品牌清理）。
!macro MeowoRenameAside FileName
  ${If} ${FileExists} "$INSTDIR\${FileName}"
    GetTempFileName $R4 "$INSTDIR"
    Delete $R4
    ClearErrors
    Rename "$INSTDIR\${FileName}" $R4
    ${If} ${Errors}
      DetailPrint "Could not move aside ${FileName}; installer will overwrite in place."
    ${Else}
      Delete /REBOOTOK $R4
    ${EndIf}
  ${EndIf}
!macroend

; 把旧品牌的快捷方式按新名字补回来。
;
; 为什么非补不可：tauri-plugin-updater 调安装器时**硬编码**传 /UPDATE（不可配），于是
; $UpdateMode = 1，而模板的 CreateOrUpdateDesktopShortcut / CreateOrUpdateStartMenuShortcut
; 在 update mode 下都直接 Return——同名升级时这么做是对的（旧快捷方式指向同一个 INSTDIR，
; 原地就能继续用，不必重建）。但我们改了名：旧的那份刚被上面的卸载器删掉，新的这份模板又
; 不肯建，一来一回用户的桌面和开始菜单就空了。这里把它补上。
;
; 只在 update mode 下补：GUI 安装（$UpdateMode = 0）时模板自己会建开始菜单快捷方式，桌面的
; 那份则由结束页的勾选框决定——那是用户的选择，不该由我们越过去替他建。
!macro NSIS_HOOK_POSTINSTALL
  ${If} $UpdateMode = 1
    ${If} $LegacyHadDesktopLnk = 1
      CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
      !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}
    ${If} $LegacyHadStartMenuLnk = 1
      ; 与模板 CreateOrUpdateStartMenuShortcut 的条件编译保持一致
      !if "${STARTMENUFOLDER}" != ""
        CreateDirectory "$SMPROGRAMS\$AppStartMenuFolder"
        CreateShortcut "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
        !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
      !else
        CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
        !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
      !endif
    ${EndIf}
  ${EndIf}
!macroend
