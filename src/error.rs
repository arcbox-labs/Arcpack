/// arcpack 统一错误类型
///
/// 所有错误分为三类：
/// - 用户错误（可修复，友好提示）
/// - 系统错误（需排查）
/// - 内部错误（框架自动转换）
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ArcpackError {
    // 用户错误
    #[error("配置解析失败: {path}: {message}")]
    ConfigParse { path: String, message: String },

    #[error("未识别项目类型，无匹配的 Provider")]
    NoProviderMatched,

    #[error("未知的 Provider: {name}")]
    UnknownProvider { name: String },

    #[error("未找到启动命令{}", help.as_ref().map(|h| format!(": {}", h)).unwrap_or_default())]
    NoStartCommand { help: Option<String> },

    #[error("源码路径不可访问: {path}")]
    SourceNotAccessible { path: String },

    // 系统错误
    #[error("buildkitd 启动失败: {message}")]
    DaemonStartFailed { message: String },

    #[error("buildkitd 启动超时（{timeout_secs}秒）")]
    DaemonTimeout { timeout_secs: u64 },

    #[error("构建失败（退出码 {exit_code}）: {stderr}")]
    BuildFailed { exit_code: i32, stderr: String },

    #[error("推送失败: {message}")]
    PushFailed { message: String },

    // 内部错误
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("序列化错误: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_parse_display_shows_path_and_message() {
        let err = ArcpackError::ConfigParse {
            path: "arcpack.json".to_string(),
            message: "unexpected token".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "配置解析失败: arcpack.json: unexpected token"
        );
    }

    #[test]
    fn test_no_provider_matched_display() {
        let err = ArcpackError::NoProviderMatched;
        assert_eq!(err.to_string(), "未识别项目类型，无匹配的 Provider");
    }

    #[test]
    fn test_no_start_command_display_with_help() {
        let err = ArcpackError::NoStartCommand {
            help: Some("请在 package.json 中添加 start 脚本".to_string()),
        };
        assert!(err.to_string().contains("请在 package.json"));
    }

    #[test]
    fn test_no_start_command_display_without_help() {
        let err = ArcpackError::NoStartCommand { help: None };
        assert_eq!(err.to_string(), "未找到启动命令");
    }

    #[test]
    fn test_daemon_timeout_display_shows_seconds() {
        let err = ArcpackError::DaemonTimeout { timeout_secs: 30 };
        assert!(err.to_string().contains("30"));
    }

    #[test]
    fn test_build_failed_display_shows_exit_code_and_stderr() {
        let err = ArcpackError::BuildFailed {
            exit_code: 1,
            stderr: "error: build failed".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("1"));
        assert!(msg.contains("error: build failed"));
    }

    #[test]
    fn test_unknown_provider_display() {
        let err = ArcpackError::UnknownProvider { name: "xxx".to_string() };
        assert_eq!(err.to_string(), "未知的 Provider: xxx");
    }

    #[test]
    fn test_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: ArcpackError = io_err.into();
        assert!(matches!(err, ArcpackError::Io(_)));
        assert!(err.to_string().contains("file not found"));
    }

    #[test]
    fn test_from_serde_error() {
        let serde_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err: ArcpackError = serde_err.into();
        assert!(matches!(err, ArcpackError::Serde(_)));
    }

    #[test]
    fn test_from_anyhow_error() {
        let anyhow_err = anyhow::anyhow!("something went wrong");
        let err: ArcpackError = anyhow_err.into();
        assert!(matches!(err, ArcpackError::Other(_)));
        assert!(err.to_string().contains("something went wrong"));
    }
}
