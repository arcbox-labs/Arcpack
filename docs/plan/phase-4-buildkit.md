# Phase 4: BuildKit 集成

> [← 返回目录](./README.md) | 上一阶段：[← Phase 3](./phase-3-cli.md) | 下一阶段：[Phase 5 →](./phase-5-providers.md)

**目标：** 完成从 BuildPlan 到 OCI 镜像的完整构建流程（Phase A：通过 Dockerfile + buildctl CLI）。

**前置条件：** Phase 3 全部完成

## 任务依赖图

```
T4.1 (graph/ 通用 DAG)
 └──► T4.2 (StepNode + BuildEnv)
       └──► T4.3 (BuildGraph + Dockerfile 生成)
             ├──► T4.4 (layers + cache_store)
             └──► T4.5 (image + platform)
                   │
                   ▼
             T4.6 (DaemonManager)
              └──► T4.7 (BuildKit client: buildctl)
                    └──► T4.8 (build 命令集成)
```

## 任务列表

### T4.1 graph/ 通用 DAG + 拓扑排序

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | `rp:buildkit/graph/graph.go` |
| **依赖** | Phase 3 |

**描述：** 通用有向无环图数据结构。

**交付文件：**
- `src/graph/mod.rs` — 泛型 `Graph<T>`（邻接表），方法：`add_node(id, data)` / `add_edge(from, to)` / `topological_sort() -> Result<Vec<&T>>` / `has_cycle() -> bool`。拓扑排序使用 Kahn 算法。

**测试要求：**
- 线性依赖链排序正确性
- 菱形依赖（A→B, A→C, B→D, C→D）排序测试
- 环检测测试（A→B→C→A 应返回错误）
- 空图和单节点测试

---

### T4.2 StepNode + BuildEnvironment

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | `rp:buildkit/build_llb/step_node.go`, `rp:buildkit/build_llb/build_env.go` |
| **依赖** | T4.1 |

**描述：** 构建图中的节点表示和环境变量管理。

**交付文件：**
- `src/buildkit/build_llb/step_node.rs` — StepNode：封装 Step 在构建图中的节点表示
- `src/buildkit/build_llb/build_env.rs` — BuildEnvironment：管理累积 PATH 和环境变量

**测试要求：**
- StepNode 从 Step 构造正确性测试
- BuildEnvironment PATH 累积和去重测试
- 环境变量覆盖测试

---

### T4.3 BuildGraph + Dockerfile 生成

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.10, Arch§8.5 |
| **railpack 参考** | `rp:buildkit/build_llb/build_graph.go` |
| **依赖** | T4.2 |

**描述：** BuildPlan → Dockerfile 转换（Phase A 核心）。

**交付文件：**
- `src/buildkit/build_llb/mod.rs` — BuildGraph（持有 Graph<StepNode> + BuildPlan 引用），`new(plan) -> Result<BuildGraph>`（从 Step.inputs[].step 建立 DAG）+ `to_dockerfile() -> Result<String>`（拓扑排序后线性化为 Dockerfile）

**Dockerfile 生成规则：**
- 每个 Step → `FROM ... AS {step_name}` 阶段
- Command::Exec → `RUN`
- Command::Copy → `COPY --from=...`
- Command::Path → `ENV PATH=...`
- Command::File → 内联 heredoc 或 COPY from asset
- 最终 deploy 阶段汇聚所有 deploy inputs

**测试要求：**
- 构造含 packages/install/build 三步骤的 BuildPlan，to_dockerfile() 快照测试
- 验证 FROM/RUN/COPY/ENV 指令的正确性和顺序
- deploy 阶段包含正确的 COPY --from 和 CMD

---

### T4.4 layers + cache_store

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.10, Arch 附录 B |
| **railpack 参考** | `rp:buildkit/build_llb/layers.go`, `rp:buildkit/build_llb/cache_store.go` |
| **依赖** | T4.3 |

**描述：** Layer 合并策略和缓存映射。

**交付文件：**
- `src/buildkit/build_llb/layers.rs` — Layer 输入 → Dockerfile COPY 指令转换（step 引用 → `COPY --from={step_name}`，image 引用 → `COPY --from={image}`，local 引用 → `COPY . .`），处理 Filter 的 include/exclude
- `src/buildkit/build_llb/cache_store.rs` — BuildPlan.caches → `RUN --mount=type=cache,target=...` 语法（BuildKit 扩展语法需 `# syntax=docker/dockerfile:1` 头部）

**测试要求：**
- 各种 Layer 类型转 COPY 指令的单元测试
- Filter include/exclude 转路径列表测试
- cache mount 语法生成测试

---

### T4.5 image config + platform 解析

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | `rp:buildkit/image.go`, `rp:buildkit/platform.go` |
| **依赖** | T4.3 |

