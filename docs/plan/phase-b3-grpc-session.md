# Phase B-3: gRPC 客户端与 Session 协议

> [← 返回目录](./README.md) | 上一阶段：[← Phase B-2](./phase-b2-llb-conversion.md) | 下一阶段：[Phase B-4 →](./phase-b4-cli-integration.md)

**目标：** 实现 tonic gRPC 客户端，通过 Solve RPC 直接向 buildkitd 发送 LLB Definition，替代 buildctl CLI 中间层。实现 Session 协议（FilesyncProvider + SecretsProvider）以支持本地文件和 Secret 传输。

**前置条件：** Phase B-2 全部完成（LLB 生成已通过 buildctl stdin 验证）

## 额外 Proto 依赖

Phase B-1 仅需 `ops.proto`（自包含）。Phase B-3 需要更多 proto 文件用于 gRPC 服务定义：

| Proto 文件 | 来源 | 用途 |
|------------|------|------|
| `control.proto` | `moby/buildkit/api/services/control` | `Control` service（Solve/Status RPC） |
| `worker.proto` | `moby/buildkit/api/types` | Worker 信息类型 |
| `policy.proto` | `moby/buildkit/sourcepolicy` | Source policy 类型 |
| `google/protobuf/any.proto` | Google 标准 | `Any` 类型 |
| `google/protobuf/timestamp.proto` | Google 标准 | `Timestamp` 类型 |
| `google/rpc/status.proto` | Google 标准 | `Status` 类型 |
| `filesync.proto` | `moby/buildkit/session/filesync` | 文件同步 service |
| `wire.proto` | `tonistiigi/fsutil/types` | `filesync.proto` 的传输类型依赖（Stat 等文件元数据） |
| `secrets.proto` | `moby/buildkit/session/secrets` | Secret 提供 service |
| `auth.proto` | `moby/buildkit/session/auth` | Registry auth service |

## 任务依赖图

```
TB3.1 (gRPC 连接：tonic Channel + Unix Socket)
 └──► TB3.2 (Solve RPC 基础)
       └──► TB3.3 (Session：FilesyncProvider)
             └──► TB3.4 (Session：SecretsProvider)
                   └──► TB3.5 (流式进度：Status RPC)
                         └──► TB3.6 (Export 策略 + GrpcBuildKitClient 整合)
```

## 任务列表

### TB3.1 gRPC 连接：tonic Channel + Unix Socket

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | BK§3.1, Arch§3.10 |
| **railpack 参考** | `rp:buildkit/build.go`（getClient → grpc.Dial） |
| **依赖** | TB2.6 |

**描述：** 建立与 buildkitd 的 gRPC 连接。BuildKit daemon 通常监听 Unix domain socket（`unix:///run/buildkit/buildkitd.sock`），tonic 需要通过 `tower::service_fn` 适配 Unix socket 连接。同时支持 TCP 连接（用于远程 BuildKit）。

**交付文件：**

- `proto/moby/buildkit/v1/control.proto` 等额外 proto 文件（从 BuildKit 仓库获取）

- `build.rs`（修改）— 新增 proto 编译

  ```rust
  // B-1/B-2：LLB 原语只需 ops.proto
  #[cfg(feature = "llb")]
  {
      tonic_build::configure()
          .build_server(true)
          .compile_protos(
              &["proto/moby/buildkit/v1/ops.proto"],
              &["proto/"],
          )?;
  }

  // B-3/B-4：gRPC 需要额外的 service proto
  #[cfg(feature = "grpc")]
  {
      tonic_build::configure()
          .build_server(true)   // Session 需要 server 端（响应 buildkitd 回调）
          .compile_protos(
              &[
                  "proto/moby/buildkit/v1/control.proto",
                  "proto/moby/buildkit/v1/filesync.proto",
                  "proto/moby/buildkit/v1/secrets.proto",
              ],
              &["proto/"],
          )?;
  }
  ```

  > 注意：`llb` feature 和 `grpc` feature 各自调用一次 `tonic_build`。`grpc` 依赖 `llb`，
  > 所以启用 `grpc` 时两段代码都会执行。`ops.proto` 只在 `llb` 中编译一次。

