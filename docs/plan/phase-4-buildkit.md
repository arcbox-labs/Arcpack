# Phase 4: BuildKit 集成

> [← 返回目录](./README.md) | 上一阶段：[← Phase 3](./phase-3-cli.md) | 下一阶段：[Phase 5 →](./phase-5-providers.md)

**目标：** 完成从 BuildPlan 到 OCI 镜像的完整构建流程（Phase A：通过 Dockerfile + buildctl CLI）。

**前置条件：** Phase 3 全部完成

## 任务依赖图

```
T4.1 (graph/ 通用 DAG)
 └──► T4.2 (StepNode + BuildEnv)
       │
       ├──► T4.4 (layers 合并策略)
       ├──► T4.5 (cache_store 缓存映射)
       │       │
       │       ▼
       └──► T4.3 (BuildGraph + Dockerfile 生成)   ← 依赖 T4.2, T4.4, T4.5
             │
             ▼
       T4.6 (image config + platform + convert 编排)
             │
             ▼
       T4.7 (DaemonManager)
        └──► T4.8 (BuildKit client: buildctl)
              └──► T4.9 (build 命令集成)
```

## 任务列表

### T4.1 graph/ 通用 DAG + 拓扑排序

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | `rp:buildkit/graph/graph.go`, `rp:buildkit/graph/graph_test.go` |
| **依赖** | Phase 3 |

**描述：** 通用有向无环图数据结构，支持双向边、拓扑排序和传递归约。

**交付文件：**
- `src/graph/mod.rs` — `Node` trait + `Graph` 结构体

  **Node trait**（对齐 railpack `graph.Node` interface）：

  ```rust
  pub trait Node {
      fn name(&self) -> &str;
      fn parents(&self) -> &[Arc<RefCell<dyn Node>>];
      fn children(&self) -> &[Arc<RefCell<dyn Node>>];
      fn set_parents(&mut self, parents: Vec<...>);
      fn set_children(&mut self, children: Vec<...>);
  }
  ```

  > 注：Rust 中 Node trait 的具体生命周期和所有权设计在实现时确定。可简化为 Graph 内部管理边关系（`HashMap<String, Vec<String>>`），Node trait 只需 `name()`，边存储在 Graph 侧。

  **Graph 结构体**（对齐 railpack `graph.Graph`）：

  | 方法 | 签名 | 说明 | railpack 对应 |
  |------|------|------|--------------|
  | `add_node` | `(name, node)` | 注册节点 | `AddNode()` |
  | `get_node` | `(name) -> Option<&T>` | 按名称查找 | `GetNode()` |
  | `get_nodes` | `() -> &HashMap` | 返回所有节点 | `GetNodes()` |
  | `add_edge` | `(parent, child)` | 添加有向边（双向记录） | 隐含在 `SetParents/SetChildren` |
  | `compute_processing_order` | `() -> Result<Vec<&T>>` | **DFS-based 拓扑排序**（对齐 railpack） | `ComputeProcessingOrder()` |
  | `compute_transitive_dependencies` | `()` | **传递归约**（移除冗余边） | `ComputeTransitiveDependencies()` |
  | `print_graph` | `()` | 调试输出图结构 | `PrintGraph()` |

  **拓扑排序算法**（对齐 railpack，DFS-based，**不是 Kahn 算法**）：

  ```
  visited = {}
  temp = {}       // 灰色标记，检测回边
  order = []

  fn visit(node):
      if temp[node]: return Err("cycle detected")
      if visited[node]: return Ok(())
      temp[node] = true
      for parent in node.parents:
          visit(parent)?
      temp.remove(node)
      visited[node] = true
      order.push(node)

  // 从叶节点（无 children）开始
  for node in nodes where children.is_empty():
      visit(node)?
  // 捕获剩余未访问节点
  for node in nodes where !visited[node]:
      visit(node)?
  ```

  **传递归约**（对齐 railpack `ComputeTransitiveDependencies()`）：
  - 遍历每个节点 N 的 parents，若 parent P 可通过 N 的其他 parent 间接到达，则移除 P→N 边
  - 例：A→B, A→C, B→C 中，A→C 是冗余的（A→B→C 已覆盖），移除 A→C
  - 目的：最小化依赖图，减少不必要的串行约束

