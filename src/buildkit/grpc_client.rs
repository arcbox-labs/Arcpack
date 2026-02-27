use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use tonic::transport::Channel;
use tracing::{debug, info, warn};

use crate::buildkit::client::BuildOutput;
use crate::buildkit::grpc::channel::create_channel;
use crate::buildkit::grpc::progress::{
    parse_status_response, render_plain, ProgressMode,
};
use crate::buildkit::grpc::session::filesync::FilesyncProvider;
use crate::buildkit::grpc::session::manager::SessionManager;
use crate::buildkit::grpc::session::secrets::SecretsProvider;
use crate::buildkit::grpc::solve::{self, ExportConfig, SolveConfig};
use crate::buildkit::image::ImageConfig;
use crate::buildkit::proto::control::control_client::ControlClient;
use crate::buildkit::proto::control::StatusRequest;
use crate::buildkit::proto::pb;

/// gRPC 构建客户端——通过 tonic 直连 buildkitd
///
/// 对齐 railpack `build.go:BuildWithBuildkitClient`
pub struct GrpcBuildKitClient {
    channel: Channel,
}

/// gRPC 构建请求
pub struct GrpcBuildRequest {
    /// LLB Definition（DAG 序列化结果）
    pub definition: pb::Definition,
    /// OCI Image 运行时配置
    pub image_config: ImageConfig,
    /// 构建上下文目录
    pub context_dir: PathBuf,
    /// 输出策略
    pub export: ExportConfig,
    /// Secret 键值对
    pub secrets: HashMap<String, String>,
    /// 本地目录映射（name → path）
    pub local_dirs: HashMap<String, PathBuf>,
    /// 进度渲染模式
    pub progress_mode: ProgressMode,
}

impl GrpcBuildKitClient {
    /// 创建 gRPC 客户端，连接 buildkitd
    pub async fn new(addr: &str) -> Result<Self> {
        let channel = create_channel(addr)
            .await
            .context("failed to create gRPC channel")?;
        Ok(Self { channel })
    }

    /// 从已有 Channel 构建（用于测试或共享连接）
    pub fn from_channel(channel: Channel) -> Self {
        Self { channel }
    }

    /// 完整 gRPC 构建流程
    ///
    /// 编排顺序（对齐 railpack `build.go:60-273`）：
    /// 1. SessionManager 注册 filesync + secrets provider
    /// 2. 启动 Session 后台 task（bidi stream）
    /// 3. 构造 SolveConfig（definition + exporter + session_id + frontend_attrs）
    /// 4. 启动进度监听后台 task（Status stream）
    /// 5. 发送 Solve RPC
    /// 6. 等待进度完成 + 停止 Session
    /// 7. 返回 BuildOutput
    pub async fn build(&self, request: GrpcBuildRequest) -> Result<BuildOutput> {
        let start = Instant::now();

        // 1. 创建 Session Manager，注册 providers
        // session service 路由尚未实现，提前警告用户
        if !request.secrets.is_empty() || !request.local_dirs.is_empty() {
            warn!(
                "session service routing not yet implemented; \
                 secrets and local_dirs are registered but buildkitd callbacks will not be served"
            );
        }

        let session = SessionManager::new()
            .with_filesync(FilesyncProvider::new(request.local_dirs))
            .with_secrets(SecretsProvider::new(request.secrets));

        let session_id = session.session_id().to_string();
        debug!(session_id = %session_id, "session created");

        // 2. 启动 Session 后台 task
        let session_handle = session.run(self.channel.clone());

        // 3. 构造 SolveRequest（只构造一次，保证 ref 一致性）
        let frontend_attrs = build_frontend_attrs(&request.image_config)?;
        let config = SolveConfig {
            definition: request.definition,
            exporter: request.export,
            session_id: Some(session_id.clone()),
            frontend_attrs,
        };
        let solve_request = solve::build_solve_request(&config)?;
        let progress_ref = solve_request.r#ref.clone();

        // 4. 先建立进度订阅（消除竞态：stream 在 Solve 之前就绑定到 ref）
        let progress_mode = request.progress_mode;
        let progress_handle = {
            let mut status_client = ControlClient::new(self.channel.clone());
            let status_request = StatusRequest {
                r#ref: progress_ref,
            };
            match status_client.status(status_request).await {
                Ok(response) => {
                    let mut stream = response.into_inner();
                    tokio::spawn(async move {
                        while let Some(msg) =
                            tokio_stream::StreamExt::next(&mut stream).await
                        {
                            match msg {
                                Ok(status_resp) => {
                                    let events =
                                        parse_status_response(&status_resp);
                                    for event in &events {
                                        match progress_mode {
                                            ProgressMode::Quiet => {}
                                            _ => {
                                                let line = render_plain(event);
                                                info!("{line}");
                                            }
                                        }
                                    }
                                }
                                Err(status) => {
                                    if status.code() == tonic::Code::Cancelled {
                                        break;
                                    }
                                    debug!(
                                        error = %status,
                                        "progress stream error"
                                    );
                                    break;
                                }
                            }
                        }
                    })
                }
                Err(status) => {
                    debug!(error = %status, "failed to subscribe to progress");
                    // 订阅失败时 spawn 空 task，保持类型一致
                    tokio::spawn(async {})
                }
            }
        };

        // 5. 发送 Solve RPC（Status stream 已就绪）
        let mut client = ControlClient::new(self.channel.clone());
        let solve_result = solve::solve(&mut client, solve_request).await;

        // 6. 无论成功失败，都清理后台 task
        if progress_handle.is_finished() {
            if let Err(e) = progress_handle.await {
                debug!(error = %e, "progress task ended abnormally");
            }
        } else {
            progress_handle.abort();
            debug!("progress task aborted (normal cleanup)");
        }

        // 检查 session 状态：已结束则收集错误，否则 abort
        if session_handle.is_finished() {
            match session_handle.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    warn!(error = %e, "session ended with error");
                }
                Err(join_err) => {
                    warn!(error = %join_err, "session task panicked or was cancelled");
                }
            }
        } else {
            session_handle.abort();
        }

        let result = solve_result.context("build failed")?;

        let image_digest = result
            .exporter_response
            .get("containerimage.digest")
            .cloned();

        info!(
            digest = image_digest.as_deref().unwrap_or("none"),
            elapsed = ?start.elapsed(),
            "build completed"
        );

        Ok(BuildOutput {
            image_digest,
            duration: start.elapsed(),
        })
    }
}

