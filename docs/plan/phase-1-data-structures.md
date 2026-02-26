# Phase 1: 基础数据结构

> [← 返回目录](./README.md) | 下一阶段：[Phase 2 →](./phase-2-provider-framework.md)

**目标：** 建立项目骨架和所有核心纯数据结构，使其可编译、可序列化、可单元测试。

**前置条件：** 无（项目起点）

## 全局序列化约定

> **重要：** railpack 所有 JSON 字段使用 camelCase（`startCommand`, `buildAptPackages`, `customName`, `deployOutputs`）。
> arcpack 中所有可序列化结构体必须标注 `#[serde(rename_all = "camelCase")]`，
> 个别字段名与 camelCase 规则不一致时使用 `#[serde(rename = "...")]` 单独指定。
> 此约定贯穿 Phase 1 所有任务，不再逐个重复说明。

## 任务依赖图

```
T1.1 (scaffolding)
 └──► T1.2 (error.rs)
       ├──► T1.3 (command + filter + layer + cache)
       │     └──► T1.4 (step + spread + packages)
       │           └──► T1.5 (BuildPlan + Deploy + dockerignore)
       ├──► T1.6 (app/ + environment)
       └──► T1.7 (config/)
```

## 任务列表

### T1.1 项目脚手架初始化

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§2.1, Arch§5.1, Arch§5.2 |
| **依赖** | 无 |

**描述：** 创建项目基础结构。

**交付文件：**
- `Cargo.toml` — 含全部 dependencies / dev-dependencies / features（内容见 Arch§5.1）
- `.gitignore` — 内容见 Arch§5.2
- `src/main.rs` — 空 main 函数
- `src/lib.rs` — 空 mod 声明（预留所有模块）
- `tests/` 目录 + `tests/fixtures/` 目录

**测试要求：** `cargo check` 通过、`cargo test` 通过（零测试）。

---

### T1.2 统一错误类型 error.rs

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.11, Arch§6.3 |
| **railpack 参考** | 无对应文件（railpack 错误分散于各文件，arcpack 集中定义） |
| **依赖** | T1.1 |

**描述：** 使用 `thiserror` 派生 `ArcpackError` 枚举，包含所有 11 个变体。在 `lib.rs` 中声明 `pub type Result<T> = std::result::Result<T, ArcpackError>`。

**交付文件：**
- `src/error.rs`

**变体清单（Arch§3.11）：**
- 用户错误：ConfigParse / NoProviderMatched / NoStartCommand / SourceNotAccessible
- 系统错误：DaemonStartFailed / DaemonTimeout / BuildFailed / PushFailed
- 内部错误：Io (`#[from] std::io::Error`) / Serde (`#[from] serde_json::Error`) / Other (`#[from] anyhow::Error`)

**测试要求：**
- 各变体可构造
- Display trait 输出人类可读信息
- From trait 自动转换验证（io::Error / serde_json::Error / anyhow::Error）

---

### T1.3 Plan 基础类型：Command + Filter + Layer + Cache

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.4, Arch§3.5, Arch§3.6, Arch§8.2 |
| **railpack 参考** | `rp:core/plan/command.go`, `rp:core/plan/filters.go`, `rp:core/plan/layer.go`, `rp:core/plan/cache.go` |
| **依赖** | T1.2 |

**描述：** 实现 plan 模块的四个基础类型。

**交付文件：**
- `src/plan/command.rs` — Command 枚举，含 Exec/Copy/Path/File 四变体。使用 `#[serde(untagged)]` + 自定义 Deserialize（按字段检测区分变体），与 railpack 的 JSON 格式一致（无 type 标签）。各变体 `command_type()` 方法返回值与 railpack 对齐：`"exec"` / `"copy"` / `"globalPath"` / `"file"`。所有变体实现 `Spreadable` trait（见 T1.4）。
- `src/plan/filter.rs` — Filter 结构体，含 include/exclude，空字段 `skip_serializing_if`。工厂方法：`Filter::new(include, exclude)` + `Filter::include_only(include)`（对齐 railpack 的 `NewFilter` / `NewIncludeFilter`）
- `src/plan/layer.rs` — Layer 结构体，含 step/image/local 三种互斥引用 + spread + filter（serde flatten）。实现 `Spreadable` trait。工厂方法：`new_step_layer(name, filter: Option<Filter>)` / `new_image_layer(image, filter: Option<Filter>)` / `new_local_layer()`（对齐 railpack 的 variadic filter 参数风格）
- `src/plan/cache.rs` — `CacheType` 枚举（`Shared` / `Locked`，序列化为 `"shared"` / `"locked"`）+ Cache 结构体（directory + cache_type）。工厂方法：`Cache::new(directory)` 默认 `CacheType::Shared`（对齐 railpack 的 `NewCache`）