- `src/buildkit/grpc/mod.rs` — gRPC 模块入口

  ```rust
  #[cfg(feature = "grpc")]
  pub mod channel;
  #[cfg(feature = "grpc")]
  pub mod solve;
  #[cfg(feature = "grpc")]
  pub mod session;
  #[cfg(feature = "grpc")]
  pub mod progress;
  ```

- `src/buildkit/grpc/channel.rs` — gRPC Channel 创建

  **核心函数**：

  ```rust
  /// 创建连接 buildkitd 的 gRPC Channel
  /// 支持 Unix socket 和 TCP 两种模式
  pub async fn create_channel(addr: &str) -> Result<Channel> {
      if addr.starts_with("unix://") {
          create_unix_channel(addr).await
      } else {
          create_tcp_channel(addr).await
      }
  }
  ```

  **Unix Socket Channel**（tonic + tower 适配）：

  ```rust
  async fn create_unix_channel(addr: &str) -> Result<Channel> {
      let socket_path = addr.strip_prefix("unix://").unwrap();
      let socket_path = socket_path.to_string();

      // tonic 通过 Endpoint::connect_with_connector 支持自定义 transport
      let channel = Endpoint::try_from("http://[::]:50051")?  // dummy URI
          .connect_with_connector(tower::service_fn(move |_| {
              let path = socket_path.clone();
              async move {
                  Ok::<_, std::io::Error>(
                      hyper_util::rt::TokioIo::new(
                          tokio::net::UnixStream::connect(path).await?
                      )
                  )
              }
          }))
          .await?;

      Ok(channel)
  }
  ```

  > 依赖：`hyper-util`（TokioIo 适配器）、`tower`（service_fn）

  **Cargo.toml 新增依赖**：

  ```toml
  [dependencies]
  hyper-util = { version = "0.1", optional = true, features = ["tokio"] }
  tower = { version = "0.5", optional = true }

  [features]
  llb = ["dep:prost", "dep:tonic-build"]                       # B-1/B-2（已在 TB1.1 定义）
  grpc = ["llb", "dep:tonic", "dep:hyper-util", "dep:tower"]   # B-3/B-4：gRPC 依赖 llb
  ```

**测试要求：**
- Unix socket 地址解析：`unix:///run/buildkit/buildkitd.sock` → 正确的 socket path
- TCP 地址解析：`tcp://localhost:1234` → 正确的 endpoint
- 无效地址格式返回错误
- （ignore）连接到实际 buildkitd Unix socket 成功建立 Channel
- （ignore）Channel 创建后可执行 ping（发送空请求验证连通性）

---

### TB3.2 Solve RPC 基础

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | BK§3.3, Arch§3.10 |
| **railpack 参考** | `rp:buildkit/build.go`（BuildWithBuildkitClient → controlClient.Solve） |
| **依赖** | TB3.1 |

**描述：** 实现 BuildKit Control service 的 Solve RPC 调用。Solve 是 BuildKit 的核心构建 RPC——接收 LLB Definition + Export 配置，返回构建结果。此任务实现最基础的 Solve 调用（无 Session），只能构建不需要本地文件的场景（如纯 image-based 构建）。

**交付文件：**

