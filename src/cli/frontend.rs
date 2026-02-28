/// Frontend 子命令 —— buildkitd 前端模式入口
///
/// 由 buildkitd 在容器内调用（设置 BUILDKIT_FRONTEND_ADDR 环境变量），
/// 通过 gRPC gateway 与 buildkitd 交互：读取构建上下文并返回 LLB Definition + Image Config。
///
/// 工作流：
/// 1. 从 BUILDKIT_FRONTEND_ADDR 连接 gateway
/// 2. 通过 gateway Solve RPC 获取构建上下文引用
/// 3. 通过 ReadFile/ReadDir RPC 将源码同步到临时目录
/// 4. 执行标准检测 → BuildPlan → LLB 转换流水线
/// 5. 通过 Return RPC 将 LLB + ImageConfig 返回给 buildkitd
///
/// 对齐 railpack `cmd/cli/frontend.go`

use std::collections::HashMap;

use tonic::transport::Channel;

use crate::buildkit::grpc::channel::create_channel;
use crate::buildkit::image::ImageConfig;
use crate::buildkit::proto::{gateway, pb};
use crate::plan::BuildPlan;

/// Frontend 命令参数（无用户参数——所有配置通过 buildkitd 的 frontend opts 传入）
#[derive(Debug, clap::Args)]
pub struct FrontendArgs {}

/// 从环境变量解析 frontend options
///
/// buildkitd 启动 frontend 时，将 `--opt key=value` 编码为
/// `BUILDKIT_FRONTEND_OPT_key=value` 环境变量。
/// 对齐 Go `grpcclient.opts()`
fn parse_frontend_opts() -> HashMap<String, String> {
    std::env::vars()
        .filter_map(|(k, v)| {
            k.strip_prefix("BUILDKIT_FRONTEND_OPT_")
                .map(|key| (key.to_string(), v))
        })
        .collect()
}

/// 从 frontend opts 提取 build-arg 参数
///
/// 对齐 railpack `parseBuildArgs()`：
/// `build-arg:secrets-hash=xxx` → `{"secrets-hash": "xxx"}`
fn parse_build_args(opts: &HashMap<String, String>) -> HashMap<String, String> {
    opts.iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("build-arg:")
                .map(|name| (name.to_string(), v.clone()))
        })
        .collect()
}

/// Gateway 客户端 —— 与 buildkitd LLBBridge 服务通信
///
/// 封装 tonic 生成的 LlbBridgeClient，提供面向 arcpack 的高级接口。
pub struct GatewayClient {
    client: gateway::llb_bridge_client::LlbBridgeClient<Channel>,
}

impl std::fmt::Debug for GatewayClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayClient").finish()
    }
}

/// 构建上下文中的文件信息（从 ReadDir 返回）
#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: String,
    pub is_dir: bool,
    pub size: i64,
}

impl GatewayClient {
    /// 从 BUILDKIT_FRONTEND_ADDR 环境变量获取 gateway 地址并连接
    pub async fn from_env() -> crate::Result<Self> {
        let addr = std::env::var("BUILDKIT_FRONTEND_ADDR").map_err(|_| {
            anyhow::anyhow!(
                "BUILDKIT_FRONTEND_ADDR not set. \
                 Frontend mode must be invoked by buildkitd, not directly."
            )
        })?;

        Self::connect(&addr).await
    }

    /// 连接指定地址的 gateway（可测试入口）
    pub async fn connect(addr: &str) -> crate::Result<Self> {
        let channel = create_channel(addr)
            .await
            .map_err(|e| anyhow::anyhow!("failed to connect to gateway at {addr}: {e}"))?;

        Ok(Self {
            client: gateway::llb_bridge_client::LlbBridgeClient::new(channel),
        })
    }

    /// Ping gateway 验证连接（同时获取 capabilities）
    pub async fn ping(&mut self) -> crate::Result<gateway::PongResponse> {
        let resp = self
            .client
            .ping(gateway::PingRequest {})
            .await
            .map_err(|e| anyhow::anyhow!("gateway ping failed: {e}"))?;
        Ok(resp.into_inner())
    }

    /// 读取构建上下文中的文件内容
    ///
    /// `context_ref` 是通过 Solve 获取的上下文引用 ID
    pub async fn read_file(
        &mut self,
        context_ref: &str,
        path: &str,
    ) -> crate::Result<Vec<u8>> {
        let resp = self
            .client
            .read_file(gateway::ReadFileRequest {
                r#ref: context_ref.to_string(),
                file_path: path.to_string(),
                range: None,
            })
            .await
            .map_err(|e| {
                anyhow::anyhow!("gateway ReadFile({path}) failed: {e}")
            })?;
        Ok(resp.into_inner().data)
    }

