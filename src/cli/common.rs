/// 公共 CLI 参数和辅助函数
///
/// 对齐 railpack `cli/common.go`

use std::collections::HashMap;
use std::path::Path;

use crate::{BuildResult, GenerateBuildPlanOptions};

/// 公共构建参数（plan / info / build / prepare 共用）
///
/// 对齐 railpack `commonPlanFlags()`
#[derive(Debug, clap::Args)]
pub struct CommonBuildArgs {
    /// 源码目录路径
    #[arg(default_value = ".")]
    pub directory: String,

    /// 环境变量（可多次使用，格式 KEY=VALUE）
    #[arg(short = 'e', long = "env")]
    pub env: Vec<String>,

    /// 上次构建的包版本（格式 package@version）
    #[arg(long = "previous")]
    pub previous: Vec<String>,

    /// 覆盖构建命令
    #[arg(long = "build-cmd")]
    pub build_cmd: Option<String>,

    /// 覆盖启动命令
    #[arg(long = "start-cmd")]
    pub start_cmd: Option<String>,

    /// 配置文件相对路径（默认 arcpack.json）
    #[arg(long = "config-file")]
    pub config_file: Option<String>,

    /// 禁用启动命令缺失检查（默认启用检查）
    #[arg(long = "no-error-missing-start")]
    pub no_error_missing_start: bool,
}

/// 从 CLI 参数生成 BuildResult
///
/// 对齐 railpack `GenerateBuildResultForCommand()`
pub fn generate_build_result_for_command(args: &CommonBuildArgs) -> crate::Result<BuildResult> {
    let env_vars = parse_env_vars(&args.env)?;
    let previous_versions = parse_previous_versions(&args.previous)?;

    let options = GenerateBuildPlanOptions {
        build_command: args.build_cmd.clone(),
        start_command: args.start_cmd.clone(),
        previous_versions,
        config_file_path: args.config_file.clone(),
        error_missing_start_command: !args.no_error_missing_start,
    };

    crate::generate_build_plan(&args.directory, env_vars, &options)
}

/// 初始化 tracing-subscriber
///
/// 0=WARN, 1=DEBUG, 2=TRACE，输出到 stderr
pub fn init_tracing(verbosity: u8) {
    use tracing_subscriber::EnvFilter;

    let level = match verbosity {
        0 => "warn",
        1 => "debug",
        _ => "trace",
    };

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(supports_ansi())
        .init();
}

/// 检测终端是否支持 ANSI 颜色
fn supports_ansi() -> bool {
    // NO_COLOR 环境变量存在则禁用
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }
    // FORCE_COLOR 环境变量存在且非 falsy 值则强制启用
    if let Ok(val) = std::env::var("FORCE_COLOR") {
        if val != "0" && val != "false" {
            return true;
        }
    }
    // 默认检测 stderr 是否为终端
    use std::io::IsTerminal;
    std::io::stderr().is_terminal()
}

/// 向 plan JSON 注入 `$schema` 字段
///
/// 对齐 railpack `addSchemaToPlanMap()`
pub fn add_schema_to_plan_json(plan: &crate::plan::BuildPlan) -> crate::Result<serde_json::Value> {
    let mut value = serde_json::to_value(plan)?;
    if let Some(obj) = value.as_object_mut() {
        // serde_json::Map 底层为 BTreeMap，'$'(0x24) 天然排在所有字母前
        obj.insert(
            "$schema".to_string(),
            serde_json::Value::String(crate::config::SCHEMA_URL.to_string()),
        );
    }
    Ok(value)
}

/// 将 JSON 值写入文件（2 空格缩进 + 换行）
///
/// 对齐 railpack `writeJSONFile()`
pub fn write_json_file(path: &str, value: &serde_json::Value, log_msg: &str) -> crate::Result<()> {
    let json = serde_json::to_string_pretty(value)?;
    write_text_file(path, &format!("{}\n", json), log_msg)
}

