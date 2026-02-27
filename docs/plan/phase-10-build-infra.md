# Phase 10: 构建基础设施完善

> [← 返回目录](./README.md) | 上一阶段：[← Phase 9](./phase-9-php-dotnet.md)

**目标：** 补齐构建基础设施功能：docker load 管道、--dump-llb 调试命令、GITHUB_TOKEN 自动传递、环境变量前缀统一。

**前置条件：** Phase 4（BuildKit Phase A）

## 任务依赖图

```
（以下任务可并行开发）

T10.1 (docker load 管道)      ──┐
T10.2 (--dump-llb 调试)        ──┤
T10.3 (GITHUB_TOKEN 传递)      ──┤──► 全部完成后验证
T10.4 (环境变量前缀对齐)       ──┘
```

> **注意：** T10.2 需要 Phase B-2（LLB 转换）完成后才可实际验证；T10.4 影响所有 Provider，需在全部 Provider 实现后执行。

## 任务列表

### T10.1 docker load 管道

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **railpack 参考** | `rp:buildkit/build.go`（docker load pipe 部分） |
| **依赖** | Phase 4 |

**描述：** 构建完成后自动将镜像加载到本地 Docker daemon，实现 `arcpack build → docker run` 的无缝工作流。

**修改文件：** `src/buildkit/client.rs`（或 `src/buildkit/grpc_client.rs`，取决于当前使用的客户端）

**当前行为：**
- BuildKit 构建完成后镜像留在 BuildKit 缓存中
- 用户需要手动 `docker load` 或指定 output

**目标行为：**

1. **默认模式（无 `--output-dir`）：docker load 管道**
   ```
   BuildKit (type=docker,name=tag) ──pipe──► docker load
   ```
   - 创建 `tokio::io::DuplexStream`（或 `os_pipe::pipe()`）
   - BuildKit exporter 设置 `type=docker`，output 写入 pipe writer 端
   - 另起 `tokio::spawn` 任务运行 `docker load`，stdin 从 pipe reader 端读取
   - 两个任务并行执行（流式传输，不需要完整 tar 落盘）
   - `docker load` 完成后输出镜像 tag

2. **有 `--output-dir` 时：本地目录导出**
   - BuildKit exporter 设置 `type=local`，output 到指定目录
   - 跳过 docker load
   - 输出导出路径

**实现细节：**

```rust
/// BuildKit 导出 + docker load 管道
async fn export_and_load(
    &self,
    image_name: &str,
    solve_response: SolveResponse,
) -> Result<()> {
    // 方式一：使用 buildctl（Phase A）
    // buildctl build ... --output type=docker,name={image_name} | docker load

    // 方式二：使用 gRPC（Phase B）
    // SolveConfig.exports = [ExportConfig::DockerTar { name: image_name }]
    // 通过 session 的 content store 获取 tar 流
    // pipe 到 docker load stdin
}
```

**CLI 参数扩展：**
- `src/cli/build.rs` 新增 `--output-dir <PATH>` 参数
- `--output-dir` 与默认 docker load 模式互斥

**错误处理：**
- `docker load` 失败（如 Docker daemon 未运行）→ 友好错误提示
- 管道断裂（writer 先关闭）→ 超时处理
- BuildKit 导出失败 → 保留错误信息

**测试要求：**
- docker load 管道创建和数据传输测试（mock）
- `--output-dir` 模式导出路径正确性测试
- 互斥参数校验测试
- docker daemon 不可用时的错误提示测试

**集成测试：**
- 构建一个简单 fixture → 验证 `docker images` 中出现对应 tag
- 构建同一 fixture 到 `--output-dir` → 验证目录中有文件

---

### T10.2 --dump-llb 调试命令

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P2 |
| **railpack 参考** | `rp:cli/build.go`（`--dump-llb` hidden flag） |
| **依赖** | Phase B-2（LLB 转换完成） |

**描述：** 隐藏的 CLI flag，将 LLB Definition 原始二进制输出到 stdout，不执行构建。用于调试 LLB 生成是否正确。

**修改文件：** `src/cli/build.rs`

**CLI 参数：**
```rust
#[clap(long, hide = true, help = "Dump raw LLB definition to stdout")]
dump_llb: bool,
```