**测试要求：**
- 线性依赖链排序正确性
- 菱形依赖（A→B, A→C, B→D, C→D）排序测试
- 环检测测试（A→B→C→A 应返回错误）
- 空图和单节点测试
- 传递归约测试（A→B→C + A→C，归约后 A→C 被移除）
- `get_node()` 查找存在/不存在节点

---

### T4.2 StepNode + BuildEnvironment

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | `rp:buildkit/build_llb/step_node.go`, `rp:buildkit/build_llb/build_env.go` |
| **依赖** | T4.1 |

**描述：** 构建图中的节点表示（含处理状态和双环境）和累积环境变量管理。

**交付文件：**

- `src/buildkit/build_llb/step_node.rs` — StepNode

  **字段**（对齐 railpack `StepNode`）：

  ```rust
  pub struct StepNode {
      pub step: Step,                    // 原始 Step（owned 或引用）
      pub dockerfile_stage: String,      // Phase A: 生成的 Dockerfile 阶段片段
      pub processed: bool,               // 是否已处理完成
      pub in_progress: bool,             // 处理中标记（递归环检测）
      pub input_env: BuildEnvironment,   // 从父节点继承的环境
      pub output_env: BuildEnvironment,  // 本步骤处理后的累积环境
  }
  ```

  > railpack 中 `State` 字段存储 `llb.State`，Phase A 中对应为 Dockerfile 阶段片段字符串。

  **方法：**
  - `new(step) -> Self` — 构造，初始化空环境
  - `name() -> &str` — 返回 `step.name`（实现 graph::Node）
  - `get_path_list() -> &[String]` — 返回累积 PATH 条目

- `src/buildkit/build_llb/build_env.rs` — BuildEnvironment

  **字段**（对齐 railpack `BuildEnvironment`）：

  ```rust
  pub struct BuildEnvironment {
      pub path_list: Vec<String>,              // PATH 条目（prepend 顺序）
      pub env_vars: HashMap<String, String>,   // 环境变量映射
  }
  ```

  **方法**（对齐 railpack）：

  | 方法 | 说明 | railpack 对应 |
  |------|------|--------------|
  | `new()` | 创建空环境 | `NewGraphEnvironment()` |
  | `merge(&mut self, other)` | 合并另一个环境：追加 path_list，**深拷贝**合并 env_vars | `Merge()` |
  | `push_path(path)` | 前置 PATH 条目（影响最终 `ENV PATH=` 顺序） | `PushPath()` |
  | `add_env_var(key, value)` | 设置环境变量 | `AddEnvVar()` |

  **双环境传播机制**（对齐 railpack 核心设计）：

  ```
  processNode(node):
    1. node.input_env = 合并所有父节点的 output_env
    2. 处理 node 的 commands → 更新 node.output_env
       - Path 命令 → push_path() 到 output_env
       - Exec 命令 → 继承 input_env + step.variables
    3. 子节点将继承此 node.output_env
  ```

  这保证了环境变量沿 DAG 向下正确传播。

**测试要求：**
- StepNode 从 Step 构造，初始状态 processed=false, in_progress=false
- BuildEnvironment `merge()` 深拷贝测试（修改源不影响目标）
- `push_path()` 前置顺序测试：先 push "/a" 再 push "/b"，path_list = ["/a", "/b"]
- 环境变量覆盖测试：merge 时后者覆盖前者
- 双环境传播：父节点 output_env 正确合并为子节点 input_env

---

### T4.3 BuildGraph + Dockerfile 生成

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10, Arch§8.5 |
| **railpack 参考** | `rp:buildkit/build_llb/build_graph.go` |
| **依赖** | T4.2, T4.4, T4.5 |

**描述：** BuildPlan → Dockerfile 转换（Phase A 核心）。从 Step DAG 生成多阶段 Dockerfile，处理命令转换、环境传播和 secret 挂载。