/// 构造 frontend_attrs（传递 OCI Image Config）
///
/// 对齐 Go `ExporterImageConfigKey = "containerimage.config"`
pub fn build_frontend_attrs(config: &ImageConfig) -> Result<HashMap<String, String>> {
    let mut attrs = HashMap::new();

    // OCI Image Config JSON 编码
    let image_config_json = serialize_image_config(config)?;
    attrs.insert(
        "containerimage.config".to_string(),
        image_config_json,
    );

    Ok(attrs)
}

/// 将 ImageConfig 序列化为 OCI 规范的 JSON 格式
fn serialize_image_config(config: &ImageConfig) -> Result<String> {
    // 构造 OCI Image Config 结构
    // 对齐 Go `ocispec.ImageConfig`
    let mut oci_config = serde_json::Map::new();

    if !config.env.is_empty() {
        oci_config.insert(
            "Env".to_string(),
            serde_json::Value::Array(
                config
                    .env
                    .iter()
                    .map(|e| serde_json::Value::String(e.clone()))
                    .collect(),
            ),
        );
    }

    if !config.working_dir.is_empty() {
        oci_config.insert(
            "WorkingDir".to_string(),
            serde_json::Value::String(config.working_dir.clone()),
        );
    }

    if !config.entrypoint.is_empty() {
        oci_config.insert(
            "Entrypoint".to_string(),
            serde_json::Value::Array(
                config
                    .entrypoint
                    .iter()
                    .map(|e| serde_json::Value::String(e.clone()))
                    .collect(),
            ),
        );
    }

    if !config.cmd.is_empty() {
        oci_config.insert(
            "Cmd".to_string(),
            serde_json::Value::Array(
                config
                    .cmd
                    .iter()
                    .map(|c| serde_json::Value::String(c.clone()))
                    .collect(),
            ),
        );
    }

    // 包裹在 { "config": { ... } } 中
    let mut root = serde_json::Map::new();
    root.insert(
        "config".to_string(),
        serde_json::Value::Object(oci_config),
    );

    serde_json::to_string(&root).context("failed to serialize image config")
}

