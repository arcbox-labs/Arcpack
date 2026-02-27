# Phase B-2: BuildPlan → LLB 转换

> [← 返回目录](./README.md) | 上一阶段：[← Phase B-1](./phase-b1-llb-primitives.md) | 下一阶段：[Phase B-3 →](./phase-b3-grpc-session.md)

**目标：** 将 Phase A 的 Dockerfile 生成路径扩展为 LLB 原生路径，复用现有 BuildGraph 架构，新增 `to_llb()` 方法将 BuildPlan 直接转换为 `pb::Definition`。

**前置条件：** Phase B-1 全部完成

## Phase A 代码复用分析

| 组件 | 文件路径 | 复用方式 |
|------|---------|---------|
| `Graph<T: Node>` | `src/graph/mod.rs` | 完全复用 — 图遍历和拓扑排序不变 |
| `BuildEnvironment` | `build_llb/build_env.rs` | 完全复用 — 环境变量管理逻辑不变 |
| `should_merge()` / `has_significant_overlap()` / `resolve_paths()` | `build_llb/layers.rs` | 完全复用 — 纯决策函数，与输出格式无关 |
| `build_image_config()` | `image.rs` | 完全复用 — OCI Image config 与 LLB/Dockerfile 无关 |
| `StepNode` | `build_llb/step_node.rs` | 扩展 — 新增 `#[cfg(feature = "llb")] llb_state` 字段 |
| `BuildKitCacheStore` | `build_llb/cache_store.rs` | 扩展 — 新增 `get_cache_mount_spec()` |
| `BuildGraph` | `build_llb/mod.rs` | 分叉 — 新增 `to_llb()` + `convert_*_to_llb()` 方法 |

## 任务依赖图

```
TB2.1 (StepNode 扩展 + LLB BuildGraph 骨架)
 ├──► TB2.2 (CacheStore LLB 改造)
 ├──► TB2.3 (Layers LLB 策略)
 └──► TB2.4 (convertNodeToLLB — 4种命令转换)
       └──► TB2.5 (Deploy 阶段 + convert_plan_to_llb 入口)
             └──► TB2.6 (buildctl + LLB stdin 中间验证)
```

## 任务列表

### TB2.1 StepNode 扩展 + LLB BuildGraph 骨架

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | `rp:buildkit/build_llb/step_node.go`（State 字段）, `rp:buildkit/build_llb/build_graph.go`（ToLLB 方法） |
| **依赖** | TB1.6 |

**描述：** 扩展 StepNode 以支持 LLB State 存储，为 BuildGraph 增加 `to_llb()` 骨架方法。Phase A 中 StepNode 的 `dockerfile_stage` 字段存储 Dockerfile 阶段片段；Phase B 新增 `llb_state` 字段存储 LLB `OperationOutput`，两者在处理流程中并行存在。

**交付文件：**

- `src/buildkit/build_llb/step_node.rs`（修改）

  **新增字段**：

  ```rust
  pub struct StepNode {
      // ... Phase A 字段保持不变 ...

      /// Phase B: LLB State，处理完成后存储该步骤的输出 OperationOutput
      #[cfg(feature = "llb")]
      pub llb_state: Option<OperationOutput>,
  }
  ```

  **新增方法**：

  | 方法 | 签名 | 说明 |
  |------|------|------|
  | `set_llb_state` | `(&mut self, state: OperationOutput)` | 设置 LLB State（`#[cfg(feature = "llb")]`） |
  | `get_llb_state` | `(&self) -> Option<&OperationOutput>` | 获取 LLB State |

