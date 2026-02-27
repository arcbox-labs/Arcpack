/// Session 管理器——编排 buildkitd 的 Session 双向流
///
/// Session 协议核心：客户端调用 Control.Session() bidi stream，
/// buildkitd 在此 stream 内嵌套完整的 HTTP/2 连接并反转角色，
/// 回调客户端的 FileSyncProvider / SecretsProvider。
///
/// 实现对齐 Go `session/grpc.go` + `grpchijack/dial.go`：
/// 1. 建立 bidi stream，注入 session metadata
/// 2. 用 GrpcStreamIo 将 stream 适配为 AsyncRead/AsyncWrite
/// 3. 在适配器上启动 h2 server
/// 4. 根据 HTTP/2 请求的 :path 路由到服务 handler
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use bytes::Bytes;
use h2::server::SendResponse;
use h2::RecvStream;
use prost::Message;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;
use tracing::{debug, warn};

use crate::buildkit::proto::control::control_client::ControlClient;
use crate::buildkit::proto::control::BytesMessage;
use crate::buildkit::proto::secrets::{GetSecretRequest, GetSecretResponse};

use super::filesync::FilesyncProvider;
use super::grpc_frame::{decode_grpc_frame, encode_grpc_frame};
use super::secrets::SecretsProvider;
use super::stream_adapter::GrpcStreamIo;

/// Session 管理器
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

    /// 收集已注册服务的 gRPC method 列表
    ///
    /// buildkitd 通过 `x-docker-expose-session-grpc-method` header 得知
    /// 客户端能响应哪些服务回调。
    fn registered_methods(&self) -> Vec<&'static str> {
        let mut methods = Vec::new();
        if self.filesync.is_some() {
            methods.push("/moby.filesync.v1.FileSync/DiffCopy");
            methods.push("/moby.filesync.v1.FileSync/TarStream");
        }
        if self.secrets.is_some() {
            methods.push("/moby.buildkit.secrets.v1.Secrets/GetSecret");
        }
        methods
    }

    /// 启动 Session 后台 task
    ///
    /// 连接 buildkitd 的 Control.Session() bidi stream，
    /// 在 stream 内启动 HTTP/2 server 响应 buildkitd 的服务回调。
    pub fn run(self, channel: Channel) -> JoinHandle<Result<()>> {
        let session_id = self.session_id.clone();
        let methods = self.registered_methods();

        // 将 provider 包装为 Arc 供多个 handler task 共享
        let filesync = self.filesync.map(Arc::new);
        let secrets = self.secrets.map(Arc::new);

        tokio::spawn(async move {
            debug!(session_id = %session_id, "starting session");

            let mut client = ControlClient::new(channel);

            // 创建 outbound channel，作为 bidi stream 的客户端 → buildkitd 方向
            let (tx, rx) = mpsc::channel::<BytesMessage>(64);
            let outbound = ReceiverStream::new(rx);

            // 构造 request 并注入 session metadata
            let mut request = tonic::Request::new(outbound);
            inject_metadata(&mut request, &session_id, &methods)?;

            // 建立 bidi stream
            let response = client
                .session(request)
                .await
                .map_err(|s| anyhow::anyhow!("failed to establish session: {s}"))?;
            let inbound = response.into_inner();

            // 构建 IO 适配器：bidi stream → AsyncRead/AsyncWrite
            let io = GrpcStreamIo::new(inbound, tx);

            // 在适配器上启动 HTTP/2 server
            let mut conn = h2::server::handshake(io)
                .await
                .map_err(|e| anyhow::anyhow!("h2 server handshake failed: {e}"))?;

            debug!(session_id = %session_id, "h2 server handshake complete");

            // 接受并路由 HTTP/2 请求
            while let Some(result) = conn.accept().await {
                let (request, respond) = result
                    .map_err(|e| anyhow::anyhow!("h2 accept error: {e}"))?;

                let path = request.uri().path().to_string();
                debug!(path = %path, "session received h2 request");

                let fs = filesync.clone();
                let sec = secrets.clone();

                // 提取 dir-name header（用于 filesync 路由）
                let dir_name = request
                    .headers()
                    .get("dir-name")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("context")
                    .to_string();

                tokio::spawn(async move {
                    let result = route_request(&path, &dir_name, request.into_body(), respond, fs, sec).await;
                    if let Err(e) = result {
                        warn!(path = %path, error = %e, "session handler error");
                    }
                });
            }

            debug!(session_id = %session_id, "session ended");
            Ok(())
        })
    }
}

