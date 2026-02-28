/// Build 命令 —— 生成 OCI 镜像
///
/// 对齐 railpack `cmd/cli/build.go`

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use crate::buildkit::BuildOutput;
use crate::buildkit::convert::{convert_plan_to_llb, ConvertPlanOptions};
use crate::buildkit::daemon::select_daemon_manager;
use crate::buildkit::platform::parse_platform_with_defaults;
use crate::buildkit::grpc::solve::CacheConfig;
use crate::buildkit::grpc_client::{GrpcBuildKitClient, GrpcBuildRequest, build_export_config};
use crate::buildkit::grpc::progress::ProgressMode;
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

    /// 缓存导入配置（BuildKit cache-import 格式，如 type=gha,url=...）
    #[arg(long)]
    pub cache_import: Option<String>,

    /// 缓存导出配置（BuildKit cache-export 格式，如 type=gha,url=...）
    #[arg(long)]
    pub cache_export: Option<String>,

    /// 输出 LLB protobuf 到文件（- 表示 stdout），不执行构建
    #[arg(long)]
    pub dump_llb: Option<String>,

    /// 以 JSON 格式输出 LLB（配合 --dump-llb）
    #[arg(long, requires = "dump_llb")]
    pub dump_llb_json: bool,
}

/// 执行构建命令
pub fn run_build(args: &BuildArgs) -> crate::Result<bool> {
    // 1. 生成 BuildResult
    let mut result = generate_build_result_for_command(&args.common)?;

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

    if result.plan.is_none() {
        return Err(anyhow::anyhow!("构建计划生成成功但无 plan 数据").into());
    }

    // 4. --show-plan -> 输出 plan JSON 到 stdout
    if args.show_plan {
        let json = serde_json::to_string_pretty(result.plan.as_ref().unwrap())?;
        println!("{}", json);
    }

    // 5. 注入 GITHUB_TOKEN 到 mise install 步骤 + 验证 secrets
    let mut env_vars = parse_env_vars(&args.common.env)?;
    inject_github_token_for_mise(result.plan.as_mut().unwrap(), &mut env_vars);
    let plan = result.plan.as_ref().unwrap();
    validate_secrets(plan, &env_vars)?;

    // 6. 计算 secrets hash
    let secrets_hash = compute_secrets_hash(&env_vars);

    // 7. 解析平台
    let platform_str = args.platform.as_deref().unwrap_or("");
    let platform = parse_platform_with_defaults(platform_str)?;

    let cache_key = args.cache_key.clone().unwrap_or_default();
    let opts = ConvertPlanOptions {
        secrets_hash: Some(secrets_hash),
        platform,
        cache_key,
    };

    // 7.5. --dump-llb：导出 LLB 后提前返回
    if let Some(ref dump_path) = args.dump_llb {
        return dump_llb_definition(plan, &opts, dump_path, args.dump_llb_json);
    }

    // 8. 解析缓存配置
    let cache_imports: Vec<CacheConfig> = args
        .cache_import
        .iter()
        .map(|s| CacheConfig::parse(s))
        .collect();
    let cache_exports: Vec<CacheConfig> = args
        .cache_export
        .iter()
        .map(|s| CacheConfig::parse(s))
        .collect();

    // 9. BuildPlan → LLB → gRPC Solve
    let llb_result = convert_plan_to_llb(plan, &opts)?;

    // 当 --name 和 --output 均未指定时，从源码目录名派生镜像名
    let default_name;
    let image_name = if args.name.is_some() || args.output.is_some() {
        args.name.as_deref()
    } else {
        let dir_path = std::path::Path::new(&args.common.directory);
        let dir_abs = dir_path
            .canonicalize()
            .unwrap_or_else(|_| dir_path.to_path_buf());
        let raw = dir_abs
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        default_name = sanitize_image_name(raw);
        Some(default_name.as_str())
    };

    let export = build_export_config(
        image_name,
        args.output.as_ref().map(std::path::PathBuf::from).as_ref(),
        false,
    )
    .map_err(ArcpackError::Other)?;

    let progress_mode = match args.progress.as_str() {
        "plain" => ProgressMode::Plain,
        "tty" => ProgressMode::Tty,
        "quiet" => ProgressMode::Quiet,
        _ => ProgressMode::Auto,
    };

    let context_dir = std::path::PathBuf::from(&args.common.directory);

    run_with_daemon(|addr| async move {
        let client = GrpcBuildKitClient::new(&addr)
            .await
            .map_err(ArcpackError::Other)?;

        let mut local_dirs = HashMap::new();
        local_dirs.insert("context".to_string(), context_dir);

        let request = GrpcBuildRequest {
            definition: llb_result.definition,
            image_config: llb_result.image_config,
            export,
            secrets: env_vars,
            local_dirs,
            progress_mode,
            cache_imports,
            cache_exports,
        };

        client.build(request).await.map_err(ArcpackError::Other)
    })
}