- `src/buildkit/build_llb/mod.rs`（修改）

  **BuildGraph 新增方法骨架**：

  ```rust
  impl BuildGraph {  // 无生命周期参数——BuildGraph 拥有 plan: BuildPlan（owned）
      // Phase A：保持不变
      pub fn to_dockerfile(&mut self) -> Result<String> { ... }

      // Phase B：LLB 转换入口
      // 返回 (Definition, BuildEnvironment) —— output_env 供 build_image_config() 使用
      #[cfg(feature = "llb")]
      pub fn to_llb(&mut self) -> Result<(pb::Definition, BuildEnvironment)> {
          let order = self.graph.compute_processing_order()?;
          for node_name in &order {
              self.process_node_llb(node_name)?;
          }
          let definition = self.build_deploy_llb()?;
          let output_env = self.collect_output_env();  // 收集最终环境变量
          Ok((definition, output_env))
      }

      #[cfg(feature = "llb")]
      fn process_node_llb(&mut self, node_name: &str) -> Result<()> {
          // TB2.4 实现
          todo!()
      }

      #[cfg(feature = "llb")]
      fn build_deploy_llb(&self) -> Result<pb::Definition> {
          // TB2.5 实现
          todo!()
      }
  }
  ```

**测试要求：**
- StepNode `llb_state` 初始为 `None`
- `set_llb_state` / `get_llb_state` 读写正确
- `to_llb()` 骨架在无实现时返回 `todo!` panic（占位确认）
- Feature gate 验证：无 `llb` feature 时 `llb_state` 字段和 `to_llb()` 方法不存在

---

### TB2.2 CacheStore LLB 改造

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | `rp:buildkit/build_llb/cache_store.go`（GetCacheMount） |
| **依赖** | TB2.1 |

**描述：** 扩展 `BuildKitCacheStore`，新增 `get_cache_mount_spec()` 方法，返回 LLB `MountSpec::Cache` 而非 Dockerfile `--mount=type=cache,...` 字符串。Phase A 的 `get_cache_mount_option()` 保持不变。

**交付文件：**

- `src/buildkit/build_llb/cache_store.rs`（修改）

  **新增方法**：

  ```rust
  impl BuildKitCacheStore {
      // Phase A：保持不变
      pub fn get_cache_mount_option(&self, key: &str) -> String { ... }

      /// Phase B：返回 LLB 缓存挂载规格
      /// 对齐 railpack `GetCacheMount()` → `llb.AddCacheMountOption()`
      #[cfg(feature = "llb")]
      pub fn get_cache_mount_spec(
          &mut self,
          key: &str,
          plan_cache: &Cache,
      ) -> MountSpec {
          let cache = self.get_cache(key, plan_cache);
          MountSpec::Cache {
              target: cache.plan_cache.directory.clone(),
              cache_id: cache.cache_key.clone(),
              sharing: match cache.plan_cache.cache_type {
                  CacheType::Locked => CacheSharingMode::Locked,
                  _ => CacheSharingMode::Shared,
              },
          }
      }
  }
  ```

**测试要求：**
- `get_cache_mount_spec()` 返回正确的 `MountSpec::Cache` 变体
- `target` 对应 `Cache.directory`
- `cache_id` 带 `unique_id-` 前缀
- `CacheType::Locked` 映射为 `CacheSharingMode::Locked`
- 默认类型映射为 `CacheSharingMode::Shared`
- Phase A `get_cache_mount_option()` 行为不受影响

---

### TB2.3 Layers LLB 策略

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | `rp:buildkit/build_llb/layers.go`（getFullStateFromLayers） |
| **依赖** | TB2.1 |

**描述：** 实现 Phase A `get_full_state_from_layers()` 的 LLB 等价版本。Phase A 中此函数输出 Dockerfile `FROM`/`COPY` 指令；Phase B 中输出 `OperationOutput`（通过 `llb::merge()` 或 `llb::copy()` 构建）。决策函数（`should_merge()`, `has_significant_overlap()`, `resolve_paths()`）完全复用，仅输出转换逻辑不同。

**交付文件：**

