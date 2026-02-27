# Phase 3: CLI + Plan 输出

> [← 返回目录](./README.md) | 上一阶段：[← Phase 2](./phase-2-provider-framework.md) | 下一阶段：[Phase 4 →](./phase-4-buildkit.md)

**目标：** 通过命令行工具输出 BuildPlan、构建信息和 JSON Schema，使 arcpack 可作为独立 CLI 工具使用。

**前置条件：** Phase 2 全部完成

## 任务依赖图

```
T3.1 (clap 定义 + common + BuildResult)
 ├──► T3.2 (plan command)
 ├──► T3.3 (info command + pretty print)
 ├──► T3.4 (schema command)
 └──► T3.5 (prepare command)
       │
       ▼
T3.6 (main.rs 集成 + CLI 测试)
```

## 任务列表

### T3.1 clap 命令行定义 + 公共逻辑 + BuildResult

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§1.4, Arch§6.4 |
| **railpack 参考** | `rp:cli/common.go`, `rp:cmd/cli/main.go`, `rp:core/core.go` |
| **依赖** | Phase 2 |

**描述：** 使用 clap derive 定义 CLI 结构，实现公共 flag 和 BuildResult 生成函数。

**交付文件：**

- `src/cli/mod.rs` — 顶层 `Cli` 结构体 + `Commands` 枚举（Plan/Info/Schema/Build/Prepare）

  全局参数（对齐 railpack `cmd/cli/main.go`）：
  - `-v`/`-vv` 日志级别（`-v` = DEBUG，`-vv` = TRACE；railpack 仅 `--verbose` 单级 Debug，arcpack 扩展为双级）
  - `--version` 版本信息（编译时注入，对齐 railpack goreleaser 机制）

- `src/cli/common.rs` — 公共 flag 定义 + `generate_build_result_for_command()` 函数

  **公共构建 flags**（对齐 railpack `commonPlanFlags()`，plan/info/build/prepare 共用）：

  | Flag | 短名 | 类型 | 说明 | railpack 对应 |
  |------|------|------|------|--------------|
  | `--env` | `-e` | `Vec<String>` | 环境变量（可多次使用，格式 `KEY=VALUE`） | `--env` |
  | `--previous` | — | `Vec<String>` | 上次构建的包版本（格式 `package@version`） | `--previous` |
  | `--build-cmd` | — | `Option<String>` | 覆盖构建命令 | `--build-cmd` |
  | `--start-cmd` | — | `Option<String>` | 覆盖启动命令 | `--start-cmd` |
  | `--config-file` | — | `Option<String>` | 配置文件相对路径（默认 `arcpack.json`） | `--config-file` |
  | `--error-missing-start` | — | `bool` | 无启动命令时返回错误 | `--error-missing-start` |

  **核心函数：**
  - `generate_build_result_for_command(args) -> Result<(BuildResult, App, Environment)>` — 从 CLI 参数构造 App、Environment、Config，调用 `generate_build_plan()` 返回 BuildResult。对齐 railpack `GenerateBuildResultForCommand()`。
  - `init_tracing(verbosity)` — 初始化 tracing-subscriber，支持 `NO_COLOR` / `FORCE_COLOR` 环境变量（对齐 railpack 的 `termenv` 色彩控制）。
  - `add_schema_to_plan_json(plan) -> Result<serde_json::Value>` — 向 plan JSON 注入 `$schema` 字段（对齐 railpack `addSchemaToPlanMap()`），提供 IDE 编辑器支持。

- `src/core.rs`（或 `src/lib.rs` 中新增）— `BuildResult` 结构体定义

  **BuildResult**（对齐 railpack `core.BuildResult`）：

  ```rust
  pub struct BuildResult {
      pub arcpack_version: String,
      pub plan: Option<BuildPlan>,
      pub resolved_packages: HashMap<String, ResolvedPackage>,
      pub metadata: HashMap<String, String>,
      pub detected_providers: Vec<String>,
      pub logs: Vec<LogMsg>,
      pub success: bool,
  }
  ```

