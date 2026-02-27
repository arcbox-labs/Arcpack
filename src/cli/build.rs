/// Build 命令 —— 生成 OCI 镜像
///
/// 对齐 railpack `cmd/cli/build.go`

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use crate::buildkit::client::{BuildKitClient, BuildRequest};
use crate::buildkit::convert::{convert_plan_to_dockerfile, ConvertPlanOptions};
use crate::buildkit::daemon::select_daemon_manager;
use crate::buildkit::platform::parse_platform_with_defaults;
use crate::cli::common::{generate_build_result_for_command, parse_env_vars, CommonBuildArgs};
use crate::cli::pretty_print::{pretty_print_build_result, OutputStream, PrintOptions};
use crate::ArcpackError;

/// Build 命令参数
#[derive(Debug, clap::Args)]
pub struct BuildArgs {
    #[command(flatten)]
    pub common: CommonBuildArgs,

    /// 镜像名称
    #[arg(long)]
    pub name: Option<String>,

    /// 输出到本地目录
    #[arg(long)]
    pub output: Option<String>,

    /// 目标平台（例如 linux/amd64）
    #[arg(long)]
    pub platform: Option<String>,

    /// 进度模式：auto/plain/tty
    #[arg(long, default_value = "auto")]
    pub progress: String,

    /// 构建前展示 plan JSON
    #[arg(long)]
    pub show_plan: bool,

    /// 缓存键前缀
    #[arg(long)]
    pub cache_key: Option<String>,
}

/// 执行构建命令
pub fn run_build(args: &BuildArgs) -> crate::Result<bool> {
    // 1. 生成 BuildResult
    let result = generate_build_result_for_command(&args.common)?;

    // 2. Pretty print 到 stderr
    let print_options = PrintOptions {
        metadata: false,
        version: env!("CARGO_PKG_VERSION").to_string(),
        stream: OutputStream::Stderr,
    };
    pretty_print_build_result(&result, &print_options);

    // 3. 检查是否成功
    if !result.success {
        return Ok(false);
    }

    let plan = result.plan.as_ref().ok_or_else(|| {
        anyhow::anyhow!("构建计划生成成功但无 plan 数据")
    })?;

    // 4. --show-plan -> 输出 plan JSON 到 stdout
    if args.show_plan {
        let json = serde_json::to_string_pretty(plan)?;
        println!("{}", json);
    }

    // 5. 验证 secrets
    let env_vars = parse_env_vars(&args.common.env)?;
    validate_secrets(plan, &env_vars)?;

    // 6. 计算 secrets hash
    let secrets_hash = compute_secrets_hash(&env_vars);

    // 7. 解析平台
    let platform_str = args.platform.as_deref().unwrap_or("");
    let platform = parse_platform_with_defaults(platform_str)?;

    // 8. 转换为 Dockerfile
    let cache_key = args.cache_key.clone().unwrap_or_default();
    let opts = ConvertPlanOptions {
        secrets_hash: Some(secrets_hash),
        platform: platform.clone(),
        cache_key,
    };
    let convert_result = convert_plan_to_dockerfile(plan, &opts)?;

    // 9-11. BuildKit 构建（异步）
    let rt = tokio::runtime::Runtime::new().map_err(|e| {
        anyhow::anyhow!("无法创建 tokio 运行时: {}", e)
    })?;

    rt.block_on(async {
        // 选择并启动 daemon
        let mut daemon = select_daemon_manager();
        daemon.start().await?;

        // wait_ready + build 包装在闭包中，确保无论成功失败都执行 stop
        let build_result = async {
            daemon
                .wait_ready(std::time::Duration::from_secs(30))
                .await?;

            let client = BuildKitClient::new(daemon.socket_addr());
            let request = BuildRequest {
                context_dir: std::path::PathBuf::from(&args.common.directory),
                dockerfile_content: convert_result.dockerfile,
                image_name: args.name.clone(),
                output_dir: args.output.as_ref().map(std::path::PathBuf::from),
                push: false,
                platform: platform.to_string(),
                progress_mode: args.progress.clone(),
                cache_import: None,
                cache_export: None,
                secrets: env_vars.clone(),
            };

            client.build(&request).await
        }
        .await;

        // 停止 daemon（无论构建是否成功都必须执行）
        if let Err(e) = daemon.stop().await {
            tracing::warn!("停止 daemon 失败: {}", e);
        }

        match build_result {
            Ok(output) => {
                eprintln!(
                    "构建完成，耗时 {:.1}s",
                    output.duration.as_secs_f64()
                );
                Ok(true)
            }
            Err(e) => Err(e),
        }
    })
}