- `src/buildkit/build_llb/layers.rs`（修改）

  **新增函数**：

  ```rust
  /// Phase B：将多个 Layer 输入转换为 LLB OperationOutput
  /// 对齐 railpack `getFullStateFromLayers()` 的 LLB 路径
  #[cfg(feature = "llb")]
  pub fn get_full_state_from_layers_llb(
      layers: &[Layer],
      step_nodes: &HashMap<String, StepNode>,
      base_image: &OperationOutput,
  ) -> Result<OperationOutput>
  ```

  **实现逻辑**（复用 Phase A 的决策逻辑）：

  ```
  get_full_state_from_layers_llb(layers, step_nodes, base_image):
    if layers.len() == 1:
      return convert_single_layer_llb(layers[0], step_nodes, base_image)

    if should_merge(layers):                    // ← Phase A 函数，完全复用
      // Merge 策略：使用 llb::merge()
      let states: Vec<OperationOutput> = layers.map(|l| layer_to_llb_state(l, step_nodes))
      return llb::merge(states)
    else:
      // Copy 策略：使用 llb::copy() 逐层叠加
      let mut state = base_image.clone();
      for layer in layers:
          let (src_paths, dest_paths) = resolve_layer_paths(layer)  // ← Phase A 复用
          let src_state = layer_to_llb_state(layer, step_nodes)
          for (src, dest) in zip(src_paths, dest_paths):
              state = llb::copy(src_state, src, state, dest)
      return state
  ```

  **辅助函数**：

  | 函数 | 签名 | 说明 |
  |------|------|------|
  | `layer_to_llb_state` | `(layer, step_nodes) -> OperationOutput` | Layer → LLB State（step 引用 / image 引用 / local 引用） |
  | `convert_single_layer_llb` | `(layer, step_nodes, base) -> OperationOutput` | 单 Layer 转换 |

**测试要求：**
- 单 Layer（step 引用）正确返回对应 StepNode 的 `llb_state`
- 单 Layer（image 引用）返回 `llb::image()` 输出
- Merge 策略：多 Layer 合并后的 `OperationOutput` 包含 MergeOp
- Copy 策略：多 Layer 逐层 Copy 后的 `OperationOutput` 包含 FileOp 链
- `should_merge()` 决策结果与 Phase A 一致（共用函数，验证未破坏）
- Layer include/exclude 路径正确传递到 `llb::copy()` 的 src/dest

---

### TB2.4 convertNodeToLLB — 4 种命令转换

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | `rp:buildkit/build_llb/build_graph.go`（convertNodeToLLB, convertExecCommand, convertPathCommand, convertCopyCommand, convertFileCommand） |
| **依赖** | TB2.1, TB2.2, TB2.3 |

**描述：** 将 BuildPlan 中的 4 种 Command 类型转换为 LLB 操作。这是 Phase B 转换的核心——每种 Command 映射为一个或多个 LLB 原语调用。对齐 railpack `build_graph.go` 中的 `convertNodeToLLB()` 和 4 个 `convert*Command()` 方法。

**交付文件：**