**测试要求：**
- clap 解析正确性测试（各子命令、全局参数、公共 flags 正确解析）
- `--env KEY=VALUE` 解析为正确的键值对
- `--previous pkg@1.0` 解析为正确的包版本映射
- `generate_build_result_for_command` 对有效/无效路径的处理测试
- `add_schema_to_plan_json` 输出包含 `$schema` 字段

---

### T3.2 arcpack plan 命令

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§1.4 |
| **railpack 参考** | `rp:cli/plan.go` |
| **依赖** | T3.1 |

**描述：** 输出 JSON 格式的 BuildPlan，支持文件输出和 `$schema` 注入。

**交付文件：**
- `src/cli/plan.rs` — PlanArgs + `run_plan(args) -> Result<()>`

  **专属 flags**（对齐 railpack `PlanCommand`）：

  | Flag | 短名 | 类型 | 说明 | railpack 对应 |
  |------|------|------|------|--------------|
  | `--out` | `-o` | `Option<String>` | 输出文件路径（空则写 stdout） | `--out` |

  加上所有公共构建 flags。

  **输出行为**（对齐 railpack）：
  - JSON 使用 2 空格缩进（`serde_json::to_string_pretty`）
  - 输出 JSON 包含 `$schema` 字段（调用 `add_schema_to_plan_json()`）
  - `--out` 指定时自动创建父目录，写入文件后日志提示路径
  - 无 `--out` 时写入 stdout + 换行符

**测试要求：**
- 调用 run_plan 对 node-npm fixture，验证 stdout 输出为合法 JSON 且可反序列化为 BuildPlan
- 输出 JSON 包含 `$schema` 字段
- `--out /tmp/test-plan.json` 正确写入文件

---

### T3.3 arcpack info 命令 + pretty print

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§1.4 |
| **railpack 参考** | `rp:cli/info.go`, `rp:core/prettyPrint.go` |
| **依赖** | T3.1 |

**描述：** 输出构建元信息，支持 pretty/json 两种格式。

**交付文件：**
- `src/cli/info.rs` — InfoArgs + `run_info(args) -> Result<()>`

  **专属 flags**（对齐 railpack `InfoCommand`）：

  | Flag | 短名 | 类型 | 默认值 | 说明 | railpack 对应 |
  |------|------|------|--------|------|--------------|
  | `--format` | — | `String` | `pretty` | 输出格式：`pretty` 或 `json` | `--format` |
  | `--out` | — | `Option<String>` | — | 输出文件路径 | `--out` |

  加上所有公共构建 flags。

  **输出行为**（对齐 railpack）：
  - `--format pretty`（默认）：调用 `format_build_result()` 输出终端美化格式
  - `--format json`：输出 BuildResult 的 JSON（2 空格缩进）
  - `--out` 行为同 plan 命令
  - **`buildResult.success == false` 时退出码为 1**（对齐 railpack `os.Exit(1)`）

- `src/core/pretty_print.rs`（或 `src/pretty_print.rs`）— 终端美化输出模块

  **pretty print 内容**（对齐 railpack `core/prettyPrint.go`）：
  - `format_build_result(result, options) -> String` — 格式化 BuildResult
  - `pretty_print_build_result(result, options)` — 直接打印到 stderr（build/prepare 命令用）
  - 显示内容：detected providers、resolved packages（名称 + 版本 + 来源）、logs、metadata
  - 终端着色：使用 `owo-colors` 或 `console` crate（对应 railpack 的 `lipgloss`），支持 `NO_COLOR` / `FORCE_COLOR`

  `PrintOptions`（对齐 railpack `PrintOptions`）：

  ```rust
  pub struct PrintOptions {
      pub metadata: bool,
      pub version: String,
  }
  ```