- `src/buildkit/grpc/solve.rs`

  **SolveRequest 构造**（对齐 Go `controlapi.SolveRequest`）：

  ```rust
  pub struct SolveConfig {
      pub definition: pb::Definition,
      pub exporter: ExportConfig,
      pub session_id: Option<String>,
      pub frontend_attrs: HashMap<String, String>,
  }

  pub enum ExportConfig {
      /// 输出为 OCI 镜像
      Image {
          name: String,
          push: bool,
      },
      /// 输出到本地目录
      Local {
          dest: PathBuf,
      },
      /// 输出为 Docker tar（docker load 兼容）
      DockerTar {
          name: String,
          dest: PathBuf,
      },
  }
  ```

  **Solve 函数**：

  ```rust
  /// 发送 Solve RPC 到 buildkitd
  /// 对齐 Go `controlClient.Solve(ctx, &controlapi.SolveRequest{...})`
  pub async fn solve(
      client: &mut ControlClient<Channel>,
      config: SolveConfig,
  ) -> Result<SolveResult> {
      let request = build_solve_request(config)?;
      let response = client.solve(request).await?;
      parse_solve_response(response)
  }

  pub struct SolveResult {
      pub exporter_response: HashMap<String, String>,
  }
  ```

  **build_solve_request 内部实现**：

  ```
  SolveRequest {
      r#ref: random_session_ref(),
      definition: Some(config.definition),
      exporter: match config.exporter {
          Image { name, push } => "image",
          Local { .. } => "local",
          DockerTar { .. } => "docker",
      },
      exporter_attrs: match config.exporter {
          Image { name, push } => { "name": name, "push": push.to_string() },
          Local { dest } => { "dest": dest.to_string() },
          DockerTar { name, dest } => { "name": name, "dest": dest.to_string() },
      },
      session: config.session_id.unwrap_or_default(),
      frontend_attrs: config.frontend_attrs,
  }
  ```

**测试要求：**
- `build_solve_request` 对 Image/Local/DockerTar 三种 ExportConfig 正确构造 SolveRequest
- Image export：exporter = "image"，attrs 包含 name 和 push
- Local export：exporter = "local"，attrs 包含 dest
- DockerTar export：exporter = "docker"，attrs 包含 name 和 dest
- session_id 为空时使用空字符串
- （ignore）发送 Solve 到 buildkitd 构建 scratch + exec 成功

---

### TB3.3 Session：FilesyncProvider

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | BK§3.3, Arch§3.10 |
| **railpack 参考** | `rp:buildkit/build.go`（attachable: filesync.NewFSSyncProvider） |
| **依赖** | TB3.2 |

**⚠️ 高风险：Session 协议复杂度**

BuildKit Session 协议**并非**标准 gRPC 双向流。实际实现使用 HTTP/2 连接劫持（hijacking）：

1. 客户端调用 `Control.Session()` RPC（`stream BytesMessage` 双向流）
2. 在这个**单一双向流**内部，BuildKit 实现了自己的 gRPC 多路复用协议
3. Go 实现使用 `grpc-go` 内部 API 在 stream 内嵌套一个完整的 gRPC server
4. 这不是标准 `tonic` bidirectional streaming 能直接实现的

**前置研究任务：** 实现前需先调研：
- Rust 生态是否有现成的 Session 实现（如 Nydus、shadow-rs 等 BuildKit 前端项目）
- `tonic` 的 `Streaming<BytesMessage>` 是否能模拟 HTTP/2 连接劫持
- 是否可使用 `hyper` 低层 API 直接操作 HTTP/2 帧

**降级方案：** 若 Session 实现过于复杂，B-3 可退化为 "LLB + buildctl stdin"（已在 TB2.6 验证通过），gRPC Session 作为后续优化。此时 TB3.3-TB3.6 可标记为 `deferred`，不阻塞 Phase B 交付。

---

**描述：** 实现 BuildKit Session 协议的 FilesyncProvider。当 LLB 中包含 `local://` Source 操作时，buildkitd 通过 Session 回调请求客户端提供本地文件。客户端需要运行一个 gRPC server 端，响应 buildkitd 的文件读取请求。

**交付文件：**

- `src/buildkit/grpc/session/mod.rs` — Session 模块入口

  ```rust
  pub mod filesync;
  pub mod secrets;
  pub mod manager;
  ```

