# Phase B-4: CLI 集成与验证

> [← 返回目录](./README.md) | 上一阶段：[← Phase B-3](./phase-b3-grpc-session.md)

**目标：** 将 Phase B（LLB + gRPC）路径集成到 CLI `build` 命令，提供 Phase A / Phase B 双路径切换、调试工具和等价性验证，最终实现 BuildKit custom frontend 模式。

**前置条件：** Phase B-3 全部完成

## 任务依赖图

```
TB4.1 (CLI build 命令双路径集成)
 ├──► TB4.2 (--dump-llb 调试命令)
 ├──► TB4.3 (等价性验证工具)
 └──► TB4.4 (BuildKit Frontend 模式)
```

## 任务列表

### TB4.1 CLI build 命令双路径集成

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§6.1 |
| **railpack 参考** | `rp:cli/build.go` |
| **依赖** | TB3.6 |

**描述：** 为 `arcpack build` 命令增加 `--backend` 参数，支持在 Phase A（Dockerfile + buildctl）和 Phase B（LLB + gRPC / LLB + buildctl stdin）之间切换。默认使用 Phase A（稳定路径），用户可通过参数或环境变量切换到 Phase B。

**交付文件：**

- `src/cli/build.rs`（修改）

  **新增 CLI flags**：

  | Flag | 类型 | 默认值 | 说明 |
  |------|------|--------|------|
  | `--backend` | `Backend` | `dockerfile` | 构建后端：`dockerfile` (Phase A) / `llb` (Phase B-2, buildctl stdin) / `grpc` (Phase B-3) |

  **Backend 枚举**：

  ```rust
  #[derive(Clone, Debug, Default, ValueEnum)]
  pub enum Backend {
      /// Phase A：BuildPlan → Dockerfile → buildctl CLI（默认，稳定路径）
      #[default]
      Dockerfile,
      /// Phase B-2：BuildPlan → LLB → buildctl stdin（中间验证路径）
      #[cfg(feature = "llb")]
      Llb,
      /// Phase B-3：BuildPlan → LLB → gRPC Solve（目标路径）
      #[cfg(feature = "grpc")]
      Grpc,
  }
  ```

  **环境变量覆盖**：
  - `ARCPACK_BACKEND=dockerfile|llb|grpc` 覆盖 `--backend` 参数
  - 优先级：`--backend` > `ARCPACK_BACKEND` > 默认 `dockerfile`

  **run_build 分发逻辑**：

  ```rust
  pub async fn run_build(args: BuildArgs) -> Result<()> {
      // 1-6. 共享流程：检测 → BuildPlan → validate secrets → secrets hash
      // （与 Phase A 完全一致）

      let backend = resolve_backend(&args)?;

      match backend {
          Backend::Dockerfile => {
              // Phase A 路径（保持不变）
              let (dockerfile, image_config) = convert_plan_to_dockerfile(&plan, &opts)?;
              let client = BuildKitClient::new(&daemon.socket_addr());
              client.build(BuildRequest { dockerfile_content: dockerfile, ... }).await?;
          }

          #[cfg(feature = "llb")]
          Backend::Llb => {
              // Phase B-2 路径：LLB + buildctl stdin（只需 llb feature）
              let (definition, image_config) = convert_plan_to_llb(&plan, &opts)?;
              let client = BuildKitClient::new(&daemon.socket_addr());
              client.build_from_llb(&definition, &llb_request).await?;
          }

          #[cfg(feature = "grpc")]
          Backend::Grpc => {
              // Phase B-3 路径：LLB + gRPC Solve
              let (definition, image_config) = convert_plan_to_llb(&plan, &opts)?;
              let client = GrpcBuildKitClient::new(&daemon.socket_addr()).await?;
              client.build(GrpcBuildRequest { definition, image_config, ... }).await?;
          }
      }
  }
  ```

  > DaemonManager 在所有路径中共享——无论使用哪种 backend，都需要 buildkitd 运行。