**测试要求：**
- 每种 Command 变体的 JSON 序列化/反序列化往返测试，**验证 JSON 格式与 railpack 一致**（无 type 标签，按字段区分）
- `command_type()` 返回值测试：Exec→`"exec"`, Copy→`"copy"`, Path→`"globalPath"`, File→`"file"`
- Layer 三种工厂方法构造正确性测试（含 filter 参数传递）
- Filter 空字段跳过序列化测试 + 工厂方法测试
- Cache 序列化测试 + `Cache::new()` 默认 shared 测试
- CacheType 序列化往返测试

---

### T1.4 Plan 组合类型：Step + Spread + Packages

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.4, Arch§3.6 (Spread) |
| **railpack 参考** | `rp:core/plan/step.go`, `rp:core/plan/spread.go`, `rp:core/plan/packages.go` |
| **依赖** | T1.3 |

**描述：** 实现依赖基础类型的组合结构。

**交付文件：**
- `src/plan/step.rs` — Step 结构体，含 name/inputs/commands/secrets/assets/variables/caches，空字段 `skip_serializing_if`。`Step::new(name)` 工厂方法：初始化空 maps，**secrets 默认为 `vec!["*".to_string()]`**（对齐 railpack 的 `NewStep`，默认授予所有 secret 访问权）
- `src/plan/spread.rs` — `Spreadable` trait（`fn is_spread(&self) -> bool`），由 Command 和 Layer 实现。泛型函数 `spread<T: Spreadable>(left: Vec<T>, right: Vec<T>) -> Vec<T>` + 便捷函数 `spread_strings(left: Vec<String>, right: Vec<String>) -> Vec<String>`（对齐 railpack 的 `Spreadable` interface + `Spread[T]` + `SpreadStrings`）
- `src/plan/packages.rs` — PlanPackages **结构体**（`apt: Vec<String>` + `mise: HashMap<String, String>`），方法：`new()` / `add_apt_package(pkg)` / `add_mise_package(pkg, version)`（对齐 railpack 的 `PlanPackages` struct，含 Apt 和 Mise 两个字段）

**测试要求：**
- Step 完整字段的 JSON 往返测试
- `Step::new()` 默认 secrets = `["*"]` 测试
- Spreadable trait 实现测试（Command::is_spread / Layer::is_spread 对 `"..."` 值的判断）
- `spread()` 泛型函数多场景测试（`"..."` 在头部/中间/尾部/不存在时的行为）
- `spread_strings()` 便捷函数测试
- PlanPackages 序列化测试（验证 apt 和 mise 两个字段均正确序列化，空字段 omit）

---

### T1.5 Plan 顶层：BuildPlan + Deploy + dockerignore

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.4 |
| **railpack 参考** | `rp:core/plan/plan.go`, `rp:core/plan/dockerignore.go` |
| **依赖** | T1.4 |

**描述：** 实现 plan 模块的顶层结构和 .dockerignore 解析。

**交付文件：**
- `src/plan/mod.rs` — BuildPlan 结构体（steps/caches/secrets/deploy），方法：`add_step(step)` + `normalize()`（移除空 Layer 输入、移除 deploy 未引用的孤立步骤，对齐 railpack 的 `AddStep` / `Normalize`）。Deploy 结构体（base/inputs/start_cmd/variables/paths），`start_cmd` 字段使用 `#[serde(rename = "startCommand", skip_serializing_if = "Option::is_none")]`。re-export 所有子模块类型
- `src/plan/dockerignore.rs` — DockerignoreContext 结构体（excludes: Vec\<String\> / includes: Vec\<String\> / has_file: bool），`DockerignoreContext::new(app) -> Result<Self>` 构造函数（读取 .dockerignore 文件并解析）。基于 `ignore` crate（推荐，语义更贴近 Docker）或 `globset` 实现 include/exclude 匹配，需正确处理 `!` 否定模式和 `**` 通配符

**测试要求：**
- 构造包含多步骤的完整 BuildPlan，验证 JSON 序列化/反序列化往返一致性
- `BuildPlan::add_step()` 添加步骤后可通过 steps 字段访问
- `BuildPlan::normalize()` 移除空输入和孤立步骤测试
- Deploy `start_cmd` 序列化为 `"startCommand"` 且空值跳过测试
- DockerignoreContext 构造 + 匹配/排除测试（含 `!` 否定模式）
- 无 .dockerignore 文件时 `has_file = false`，返回空 excludes 测试

---

### T1.6 App 文件系统抽象 + Environment

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.7, Arch§3.8 |
| **railpack 参考** | `rp:core/app/app.go`, `rp:core/app/environment.go` |
| **依赖** | T1.2 |

