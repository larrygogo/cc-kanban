//! 变体（Variant）：同一 agent 的一个版本形态。数据目录 / hooks 规格 / 鉴权 / 可执行位置的差异
//! 全部收敛在这一张声明表里；`probe` 命中后产出 [`Installation`]——「这台机器上该 agent 的实况」。

use std::path::{Path, PathBuf};

use crate::{auth::AuthScheme, config::HookSpec, id::AgentId, launch::LaunchSpec};

/// 数据目录的解析规则。
#[derive(Debug, Clone, Copy)]
pub struct DataDirSpec {
    /// 环境变量覆盖名（如 `KIMI_SHARE_DIR` / `CODEX_HOME` / `CLAUDE_CONFIG_DIR`）。
    /// 三家语义一致：变量值**就是**数据目录本身。设了就优先于候选目录。
    pub env: Option<&'static str>,
    /// 相对 home 的候选目录，按优先级排列。`probe` 取第一个**已存在**的。
    pub candidates: &'static [&'static str],
}

impl DataDirSpec {
    /// env 覆盖值（非空即取，不校验存在）。
    fn env_override(&self) -> Option<PathBuf> {
        let key = self.env?;
        let v = std::env::var(key).ok()?;
        (!v.is_empty()).then(|| PathBuf::from(v))
    }

    /// **已存在**的数据目录：env 覆盖（须是目录）→ 首个存在的候选 → None（＝该变体未配置过）。
    pub fn probe(&self, home: &Path) -> Option<PathBuf> {
        self.probe_tagged(home).map(|(d, _)| d)
    }

    /// 同 [`probe`](Self::probe)，并告知命中的是否为 env 覆盖。env 指向的目录**无法**判定属于哪个变体
    /// （没有形态信号），故命中它的变体会把 tag 改成 `"env-override"`，而非谎报成自己的 tag。
    fn probe_tagged(&self, home: &Path) -> Option<(PathBuf, bool)> {
        if let Some(d) = self.env_override() {
            return d.is_dir().then_some((d, true));
        }
        self.candidates
            .iter()
            .map(|c| home.join(c))
            .find(|p| p.is_dir())
            .map(|d| (d, false))
    }

    /// 全新安装**应当**写入的位置：env 覆盖 → 首个候选。不要求存在。
    /// 所有变体都 probe 不中时，由 agent 的首选变体给出这个默认，供「用之前」的路径展示与写入。
    pub fn default_dir(&self, home: &Path) -> Option<PathBuf> {
        self.env_override()
            .or_else(|| self.candidates.first().map(|c| home.join(c)))
    }
}

/// 环境变量指定数据目录时的变体标签。此时用的是首个变体的规则——env 目录的版本形态无从判定，
/// 故不谎报成 `"modern"`/`"legacy"`，日志里如实标出。
pub const ENV_OVERRIDE_TAG: &str = "env-override";

/// 同一 agent 的一个版本形态——所有版本差异收敛于此。声明为 `const`，进 `&'static [Variant]`。
#[derive(Debug, Clone, Copy)]
pub struct Variant {
    /// 变体标识，仅用于日志/诊断（如 `"modern"` / `"legacy"`）。
    pub tag: &'static str,
    pub data_dir: DataDirSpec,
    pub hooks: &'static HookSpec,
    /// 无鉴权概念的 agent 为 None。
    pub auth: Option<&'static AuthScheme>,
    pub launch: &'static LaunchSpec,
    /// 工作区信任名册（起进程前可预写，免掉「是否信任此目录」确认屏，见 [`crate::trust`]）。
    /// 无此机制的变体为 None。
    pub trust: Option<&'static crate::trust::WorkspaceTrustSpec>,
}

impl Variant {
    /// 该变体在本机是否配置过（数据目录存在）；命中则产出实况。
    pub fn probe(&self, id: AgentId, home: &Path) -> Option<Installation> {
        let (data_dir, from_env) = self.data_dir.probe_tagged(home)?;
        let mut inst = self.installation_at(id, data_dir, Some(home));
        if from_env {
            inst.variant_tag = ENV_OVERRIDE_TAG;
        }
        Some(inst)
    }

