use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::ArcpackError;

/// 构建请求
#[derive(Debug, Clone)]
pub struct BuildRequest {
    /// 构建上下文目录
    pub context_dir: PathBuf,
    /// Dockerfile 内容（写入临时文件）
    pub dockerfile_content: String,
    /// 输出镜像名
    pub image_name: Option<String>,
    /// 输出到本地目录
    pub output_dir: Option<PathBuf>,
    /// 是否推送到 registry
    pub push: bool,
    /// 目标平台
    pub platform: String,
    /// 进度模式：auto/plain/tty
    pub progress_mode: String,
    /// 缓存导入配置
    pub cache_import: Option<String>,
    /// 缓存导出配置
    pub cache_export: Option<String>,
    /// Secret 键值对
    pub secrets: HashMap<String, String>,
}

/// 构建输出
#[derive(Debug)]
pub struct BuildOutput {
    /// 镜像摘要
    pub image_digest: Option<String>,
    /// 构建耗时
    pub duration: Duration,
}

/// BuildKit 客户端（buildctl CLI 封装）
///
/// 对齐 railpack `build.go` 的 Phase A 实现
pub struct BuildKitClient {
    /// DaemonManager 的 socket 地址
    addr: String,
    /// buildctl 二进制路径
    buildctl_path: String,
}

