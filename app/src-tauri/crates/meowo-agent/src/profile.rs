//! 多账号（profile）：**一个 profile = 一个私有目录 + 启动该 agent 时注入的一组环境变量**。
//!
//! # 为什么是「目录隔离」而不是「轮换凭据」
//!
//! 直觉上更简单的做法是把选中账号的凭据写进 agent 真实的凭据位置（切换 = 换文件内容）。它有一个
//! **没法绕开**的冲突：agent 会用 refresh_token 换新 token 并**写回**凭据文件。你切到账号 B 之后，
//! 一个还在跑的账号 A 的会话刷新了 token，就会把 A 的凭据写回去——直接覆盖掉你刚切过去的 B。
//! 这不是理论风险，是 OAuth agent 的常规行为。
//!
//! 目录隔离没有这个问题：每个 profile 各写各的凭据与配置，谁也覆盖不了谁。Agent 自己通常也会
//! 把会话历史放进该目录；Meowo 对支持安全迁移的 provider（目前是 Claude）会在恢复前仅同步指定
//! session 的 transcript/file-history/tasks，使用户能用当前账号继续旧会话，同时绝不复制凭据。
//! 其他 provider 在有可靠的会话文件协议前仍保持各 profile 独立。
//!
//! # 默认 profile 不注入任何东西
//!
//! 「默认账号」就是 agent 自己的目录（`~/.claude`），**不注入环境变量**。于是现有用户零感知：
//! 不建新 profile，一切与从前一模一样。
//!
//! # 各家的隔离变量（全部实测）
//!
//! | agent | 变量 | 备注 |
//! |---|---|---|
//! | claude | `CLAUDE_CONFIG_DIR` | 一个变量搞定 |
//! | codex | `CODEX_HOME` | 同上 |
//! | kimi | `KIMI_SHARE_DIR` | 同上 |
//! | opencode | `OPENCODE_CONFIG_DIR` + `XDG_DATA_HOME` | **要两个**，见下 |
//! | gemini | —— | **不支持**：`GEMINI_DIR` 实测无效（设了照样读 `~/.gemini`） |
//!
//! opencode 需要两个变量，是因为它把**配置**与**数据**分了家：插件读配置目录
//! （`~/.config/opencode`），凭据却写数据目录（`~/.local/share/opencode/auth.json`）。只设一个的话，
//! 另一半仍然共用——账号根本没隔离开，而这种「看起来隔离了、其实没有」是最坏的一种失败。

use std::path::{Path, PathBuf};

/// **跨账号会话迁移**规格。声明它 = 该 agent 的会话可以搬到当前活跃账号下继续：
/// 恢复前把 session 级数据同步过去，因此恢复用的是**活跃账号**而非会话原先所属账号。
///
/// 不声明（默认）= 不支持，恢复沿用会话记录的原账号。这不是「还没做」而是安全默认：
/// 迁移要求该 agent 的会话数据可整体复制且换目录后仍能被它自己认出，未取证就搬运
/// 只会造出一个 agent 读不了的半份副本。
///
/// 字段描述的是「会话级数据散落在数据根的哪些地方」，宿主据此复制，不必知道任何
/// 具体 agent 的目录名。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossAccountSession {
    /// 会话正文所在的目录（相对数据根），其下按项目再分子目录，文件名为
    /// `<session-id>` + [`transcript_ext`](Self::transcript_ext)。
    ///
    /// **必须是单层目录名**（如 `"projects"`，不能是 `"data/sessions"`）：宿主从正文
    /// 路径上溯三级还原数据根（`<root>/<transcript_dir>/<项目>/<id><ext>`）。写成多层
    /// 会让数据根落在中间目录上——`session_buckets` 从错误位置复制，且「源与目标同根」
    /// 的短路判断永不成立，同账号内每次恢复都全量重拷一遍。
    pub transcript_dir: &'static str,
    /// 会话正文的扩展名（含点，如 `.jsonl`）。
    pub transcript_ext: &'static str,
    /// 其余按 session id 分目录保存的数据桶（相对数据根）：`<桶>/<session-id>/…`。
    pub session_buckets: &'static [&'static str],
    /// 子 agent 数据是否放在「与正文同名的同级目录」（`<项目>/<session-id>/`）。
    pub subagents_beside_transcript: bool,
}

/// 某 agent 的 profile 隔离规格。声明式，加/改 agent 只动 `plugins/`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileSpec {
    /// 启动该 agent 时注入的环境变量：`(变量名, 相对 profile 根的子路径)`。
    /// 空子路径 = profile 根本身。
    ///
    /// **首条必须指向承载 hooks 的那个目录**（即 [`data_rel`](Self::data_rel) 所指），
    /// 下方有绊线测试盯着这条约定。
    pub envs: &'static [(&'static str, &'static str)],
    /// 承载 hooks 的目录（＝该 agent 的 `data_dir`），相对 profile 根。
    pub data_rel: &'static str,
    /// 凭据文件，相对 profile 根。
    ///
    /// 刻意**不复用** [`crate::auth::CredentialSource`]：那个描述的是「默认安装」下凭据在哪
    /// （opencode 的是相对 home 的 `~/.local/share/opencode/auth.json`），而 profile 模式下整个
    /// 数据目录都被搬走了，那条路径不再成立。两者描述的是不同世界，硬要合并只会拼出错误的路径。
    pub creds_rel: &'static str,
}