- `src/buildkit/build_llb/mod.rs`（修改）

  **process_node_llb 实现**：

  ```rust
  #[cfg(feature = "llb")]
  fn process_node_llb(&mut self, node_name: &str) -> Result<()> {
      // 1. 确保所有父节点已处理（递归，复用 Phase A 的 in_progress 环检测）
      // 2. 合并父节点 output_env → 当前节点 input_env（复用 Phase A）
      // 3. 获取起始 LLB State（get_node_starting_state_llb）
      // 4. 逐命令转换为 LLB 操作
      // 5. 存储结果到 node.llb_state
  }
  ```

  **get_node_starting_state_llb**（对齐 Phase A 的 `get_node_starting_state`）：

  ```rust
  #[cfg(feature = "llb")]
  fn get_node_starting_state_llb(&self, node: &StepNode) -> Result<OperationOutput> {
      // 调用 get_full_state_from_layers_llb() 获取基础 State
      // 不需要 WORKDIR/ENV 指令——LLB 在 Exec 时通过 Meta 设置
  }
  ```

  **4 种命令转换方法**：

  | 方法 | Command 类型 | LLB 操作 | Phase A 对应 |
  |------|-------------|----------|-------------|
  | `convert_exec_command_llb` | `Exec` | `ExecBuilder::new().env().cwd().add_cache_mount().add_secret_env().root()` | `convert_exec_command` → `RUN` |
  | `convert_path_command_llb` | `Path` | 无 LLB 操作，仅更新 `output_env.push_path()` | `convert_path_command` → `ENV PATH=` |
  | `convert_copy_command_llb` | `Copy` | `llb::copy(src, src_path, state, dest_path)` | `convert_copy_command` → `COPY --from=` |
  | `convert_file_command_llb` | `File` | `llb::make_file(state, path, content, mode)` | `convert_file_command` → `COPY <<EOF` |

  **convert_exec_command_llb 详细实现**（最复杂的转换）：

  > 注意：`caches`、`secrets`、`variables` 均在 `Step` 级别而非 `ExecCommand` 级别。
  > 对齐 Phase A `convert_exec_command(&mut self, cmd, step_caches, step_secrets, secrets_hash)` 的签名。

  ```rust
  #[cfg(feature = "llb")]
  fn convert_exec_command_llb(
      &mut self,
      state: OperationOutput,
      exec: &ExecCommand,
      step: &Step,
      node: &StepNode,
  ) -> Result<OperationOutput> {
      let mut builder = ExecBuilder::new(state, vec![
          "/bin/sh".into(), "-c".into(), exec.cmd.clone(),
      ])
      .cwd("/app");

      // 环境变量：input_env + step.variables
      for (key, value) in &node.input_env.env_vars {
          builder = builder.env(key, value);
      }
      for (key, value) in &step.variables {
          builder = builder.env(key, value);
      }

      // PATH 环境变量
      let path = build_path_env(&node.input_env.path_list);
      builder = builder.env("PATH", &path);

      // 缓存挂载（caches 在 Step 上，不在 ExecCommand 上）
      for cache_key in &step.caches {
          let plan_cache = self.plan.get_cache(cache_key)?;
          let mount_spec = self.cache_store.get_cache_mount_spec(cache_key, &plan_cache);
          builder = builder.add_mount(mount_spec);
      }

      // Secret 挂载（secrets 在 Step 上，不在 ExecCommand 上）
      for secret_name in &step.secrets {
          builder = builder.add_secret_env(secret_name, secret_name);
      }

      // Secret 失效：将 hash 作为环境变量注入 ExecOp 的 Meta.env
      // （不通过 Secret mount，避免 buildkitd 回调请求不存在的 secret）
      if !step.secrets.is_empty() {
          if let Some(hash) = &self.secrets_hash {
              builder = builder.env("_SECRET_HASH", hash);
          }
      }

      Ok(builder.root())
  }
  ```

**测试要求：**
- Exec 命令：args 为 `["/bin/sh", "-c", "npm install"]`
- Exec 环境变量：input_env + step.variables 正确合并
- Exec 缓存挂载：每个 cache_ref 生成对应 `MountSpec::Cache`
- Exec Secret 挂载：每个 secret 生成对应 `MountSpec::SecretEnv`
- Exec Secret 失效：有 secrets_hash 时注入 `_SECRET_HASH` 环境变量（非 Secret mount）
- Path 命令：不生成 LLB 操作，仅更新 output_env
- Copy 命令（image 源）：生成 `llb::copy()` 从 image OperationOutput 复制
- Copy 命令（local 源）：生成 `llb::copy()` 从 local OperationOutput 复制
- File 命令：生成 `llb::make_file()` 包含正确内容和路径
- 混合命令序列：Exec + Path + Copy 按序执行，State 正确传递

---

### TB2.5 Deploy 阶段 + convert_plan_to_llb 入口

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | `rp:buildkit/convert.go`（ConvertPlanToLLB）, `rp:buildkit/build_llb/build_graph.go`（buildDeployState） |
| **依赖** | TB2.4 |

**描述：** 实现 LLB 路径的 Deploy 阶段和顶层编排函数。Deploy 阶段将构建产出从 build 阶段复制到新的基础镜像，设置运行时配置。`convert_plan_to_llb()` 是 Phase B 的入口函数，对齐 railpack `ConvertPlanToLLB()`。

**交付文件：**