    /// 读取构建上下文目录列表
    ///
    /// `context_ref` 是通过 Solve 获取的上下文引用 ID
    pub async fn read_dir(
        &mut self,
        context_ref: &str,
        path: &str,
    ) -> crate::Result<Vec<FileInfo>> {
        let resp = self
            .client
            .read_dir(gateway::ReadDirRequest {
                r#ref: context_ref.to_string(),
                dir_path: path.to_string(),
                include_pattern: String::new(),
            })
            .await
            .map_err(|e| {
                anyhow::anyhow!("gateway ReadDir({path}) failed: {e}")
            })?;

        let entries = resp
            .into_inner()
            .entries
            .into_iter()
            .map(|stat| {
                // stat.mode 的高 4 位表示文件类型，0o040000 = 目录
                let is_dir = (stat.mode & 0o170000) == 0o040000;
                FileInfo {
                    path: stat.path,
                    is_dir,
                    size: stat.size,
                }
            })
            .collect();
        Ok(entries)
    }

    /// 通过 Solve 获取构建上下文引用
    ///
    /// 发送一个空 Definition 的 Solve 请求，buildkitd 会返回包含上下文文件的引用。
    /// 此引用可用于后续的 ReadFile/ReadDir 调用。
    pub async fn solve_context(&mut self) -> crate::Result<String> {
        self.solve_local_source("local://context", HashMap::new(), "load build context")
            .await
    }

    /// 通过 dockerfile mount 获取 plan 文件引用
    ///
    /// 对齐 railpack `readFile()` 中 `llb.Local("dockerfile", followPaths)` + Solve 的模式。
    /// `local://dockerfile` 是 BuildKit 约定的 config 文件 mount 名称。
    async fn solve_plan_mount(&mut self, filename: &str) -> crate::Result<String> {
        let attrs = HashMap::from([(
            "local.followpaths".to_string(),
            serde_json::to_string(&[filename]).unwrap_or_default(),
        )]);
        self.solve_local_source(
            "local://dockerfile",
            attrs,
            &format!("load build definition from {filename}"),
        )
        .await
    }

    /// 构造 local source Definition → Solve → 返回 ref ID
    ///
    /// `solve_context()` 和 `solve_plan_mount()` 的共享实现。
    async fn solve_local_source(
        &mut self,
        identifier: &str,
        attrs: HashMap<String, String>,
        description: &str,
    ) -> crate::Result<String> {
        use prost::Message;

        let source_op = pb::Op {
            inputs: vec![],
            op: Some(pb::op::Op::Source(pb::SourceOp {
                identifier: identifier.to_string(),
                attrs,
            })),
            platform: None,
            constraints: None,
        };
        let source_bytes = source_op.encode_to_vec();
        let source_digest = format!("sha256:{}", sha2_digest(&source_bytes));

        let terminal = pb::Op {
            inputs: vec![pb::Input {
                digest: source_digest.clone(),
                index: 0,
            }],
            op: None,
            platform: None,
            constraints: None,
        };
        let terminal_bytes = terminal.encode_to_vec();

        let definition = pb::Definition {
            def: vec![source_bytes, terminal_bytes],
            metadata: HashMap::from([(
                source_digest,
                pb::OpMetadata {
                    description: HashMap::from([(
                        "llb.customname".to_string(),
                        description.to_string(),
                    )]),
                    ..Default::default()
                },
            )]),
            source: None,
        };

        let resp = self
            .client
            .solve(gateway::SolveRequest {
                definition: Some(definition),
                frontend: String::new(),
                frontend_opt: HashMap::new(),
                allow_result_return: true,
                allow_result_array_ref: true,
                r#final: false,
                exporter_attr: Vec::new(),
                cache_imports: vec![],
                frontend_inputs: HashMap::new(),
                evaluate: false,
                source_policies: vec![],
            })
            .await
            .map_err(|e| anyhow::anyhow!("gateway Solve ({identifier}) failed: {e}"))?;

        let result = resp
            .into_inner()
            .result
            .ok_or_else(|| anyhow::anyhow!("gateway Solve ({identifier}) returned no result"))?;

        match result.result {
            Some(gateway::result::Result::Ref(r)) => Ok(r.id),
            Some(gateway::result::Result::RefDeprecated(id)) => Ok(id),
            _ => Err(
                anyhow::anyhow!("gateway Solve returned unexpected result type").into(),
            ),
        }
    }