**测试要求：**
- `--backend dockerfile` 走 Phase A 路径（默认）
- `--backend llb` 走 Phase B-2 路径（需 `llb` feature）
- `--backend grpc` 走 Phase B-3 路径（需 `grpc` feature）
- `ARCPACK_BACKEND` 环境变量覆盖 `--backend`
- 无 `llb` feature 时 `--backend llb` 返回友好错误
- 无 `grpc` feature 时 `--backend grpc` 返回友好错误
- Backend 解析失败时返回清晰的帮助信息
- （ignore）三种 backend 端到端构建同一 fixture，产出镜像等价

---

### TB4.2 --dump-llb 调试命令

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | — |
| **railpack 参考** | — |
| **依赖** | TB4.1 |

**描述：** 新增 `--dump-llb` flag，将生成的 LLB Definition 序列化输出到 stdout 或文件，不执行实际构建。用于调试 LLB 生成是否正确，也可配合 `buildctl build < llb.pb` 手动验证。

**交付文件：**

- `src/cli/build.rs`（修改）

  **新增 CLI flags**：

  | Flag | 类型 | 默认值 | 说明 |
  |------|------|--------|------|
  | `--dump-llb` | `Option<PathBuf>` | — | 输出 LLB protobuf 到文件（`-` 表示 stdout） |
  | `--dump-llb-json` | `bool` | `false` | 以 JSON 格式输出 LLB（可读性更好） |

  **实现逻辑**：

  ```rust
  if let Some(dump_path) = &args.dump_llb {
      let (definition, _) = convert_plan_to_llb(&plan, &opts)?;

      if args.dump_llb_json {
          // JSON 格式：人类可读
          let json = serde_json::to_string_pretty(&definition_to_json(&definition)?)?;
          write_output(dump_path, json.as_bytes())?;
      } else {
          // Protobuf 二进制：可直接 pipe 到 buildctl
          let bytes = serialize_definition(&definition)?;
          write_output(dump_path, &bytes)?;
      }

      return Ok(());  // 不执行构建
  }
  ```

  **definition_to_json 辅助函数**：

  ```rust
  /// 将 pb::Definition 转换为可读的 JSON 结构
  /// 每个 Op 解码为人类可读的描述（Source/Exec/File/Merge）
  fn definition_to_json(def: &pb::Definition) -> Result<serde_json::Value> {
      let ops: Vec<_> = def.def.iter()
          .map(|bytes| decode_and_describe_op(bytes))
          .collect::<Result<_>>()?;

      Ok(json!({
          "ops": ops,
          "metadata": def.metadata,
      }))
  }
  ```

  **使用场景**：

  ```bash
  # 输出 protobuf 二进制到文件
  arcpack build ./my-app --dump-llb llb.pb

  # 通过 buildctl 手动验证
  buildctl build --local context=./my-app < llb.pb

  # 输出 JSON 格式到 stdout
  arcpack build ./my-app --dump-llb - --dump-llb-json | jq .

  # 对比两次生成的 LLB 是否一致
  arcpack build ./my-app --dump-llb v1.pb
  arcpack build ./my-app --dump-llb v2.pb
  diff <(xxd v1.pb) <(xxd v2.pb)
  ```

**测试要求：**
- `--dump-llb output.pb` 写入有效的 protobuf 文件
- `--dump-llb -` 输出到 stdout
- `--dump-llb-json` 输出可解析的 JSON
- JSON 输出包含所有 Op 的类型和关键字段
- `--dump-llb` 不触发实际构建（不需要 buildkitd）
- 无 `llb` feature 时 `--dump-llb` 返回友好错误

---

### TB4.3 等价性验证工具

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | — |
| **railpack 参考** | — |
| **依赖** | TB4.1 |

**描述：** 自动化等价性验证——确保 Phase A（Dockerfile）和 Phase B（LLB）在相同输入下产出功能等价的 OCI 镜像。这是 Phase B 质量保证的关键工具，用于回归测试和渐进式迁移。

**交付文件：**