- `src/buildkit/build_llb/mod.rs`（修改）

  **build_deploy_llb 实现**：

  ```rust
  #[cfg(feature = "llb")]
  fn build_deploy_llb(&self) -> Result<pb::Definition> {
      // 1. 创建 deploy 基础镜像 State
      let deploy_image = llb::image_with_platform(
          &self.plan.deploy.image,
          &self.platform,
      );

      // 2. 从构建步骤复制产出到 deploy 镜像
      let mut state = deploy_image;
      for layer in &self.plan.deploy.inputs {
          let src_state = self.get_layer_llb_state(layer)?;
          let (src_paths, dest_paths) = resolve_deploy_paths(layer);
          for (src, dest) in zip(src_paths, dest_paths) {
              state = llb::copy(src_state.clone(), &src, state, &dest);
          }
      }

      // 3. marshal 为 Definition
      llb::marshal(&state)
  }
  ```

- `src/buildkit/convert.rs`（修改）

  **新增函数**：

  ```rust
  /// Phase B：BuildPlan → LLB Definition 转换
  /// 对齐 railpack `ConvertPlanToLLB()` → `(llb.State, Image, error)`
  #[cfg(feature = "llb")]
  pub fn convert_plan_to_llb(
      plan: &BuildPlan,
      opts: &ConvertPlanOptions,
  ) -> Result<(pb::Definition, ImageConfig)> {
      // 1. 创建 BuildKitCacheStore
      let cache_store = BuildKitCacheStore::new(&opts.cache_key);

      // 2. 创建 BuildGraph
      let mut graph = BuildGraph::new(
          plan,
          cache_store,
          opts.secrets_hash.clone(),
          opts.platform.clone(),
      )?;

      // 3. 转换为 LLB Definition
      // to_llb() 需要返回 output_env 供 build_image_config 使用
      // （对齐 Phase A 的 to_dockerfile() 返回 BuildGraphOutput { dockerfile, output_env }）
      let (definition, output_env) = graph.to_llb()?;

      // 4. 构建 ImageConfig（复用 Phase A）
      let image_config = build_image_config(&output_env, &plan.deploy, &opts.platform);

      Ok((definition, image_config))
  }
  ```

  > `convert_plan_to_dockerfile()`（Phase A）保持不变，两个函数并存。

**测试要求：**
- Deploy 阶段：基础镜像正确（`plan.deploy.image`）
- Deploy 阶段：构建产出通过 `llb::copy()` 复制到 deploy State
- `convert_plan_to_llb()` 编排正确：返回 `(Definition, ImageConfig)`
- ImageConfig 与 Phase A `convert_plan_to_dockerfile()` 的输出一致
- marshal 后的 Definition 非空，包含正确数量的 Op

---

### TB2.6 buildctl + LLB stdin 中间验证

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | — |
| **railpack 参考** | — |
| **依赖** | TB2.5 |

**描述：** **关键里程碑**——在实现 gRPC 客户端（Phase B-3）之前，先通过 `buildctl build --local context=... < llb.pb` 验证生成的 LLB Definition 能被 BuildKit 正确执行。这大幅降低了 Phase B-3 的风险，因为如果 LLB 本身有错误，gRPC 层会更难调试。

**交付文件：**

- `src/buildkit/client.rs`（修改）

  **新增方法**：

  ```rust
  impl BuildKitClient {
      // Phase A：保持不变
      pub async fn build(&self, request: BuildRequest) -> Result<BuildOutput> { ... }

      /// Phase B 中间验证：通过 buildctl 的 stdin 发送 LLB Definition
      /// 命令：buildctl build --local context={dir} --progress {mode} < llb.pb
      #[cfg(feature = "llb")]
      pub async fn build_from_llb(
          &self,
          definition: &pb::Definition,
          request: &LlbBuildRequest,
      ) -> Result<BuildOutput> {
          // 1. 将 Definition 序列化为 protobuf bytes
          let llb_bytes = serialize_definition(definition)?;

          // 2. 组装 buildctl 命令
          // buildctl --addr {addr} build
          //   --no-cache        (可选)
          //   --local context={context_dir}
          //   --secret id={key},env={key}   (每个 secret 一个 --secret 参数)
          //   --progress {mode}
          //   --output type=image,name={name},push={push}
          //   < llb.pb           (通过 stdin 传入)

          // 3. 写入子进程 stdin，等待完成
          // 4. 解析退出码
      }
  }
  ```

  **LlbBuildRequest**：

  ```rust
  #[cfg(feature = "llb")]
  pub struct LlbBuildRequest {
      pub context_dir: PathBuf,
      pub image_name: Option<String>,
      pub output_dir: Option<PathBuf>,
      pub push: bool,
      pub progress_mode: String,
      /// Secret 映射：name → value，通过 buildctl `--secret id=KEY,env=KEY` 注入
      /// （buildkitd 执行 LLB 中的 Secret mount 时仍会回调请求 secret）
      pub secrets: HashMap<String, String>,
  }
  ```

