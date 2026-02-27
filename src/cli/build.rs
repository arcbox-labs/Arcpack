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

#[cfg(feature = "llb")]
use crate::buildkit::convert::convert_plan_to_llb;
#[cfg(feature = "llb")]
use crate::buildkit::client::LlbBuildRequest;
#[cfg(feature = "grpc")]
use crate::buildkit::grpc_client::{GrpcBuildKitClient, GrpcBuildRequest, build_export_config};
#[cfg(feature = "grpc")]
use crate::buildkit::grpc::progress::ProgressMode;

/// 构建后端路径
///
/// clap ValueEnum derive 不支持 #[cfg] 在 variant 上，
/// 所以所有 variant 始终可见，feature 检查在运行时完成。
#[derive(Clone, Debug, PartialEq, clap::ValueEnum)]
pub enum Backend {
    /// Phase A：BuildPlan → Dockerfile → buildctl CLI（默认，稳定路径）
    Dockerfile,
    /// Phase B-2：BuildPlan → LLB → buildctl stdin
    Llb,
    /// Phase B-3：BuildPlan → LLB → gRPC Solve
    Grpc,
}

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

    /// 构建后端：dockerfile / llb / grpc
    #[arg(long, value_enum)]
    pub backend: Option<Backend>,

    /// 输出 LLB protobuf 到文件（- 表示 stdout），不执行构建
    #[arg(long)]
    pub dump_llb: Option<String>,

    /// 以 JSON 格式输出 LLB（配合 --dump-llb）
    #[arg(long, requires = "dump_llb")]
    pub dump_llb_json: bool,
}

/// 解析后端：--backend > ARCPACK_BACKEND 环境变量 > 默认 Dockerfile
fn resolve_backend(args: &BuildArgs) -> crate::Result<Backend> {
    let env_val = std::env::var("ARCPACK_BACKEND").ok();
    resolve_backend_from(args.backend.as_ref(), env_val.as_deref())
}

/// 纯逻辑后端解析（不读取全局状态，方便测试）
///
/// 优先级：CLI flag > env_val > 默认 Dockerfile
fn resolve_backend_from(
    flag: Option<&Backend>,
    env_val: Option<&str>,
) -> crate::Result<Backend> {
    // 1. CLI flag 优先
    if let Some(backend) = flag {
        return validate_backend_feature(backend.clone());
    }
    // 2. 环境变量
    if let Some(val) = env_val {
        let backend = match val.to_lowercase().as_str() {
            "dockerfile" => Backend::Dockerfile,
            "llb" => Backend::Llb,
            "grpc" => Backend::Grpc,
            other => {
                return Err(
                    anyhow::anyhow!("unknown ARCPACK_BACKEND value: {other}").into(),
                )
            }
        };
        return validate_backend_feature(backend);
    }
    // 3. 默认
    Ok(Backend::Dockerfile)
}

/// 校验所选后端是否有对应 feature 编译支持
fn validate_backend_feature(backend: Backend) -> crate::Result<Backend> {
    match &backend {
        Backend::Llb => {
            #[cfg(not(feature = "llb"))]
            return Err(anyhow::anyhow!(
                "backend 'llb' requires the 'llb' feature. \
                 Rebuild with: cargo build --features llb"
            )
            .into());
        }
        Backend::Grpc => {
            #[cfg(not(feature = "grpc"))]
            return Err(anyhow::anyhow!(
                "backend 'grpc' requires the 'grpc' feature. \
                 Rebuild with: cargo build --features grpc"
            )
            .into());
        }
        Backend::Dockerfile => {}
    }
    Ok(backend)
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
        platform: platform.clone(),
        cache_key,
    };

    // 7.5. --dump-llb：导出 LLB 后提前返回，不需要 backend
    if let Some(ref dump_path) = args.dump_llb {
        return dump_llb_definition(plan, &opts, dump_path, args.dump_llb_json);
    }

    // 8. 解析 backend（仅在实际构建时才需要）
    let backend = resolve_backend(args)?;

    // 8.5. cache 参数仅 dockerfile 后端支持，其他路径提示 warning
    if backend != Backend::Dockerfile
        && (args.cache_import.is_some() || args.cache_export.is_some())
    {
        tracing::warn!(
            "--cache-import/--cache-export are only supported with the 'dockerfile' backend; \
             these flags will be ignored for backend '{:?}'",
            backend
        );
    }

    // 9. 按 backend 分路构建
    match backend {
        Backend::Dockerfile => build_via_dockerfile(args, plan, &opts, &platform, &env_vars),
        Backend::Llb => build_via_llb(args, plan, &opts, &env_vars),
        Backend::Grpc => build_via_grpc(args, plan, &opts, &env_vars),
    }
}

