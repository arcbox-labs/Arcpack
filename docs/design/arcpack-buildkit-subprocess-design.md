# arcpack BuildKit 子进程集成技术方案

## 1. 背景与目标

arcpack 是一个零配置应用构建器，需要调用 BuildKit 来构建容器镜像。本方案采用**子进程模式**集成 BuildKit，实现以下目标：

- **用完即停**：构建时启动 buildkitd，构建完成后立即销毁，不浪费资源
- **运行时隔离**：Rust（arcpack）和 Go（BuildKit）各自独立运行，互不干扰
- **零改造**：直接使用官方 BuildKit 二进制，无需修改 BuildKit 代码
- **Dogfooding**：构建任务以 Spot 实例运行在 ArcBox 平台上

## 2. 架构总览

```
┌─────────────────────────────────────────────────┐
│                 ArcBox Spot 实例                  │
│                                                   │
│  ┌───────────┐  Unix Socket gRPC  ┌────────────┐ │
│  │  arcpack   │ ◄───────────────► │ buildkitd  │ │
│  │  (Rust)    │                    │ (子进程)    │ │
│  │           │                    │            │ │
│  │  1.spawn()│───启动──────────►  │ 监听 sock  │ │
│  │  2.build() │───构建请求─────►  │ 执行构建    │ │
│  │  3.kill()  │───终止──────────► │ 退出       │ │
│  └───────────┘                    └────────────┘ │
│                                                   │
│  构建完成 → Spot 实例销毁                          │
└─────────────────────────────────────────────────┘
```

## 3. 核心流程

整个生命周期分为四个阶段：

```
启动 buildkitd → 等待就绪 → 执行构建 → 停止 buildkitd
```

### 3.1 启动 buildkitd

```rust
use std::process::{Command, Child, Stdio};
use std::path::Path;

const BUILDKIT_SOCK: &str = "/tmp/buildkit.sock";

fn start_buildkitd() -> anyhow::Result<Child> {
    // 清理可能残留的 socket 文件
    let sock_path = Path::new(BUILDKIT_SOCK);
    if sock_path.exists() {
        std::fs::remove_file(sock_path)?;
    }

    let child = Command::new("buildkitd")
        .args(&[
            "--addr", &format!("unix://{}", BUILDKIT_SOCK),
            "--oci-worker-no-process-sandbox",  // 容器内运行通常需要
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    Ok(child)
}
```

### 3.2 等待 buildkitd 就绪

buildkitd 启动后需要一定时间初始化。不能用简单的 `sleep`，应该主动探测 socket 是否可连接：

```rust
use std::time::{Duration, Instant};

fn wait_for_buildkitd_ready(timeout: Duration) -> anyhow::Result<()> {
    let start = Instant::now();
    let sock_path = Path::new(BUILDKIT_SOCK);

    loop {
        if start.elapsed() > timeout {
            anyhow::bail!("buildkitd 启动超时");
        }

        if sock_path.exists() {
            // 尝试连接 Unix socket 确认真正就绪
            match std::os::unix::net::UnixStream::connect(sock_path) {
                Ok(_) => return Ok(()),
                Err(_) => {}
            }
        }

        std::thread::sleep(Duration::from_millis(200));
    }
}
```

### 3.3 执行构建

两种调用方式，建议先用方式 A 跑通，后续再迁移到方式 B。

**方式 A：调用 buildctl 命令行（推荐先实现）**

```rust
struct BuildRequest {
    context_dir: String,       // 构建上下文目录
    dockerfile_dir: String,    // Dockerfile 所在目录
    output_image: String,      // 输出镜像名 (e.g. registry.example.com/app:v1)
    push: bool,                // 是否推送到 registry
}

fn build_image(req: &BuildRequest) -> anyhow::Result<()> {
    let mut args = vec![
        "--addr".to_string(),
        format!("unix://{}", BUILDKIT_SOCK),
        "build".to_string(),
        "--frontend".to_string(),
        "dockerfile.v0".to_string(),
        "--local".to_string(),
        format!("context={}", req.context_dir),
        "--local".to_string(),
        format!("dockerfile={}", req.dockerfile_dir),
    ];

    // 输出配置
    if req.push {
        args.extend([
            "--output".to_string(),
            format!("type=image,name={},push=true", req.output_image),
        ]);
    } else {
        args.extend([
            "--output".to_string(),
            format!("type=image,name={}", req.output_image),
        ]);
    }

    let status = Command::new("buildctl")
        .args(&args)
        .stdout(Stdio::inherit())   // 实时输出构建日志
        .stderr(Stdio::inherit())
        .status()?;

    if !status.success() {
        anyhow::bail!("构建失败，退出码: {:?}", status.code());
    }

    Ok(())
}
```

**方式 B：Rust gRPC 客户端直连（后续优化）**

```rust
// 使用 tonic 连接 BuildKit 的 gRPC API
// 优势：可获取构建进度流、更精细的错误处理、不依赖 buildctl 二进制

// Cargo.toml 依赖:
// tonic = "0.11"
// tokio = { version = "1", features = ["full"] }
// tower = "0.4"

use tonic::transport::{Endpoint, Uri};
use tower::service_fn;
use tokio::net::UnixStream;

async fn connect_buildkit() -> anyhow::Result</* BuildKitClient */> {
    let channel = Endpoint::try_from("http://[::]:50051")?
        .connect_with_connector(service_fn(|_: Uri| {
            UnixStream::connect(BUILDKIT_SOCK)
        }))
        .await?;

    // 使用 BuildKit 的 protobuf 定义生成的客户端
    // 需要从 BuildKit 仓库获取 .proto 文件并用 tonic-build 生成
    todo!("基于 BuildKit proto 生成客户端代码")
}
```