- `tests/equivalence_tests.rs`（新增，`#[ignore]`）

  **等价性验证策略**：

  ```rust
  /// 等价性验证：Phase A vs Phase B 产出的 OCI 镜像功能一致
  ///
  /// 验证维度：
  /// 1. 文件系统内容一致（关键路径）
  /// 2. 环境变量一致
  /// 3. CMD / ENTRYPOINT 一致
  /// 4. WORKDIR 一致
  /// 5. 应用可正常启动
  ```

  **测试用例**：

  ```rust
  #[tokio::test]
  #[ignore]
  async fn test_equivalence_node_npm() {
      let fixture = "tests/fixtures/node-npm";

      // Phase A
      let image_a = build_with_backend(fixture, Backend::Dockerfile).await?;
      // Phase B
      let image_b = build_with_backend(fixture, Backend::Grpc).await?;

      // 比较文件系统关键路径
      assert_files_equal(&image_a, &image_b, "/app/node_modules").await;
      assert_files_equal(&image_a, &image_b, "/app/package.json").await;

      // 比较环境变量
      assert_env_equal(&image_a, &image_b).await;

      // 比较 CMD
      assert_cmd_equal(&image_a, &image_b).await;

      // 运行验证
      assert_app_runs(&image_a).await;
      assert_app_runs(&image_b).await;
  }

  #[tokio::test]
  #[ignore]
  async fn test_equivalence_node_pnpm() { ... }

  #[tokio::test]
  #[ignore]
  async fn test_equivalence_python_pip() { ... }

  // ... 每种 Provider fixture 一个等价性测试
  ```

  **辅助函数**：

  | 函数 | 签名 | 说明 |
  |------|------|------|
  | `build_with_backend` | `(fixture, backend) -> Result<String>` | 使用指定 backend 构建，返回镜像 ID |
  | `assert_files_equal` | `(image_a, image_b, path)` | 比较两个镜像中指定路径的文件内容 |
  | `assert_env_equal` | `(image_a, image_b)` | 比较两个镜像的环境变量 |
  | `assert_cmd_equal` | `(image_a, image_b)` | 比较两个镜像的 CMD/ENTRYPOINT |
  | `assert_app_runs` | `(image)` | 验证镜像可正常启动并响应 |

**测试要求：**
- （非 ignore）`build_with_backend()` 参数组装正确
- （非 ignore）`assert_files_equal` 辅助函数对相同内容返回 Ok，不同内容 panic
- （ignore）Node.js npm fixture 等价性通过
- （ignore）所有已实现 Provider 的 fixture 等价性通过

---

### TB4.4 BuildKit Frontend 模式

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§6.1 |
| **railpack 参考** | `rp:cli/frontend.go`（RunFrontend 命令） |
| **依赖** | TB4.1 |

**描述：** 实现 `arcpack frontend` 子命令，使 arcpack 可作为 BuildKit custom frontend 运行。在 frontend 模式下，arcpack 由 buildkitd 调用（而非反过来），通过 gRPC gateway 接收构建请求、返回 LLB Definition。这是 arcpack 作为 PaaS 组件的高级集成模式。

**交付文件：**