/// 启动 daemon → wait_ready → 执行构建 → 停止 daemon
///
/// 将三条构建路径共用的 daemon 生命周期管理提取为单一入口。
/// `label` 仅用于成功日志（如 ""、"（LLB）"、"（gRPC）"）。
fn run_with_daemon<F, Fut>(
    label: &str,
    build_fn: F,
) -> crate::Result<bool>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = crate::Result<crate::buildkit::client::BuildOutput>>,
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
        eprintln!("构建完成{label}，耗时 {:.1}s", output.duration.as_secs_f64());
        Ok(true)
    })
}

/// Phase A：BuildPlan → Dockerfile → buildctl CLI
fn build_via_dockerfile(
    args: &BuildArgs,
    plan: &crate::plan::BuildPlan,
    opts: &ConvertPlanOptions,
    platform: &crate::buildkit::platform::Platform,
    env_vars: &HashMap<String, String>,
) -> crate::Result<bool> {
    let convert_result = convert_plan_to_dockerfile(plan, opts)?;

    run_with_daemon("", |addr| async move {
        let client = BuildKitClient::new(addr);
        let request = BuildRequest {
            context_dir: std::path::PathBuf::from(&args.common.directory),
            dockerfile_content: convert_result.dockerfile,
            image_name: args.name.clone(),
            output_dir: args.output.as_ref().map(std::path::PathBuf::from),
            push: false,
            platform: platform.to_string(),
            progress_mode: args.progress.clone(),
            cache_import: args.cache_import.clone(),
            cache_export: args.cache_export.clone(),
            secrets: env_vars.clone(),
        };

        client.build(&request).await
    })
}

/// Phase B-2：BuildPlan → LLB → buildctl stdin
#[cfg(feature = "llb")]
fn build_via_llb(
    args: &BuildArgs,
    plan: &crate::plan::BuildPlan,
    opts: &ConvertPlanOptions,
    env_vars: &HashMap<String, String>,
) -> crate::Result<bool> {
    let llb_result = convert_plan_to_llb(plan, opts)?;

    run_with_daemon("（LLB）", |addr| async move {
        let client = BuildKitClient::new(addr);
        let request = LlbBuildRequest {
            definition: llb_result.definition,
            context_dir: std::path::PathBuf::from(&args.common.directory),
            image_name: args.name.clone(),
            output_dir: args.output.as_ref().map(std::path::PathBuf::from),
            push: false,
            progress_mode: args.progress.clone(),
            secrets: env_vars.clone(),
            no_cache: false,
        };

        client.build_from_llb(&request).await
    })
}

#[cfg(not(feature = "llb"))]
fn build_via_llb(
    _args: &BuildArgs,
    _plan: &crate::plan::BuildPlan,
    _opts: &ConvertPlanOptions,
    _env_vars: &HashMap<String, String>,
) -> crate::Result<bool> {
    Err(anyhow::anyhow!(
        "backend 'llb' requires the 'llb' feature. \
         Rebuild with: cargo build --features llb"
    )
    .into())
}

/// Phase B-3：BuildPlan → LLB → gRPC Solve
#[cfg(feature = "grpc")]
fn build_via_grpc(
    args: &BuildArgs,
    plan: &crate::plan::BuildPlan,
    opts: &ConvertPlanOptions,
    env_vars: &HashMap<String, String>,
) -> crate::Result<bool> {
    let llb_result = convert_plan_to_llb(plan, opts)?;

    let export = build_export_config(
        args.name.as_deref(),
        args.output.as_ref().map(std::path::PathBuf::from).as_ref(),
        false, // push
    )
    .map_err(|e| ArcpackError::Other(e))?;

    let progress_mode = match args.progress.as_str() {
        "plain" => ProgressMode::Plain,
        "tty" => ProgressMode::Tty,
        "quiet" => ProgressMode::Quiet,
        _ => ProgressMode::Auto,
    };

    run_with_daemon("（gRPC）", |addr| async move {
        let client = GrpcBuildKitClient::new(addr)
            .await
            .map_err(|e| ArcpackError::Other(e))?;

        let mut local_dirs = HashMap::new();
        local_dirs.insert(
            "context".to_string(),
            std::path::PathBuf::from(&args.common.directory),
        );

        let request = GrpcBuildRequest {
            definition: llb_result.definition,
            image_config: llb_result.image_config,
            context_dir: std::path::PathBuf::from(&args.common.directory),
            export,
            secrets: env_vars.clone(),
            local_dirs,
            progress_mode,
        };

        client
            .build(request)
            .await
            .map_err(|e| ArcpackError::Other(e))
    })
}