- `tests/integration_llb.rs`（新增，`#[ignore]`）

  ```rust
  /// 中间验证：BuildPlan → LLB → buildctl stdin → OCI 镜像
  #[tokio::test]
  #[ignore]  // 需要 buildkitd + buildctl
  async fn test_llb_build_via_buildctl_stdin() {
      // 1. 构造 Node.js fixture 的 BuildPlan
      // 2. convert_plan_to_llb() → Definition
      // 3. build_from_llb() → BuildOutput
      // 4. 验证镜像可正常运行
  }

  // 注意：完整的多 fixture 等价性验证留给 TB4.3（tests/equivalence_tests.rs）
  // 此处仅做 smoke test：验证 LLB 可以通过 buildctl stdin 成功构建
  ```

**测试要求：**
- （非 ignore）`build_from_llb()` 命令行参数组装正确性
- （非 ignore）Definition 序列化为 bytes 的 round-trip 测试
- （非 ignore）`LlbBuildRequest` 各字段映射到 buildctl 参数正确
- （ignore）Node.js fixture → LLB → buildctl stdin → OCI 镜像构建成功（smoke test）
- 完整等价性验证由 TB4.3 覆盖，此处不重复

---

## 与 railpack 的已知差异

| 方面 | railpack (Go) | arcpack Phase B-2 (Rust) | 原因 |
|------|--------------|--------------------------|------|
| 转换入口 | `ConvertPlanToLLB()` 直接返回 `llb.State` | `convert_plan_to_llb()` 返回 `pb::Definition`（已 marshal） | Rust 无 `llb.State` 延迟求值，提前 marshal |
| Deploy 阶段 | `buildDeployState()` 返回 `llb.State` | `build_deploy_llb()` 直接 marshal 为 Definition | 同上 |
| 中间验证 | 无（Go SDK 直接 gRPC） | `buildctl + LLB stdin` 中间验证步骤 | 降低风险，早期验证 LLB 正确性 |
| 双路径并存 | 无 Dockerfile 路径 | Phase A（Dockerfile）+ Phase B（LLB）并存 | Phase A 永久保留作为 fallback |
| Secret 失效 | 通过 `llb.AddSecret()` 挂载 hash 文件 | 将 hash 作为环境变量 `_SECRET_HASH` 注入 ExecOp 的 Meta.env | 避免通过 Secret mount 传递不存在于 Session SecretsProvider 中的值 |

---

## Phase B-2 Gate

**执行命令：**
```bash
cargo check --features llb
cargo test --features llb
cargo test --features llb -- --ignored   # 需要 buildkitd + buildctl
```

**验收清单：**
- [x] `cargo check --features llb` 无错误无警告
- [x] Phase A 所有测试仍然通过（`cargo test` 无 llb feature）
- [x] StepNode `llb_state` 字段在处理后正确存储 `OperationOutput`
- [x] `get_cache_mount_spec()` 返回正确的 `MountSpec::Cache`
- [x] `get_full_state_from_layers_llb()` merge/copy 策略与 Phase A 决策一致
- [x] 4 种命令转换（Exec/Path/Copy/File）生成正确的 LLB 操作
- [x] Exec 转换包含 cache mount + secret mount + secret 失效
- [x] Deploy 阶段正确复制构建产出到新基础镜像
- [x] `convert_plan_to_llb()` 返回有效的 `(Definition, ImageConfig)`
- [ ] `build_from_llb()` 通过 buildctl stdin 成功构建（`#[ignore]` smoke test）
- [ ] 完整等价性验证由 TB4.3 覆盖
- [x] 预计 ~25 个测试用例全部通过
