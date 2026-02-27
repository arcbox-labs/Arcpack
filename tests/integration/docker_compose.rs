/// Docker Compose 生命周期管理
///
/// 用于需要额外服务（数据库、Redis 等）的集成测试。

use std::process::Command;
use std::path::Path;

/// Docker Compose 管理器
#[allow(dead_code)]
pub struct ComposeManager {
    compose_file: String,
    project_name: String,
}

#[allow(dead_code)]
impl ComposeManager {
    /// 创建管理器
    pub fn new(fixture_path: &str, fixture_name: &str) -> Option<Self> {
        let compose_file = format!("{}/docker-compose.yml", fixture_path);
        if !Path::new(&compose_file).exists() {
            return None;
        }

        Some(Self {
            compose_file,
            project_name: format!("arcpack-test-{}", fixture_name),
        })
    }

    /// 启动所有服务
    pub fn up(&self) -> Result<(), String> {
        let output = Command::new("docker")
            .args([
                "compose",
                "-f", &self.compose_file,
                "-p", &self.project_name,
                "up", "-d",
            ])
            .output()
            .map_err(|e| format!("failed to run docker compose up: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("docker compose up failed: {}", stderr));
        }

        Ok(())
    }

    /// 停止并删除所有服务
    pub fn down(&self) {
        let _ = Command::new("docker")
            .args([
                "compose",
                "-f", &self.compose_file,
                "-p", &self.project_name,
                "down", "--volumes", "--remove-orphans",
            ])
            .output();
    }
}

impl Drop for ComposeManager {
    fn drop(&mut self) {
        self.down();
    }
}