**描述：** OCI Image config 和平台字符串解析。

**交付文件：**
- `src/buildkit/image.rs` — 从 Deploy 提取 CMD/ENV/PATH/EXPOSE/WORKDIR → Dockerfile 末尾指令
- `src/buildkit/platform.rs` — 平台字符串解析（"linux/amd64" → os + arch），默认 "linux/amd64"

**测试要求：**
- Deploy → Dockerfile 尾部指令正确性测试
- 平台解析合法/非法输入测试

---

### T4.6 DaemonManager（buildkitd 生命周期管理）

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | BK§3.1, BK§3.2, BK§3.4, Arch§3.10 |
| **railpack 参考** | 无对应文件（arcpack 独有，railpack 通过 Go client 库直连） |
| **依赖** | Phase 3 |

**描述：** buildkitd 子进程生命周期管理，通过 trait 抽象支持 mock 测试。

**交付文件：**
- `src/buildkit/daemon.rs` — `DaemonManager` async trait（start / wait_ready / stop / is_running / socket_path）+ `SubprocessDaemonManager`（tokio::process::Command 启动 buildkitd，Unix socket 探测就绪，SIGTERM 优雅停止 + 超时 SIGKILL）

**测试要求：**
- MockDaemonManager 测试 wait_ready 超时返回 DaemonTimeout 错误
- MockDaemonManager 测试 start → wait_ready → stop 生命周期
- SubprocessDaemonManager 构造参数验证（socket 路径格式）
- 实际启动 buildkitd 的测试标记 `#[ignore]`

---

### T4.7 BuildKit client（buildctl CLI 封装）

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | BK§3.3, BK§3.5 |
| **railpack 参考** | 无对应文件（arcpack 独有） |
| **依赖** | T4.6 |

**描述：** 通过 buildctl CLI 执行构建。

**交付文件：**
- `src/buildkit/client.rs` — BuildKitClient（持有 socket_path + buildctl path），`build(request) -> Result<BuildOutput>` 方法。BuildRequest 结构体（context_dir / dockerfile_content / output_image / push / cache_from / cache_to / platform）+ BuildOutput（image_digest / duration）。Dockerfile 内容写入临时文件。

**测试要求：**
- buildctl 命令行参数组装正确性验证（--addr / --frontend / --local / --output 等）
- BuildFailed 错误构造测试
- 实际调用 buildctl 的测试标记 `#[ignore]`

---

### T4.8 arcpack build 命令集成

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§6.1, BK§3.5 |
| **railpack 参考** | `rp:buildkit/build.go`（BuildWithBuildkitClient 函数）, `rp:cli/build.go` |
| **依赖** | T4.3, T4.4, T4.5, T4.6, T4.7 |

**描述：** 端到端构建命令和编排。

**交付文件：**
- `src/cli/build.rs` — BuildArgs（source / output / push / platform / cache-from / cache-to / build-cmd / start-cmd / env）+ `run_build(args) -> Result<()>`
- `src/buildkit/convert.rs` — `convert_plan_to_build(plan, request) -> Result<BuildOutput>` 编排函数
- `src/buildkit/build.rs` — 构建主流程
- `src/buildkit/mod.rs` — re-export
- `tests/integration_buildkit.rs` — `#[ignore]` 端到端测试

**构建流程：** generate_build_plan() → BuildGraph::new() → to_dockerfile() → DaemonManager::start() + wait_ready() → BuildKitClient::build() → DaemonManager::stop() → 输出结果

**测试要求：**
- （非 ignore）run_build 参数解析测试、convert_plan_to_build 单元测试（mock DaemonManager + Client）
- （ignore）完整 build 流程集成测试

---

## Phase 4 Gate

**执行命令：**
```bash
cargo check
cargo test
cargo test -- --ignored        # 需要 buildkitd + buildctl 环境
./target/release/arcpack build tests/fixtures/node-npm --name test-node-app
```

**验收清单：**
- [ ] `cargo check` 无错误无警告
- [ ] `cargo test` 全部通过（预计 140+ 个测试用例）
- [ ] Graph<T> 拓扑排序对线性/菱形/空图正确，环检测有效
- [ ] BuildGraph::to_dockerfile() 快照测试通过
- [ ] Dockerfile 包含 `# syntax=docker/dockerfile:1` 头部
- [ ] Cache mount 语法正确（`RUN --mount=type=cache,target=...`）
- [ ] DaemonManager trait 可 mock，SubprocessDaemonManager 构造参数有效
- [ ] BuildKitClient 组装的 buildctl 命令行参数正确
- [ ] `arcpack build` 命令 help 包含所有参数
- [ ] 集成测试（`#[ignore]`）在安装了 buildkitd + buildctl 的环境中通过
- [ ] 构建产出 OCI 镜像，`docker run` 可正常执行