**交付文件：**
- `src/buildkit/build_llb/mod.rs` — BuildGraph + Dockerfile 生成

  **BuildGraph 结构体**（对齐 railpack `BuildGraph`）：

  ```rust
  pub struct BuildGraph<'a> {
      graph: Graph<StepNode>,
      cache_store: BuildKitCacheStore,
      plan: &'a BuildPlan,
      secrets_hash: Option<String>,      // secret 值 hash（缓存失效用）
      platform: Platform,
  }
  ```

  **构造流程** `new(plan, cache_store, secrets_hash, platform) -> Result<Self>`（对齐 railpack `NewBuildGraph`）：

  1. 为 `plan.steps` 中每个 Step 创建 StepNode，加入 graph
  2. 遍历每个 Step 的 `inputs`，若 Layer 有 `step` 引用 → 在 graph 中添加边
  3. 调用 `graph.compute_transitive_dependencies()` 移除冗余边

  **Dockerfile 生成** `to_dockerfile() -> Result<String>`：

  1. 调用 `graph.compute_processing_order()` 获取拓扑排序
  2. 逐节点调用 `process_node()` 生成 Dockerfile 阶段
  3. 生成 deploy 阶段（从 `plan.deploy` 构建最终镜像）
  4. 拼接所有阶段，头部添加 `# syntax=docker/dockerfile:1`

  **核心内部方法**（对齐 railpack `build_graph.go`）：

  `process_node(node) -> Result<()>`：
  - 确保所有父节点已处理（递归调用 `process_node`）
  - 使用 `in_progress` 标记检测处理循环
  - 合并父节点 `output_env` → 当前节点 `input_env`
  - 调用 `convert_node_to_dockerfile(node)` 生成阶段内容
  - 标记 `processed = true`

  `get_node_starting_state(node) -> DockerfileStage`：
  - 调用 T4.4 的 `get_full_state_from_layers(node.step.inputs)` 决定 FROM 和 COPY 指令
  - 添加 `WORKDIR /app`
  - 从 `input_env` + `step.variables` 生成 `ENV` 指令
  - 从 `input_env.path_list` 生成 `ENV PATH=...` 指令

  **命令转换方法**（4 种，对齐 railpack）：

  | 方法 | Command 类型 | Dockerfile 输出 | 特殊处理 |
  |------|-------------|----------------|---------|
  | `convert_exec_command` | `Exec` | `RUN [--mount=type=cache,...] [--mount=type=secret,...] cmd` | cache mount + secret mount + secret 失效 |
  | `convert_path_command` | `Path` | 更新 `output_env.push_path()` + `ENV PATH=...` | 不直接生成指令，通过环境传播 |
  | `convert_copy_command` | `Copy` | `COPY [--from=image] src dest` | 区分 image/local 来源 |
  | `convert_file_command` | `File` | `COPY <<EOF` heredoc 或 asset COPY | 从 step.assets 获取内容 |

  **Secret 处理**（Phase A Dockerfile 映射，对齐 railpack secret 机制）：

  - Plan 中声明的 secrets → `RUN --mount=type=secret,id={name}` 语法
  - 构建时通过 `buildctl --secret id={name},env={NAME}` 传入
  - **Secret 失效机制**（对齐 railpack `getSecretInvalidationMountOptions`）：
    - 若 step 声明了 secrets，生成一个 secret hash mount
    - `"*"` 表示挂载全部 secrets 的 hash
    - 否则只挂载该 step 使用的 secrets 的 hash
    - 目的：secret 值变化时自动使相关步骤缓存失效

**测试要求：**
- 构造含 packages/install/build 三步骤的 BuildPlan，`to_dockerfile()` 快照测试
- 验证 `FROM ... AS {step_name}` / `RUN` / `COPY --from` / `ENV` 指令的正确性和顺序
- deploy 阶段包含正确的 `COPY --from` 和 `CMD`
- 环境传播测试：父步骤 PATH 在子步骤的 ENV 中出现
- secret mount 语法正确性：`--mount=type=secret,id=...`
- cache mount 与 secret mount 组合在同一 RUN 指令中

---

### T4.4 layers 合并策略

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | `rp:buildkit/build_llb/layers.go`, `rp:buildkit/build_llb/layers_test.go` |
| **依赖** | T4.2 |