/// 启动 daemon → wait_ready → 执行构建 → 停止 daemon
fn run_with_daemon<F, Fut>(build_fn: F) -> crate::Result<bool>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = crate::Result<BuildOutput>>,
{
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| anyhow::anyhow!("无法创建 tokio 运行时: {}", e))?;

    rt.block_on(async {
        let mut daemon = select_daemon_manager();
        daemon.start().await?;

        let build_result = async {
            daemon
                .wait_ready(std::time::Duration::from_secs(30))
                .await?;
            build_fn(daemon.socket_addr().to_string()).await
        }
        .await;

        if let Err(e) = daemon.stop().await {
            tracing::warn!("停止 daemon 失败: {}", e);
        }

        let output = build_result?;
        eprintln!("构建完成，耗时 {:.1}s", output.duration.as_secs_f64());
        Ok(true)
    })
}

// === --dump-llb 调试功能 ===

/// 导出 LLB Definition 到文件或 stdout，不执行构建
fn dump_llb_definition(
    plan: &crate::plan::BuildPlan,
    opts: &ConvertPlanOptions,
    dump_path: &str,
    as_json: bool,
) -> crate::Result<bool> {
    let llb_result = convert_plan_to_llb(plan, opts)?;
    if as_json {
        let json = definition_to_json(&llb_result.definition)?;
        let output = serde_json::to_string_pretty(&json)?;
        write_dump_output(dump_path, output.as_bytes())?;
    } else {
        use prost::Message;
        write_dump_output(dump_path, &llb_result.definition.encode_to_vec())?;
    }
    Ok(true)
}

/// 将 LLB Definition 转换为 JSON 值（调试用）
///
/// 解码每个 pb::Op 并描述类型和关键字段
pub(crate) fn definition_to_json(
    def: &crate::buildkit::proto::pb::Definition,
) -> crate::Result<serde_json::Value> {
    use crate::buildkit::proto::pb;
    use prost::Message;

    let mut ops = Vec::new();
    for (i, bytes) in def.def.iter().enumerate() {
        let op = pb::Op::decode(bytes.as_slice())
            .map_err(|e| anyhow::anyhow!("failed to decode Op[{}]: {}", i, e))?;

        let op_type = match op.op.as_ref() {
            None => "terminal",
            Some(pb::op::Op::Source(_)) => "source",
            Some(pb::op::Op::Exec(_)) => "exec",
            Some(pb::op::Op::File(_)) => "file",
            Some(pb::op::Op::Merge(_)) => "merge",
            Some(pb::op::Op::Diff(_)) => "diff",
            Some(pb::op::Op::Build(_)) => "build",
        };

        let mut entry = serde_json::json!({
            "index": i,
            "type": op_type,
            "inputs": op.inputs.iter().map(|inp| {
                serde_json::json!({
                    "digest": &inp.digest,
                    "index": inp.index,
                })
            }).collect::<Vec<_>>(),
        });

        // 附加类型特定信息
        if let Some(ref inner) = op.op {
            match inner {
                pb::op::Op::Source(src) => {
                    entry["identifier"] =
                        serde_json::Value::String(src.identifier.clone());
                }
                pb::op::Op::Exec(exec) => {
                    if let Some(ref meta) = exec.meta {
                        entry["args"] = serde_json::json!(meta.args);
                        entry["cwd"] =
                            serde_json::Value::String(meta.cwd.clone());
                    }
                }
                _ => {}
            }
        }

        ops.push(entry);
    }

    Ok(serde_json::json!({
        "ops": ops,
        "metadata_count": def.metadata.len(),
    }))
}