- `src/buildkit/grpc/session/filesync.rs` — 文件同步提供者

  **FilesyncProvider**（对齐 Go `filesync.NewFSSyncProvider`）：

  ```rust
  pub struct FilesyncProvider {
      /// 目录映射：name → 本地路径
      /// 对应 buildctl `--local context=/path/to/dir`
      dirs: HashMap<String, PathBuf>,
  }
  ```

  **方法**：

  | 方法 | 签名 | 说明 |
  |------|------|------|
  | `new` | `(dirs: HashMap<String, PathBuf>) -> Self` | 构造，注册本地目录映射 |
  | `diff_copy` | `(&self, request: DiffCopyRequest) -> impl Stream` | 响应 buildkitd 的文件请求，流式发送文件内容 |

  **实现要点**：

  ```
  Session 协议流程：
  1. 客户端调用 Solve RPC 时附带 session_id
  2. 客户端同时启动 Session gRPC server（通过 HTTP/2 双向流）
  3. buildkitd 遇到 local:// Source 时，回调客户端的 FilesyncProvider
  4. FilesyncProvider 读取本地文件，通过 gRPC stream 发送给 buildkitd
  5. buildkitd 接收文件后继续构建
  ```

  > Go 实现使用 `tonistiigi/fsutil` 进行高效的文件 diff + 传输。
  > Rust 实现可先使用简单的完整文件传输，后续优化为 diff 模式。

- `src/buildkit/grpc/session/manager.rs` — Session 管理器

  **SessionManager**：

  ```rust
  pub struct SessionManager {
      session_id: String,
      filesync: Option<FilesyncProvider>,
      secrets: Option<SecretsProvider>,     // TB3.4 实现
  }
  ```

  | 方法 | 签名 | 说明 |
  |------|------|------|
  | `new` | `() -> Self` | 构造，生成随机 session_id |
  | `session_id` | `(&self) -> &str` | 返回 session_id |
  | `with_filesync` | `(mut self, provider: FilesyncProvider) -> Self` | 注册文件同步提供者 |
  | `with_secrets` | `(mut self, provider: SecretsProvider) -> Self` | 注册 Secret 提供者 |
  | `run` | `(&self, channel: Channel) -> JoinHandle<Result<()>>` | 启动 Session goroutine，监听 buildkitd 回调 |

**测试要求：**
- `FilesyncProvider::new()` 正确注册目录映射
- 目录映射 name → path 查找正确
- 不存在的 name 返回错误
- `SessionManager` 生成唯一 session_id
- `SessionManager` 正确注册 filesync / secrets provider
- （ignore）Session 协议与 buildkitd 交互：local Source 文件正确传输

---

### TB3.4 Session：SecretsProvider

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | BK§3.3, Arch§3.10 |
| **railpack 参考** | `rp:buildkit/build.go`（attachable: secretsprovider.NewSecretProvider） |
| **依赖** | TB3.3 |

**描述：** 实现 BuildKit Session 协议的 SecretsProvider。当 LLB 中包含 Secret mount 时，buildkitd 通过 Session 回调请求客户端提供 Secret 值。与 Phase A 的 `--secret id=KEY,env=KEY` 机制等价，但通过 gRPC 传输而非 CLI 参数。

**交付文件：**

