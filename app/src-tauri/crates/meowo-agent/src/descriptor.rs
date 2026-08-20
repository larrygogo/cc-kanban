//! Agent 描述符：`list_agents()` 下发前端的每-agent 能力总览。
//!
//! 全部字段由插件声明推导（能力槽是否声明、`ProxySpec` 的形状），组装因此住在插件层——
//! 宿主只负责把它经 Tauri command 透传。图标与品牌色**不在此**：那是前端资产不是数据
//! （kimi 的 logo 是位图、claude 品牌橙分明暗两值，见 docs/architecture/agent-plugin.md）。

use crate::registry::AgentPlugin;

#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct AgentDescriptor {
    pub id: String,
    /// 产品名（"Claude Code" / "Kimi Code" / "Codex"）。**不翻译**——产品名没有译名。
    pub display_name: String,
    /// 可执行是否装在本机（决定各处是否列出/可选它）。
    pub installed: bool,
    /// 这个 agent **能不能被套上代理**（＝插件是否声明了 `ProxySpec`）。
    ///
    /// 为 false 的 agent，设置页不给它代理行。没有这个字段时，前端只能给每个 agent 都画一行——
    /// 于是用户会给一个根本读不到代理配置的 agent 认真填上代理，然后对着「连不上」毫无线索地瞎试。
    /// 这正是网络分区最忌讳的失败模式：**静默不生效**。宁可不给入口，也不给一个假的。
    pub supports_proxy: bool,
    /// 代理能写进它**自己的配置文件**吗（＝`ProxySpec.config_env`）——从而不管由谁启动
    /// 都生效；false 则只覆盖 Meowo 拉起的会话，用户自己在终端敲命令时不走代理。
    pub proxy_covers_all_launches: bool,
    /// 支持 SOCKS 代理吗（＝`ProxySpec.socks`）。填错的后果是静默连不上，故设置页要
    /// 按它当场拒绝非法代理串，而不是等用户去猜。
    pub proxy_accepts_socks: bool,
    /// 这个 agent 有没有**账号概念**（＝插件是否声明了 account 能力槽）。
    ///
    /// 为 false 时，设置页与新建会话面板都不得显示登录态、也不得给出登录入口——它的
    /// `login_argv()` 是 `None`，按钮点下去只会得到一句「拉起登录失败」。
    ///
    /// 没有这个字段时，前端只能靠「账号查不出来」推断，而那与「真的没登录」长得一模一样：
    /// gemini / opencode 因此被判成「未登录」，亮出一个必然失败的按钮。**给出走不通的入口，
    /// 比不给入口更糟**——用户会以为是自己的问题，反复去点。
    pub supports_account: bool,
    /// 这个 agent 能不能**用 API Key 登录**（＝插件声明了 `ApiKeyLoginCap`）。
    ///
    /// 为 gemini 而设：Google 停掉了个人版 Code Assist 的 OAuth（交互式登录必然失败），
    /// API Key 是唯一活路，而它没有「输入 key」的登录子命令——必须由 meowo 提供入口。
    /// 为 true 时，前端在未登录状态额外给出「填 API Key」输入。
    pub supports_api_key_login: bool,
    /// 这个 agent 能不能有**多个账号**（＝插件声明了 `ProfileSpec`）。
    ///
    /// false（gemini：数据目录不可被环境变量覆盖）→ 前端不给「添加账号」入口。「只有一个默认账号」
    /// 与「压根不支持多账号」在账号列表上长得一模一样（都只有一条），必须由后端如实说清，
    /// 否则会给一个点了必然报错的按钮。
    pub supports_profiles: bool,
    /// meowo 能否显示这个 agent 的**上下文占用**（贴纸上的百分比液柱）。
    ///
    /// 为 false（gemini：官方 hook 不给 token；opencode：会话 token 在它自己库里，不经 hook）时，
    /// 前端显式标注「上下文占用：不支持」——不留空白让用户以为是 bug。
    pub supports_context: bool,
    /// 这个 agent 的会话历史能不能**导出交接**（＝transcript 能力槽 + `supports_chat()`）。
    ///
    /// 决定「切换引擎」入口的可见性：为 false（gemini/opencode：无结构化 transcript）的
    /// 会话没有可交接的历史，跨 provider 切换只能以它为**目标**、不能以它为来源。
    /// 前端不得按 id 判断——这正是守卫测试盯着的那类分支。
    pub supports_chat_export: bool,
    /// 这个 agent 支不支持**一个会话访问多个目录**（＝插件声明了 `extra_dir_flag`）。
    /// 为 true 时新建面板给「附加目录」入口（跨仓同一需求开一个会话）；false 不显示。
    pub supports_extra_dirs: bool,
    /// 新建会话的启动选项（选择 → CLI flag 映射，由插件声明）。空 = 面板不给选项栏。
    /// 前端只回传 choice id，翻译成 argv 在后端按这张表进行——用户输入进不了命令行。
    pub launch_options: &'static [crate::LaunchOption],
    /// 插件显式声明才存在；None 时前端不显示中转入口。
    pub relay: Option<crate::relay::RelayUi>,
}

impl AgentDescriptor {
    /// 由插件声明组装描述符。relay 需按已装变体门控（未装/变体不支持则不下发入口）。
    pub fn of(plugin: &'static dyn AgentPlugin) -> Self {
        let relay = plugin.relay().and_then(|cap| {
            let installation = plugin.resolve()?;
            cap.supports_variant(installation.variant_tag)
                .then(|| cap.ui())
        });
        Self {
            id: plugin.id().as_str().to_string(),
            display_name: plugin.display_name().to_string(),
            installed: plugin.is_installed(),
            supports_proxy: plugin.proxy().is_some(),
            // 没声明 ProxySpec 的 agent 这两项无意义（前端也不会给它代理行）。
            proxy_covers_all_launches: plugin.proxy().is_some_and(|spec| spec.config_env),
            proxy_accepts_socks: plugin.proxy().is_some_and(|spec| spec.socks),
            supports_account: plugin.account().is_some(),
            supports_api_key_login: plugin.api_key_login().is_some(),
            supports_profiles: plugin.profile().is_some(),
            supports_context: plugin.provides_context(),
            supports_chat_export: plugin
                .telemetry()
                .and_then(|telemetry| telemetry.transcript())
                .is_some_and(|spec| spec.supports_chat()),
            supports_extra_dirs: plugin.extra_dir_flag().is_some(),
            launch_options: plugin.launch_options(),
            relay,
        }
    }
}
