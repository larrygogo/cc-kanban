#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// 调用方传入的参数违反数据完整性约束（如接续链自环/分叉）。
    /// 与 Sqlite 分开：这类错误重试无意义，上层应把 message 原样透给用户。
    #[error("{0}")]
    InvalidInput(String),
}
