# meowo-app(Tauri 后端)

## Command 线程纪律

Tauri v2 中**非 async 的 command 在主线程(消息泵)上执行**——主线程一被阻塞,
所有窗口一起进入"未响应"。历史上已两次栽在这上面(0.2.0 设置页卡死于主线程
spawn 子进程;0.5.x 整应用偶发未响应于主线程写 ConPTY 管道)。

- 同步 command 只允许纯内存操作:读原子量、微秒级临界区的 map 读写、窗口句柄转发。
- 凡是碰**文件 / 注册表 / 数据库 / 进程 spawn / 网络 / ConPTY / 可能被后台线程
  长持的锁**,一律 `async fn` + `tauri::async_runtime::spawn_blocking`。
- `State<'_, T>` 进不了 blocking 闭包:clone 出需要的字段(多为 `Arc`),或改收
  `tauri::AppHandle` 在闭包内 `app.state::<T>()`。
- PTY 输入永远走 `PtyBroker::write` 的有界队列,任何线程都不得直接对管道做阻塞写
  (子进程不读 stdin 时管道写会无限期阻塞)。
- 托盘/菜单(muda)对象有线程亲和,只能在主线程操作:后台线程要改它们时用
  `app.run_on_main_thread`。
- 对话窗的 PTY 实时输出必须经 emitter 合帧线程发送,不得在 reader 里逐 chunk
  emit(重输出时每秒数百条事件会打满主线程事件循环与 WebView2 IPC)。

## 平台专属代码的验证

`src/macos/**` 与其它 `#[cfg(target_os = "macos")]` 分支在 Windows 开发机上**根本不编译**——
本地 `cargo test`/`clippy` 全绿**不代表**它们能编译。交叉验证也不可行(实测:`objc2`
要编译 Objective-C,需 macOS SDK)。

故:改动 macOS 专属代码后,唯一的验证是 CI 的 `macos-latest` 矩阵(`cargo clippy
--workspace --all-targets`)。推送前请自觉逐行复核该分支的语法与 API,并在 PR 里
明确说明"macOS 侧未本机验证,依赖 CI"。反向同理(在 mac 上改 Windows 专属代码)。

## 屏幕状态检测的规则维护

`detect.rs` 的规则匹配的是各 agent TUI 的**界面文案**,随对方版本改版而失效。改规则
前先取证:`screen_detect_explain` 命令返回该会话的真实末屏文本、命中规则与回退原因
——照着真实屏幕改,不要凭想象改。

规则失准的表现必须是**少报 blocked**而不是误报:一条都没命中时回退 idle
(`FALLBACK_RULE_ID`),弱证据规则不带 `visible` 标志。宁可漏掉一次"agent 在等你",
不可把正在跑的会话谎报成阻塞。