**描述：** Layer 输入的智能合并策略，决定如何将多个 Layer 转换为 Dockerfile 的 COPY 指令或 multi-stage 结构。对齐 railpack `layers.go` 的完整决策逻辑。

**交付文件：**
- `src/buildkit/build_llb/layers.rs`

  **核心函数**（对齐 railpack `layers.go`）：

  `get_full_state_from_layers(layers, step_nodes) -> DockerfileFragment`：
  - 单 layer → 直接返回其 FROM / COPY
  - 多 layer → 调用 `should_merge(layers)` 决定策略：
    - `true` → `get_merge_state()` — 使用 multi-stage COPY（高效，fewer layers）
    - `false` → `get_copy_state()` — 逐层 COPY（安全，无重复）

  `should_merge(layers) -> bool`（对齐 railpack `shouldLLBMerge()`）：
  返回 `false` 的条件（任一满足则不合并）：
  1. 非首层无 include 路径（完整基础替换）
  2. 任何层包含根路径 `"/"`
  3. 任何层是 local 引用
  4. 两层之间存在**显著重叠**

  `has_significant_overlap(layer1, layer2) -> bool`（对齐 railpack `hasSignificantOverlap()`）：
  - 检测两个 Layer 的 include 路径是否冲突
  - 重叠类型：精确匹配（`/app` vs `/app`）或前缀包含（`/app/dist` 在 `/app` 内）
  - 被 exclude 模式覆盖的重叠**不算**显著重叠
  - 使用 `is_path_excluded(path, excludes)` 检查

  `resolve_paths(path, is_local) -> (src, dest)`（对齐 railpack `resolvePaths()`）：
  - local 路径：只保留 basename（`/path/to/file` → `/app/file`）
  - 绝对容器路径：保持原样
  - 相对路径：前缀 `/app`

  `copy_layer_paths(dest, src, filter, is_local) -> Vec<DockerfileLine>`：
  - 对 filter.include 中每个路径调用 `resolve_paths()`
  - 生成 `COPY [--from=...] src dest` 指令
  - 处理 `filter.exclude` 为 `.dockerignore` 兼容语法

**测试要求：**
- `should_merge()` 各种条件的决策正确性（对齐 railpack layers_test.go）
- `has_significant_overlap()` 路径重叠检测（精确匹配、前缀包含、exclude 覆盖）
- `resolve_paths()` 对 local/absolute/relative 路径的转换
- 各种 Layer 类型（step/image/local）→ COPY 指令的单元测试
- merge vs copy 两种策略的 Dockerfile 片段输出对比
- Filter include/exclude 转路径列表测试

---

### T4.5 cache_store 缓存映射

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10, Arch 附录 B |
| **railpack 参考** | `rp:buildkit/build_llb/cache_store.go` |
| **依赖** | T4.2 |

**描述：** BuildKit 持久化缓存挂载管理。

**交付文件：**
- `src/buildkit/build_llb/cache_store.rs`

  **类型定义**（对齐 railpack `BuildKitCacheStore`）：

  ```rust
  pub struct BuildKitCache {
      pub cache_key: String,         // 带前缀的缓存键
      pub plan_cache: Cache,         // 原始 Cache 定义（directory + type）
  }

  pub struct BuildKitCacheStore {
      unique_id: String,             // 缓存键前缀（多租户隔离）
      cache_map: HashMap<String, BuildKitCache>,  // 缓存注册表（memoization）
  }
  ```

  **方法**（对齐 railpack）：

  | 方法 | 说明 |
  |------|------|
  | `new(unique_id)` | 构造，设置键前缀 |
  | `get_cache(key, plan_cache) -> &BuildKitCache` | 获取缓存（有则复用，无则创建），key 自动添加 `unique_id-` 前缀 |
  | `get_cache_mount_option(key) -> String` | 生成 `--mount=type=cache,target={dir},id={key}[,sharing=locked]` 语法 |

  **Dockerfile 映射**（Phase A）：
  - `RUN --mount=type=cache,target=/root/.npm,id=npm-cache cmd`
  - 锁定模式：`sharing=locked`（对应 railpack `CacheMountLocked`）
  - 共享模式：默认（对应 railpack `CacheMountShared`）
  - 需要 Dockerfile 头部 `# syntax=docker/dockerfile:1`

