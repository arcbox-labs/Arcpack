# Phase 2: Provider 框架 + Node.js Provider

> [← 返回目录](./README.md) | 上一阶段：[← Phase 1](./phase-1-data-structures.md) | 下一阶段：[Phase 3 →](./phase-3-cli.md)

**目标：** 实现 Provider 框架和 StepBuilder 体系，以 Node.js 为首个 Provider 完成端到端的 BuildPlan 生成。

**前置条件：** Phase 1 全部完成

## 任务依赖图

```
T2.1 (StepBuilder trait)
 ├──► T2.2 (CommandStepBuilder)
 ├──► T2.3 (MiseStepBuilder + ImageStepBuilder)
 └──► T2.4 (DeployBuilder + CacheContext)
       │
       ▼
T2.5 (mise/ + resolver/) ◄── T2.3
       │
       ▼
T2.6 (GenerateContext)
       │
       ▼
T2.7 (Provider trait + registry)
       │
       ▼
T2.8 (Node.js Provider)
       │
       ▼
T2.9 (generate_build_plan() + snapshot tests)
```

## 任务列表

### T2.1 StepBuilder trait 定义

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.3 |
| **railpack 参考** | `rp:core/generate/step_builder.go`（StepBuilder interface） |
| **依赖** | Phase 1 |

**描述：** 定义 StepBuilder trait 和 BuildStepOptions。

**交付文件：**
- `src/generate/mod.rs`（初始版本）— StepBuilder trait（`name() -> &str` + `build(&self, &mut BuildPlan, &BuildStepOptions) -> Result<()>`）+ BuildStepOptions 结构体

**测试要求：** 创建 MockStepBuilder 实现 trait，验证 build() 正确写入 BuildPlan。

---

### T2.2 CommandStepBuilder 实现

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.3 |
| **railpack 参考** | `rp:core/generate/command_step_builder.go` |
| **依赖** | T2.1 |

**描述：** 最常用的 StepBuilder 实现，支持链式调用。

**交付文件：**
- `src/generate/command_step_builder.rs` — 持有 name/commands/inputs/variables/caches/secrets/assets/deploy_outputs。链式方法：`add_cmd()` / `add_input()` / `add_variable()` / `add_cache()` / `add_secret()` / `add_asset()` / `add_deploy_output()`。实现 StepBuilder::build()。

**测试要求：**
- 构造 CommandStepBuilder 添加多种命令/输入后调用 build()，验证产出的 Step 字段正确
- 链式调用 API 可用性测试

---

### T2.3 MiseStepBuilder + ImageStepBuilder 实现

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.3 |
| **railpack 参考** | `rp:core/generate/mise_step_builder.go`, `rp:core/generate/image_step_builder.go` |
| **依赖** | T2.1 |

**描述：** 语言运行时安装和镜像提取的 StepBuilder。

**交付文件：**
- `src/generate/mise_step_builder.rs` — 全局唯一，持有 packages:HashMap。`add_package(name, version)` 方法。build() 产出 Step name = "packages"
- `src/generate/image_step_builder.rs` — 持有 name + image。build() 产出从 Docker 镜像提取文件的 Step

**测试要求：**
- MiseStepBuilder 添加多个包后 build() 产出 Step 验证（步骤名 "packages"，commands 包含 mise install）
- ImageStepBuilder build() 测试

---

### T2.4 DeployBuilder + CacheContext + InstallBinBuilder

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.2, Arch§3.3 |
| **railpack 参考** | `rp:core/generate/deploy_builder.go`, `rp:core/generate/cache_context.go`, `rp:core/generate/install_bin_builder.go` |
| **依赖** | T2.1 |

**描述：** 部署配置构建器、缓存注册和二进制安装工具。

**交付文件：**
- `src/generate/deploy_builder.rs` — 持有 start_cmd/variables/paths/inputs/apt_packages。`set_start_cmd()` / `add_path()` / `add_variable()` / `add_input()` 等方法 + `build() -> Deploy`
- `src/generate/cache_context.rs` — 持有 caches:HashMap<String,Cache>。`register_cache(key, dir, type)` 方法
- `src/generate/install_bin_builder.rs` — 用于安装独立二进制工具

**测试要求：**
- DeployBuilder 设置各字段后 build() 产出 Deploy 验证
- CacheContext 注册/去重测试
- InstallBinBuilder build() 测试

---

### T2.5 mise/ 封装 + resolver/ 版本解析

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§2.2 (resolver/mise) |
| **railpack 参考** | `rp:core/mise/mise.go`, `rp:core/mise/install.go`, `rp:core/resolver/resolver.go`, `rp:core/resolver/version.go` |
| **依赖** | T2.3 |

**描述：** mise CLI 封装和版本解析。不要求实际调用 mise 二进制，仅测试脚本生成和数据流。

**交付文件：**
- `src/mise/mod.rs` — Mise 结构体：封装 mise CLI 路径检测、mise.toml 生成
- `src/mise/install.rs` — 生成 mise install 脚本的命令序列
- `src/resolver/mod.rs` — Resolver 结构体：`resolve(package, version_constraint) -> Result<String>`（离线时直接返回原始版本字符串）

**测试要求：**
- Mise install 脚本生成内容正确性测试
- Resolver 版本解析逻辑测试（精确版本直接返回、范围版本标记待解析）

---

### T2.6 GenerateContext 核心编排

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.2 |
| **railpack 参考** | `rp:core/generate/context.go` |
| **依赖** | T2.2, T2.3, T2.4, T2.5 |

**描述：** Provider 执行时的运行时上下文，核心编排器。