    /// 从 dockerfile mount 读取并反序列化 plan 文件
    ///
    /// 对齐 railpack `readRailpackPlan()`
    pub async fn read_plan_file(&mut self, filename: &str) -> crate::Result<BuildPlan> {
        let ref_id = self.solve_plan_mount(filename).await?;
        let data = self.read_file(&ref_id, filename).await?;
        let plan: BuildPlan = serde_json::from_slice(&data)
            .map_err(|e| anyhow::anyhow!("解析 plan 文件 '{filename}' 失败: {e}"))?;
        Ok(plan)
    }

    /// 将构建上下文同步到本地临时目录
    ///
    /// 递归读取 gateway 中的文件树，写入本地目录。
    /// 用于将远程构建上下文转换为本地路径，供标准检测流水线使用。
    pub async fn sync_context_to_dir(
        &mut self,
        context_ref: &str,
        target_dir: &std::path::Path,
    ) -> crate::Result<()> {
        self.sync_dir_recursive(context_ref, "", target_dir).await
    }

    /// 递归同步目录（Box::pin 避免 async 递归无限大小）
    fn sync_dir_recursive<'a>(
        &'a mut self,
        context_ref: &'a str,
        remote_path: &'a str,
        local_dir: &'a std::path::Path,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::Result<()>> + 'a>>
    {
        Box::pin(async move {
            let entries = self.read_dir(context_ref, remote_path).await?;

            for entry in entries {
                // 结构化路径校验，防止路径穿越写出 temp_dir
                validate_entry_path(&entry.path)?;
                let local_path = local_dir.join(&entry.path);
                let entry_remote = if remote_path.is_empty() {
                    entry.path.clone()
                } else {
                    format!("{}/{}", remote_path, entry.path)
                };

                if entry.is_dir {
                    std::fs::create_dir_all(&local_path).map_err(|e| {
                        anyhow::anyhow!("创建目录 {} 失败: {e}", local_path.display())
                    })?;
                    self.sync_dir_recursive(context_ref, &entry_remote, local_dir)
                        .await?;
                } else {
                    if let Some(parent) = local_path.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| {
                            anyhow::anyhow!("创建目录 {} 失败: {e}", parent.display())
                        })?;
                    }
                    let data = self.read_file(context_ref, &entry_remote).await?;
                    std::fs::write(&local_path, &data).map_err(|e| {
                        anyhow::anyhow!("写入文件 {} 失败: {e}", local_path.display())
                    })?;
                }
            }
            Ok(())
        })
    }

    /// 返回构建结果给 buildkitd
    ///
    /// 将 LLB Definition 和 ImageConfig 打包为 ReturnRequest 发送。
    pub async fn return_result(
        &mut self,
        definition: pb::Definition,
        image_config: &ImageConfig,
    ) -> crate::Result<()> {
        // 将 ImageConfig 编码为 metadata
        let config_json =
            crate::buildkit::grpc_client::build_frontend_attrs(image_config)
                .map_err(|e| anyhow::anyhow!("序列化 ImageConfig 失败: {e}"))?;

        let metadata: HashMap<String, Vec<u8>> = config_json
            .into_iter()
            .map(|(k, v)| (k, v.into_bytes()))
            .collect();

        let result = gateway::Result {
            metadata,
            attestations: HashMap::new(),
            result: Some(gateway::result::Result::Ref(gateway::Ref {
                id: String::new(),
                def: Some(definition),
            })),
        };

        self.client
            .r#return(gateway::ReturnRequest {
                result: Some(result),
                error: None,
            })
            .await
            .map_err(|e| anyhow::anyhow!("gateway Return failed: {e}"))?;

        Ok(())
    }
}

/// 校验构建上下文中的文件路径，拒绝路径穿越（..）和绝对路径
fn validate_entry_path(path: &str) -> crate::Result<()> {
    use std::path::Component;
    for component in std::path::Path::new(path).components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(anyhow::anyhow!("不安全的路径: {}", path).into());
            }
            _ => {}
        }
    }
    Ok(())
}

/// 计算数据的 SHA256 摘要（十六进制字符串）
fn sha2_digest(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// 执行 frontend 模式
///
/// 完整流程：连接 gateway → 同步构建上下文 → 检测 → BuildPlan → LLB → 返回结果
pub fn run_frontend(_args: &FrontendArgs) -> crate::Result<bool> {
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| anyhow::anyhow!("无法创建 tokio 运行时: {e}"))?;

    rt.block_on(async { run_frontend_async().await })
}

