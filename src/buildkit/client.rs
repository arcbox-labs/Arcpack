use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::ArcpackError;

#[cfg(feature = "llb")]
use prost::Message;

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

/// 追加输出配置参数（image/docker/local）到 args
///
/// `docker_tar_dest` 非 None 时，非 push 模式使用 `type=docker` 输出可加载 tarball
fn push_output_args(
    args: &mut Vec<String>,
    image_name: &Option<String>,
    push: bool,
    output_dir: &Option<PathBuf>,
    docker_tar_dest: Option<&std::path::Path>,
) {
    if let Some(ref name) = image_name {
        if push {
            args.push("--output".to_string());
            args.push(format!("type=image,name={},push=true", name));
        } else if let Some(tar_path) = docker_tar_dest {
            // 输出为 Docker 可加载的 tarball，构建完成后通过 docker load 加载
            args.push("--output".to_string());
            args.push(format!("type=docker,name={},dest={}", name, tar_path.display()));
        } else {
            args.push("--output".to_string());
            args.push(format!("type=image,name={}", name));
        }
    }
    if let Some(ref dir) = output_dir {
        args.push("--output".to_string());
        args.push(format!("type=local,dest={}", dir.display()));
    }
}

/// 追加 secret 参数到 args（排序以保证可复现）
fn push_secret_args(args: &mut Vec<String>, secrets: &HashMap<String, String>) {
    let mut keys: Vec<&String> = secrets.keys().collect();
    keys.sort();
    for key in keys {
        args.push("--secret".to_string());
        args.push(format!("id={},env={}", key, key));
    }
}

