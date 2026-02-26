# Phase 3: CLI + Plan 输出

> [← 返回目录](./README.md) | 上一阶段：[← Phase 2](./phase-2-provider-framework.md) | 下一阶段：[Phase 4 →](./phase-4-buildkit.md)

**目标：** 通过命令行工具输出 BuildPlan、构建信息和 JSON Schema，使 arcpack 可作为独立 CLI 工具使用。

**前置条件：** Phase 2 全部完成

## 任务依赖图

```
T3.1 (clap 定义 + common)
 ├──► T3.2 (plan command)
 ├──► T3.3 (info command)
 └──► T3.4 (schema command)
       │
       ▼
T3.5 (main.rs 集成 + CLI 测试)
```

## 任务列表

### T3.1 clap 命令行定义 + 公共逻辑

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§1.4, Arch§6.4 |
| **railpack 参考** | `rp:cli/common.go` |
| **依赖** | Phase 2 |

**描述：** 使用 clap derive 定义 CLI 结构。

**交付文件：**
- `src/cli/mod.rs` — 顶层 `Cli` 结构体（全局参数：`-v`/`-vv` 日志级别、`--config` 配置路径覆盖）+ `Commands` 枚举（Plan/Info/Schema/Build）
- `src/cli/common.rs` — `prepare_context(args) -> Result<(App, Environment, Config)>` + `init_tracing(verbosity)`

**测试要求：**
- clap 解析正确性测试（各子命令和全局参数正确解析）
- prepare_context 对有效/无效路径的处理测试

---

### T3.2 arcpack plan 命令

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§1.4 |
| **railpack 参考** | `rp:cli/plan.go` |
| **依赖** | T3.1 |

**描述：** 输出 JSON 格式的 BuildPlan。

**交付文件：**
- `src/cli/plan.rs` — PlanArgs（source 路径 + format 选项 json/yaml）+ `run_plan(args) -> Result<()>`

**测试要求：** 调用 run_plan 对 node-npm fixture，验证 stdout 输出为合法 JSON 且可反序列化为 BuildPlan。

---

### T3.3 arcpack info 命令

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§1.4 |
| **railpack 参考** | `rp:cli/info.go` |
| **依赖** | T3.1 |

**描述：** 输出构建元信息。

**交付文件：**
- `src/cli/info.rs` — InfoArgs（source 路径）+ `run_info(args) -> Result<()>`：提取 provider 名称、语言版本、步骤列表摘要

**测试要求：** 调用 run_info 对 node-npm fixture，验证输出包含 "node" provider 名称。

---

### T3.4 arcpack schema 命令

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§1.4 |
| **railpack 参考** | `rp:cli/schema.go` |
| **依赖** | T3.1 |

**描述：** 输出 arcpack.json 的 JSON Schema。

**交付文件：**
- `src/cli/schema.rs` — `run_schema() -> Result<()>`：使用 `schemars::schema_for!(Config)` 生成 JSON Schema

**测试要求：** 验证输出为合法 JSON Schema（包含 `"type": "object"`），Schema 包含 Config 所有顶层字段。

---

### T3.5 main.rs 集成 + assert_cmd 测试

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§1.4, Arch§6.1 |
| **railpack 参考** | `rp:cmd/cli/main.go` |
| **依赖** | T3.2, T3.3, T3.4 |

**描述：** 完成 main.rs 的命令分发和端到端 CLI 测试。

**交付文件：**
- `src/main.rs`（更新）— 初始化 tracing → 解析 CLI → 分发到 run_plan / run_info / run_schema / run_build（build 暂输出 "not yet implemented"）
- `tests/cli_tests.rs` — 使用 `assert_cmd` + `predicates` 编写端到端测试

**测试要求：**
- `arcpack plan tests/fixtures/node-npm` 退出码 0，stdout 包含 `"steps"`
- `arcpack info tests/fixtures/node-npm` 退出码 0，stdout 包含 `node`
- `arcpack schema` 退出码 0，stdout 包含 `"$schema"`
- `arcpack plan /nonexistent` 退出码非 0，stderr 包含错误信息
- `arcpack --help` 输出帮助信息

---

## Phase 3 Gate

**执行命令：**
```bash
cargo check
cargo test
cargo build --release
./target/release/arcpack plan tests/fixtures/node-npm | python3 -m json.tool
./target/release/arcpack info tests/fixtures/node-npm
./target/release/arcpack schema | python3 -m json.tool
```

**验收清单：**
- [ ] `cargo check` 无错误无警告
- [ ] `cargo test` 全部通过（预计 100+ 个测试用例）
- [ ] `arcpack plan tests/fixtures/node-npm` 输出合法 JSON，包含完整 BuildPlan
- [ ] `arcpack info tests/fixtures/node-npm` 输出 Provider 名称 "node" 和版本信息
- [ ] `arcpack schema` 输出合法 JSON Schema，包含 Config 所有字段
- [ ] `-v` 启用 DEBUG 日志，`-vv` 启用 TRACE 日志
- [ ] 无效路径输入时返回友好错误信息和非零退出码
- [ ] `assert_cmd` 集成测试全部通过
- [ ] `arcpack build` 子命令存在但输出 "not yet implemented"