**执行流程：**
```
arcpack build --dump-llb <dir>
  ├── generate_build_plan(dir)
  ├── BuildGraph::from_plan(plan)
  ├── build_graph.to_llb_definition()      → pb::Definition
  ├── definition.encode_to_vec()           → Vec<u8> (protobuf bytes)
  ├── stdout.write_all(&bytes)             → 写入 stdout
  └── return (不执行 BuildKit solve)
```

**用法示例：**
```bash
# 导出 LLB 并用 buildctl 调试查看
arcpack build --dump-llb ./myapp | buildctl debug dump-llb

# 保存到文件
arcpack build --dump-llb ./myapp > plan.llb

# 直接用 buildctl 构建（跳过 arcpack 的 BuildKit 客户端）
arcpack build --dump-llb ./myapp | buildctl build --no-cache
```

**互斥规则：**
- `--dump-llb` 与 `--output-dir` 互斥
- `--dump-llb` 时不启动 buildkitd、不连接 gRPC

**测试要求：**
- `--dump-llb` flag 解析测试
- 输出为有效 protobuf 的测试（反序列化验证）
- 互斥参数校验测试
- 不启动 buildkitd 的验证（mock 检查）

---

### T10.3 GITHUB_TOKEN 传递

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **railpack 参考** | `rp:buildkit/build_llb/build_graph.go`（`addGitHubTokenToMiseInstall` 函数） |
| **依赖** | Phase 4 |

**描述：** 将 `GITHUB_TOKEN` 环境变量自动注入 mise install 步骤，避免 GitHub API 限速导致 mise 安装工具失败。

**修改文件：** `src/buildkit/build_llb/mod.rs`（或 `src/buildkit/convert.rs`）

**注入条件（所有条件必须同时满足）：**
1. 当前环境中 `GITHUB_TOKEN` 环境变量非空
2. `GITHUB_TOKEN` 不在 `plan.secrets` 列表中（避免重复注入导致 secret 泄露）
3. 目标步骤的命令匹配 `mise install`（正则：`\bmise\s+install\b`）

**注入方式：**
- 作为 BuildKit secret mount 注入（`--mount=type=secret,id=GITHUB_TOKEN`）
- 在 mise install 命令前添加 `export GITHUB_TOKEN=$(cat /run/secrets/GITHUB_TOKEN)`
- 这样 token 不会出现在 Docker layer 中

**不注入的场景：**
- 非 `mise install` 步骤：绝不注入（安全边界）
- `GITHUB_TOKEN` 为空：跳过
- 已在 `plan.secrets` 中：由用户显式控制，不重复处理

**实现示例：**

```rust
fn inject_github_token(
    step: &Step,
    secrets: &[String],
    env: &Environment,
) -> Option<SecretMount> {
    // 检查是否为 mise install 步骤
    let is_mise_install = step.commands.iter().any(|cmd| {
        matches!(cmd, Command::Exec(exec) if exec.cmd.contains("mise install"))
    });

    if !is_mise_install {
        return None;
    }

    // 检查 GITHUB_TOKEN 是否存在且不在 secrets 列表中
    let token = env.get("GITHUB_TOKEN")?;
    if token.is_empty() || secrets.contains(&"GITHUB_TOKEN".to_string()) {
        return None;
    }

    Some(SecretMount {
        id: "GITHUB_TOKEN".to_string(),
        target: "/run/secrets/GITHUB_TOKEN".to_string(),
    })
}
```

**Dockerfile 模式支持（Phase A）：**
```dockerfile
RUN --mount=type=secret,id=GITHUB_TOKEN \
    export GITHUB_TOKEN=$(cat /run/secrets/GITHUB_TOKEN 2>/dev/null || true) && \
    mise install
```

**LLB 模式支持（Phase B）：**
- `ExecOp` 的 `secretenv` 字段直接注入

**测试要求：**
- GITHUB_TOKEN 存在时注入到 mise install 步骤测试
- GITHUB_TOKEN 为空时不注入测试
- GITHUB_TOKEN 已在 secrets 列表中时不注入测试
- 非 mise install 步骤不注入测试
- Dockerfile 模式输出包含 `--mount=type=secret` 的测试
- 快照测试（含/不含 GITHUB_TOKEN 两种场景）

