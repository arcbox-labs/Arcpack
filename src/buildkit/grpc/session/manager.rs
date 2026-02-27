use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use tokio::task::JoinHandle;
use tonic::transport::Channel;
use tracing::debug;

use crate::buildkit::proto::control::control_client::ControlClient;
use crate::buildkit::proto::control::BytesMessage;

use super::filesync::FilesyncProvider;
use super::secrets::SecretsProvider;

/// Session 管理器——编排 buildkitd 的 Session 双向流
///
/// Session 协议核心：客户端调用 Control.Session() bidi stream，
/// buildkitd 在此 stream 内反转角色，回调客户端的 FilesyncProvider / SecretsProvider。
///
/// 对齐 railpack `build.go` 中的 session 创建逻辑。
pub struct SessionManager {
    session_id: String,
    secrets: Option<SecretsProvider>,
    filesync: Option<FilesyncProvider>,
}

impl SessionManager {
    /// 创建新的 Session，生成唯一 session_id
    pub fn new() -> Self {
        Self {
            session_id: generate_session_id(),
            secrets: None,
            filesync: None,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 注册 SecretsProvider
    pub fn with_secrets(mut self, provider: SecretsProvider) -> Self {
        self.secrets = Some(provider);
        self
    }

    /// 注册 FilesyncProvider
    pub fn with_filesync(mut self, provider: FilesyncProvider) -> Self {
        self.filesync = Some(provider);
        self
    }

    /// 启动 Session 后台 task
    ///
    /// 连接 buildkitd 的 Control.Session() bidi stream，
    /// 在 stream 内响应 buildkitd 的服务回调。
    ///
    /// ⚠️ Session 协议实现复杂度高（HTTP/2 连接劫持），
    /// 当前版本使用简化实现，后续可能需要迁移到底层 hyper API。
    pub fn run(self, channel: Channel) -> JoinHandle<Result<()>> {
        let session_id = self.session_id.clone();
        tokio::spawn(async move {
            debug!(session_id = %session_id, "starting session");

            let mut client = ControlClient::new(channel);

            // 创建 Session bidi stream
            // 发送初始握手消息（携带 session_id 作为 gRPC metadata）
            let request = tonic::Request::new(tokio_stream::empty::<BytesMessage>());

            // 注入 session_id 到 gRPC metadata，buildkitd 通过此关联 Solve 请求
            let mut request = request;
            request.metadata_mut().insert(
                "x-docker-expose-session-uuid",
                session_id.parse().map_err(|_| {
                    anyhow::anyhow!("invalid session_id for gRPC metadata: {session_id}")
                })?,
            );

            // Session 名称（对齐 Go 实现的 session.NewSession 参数）
            request.metadata_mut().insert(
                "x-docker-expose-session-name",
                "arcpack".parse().map_err(|_| {
                    anyhow::anyhow!("invalid session name for gRPC metadata")
                })?,
            );

            // Session 方法声明（x-docker-expose-session-grpc-method）暂未注入。
            // 原因：当前 bidi stream 仅保持连接活跃，尚未实现 HTTP/2 反转 +
            // gRPC server 路由，声明方法会导致 buildkitd 回调无响应。
            // 等消息路由实现后再恢复 Secrets/FileSync 声明。

            match client.session(request).await {
                Ok(response) => {
                    let mut stream = response.into_inner();
                    // 持续读取 stream 直到 buildkitd 关闭
                    // 实际的 service 路由需要将 stream 适配为 gRPC server
                    // 当前简化实现：仅保持连接活跃
                    while let Some(msg) = tokio_stream::StreamExt::next(&mut stream).await {
                        match msg {
                            Ok(_bytes_msg) => {
                                // TODO: 将 bidi stream 消息路由到对应的 service handler
                                // 完整实现需将 stream 适配为 HTTP/2 连接，
                                // 然后在其上运行 tonic server
                                debug!("session received message from buildkitd");
                            }
                            Err(status) => {
                                if status.code() == tonic::Code::Cancelled {
                                    debug!("session stream cancelled (normal shutdown)");
                                    break;
                                }
                                anyhow::bail!("session stream error: {status}");
                            }
                        }
                    }
                    debug!(session_id = %session_id, "session ended");
                }
                Err(status) => {
                    anyhow::bail!("failed to establish session: {status}");
                }
            }

            Ok(())
        })
    }
}

/// 生成唯一的 session ID（对齐 Go 的 identity.NewID）
fn generate_session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{pid:x}-{ts:x}-{seq:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn test_session_manager_unique_id() {
        let s1 = SessionManager::new();
        let s2 = SessionManager::new();
        assert_ne!(s1.session_id(), s2.session_id());
    }

    #[test]
    fn test_session_manager_id_not_empty() {
        let s = SessionManager::new();
        assert!(!s.session_id().is_empty());
    }

    #[test]
    fn test_session_manager_register_secrets() {
        let secrets = HashMap::from([("KEY".to_string(), "VAL".to_string())]);
        let s = SessionManager::new().with_secrets(SecretsProvider::new(secrets));
        assert!(s.secrets.is_some());
    }

    #[test]
    fn test_session_manager_register_filesync() {
        let dirs = HashMap::from([("context".to_string(), PathBuf::from("/app"))]);
        let s = SessionManager::new().with_filesync(FilesyncProvider::new(dirs));
        assert!(s.filesync.is_some());
    }

    #[test]
    fn test_session_manager_register_both() {
        let secrets = HashMap::from([("KEY".to_string(), "VAL".to_string())]);
        let dirs = HashMap::from([("context".to_string(), PathBuf::from("/app"))]);
        let s = SessionManager::new()
            .with_secrets(SecretsProvider::new(secrets))
            .with_filesync(FilesyncProvider::new(dirs));
        assert!(s.secrets.is_some());
        assert!(s.filesync.is_some());
    }

    #[test]
    fn test_session_manager_default_no_providers() {
        let s = SessionManager::new();
        assert!(s.secrets.is_none());
        assert!(s.filesync.is_none());
    }
}