    /// 以给定 data_dir 构造实况（跳过目录存在性判定）。供 `probe` 与「未配置时的默认位置」共用。
    pub(crate) fn installation_at(
        &self,
        id: AgentId,
        data_dir: PathBuf,
        home: Option<&Path>,
    ) -> Installation {
        let launch = self.launch.probe(Some(&data_dir), home);
        Installation {
            id,
            variant_tag: self.tag,
            data_dir,
            hooks: self.hooks,
            auth: self.auth,
            launch,
            launch_stem: self.launch.stem,
            trust: self.trust,
            // 默认账号。profile 实况由 `installation_for_profile` 事后填上——可执行的探测与
            // profile 无关（换账号不换二进制），故这里共用同一条 probe 路径。
            profile: None,
        }
    }
}

/// probe 命中后的运行时事实。setup / account / reporter 三条链路都以它为唯一输入，
/// 不再各自重推「这个 agent 的目录到底是哪个」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    pub id: AgentId,
    pub variant_tag: &'static str,
    pub data_dir: PathBuf,
    pub hooks: &'static HookSpec,
    pub auth: Option<&'static AuthScheme>,
    /// 启动 argv（`["<exe>"]` 或 `["node", "<js>"]`）；None = 候选位置都没找到。
    pub launch: Option<Vec<String>>,
    /// 回退裸名（无扩展名）。
    pub launch_stem: &'static str,
    /// 该变体的工作区信任名册形态。None = 该 agent 没有这道确认屏。
    pub trust: Option<&'static crate::trust::WorkspaceTrustSpec>,
    /// 这份实况属于哪个 **profile**（多账号）：`(profile 根目录, 隔离规格)`。
    /// `None` = 默认账号，即 agent 自己的目录。由 [`AgentPlugin::installation_for_profile`] 填。
    pub profile: Option<(PathBuf, &'static crate::profile::ProfileSpec)>,
}

impl Installation {
    /// 承载 hooks 的配置文件路径。
    pub fn config_path(&self) -> PathBuf {
        crate::join_rel(&self.data_dir, self.hooks.config_rel)
    }

    /// 凭据文件路径（该变体无鉴权则 None）。macOS Keychain 变体返回的是其文件回退路径。
    ///
    /// 三种情形，**这是凭据路径的唯一出口**，别处不要自行拼接：
    ///
    /// 1. **profile（多账号）**：一切都在 profile 根底下——凭据也不例外。默认路径在这里全不作数：
    ///    opencode 的凭据默认在 `~/.local/share/opencode`，而 profile 模式下整个数据目录都被
    ///    `XDG_DATA_HOME` 搬走了，再按 home 去拼就会指向一个属于**默认账号**的文件。
    /// 2. **凭据不在 `data_dir` 底下**（[`CredentialSource::HomeFile`]）：相对 home 解析。
    /// 3. 其余：相对 `data_dir`。
    pub fn credentials_path(&self) -> Option<PathBuf> {
        self.auth?;
        if let Some((root, spec)) = &self.profile {
            return Some(spec.credentials(root));
        }
        let auth = self.auth?;
        match auth.credentials {
            crate::auth::CredentialSource::HomeFile(rel) => {
                Some(crate::join_rel(&crate::home_dir()?, rel))
            }
            _ => Some(crate::join_rel(&self.data_dir, auth.credentials.file_rel())),
        }
    }

    /// 启动该 agent 时要注入的环境变量。默认账号 → 空（什么都不注入，现有用户零感知）。
    pub fn profile_env(&self) -> Vec<(String, String)> {
        match &self.profile {
            Some((root, spec)) => spec.env_for(root),
            None => Vec::new(),
        }
    }

    /// 启动 argv：绝对路径优先，找不到则回退裸名交给 PATH 解析。
    pub fn launch_argv(&self) -> Vec<String> {
        self.launch
            .clone()
            .unwrap_or_else(|| vec![self.launch_stem.to_string()])
    }