---

### T10.4 环境变量前缀对齐

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P0 |
| **依赖** | 所有 Provider 实现完成 |

**描述：** 将 railpack 的 `RAILPACK_*` 前缀统一替换为 `ARCPACK_*`，并在 `Environment` 中新增配置变量获取便捷方法。

**影响范围：** 所有 Provider 中的 `RAILPACK_*` 引用（Phase 5 设计文档中使用了 `RAILPACK_*` 前缀）。

**修改文件：** `src/app/environment.rs` + 所有 Provider 文件

**Environment 新增方法：**

```rust
impl Environment {
    /// 获取 ARCPACK_{suffix} 环境变量值
    /// 例如：get_config_variable("PRUNE_DEPS") → 查找 ARCPACK_PRUNE_DEPS
    pub fn get_config_variable(&self, suffix: &str) -> Option<&str> {
        self.get(&format!("ARCPACK_{}", suffix))
    }

    /// 检查 ARCPACK_{suffix} 是否为 truthy 值（true/1/yes）
    pub fn is_config_variable_truthy(&self, suffix: &str) -> bool {
        self.get_config_variable(suffix)
            .map(|v| matches!(v.to_lowercase().as_str(), "true" | "1" | "yes"))
            .unwrap_or(false)
    }
}
```

**关键变量清单（RAILPACK_* → ARCPACK_*）：**

| 旧名称（Phase 5 文档中） | 新名称 | 使用位置 |
|-------------------------|--------|---------|
| `RAILPACK_GO_VERSION` | `ARCPACK_GO_VERSION` | Go Provider |
| `RAILPACK_GO_BIN` | `ARCPACK_GO_BIN` | Go Provider |
| `RAILPACK_GO_WORKSPACE_MODULE` | `ARCPACK_GO_WORKSPACE_MODULE` | Go Provider |
| `RAILPACK_PYTHON_VERSION` | `ARCPACK_PYTHON_VERSION` | Python Provider |
| `RAILPACK_RUST_VERSION` | `ARCPACK_RUST_VERSION` | Rust Provider |
| `RAILPACK_JAVA_VERSION` | `ARCPACK_JAVA_VERSION` | Java Provider |
| `RAILPACK_STATIC_FILE_ROOT` | `ARCPACK_STATIC_FILE_ROOT` | StaticFile Provider |
| `RAILPACK_SHELL_SCRIPT` | `ARCPACK_SHELL_SCRIPT` | Shell Provider |
| `RAILPACK_INSTALL_CMD` | `ARCPACK_INSTALL_CMD` | 通用 |
| `RAILPACK_BUILD_CMD` | `ARCPACK_BUILD_CMD` | 通用 |
| `RAILPACK_START_CMD` | `ARCPACK_START_CMD` | 通用 |
| `RAILPACK_PACKAGES` | `ARCPACK_PACKAGES` | 通用 |
| `RAILPACK_DISABLE_CACHES` | `ARCPACK_DISABLE_CACHES` | 通用 |
| — | `ARCPACK_PRUNE_DEPS` | Node.js Provider (Phase 6) |
| — | `ARCPACK_NO_SPA` | Node.js Provider (Phase 6) |
| — | `ARCPACK_SPA_OUTPUT_DIR` | Node.js Provider (Phase 6) |
| — | `ARCPACK_NODE_PRUNE_CMD` | Node.js Provider (Phase 6) |
| — | `ARCPACK_NODE_INSTALL_PATTERNS` | Node.js Provider (Phase 6) |
| — | `ARCPACK_DENO_VERSION` | Deno Provider (Phase 7) |
| — | `ARCPACK_GLEAM_VERSION` | Gleam Provider (Phase 7) |
| — | `ARCPACK_ERLANG_VERSION` | Gleam/Elixir Provider |
| — | `ARCPACK_CMAKE_VERSION` | C++ Provider (Phase 7) |
| — | `ARCPACK_MESON_VERSION` | C++ Provider (Phase 7) |
| — | `ARCPACK_CPP_BINARY_NAME` | C++ Provider (Phase 7) |
| — | `ARCPACK_CPP_BUILD_DIR` | C++ Provider (Phase 7) |
| — | `ARCPACK_PHP_VERSION` | PHP Provider (Phase 9) |
| — | `ARCPACK_PHP_ROOT_DIR` | PHP Provider (Phase 9) |
| — | `ARCPACK_SKIP_MIGRATIONS` | PHP Provider (Phase 9) |
| — | `ARCPACK_DOTNET_VERSION` | .NET Provider (Phase 9) |