impl ProfileSpec {
    /// 该 profile 要注入给 agent 进程的环境变量（绝对路径）。
    pub fn env_for(&self, root: &Path) -> Vec<(String, String)> {
        self.envs
            .iter()
            .map(|(key, rel)| {
                (
                    (*key).to_string(),
                    crate::join_rel(root, rel).to_string_lossy().into_owned(),
                )
            })
            .collect()
    }

    /// 该 profile 需要**预先建出**的目录——每个环境变量各指一处（opencode 是两处）。
    ///
    /// 得先于 agent 的第一次启动建好：接线要往里写 hooks，而 hooks 必须在会话开始前就位。
    pub fn dirs(&self, root: &Path) -> Vec<PathBuf> {
        self.envs
            .iter()
            .map(|(_, rel)| crate::join_rel(root, rel))
            .collect()
    }

    /// 该 profile 的数据目录（hooks 落在这里）。
    pub fn data_dir(&self, root: &Path) -> PathBuf {
        crate::join_rel(root, self.data_rel)
    }

    /// 该 profile 的凭据文件。
    pub fn credentials(&self, root: &Path) -> PathBuf {
        crate::join_rel(root, self.creds_rel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::all;

    fn root() -> PathBuf {
        PathBuf::from("/p/root")
    }

    /// 约定：`envs` 的**首条**必须指向 `data_rel`。接线走 data_dir，而 agent 进程按 envs 找目录——
    /// 两者一旦指向不同的地方，hooks 会被写进一个 agent 根本不读的目录里：接线「成功」了，
    /// 会话却永远不上板，且没有任何报错。
    #[test]
    fn first_env_must_point_at_the_data_dir() {
        for p in all() {
            let Some(spec) = p.profile() else { continue };
            let (_, first_rel) = spec.envs.first().unwrap_or_else(|| {
                panic!(
                    "{} 声明了 profile 却没有任何环境变量——那就隔离不了任何东西",
                    p.id()
                )
            });
            assert_eq!(
                *first_rel,
                spec.data_rel,
                "{} 的首个环境变量没指向 data_rel：hooks 会被写进 agent 不读的目录",
                p.id()
            );
        }
    }

    /// 凭据必须落在 profile 根**底下**——跑到外面去就等于没隔离（几个 profile 共用同一份凭据）。
    #[test]
    fn credentials_stay_inside_the_profile_root() {
        for p in all() {
            let Some(spec) = p.profile() else { continue };
            let creds = spec.credentials(&root());
            assert!(
                creds.starts_with(root()),
                "{} 的凭据跑到了 profile 根之外（{}）——那几个账号会共用同一份凭据",
                p.id(),
                creds.display()
            );
            assert!(
                !spec.creds_rel.is_empty(),
                "{} 的 creds_rel 为空：凭据路径会退化成 profile 根目录本身",
                p.id()
            );
        }
    }

    /// opencode 必须隔离**两个**目录。只设 `OPENCODE_CONFIG_DIR` 的话，凭据（在数据目录里）仍然
    /// 共用——「看起来隔离了、其实没有」是这里最坏的失败模式，故单独钉死。
    #[test]
    fn opencode_isolates_both_config_and_data() {
        let spec = crate::by_id("opencode")
            .and_then(|p| p.profile())
            .expect("opencode 支持多账号");
        let keys: Vec<&str> = spec.envs.iter().map(|(k, _)| *k).collect();
        assert!(
            keys.contains(&"OPENCODE_CONFIG_DIR"),
            "配置目录（插件）没隔离"
        );
        assert!(
            keys.contains(&"XDG_DATA_HOME"),
            "数据目录（凭据）没隔离——账号看起来切了，其实共用同一份 auth.json"
        );

        // 两者必须指向不同的子目录。
        let env = spec.env_for(&root());
        let cfg = &env
            .iter()
            .find(|(k, _)| k == "OPENCODE_CONFIG_DIR")
            .unwrap()
            .1;
        let data = &env.iter().find(|(k, _)| k == "XDG_DATA_HOME").unwrap().1;
        assert_ne!(cfg, data);

        // 凭据落在 XDG_DATA_HOME/opencode/auth.json —— opencode 自己就是这么拼的。
        assert_eq!(
            spec.credentials(&root()),
            crate::join_rel(&root(), "data/opencode/auth.json")
        );
    }

    /// gemini 不支持多账号——`GEMINI_DIR` 实测无效（设了它，gemini 照样读 `~/.gemini`）。
    /// 谎称支持的后果是：切了账号，两个 profile 却仍在共用同一份凭据。
    #[test]
    fn gemini_declares_no_profile_support() {
        assert!(
            crate::by_id("gemini").unwrap().profile().is_none(),
            "gemini 的数据目录不可被环境变量覆盖，不能谎称支持多账号"
        );
    }

    #[test]
    fn env_for_resolves_absolute_paths_under_the_root() {
        let spec = crate::by_id("claude").and_then(|p| p.profile()).unwrap();
        let env = spec.env_for(&root());
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, "CLAUDE_CONFIG_DIR");
        // 空子路径 → profile 根本身。
        assert_eq!(PathBuf::from(&env[0].1), root());
        assert_eq!(spec.data_dir(&root()), root());
    }
}