**测试要求：**
- `get_cache()` memoization：同 key 返回相同实例
- `unique_id` 前缀正确拼接
- shared/locked 两种模式的 mount 语法生成
- cache mount 字符串格式正确性

---

### T4.6 image config + platform + convert 编排

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | `rp:buildkit/image.go`, `rp:buildkit/platform.go`, `rp:buildkit/convert.go` |
| **依赖** | T4.3 |

**描述：** OCI Image config、平台解析和 BuildPlan → Dockerfile 的顶层编排。对齐 railpack `convert.go`（独立编排模块）、`image.go`、`platform.go`。

**交付文件：**

- `src/buildkit/image.rs` — Image config 构建

  **Image config 构建逻辑**（对齐 railpack `convert.go` 中的 `getImageEnv()`）：

  从以下来源合并最终的 ENV（按顺序，后者覆盖前者）：

  ```
  最终 ENV 合并来源：
  1. plan.deploy.paths         — Provider 声明的运行时 PATH
  2. graph_env.path_list       — 构建过程中累积的 PATH
  3. 系统默认 PATH              — /usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
  → 拼接、去重为单个 PATH=... 条目

  4. graph_env.env_vars        — 构建过程中累积的变量
  5. plan.deploy.variables     — Provider 声明的运行时变量
  → 合并为 KEY=value 列表，排序
  ```

  **Dockerfile deploy 阶段输出**（对齐 railpack `Image` struct → Dockerfile 尾部）：

  | 项目 | Dockerfile 指令 | 来源 |
  |------|----------------|------|
  | 工作目录 | `WORKDIR /app` | 固定 |
  | 环境变量 | `ENV KEY=value` | 合并后的 ENV 列表 |
  | 入口点 | `ENTRYPOINT ["/bin/bash", "-c"]` | 固定（对齐 railpack） |
  | 启动命令 | `CMD ["start_cmd"]` | `plan.deploy.start_cmd`，无则 `["/bin/bash"]` |

- `src/buildkit/platform.rs` — 平台字符串解析

  **平台解析**（对齐 railpack `ParsePlatformWithDefaults()`）：

  ```rust
  pub fn parse_platform_with_defaults(platform_str: &str) -> Result<Platform> {
      if platform_str.is_empty() {
          // 默认匹配宿主机架构（不是硬编码 amd64）
          // Intel (x86_64) → linux/amd64
          // ARM (aarch64)  → linux/arm64/v8
          return Ok(detect_host_platform());
      }
      parse_platform(platform_str)  // "linux/amd64" → Platform { os, arch, variant }
  }
  ```

  > 注意：硬编码 `linux/amd64` 会导致 Apple Silicon 用户走 QEMU 模拟，构建慢 10x。

- `src/buildkit/convert.rs` — 顶层编排函数

  **ConvertPlanOptions**（对齐 railpack `ConvertPlanOptions`）：

  ```rust
  pub struct ConvertPlanOptions {
      pub secrets_hash: Option<String>,
      pub platform: Platform,
      pub cache_key: String,
  }
  ```

  **核心函数** `convert_plan_to_dockerfile(plan, opts) -> Result<(String, ImageConfig)>`：

  1. 创建 `BuildKitCacheStore::new(opts.cache_key)`
  2. 创建 `BuildGraph::new(plan, cache_store, opts.secrets_hash, opts.platform)?`
  3. 调用 `graph.to_dockerfile()?` 获取 Dockerfile 内容
  4. 构建 `ImageConfig`（合并 graph 环境 + plan.deploy）
  5. 返回 `(dockerfile_content, image_config)`

  > 此函数是 BuildGraph ↔ BuildKit client 之间的粘合层，对应 railpack 的 `ConvertPlanToLLB()`。Phase A 输出 Dockerfile + ImageConfig，Phase B 输出 LLB State + Image。

