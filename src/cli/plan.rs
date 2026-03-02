/// plan 命令 —— 输出 JSON 格式的 BuildPlan
///
/// 对齐 railpack `cli/plan.go`
use super::common::{
    add_schema_to_plan_json, generate_build_result_for_command, write_json_file, CommonBuildArgs,
};

/// Plan 命令参数
#[derive(Debug, clap::Args)]
pub struct PlanArgs {
    #[command(flatten)]
    pub common: CommonBuildArgs,

    /// 输出文件路径（空则写 stdout）
    #[arg(short = 'o', long = "out")]
    pub out: Option<String>,
}

/// 执行 plan 命令
pub fn run_plan(args: &PlanArgs) -> crate::Result<()> {
    let result = generate_build_result_for_command(&args.common)?;
    let plan = result
        .plan
        .ok_or_else(|| crate::ArcpackError::Other(anyhow::anyhow!("BuildResult missing plan")))?;
    let value = add_schema_to_plan_json(&plan)?;

    if let Some(ref path) = args.out {
        write_json_file(path, &value, "plan written to")?;
    } else {
        let json = serde_json::to_string_pretty(&value)?;
        println!("{}", json);
    }

    Ok(())
}