/// 验证 plan 中声明的 secrets 在环境变量中都存在
fn validate_secrets(
    plan: &crate::plan::BuildPlan,
    env_vars: &HashMap<String, String>,
) -> crate::Result<()> {
    for secret in &plan.secrets {
        if !env_vars.contains_key(secret) {
            return Err(ArcpackError::MissingSecret {
                name: secret.clone(),
            });
        }
    }
    Ok(())
}

/// 计算所有环境变量值的 SHA256 哈希（用于缓存失效）
fn compute_secrets_hash(env_vars: &HashMap<String, String>) -> String {
    let mut hasher = Sha256::new();
    // 排序保证确定性
    let mut keys: Vec<&String> = env_vars.keys().collect();
    keys.sort();
    for key in keys {
        hasher.update(key.as_bytes());
        hasher.update(b"=");
        hasher.update(env_vars[key].as_bytes());
        hasher.update(b"\0");
    }
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::BuildPlan;

    #[test]
    fn test_validate_secrets_all_present() {
        let mut plan = BuildPlan::new();
        plan.secrets = vec!["API_KEY".to_string(), "DB_PASS".to_string()];

        let mut env = HashMap::new();
        env.insert("API_KEY".to_string(), "key123".to_string());
        env.insert("DB_PASS".to_string(), "pass456".to_string());

        assert!(validate_secrets(&plan, &env).is_ok());
    }

    #[test]
    fn test_validate_secrets_missing() {
        let mut plan = BuildPlan::new();
        plan.secrets = vec!["API_KEY".to_string()];

        let env = HashMap::new();
        let result = validate_secrets(&plan, &env);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("API_KEY"));
    }

    #[test]
    fn test_validate_secrets_empty_plan() {
        let plan = BuildPlan::new();
        let env = HashMap::new();
        assert!(validate_secrets(&plan, &env).is_ok());
    }

    #[test]
    fn test_compute_secrets_hash_deterministic() {
        let mut env = HashMap::new();
        env.insert("A".to_string(), "val1".to_string());
        env.insert("B".to_string(), "val2".to_string());

        let hash1 = compute_secrets_hash(&env);
        let hash2 = compute_secrets_hash(&env);
        assert_eq!(hash1, hash2);
        assert!(!hash1.is_empty());
    }

    #[test]
    fn test_compute_secrets_hash_different_values() {
        let mut env1 = HashMap::new();
        env1.insert("A".to_string(), "val1".to_string());

        let mut env2 = HashMap::new();
        env2.insert("A".to_string(), "val2".to_string());

        assert_ne!(compute_secrets_hash(&env1), compute_secrets_hash(&env2));
    }

    #[test]
    fn test_build_args_parsing() {
        use clap::Parser;
        let cli = crate::cli::Cli::parse_from([
            "arcpack",
            "build",
            ".",
            "--name",
            "myapp",
            "--platform",
            "linux/amd64",
            "--progress",
            "plain",
            "--show-plan",
        ]);
        if let crate::cli::Commands::Build(args) = cli.command {
            assert_eq!(args.name, Some("myapp".to_string()));
            assert_eq!(args.platform, Some("linux/amd64".to_string()));
            assert_eq!(args.progress, "plain");
            assert!(args.show_plan);
        } else {
            panic!("Expected Build command");
        }
    }

    #[test]
    fn test_build_args_defaults() {
        use clap::Parser;
        let cli = crate::cli::Cli::parse_from(["arcpack", "build", "."]);
        if let crate::cli::Commands::Build(args) = cli.command {
            assert_eq!(args.name, None);
            assert_eq!(args.progress, "auto");
            assert!(!args.show_plan);
        } else {
            panic!("Expected Build command");
        }
    }
}