**交付文件：**
- `src/generate/mod.rs`（完整版）— GenerateContext 结构体（持有 app/env/config/base_image/steps/deploy/caches/secrets/sub_contexts/metadata/resolver/mise_step_builder）

**关键方法：**
- `new(app, env, config)` / `app()` / `env()` / `config()`
- `new_command_step(name) -> &mut CommandStepBuilder`
- `get_mise_step_builder() -> &mut MiseStepBuilder`（懒初始化）
- `new_local_layer()` / `new_local_layer_with_filter(filter)`
- `enter_sub_context(name)` / `exit_sub_context()`
- `generate() -> Result<BuildPlan>`：应用 Config 覆盖 → 解析包版本 → 各 StepBuilder.build() → 组装 Deploy → 返回 BuildPlan

**测试要求：**
- 构造 GenerateContext 后通过 new_command_step 添加步骤，调用 generate() 验证产出 BuildPlan 的完整性
- Config 覆盖生效测试
- 子上下文命名空间前缀测试

---

### T2.7 Provider trait + 注册表 + 检测编排

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.1, Arch§8.3 |
| **railpack 参考** | `rp:core/providers/provider.go` |
| **依赖** | T2.6 |

**描述：** 定义 Provider trait 和检测流程。

**交付文件：**
- `src/provider/mod.rs` — Provider trait（name / detect / initialize / plan / cleanse_plan / start_command_help），其中 initialize / cleanse_plan / start_command_help 提供默认空实现。`get_all_providers() -> Vec<Box<dyn Provider>>`（按 Arch 附录 A 优先级排序）+ `get_matching_provider(providers, ctx) -> Result<&dyn Provider>`（首个 detect 返回 true 胜出；支持 Config 中 provider 字段强制指定；全未匹配返回 NoProviderMatched）

**测试要求：**
- 用 MockProvider 测试单匹配/无匹配/Config 强制指定三种场景
- Provider trait 默认方法不 panic 测试

---

### T2.8 Node.js Provider 完整实现

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.1, Arch 附录 A, Arch 附录 B |
| **railpack 参考** | `rp:core/providers/node/node.go`, `rp:core/providers/node/package_manager.go` |
| **依赖** | T2.7 |

**描述：** 首个完整 Provider，覆盖 Node.js 四种包管理器。

**交付文件：**
- `src/provider/node/mod.rs` — NodeProvider：detect 检测 package.json；initialize 解析 package.json（scripts/dependencies/engines）；plan 创建 packages/install/build 步骤 + deploy 配置；cleanse_plan 清理
- `src/provider/node/detect.rs` — 包管理器检测（npm/yarn/pnpm/bun 通过 lockfile 判断）
- `src/provider/node/npm.rs` — npm install 命令、缓存目录（`/root/.npm`）
- `src/provider/node/yarn.rs` — yarn install 命令、缓存目录（`/root/.cache/yarn`）
- `src/provider/node/pnpm.rs` — pnpm install 命令、缓存目录（`/root/.local/share/pnpm/store`）
- `src/provider/node/bun.rs` — bun install 命令

**测试要求：**
- 使用 fixture 目录验证 detect 正确识别
- 各包管理器的 install 命令和缓存路径测试
- plan() 后 GenerateContext 中步骤数量和内容测试

---

### T2.9 generate_build_plan() 编排 + 快照测试

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§6.1, Arch§8.7 |
| **railpack 参考** | `rp:core/core.go`（GenerateBuildPlan 函数）, `rp:core/core_test.go` |
| **依赖** | T2.8 |

**描述：** 顶层编排函数和快照测试。

**交付文件：**
- `src/lib.rs` 中的 `generate_build_plan(source, env_vars, config_override) -> Result<BuildResult>` — 串联：App::new → Environment::new → Config::load → GenerateContext::new → get_matching_provider → provider.initialize → provider.plan → ctx.generate → provider.cleanse_plan → 返回 BuildResult（含 BuildPlan + Metadata）
- `tests/fixtures/node-npm/package.json`
- `tests/fixtures/node-yarn/package.json` + `yarn.lock`
- `tests/fixtures/node-pnpm/package.json` + `pnpm-lock.yaml`
- `tests/fixtures/node-bun/package.json` + `bun.lockb`
- `tests/snapshot_tests.rs` — 对每个 fixture 调用 `generate_build_plan()` 并使用 `insta` crate 做 JSON 快照断言

**测试要求：**
- 4 个 Node.js fixture 的快照测试全部通过
- BuildPlan 结构与 railpack 语义对齐（步骤名、依赖关系、缓存键名对齐，参照 Arch§8.7）

---

## Phase 2 Gate

**执行命令：**
```bash
cargo check
cargo test
cargo test -- snapshot         # 快照测试单独确认
cargo insta review             # 审查快照（首次运行需 accept）
```

**验收清单：**
- [ ] `cargo check` 无错误无警告
- [ ] `cargo test` 全部通过（预计 80+ 个测试用例）
- [ ] `generate_build_plan("tests/fixtures/node-npm")` 返回包含 packages/install/build 三步骤的 BuildPlan
- [ ] Node.js Provider 正确检测 npm/yarn/pnpm/bun 四种包管理器
- [ ] MiseStepBuilder 产出 "packages" 步骤，包含 node 运行时包
- [ ] CommandStepBuilder 链式 API 可流畅使用
- [ ] GenerateContext.generate() 正确应用 Config 覆盖
- [ ] 4 个 Node.js fixture 的 `insta` 快照测试全部通过
- [ ] BuildPlan 的 Step DAG 依赖关系正确（packages <- install <- build）
- [ ] 缓存键名与 railpack 对齐（npm-cache / yarn-cache / pnpm-store）