/// 从 GrpcBuildRequest 的字段推导 ExportConfig
///
/// 当 image_name 和 output_dir 都为 None 时返回错误，
/// 避免生成空名称的 Image 导致 buildkitd 行为不确定。
pub fn build_export_config(
    image_name: Option<&str>,
    output_dir: Option<&PathBuf>,
    push: bool,
) -> Result<ExportConfig> {
    if let Some(dir) = output_dir {
        return Ok(ExportConfig::Local {
            dest: dir.clone(),
        });
    }

    if let Some(name) = image_name {
        return Ok(ExportConfig::Image {
            name: name.to_string(),
            push,
        });
    }

    anyhow::bail!("either image_name or output_dir must be specified for export config")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buildkit::platform::Platform;

    fn make_image_config(
        env: &[&str],
        working_dir: &str,
        entrypoint: &[&str],
        cmd: &[&str],
    ) -> ImageConfig {
        ImageConfig {
            env: env.iter().map(|s| s.to_string()).collect(),
            working_dir: working_dir.to_string(),
            entrypoint: entrypoint.iter().map(|s| s.to_string()).collect(),
            cmd: cmd.iter().map(|s| s.to_string()).collect(),
            platform: Platform {
                os: "linux".to_string(),
                arch: "amd64".to_string(),
                variant: None,
            },
        }
    }

    #[test]
    fn test_build_frontend_attrs_has_config_key() {
        let config = make_image_config(&["PATH=/usr/bin"], "/app", &[], &[]);
        let attrs = build_frontend_attrs(&config).unwrap();
        assert!(attrs.contains_key("containerimage.config"));
    }

    #[test]
    fn test_build_frontend_attrs_json_contains_env() {
        let config = make_image_config(
            &["PATH=/usr/bin", "NODE_ENV=production"],
            "/app",
            &[],
            &[],
        );
        let attrs = build_frontend_attrs(&config).unwrap();
        let json = attrs.get("containerimage.config").unwrap();

        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        let env = parsed["config"]["Env"].as_array().unwrap();
        assert_eq!(env.len(), 2);
        assert_eq!(env[0], "PATH=/usr/bin");
        assert_eq!(env[1], "NODE_ENV=production");
    }

    #[test]
    fn test_build_frontend_attrs_json_contains_workdir() {
        let config = make_image_config(&[], "/app", &[], &[]);
        let attrs = build_frontend_attrs(&config).unwrap();
        let json = attrs.get("containerimage.config").unwrap();

        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(parsed["config"]["WorkingDir"], "/app");
    }

    #[test]
    fn test_build_frontend_attrs_json_contains_entrypoint() {
        let config =
            make_image_config(&[], "", &["/bin/bash", "-c"], &["node server.js"]);
        let attrs = build_frontend_attrs(&config).unwrap();
        let json = attrs.get("containerimage.config").unwrap();

        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        let ep = parsed["config"]["Entrypoint"].as_array().unwrap();
        assert_eq!(ep, &["/bin/bash", "-c"]);

        let cmd = parsed["config"]["Cmd"].as_array().unwrap();
        assert_eq!(cmd, &["node server.js"]);
    }

    #[test]
    fn test_build_frontend_attrs_empty_config() {
        let config = make_image_config(&[], "", &[], &[]);
        let attrs = build_frontend_attrs(&config).unwrap();
        let json = attrs.get("containerimage.config").unwrap();

        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        // 空 config 不应有 Env/Entrypoint/Cmd 字段
        assert!(parsed["config"]["Env"].is_null());
        assert!(parsed["config"]["Entrypoint"].is_null());
        assert!(parsed["config"]["Cmd"].is_null());
    }

    #[test]
    fn test_build_export_config_image_push() {
        let config = build_export_config(Some("myapp:latest"), None, true).unwrap();
        match config {
            ExportConfig::Image { name, push } => {
                assert_eq!(name, "myapp:latest");
                assert!(push);
            }
            other => panic!("expected Image, got: {other:?}"),
        }
    }

    #[test]
    fn test_build_export_config_image_no_push() {
        let config = build_export_config(Some("myapp:v1"), None, false).unwrap();
        match config {
            ExportConfig::Image { name, push } => {
                assert_eq!(name, "myapp:v1");
                assert!(!push);
            }
            other => panic!("expected Image, got: {other:?}"),
        }
    }

    #[test]
    fn test_build_export_config_local() {
        let dest = PathBuf::from("/tmp/output");
        let config = build_export_config(None, Some(&dest), false).unwrap();
        match config {
            ExportConfig::Local { dest } => {
                assert_eq!(dest, PathBuf::from("/tmp/output"));
            }
            other => panic!("expected Local, got: {other:?}"),
        }
    }

    #[test]
    fn test_build_export_config_local_overrides_image() {
        // 同时指定 output_dir 和 image_name 时，local 优先
        let dest = PathBuf::from("/tmp/output");
        let config = build_export_config(Some("myapp"), Some(&dest), true).unwrap();
        match config {
            ExportConfig::Local { dest } => {
                assert_eq!(dest, PathBuf::from("/tmp/output"));
            }
            other => panic!("expected Local, got: {other:?}"),
        }
    }

    #[test]
    fn test_build_export_config_none_none_returns_error() {
        // 无 image_name 无 output_dir → 应返回错误
        let result = build_export_config(None, None, false);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("image_name"));
        assert!(msg.contains("output_dir"));
    }

    #[test]
    fn test_serialize_image_config_valid_json() {
        let config = make_image_config(
            &["PATH=/usr/bin"],
            "/app",
            &["/bin/sh"],
            &["start"],
        );
        let json = serialize_image_config(&config).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_object());
        assert!(parsed["config"].is_object());
    }
}