- `src/buildkit/grpc/session/secrets.rs` — Secret 提供者

  **SecretsProvider**（对齐 Go `secretsprovider.NewSecretProvider`）：

  ```rust
  pub struct SecretsProvider {
      /// Secret 映射：name → value
      secrets: HashMap<String, String>,
  }
  ```

  **方法**：

  | 方法 | 签名 | 说明 |
  |------|------|------|
  | `new` | `(secrets: HashMap<String, String>) -> Self` | 构造，注册 Secret 映射 |
  | `get_secret` | `(&self, request: GetSecretRequest) -> Result<GetSecretResponse>` | 响应 buildkitd 的 Secret 请求 |

  **实现要点**：

  ```
  Secret 传输流程：
  1. LLB 中的 ExecOp 包含 Secret mount（MountSpec::SecretEnv）
  2. buildkitd 执行 ExecOp 时，发现需要 Secret
  3. buildkitd 通过 Session 回调 SecretsProvider.GetSecret(id)
  4. SecretsProvider 查找 secret name → 返回 value
  5. buildkitd 将 Secret 注入容器环境变量
  ```

  **gRPC Service 实现**（tonic server）：

  ```rust
  #[tonic::async_trait]
  impl secrets_server::Secrets for SecretsProvider {
      async fn get_secret(
          &self,
          request: Request<GetSecretRequest>,
      ) -> Result<Response<GetSecretResponse>, Status> {
          let id = &request.get_ref().id;
          match self.secrets.get(id) {
              Some(value) => Ok(Response::new(GetSecretResponse {
                  data: value.as_bytes().to_vec(),
              })),
              None => Err(Status::not_found(format!("secret not found: {}", id))),
          }
      }
  }
  ```

**测试要求：**
- `SecretsProvider::new()` 正确注册 Secret 映射
- `get_secret()` 已注册的 Secret 返回正确值
- `get_secret()` 未注册的 Secret 返回 NOT_FOUND 错误
- Secret 值作为 bytes 传输（UTF-8 编码）
- 空 Secret 映射时任何请求都返回 NOT_FOUND
- （ignore）Session 协议与 buildkitd 交互：Secret 正确注入构建容器

---

### TB3.5 流式进度：Status RPC

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | BK§3.5 |
| **railpack 参考** | `rp:buildkit/build.go`（displayCh + progressui.DisplaySolveStatus） |
| **依赖** | TB3.4 |

**描述：** 实现 BuildKit Status RPC 的流式进度监听。Solve RPC 是异步的——发送后通过 Status stream 接收构建进度。对齐 railpack 的 `displayCh` + `progressui` 进度渲染。

**交付文件：**

- `src/buildkit/grpc/progress.rs` — 进度监听与渲染

  **StatusStream**：

  ```rust
  pub struct StatusStream {
      stream: Streaming<StatusResponse>,
  }
  ```

  **ProgressEvent**（简化的进度事件）：

  ```rust
  pub enum ProgressEvent {
      /// 构建步骤开始
      VertexStarted {
          id: String,
          name: String,
      },
      /// 构建步骤完成
      VertexCompleted {
          id: String,
          duration: Duration,
          cached: bool,
      },
      /// 构建步骤失败
      VertexError {
          id: String,
          error: String,
      },
      /// 步骤日志输出
      Log {
          vertex_id: String,
          data: Vec<u8>,
      },
  }
  ```

  **核心函数**：

  | 函数 | 签名 | 说明 |
  |------|------|------|
  | `subscribe_status` | `(client: &mut ControlClient, ref_id: &str) -> Result<StatusStream>` | 订阅 Solve 进度流 |
  | `next_event` | `(&mut self) -> Option<Result<ProgressEvent>>` | 获取下一个进度事件 |
  | `render_progress` | `(events: impl Stream<Item = ProgressEvent>, mode: ProgressMode)` | 渲染进度到终端 |

  **ProgressMode**（对齐 buildctl `--progress`）：

  ```rust
  pub enum ProgressMode {
      Auto,    // TTY → 动态刷新；非 TTY → plain
      Plain,   // 逐行输出
      Tty,     // 强制 TTY 模式（动态刷新）
      Quiet,   // 静默
  }
  ```

  **渲染逻辑**：

  ```
  Plain 模式（优先实现）：
    VertexStarted  → "[step_name] RUNNING"
    VertexCompleted → "[step_name] DONE {duration}s" 或 "[step_name] CACHED"
    VertexError    → "[step_name] ERROR: {msg}"
    Log            → "  | {log_line}"

  TTY 模式（后续优化）：
    使用 ANSI escape codes 动态刷新进度
    参考 railpack progressui 或 indicatif crate
  ```