#[cfg(not(feature = "grpc"))]
fn build_via_grpc(
    _args: &BuildArgs,
    _plan: &crate::plan::BuildPlan,
    _opts: &ConvertPlanOptions,
    _env_vars: &HashMap<String, String>,
) -> crate::Result<bool> {
    Err(anyhow::anyhow!(
        "backend 'grpc' requires the 'grpc' feature. \
         Rebuild with: cargo build --features grpc"
    )
    .into())
}

// === --dump-llb 调试功能 ===

/// 导出 LLB Definition 到文件或 stdout，不执行构建
fn dump_llb_definition(
    plan: &crate::plan::BuildPlan,
    opts: &ConvertPlanOptions,
    dump_path: &str,
    as_json: bool,
) -> crate::Result<bool> {
    #[cfg(feature = "llb")]
    {
        let llb_result = convert_plan_to_llb(plan, opts)?;
        if as_json {
            let json = definition_to_json(&llb_result.definition)?;
            let output = serde_json::to_string_pretty(&json)?;
            write_dump_output(dump_path, output.as_bytes())?;
        } else {
            use prost::Message;
            write_dump_output(dump_path, &llb_result.definition.encode_to_vec())?;
        }
        return Ok(true);
    }
    #[cfg(not(feature = "llb"))]
    {
        let _ = (plan, opts, dump_path, as_json);
        Err(anyhow::anyhow!(
            "--dump-llb requires the 'llb' feature. \
             Rebuild with: cargo build --features llb"
        )
        .into())
    }
}

/// 将 LLB Definition 转换为 JSON 值（调试用）
///
/// 解码每个 pb::Op 并描述类型和关键字段
#[cfg(feature = "llb")]
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

    // 注入到包含 mise install 的步骤
    for step in &mut plan.steps {
        let has_mise = step.commands.iter().any(|cmd| {
            if let crate::plan::Command::Exec(exec) = cmd {
                exec.cmd.contains("mise install")
            } else {
                false
            }
        });
        if has_mise && !step.secrets.contains(&"GITHUB_TOKEN".to_string()) {
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

    // === Backend 枚举与解析测试 ===

    #[test]
    fn test_backend_default_is_none() {
        let cli = crate::cli::Cli::parse_from(["arcpack", "build", "."]);
        if let crate::cli::Commands::Build(args) = cli.command {
            assert!(args.backend.is_none(), "默认应为 None");
        } else {
            panic!("Expected Build command");
        }
    }

    #[test]
    fn test_backend_flag_llb_parses() {
        let cli =
            crate::cli::Cli::parse_from(["arcpack", "build", ".", "--backend", "llb"]);
        if let crate::cli::Commands::Build(args) = cli.command {
            assert_eq!(args.backend, Some(Backend::Llb));
        } else {
            panic!("Expected Build command");
        }
    }

    #[test]
    fn test_backend_flag_grpc_parses() {
        let cli =
            crate::cli::Cli::parse_from(["arcpack", "build", ".", "--backend", "grpc"]);
        if let crate::cli::Commands::Build(args) = cli.command {
            assert_eq!(args.backend, Some(Backend::Grpc));
        } else {
            panic!("Expected Build command");
        }
    }

    #[test]
    fn test_backend_flag_invalid_rejected() {
        let result =
            crate::cli::Cli::try_parse_from(["arcpack", "build", ".", "--backend", "foobar"]);
        assert!(result.is_err(), "无效的 backend 值应被 clap 拒绝");
    }

    #[test]
    fn test_resolve_backend_default_dockerfile() {
        let backend = resolve_backend_from(None, None).unwrap();
        assert_eq!(backend, Backend::Dockerfile);
    }

    #[test]
    fn test_resolve_backend_env_override() {
        let result = resolve_backend_from(None, Some("llb"));
        // llb feature 可能未启用，检查是否正确解析了值
        #[cfg(feature = "llb")]
        assert_eq!(result.unwrap(), Backend::Llb);
        #[cfg(not(feature = "llb"))]
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_backend_flag_over_env() {
        // flag=dockerfile + env=grpc → flag 胜出
        let backend =
            resolve_backend_from(Some(&Backend::Dockerfile), Some("grpc")).unwrap();
        assert_eq!(backend, Backend::Dockerfile, "CLI flag 应覆盖环境变量");
    }

    #[test]
    fn test_resolve_backend_env_invalid() {
        let result = resolve_backend_from(None, Some("foobar"));
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("foobar"),
            "错误信息应包含无效值"
        );
    }

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

    #[cfg(feature = "llb")]
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
}