**测试要求：**
- Image ENV 合并测试：deploy paths + graph env + system defaults 正确合并去重
- 无 start_cmd 时 CMD 为 `["/bin/bash"]`
- 平台解析合法/非法输入测试
- 平台默认值匹配宿主架构（非硬编码 amd64）
- `convert_plan_to_dockerfile()` 编排正确性：给定 BuildPlan 输出 Dockerfile + ImageConfig
- Deploy 阶段 Dockerfile 快照测试

---

### T4.7 DaemonManager（buildkitd 生命周期管理）

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | BK§3.1, BK§3.2, BK§3.4, Arch§3.10 |
| **railpack 参考** | 无直接对应（railpack 通过 `BUILDKIT_HOST` 连接外部 daemon；arcpack 独有子进程模式） |
| **依赖** | Phase 3 |

**描述：** buildkitd 守护进程管理，通过 trait 抽象支持 mock 测试。支持**两种模式**：子进程模式（arcpack 独有）和外部连接模式（对齐 railpack `BUILDKIT_HOST`）。

**交付文件：**
- `src/buildkit/daemon.rs`

  **DaemonManager async trait**：

  ```rust
  #[async_trait]
  pub trait DaemonManager: Send + Sync {
      async fn start(&mut self) -> Result<()>;
      async fn wait_ready(&self, timeout: Duration) -> Result<()>;
      async fn stop(&mut self) -> Result<()>;
      fn is_running(&self) -> bool;
      fn socket_addr(&self) -> &str;  // "unix:///tmp/buildkit.sock" 或 "tcp://host:port"
  }
  ```

  **两种实现**：

  1. `SubprocessDaemonManager`（arcpack 独有）：
     - `start()`: tokio::process::Command 启动 buildkitd，指定 `--addr unix://{socket_path}`
     - `wait_ready()`: 循环探测 Unix socket 连接（200ms 间隔 + 超时）
     - `stop()`: SIGTERM 优雅停止 + 超时 SIGKILL + 清理 socket 文件
     - 适用场景：ArcBox Spot 实例（用完即销毁）

  2. `ExternalDaemonManager`（对齐 railpack `BUILDKIT_HOST`）：
     - `start()`: no-op（外部 daemon 已运行）
     - `wait_ready()`: 探测 socket/TCP 连接
     - `stop()`: no-op（不管理外部 daemon 生命周期）
     - 适用场景：开发环境、CI（BuildKit 作为 Docker 容器运行）

  **选择逻辑**：
  - 若 `BUILDKIT_HOST` 环境变量存在 → `ExternalDaemonManager`
  - 否则 → `SubprocessDaemonManager`

**测试要求：**
- MockDaemonManager 测试 wait_ready 超时返回 DaemonTimeout 错误
- MockDaemonManager 测试 start → wait_ready → stop 生命周期
- SubprocessDaemonManager 构造参数验证（socket 路径格式）
- ExternalDaemonManager 从 `BUILDKIT_HOST` 解析地址
- 实际启动 buildkitd 的测试标记 `#[ignore]`

---

### T4.8 BuildKit client（buildctl CLI 封装）

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | BK§3.3, BK§3.5 |
| **railpack 参考** | `rp:buildkit/build.go`（BuildWithBuildkitClient 函数，arcpack Phase A 映射为 buildctl CLI 调用） |
| **依赖** | T4.7 |

**描述：** 通过 buildctl CLI 执行构建。对应 railpack `BuildWithBuildkitClient()` 的 Phase A 实现。