/// 注入 Session gRPC metadata
fn inject_metadata(
    request: &mut tonic::Request<ReceiverStream<BytesMessage>>,
    session_id: &str,
    methods: &[&str],
) -> Result<()> {
    // session UUID
    request.metadata_mut().insert(
        "x-docker-expose-session-uuid",
        session_id.parse().map_err(|_| {
            anyhow::anyhow!("invalid session_id for gRPC metadata: {session_id}")
        })?,
    );

    // session 名称
    request.metadata_mut().insert(
        "x-docker-expose-session-name",
        "arcpack"
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid session name for gRPC metadata"))?,
    );

    // 声明可用的 gRPC method（buildkitd 据此决定回调哪些服务）
    for method in methods {
        request.metadata_mut().append(
            "x-docker-expose-session-grpc-method",
            method.parse().map_err(|_| {
                anyhow::anyhow!("invalid method for gRPC metadata: {method}")
            })?,
        );
    }

    Ok(())
}

/// 根据 HTTP/2 请求 :path 路由到对应 handler
async fn route_request(
    path: &str,
    dir_name: &str,
    body: RecvStream,
    respond: SendResponse<Bytes>,
    filesync: Option<Arc<FilesyncProvider>>,
    secrets: Option<Arc<SecretsProvider>>,
) -> Result<()> {
    match path {
        "/moby.filesync.v1.FileSync/DiffCopy" => {
            let provider = filesync.ok_or_else(|| {
                anyhow::anyhow!("DiffCopy requested but no FilesyncProvider registered")
            })?;
            handle_diff_copy(dir_name, body, respond, &provider).await
        }
        "/moby.filesync.v1.FileSync/TarStream" => {
            // TarStream 暂不实现，返回 gRPC Unimplemented
            send_grpc_error(respond, 12, "TarStream not implemented")
        }
        "/moby.buildkit.secrets.v1.Secrets/GetSecret" => {
            let provider = secrets.ok_or_else(|| {
                anyhow::anyhow!("GetSecret requested but no SecretsProvider registered")
            })?;
            handle_get_secret(body, respond, &provider).await
        }
        _ => {
            debug!(path = %path, "unhandled session method, returning Unimplemented");
            send_grpc_error(respond, 12, &format!("unknown method: {path}"))
        }
    }
}

/// 处理 DiffCopy 请求——委托给 FilesyncProvider 的 DiffCopy 发送器
async fn handle_diff_copy(
    dir_name: &str,
    body: RecvStream,
    mut respond: SendResponse<Bytes>,
    provider: &FilesyncProvider,
) -> Result<()> {
    use super::filesync::DiffCopySender;

    let dir_path = provider.get_dir(dir_name).ok_or_else(|| {
        anyhow::anyhow!(
            "directory not registered: {dir_name} (available: {:?})",
            provider.dir_names()
        )
    })?;

    debug!(dir_name = dir_name, path = %dir_path.display(), "handling DiffCopy");

    // 发送 HTTP/2 200 响应头
    let response = http::Response::builder()
        .status(200)
        .header("content-type", "application/grpc")
        .body(())
        .map_err(|e| anyhow::anyhow!("failed to build response: {e}"))?;

    let mut send_stream = respond
        .send_response(response, false)
        .map_err(|e| anyhow::anyhow!("failed to send response headers: {e}"))?;

    // DiffCopy 协议：walk + 请求响应
    let mut sender = DiffCopySender::new(dir_path.clone());
    sender.run(body, &mut send_stream).await?;

    // 发送 gRPC trailers（status 0 = OK）
    let mut trailers = http::HeaderMap::new();
    trailers.insert("grpc-status", "0".parse().unwrap());
    send_stream
        .send_trailers(trailers)
        .map_err(|e| anyhow::anyhow!("failed to send trailers: {e}"))?;

    Ok(())
}

/// 处理 GetSecret 请求——解码 protobuf 请求，查找 secret，编码响应
async fn handle_get_secret(
    mut body: RecvStream,
    mut respond: SendResponse<Bytes>,
    provider: &SecretsProvider,
) -> Result<()> {
    // 读取请求 body（gRPC 帧）
    let mut request_data = Vec::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk.map_err(|e| anyhow::anyhow!("failed to read request body: {e}"))?;
        request_data.extend_from_slice(&chunk);
        // 释放 flow control 容量
        body.flow_control()
            .release_capacity(chunk.len())
            .map_err(|e| anyhow::anyhow!("flow control error: {e}"))?;
    }

    // 解码 gRPC 帧 → protobuf
    let payload = decode_grpc_frame(&request_data)?;
    let req = GetSecretRequest::decode(payload)
        .map_err(|e| anyhow::anyhow!("failed to decode GetSecretRequest: {e}"))?;

    debug!(secret_id = %req.id, "handling GetSecret");

    // 查找 secret
    match provider.get_secret(&req.id) {
        Some(value) => {
            let resp = GetSecretResponse {
                data: value.as_bytes().to_vec(),
            };
            let resp_bytes = resp.encode_to_vec();
            let frame = encode_grpc_frame(&resp_bytes);

            // 发送 HTTP/2 200 + gRPC 响应
            let response = http::Response::builder()
                .status(200)
                .header("content-type", "application/grpc")
                .body(())
                .map_err(|e| anyhow::anyhow!("failed to build response: {e}"))?;

            let mut send_stream = respond
                .send_response(response, false)
                .map_err(|e| anyhow::anyhow!("failed to send response headers: {e}"))?;

            send_stream
                .send_data(frame, false)
                .map_err(|e| anyhow::anyhow!("failed to send response data: {e}"))?;

            // gRPC OK trailers
            let mut trailers = http::HeaderMap::new();
            trailers.insert("grpc-status", "0".parse().unwrap());
            send_stream
                .send_trailers(trailers)
                .map_err(|e| anyhow::anyhow!("failed to send trailers: {e}"))?;

            Ok(())
        }
        None => {
            send_grpc_error(respond, 5, &format!("secret not found: {}", req.id))
        }
    }
}

