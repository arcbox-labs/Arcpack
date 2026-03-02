use std::time::Duration;

use async_trait::async_trait;

use crate::ArcpackError;

/// BuildKit 守护进程管理器
///
/// 对齐 railpack 的 BUILDKIT_HOST 机制 + arcpack 独有的子进程模式
#[async_trait]
pub trait DaemonManager: Send + Sync {
    /// 启动守护进程
    async fn start(&mut self) -> crate::Result<()>;
    /// 等待守护进程就绪（探测连接）
    async fn wait_ready(&self, timeout: Duration) -> crate::Result<()>;
    /// 停止守护进程
    async fn stop(&mut self) -> crate::Result<()>;
    /// 是否正在运行
    fn is_running(&self) -> bool;
    /// 返回 socket 地址
    fn socket_addr(&self) -> &str;
}

/// 探测连接就绪（支持 unix:// 和 tcp:// 协议）
async fn poll_socket_ready(addr: &str, timeout: Duration) -> crate::Result<()> {
    let start = std::time::Instant::now();
    let interval = Duration::from_millis(200);

    loop {
        if start.elapsed() > timeout {
            return Err(ArcpackError::DaemonTimeout {
                timeout_secs: timeout.as_secs(),
            });
        }

        let connected = if let Some(path) = addr.strip_prefix("unix://") {
            tokio::net::UnixStream::connect(path).await.is_ok()
        } else if let Some(host_port) = addr.strip_prefix("tcp://") {
            tokio::net::TcpStream::connect(host_port).await.is_ok()
        } else {
            return Err(ArcpackError::ConfigError {
                message: format!("不支持的 BuildKit 地址协议: {}", addr),
            });
        };

        if connected {
            return Ok(());
        }
        tokio::time::sleep(interval).await;
    }
}

/// 子进程模式 —— 由 arcpack 启动和管理 buildkitd
pub struct SubprocessDaemonManager {
    socket_path: String,
    addr: String,
    child: Option<tokio::process::Child>,
}

impl SubprocessDaemonManager {
    pub fn new(socket_path: impl Into<String>) -> Self {
        let socket_path = socket_path.into();
        let addr = format!("unix://{}", socket_path);
        Self {
            socket_path,
            addr,
            child: None,
        }
    }

    /// 生成默认 socket 路径
    pub fn default_socket_path() -> String {
        format!("/tmp/arcpack-buildkitd-{}.sock", std::process::id())
    }
}

#[async_trait]
impl DaemonManager for SubprocessDaemonManager {
    async fn start(&mut self) -> crate::Result<()> {
        // 清理旧 socket 文件
        let _ = std::fs::remove_file(&self.socket_path);

        let child = tokio::process::Command::new("buildkitd")
            .arg("--addr")
            .arg(&self.addr)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| ArcpackError::DaemonStartFailed {
                message: format!("无法启动 buildkitd: {}", e),
            })?;

        self.child = Some(child);
        Ok(())
    }

    async fn wait_ready(&self, timeout: Duration) -> crate::Result<()> {
        poll_socket_ready(&self.addr, timeout).await
    }

    async fn stop(&mut self) -> crate::Result<()> {
        if let Some(ref mut child) = self.child {
            // 先尝试 SIGTERM
            if let Some(pid) = child.id() {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid as i32),
                    nix::sys::signal::Signal::SIGTERM,
                );

                // 等待最多 5 秒
                let timeout = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;

                if timeout.is_err() {
                    // 超时则 SIGKILL
                    let _ = child.kill().await;
                }
            }
            self.child = None;
        }

        // 清理 socket 文件
        let _ = std::fs::remove_file(&self.socket_path);
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.child.is_some()
    }

    fn socket_addr(&self) -> &str {
        &self.addr
    }
}

/// 外部连接模式 —— 连接已运行的 buildkitd
pub struct ExternalDaemonManager {
    addr: String,
}

impl ExternalDaemonManager {
    pub fn new(addr: impl Into<String>) -> Self {
        Self { addr: addr.into() }
    }
}

#[async_trait]
impl DaemonManager for ExternalDaemonManager {
    async fn start(&mut self) -> crate::Result<()> {
        // 外部模式：no-op
        Ok(())
    }

    async fn wait_ready(&self, timeout: Duration) -> crate::Result<()> {
        poll_socket_ready(&self.addr, timeout).await
    }

    async fn stop(&mut self) -> crate::Result<()> {
        // 外部模式：no-op
        Ok(())
    }

    fn is_running(&self) -> bool {
        true // 外部 daemon 假设始终运行
    }

    fn socket_addr(&self) -> &str {
        &self.addr
    }
}

/// 根据 host 参数选择 DaemonManager 实现（纯函数，可安全测试）
fn select_daemon_manager_with_host(host: Option<&str>) -> Box<dyn DaemonManager> {
    if let Some(host) = host {
        Box::new(ExternalDaemonManager::new(host))
    } else {
        let socket_path = SubprocessDaemonManager::default_socket_path();
        Box::new(SubprocessDaemonManager::new(socket_path))
    }
}

/// 根据环境变量选择 DaemonManager 实现
///
/// BUILDKIT_HOST 存在 -> ExternalDaemonManager
/// 否则 -> SubprocessDaemonManager
pub fn select_daemon_manager() -> Box<dyn DaemonManager> {
    let host = std::env::var("BUILDKIT_HOST").ok();
    select_daemon_manager_with_host(host.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subprocess_new_constructs_addr() {
        let dm = SubprocessDaemonManager::new("/tmp/test.sock");
        assert_eq!(dm.socket_addr(), "unix:///tmp/test.sock");
        assert!(!dm.is_running());
    }

    #[test]
    fn test_subprocess_default_socket_path_contains_pid() {
        let path = SubprocessDaemonManager::default_socket_path();
        assert!(path.contains("arcpack-buildkitd"));
        assert!(path.ends_with(".sock"));
    }

    #[test]
    fn test_external_new_preserves_addr() {
        let dm = ExternalDaemonManager::new("unix:///run/buildkit/buildkitd.sock");
        assert_eq!(dm.socket_addr(), "unix:///run/buildkit/buildkitd.sock");
        assert!(dm.is_running()); // 外部 daemon 假设始终运行
    }

    #[test]
    fn test_select_daemon_without_env_returns_subprocess() {
        let dm = select_daemon_manager_with_host(None);
        assert!(dm.socket_addr().starts_with("unix://"));
    }

    #[test]
    fn test_select_daemon_with_env_returns_external() {
        let dm = select_daemon_manager_with_host(Some("unix:///custom/path.sock"));
        assert_eq!(dm.socket_addr(), "unix:///custom/path.sock");
    }

    #[tokio::test]
    async fn test_external_start_stop_are_noop() {
        let mut dm = ExternalDaemonManager::new("unix:///tmp/noop.sock");
        assert!(dm.start().await.is_ok());
        assert!(dm.stop().await.is_ok());
    }

    #[test]
    fn test_poll_socket_ready_rejects_unsupported_protocol() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(poll_socket_ready(
            "http://localhost:8080",
            Duration::from_millis(100),
        ));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("不支持的"));
    }
}
