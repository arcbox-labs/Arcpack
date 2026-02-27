/// prepare 命令 —— 准备构建产物文件
///
/// 对齐 railpack `cli/prepare.go`

use super::common::{
    CommonBuildArgs, generate_build_result_for_command, add_schema_to_plan_json, write_json_file,
};
use super::pretty_print::{PrintOptions, OutputStream, pretty_print_build_result};

/// Prepare 命令参数
#[derive(Debug, clap::Args)]
pub struct PrepareArgs {
    #[command(flatten)]
    pub common: CommonBuildArgs,

    /// plan JSON 输出文件路径
    #[arg(long = "plan-out")]
    pub plan_out: Option<String>,

    /// info JSON 输出文件路径（plan 字段置空）
    #[arg(long = "info-out")]
    pub info_out: Option<String>,

    /// 将 plan JSON 输出到 stdout
    #[arg(long = "show-plan")]
    pub show_plan: bool,

    /// 隐藏 pretty print 输出
    #[arg(long = "hide-pretty-plan")]
    pub hide_pretty_plan: bool,
}

/// 执行 prepare 命令
///
/// 返回 result.success，调用方据此决定退出码
pub fn run_prepare(args: &PrepareArgs) -> crate::Result<bool> {
    let mut result = generate_build_result_for_command(&args.common)?;
    let success = result.success;

    // pretty print 到 stderr（除非隐藏）
    if !args.hide_pretty_plan {
        let options = PrintOptions {
            metadata: true,
            version: result.arcpack_version.clone(),
            stream: OutputStream::Stderr,
        };
        pretty_print_build_result(&result, &options);
    }

    // 统一计算 plan JSON（避免 --show-plan 和 --plan-out 重复调用）
    let plan_json = if args.show_plan || args.plan_out.is_some() {
        result.plan.as_ref().map(add_schema_to_plan_json).transpose()?
    } else {
        None
    };

    // --show-plan：plan JSON 到 stdout
    if args.show_plan {
        if let Some(ref value) = plan_json {
            let json = serde_json::to_string_pretty(value)?;
            println!("{}", json);
        }
    }

    // --plan-out：plan JSON 写文件（含 $schema）
    if let Some(ref path) = args.plan_out {
        if let Some(ref value) = plan_json {
            write_json_file(path, value, "plan written to")?;
        }
    }

    // --info-out：BuildResult JSON 写文件（plan 置为 null）
    if let Some(ref path) = args.info_out {
        result.plan = None;
        let value = serde_json::to_value(&result)?;
        write_json_file(path, &value, "info written to")?;
    }

    Ok(success)
}