### 3.4 停止 buildkitd

```rust
fn stop_buildkitd(mut child: Child) -> anyhow::Result<()> {
    // 先发 SIGTERM，给 buildkitd 优雅退出的机会
    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        let pid = Pid::from_raw(child.id() as i32);
        let _ = kill(pid, Signal::SIGTERM);
    }

    // 等待最多 5 秒
    let timeout = Duration::from_secs(5);
    let start = Instant::now();
    loop {
        match child.try_wait()? {
            Some(_status) => {
                // 已退出，清理 socket
                let _ = std::fs::remove_file(BUILDKIT_SOCK);
                return Ok(());
            }
            None if start.elapsed() > timeout => {
                // 超时，强制 kill
                child.kill()?;
                child.wait()?;
                let _ = std::fs::remove_file(BUILDKIT_SOCK);
                return Ok(());
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}
```

### 3.5 完整主流程

```rust
fn run_build(req: &BuildRequest) -> anyhow::Result<()> {
    // 1. 启动
    let child = start_buildkitd()?;

    // 2. 等待就绪
    let result = wait_for_buildkitd_ready(Duration::from_secs(30));
    if let Err(e) = result {
        stop_buildkitd(child)?;
        return Err(e);
    }

    // 3. 构建（确保无论成功失败都会清理）
    let build_result = build_image(req);

    // 4. 停止
    stop_buildkitd(child)?;

    build_result
}
```

## 4. 部署方案

### 4.1 容器镜像构建

```dockerfile
# ── 阶段 1: 编译 arcpack ──
FROM rust:1.78 AS builder
WORKDIR /src
COPY . .
RUN cargo build --release

# ── 阶段 2: 运行时镜像 ──
FROM ubuntu:22.04

# 安装必要的运行时依赖
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# 从官方 BuildKit 镜像中复制二进制
COPY --from=moby/buildkit:latest /usr/bin/buildkitd /usr/bin/buildkitd
COPY --from=moby/buildkit:latest /usr/bin/buildctl  /usr/bin/buildctl

# 复制 arcpack
COPY --from=builder /src/target/release/arcpack /usr/bin/arcpack

ENTRYPOINT ["arcpack"]
```

### 4.2 最终容器内文件结构

```
/usr/bin/
├── arcpack         # Rust 二进制 (主程序)
├── buildkitd       # BuildKit daemon (被 arcpack 按需启动)
└── buildctl        # BuildKit CLI (被 arcpack 调用执行构建)
```

### 4.3 ArcBox Spot 实例部署流程

```
用户提交构建请求
       │
       ▼
ArcBox 控制面板收到请求
       │
       ▼
启动 Spot 实例（拉取 arcpack 镜像）
       │
       ▼
容器内 arcpack 启动
  ├── spawn buildkitd 子进程
  ├── wait_for_ready()
  ├── build_image()        ← 执行构建，push 镜像到 registry
  └── stop_buildkitd()
       │
       ▼
arcpack 退出，Spot 实例销毁（不再计费）
```

## 5. 需要注意的问题

### 5.1 权限

buildkitd 通常需要特权来操作容器。在容器内运行时有两种方案：

| 方案 | 配置 | 安全性 |
|------|------|--------|
| 特权模式 | `--privileged` | 低，但最简单 |
| rootless 模式 | `buildkitd --oci-worker-no-process-sandbox` | 较高，推荐 |

### 5.2 构建缓存

Spot 实例销毁后，BuildKit 的 layer 缓存会丢失。解决方案：

- **Registry 缓存**：`--export-cache type=registry,ref=cache-image` + `--import-cache type=registry,ref=cache-image`
- **挂载持久卷**：将 `/var/lib/buildkit` 挂载到持久存储

推荐使用 registry 缓存，无需管理额外存储，且天然支持跨实例共享。

### 5.3 错误处理要点

- buildkitd 启动超时 → 检查权限和日志
- 构建失败 → 确保 stop_buildkitd 仍然被调用（用 defer/RAII 模式）
- buildkitd 意外崩溃 → arcpack 检测到子进程退出，清理并报错
- socket 文件残留 → 启动前先清理

### 5.4 Registry 认证

如果需要 push 到私有 registry，需要在容器内配置认证：

```bash
# 方式 1: 挂载 docker config
# 运行时挂载 ~/.docker/config.json

# 方式 2: 环境变量
# REGISTRY_AUTH_TOKEN=xxx arcpack build ...
```

## 6. 实施路线

| 阶段 | 内容 | 预期产出 |
|------|------|---------|
| P0 | 实现 start/wait/build/stop 四个核心函数，用 buildctl CLI 调用 | 可在本地完成一次完整构建 |
| P1 | 打包 Dockerfile，部署到 ArcBox Spot 实例 | 可通过 ArcBox 触发远程构建 |
| P2 | 接入 registry 缓存，优化冷启动速度 | 二次构建速度显著提升 |
| P3 | 替换 buildctl CLI 为 tonic gRPC 客户端 | 获取构建进度流，更精细的错误处理 |