- `src/cli/frontend.rs`（新增）

  **Frontend 子命令**：

  ```rust
  #[derive(Parser)]
  pub struct FrontendArgs {
      // 无需用户参数——所有配置通过 buildkitd 的 frontend opts 传入
  }
  ```

  **Frontend 模式工作流**（对齐 railpack `RunFrontend`）：

  ```
  buildkitd 调用 arcpack frontend：
  1. buildkitd 启动 arcpack 容器，注入 gRPC gateway 地址
  2. arcpack 连接 gateway（BUILDKIT_FRONTEND_ADDR 环境变量）
  3. arcpack 通过 gateway RPC 读取构建上下文（源码文件）
  4. arcpack 执行检测 → BuildPlan → LLB 转换
  5. arcpack 通过 gateway RPC 返回 LLB Definition + Image Config
  6. buildkitd 执行 LLB → 输出 OCI 镜像
  ```

  **Gateway Client**：

  ```rust
  #[cfg(feature = "grpc")]
  pub struct GatewayClient {
      client: LlbBridgeClient<Channel>,
  }

  impl GatewayClient {
      /// 从环境变量获取 gateway 地址并连接
      pub async fn from_env() -> Result<Self> {
          let addr = std::env::var("BUILDKIT_FRONTEND_ADDR")?;
          let channel = create_channel(&addr).await?;
          Ok(Self {
              client: LlbBridgeClient::new(channel),
          })
      }

      /// 读取构建上下文中的文件
      pub async fn read_file(&mut self, path: &str) -> Result<Vec<u8>> { ... }

      /// 读取构建上下文目录
      pub async fn read_dir(&mut self, path: &str) -> Result<Vec<FileInfo>> { ... }

      /// 返回构建结果
      pub async fn return_result(
          &mut self,
          definition: pb::Definition,
          image_config: ImageConfig,
      ) -> Result<()> { ... }
  }
  ```

  **run_frontend 入口**：

  ```rust
  #[cfg(feature = "grpc")]
  pub async fn run_frontend(_args: FrontendArgs) -> Result<()> {
      // 1. 连接 gateway
      let mut gateway = GatewayClient::from_env().await?;

      // 2. 读取构建上下文（通过 gateway RPC）
      let source = GatewaySourceAnalyzer::new(&mut gateway);

      // 3. 执行标准流水线：检测 → BuildPlan
      let plan = generate_build_plan(&source)?;

      // 4. 转换为 LLB
      let (definition, image_config) = convert_plan_to_llb(&plan, &opts)?;

      // 5. 返回结果给 buildkitd
      gateway.return_result(definition, image_config).await?;

      Ok(())
  }
  ```

  > 需要额外 proto：`gateway.proto`（BuildKit LLB Bridge service 定义）

  **使用方式**（构建 arcpack frontend 镜像后）：

  ```bash
  # 用户 Dockerfile 引用 arcpack frontend
  # syntax=arcpack:latest
  # 无需其他内容——arcpack 自动检测并构建

  # 或通过 buildctl 直接使用
  buildctl build \
    --frontend gateway.v0 \
    --opt source=arcpack:latest \
    --local context=./my-app
  ```

**测试要求：**
- `FrontendArgs` 解析成功（空参数）
- `GatewayClient::from_env()` 正确读取 `BUILDKIT_FRONTEND_ADDR`
- `GatewayClient::from_env()` 环境变量不存在时返回友好错误
- （ignore）Frontend 模式端到端：buildctl → arcpack frontend → OCI 镜像
- （ignore）通过 gateway 读取构建上下文文件正确

---

## 与 railpack 的已知差异

| 方面 | railpack (Go) | arcpack Phase B-4 (Rust) | 原因 |
|------|--------------|--------------------------|------|
| 后端切换 | 无（仅 LLB 路径） | `--backend dockerfile\|llb\|grpc` 三路径切换 | Phase A 保留作为 fallback |
| LLB 调试 | 无直接工具 | `--dump-llb` + `--dump-llb-json` | 辅助开发调试和问题排查 |
| 等价性测试 | 无 | 自动化等价性验证套件 | 双路径并存需要一致性保证 |
| Frontend | `frontend` 子命令 | 同样实现 `frontend` 子命令 | 对齐 railpack |
| 默认后端 | LLB (唯一) | Dockerfile (Phase A，稳定) | 渐进式迁移，Phase B 需验证后切换默认 |

---

## Phase B-4 Gate

**执行命令：**
```bash
cargo check --features grpc
cargo test --features grpc
cargo test --features grpc -- --ignored   # 需要 buildkitd
```

**验收清单：**
- [x] `--backend` 参数正确切换三种构建路径
- [x] `ARCPACK_BACKEND` 环境变量覆盖 `--backend`
- [x] 无 `llb` feature 时 LLB backend 返回友好错误；无 `grpc` feature 时 gRPC backend 返回友好错误
- [x] `--dump-llb` 输出有效的 protobuf / JSON
- [x] `--dump-llb` 不触发实际构建
- [x] 等价性测试框架可运行（5 个 Node.js fixture 等价性测试 + 辅助函数 + ImageCleanupGuard）
- [x] `arcpack frontend` 子命令存在且可解析
- [ ] `GatewayClient` 正确连接 `BUILDKIT_FRONTEND_ADDR`
- [ ] （ignore）三种 backend 构建同一 fixture 产出等价镜像
- [ ] （ignore）所有 Provider fixture 等价性验证通过
- [ ] （ignore）Frontend 模式端到端成功
- [ ] 预计 ~15 个测试用例全部通过