**Provider 代码替换规则：**

所有 Provider 中的 `env.get("RAILPACK_*")` 调用替换为 `env.get_config_variable("*")`：

```rust
// Before (Phase 5 style)
let version = env.get("RAILPACK_GO_VERSION");

// After
let version = env.get_config_variable("GO_VERSION");
```

**测试要求：**
- `get_config_variable()` 方法正确拼接前缀测试
- `is_config_variable_truthy()` 对 true/1/yes/TRUE/Yes 返回 true 测试
- `is_config_variable_truthy()` 对 false/0/no/空 返回 false 测试
- 所有 Provider 中无残留 `RAILPACK_*` 引用（可通过 grep 验证）
- 现有测试全部通过（替换后行为不变）

---

## 验证清单

Phase 10 完成后：

```bash
cargo check                                             # 编译无错误
cargo test                                              # 全部单元测试通过

# 验证无残留 RAILPACK_ 引用
grep -r "RAILPACK_" src/ --include="*.rs" && echo "FAIL: RAILPACK_ still exists" || echo "OK"

# 集成测试
cargo test --test integration_tests -- --ignored

# docker load 验证
arcpack build tests/fixtures/node-npm/
docker images | grep arcpack-test-node-npm

# --dump-llb 验证（需 Phase B-2 完成）
arcpack build --dump-llb tests/fixtures/node-npm/ | buildctl debug dump-llb
```

---

## 全局集成测试策略总结

从 Phase 6 开始，每个 Phase 都包含集成测试：

| Phase | 新增集成测试 fixture | 测试类型 |
|-------|---------------------|---------|
| Phase 6 | node-vite-spa, node-cra, node-next, node-monorepo + Phase 5 全部 fixture | expectedOutput + httpCheck |
| Phase 7 | deno-basic, gleam-basic, cpp-cmake, cpp-meson | expectedOutput |
| Phase 8 | ruby-basic, ruby-rails, elixir-basic, elixir-phoenix | expectedOutput + httpCheck + docker-compose |
| Phase 9 | php-basic, php-laravel, dotnet-basic | httpCheck + docker-compose |
| Phase 10 | docker-load-test, dump-llb-test | 功能验证 |

**运行方式：**
```bash
# 全部集成测试
cargo test --test integration_tests -- --ignored

# 按语言过滤
cargo test --test integration_tests -- --ignored node
cargo test --test integration_tests -- --ignored ruby

# CI 带缓存
cargo test --test integration_tests -- --ignored -- \
  --cache-import "type=gha,url=..." --cache-export "type=gha,url=..."
```

---

## 完成后的 Provider 覆盖对照

| # | Provider | railpack | arcpack Phase 5 | arcpack Phase 6-9 |
|---|----------|---------|-----------------|-------------------|
| 1 | PHP | Yes | — | **Phase 9** |
| 2 | Go | Yes | **Phase 5** | — |
| 3 | Java | Yes | **Phase 5** | — |
| 4 | Rust | Yes | **Phase 5** | — |
| 5 | Ruby | Yes | — | **Phase 8** |
| 6 | Elixir | Yes | — | **Phase 8** |
| 7 | Python | Yes | **Phase 5** | — |
| 8 | Deno | Yes | — | **Phase 7** |
| 9 | .NET | Yes | — | **Phase 9** |
| 10 | Node.js | Yes (深度) | **Phase 2** (基础) | **Phase 6** (深度) |
| 11 | Gleam | Yes | — | **Phase 7** |
| 12 | C++ | Yes | — | **Phase 7** |
| 13 | StaticFile | Yes | **Phase 5** | — |
| 14 | Shell | Yes | **Phase 5** | — |
| — | Procfile | Yes | **Phase 5** | — |

**Phase 6-10 全部完成后：14/14 Provider + Procfile + 集成测试 + 构建基础设施 = 对齐 railpack。**
