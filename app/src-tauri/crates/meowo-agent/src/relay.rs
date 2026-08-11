//! API 中转能力。插件显式声明才支持；宿主只负责密钥存储、HTTP 与进程启动。

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct RelayOption {
    pub value: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct RelaySuggestionGroup {
    /// 空串表示默认组；其它值对应协议 value。
    pub protocol: &'static str,
    pub models: &'static [&'static str],
}

/// 随中转注入的附加环境变量选项（如 Claude Code 的 `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`）。
/// 插件在 [`RelayUi::env_options`] 里声明可选项，用户勾选的 id 经 [`RelayConfig::env_options`] 回传。
#[derive(Debug, Clone, Copy, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct RelayEnvOption {
    /// 稳定 id，持久化在设置里；前端 i18n 按它取文案。
    pub id: &'static str,
    /// 英文兜底文案（前端没有对应 i18n 条目时用）。
    pub label: &'static str,
    /// 勾选后注入的（变量名， 值）。
    pub env: (&'static str, &'static str),
}

#[derive(Debug, Clone, Copy, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../../../src/generated/contracts/"))]
pub struct RelayUi {
    pub protocols: &'static [RelayOption],
    pub auth_modes: &'static [RelayOption],
    pub default_protocol: &'static str,
    pub default_auth: &'static str,
    pub suggestions: &'static [RelaySuggestionGroup],
    /// 可勾选的附加环境变量；无则空切片，前端不渲染这一区。
    pub env_options: &'static [RelayEnvOption],
}

#[derive(Debug, Clone, Copy)]
pub struct RelayConfig<'a> {
    pub base_url: &'a str,
    pub model: &'a str,
    pub protocol: &'a str,
    pub auth: &'a str,
    /// 用户勾选的附加环境变量选项 id（来自 [`RelayUi::env_options`]）。
    pub env_options: &'a [String],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayModelAuth {
    Bearer,
    ApiKey,
}

#[derive(Debug, Clone, Copy)]
pub struct RelayModelRequest {
    pub auth: RelayModelAuth,
    pub anthropic_version: bool,
}

pub trait RelayCap: Sync {
    fn ui(&self) -> RelayUi;

    /// 同一插件的旧变体可不支持中转（例如 legacy Kimi）。
    fn supports_variant(&self, _variant_tag: &str) -> bool {
        true
    }

    fn validate(&self, config: RelayConfig<'_>, variant_tag: &str) -> Result<(), String> {
        if !self.supports_variant(variant_tag) {
            return Err("当前安装版本不支持 API 中转".into());
        }
        let ui = self.ui();
        if !ui.protocols.is_empty()
            && !ui
                .protocols
                .iter()
                .any(|option| option.value == config.protocol)
        {
            return Err("中转接口协议不受支持".into());
        }
        if !ui
            .auth_modes
            .iter()
            .any(|option| option.value == config.auth)
        {
            return Err("中转凭据类型不受支持".into());
        }
        if config
            .env_options
            .iter()
            .any(|id| !ui.env_options.iter().any(|option| option.id == id))
        {
            return Err("中转环境变量选项不受支持".into());
        }
        Ok(())
    }

    fn launch_env(&self, config: RelayConfig<'_>, key: &str) -> Vec<(String, String)>;
    fn augment_argv(
        &self,
        config: RelayConfig<'_>,
        has_secret: bool,
        argv: Vec<String>,
    ) -> Vec<String>;
    fn model_request(&self, config: RelayConfig<'_>) -> RelayModelRequest;
}
