/// CLI 模块 —— 命令行界面定义与命令实现
///
/// 对齐 railpack `cmd/cli/main.go`
pub mod build;
pub mod common;
pub mod info;
pub mod plan;
pub mod prepare;
pub mod pretty_print;
pub mod schema;

pub mod frontend;

use clap::{ArgAction, Parser, Subcommand};

use self::info::InfoArgs;
use self::plan::PlanArgs;
use self::prepare::PrepareArgs;

/// arcpack — 零配置应用构建器
#[derive(Debug, Parser)]
#[command(name = "arcpack", version, about = "零配置应用构建器")]
pub struct Cli {
    /// 日志级别（-v = DEBUG, -vv = TRACE）
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count, global = true)]
    pub verbosity: u8,

    #[command(subcommand)]
    pub command: Commands,
}

/// 子命令定义
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// 输出 JSON 格式的 BuildPlan
    Plan(PlanArgs),

    /// 输出构建元信息（pretty / json）
    Info(InfoArgs),

    /// 输出 arcpack.json 的 JSON Schema
    Schema,

    /// 准备构建产物文件（plan + info JSON）
    Prepare(PrepareArgs),

    /// 执行构建（生成 OCI 镜像）
    Build(build::BuildArgs),

    /// BuildKit 前端模式（由 buildkitd 调用）
    Frontend(frontend::FrontendArgs),
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_cli_parse_plan_subcommand() {
        let cli = Cli::parse_from(["arcpack", "plan", "."]);
        assert!(matches!(cli.command, Commands::Plan(_)));
    }

    #[test]
    fn test_cli_parse_info_subcommand() {
        let cli = Cli::parse_from(["arcpack", "info", "."]);
        assert!(matches!(cli.command, Commands::Info(_)));
    }

    #[test]
    fn test_cli_parse_schema_subcommand() {
        let cli = Cli::parse_from(["arcpack", "schema"]);
        assert!(matches!(cli.command, Commands::Schema));
    }

    #[test]
    fn test_cli_parse_verbosity_flags() {
        let cli = Cli::parse_from(["arcpack", "-vv", "schema"]);
        assert_eq!(cli.verbosity, 2);
    }

    #[test]
    fn test_cli_parse_frontend_subcommand() {
        let cli = Cli::parse_from(["arcpack", "frontend"]);
        assert!(matches!(cli.command, Commands::Frontend(_)));
    }
}