**交付文件：**
- `src/buildkit/client.rs`

  **BuildRequest**（对齐 railpack `BuildWithBuildkitClientOptions`）：

  ```rust
  pub struct BuildRequest {
      pub context_dir: PathBuf,              // 构建上下文目录
      pub dockerfile_content: String,        // Dockerfile 内容（写入临时文件）
      pub image_name: Option<String>,        // 输出镜像名
      pub output_dir: Option<PathBuf>,       // 输出到本地目录（替代 docker load）
      pub push: bool,                        // 是否推送到 registry
      pub platform: String,                  // 目标平台
      pub progress_mode: String,             // 进度模式：auto/plain/tty
      pub cache_import: Option<String>,      // 缓存导入配置
      pub cache_export: Option<String>,      // 缓存导出配置
      pub secrets: HashMap<String, String>,  // Secret 键值对
  }

  pub struct BuildOutput {
      pub image_digest: Option<String>,
      pub duration: Duration,
  }
  ```

  **BuildKitClient**：

  ```rust
  pub struct BuildKitClient {
      addr: String,           // DaemonManager.socket_addr()
      buildctl_path: String,  // buildctl 二进制路径（默认 "buildctl"）
  }
  ```

  `build(request) -> Result<BuildOutput>`：
  1. 将 `dockerfile_content` 写入临时文件
  2. 组装 buildctl 命令行参数：
     ```
     buildctl --addr {addr} build
       --frontend dockerfile.v0
       --local context={context_dir}
       --local dockerfile={temp_dir}
       --progress {progress_mode}
       [--output type=image,name={name},push=true]
       [--output type=local,dest={output_dir}]
       [--secret id={key},env={KEY} ...]
       [--export-cache {cache_export}]
       [--import-cache {cache_import}]
     ```
  3. 执行子进程，实时输出构建日志（stdout/stderr inherit）
  4. 解析退出码，非 0 返回 `BuildFailed` 错误
  5. 清理临时文件

**测试要求：**
- buildctl 命令行参数组装正确性验证（各种组合）
- secret 参数格式：`--secret id=MY_SECRET,env=MY_SECRET`
- 无 image_name 时不添加 --output
- cache import/export 参数传递
- BuildFailed 错误构造测试（含退出码 + stderr）
- 实际调用 buildctl 的测试标记 `#[ignore]`

---

### T4.9 arcpack build 命令集成

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§6.1, BK§3.5 |
| **railpack 参考** | `rp:buildkit/build.go`, `rp:cli/build.go` |
| **依赖** | T4.3, T4.6, T4.7, T4.8 |

**描述：** 替换 Phase 3 的 build 占位实现，完成端到端构建命令。

**交付文件：**

- `src/cli/build.rs`（更新 Phase 3 占位）— 补充 Phase 4 特有的 build flags

  **Build 专属 flags**（对齐 railpack `BuildCommand`，Phase 3 定义的公共 flags 之外）：

  | Flag | 类型 | 默认值 | 说明 | railpack 对应 |
  |------|------|--------|------|--------------|
  | `--name` | `Option<String>` | — | 镜像名称 | `--name` |
  | `--output` | `Option<String>` | — | 输出到本地目录 | `--output` |
  | `--platform` | `Option<String>` | — | 目标平台 | `--platform` |
  | `--progress` | `String` | `auto` | 进度模式：auto/plain/tty | `--progress` |
  | `--show-plan` | `bool` | `false` | 构建前展示 plan JSON | `--show-plan` |
  | `--cache-key` | `Option<String>` | — | 缓存键前缀 | `--cache-key` |

  `run_build(args) -> Result<()>` 完整流程（对齐 railpack `build.go` + `cli/build.go`）：

  ```
  1. generate_build_result_for_command(args)  → BuildResult
  2. pretty_print_build_result(&result)       → 输出到 stderr
  3. if !result.success → exit(1)
  4. if --show-plan → 输出 plan JSON 到 stdout
  5. validate_secrets(plan, env)              → 检查 plan.secrets 是否都有对应 env
  6. secrets_hash = sha256(所有 secret 值拼接)
  7. convert_plan_to_dockerfile(plan, opts)   → (dockerfile, image_config)
  8. daemon = select_daemon_manager()          → Subprocess 或 External
  9. daemon.start() + daemon.wait_ready()
  10. client.build(request)                    → BuildOutput
  11. daemon.stop()
  12. 输出构建结果
  ```

  **Secret 验证**（对齐 railpack `validateSecrets()`）：
  - 遍历 `plan.secrets`，确保每个 secret 在 `env.variables` 中存在
  - 不存在则报错：`"missing environment variable: {secret}. Please set the envvar with --env {secret}=..."`

  **Secret hash**（对齐 railpack `getSecretsHash()`）：
  - 将所有 env 变量值拼接 → SHA256 hash
  - 传递给 `convert_plan_to_dockerfile` 用于缓存失效