/// 将文本写入文件
pub fn write_text_file(path: &str, content: &str, log_msg: &str) -> crate::Result<()> {
    let path = Path::new(path);

    // 创建父目录
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(path, content)?;

    tracing::debug!("{}: {}", log_msg, path.display());
    Ok(())
}

/// 解析 --env KEY=VALUE 参数
pub fn parse_env_vars(env_args: &[String]) -> crate::Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    for arg in env_args {
        if let Some((key, value)) = arg.split_once('=') {
            map.insert(key.to_string(), value.to_string());
        } else {
            return Err(anyhow::anyhow!("invalid --env '{}', expected KEY=VALUE", arg).into());
        }
    }
    Ok(map)
}

/// 解析 --previous package@version 参数
fn parse_previous_versions(previous_args: &[String]) -> crate::Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    for arg in previous_args {
        if let Some((pkg, version)) = arg.split_once('@') {
            map.insert(pkg.to_string(), version.to_string());
        } else {
            return Err(anyhow::anyhow!(
                "invalid --previous '{}', expected package@version",
                arg
            )
            .into());
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    // parse_env_vars 测试
    #[test]
    fn test_parse_env_vars_valid() {
        let args = vec!["FOO=bar".to_string(), "BAZ=qux".to_string()];
        let map = parse_env_vars(&args).unwrap();
        assert_eq!(map.get("FOO"), Some(&"bar".to_string()));
        assert_eq!(map.get("BAZ"), Some(&"qux".to_string()));
    }

    #[test]
    fn test_parse_env_vars_empty() {
        let args: Vec<String> = vec![];
        let map = parse_env_vars(&args).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn test_parse_env_vars_value_with_equals() {
        let args = vec!["KEY=val=ue".to_string()];
        let map = parse_env_vars(&args).unwrap();
        assert_eq!(map.get("KEY"), Some(&"val=ue".to_string()));
    }

    #[test]
    fn test_parse_env_vars_no_equals_returns_error() {
        let args = vec!["INVALID".to_string()];
        let result = parse_env_vars(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("KEY=VALUE"));
    }

    // parse_previous_versions 测试
    #[test]
    fn test_parse_previous_versions_valid() {
        let args = vec!["node@20.0.0".to_string(), "pnpm@9.0.0".to_string()];
        let map = parse_previous_versions(&args).unwrap();
        assert_eq!(map.get("node"), Some(&"20.0.0".to_string()));
        assert_eq!(map.get("pnpm"), Some(&"9.0.0".to_string()));
    }

    #[test]
    fn test_parse_previous_versions_empty() {
        let args: Vec<String> = vec![];
        let map = parse_previous_versions(&args).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn test_parse_previous_versions_no_at_returns_error() {
        let args = vec!["invalid".to_string()];
        let result = parse_previous_versions(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("package@version"));
    }

    #[test]
    fn test_parse_previous_versions_version_with_at() {
        let args = vec!["pkg@1.0@beta".to_string()];
        let map = parse_previous_versions(&args).unwrap();
        assert_eq!(map.get("pkg"), Some(&"1.0@beta".to_string()));
    }

    // add_schema_to_plan_json 测试
    #[test]
    fn test_add_schema_to_plan_json_contains_schema() {
        let plan = crate::plan::BuildPlan::new();
        let value = add_schema_to_plan_json(&plan).unwrap();
        assert!(value.get("$schema").is_some());
        assert_eq!(
            value["$schema"].as_str().unwrap(),
            crate::config::SCHEMA_URL
        );
    }

    // error_missing_start 默认值测试
    #[test]
    fn test_no_error_missing_start_defaults_to_false() {
        use clap::Parser;
        // 不传 --no-error-missing-start 时，默认 false → error_missing_start_command = true
        let cli = crate::cli::Cli::parse_from(["arcpack", "plan", "."]);
        if let crate::cli::Commands::Plan(args) = cli.command {
            assert!(!args.common.no_error_missing_start);
        }
    }
}
