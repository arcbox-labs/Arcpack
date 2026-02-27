/// info 命令 —— 输出构建元信息
///
/// 对齐 railpack `cli/info.go`

use super::common::{CommonBuildArgs, generate_build_result_for_command, write_json_file, write_text_file};
use super::pretty_print::{PrintOptions, OutputStream, format_build_result};

/// 输出格式
#[derive(Debug, Clone, clap::ValueEnum)]
pub enum InfoFormat {
    Pretty,
    Json,
}

/// Info 命令参数
#[derive(Debug, clap::Args)]
pub struct InfoArgs {
    #[command(flatten)]
    pub common: CommonBuildArgs,

    /// 输出格式：pretty 或 json
    #[arg(long = "format", value_enum, default_value_t = InfoFormat::Pretty)]
    pub format: InfoFormat,

    /// 输出文件路径
    #[arg(long = "out")]
    pub out: Option<String>,
}

/// 执行 info 命令
///
/// 返回 result.success，调用方据此决定退出码
pub fn run_info(args: &InfoArgs) -> crate::Result<bool> {
    let result = generate_build_result_for_command(&args.common)?;
    let success = result.success;

    match args.format {
        InfoFormat::Json => {
            let value = serde_json::to_value(&result)?;
            if let Some(ref path) = args.out {
                write_json_file(path, &value, "info written to")?;
            } else {
                let json = serde_json::to_string_pretty(&value)?;
                println!("{}", json);
            }
        }
        InfoFormat::Pretty => {
            let options = PrintOptions {
                metadata: true,
                version: result.arcpack_version.clone(),
                stream: OutputStream::Stdout,
            };
            let text = format_build_result(&result, &options);
            if let Some(ref path) = args.out {
                write_text_file(path, &text, "info written to")?;
            } else {
                print!("{}", text);
            }
        }
    }

    Ok(success)
}
