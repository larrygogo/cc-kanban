//! 安装器写下的功能开关种子（Windows only）。
//!
//! NSIS 一键安装器的「自定义安装」里有一颗「对话窗口功能」勾选（默认勾），结果写在
//! `HKCU\Software\larrygogo\Meowo` 的 `ChatEnabled` (REG_DWORD 0/1)。**只有 GUI 交互
//! 安装会写**——updater 静默升级（/P /UPDATE）与 /S 静默装都不写，存量用户的偏好
//! 不会被安装器动到（见 nsis/installer.nsi 的 chat-enabled-seed 块）。
//!
//! 合并规则：settings.json 里 `chat_enabled` 字段**在场**即代表用户已做过显式选择
//! （设置页开关落盘，或上一次种子已合并），种子被忽略；缺席才采纳并落盘。无论采纳
//! 与否读后即焚——种子终身只生效一次，卸载器另有兜底清理（装完从未启动的情况）。
//!
//! 线程纪律说明：本函数在 setup 闭包最前部同步执行——一次注册表读 + 至多一次小 JSON
//! 写，微秒~毫秒级，此刻事件循环尚未起跑、没有可冻结的消息泵，不适用「文件/注册表
//! 必须 spawn_blocking」的命令线程纪律。

pub(crate) fn consume_installer_seed() {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
    use winreg::RegKey;

    // 与 NSIS 的 ${MANUPRODUCTKEY}（Software\<manufacturer>\<productName>）保持一致。
    const SEED_KEY: &str = r"Software\larrygogo\Meowo";
    const SEED_VALUE: &str = "ChatEnabled";

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    // 读+删要同一把打开的键；键不存在（非安装器分发 / 种子已焚）是常态，静默返回。
    let Ok(key) = hkcu.open_subkey_with_flags(SEED_KEY, KEY_READ | KEY_SET_VALUE) else {
        return;
    };
    let Ok(seed) = key.get_value::<u32, _>(SEED_VALUE) else {
        return;
    };
    crate::settings::adopt_chat_enabled_seed(seed != 0);
    // 读后即焚：即便合并失败（磁盘只读等）也删——下次启动按默认走，好过每次启动
    // 都被一个陈旧种子改写。
    let _ = key.delete_value(SEED_VALUE);
}