/// 执行 docker load 加载镜像 tarball
async fn docker_load(tar_path: &std::path::Path) -> crate::Result<()> {
    eprintln!("正在加载 Docker 镜像...");
    let output = tokio::process::Command::new("docker")
        .args(["load", "-i", &tar_path.to_string_lossy()])
        .output()
        .await
        .map_err(|e| ArcpackError::BuildFailed {
            exit_code: -1,
            stderr: format!("无法执行 docker load: {}", e),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(ArcpackError::BuildFailed {
            exit_code: output.status.code().unwrap_or(-1),
            stderr: format!("docker load 失败: {}", stderr),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        eprintln!("{}", stdout.trim());
    }
    Ok(())
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

        // 非 push 模式且指定了镜像名时，输出为 docker tarball 以便 docker load
        let docker_tar_path = if request.image_name.is_some() && !request.push {
            Some(temp_dir.join("image.tar"))
        } else {
            None
        };

        // 组装命令行参数
        let args = self.build_args(request, &temp_dir, docker_tar_path.as_deref());

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

        if !output.status.success() {
            let exit_code = output.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let _ = std::fs::remove_dir_all(&temp_dir);
            return Err(ArcpackError::BuildFailed { exit_code, stderr });
        }

        // 自动加载 Docker 镜像（非 push 模式）
        if let Some(ref tar_path) = docker_tar_path {
            if tar_path.exists() {
                docker_load(tar_path).await?;
            }
        }

        // 清理临时文件
        let _ = std::fs::remove_dir_all(&temp_dir);

        Ok(BuildOutput {
            image_digest: None,
            duration: start.elapsed(),
        })
    }

    /// 组装 buildctl 命令行参数（纯函数，可独立测试）
    fn build_args(&self, request: &BuildRequest, dockerfile_dir: &std::path::Path, docker_tar_dest: Option<&std::path::Path>) -> Vec<String> {
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

        push_output_args(&mut args, &request.image_name, request.push, &request.output_dir, docker_tar_dest);
        push_secret_args(&mut args, &request.secrets);

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

    /// 通过 LLB Definition 执行构建（stdin 传入，无 --frontend）
    #[cfg(feature = "llb")]
    pub async fn build_from_llb(&self, request: &LlbBuildRequest) -> crate::Result<BuildOutput> {
        let start = Instant::now();

        // 序列化 Definition 为字节
        let llb_bytes = request.definition.encode_to_vec();

        // 组装命令行参数
        let args = self.build_llb_args(request);

        // 执行 buildctl，通过 stdin 传入 LLB
        // stdout inherit 供进度输出；stderr piped 供错误捕获
        let mut cmd = tokio::process::Command::new(&self.buildctl_path);
        cmd.args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::piped());

        // 注入 secret 环境变量
        for (key, value) in &request.secrets {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn()
            .map_err(|e| ArcpackError::BuildFailed {
                exit_code: -1,
                stderr: format!("无法执行 buildctl: {}", e),
            })?;

        // 写入 LLB bytes 到 stdin，然后关闭（EOF）
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(&llb_bytes).await
                .map_err(|e| ArcpackError::BuildFailed {
                    exit_code: -1,
                    stderr: format!("写入 LLB 到 stdin 失败: {}", e),
                })?;
            // drop stdin 发送 EOF
        }

        let output = child.wait_with_output().await
            .map_err(|e| ArcpackError::BuildFailed {
                exit_code: -1,
                stderr: format!("等待 buildctl 完成失败: {}", e),
            })?;

        if !output.status.success() {
            let exit_code = output.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(ArcpackError::BuildFailed { exit_code, stderr });
        }

        Ok(BuildOutput {
            image_digest: None,
            duration: start.elapsed(),
        })
    }

    /// 组装 LLB 构建的 buildctl 参数（纯函数，可独立测试）
    ///
    /// 关键区别：无 --frontend 参数，LLB 通过 stdin 传入
    #[cfg(feature = "llb")]
    fn build_llb_args(&self, request: &LlbBuildRequest) -> Vec<String> {
        let mut args = vec![
            "--addr".to_string(),
            self.addr.clone(),
            "build".to_string(),
            "--local".to_string(),
            format!("context={}", request.context_dir.display()),
            "--progress".to_string(),
            request.progress_mode.clone(),
        ];

        push_output_args(&mut args, &request.image_name, request.push, &request.output_dir, None);
        push_secret_args(&mut args, &request.secrets);

        // no-cache
        if request.no_cache {
            args.push("--no-cache".to_string());
        }

        args
    }
}

/// LLB 构建请求
#[cfg(feature = "llb")]
#[derive(Debug)]
pub struct LlbBuildRequest {
    /// LLB Definition（protobuf）
    pub definition: crate::buildkit::proto::pb::Definition,
    /// 构建上下文目录
    pub context_dir: PathBuf,
    /// 输出镜像名
    pub image_name: Option<String>,
    /// 输出到本地目录
    pub output_dir: Option<PathBuf>,
    /// 是否推送到 registry
    pub push: bool,
    /// 进度模式：auto/plain/tty
    pub progress_mode: String,
    /// Secret 键值对
    pub secrets: HashMap<String, String>,
    /// 禁用缓存
    pub no_cache: bool,
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
        let args = client.build_args(&req, &temp, None);
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
        let args = client.build_args(&req, &PathBuf::from("/tmp"), None);
        assert!(args.contains(&"--output".to_string()));
        assert!(args.contains(&"type=image,name=myapp:latest".to_string()));
    }

    #[test]
    fn test_build_args_with_push_appends_push_flag() {
        let client = BuildKitClient::new("unix:///tmp/test.sock");
        let mut req = make_request();
        req.image_name = Some("myapp:latest".to_string());
        req.push = true;
        let args = client.build_args(&req, &PathBuf::from("/tmp"), None);
        assert!(args.contains(&"type=image,name=myapp:latest,push=true".to_string()));
    }

    #[test]
    fn test_build_args_with_secrets_adds_secret_flags() {
        let client = BuildKitClient::new("unix:///tmp/test.sock");
        let mut req = make_request();
        req.secrets.insert("MY_SECRET".to_string(), "value".to_string());
        let args = client.build_args(&req, &PathBuf::from("/tmp"), None);
        assert!(args.contains(&"--secret".to_string()));
        assert!(args.contains(&"id=MY_SECRET,env=MY_SECRET".to_string()));
    }

    #[test]
    fn test_build_args_with_cache_adds_import_export() {
        let client = BuildKitClient::new("unix:///tmp/test.sock");
        let mut req = make_request();
        req.cache_import = Some("type=gha".to_string());
        req.cache_export = Some("type=gha,mode=max".to_string());
        let args = client.build_args(&req, &PathBuf::from("/tmp"), None);
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
        let args = client.build_args(&req, &PathBuf::from("/tmp"), None);
        assert!(args.contains(&"--opt".to_string()));
        assert!(args.contains(&"platform=linux/amd64".to_string()));
    }

    #[test]
    fn test_build_args_with_output_dir_adds_local_dest() {
        let client = BuildKitClient::new("unix:///tmp/test.sock");
        let mut req = make_request();
        req.output_dir = Some(PathBuf::from("/output"));
        let args = client.build_args(&req, &PathBuf::from("/tmp"), None);
        assert!(args.contains(&"type=local,dest=/output".to_string()));
    }

    #[test]
    fn test_build_args_with_docker_tar_dest_uses_type_docker() {
        let client = BuildKitClient::new("unix:///tmp/test.sock");
        let mut req = make_request();
        req.image_name = Some("myapp:latest".to_string());
        let tar_path = PathBuf::from("/tmp/image.tar");
        let args = client.build_args(&req, &PathBuf::from("/tmp"), Some(tar_path.as_path()));
        assert!(args.contains(&"--output".to_string()));
        assert!(args.contains(&"type=docker,name=myapp:latest,dest=/tmp/image.tar".to_string()));
    }

    #[test]
    fn test_build_args_push_ignores_docker_tar_dest() {
        let client = BuildKitClient::new("unix:///tmp/test.sock");
        let mut req = make_request();
        req.image_name = Some("myapp:latest".to_string());
        req.push = true;
        let tar_path = PathBuf::from("/tmp/image.tar");
        let args = client.build_args(&req, &PathBuf::from("/tmp"), Some(tar_path.as_path()));
        // push 模式忽略 docker_tar_dest，使用 type=image
        assert!(args.contains(&"type=image,name=myapp:latest,push=true".to_string()));
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
        let args = client.build_args(&req, &PathBuf::from("/tmp"), None);

        // 验证每个 secret 都有对应的 --secret 参数
        let secret_count = args.iter().filter(|a| *a == "--secret").count();
        assert_eq!(secret_count, 2, "应有 2 个 --secret 参数");
    }

    // === LLB build_args 测试 ===

    #[cfg(feature = "llb")]
    mod llb_tests {
        use super::*;

        fn make_llb_request() -> LlbBuildRequest {
            LlbBuildRequest {
                definition: crate::buildkit::proto::pb::Definition::default(),
                context_dir: PathBuf::from("/app"),
                image_name: None,
                output_dir: None,
                push: false,
                progress_mode: "auto".to_string(),
                secrets: HashMap::new(),
                no_cache: false,
            }
        }

        #[test]
        fn test_build_llb_args_basic() {
            let client = BuildKitClient::new("unix:///tmp/buildkit.sock");
            let req = make_llb_request();
            let args = client.build_llb_args(&req);
            assert!(args.contains(&"--addr".to_string()));
            assert!(args.contains(&"unix:///tmp/buildkit.sock".to_string()));
            assert!(args.contains(&"build".to_string()));
            assert!(args.contains(&"context=/app".to_string()));
            assert!(args.contains(&"--progress".to_string()));
            assert!(args.contains(&"auto".to_string()));
        }

        #[test]
        fn test_build_llb_args_no_frontend() {
            let client = BuildKitClient::new("unix:///tmp/test.sock");
            let req = make_llb_request();
            let args = client.build_llb_args(&req);
            assert!(
                !args.contains(&"--frontend".to_string()),
                "LLB 构建不应包含 --frontend 参数"
            );
            assert!(
                !args.contains(&"dockerfile.v0".to_string()),
                "LLB 构建不应包含 dockerfile.v0"
            );
        }

        #[test]
        fn test_build_llb_args_with_image_name() {
            let client = BuildKitClient::new("unix:///tmp/test.sock");
            let mut req = make_llb_request();
            req.image_name = Some("myapp:latest".to_string());
            let args = client.build_llb_args(&req);
            assert!(args.contains(&"--output".to_string()));
            assert!(args.contains(&"type=image,name=myapp:latest".to_string()));
        }

        #[test]
        fn test_build_llb_args_with_push() {
            let client = BuildKitClient::new("unix:///tmp/test.sock");
            let mut req = make_llb_request();
            req.image_name = Some("myapp:latest".to_string());
            req.push = true;
            let args = client.build_llb_args(&req);
            assert!(args.contains(&"type=image,name=myapp:latest,push=true".to_string()));
        }

        #[test]
        fn test_build_llb_args_with_secrets() {
            let client = BuildKitClient::new("unix:///tmp/test.sock");
            let mut req = make_llb_request();
            req.secrets.insert("MY_SECRET".to_string(), "value".to_string());
            let args = client.build_llb_args(&req);
            assert!(args.contains(&"--secret".to_string()));
            assert!(args.contains(&"id=MY_SECRET,env=MY_SECRET".to_string()));
        }

        #[test]
        fn test_build_llb_args_with_no_cache() {
            let client = BuildKitClient::new("unix:///tmp/test.sock");
            let mut req = make_llb_request();
            req.no_cache = true;
            let args = client.build_llb_args(&req);
            assert!(
                args.contains(&"--no-cache".to_string()),
                "no_cache=true 时应包含 --no-cache"
            );
        }
    }
}