async fn run_frontend_async() -> crate::Result<bool> {
    use crate::buildkit::convert::{convert_plan_to_llb, ConvertPlanOptions};
    use crate::buildkit::platform::parse_platform_with_defaults;

    // 1. 连接 gateway
    let mut gateway = GatewayClient::from_env().await?;
    tracing::info!("已连接 BuildKit gateway");

    // 2. Ping 验证连接
    let _pong = gateway.ping().await?;
    tracing::info!("gateway ping 成功");

    // 3. 解析 frontend options（来自 BUILDKIT_FRONTEND_OPT_* 环境变量）
    let frontend_opts = parse_frontend_opts();
    let build_args = parse_build_args(&frontend_opts);
    tracing::debug!(?frontend_opts, ?build_args, "解析 frontend options");

    // 4. 根据 filename option 选择路径
    let plan = if frontend_opts.contains_key("filename") {
        // Plan-file 路径（对齐 railpack）
        let filename = frontend_opts
            .get("filename")
            .filter(|s| !s.is_empty())
            .map(|s| s.as_str())
            .unwrap_or("arcpack-plan.json");
        tracing::info!(filename, "从 dockerfile mount 读取 plan 文件");
        gateway.read_plan_file(filename).await?
    } else {
        // 检测路径（向后兼容）
        tracing::info!("未指定 filename，使用检测流水线");
        let context_ref = gateway.solve_context().await?;
        tracing::info!(context_ref = %context_ref, "获取构建上下文引用");

        let temp_dir = tempfile::tempdir()
            .map_err(|e| anyhow::anyhow!("创建临时目录失败: {e}"))?;
        gateway
            .sync_context_to_dir(&context_ref, temp_dir.path())
            .await?;
        tracing::info!(path = %temp_dir.path().display(), "构建上下文已同步");

        let source = temp_dir
            .path()
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("临时目录路径含非 UTF-8 字符"))?;

        let result = crate::generate_build_plan(
            source,
            HashMap::new(),
            &crate::GenerateBuildPlanOptions::default(),
        )?;

        result
            .plan
            .ok_or_else(|| anyhow::anyhow!("构建计划生成失败"))?
    };

    // 5. 从 build-args 构建转换选项
    let platform_str = build_args
        .get("platform")
        .or_else(|| frontend_opts.get("platform"))
        .map(|s| s.as_str())
        .unwrap_or("");
    let platform = parse_platform_with_defaults(platform_str)?;

    let opts = ConvertPlanOptions {
        secrets_hash: build_args.get("secrets-hash").cloned(),
        platform,
        cache_key: build_args
            .get("cache-key")
            .cloned()
            .unwrap_or_default(),
    };

    // 6. 转换为 LLB
    let llb_result = convert_plan_to_llb(&plan, &opts)?;

    // 7. 返回结果给 buildkitd
    gateway
        .return_result(llb_result.definition, &llb_result.image_config)
        .await?;
    tracing::info!("已返回 LLB 结果给 buildkitd");

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

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
    #[serial]
    fn test_frontend_no_env_returns_error() {
        // 保存原值并在测试结束时恢复
        let original = std::env::var("BUILDKIT_FRONTEND_ADDR").ok();
        // SAFETY: 通过 #[serial] 确保不会与其他测试并发修改环境变量
        unsafe { std::env::remove_var("BUILDKIT_FRONTEND_ADDR") };

        let args = FrontendArgs {};
        let result = run_frontend(&args);

        // 恢复原值
        if let Some(val) = original {
            unsafe { std::env::set_var("BUILDKIT_FRONTEND_ADDR", val) };
        }

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("BUILDKIT_FRONTEND_ADDR"),
            "错误信息应提及环境变量: {err_msg}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_gateway_client_from_env_missing_var() {
        // 保存原值
        let original = std::env::var("BUILDKIT_FRONTEND_ADDR").ok();
        // SAFETY: 通过 #[serial] 确保不会与其他测试并发修改环境变量
        unsafe { std::env::remove_var("BUILDKIT_FRONTEND_ADDR") };

        let result = GatewayClient::from_env().await;

        // 恢复原值
        if let Some(val) = original {
            unsafe { std::env::set_var("BUILDKIT_FRONTEND_ADDR", val) };
        }

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("BUILDKIT_FRONTEND_ADDR"),
            "错误信息应提及环境变量: {err_msg}"
        );
    }

    #[tokio::test]
    async fn test_gateway_client_connect_invalid_addr() {
        // 连接不存在的地址应返回错误
        let result = GatewayClient::connect("unix:///tmp/nonexistent-gateway.sock").await;
        assert!(result.is_err(), "连接不存在的地址应失败");
    }

    #[test]
    fn test_sha2_digest() {
        let data = b"hello world";
        let digest = sha2_digest(data);
        // SHA256("hello world") 的已知值
        assert_eq!(
            digest,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_path_traversal_parent_rejected() {
        let result = validate_entry_path("../etc/passwd");
        assert!(result.is_err(), "../etc/passwd 应被拒绝");
    }

    #[test]
    fn test_path_traversal_absolute_rejected() {
        let result = validate_entry_path("/etc/passwd");
        assert!(result.is_err(), "/etc/passwd 应被拒绝");
    }

    #[test]
    fn test_path_with_double_dots_in_name_accepted() {
        // foo..bar 不含 ParentDir 组件，应被接受
        let result = validate_entry_path("foo..bar");
        assert!(result.is_ok(), "foo..bar 应被接受: {:?}", result.err());
    }

    #[test]
    #[serial]
    fn test_parse_frontend_opts_reads_env() {
        // 保存原有的 BUILDKIT_FRONTEND_OPT_* 环境变量
        let original: Vec<_> = std::env::vars()
            .filter(|(k, _)| k.starts_with("BUILDKIT_FRONTEND_OPT_"))
            .collect();
        for (k, _) in &original {
            unsafe { std::env::remove_var(k) };
        }

        unsafe {
            std::env::set_var("BUILDKIT_FRONTEND_OPT_filename", "plan.json");
            std::env::set_var("BUILDKIT_FRONTEND_OPT_platform", "linux/arm64");
            std::env::set_var("BUILDKIT_FRONTEND_OPT_build-arg:cache-key", "mykey");
        }

        let opts = parse_frontend_opts();

        // 清理测试变量
        unsafe {
            std::env::remove_var("BUILDKIT_FRONTEND_OPT_filename");
            std::env::remove_var("BUILDKIT_FRONTEND_OPT_platform");
            std::env::remove_var("BUILDKIT_FRONTEND_OPT_build-arg:cache-key");
        }
        // 恢复原值
        for (k, v) in &original {
            unsafe { std::env::set_var(k, v) };
        }

        assert_eq!(opts.get("filename").map(String::as_str), Some("plan.json"));
        assert_eq!(
            opts.get("platform").map(String::as_str),
            Some("linux/arm64")
        );
        assert_eq!(
            opts.get("build-arg:cache-key").map(String::as_str),
            Some("mykey")
        );
    }

    #[test]
    fn test_parse_build_args_extracts_prefix() {
        let opts = HashMap::from([
            ("build-arg:secrets-hash".to_string(), "abc123".to_string()),
            ("build-arg:cache-key".to_string(), "mykey".to_string()),
            ("platform".to_string(), "linux/amd64".to_string()),
            ("filename".to_string(), "plan.json".to_string()),
        ]);
        let args = parse_build_args(&opts);
        assert_eq!(args.len(), 2);
        assert_eq!(
            args.get("secrets-hash").map(String::as_str),
            Some("abc123")
        );
        assert_eq!(args.get("cache-key").map(String::as_str), Some("mykey"));
    }

    #[test]
    fn test_parse_build_args_empty_when_no_prefix() {
        let opts = HashMap::from([
            ("platform".to_string(), "linux/amd64".to_string()),
            ("filename".to_string(), "plan.json".to_string()),
        ]);
        let args = parse_build_args(&opts);
        assert!(args.is_empty());
    }

    #[test]
    fn test_file_info_is_dir_detection() {
        // 目录 mode: 0o040755
        let dir_mode = 0o040755u32;
        let is_dir = (dir_mode & 0o170000) == 0o040000;
        assert!(is_dir, "0o040755 应被识别为目录");

        // 普通文件 mode: 0o100644
        let file_mode = 0o100644u32;
        let is_dir = (file_mode & 0o170000) == 0o040000;
        assert!(!is_dir, "0o100644 应被识别为普通文件");

        // 符号链接 mode: 0o120777
        let link_mode = 0o120777u32;
        let is_dir = (link_mode & 0o170000) == 0o040000;
        assert!(!is_dir, "0o120777 应被识别为符号链接（非目录）");
    }
}