**测试要求：**
- 调用 run_info 对 node-npm fixture，验证 pretty 输出包含 "node" provider 名称
- `--format json` 输出为合法 JSON 且可反序列化为 BuildResult
- `--out` 写文件正确
- success == false 时退出码为 1

---

### T3.4 arcpack schema 命令

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§1.4 |
| **railpack 参考** | `rp:cli/schema.go` |
| **依赖** | T3.1 |

**描述：** 输出 arcpack.json 的 JSON Schema。无参数无 flags（对齐 railpack）。

**交付文件：**
- `src/cli/schema.rs` — `run_schema() -> Result<()>`：使用 `schemars::schema_for!(Config)` 生成 JSON Schema，2 空格缩进输出到 stdout + 换行符。

**测试要求：** 验证输出为合法 JSON Schema（包含 `"type": "object"`），Schema 包含 Config 所有顶层字段。

---

### T3.5 arcpack prepare 命令

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§1.4 |
| **railpack 参考** | `rp:cli/prepare.go` |
| **依赖** | T3.1, T3.3（复用 pretty print） |

**描述：** 为平台集成准备构建产物文件——输出 plan JSON 和 info JSON 到指定路径，供 BuildKit frontend 或其他构建系统消费。对齐 railpack `prepare` 命令。

**交付文件：**
- `src/cli/prepare.rs` — PrepareArgs + `run_prepare(args) -> Result<()>`

  **专属 flags**（对齐 railpack `PrepareCommand`）：

  | Flag | 类型 | 说明 | railpack 对应 |
  |------|------|------|--------------|
  | `--plan-out` | `Option<String>` | plan JSON 输出文件路径 | `--plan-out` |
  | `--info-out` | `Option<String>` | info JSON 输出文件路径（plan 字段置空） | `--info-out` |
  | `--show-plan` | `bool` | 将 plan JSON 输出到 stdout | `--show-plan` |
  | `--hide-pretty-plan` | `bool` | 隐藏 pretty print 输出 | `--hide-pretty-plan` |

  加上所有公共构建 flags。

  **执行流程**（对齐 railpack）：
  1. `generate_build_result_for_command()` 生成 BuildResult
  2. 若未设 `--hide-pretty-plan`：调用 `pretty_print_build_result()` 输出到 stderr
  3. 若 `success == false`：退出码 1
  4. 若设 `--show-plan`：plan JSON（含 `$schema`）输出到 stdout
  5. 若设 `--plan-out`：plan JSON（含 `$schema`）写入文件
  6. 若设 `--info-out`：BuildResult JSON（plan 字段置为 None）写入文件

  **公共文件写入函数：** `write_json_file(path, data, log_msg) -> Result<()>` — 自动创建父目录（0755）、JSON 2 空格缩进、文件权限 0644、debug 日志。对齐 railpack `writeJSONFile()`。

**测试要求：**
- `--plan-out /tmp/plan.json` 输出包含 `$schema` 的合法 JSON
- `--info-out /tmp/info.json` 输出的 JSON 中 plan 字段为 null
- `--show-plan` 将 plan JSON 输出到 stdout
- `--hide-pretty-plan` 抑制 pretty print 输出
- success == false 时退出码为 1

---

### T3.6 main.rs 集成 + assert_cmd 测试

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§1.4, Arch§6.1 |
| **railpack 参考** | `rp:cmd/cli/main.go` |
| **依赖** | T3.2, T3.3, T3.4, T3.5 |

**描述：** 完成 main.rs 的命令分发和端到端 CLI 测试。

**交付文件：**
- `src/main.rs`（更新）— 初始化 tracing（含 `NO_COLOR` / `FORCE_COLOR` 检测）→ 解析 CLI → 分发到 run_plan / run_info / run_schema / run_prepare / run_build（build 暂输出 "not yet implemented"）
- `tests/cli_tests.rs` — 使用 `assert_cmd` + `predicates` 编写端到端测试

**测试要求：**