    /// 拉起交互式登录的 argv = 启动 argv + 该变体声明的登录子命令（如 `<claude.exe> auth login`）。
    /// 该变体无鉴权、或未声明登录入口（`login: None`）→ None。与 `launch_argv` 同源，故同样是
    /// 绝对路径优先。
    ///
    /// **空子命令是合法的**：`login: Some(&[])` 表示「裸启动即登录」——gemini 没有登录子命令，
    /// 跑 `gemini` 本身就会引导认证。此前空切片被当成「无入口」而返回 None，gemini 因此无从表达，
    /// 前端的登录按钮点下去只能得到一句「拉起登录失败」。
    pub fn login_argv(&self) -> Option<Vec<String>> {
        let args = self.auth?.login?;
        let mut argv = self.launch_argv();
        argv.extend(args.iter().map(|s| s.to_string()));
        Some(argv)
    }

    /// 非交互式退出登录 argv。CLI 未声明登出入口时返回 None，由宿主按凭据来源安全清理。
    pub fn logout_argv(&self) -> Option<Vec<String>> {
        let args = self.auth?.logout_args;
        if args.is_empty() {
            return None;
        }
        let mut argv = self.launch_argv();
        argv.extend(args.iter().map(|s| s.to_string()));
        Some(argv)
    }

    /// **可执行装了吗**——能启动/恢复会话。与 [`is_configured`](Self::is_configured) 是两回事：
    /// 卡片上「已安装」与「未检测到数据目录」曾同时出现，正是这两者被混用。
    pub fn is_launchable(&self) -> bool {
        self.launch.is_some()
    }

    /// **用过/配置过吗**——数据目录存在，才谈得上接线、读会话。
    pub fn is_configured(&self) -> bool {
        self.data_dir.is_dir()
    }

