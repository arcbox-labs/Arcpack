/// Command 枚举 —— 构建步骤中的单条命令
///
/// 四种变体对齐 railpack 的 Command interface：
/// - Exec: 执行 shell 命令（对应 RUN）
/// - Copy: 复制文件（从镜像或本地上下文）
/// - Path: 添加 PATH 目录
/// - File: 创建文件（内容来自 Step.assets）
///
/// JSON 序列化使用 untagged 模式，按字段检测区分变体，与 railpack 一致。
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use super::spread::Spreadable;

/// 命令变体
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Command {
    Exec(ExecCommand),
    File(FileCommand),
    Path(PathCommand),
    Copy(CopyCommand),
}

/// 执行 shell 命令
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExecCommand {
    pub cmd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_name: Option<String>,
}

/// 添加 PATH 目录
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PathCommand {
    pub path: String,
}

/// 复制文件
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CopyCommand {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    pub src: String,
    pub dest: String,
}

/// 创建文件（内容来自 Step.assets 映射）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileCommand {
    pub path: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_name: Option<String>,
}

impl Command {
    /// 返回命令类型标识（对齐 railpack 的 CommandType()）
    pub fn command_type(&self) -> &str {
        match self {
            Command::Exec(_) => "exec",
            Command::Copy(_) => "copy",
            Command::Path(_) => "globalPath",
            Command::File(_) => "file",
        }
    }

    /// 创建 Exec 命令（原始命令，不包裹 sh -c）
    pub fn new_exec(cmd: impl Into<String>) -> Self {
        Command::Exec(ExecCommand {
            cmd: cmd.into(),
            custom_name: None,
        })
    }

    /// 创建 Exec 命令（包裹 sh -c）
    ///
    /// 对 cmd 中的单引号做 POSIX 转义（`'` → `'\''`），
    /// 避免命令包含单引号时破坏 `sh -c '...'` 的引用结构。
    pub fn new_exec_shell(cmd: impl Into<String>) -> Self {
        let cmd = cmd.into();
        let escaped = cmd.replace('\'', "'\\''");
        Command::Exec(ExecCommand {
            cmd: format!("sh -c '{}'", escaped),
            custom_name: None,
        })
    }

    /// 创建 Path 命令
    pub fn new_path(path: impl Into<String>) -> Self {
        Command::Path(PathCommand { path: path.into() })
    }

    /// 创建 Copy 命令
    pub fn new_copy(src: impl Into<String>, dest: impl Into<String>) -> Self {
        Command::Copy(CopyCommand {
            image: None,
            src: src.into(),
            dest: dest.into(),
        })
    }

    /// 创建 File 命令
    pub fn new_file(path: impl Into<String>, name: impl Into<String>) -> Self {
        Command::File(FileCommand {
            path: path.into(),
            name: name.into(),
            mode: None,
            custom_name: None,
        })
    }
}

/// 自定义反序列化：按字段检测区分变体（与 railpack 一致，无 type 标签）
///
/// 检测顺序：
/// 1. 有 cmd 字段 → Exec
/// 2. 有 path + name 字段 → File
/// 3. 只有 path 字段 → Path
/// 4. 有 src 字段 → Copy
impl<'de> Deserialize<'de> for Command {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        let obj = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("Command 必须是 JSON 对象"))?;

        // 有 cmd 字段 → Exec
        if obj.contains_key("cmd") {
            let exec: ExecCommand =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            return Ok(Command::Exec(exec));
        }

        // 有 path + name 字段 → File
        if obj.contains_key("path") && obj.contains_key("name") {
            let file: FileCommand =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            return Ok(Command::File(file));
        }

        // 只有 path 字段 → Path
        if obj.contains_key("path") {
            let path: PathCommand =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            return Ok(Command::Path(path));
        }

        // 有 src 字段 → Copy
        if obj.contains_key("src") {
            let copy: CopyCommand =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            return Ok(Command::Copy(copy));
        }

        Err(serde::de::Error::custom(
            "无法识别的 Command 类型：缺少 cmd/path/src 字段",
        ))
    }
}

/// Command 的 Spreadable 实现
/// 对齐 railpack: Exec 变体中 cmd == "..." 时视为展开占位符
impl Spreadable for Command {
    fn is_spread(&self) -> bool {
        match self {
            Command::Exec(exec) => exec.cmd == "...",
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_command_json_roundtrip() {
        let cmd = Command::new_exec("go build -o app .");
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, parsed);
        // 验证 JSON 格式无 type 标签
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("cmd").is_some());
        assert!(value.get("type").is_none());
    }

    #[test]
    fn test_exec_command_with_custom_name_json_roundtrip() {
        let cmd = Command::Exec(ExecCommand {
            cmd: "npm install".to_string(),
            custom_name: Some("安装依赖".to_string()),
        });
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, parsed);
    }

    #[test]
    fn test_copy_command_json_roundtrip() {
        let cmd = Command::new_copy("go.mod", "go.mod");
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, parsed);
    }

    #[test]
    fn test_copy_command_with_image_json_roundtrip() {
        let cmd = Command::Copy(CopyCommand {
            image: Some("golang:1.21".to_string()),
            src: "/usr/local/go".to_string(),
            dest: "/usr/local/go".to_string(),
        });
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, parsed);
    }

    #[test]
    fn test_path_command_json_roundtrip() {
        let cmd = Command::new_path("/usr/local/bin");
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, parsed);
    }

    #[test]
    fn test_file_command_json_roundtrip() {
        let cmd = Command::new_file("/etc/mise/config.toml", "mise.toml");
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, parsed);
    }

    #[test]
    fn test_file_command_with_mode_json_roundtrip() {
        let cmd = Command::File(FileCommand {
            path: "/entrypoint.sh".to_string(),
            name: "entrypoint".to_string(),
            mode: Some(0o755),
            custom_name: Some("创建入口脚本".to_string()),
        });
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, parsed);
    }

    #[test]
    fn test_command_type_returns_correct_values() {
        assert_eq!(Command::new_exec("echo hello").command_type(), "exec");
        assert_eq!(Command::new_copy("a", "b").command_type(), "copy");
        assert_eq!(Command::new_path("/bin").command_type(), "globalPath");
        assert_eq!(Command::new_file("/a", "b").command_type(), "file");
    }

    #[test]
    fn test_exec_shell_wraps_in_sh_c() {
        let cmd = Command::new_exec_shell("npm install");
        if let Command::Exec(exec) = &cmd {
            assert_eq!(exec.cmd, "sh -c 'npm install'");
        } else {
            panic!("应为 Exec 变体");
        }
    }

    #[test]
    fn test_exec_shell_escapes_single_quotes() {
        let cmd = Command::new_exec_shell("echo 'hello world'");
        if let Command::Exec(exec) = &cmd {
            assert_eq!(exec.cmd, "sh -c 'echo '\\''hello world'\\'''");
        } else {
            panic!("应为 Exec 变体");
        }
    }

    #[test]
    fn test_invalid_command_json_returns_error() {
        let result: std::result::Result<Command, _> = serde_json::from_str(r#"{"foo": "bar"}"#);
        assert!(result.is_err());
    }
}