/// 发送 gRPC 错误响应（通过 HTTP/2 trailers）
fn send_grpc_error(
    mut respond: SendResponse<Bytes>,
    grpc_status: u8,
    message: &str,
) -> Result<()> {
    let response = http::Response::builder()
        .status(200)
        .header("content-type", "application/grpc")
        .header("grpc-status", grpc_status.to_string())
        .header("grpc-message", message)
        .body(())
        .map_err(|e| anyhow::anyhow!("failed to build error response: {e}"))?;

    // end_of_stream = true，无 body
    respond
        .send_response(response, true)
        .map_err(|e| anyhow::anyhow!("failed to send error response: {e}"))?;

    Ok(())
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

    #[test]
    fn test_registered_methods_empty() {
        let s = SessionManager::new();
        assert!(s.registered_methods().is_empty());
    }

    #[test]
    fn test_registered_methods_filesync_only() {
        let dirs = HashMap::from([("context".to_string(), PathBuf::from("/app"))]);
        let s = SessionManager::new().with_filesync(FilesyncProvider::new(dirs));
        let methods = s.registered_methods();
        assert_eq!(methods.len(), 2);
        assert!(methods.contains(&"/moby.filesync.v1.FileSync/DiffCopy"));
        assert!(methods.contains(&"/moby.filesync.v1.FileSync/TarStream"));
    }

    #[test]
    fn test_registered_methods_secrets_only() {
        let secrets = HashMap::from([("K".to_string(), "V".to_string())]);
        let s = SessionManager::new().with_secrets(SecretsProvider::new(secrets));
        let methods = s.registered_methods();
        assert_eq!(methods.len(), 1);
        assert!(methods.contains(&"/moby.buildkit.secrets.v1.Secrets/GetSecret"));
    }

    #[test]
    fn test_registered_methods_both() {
        let secrets = HashMap::from([("K".to_string(), "V".to_string())]);
        let dirs = HashMap::from([("context".to_string(), PathBuf::from("/app"))]);
        let s = SessionManager::new()
            .with_secrets(SecretsProvider::new(secrets))
            .with_filesync(FilesyncProvider::new(dirs));
        let methods = s.registered_methods();
        assert_eq!(methods.len(), 3);
    }

    #[test]
    fn test_inject_metadata_sets_uuid() {
        let (tx, _rx) = mpsc::channel(1);
        let outbound = ReceiverStream::new(_rx);
        let mut request = tonic::Request::new(outbound);
        inject_metadata(&mut request, "test-session-id", &[]).unwrap();

        let uuid = request
            .metadata()
            .get("x-docker-expose-session-uuid")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(uuid, "test-session-id");
    }

    #[test]
    fn test_inject_metadata_sets_name() {
        let (tx, _rx) = mpsc::channel(1);
        let outbound = ReceiverStream::new(_rx);
        let mut request = tonic::Request::new(outbound);
        inject_metadata(&mut request, "id", &[]).unwrap();

        let name = request
            .metadata()
            .get("x-docker-expose-session-name")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(name, "arcpack");
    }

    #[test]
    fn test_inject_metadata_sets_methods() {
        let (tx, _rx) = mpsc::channel(1);
        let outbound = ReceiverStream::new(_rx);
        let mut request = tonic::Request::new(outbound);
        let methods = vec![
            "/moby.filesync.v1.FileSync/DiffCopy",
            "/moby.buildkit.secrets.v1.Secrets/GetSecret",
        ];
        inject_metadata(&mut request, "id", &methods).unwrap();

        let vals: Vec<_> = request
            .metadata()
            .get_all("x-docker-expose-session-grpc-method")
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect();
        assert_eq!(vals.len(), 2);
        assert!(vals.contains(&"/moby.filesync.v1.FileSync/DiffCopy".to_string()));
        assert!(vals.contains(&"/moby.buildkit.secrets.v1.Secrets/GetSecret".to_string()));
    }
}