/// 写入导出数据到文件或 stdout（- 表示 stdout）
pub(crate) fn write_dump_output(path: &str, data: &[u8]) -> crate::Result<()> {
    if path == "-" {
        use std::io::Write;
        std::io::stdout()
            .write_all(data)
            .map_err(|e| anyhow::anyhow!("写入 stdout 失败: {}", e))?;
    } else {
        std::fs::write(path, data)
            .map_err(|e| anyhow::anyhow!("写入文件 {} 失败: {}", path, e))?;
    }
    Ok(())
}

// === 辅助函数 ===

/// 将目录名规范化为合法的 OCI 镜像名
///
/// 规则：小写，仅保留 `[a-z0-9._-]`，其余字符替换为 `-`，
/// 去除首尾 `-/.`，空串回退到 `app`。
fn sanitize_image_name(raw: &str) -> String {
    let sanitized: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' { c } else { '-' })
        .collect();
    let trimmed = sanitized.trim_matches(|c: char| c == '-' || c == '.');
    if trimmed.is_empty() {
        "app".to_string()
    } else {
        trimmed.to_string()
    }
}

/// 注入 GITHUB_TOKEN 到 mise install 步骤的 secrets 列表
///
/// 如果环境变量中包含 GITHUB_TOKEN，则将其添加到所有包含 mise install 的步骤中，
/// 避免 mise 下载时触发 GitHub API rate limit。
fn inject_github_token_for_mise(
    plan: &mut crate::plan::BuildPlan,
    env_vars: &mut HashMap<String, String>,
) {
    // 检查 GITHUB_TOKEN 是否存在（环境变量或 OS 环境）
    if !env_vars.contains_key("GITHUB_TOKEN") {
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            env_vars.insert("GITHUB_TOKEN".to_string(), token);
        } else {
            return;
        }
    }

    // 确保 plan.secrets 包含 GITHUB_TOKEN（通配符展开时会用到）
    if !plan.secrets.iter().any(|s| s == "GITHUB_TOKEN") {
        plan.secrets.push("GITHUB_TOKEN".to_string());
    }

    // 对于显式列出 secrets（非通配符）的步骤，也需要注入
    for step in &mut plan.steps {
        let has_mise = step.commands.iter().any(|cmd| {
            matches!(cmd, crate::plan::Command::Exec(exec) if exec.cmd.contains("mise install"))
        });
        if has_mise
            && !step.secrets.iter().any(|s| s == "*")
            && !step.secrets.contains(&"GITHUB_TOKEN".to_string())
        {
            step.secrets.push("GITHUB_TOKEN".to_string());
        }
    }
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
    use clap::Parser;
    use crate::plan::BuildPlan;

    // === --dump-llb 解析测试 ===

    #[test]
    fn test_dump_llb_flag_parses() {
        let cli = crate::cli::Cli::parse_from([
            "arcpack",
            "build",
            ".",
            "--dump-llb",
            "output.pb",
        ]);
        if let crate::cli::Commands::Build(args) = cli.command {
            assert_eq!(args.dump_llb, Some("output.pb".to_string()));
            assert!(!args.dump_llb_json);
        } else {
            panic!("Expected Build command");
        }
    }

    #[test]
    fn test_dump_llb_stdout_parses() {
        let cli =
            crate::cli::Cli::parse_from(["arcpack", "build", ".", "--dump-llb", "-"]);
        if let crate::cli::Commands::Build(args) = cli.command {
            assert_eq!(args.dump_llb, Some("-".to_string()));
        } else {
            panic!("Expected Build command");
        }
    }

    #[test]
    fn test_dump_llb_json_flag_parses() {
        let cli = crate::cli::Cli::parse_from([
            "arcpack",
            "build",
            ".",
            "--dump-llb",
            "-",
            "--dump-llb-json",
        ]);
        if let crate::cli::Commands::Build(args) = cli.command {
            assert_eq!(args.dump_llb, Some("-".to_string()));
            assert!(args.dump_llb_json);
        } else {
            panic!("Expected Build command");
        }
    }

    #[test]
    fn test_write_dump_output_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pb");
        let data = b"hello llb";
        write_dump_output(path.to_str().unwrap(), data).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), data);
    }

    #[test]
    fn test_write_dump_output_dash_does_not_panic() {
        // - 路径写入 stdout，只验证不 panic
        // 实际输出无法捕获，但确保函数正确路由
        let result = write_dump_output("-", b"test");
        assert!(result.is_ok());
    }

    #[test]
    fn test_definition_to_json_structure() {
        use crate::buildkit::proto::pb;
        use prost::Message;

        // 构造一个简单的 Definition：一个 source op
        let source_op = pb::Op {
            inputs: vec![],
            op: Some(pb::op::Op::Source(pb::SourceOp {
                identifier: "docker-image://ubuntu:22.04".to_string(),
                attrs: Default::default(),
            })),
            platform: None,
            constraints: None,
        };
        let source_bytes = source_op.encode_to_vec();

        let def = pb::Definition {
            def: vec![source_bytes],
            metadata: Default::default(),
            source: None,
        };

        let json = definition_to_json(&def).unwrap();
        let ops = json["ops"].as_array().unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0]["type"], "source");
        assert_eq!(ops[0]["identifier"], "docker-image://ubuntu:22.04");
        assert_eq!(json["metadata_count"], 0);
    }

    // === 原有测试 ===

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
        let cli = crate::cli::Cli::parse_from(["arcpack", "build", "."]);
        if let crate::cli::Commands::Build(args) = cli.command {
            assert_eq!(args.name, None);
            assert_eq!(args.progress, "auto");
            assert!(!args.show_plan);
            assert_eq!(args.cache_import, None);
            assert_eq!(args.cache_export, None);
        } else {
            panic!("Expected Build command");
        }
    }

    #[test]
    fn test_cache_import_export_flags_parse() {
        let cli = crate::cli::Cli::parse_from([
            "arcpack",
            "build",
            ".",
            "--cache-import",
            "type=gha,url=https://example.com",
            "--cache-export",
            "type=gha,mode=max",
        ]);
        if let crate::cli::Commands::Build(args) = cli.command {
            assert_eq!(
                args.cache_import,
                Some("type=gha,url=https://example.com".to_string())
            );
            assert_eq!(
                args.cache_export,
                Some("type=gha,mode=max".to_string())
            );
        } else {
            panic!("Expected Build command");
        }
    }

    // === sanitize_image_name 测试 ===

    #[test]
    fn test_sanitize_image_name_lowercase() {
        assert_eq!(sanitize_image_name("MyApp"), "myapp");
    }

    #[test]
    fn test_sanitize_image_name_special_chars() {
        assert_eq!(sanitize_image_name("my app@v2"), "my-app-v2");
    }

    #[test]
    fn test_sanitize_image_name_leading_trailing() {
        assert_eq!(sanitize_image_name("--my-app-."), "my-app");
    }

    #[test]
    fn test_sanitize_image_name_empty() {
        assert_eq!(sanitize_image_name(""), "app");
    }

    #[test]
    fn test_sanitize_image_name_all_invalid() {
        assert_eq!(sanitize_image_name("@#$"), "app");
    }

    #[test]
    fn test_sanitize_image_name_dots_and_underscores_preserved() {
        assert_eq!(sanitize_image_name("my_app.v2"), "my_app.v2");
    }
}