基础命令测试：
- `arcpack plan tests/fixtures/node-npm` 退出码 0，stdout 包含 `"steps"` 和 `"$schema"`
- `arcpack info tests/fixtures/node-npm` 退出码 0，stdout 包含 `node`
- `arcpack info --format json tests/fixtures/node-npm` 退出码 0，stdout 为合法 JSON
- `arcpack schema` 退出码 0，stdout 包含 `"type"`
- `arcpack prepare tests/fixtures/node-npm --plan-out /tmp/test-plan.json` 退出码 0，文件存在且为合法 JSON
- `arcpack --help` 输出帮助信息
- `arcpack --version` 输出版本信息

错误处理测试：
- `arcpack plan /nonexistent` 退出码非 0，stderr 包含错误信息
- `arcpack plan`（无参数）退出码非 0，提示 directory 参数必需

公共 flags 测试：
- `arcpack plan --env FOO=bar tests/fixtures/node-npm` 退出码 0
- `arcpack plan --build-cmd "npm run build" tests/fixtures/node-npm` 退出码 0
- `arcpack plan --start-cmd "node server.js" tests/fixtures/node-npm` 退出码 0

文件输出测试：
- `arcpack plan --out /tmp/test-plan.json tests/fixtures/node-npm` 正确写入文件

---

## 与 railpack 的已知差异

| 方面 | railpack | arcpack | 原因 |
|------|----------|---------|------|
| CLI 框架 | `urfave/cli/v3` | `clap` derive | Rust 生态标准 |
| 日志级别 | `--verbose` 单级（Debug） | `-v`/`-vv` 双级（DEBUG/TRACE） | 增强：TRACE 级别用于内部调试 |
| 终端美化 | `charmbracelet/lipgloss` | `owo-colors` 或 `console` crate | Rust 生态对应 |
| `frontend` 命令 | 有（启动 gRPC frontend server） | Phase 3 不含，Phase B 实现 | 依赖 gRPC SDK，属于 Phase B 范畴 |
| slice flag 分隔符 | 全局禁用 | clap 默认行为 | 框架差异，`-e A -e B` 形式一致 |

---

## Phase 3 Gate

**执行命令：**
```bash
cargo check
cargo test
cargo build --release
./target/release/arcpack plan tests/fixtures/node-npm | python3 -m json.tool
./target/release/arcpack info tests/fixtures/node-npm
./target/release/arcpack info --format json tests/fixtures/node-npm | python3 -m json.tool
./target/release/arcpack schema | python3 -m json.tool
./target/release/arcpack prepare tests/fixtures/node-npm --plan-out /tmp/gate-plan.json
./target/release/arcpack --version
```

**验收清单：**
- [x] `cargo check` 无错误无警告
- [x] `cargo test` 全部通过（预计 120+ 个测试用例）
- [x] `arcpack plan tests/fixtures/node-npm` 输出合法 JSON，包含完整 BuildPlan 和 `$schema` 字段
- [x] `arcpack plan --out /tmp/plan.json tests/fixtures/node-npm` 正确写入文件
- [x] `arcpack info tests/fixtures/node-npm` 输出 pretty 格式，包含 Provider 名称 "node" 和版本信息
- [x] `arcpack info --format json tests/fixtures/node-npm` 输出合法 JSON BuildResult
- [x] `arcpack schema` 输出合法 JSON Schema，包含 Config 所有字段
- [x] `arcpack prepare tests/fixtures/node-npm --plan-out /tmp/p.json --info-out /tmp/i.json` 两个文件均为合法 JSON
- [x] `-v` 启用 DEBUG 日志，`-vv` 启用 TRACE 日志
- [x] `--env`、`--build-cmd`、`--start-cmd` flags 正确传递到构建流程
- [x] `NO_COLOR=1` 禁用终端颜色
- [x] `--version` 输出版本信息
- [x] 无效路径输入时返回友好错误信息和非零退出码
- [x] `assert_cmd` 集成测试全部通过
- [x] `arcpack build` 子命令存在但输出 "not yet implemented"