- `src/buildkit/build.rs` — 构建主流程编排（DaemonManager + Client 协调）
- `src/buildkit/mod.rs` — re-export
- `tests/integration_buildkit.rs` — `#[ignore]` 端到端测试

**测试要求：**
- （非 ignore）run_build 参数解析测试（所有 flags）
- （非 ignore）secret 验证：缺少 secret 时返回友好错误
- （非 ignore）`convert_plan_to_dockerfile` 单元测试（mock 无需 BuildKit）
- （非 ignore）构建流程编排测试（mock DaemonManager + Client）
- （ignore）完整 build 流程集成测试：源码 → OCI 镜像 → docker run 验证

---

## 与 railpack 的已知差异

| 方面 | railpack | arcpack Phase A | 原因 |
|------|----------|-----------------|------|
| LLB 生成 | Go `llb.State` 直接构建 LLB DAG | Dockerfile 文本生成 | Rust 无 LLB SDK，Phase B 迁移 |
| BuildKit 通信 | Go gRPC client 直连 | `buildctl` CLI 子进程 | Phase A 快速跑通，Phase B 迁移 tonic |
| 步骤并行 | LLB DAG 天然并行 | Dockerfile multi-stage 有限并行 | BuildKit 对 multi-stage 有一定并行优化 |
| 流式进度 | gRPC Status 流 + 自定义渲染 | buildctl `--progress` 模式 | buildctl 已提供 auto/plain/tty 模式 |
| daemon 管理 | 外部 daemon（`BUILDKIT_HOST`） | 外部 + 子进程双模式 | arcpack 增强：支持 ArcBox Spot 实例场景 |
| frontend 命令 | `frontend` 子命令（gRPC gateway） | Phase A 不含，Phase B 实现 | 依赖 gRPC SDK |
| docker load | 通过 pipe 直接 `docker load` | buildctl `--output type=docker` | buildctl 内置 docker output 支持 |
| GHA cache | `--import-cache type=gha` | `--import-cache` / `--export-cache` 透传 | 语义一致，参数格式一致 |

---

## Phase 4 Gate

**执行命令：**
```bash
cargo check
cargo test
cargo test -- --ignored        # 需要 buildkitd + buildctl 环境
cargo build --release
./target/release/arcpack build tests/fixtures/node-npm --name test-node-app
./target/release/arcpack build tests/fixtures/node-npm --name test-node-app --progress plain --show-plan
```

**验收清单：**
- [x] `cargo check` 无错误无警告
- [x] `cargo test` 全部通过（预计 160+ 个测试用例）
- [x] Graph 拓扑排序（DFS-based）对线性/菱形/空图正确，环检测有效
- [x] Graph 传递归约移除冗余边
- [x] StepNode 双环境（InputEnv/OutputEnv）正确传播
- [x] Layer 合并策略决策正确（should_merge + has_significant_overlap）
- [x] BuildGraph `to_dockerfile()` 快照测试通过
- [x] Dockerfile 包含 `# syntax=docker/dockerfile:1` 头部
- [x] Cache mount 语法正确（`RUN --mount=type=cache,target=...,id=...`）
- [x] Secret mount 语法正确（`RUN --mount=type=secret,id=...`）
- [x] Image ENV 从 deploy + graph env + system defaults 正确合并
- [x] 平台默认值匹配宿主架构（非硬编码 amd64）
- [x] `convert_plan_to_dockerfile()` 编排正确
- [x] DaemonManager trait 可 mock，两种实现（Subprocess + External）
- [x] `BUILDKIT_HOST` 环境变量正确选择 External 模式
- [x] BuildKitClient 组装的 buildctl 命令行参数正确（含 secrets）
- [x] Secret 验证：缺少 secret 时友好报错
- [x] `--show-plan` / `--progress` / `--cache-key` flags 正确
- [x] `arcpack build` 命令 help 包含所有参数
- [ ] 集成测试（`#[ignore]`）在有 buildkitd + buildctl 环境中通过
- [ ] 构建产出 OCI 镜像，`docker run` 可正常执行