**描述：** 实现源码目录的只读文件系统抽象和环境变量管理。

**交付文件：**
- `src/app/mod.rs` — App 结构体（source:PathBuf + glob_cache:Mutex<HashMap>），方法：has_file / has_match / read_file / read_json / read_yaml / read_toml / find_files / find_directories / find_files_with_content / is_file_executable / source
- `src/app/environment.rs` — Environment 结构体（variables:HashMap），方法：
  - `from_envs(envs: Vec<String>)` — 解析 `KEY=VALUE` 字符串列表，缺少 `=` 时从 OS 环境变量取值（对齐 railpack 的 `FromEnvs`）
  - `get_variable(key)` / `set_variable(key, value)`
  - `get_config_variable(name) -> (Option<String>, String)` — 返回值 **和** 完整变量名（如 `("18", "ARCPACK_NODE_VERSION")`），用于日志追踪来源（对齐 railpack 的双返回值语义）
  - `get_config_variable_list(name)` / `is_config_variable_truthy(name)`
  - `get_secrets_with_prefix(prefix) -> HashMap<String, String>` — 按前缀过滤变量（对齐 railpack 的 `GetSecretsWithPrefix`，用于 Provider 提取配置组）

**测试要求：**
- 使用 `tempfile` 创建临时目录，写入测试文件，验证 App 各方法
- `from_envs` 解析 `KEY=VALUE` 和缺失值回退 OS 环境变量测试
- `get_config_variable` 返回值 + 变量名二元组测试
- `get_secrets_with_prefix` 前缀过滤测试
- Environment 的 `ARCPACK_` 前缀变量读取测试
- glob 缓存命中测试（同一 pattern 只遍历一次文件系统）

---

### T1.7 Config 加载与序列化

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch§3.9 |
| **railpack 参考** | `rp:core/config/config.go` |
| **依赖** | T1.2, T1.6 |

**描述：** 实现 arcpack.json 配置文件加载。

**交付文件：**
- `src/config/mod.rs` — Config 结构体（provider / build_apt_packages / steps / deploy / packages / caches / secrets）+ StepConfig 结构体 + DeployConfig 结构体。`Config::load(app, env)` 方法（合并文件配置 + 环境变量配置，对应 railpack 的 `GenerateConfigFromFile` + `GenerateConfigFromEnvironment` + `Merge`，注意 railpack 中此逻辑分散在 `core/core.go`，arcpack 统一收敛到 Config 模块）。使用 `schemars` derive JsonSchema。

**测试要求：**
- 合法 arcpack.json 的加载测试
- 空/缺失配置文件返回默认值测试
- 非法 JSON 返回 ConfigParse 错误测试
- JSON Schema 生成非空测试
- JSON 字段名使用 camelCase 验证（`buildAptPackages` 而非 `build_apt_packages`）

---

## Phase 1 Gate

**执行命令：**
```bash
cargo check                    # 编译通过，零警告
cargo test                     # 所有单元测试通过
cargo doc --no-deps            # 文档生成成功
```

**验收清单：**
- [ ] `cargo check` 无错误无警告
- [ ] `cargo test` 全部通过（预计 40+ 个测试用例）
- [ ] 所有结构体使用 `#[serde(rename_all = "camelCase")]`，JSON 输出字段名与 railpack 一致
- [ ] BuildPlan 可 JSON 序列化/反序列化往返，空字段自动跳过
- [ ] Command 枚举使用 `#[serde(untagged)]`，JSON 格式按字段区分（与 railpack 一致，无 type 标签）
- [ ] `command_type()` 返回值：Path 变体返回 `"globalPath"`（非 `"path"`）
- [ ] Spreadable trait 由 Command 和 Layer 实现，`spread()` 泛型函数可用
- [ ] PlanPackages 含 `apt` + `mise` 两个字段（非 HashMap 别名）
- [ ] `Step::new()` 默认 secrets = `["*"]`
- [ ] `BuildPlan::normalize()` 可移除空输入和孤立步骤
- [ ] DockerignoreContext 可正确解析 .dockerignore（含 `!` 否定模式）
- [ ] App 可读取临时目录中的文件，glob 结果被缓存
- [ ] Environment `get_config_variable` 返回 (value, variable_name) 二元组
- [ ] Environment `from_envs` 可解析 `KEY=VALUE` 字符串列表
- [ ] Config 从 arcpack.json 加载成功，缺失文件返回默认空配置
- [ ] ArcpackError 所有变体可构造，Display 输出人类可读信息
- [ ] `src/lib.rs` 声明了所有模块（plan/app/config/error），公共类型通过 `pub use` 导出