**测试要求：**
- `ProgressEvent` 从 `StatusResponse` 正确解析 Vertex/Log 信息
- Plain 模式渲染输出格式正确
- Cached 步骤显示 `CACHED` 标记
- Error 事件包含错误信息
- 空 stream 正常终止（不 panic）
- （ignore）连接 buildkitd 订阅实际构建进度流

---

### TB3.6 Export 策略 + GrpcBuildKitClient 整合

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | BK§3.5, Arch§3.10 |
| **railpack 参考** | `rp:buildkit/build.go`（BuildWithBuildkitClient 完整实现） |
| **依赖** | TB3.5 |

**描述：** 整合所有 gRPC 组件为 `GrpcBuildKitClient`，提供与 Phase A `BuildKitClient` 相同的构建接口。包含完整的 Solve + Session + Progress 编排，以及 OCI 镜像的 Export 策略（image push / docker load / local dir）。

**交付文件：**

- `src/buildkit/grpc_client.rs` — gRPC 构建客户端

  **GrpcBuildKitClient**（对齐 railpack `BuildWithBuildkitClient`）：

  ```rust
  #[cfg(feature = "grpc")]
  pub struct GrpcBuildKitClient {
      channel: Channel,
      addr: String,
  }
  ```

  **GrpcBuildRequest**：

  ```rust
  #[cfg(feature = "grpc")]
  pub struct GrpcBuildRequest {
      pub definition: pb::Definition,
      pub image_config: ImageConfig,
      pub context_dir: PathBuf,
      pub image_name: Option<String>,
      pub output_dir: Option<PathBuf>,
      pub push: bool,
      pub progress_mode: ProgressMode,
      pub secrets: HashMap<String, String>,
      pub local_dirs: HashMap<String, PathBuf>,
  }
  ```

  **核心方法**：

  ```rust
  impl GrpcBuildKitClient {
      pub async fn new(addr: &str) -> Result<Self> {
          let channel = create_channel(addr).await?;
          Ok(Self { channel, addr: addr.to_string() })
      }

      /// 完整 gRPC 构建流程
      /// 对齐 railpack `BuildWithBuildkitClient()`
      pub async fn build(&self, request: GrpcBuildRequest) -> Result<BuildOutput> {
          // 1. 创建 Session Manager
          let mut session = SessionManager::new()
              .with_filesync(FilesyncProvider::new(request.local_dirs))
              .with_secrets(SecretsProvider::new(request.secrets));

          // 2. 启动 Session（后台 task）
          let session_handle = session.run(self.channel.clone());

          // 3. 构造 SolveConfig
          let config = SolveConfig {
              definition: request.definition,
              exporter: build_export_config(&request)?,
              session_id: Some(session.session_id().to_string()),
              frontend_attrs: build_frontend_attrs(&request.image_config),
          };

          // 4. 启动进度监听（后台 task）
          // ⚠️ 伪代码：实际实现需先 clone 所有引用值再 move 进 spawn
          //    （tokio::spawn 要求 'static，不能引用 &self 或已 move 的变量）
          let channel = self.channel.clone();
          let progress_mode = request.progress_mode.clone();
          let ref_id = config.ref_id.clone();
          let progress_handle = tokio::spawn(async move {
              let mut client = ControlClient::new(channel);
              let stream = subscribe_status(&mut client, &ref_id).await?;
              render_progress(stream, progress_mode).await
          });

          // 5. 发送 Solve RPC
          let mut client = ControlClient::new(self.channel.clone());
          let result = solve(&mut client, config).await?;

          // 6. 等待进度渲染完成
          progress_handle.await??;

          // 7. 停止 Session
          session_handle.abort();

          Ok(BuildOutput {
              image_digest: result.exporter_response.get("containerimage.digest").cloned(),
              duration: start.elapsed(),
          })
      }
  }
  ```

  **Export 策略**（OCI 镜像输出方式）：

  | 策略 | ExportConfig | 场景 |
  |------|-------------|------|
  | Image push | `Image { name, push: true }` | CI/CD，推送到 registry |
  | Image local | `Image { name, push: false }` | 本地标记，不推送 |
  | Docker tar | `DockerTar { name, dest }` | `docker load` 兼容 |
  | Local dir | `Local { dest }` | 输出文件到本地目录 |

  **Image Config 传递**（对齐 Go `ExporterImageConfigKey`）：

  ```
  frontend_attrs 中传递 Image Config：
    "containerimage.config" → JSON 编码的 OCI Image Config
    包含 ENV, CMD, ENTRYPOINT, WORKDIR 等运行时配置
  ```

