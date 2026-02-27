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

/// Frontend 命令参数（无用户参数——所有配置通过 buildkitd 的 frontend opts 传入）
#[derive(Debug, clap::Args)]
pub struct FrontendArgs {}

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
        // 构造 local://context source op 的 Definition
        use prost::Message;
        let source_op = pb::Op {
            inputs: vec![],
            op: Some(pb::op::Op::Source(pb::SourceOp {
                identifier: "local://context".to_string(),
                attrs: HashMap::new(),
            })),
            platform: None,
            constraints: None,
        };
        let source_bytes = source_op.encode_to_vec();

        // terminal op 引用 source op
        let terminal = pb::Op {
            inputs: vec![pb::Input {
                digest: format!(
                    "sha256:{}",
                    sha2_digest(&source_bytes)
                ),
                index: 0,
            }],
            op: None,
            platform: None,
            constraints: None,
        };
        let terminal_bytes = terminal.encode_to_vec();

        let definition = pb::Definition {
            def: vec![source_bytes.clone(), terminal_bytes],
            metadata: {
                let mut m = HashMap::new();
                m.insert(
                    format!("sha256:{}", sha2_digest(&source_bytes)),
                    pb::OpMetadata {
                        description: HashMap::from([(
                            "llb.customname".to_string(),
                            "load build context".to_string(),
                        )]),
                        ..Default::default()
                    },
                );
                m
            },
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
            .map_err(|e| anyhow::anyhow!("gateway Solve (context) failed: {e}"))?;

        // 从响应中提取引用 ID
        let result = resp
            .into_inner()
            .result
            .ok_or_else(|| anyhow::anyhow!("gateway Solve returned no result"))?;

        match result.result {
            Some(gateway::result::Result::Ref(r)) => Ok(r.id),
            Some(gateway::result::Result::RefDeprecated(id)) => Ok(id),
            _ => Err(anyhow::anyhow!(
                "gateway Solve returned unexpected result type"
            )
            .into()),
        }
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

    // 3. Solve 获取构建上下文引用
    let context_ref = gateway.solve_context().await?;
    tracing::info!(context_ref = %context_ref, "获取构建上下文引用");

    // 4. 同步上下文到临时目录
    let temp_dir = tempfile::tempdir()
        .map_err(|e| anyhow::anyhow!("创建临时目录失败: {e}"))?;
    gateway
        .sync_context_to_dir(&context_ref, temp_dir.path())
        .await?;
    tracing::info!(path = %temp_dir.path().display(), "构建上下文已同步");

    // 5. 执行标准检测 → BuildPlan 流水线
    let source = temp_dir
        .path()
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("临时目录路径含非 UTF-8 字符"))?;

    let result = crate::generate_build_plan(
        source,
        HashMap::new(),
        &crate::GenerateBuildPlanOptions::default(),
    )?;

    let plan = result
        .plan
        .ok_or_else(|| anyhow::anyhow!("构建计划生成失败"))?;

    // 6. 转换为 LLB
    let platform = parse_platform_with_defaults("")?;
    let opts = ConvertPlanOptions {
        secrets_hash: None,
        platform,
        cache_key: String::new(),
    };
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