    /// 起进程前把 `cwd` 预写进该实况的工作区信任名册。true = 记录已就位（本次写入或早已存在）。
    ///
    /// 全程 best-effort：未声明该机制、数据目录不存在、写失败，一律只打日志返回 false——最坏退化成
    /// 用户在终端页手动信任一次，与没有此功能时等价，绝不因预信任失败挡住会话。
    ///
    /// 门槛是数据目录已存在（与 `wire` 同一条纪律：**绝不凭空创建 agent 的数据目录**）。调用方必须
    /// 按本次实际生效的账号给实况——kimi 的多账号靠 `KIMI_SHARE_DIR` 搬走整个数据目录，写进默认
    /// 目录而会话跑在隔离账号上，等于没写。
    pub fn pretrust_workspace(&self, cwd: &str) -> bool {
        let Some(spec) = self.trust else {
            return false;
        };
        if !self.is_configured() {
            eprintln!(
                "Meowo trust[{}]: 数据目录不存在，跳过预信任 {cwd}",
                self.id.as_str()
            );
            return false;
        }
        match spec.pretrust(&self.data_dir, cwd) {
            Ok(_) => true,
            Err(e) => {
                eprintln!(
                    "Meowo trust[{}]: 预写工作区信任失败（{cwd}）: {e}",
                    self.id.as_str()
                );
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthScheme, CredentialSource};
    use crate::config::{CommandSpec, ConfigFormat, HookEvent, MissingConfig, RepairReason};
    use crate::launch::{LaunchCandidate, LaunchSpec, Root};

    static EVENTS: [HookEvent; 1] = [HookEvent::plain("SessionStart")];
    static SPEC: HookSpec = HookSpec {
        config_rel: "config.toml",
        format: ConfigFormat::KimiToml,
        missing: MissingConfig::Fail(RepairReason::NeedLogin),
        events: &EVENTS,
        command: CommandSpec {
            quote_exe: false,
            with_provider: true,
            ps_call_operator: false,
        },
    };
    static CANDS: [LaunchCandidate; 1] = [LaunchCandidate::Exe {
        root: Root::DataDir,
        sub: "bin",
    }];
    static LAUNCH: LaunchSpec = LaunchSpec {
        stem: "x",
        candidates: &CANDS,
    };

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("meowo-variant-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn variant(env: Option<&'static str>, candidates: &'static [&'static str]) -> Variant {
        Variant {
            tag: "modern",
            data_dir: DataDirSpec { env, candidates },
            hooks: &SPEC,
            auth: None,
            launch: &LAUNCH,
            trust: None,
        }
    }

    /// 有登录入口的鉴权声明。
    static AUTH_WITH_LOGIN: AuthScheme = AuthScheme {
        credentials: CredentialSource::File("cred.json"),
        refresh: None,
        default_base_url: "",
        login: Some(&["auth", "login"]),
        logout_args: &["auth", "logout"],
    };
    /// 有鉴权但**无**登录入口（如凭据全由外部工具写入）。
    static AUTH_NO_LOGIN: AuthScheme = AuthScheme {
        credentials: CredentialSource::File("cred.json"),
        refresh: None,
        default_base_url: "",
        login: None,
        logout_args: &[],
    };
    /// **裸启动即登录**（gemini：没有登录子命令，跑它自己就会引导认证）。
    static AUTH_LOGIN_ON_LAUNCH: AuthScheme = AuthScheme {
        credentials: CredentialSource::File("cred.json"),
        refresh: None,
        default_base_url: "",
        login: Some(&[]),
        logout_args: &[],
    };
    /// 凭据不在 data_dir 底下（opencode：插件在配置目录，凭据在数据目录）。
    static AUTH_HOME_CREDS: AuthScheme = AuthScheme {
        credentials: CredentialSource::HomeFile(".local/share/x/auth.json"),
        refresh: None,
        default_base_url: "",
        login: Some(&["auth", "login"]),
        logout_args: &[],
    };

    fn inst_with_auth(auth: Option<&'static AuthScheme>) -> Installation {
        let v = Variant {
            tag: "t",
            data_dir: DataDirSpec {
                env: None,
                candidates: &[".x"],
            },
            hooks: &SPEC,
            auth,
            launch: &LAUNCH,
            trust: None,
        };
        // 不传 home：launch 候选（DataDir/bin）不会命中，launch_argv 回退裸名 "x"。
        v.installation_at(crate::id::KIMI, PathBuf::from("/nowhere"), None)
    }

    #[test]
    fn login_argv_appends_declared_subcommand_to_launch_argv() {
        let inst = inst_with_auth(Some(&AUTH_WITH_LOGIN));
        assert_eq!(inst.launch_argv(), vec!["x".to_string()]);
        assert_eq!(
            inst.login_argv(),
            Some(vec![
                "x".to_string(),
                "auth".to_string(),
                "login".to_string()
            ])
        );
    }

    #[test]
    fn login_argv_is_none_only_when_no_login_entry_is_declared() {
        // 无鉴权声明 → 无登录入口。
        assert_eq!(inst_with_auth(None).login_argv(), None);
        // 声明了鉴权但明确没有登录入口（login: None）→ 同样 None。
        assert_eq!(inst_with_auth(Some(&AUTH_NO_LOGIN)).login_argv(), None);
    }

    /// **裸启动即登录**必须能表达出来。这条曾经是反的：空子命令被当成「无登录入口」而返回 None
    /// （注释还写着「不能拉起一个只有可执行名的终端」）。gemini 恰恰就是这种 agent——它没有登录
    /// 子命令，跑 `gemini` 本身就会引导认证。于是它被判成没有入口，而前端仍旧亮出登录按钮，
    /// 点下去只得到一句「拉起登录失败」。
    ///
    /// `login: None`（真的没有入口）与 `login: Some(&[])`（入口就是启动它自己）必须泾渭分明。
    #[test]
    fn login_argv_supports_bare_launch_as_the_login_flow() {
        let inst = inst_with_auth(Some(&AUTH_LOGIN_ON_LAUNCH));
        // 登录 argv 就是启动 argv 本身，不多一个参数。
        assert_eq!(inst.login_argv(), Some(vec!["x".to_string()]));
        assert_eq!(inst.login_argv(), Some(inst.launch_argv()));
    }

    /// 凭据可以不在 data_dir 底下（opencode：插件落配置目录，凭据落数据目录）。
    /// 相对 data_dir 解析会把它拼到一个根本不存在的路径上，登录态因此永远读不出来。
    #[test]
    fn credentials_may_live_outside_the_data_dir() {
        let _env = crate::env_guard();
        let home = std::env::temp_dir().join(format!("meowo-homecred-{}", std::process::id()));
        std::env::set_var("USERPROFILE", &home);
        std::env::set_var("HOME", &home);

        let inst = inst_with_auth(Some(&AUTH_HOME_CREDS));
        // 相对 home，而不是相对 data_dir（/nowhere）。
        assert_eq!(
            inst.credentials_path(),
            Some(crate::join_rel(&home, ".local/share/x/auth.json"))
        );

        // 对照：普通的 File 仍相对 data_dir。
        let inst = inst_with_auth(Some(&AUTH_WITH_LOGIN));
        assert_eq!(
            inst.credentials_path(),
            Some(PathBuf::from("/nowhere").join("cred.json"))
        );
    }

    #[test]
    fn logout_argv_uses_declared_noninteractive_subcommand() {
        assert_eq!(
            inst_with_auth(Some(&AUTH_WITH_LOGIN)).logout_argv(),
            Some(vec![
                "x".to_string(),
                "auth".to_string(),
                "logout".to_string()
            ])
        );
        assert_eq!(inst_with_auth(Some(&AUTH_NO_LOGIN)).logout_argv(), None);
    }

    #[test]
    fn data_dir_probe_takes_first_existing_candidate() {
        let home = tmp("probe");
        let spec = DataDirSpec {
            env: None,
            candidates: &[".modern", ".legacy"],
        };

        // 都不存在 → probe None，但 default_dir 给首选（供全新安装写入）。
        assert_eq!(spec.probe(&home), None);
        assert_eq!(spec.default_dir(&home), Some(home.join(".modern")));

        // 只有旧版存在 → 命中旧版。
        std::fs::create_dir_all(home.join(".legacy")).unwrap();
        assert_eq!(spec.probe(&home), Some(home.join(".legacy")));

        // 两者都在 → 首选优先。
        std::fs::create_dir_all(home.join(".modern")).unwrap();
        assert_eq!(spec.probe(&home), Some(home.join(".modern")));

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn env_override_is_tagged_honestly_not_as_first_variant() {
        // env 覆盖指向的目录无形态信号：不能因为它命中了首个变体就报成 "modern"。
        let home = tmp("envtag");
        let target = home.join("some-copy");
        std::fs::create_dir_all(&target).unwrap();
        let key = "MEOWO_TEST_DATA_DIR";
        std::env::set_var(key, &target);

        let v = variant(Some(key), &[".modern"]);
        let inst = v
            .probe(AgentId::new("t"), &home)
            .expect("env 目录存在应命中");
        assert_eq!(inst.variant_tag, ENV_OVERRIDE_TAG);
        assert_eq!(inst.data_dir, target);

        // env 指向不存在的目录 → 不命中（detect 视为未配置），由 default_dir 兜底。
        std::env::set_var(key, home.join("nope"));
        assert!(v.probe(AgentId::new("t"), &home).is_none());
        assert_eq!(v.data_dir.default_dir(&home), Some(home.join("nope")));

        std::env::remove_var(key);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn launchable_and_configured_are_independent() {
        // A4：「可执行装了」与「数据目录存在」是两个正交事实，绝不能混为「已安装」一个字段。
        let home = tmp("split");
        let data = home.join(".modern");
        std::fs::create_dir_all(&data).unwrap();
        let v = variant(None, &[".modern"]);

        // 配置过、但可执行没找到 → 回退裸名 argv。
        let inst = v.probe(AgentId::new("t"), &home).unwrap();
        assert!(inst.is_configured());
        assert!(!inst.is_launchable());
        assert_eq!(inst.launch_argv(), vec!["x".to_string()]);
        assert_eq!(inst.config_path(), data.join("config.toml"));

        // 可执行出现 → 两者都真。
        let bin = data.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(crate::exe_file_name("x")), b"").unwrap();
        let inst = v.probe(AgentId::new("t"), &home).unwrap();
        assert!(inst.is_launchable() && inst.is_configured());
        assert_eq!(inst.launch_argv().len(), 1);
        assert!(inst.launch_argv()[0].contains("bin"));

        let _ = std::fs::remove_dir_all(&home);
    }
}