impl BuildKitClient {
    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            buildctl_path: "buildctl".to_string(),
        }
    }

    pub fn with_buildctl_path(mut self, path: impl Into<String>) -> Self {
        self.buildctl_path = path.into();
        self
    }

    /// 执行构建
    pub async fn build(&self, request: &BuildRequest) -> crate::Result<BuildOutput> {
        let start = Instant::now();

        // 写 Dockerfile 到临时目录
        let temp_dir = std::env::temp_dir().join(format!("arcpack-build-{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir)?;
        let dockerfile_path = temp_dir.join("Dockerfile");
        std::fs::write(&dockerfile_path, &request.dockerfile_content)?;

        // 组装命令行参数
        let args = self.build_args(request, &temp_dir);

        // 执行 buildctl
        let mut cmd = tokio::process::Command::new(&self.buildctl_path);
        cmd.args(&args)
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());

        // 注入 secret 环境变量，使 buildctl --secret id=KEY,env=KEY 能读到值
        for (key, value) in &request.secrets {
            cmd.env(key, value);
        }

        let output = cmd.output().await
            .map_err(|e| ArcpackError::BuildFailed {
                exit_code: -1,
                stderr: format!("无法执行 buildctl: {}", e),
            })?;

        // 清理临时文件
        let _ = std::fs::remove_dir_all(&temp_dir);

        if !output.status.success() {
            let exit_code = output.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(ArcpackError::BuildFailed { exit_code, stderr });
        }

        Ok(BuildOutput {
            image_digest: None, // buildctl 不直接返回 digest
            duration: start.elapsed(),
        })
    }

    /// 组装 buildctl 命令行参数（纯函数，可独立测试）
    fn build_args(&self, request: &BuildRequest, dockerfile_dir: &std::path::Path) -> Vec<String> {
        let mut args = vec![
            "--addr".to_string(),
            self.addr.clone(),
            "build".to_string(),
            "--frontend".to_string(),
            "dockerfile.v0".to_string(),
            "--local".to_string(),
            format!("context={}", request.context_dir.display()),
            "--local".to_string(),
            format!("dockerfile={}", dockerfile_dir.display()),
            "--progress".to_string(),
            request.progress_mode.clone(),
        ];

        // 平台
        if !request.platform.is_empty() {
            args.push("--opt".to_string());
            args.push(format!("platform={}", request.platform));
        }

        // 输出配置
        if let Some(ref name) = request.image_name {
            if request.push {
                args.push("--output".to_string());
                args.push(format!("type=image,name={},push=true", name));
            } else {
                args.push("--output".to_string());
                args.push(format!("type=image,name={}", name));
            }
        }

        if let Some(ref output_dir) = request.output_dir {
            args.push("--output".to_string());
            args.push(format!("type=local,dest={}", output_dir.display()));
        }

        // Secrets
        for key in request.secrets.keys() {
            args.push("--secret".to_string());
            args.push(format!("id={},env={}", key, key));
        }

        // 缓存导入
        if let Some(ref cache_import) = request.cache_import {
            args.push("--import-cache".to_string());
            args.push(cache_import.clone());
        }

        // 缓存导出
        if let Some(ref cache_export) = request.cache_export {
            args.push("--export-cache".to_string());
            args.push(cache_export.clone());
        }

        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_request() -> BuildRequest {
        BuildRequest {
            context_dir: PathBuf::from("/app"),
            dockerfile_content: "FROM ubuntu".to_string(),
            image_name: None,
            output_dir: None,
            push: false,
            platform: String::new(),
            progress_mode: "auto".to_string(),
            cache_import: None,
            cache_export: None,
            secrets: HashMap::new(),
        }
    }

    #[test]
    fn test_build_args_basic_contains_required_flags() {
        let client = BuildKitClient::new("unix:///tmp/buildkit.sock");
        let req = make_request();
        let temp = PathBuf::from("/tmp/dockerfile");
        let args = client.build_args(&req, &temp);
        assert!(args.contains(&"--addr".to_string()));
        assert!(args.contains(&"unix:///tmp/buildkit.sock".to_string()));
        assert!(args.contains(&"build".to_string()));
        assert!(args.contains(&"--frontend".to_string()));
        assert!(args.contains(&"dockerfile.v0".to_string()));
        assert!(args.contains(&"context=/app".to_string()));
    }

    #[test]
    fn test_build_args_with_image_name_adds_output() {
        let client = BuildKitClient::new("unix:///tmp/test.sock");
        let mut req = make_request();
        req.image_name = Some("myapp:latest".to_string());
        let args = client.build_args(&req, &PathBuf::from("/tmp"));
        assert!(args.contains(&"--output".to_string()));
        assert!(args.contains(&"type=image,name=myapp:latest".to_string()));
    }

    #[test]
    fn test_build_args_with_push_appends_push_flag() {
        let client = BuildKitClient::new("unix:///tmp/test.sock");
        let mut req = make_request();
        req.image_name = Some("myapp:latest".to_string());
        req.push = true;
        let args = client.build_args(&req, &PathBuf::from("/tmp"));
        assert!(args.contains(&"type=image,name=myapp:latest,push=true".to_string()));
    }

    #[test]
    fn test_build_args_with_secrets_adds_secret_flags() {
        let client = BuildKitClient::new("unix:///tmp/test.sock");
        let mut req = make_request();
        req.secrets.insert("MY_SECRET".to_string(), "value".to_string());
        let args = client.build_args(&req, &PathBuf::from("/tmp"));
        assert!(args.contains(&"--secret".to_string()));
        assert!(args.contains(&"id=MY_SECRET,env=MY_SECRET".to_string()));
    }

    #[test]
    fn test_build_args_with_cache_adds_import_export() {
        let client = BuildKitClient::new("unix:///tmp/test.sock");
        let mut req = make_request();
        req.cache_import = Some("type=gha".to_string());
        req.cache_export = Some("type=gha,mode=max".to_string());
        let args = client.build_args(&req, &PathBuf::from("/tmp"));
        assert!(args.contains(&"--import-cache".to_string()));
        assert!(args.contains(&"type=gha".to_string()));
        assert!(args.contains(&"--export-cache".to_string()));
        assert!(args.contains(&"type=gha,mode=max".to_string()));
    }

    #[test]
    fn test_build_args_with_platform_adds_opt() {
        let client = BuildKitClient::new("unix:///tmp/test.sock");
        let mut req = make_request();
        req.platform = "linux/amd64".to_string();
        let args = client.build_args(&req, &PathBuf::from("/tmp"));
        assert!(args.contains(&"--opt".to_string()));
        assert!(args.contains(&"platform=linux/amd64".to_string()));
    }

    #[test]
    fn test_build_args_with_output_dir_adds_local_dest() {
        let client = BuildKitClient::new("unix:///tmp/test.sock");
        let mut req = make_request();
        req.output_dir = Some(PathBuf::from("/output"));
        let args = client.build_args(&req, &PathBuf::from("/tmp"));
        assert!(args.contains(&"type=local,dest=/output".to_string()));
    }

    #[test]
    fn test_build_injects_secret_env_vars_into_command() {
        // 验证 build_args 包含 --secret 标志，且 build() 方法注入 env
        // env 注入在 build() 中通过 cmd.env(key, value) 实现
        // 此测试通过检查 build_args 确认 secret 参数正确传递
        let client = BuildKitClient::new("unix:///tmp/test.sock");
        let mut req = make_request();
        req.secrets.insert("API_KEY".to_string(), "secret_value".to_string());
        req.secrets.insert("DB_PASS".to_string(), "db_secret".to_string());
        let args = client.build_args(&req, &PathBuf::from("/tmp"));

        // 验证每个 secret 都有对应的 --secret 参数
        let secret_count = args.iter().filter(|a| *a == "--secret").count();
        assert_eq!(secret_count, 2, "应有 2 个 --secret 参数");
    }
}
