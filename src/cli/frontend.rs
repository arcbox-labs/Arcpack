/// Frontend 子命令 —— buildkitd 前端模式入口
///
/// 由 buildkitd 在容器内调用（设置 BUILDKIT_FRONTEND_ADDR 环境变量），
/// 通过 gRPC 与 buildkitd 交互读取构建上下文并返回 LLB + Image Config。
///
/// Phase B-5 实现，当前仅为桩。

/// Frontend 命令参数
#[derive(Debug, clap::Args)]
pub struct FrontendArgs {}

/// 执行 frontend 模式
///
/// 检查 BUILDKIT_FRONTEND_ADDR 环境变量是否存在——
/// 缺失说明不是由 buildkitd 调用，给出友好提示。
pub fn run_frontend(_args: &FrontendArgs) -> crate::Result<bool> {
    let _addr = std::env::var("BUILDKIT_FRONTEND_ADDR").map_err(|_| {
        anyhow::anyhow!(
            "BUILDKIT_FRONTEND_ADDR not set. \
             Frontend mode must be invoked by buildkitd, not directly."
        )
    })?;

    Err(anyhow::anyhow!(
        "frontend mode not yet implemented (planned for Phase B-5)"
    )
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frontend_args_via_cli() {
        use clap::Parser;
        // 通过顶层 Cli 间接测试 FrontendArgs 解析
        let cli = crate::cli::Cli::parse_from(["arcpack", "frontend"]);
        assert!(
            matches!(cli.command, crate::cli::Commands::Frontend(_)),
            "应解析为 Frontend 子命令"
        );
    }

    #[test]
    fn test_frontend_no_env_returns_error() {
        // 保存原值并在测试结束时恢复
        let original = std::env::var("BUILDKIT_FRONTEND_ADDR").ok();
        std::env::remove_var("BUILDKIT_FRONTEND_ADDR");

        let args = FrontendArgs {};
        let result = run_frontend(&args);

        // 恢复原值
        if let Some(val) = original {
            std::env::set_var("BUILDKIT_FRONTEND_ADDR", val);
        }

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("BUILDKIT_FRONTEND_ADDR"),
            "错误信息应提及环境变量: {err_msg}"
        );
    }
}