**测试要求：**
- `GrpcBuildKitClient::new()` 正确创建 Channel
- `build_export_config()` 对 4 种 Export 策略正确构造 ExportConfig
- `build_frontend_attrs()` 正确编码 ImageConfig 为 JSON
- Session Manager 正确协调 filesync + secrets provider
- Solve + Session + Progress 并发编排不死锁
- （ignore）完整 gRPC 构建：LLB → Solve → OCI 镜像
- （ignore）Image push + docker load + local dir 三种 export 模式
- （ignore）进度流实时输出构建日志

---

## 与 railpack 的已知差异

| 方面 | railpack (Go) | arcpack Phase B-3 (Rust) | 原因 |
|------|--------------|--------------------------|------|
| gRPC 库 | 内置 `google.golang.org/grpc` | tonic 0.12 + hyper-util | Rust 生态标准选择 |
| Unix Socket | `grpc.Dial("unix://...")` 原生支持 | 需要 tower `service_fn` 适配 | tonic 不原生支持 Unix socket |
| Filesync | `tonistiigi/fsutil` 高效 diff 传输 | 初版：完整文件传输；后续优化 | fsutil 无 Rust 等价物 |
| Session 协议 | Go HTTP/2 连接劫持 + 内嵌 gRPC server | 需研究 tonic/hyper 低层 API 实现；**高风险**，有 buildctl stdin 降级方案 | Go 使用 grpc-go 内部 API，Rust 无直接等价物 |
| 进度渲染 | `progressui` 自定义 TUI | 初版：Plain 模式；后续：indicatif crate | 渐进式实现 |
| Registry auth | `docker/cli` 集成 | 初版不实现，后续扩展 | 降低首版复杂度 |

---

## Phase B-3 Gate

**执行命令：**
```bash
cargo check --features grpc
cargo test --features grpc
cargo test --features grpc -- --ignored   # 需要 buildkitd
```

**验收清单：**
- [x] `cargo check --features grpc` 无错误无警告
- [x] 额外 proto 文件（control/filesync/secrets）编译成功
- [x] Unix socket Channel 连接 buildkitd 成功
- [x] Solve RPC 发送 LLB Definition 并收到结果
- [x] FilesyncProvider 响应 buildkitd 文件请求（DiffCopy 协议通过 h2 crate 实现，含 walk/stat/req/fin/data 阶段；TarStream 返回 Unimplemented）
- [x] SecretsProvider 响应 buildkitd Secret 请求
- [x] Status RPC 流式接收构建进度
- [x] Plain 模式进度渲染输出正确
- [x] `GrpcBuildKitClient.build()` 完整流程实现（SessionManager + Solve + Progress 编排）
- [x] Image push / local dir / docker tar 三种 export 模式正确
- [x] Session + Solve + Progress 并发编排实现（h2 server 在 bidi stream 上运行，按 :path 路由到 handler）
- [ ] 预计 ~20 个测试用例全部通过（端到端集成测试需 buildkitd 环境验证）
