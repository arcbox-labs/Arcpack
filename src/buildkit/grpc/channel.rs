use anyhow::{bail, Context, Result};
use tonic::transport::{Channel, Endpoint};

/// 创建连接 buildkitd 的 gRPC Channel
/// 支持 Unix socket（unix://）和 TCP（tcp:// / http://）两种模式
pub async fn create_channel(addr: &str) -> Result<Channel> {
    if addr.starts_with("unix://") {
        create_unix_channel(addr).await
    } else if addr.starts_with("tcp://") || addr.starts_with("http://") {
        create_tcp_channel(addr).await
    } else {
        bail!("unsupported address scheme: {addr} (expected unix:// or tcp://)")
    }
}

/// Unix socket 连接：通过 tower::service_fn 适配 tonic Endpoint
async fn create_unix_channel(addr: &str) -> Result<Channel> {
    let socket_path = addr
        .strip_prefix("unix://")
        .context("invalid unix address")?
        .to_string();

    // tonic 不原生支持 Unix socket，通过 connect_with_connector 自定义 transport
    // dummy URI 仅满足 Endpoint 解析要求，实际连接由 connector 处理
    let channel = Endpoint::try_from("http://[::]:50051")?
        .connect_with_connector(tower::service_fn(move |_: tonic::transport::Uri| {
            let path = socket_path.clone();
            async move {
                let stream = tokio::net::UnixStream::connect(path).await?;
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
            }
        }))
        .await
        .context("failed to connect to unix socket")?;

    Ok(channel)
}

/// TCP 连接：标准 tonic Endpoint
async fn create_tcp_channel(addr: &str) -> Result<Channel> {
    // tcp://host:port → http://host:port（tonic 需要 http scheme）
    let http_addr = if addr.starts_with("tcp://") {
        addr.replacen("tcp://", "http://", 1)
    } else {
        addr.to_string()
    };

    let channel = Endpoint::try_from(http_addr)?
        .connect()
        .await
        .context("failed to connect to TCP endpoint")?;

    Ok(channel)
}

/// 从地址中提取 Unix socket 路径（用于内部工具）
pub fn parse_unix_path(addr: &str) -> Option<&str> {
    addr.strip_prefix("unix://")
}

/// 从地址中提取 TCP endpoint（用于内部工具）
pub fn parse_tcp_endpoint(addr: &str) -> Option<String> {
    if addr.starts_with("tcp://") {
        Some(addr.replacen("tcp://", "http://", 1))
    } else if addr.starts_with("http://") {
        Some(addr.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_unix_addr() {
        let path = parse_unix_path("unix:///run/buildkit/buildkitd.sock");
        assert_eq!(path, Some("/run/buildkit/buildkitd.sock"));
    }

    #[test]
    fn test_parse_unix_addr_custom_path() {
        let path = parse_unix_path("unix:///tmp/my-buildkitd.sock");
        assert_eq!(path, Some("/tmp/my-buildkitd.sock"));
    }

    #[test]
    fn test_parse_tcp_addr() {
        let endpoint = parse_tcp_endpoint("tcp://localhost:1234");
        assert_eq!(endpoint, Some("http://localhost:1234".to_string()));
    }

    #[test]
    fn test_parse_http_addr() {
        let endpoint = parse_tcp_endpoint("http://10.0.0.1:9000");
        assert_eq!(endpoint, Some("http://10.0.0.1:9000".to_string()));
    }

    #[test]
    fn test_parse_unix_not_tcp() {
        let endpoint = parse_tcp_endpoint("unix:///run/buildkit/buildkitd.sock");
        assert_eq!(endpoint, None);
    }

    #[test]
    fn test_parse_tcp_not_unix() {
        let path = parse_unix_path("tcp://localhost:1234");
        assert_eq!(path, None);
    }

    #[tokio::test]
    async fn test_invalid_addr_returns_error() {
        let result = create_channel("ftp://example.com").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unsupported address scheme"),
            "unexpected error: {err}"
        );
    }
}
