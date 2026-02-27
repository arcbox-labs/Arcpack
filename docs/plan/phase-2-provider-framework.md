# Phase 2: Provider 框架 + Node.js Provider

> [← 返回目录](./README.md) | 上一阶段：[← Phase 1](./phase-1-data-structures.md) | 下一阶段：[Phase 3 →](./phase-3-cli.md)

**目标：** 实现 Provider 框架和 StepBuilder 体系，以 Node.js 为首个 Provider 完成端到端的 BuildPlan 生成。

**前置条件：** Phase 1 全部完成

## 任务依赖图

```
T2.1 (StepBuilder trait + BuildStepOptions)
 ├──► T2.2 (CommandStepBuilder)
 ├──► T2.3 (MiseStepBuilder + ImageStepBuilder)
 └──► T2.4 (DeployBuilder + CacheContext + InstallBinBuilder)
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
| **状态** | `completed` |
| **设计文档** | Arch§3.3 |
| **railpack 参考** | `rp:core/generate/step_builder.go`（StepBuilder interface）, `rp:core/generate/context.go`（BuildStepOptions） |
| **依赖** | Phase 1 |

**描述：** 定义 StepBuilder trait 和 BuildStepOptions。

**交付文件：**
- `src/generate/mod.rs`（初始版本）— StepBuilder trait + BuildStepOptions 结构体

**StepBuilder trait：**
```rust
pub trait StepBuilder {
    fn name(&self) -> &str;
    fn build(&self, plan: &mut BuildPlan, options: &BuildStepOptions) -> Result<()>;
}
```

**BuildStepOptions 结构体：**
```rust
pub struct BuildStepOptions {
    /// 由 Resolver 批量解析后的包版本映射
    pub resolved_packages: HashMap<String, ResolvedPackage>,
    /// 缓存上下文引用
    pub caches: CacheContext,
}

impl BuildStepOptions {
    /// 生成 apt-get install 命令（non-interactive，自动清理）
    pub fn new_apt_install_command(packages: &[String]) -> Command;
}
```

**测试要求：** 创建 MockStepBuilder 实现 trait，验证 build() 正确写入 BuildPlan。BuildStepOptions::new_apt_install_command() 输出正确的 apt 命令格式。

---

### T2.2 CommandStepBuilder 实现

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.3 |
| **railpack 参考** | `rp:core/generate/command_step_builder.go` |
| **依赖** | T2.1 |

**描述：** 最常用的 StepBuilder 实现，支持链式调用。构造器在 GenerateContext 上（而非独立构造），且有按名去重逻辑——同名 step 不会重复创建。

**交付文件：**
- `src/generate/command_step_builder.rs`

**字段：**
```rust
pub struct CommandStepBuilder {
    name: String,
    commands: Vec<Command>,
    inputs: Vec<Layer>,
    variables: HashMap<String, String>,
    caches: Vec<String>,       // 缓存键引用
    secrets: Vec<String>,      // secret 名称
    assets: Vec<Asset>,
    paths: Vec<String>,        // 追加到 PATH
    env_vars: HashMap<String, String>,
    // 注意：不含 deploy_outputs 字段（该概念在 Config 的 StepConfig 中，
    // 通过 applyConfig() 在 Generate 时处理）
}
```

**链式方法：**
- `add_cmd(cmd) -> &mut Self`
- `add_commands(cmds: Vec<Command>) -> &mut Self`
- `add_input(layer) -> &mut Self`
- `add_inputs(layers: Vec<Layer>) -> &mut Self`
- `add_variable(key, value) -> &mut Self`
- `add_variables(map: HashMap) -> &mut Self`
- `add_cache(cache_key) -> &mut Self`
- `add_secret(secret) -> &mut Self`
- `add_asset(asset) -> &mut Self`
- `add_env_vars(map: HashMap) -> &mut Self`
- `add_paths(paths: Vec<String>) -> &mut Self`
- `use_secrets(secrets: Vec<String>) -> &mut Self`（CI 环境下生效）
- `use_secrets_with_prefix(prefix: &str) -> &mut Self`
- `use_secrets_with_prefixes(prefixes: Vec<String>) -> &mut Self`

实现 `StepBuilder::build()`。

> **与 railpack 的差异说明：** railpack 中 CommandStepBuilder 还持有 app/env 私有字段用于引用项目上下文。arcpack 中这些信息通过 GenerateContext 传递，不在 builder 中冗余存储。

**测试要求：**
- 构造 CommandStepBuilder 添加多种命令/输入后调用 build()，验证产出的 Step 字段正确
- 链式调用 API 可用性测试
- 同名 step 去重逻辑测试

---

### T2.3 MiseStepBuilder + ImageStepBuilder 实现

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.3 |
| **railpack 参考** | `rp:core/generate/mise_step_builder.go`, `rp:core/generate/image_step_builder.go` |
| **依赖** | T2.1, T2.5（Resolver 的 PackageRef 类型） |

**描述：** 语言运行时安装（通过 mise）和 Docker 镜像提取的 StepBuilder。两者均使用 Resolver 的 PackageRef 系统管理包版本。

#### MiseStepBuilder

**交付文件：** `src/generate/mise_step_builder.rs`

全局唯一（每个 GenerateContext 最多一个），build() 产出 Step name = `"packages:mise"`。

**字段：**
```rust
pub struct MiseStepBuilder {
    /// 通过 PackageRef 引用的包列表
    packages: Vec<PackageRef>,
    /// Resolver 引用（用于注册 Default/Version）
    resolver: Resolver,
    /// 附加的 apt 依赖包（运行时编译依赖等）
    supporting_apt_packages: Vec<String>,
    /// mise 配置文件路径（如 .tool-versions, .node-version）
    supporting_mise_files: Vec<String>,
    /// 需要写入的 asset 文件
    assets: Vec<Asset>,
    /// 输入层
    inputs: Vec<Layer>,
    /// 环境变量
    variables: HashMap<String, String>,
}
```

**方法：**
- `add_package(name: &str, default_version: &str) -> PackageRef` — 通过 Resolver::default() 注册包，返回 PackageRef
- `add_supporting_apt_package(pkg: &str) -> &mut Self`
- `add_input(layer: Layer) -> &mut Self`
- `skip_mise_install() -> &mut Self` — 标记跳过 mise install（某些场景使用预构建镜像）
- `get_mise_package_versions() -> HashMap<String, String>` — 返回已解析的包版本
- `use_mise_versions(versions: HashMap) -> &mut Self` — 覆盖版本
- `get_output_paths() -> Vec<String>` — 返回安装后的路径列表
- `get_layer() -> Layer` — 返回此 step 的输出层引用
- `get_supporting_mise_config_files() -> Vec<String>` — 返回支持的 mise 配置文件列表

**build() 行为：** 生成 mise.toml asset → 运行 `mise install` 命令 → 若 supporting_apt_packages 非空则先附加 `apt-get install`

#### ImageStepBuilder

**交付文件：** `src/generate/image_step_builder.rs`

**关键设计：** 不持有 image 字符串，持有**闭包** — image 在 Build 时动态解析。

**字段：**
```rust
pub struct ImageStepBuilder {
    name: String,
    /// 闭包：在 build() 时根据 BuildStepOptions 动态决定镜像名
    resolve_step_image: Box<dyn Fn(&BuildStepOptions) -> String>,
    /// Resolver 引用
    resolver: Resolver,
    /// 通过 PackageRef 引用的包列表
    packages: Vec<PackageRef>,
    /// 附加的 apt 依赖包
    apt_packages: Vec<String>,
}
```

**方法：**
- `new(name: &str, resolve_fn: impl Fn(&BuildStepOptions) -> String)` — 构造器在 GenerateContext 上：`ctx.new_image_step(name, resolve_fn)`
- `add_package_default(name: &str, default_version: &str) -> PackageRef` — 注册 Default 包
- `set_version(pkg_ref: &PackageRef, version: &str, source: &str)` — 设定特定版本
- `set_version_available(pkg_ref: &PackageRef, available: bool)` — 标记版本是否可用

**build() 行为：** 调用闭包获取镜像名 → 生成从 Docker 镜像提取文件的 Step

**测试要求：**
- MiseStepBuilder 添加多个包后 build() 产出 Step 验证（步骤名 `"packages:mise"`，commands 包含 mise install）
- MiseStepBuilder 的 PackageRef 注册 → Resolver 集成测试
- ImageStepBuilder 闭包在不同 BuildStepOptions 下解析出不同镜像名
- ImageStepBuilder build() 测试

---

### T2.4 DeployBuilder + CacheContext + InstallBinBuilder

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.2, Arch§3.3 |
| **railpack 参考** | `rp:core/generate/deploy_builder.go`, `rp:core/generate/cache_context.go`, `rp:core/generate/install_bin_builder.go` |
| **依赖** | T2.1 |

**描述：** 部署配置构建器、缓存注册和二进制安装工具。

#### DeployBuilder

**交付文件：** `src/generate/deploy_builder.rs`

**字段：**
```rust
pub struct DeployBuilder {
    /// 基础镜像层（默认为 NewImageLayer(ARCPACK_RUNTIME_IMAGE)）
    base: Layer,
    start_cmd: Option<String>,
    variables: HashMap<String, String>,
    paths: Vec<String>,
    inputs: Vec<Layer>,
    apt_packages: Vec<String>,
    /// 包含规则：按 step 名过滤输出到 deploy
    includes: Vec<DeployInclude>,
}
```

**方法：**
- `set_start_cmd(cmd)` / `add_path(path)` / `add_variable(key, value)` / `add_input(layer)`
- `add_apt_package(pkg)` / `add_apt_packages(pkgs)`
- `has_include_for_step(step_name: &str) -> bool` — 检查某 step 是否有 deploy 包含规则

**build() 行为：** 返回 `()`（非 Deploy），直接写入 `plan.deploy`。若 apt_packages 非空，build 会创建 `"packages:apt:runtime"` 中间步骤。

#### CacheContext

**交付文件：** `src/generate/cache_context.rs`

**字段：**
```rust
pub struct CacheContext {
    caches: HashMap<String, Cache>,
}
```

**常量：**
- `APT_CACHE_KEY = "apt"` — apt 缓存键
- `MISE_CACHE_KEY = "mise"` — mise 缓存键

**方法：**
- `add_cache(name: &str, dir: &str) -> String` — 注册缓存，返回规范化键名
- `add_cache_with_type(name: &str, dir: &str, cache_type: CacheType) -> String` — 注册带类型的缓存
- `set_cache(name: &str, cache: Cache)` — 直接设置缓存
- `get_cache(name: &str) -> Option<&Cache>` — 获取缓存
- `get_apt_caches() -> Vec<String>` — 获取 apt 相关缓存列表

**内部逻辑：** `sanitize_cache_name(name)` — 去首尾斜杠，`/` 替换为 `-`

#### InstallBinBuilder

**交付文件：** `src/generate/install_bin_builder.rs`

**字段：**
```rust
pub struct InstallBinBuilder {
    name: String,
    /// 二进制安装目录
    bin_dir: String,  // 默认 "/arcpack"（对齐 railpack 的 "/railpack"）
    /// 通过 PackageRef 引用的包列表
    packages: Vec<PackageRef>,
    /// Resolver 引用
    resolver: Resolver,
}
```

**方法：**
- `add_package(name: &str, default_version: &str) -> PackageRef` — 注册 PackageRef
- `get_output_paths() -> Vec<String>` — 返回安装后的二进制路径
- `get_layer() -> Layer` — 返回此 step 的输出层引用

**build() 行为：** 生成 `mise install-into <bin_dir>` 命令

**测试要求：**
- DeployBuilder 设置各字段后 build() 正确写入 plan.deploy
- DeployBuilder apt_packages 非空时创建 "packages:apt:runtime" 中间步骤
- CacheContext 注册/去重/sanitize 测试（如 `/foo/bar/` → `foo-bar`）
- CacheContext apt/mise 常量键正确性
- InstallBinBuilder build() 测试（bin_dir 默认值，PackageRef 集成）

---

### T2.5 mise/ 封装 + resolver/ 版本解析

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§2.2 (resolver/mise) |
| **railpack 参考** | `rp:core/mise/mise.go`, `rp:core/mise/install.go`, `rp:core/resolver/resolver.go`, `rp:core/resolver/version.go` |
| **依赖** | T2.3 |

**描述：** mise CLI 封装和版本解析。不要求实际调用 mise 二进制，仅测试脚本生成和数据流。

#### Resolver（累积注册 + 批量解析模式）

**交付文件：** `src/resolver/mod.rs`, `src/resolver/version.rs`

> **重要：** Resolver 不是单函数 `resolve(pkg, version) -> String`。它是一个**累积注册表**，各 StepBuilder 在 plan 阶段注册包需求，最终由 GenerateContext.generate() 统一调用 resolve_packages() 批量解析。

**核心类型：**
```rust
/// 包引用（不可变标识符，在 Default 时创建）
pub struct PackageRef {
    pub name: String,
}

/// 请求的包信息（注册阶段累积）
pub struct RequestedPackage {
    pub name: String,
    pub version: String,           // 请求的版本约束
    pub source: String,            // 版本来源（如 "package.json engines"）
    pub is_version_available: Option<Box<dyn Fn(&str) -> bool>>,  // 版本可用性回调
    pub skip_mise_install: bool,
}

/// 解析后的包信息（批量解析输出）
pub struct ResolvedPackage {
    pub name: String,
    pub requested_version: String,
    pub resolved_version: String,  // 经过模糊匹配后的版本
    pub source: String,
}
```

**Resolver 结构体：**
```rust
pub struct Resolver {
    packages: HashMap<String, RequestedPackage>,
    previous_versions: HashMap<String, String>,  // 上次构建的版本（用于缓存优化）
}
```

**方法：**
- `default(name: &str, default_version: &str) -> PackageRef` — 注册默认包，返回 PackageRef
- `version(pkg_ref: &PackageRef, version: &str, source: &str)` — 为已注册的包设定具体版本
- `set_previous_version(name: &str, version: &str)` — 设置上次构建版本
- `set_version_available(pkg_ref: &PackageRef, callback: impl Fn(&str) -> bool)` — 设置版本可用性回调
- `set_skip_mise_install(pkg_ref: &PackageRef, skip: bool)` — 标记跳过 mise 安装
- `resolve_packages() -> Result<HashMap<String, ResolvedPackage>>` — **批量解析**所有注册的包

**版本规范化规则（`src/resolver/version.rs`）：**
- `resolve_to_fuzzy_version(version: &str) -> String`
  - `"^18.4"` → `"18"`
  - `">=22 <23"` → `"22"`
  - `"3.x"` → `"3"`
  - `"~16.0"` → `"16"`
  - `"18.4.1"` → `"18.4.1"`（精确版本不变）
  - `"lts"` / `"latest"` → `"lts"` / `"latest"`（特殊标签不变）

#### Mise 封装

**交付文件：** `src/mise/mod.rs`, `src/mise/install.rs`

**Mise 结构体方法：**
- `get_latest_version(pkg: &str, version: &str) -> Result<String>` — 查询 mise 获取最新匹配版本
- `get_all_versions(pkg: &str, version: &str) -> Result<Vec<String>>` — 查询所有可用版本
- `generate_mise_toml(packages: &HashMap<String, String>) -> String` — 生成 mise.toml 配置内容
- mise install 脚本命令序列生成

**沙箱隔离（安全边界）：**
- `MISE_PARANOID=1` — 开启严格模式
- 隔离目录：cache、data、state 分别指向临时路径，避免与宿主环境冲突

**测试要求：**
- Resolver::default() + version() 注册后 resolve_packages() 正确解析
- resolve_to_fuzzy_version 版本规范化测试（覆盖上述所有模式）
- PackageRef 引用一致性测试（注册后修改版本仍指向同一包）
- Mise generate_mise_toml() 输出正确的 TOML 格式
- Mise install 脚本生成内容正确性测试

---

### T2.6 GenerateContext 核心编排

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.2 |
| **railpack 参考** | `rp:core/generate/context.go` |
| **依赖** | T2.2, T2.3, T2.4, T2.5 |

**描述：** Provider 执行时的运行时上下文，核心编排器。

**交付文件：**
- `src/generate/mod.rs`（完整版）

**GenerateContext 字段：**
```rust
pub struct GenerateContext {
    app: App,
    env: Environment,
    config: Config,
    base_image: String,
    steps: Vec<Box<dyn StepBuilder>>,
    deploy: DeployBuilder,
    caches: CacheContext,
    secrets: Vec<String>,
    sub_contexts: Vec<String>,   // 子上下文名称栈
    metadata: Metadata,
    resolver: Resolver,
    mise_step_builder: Option<MiseStepBuilder>,
    dockerignore_ctx: DockerignoreContext,
}
```

**Metadata 类型：**
```rust
pub struct Metadata {
    properties: HashMap<String, String>,
}

impl Metadata {
    pub fn set(&mut self, key: &str, value: &str);
    pub fn set_bool(&mut self, key: &str, value: bool);
    pub fn get(&self, key: &str) -> Option<&str>;
}
```

**关键方法：**
- `new(app, env, config)` / `app()` / `env()` / `config()`
- `get_app_source() -> &str` — 获取源码路径
- `new_command_step(name) -> &mut CommandStepBuilder` — 创建命令步骤（同名去重）
- `new_image_step(name, resolve_fn) -> &mut ImageStepBuilder` — 创建镜像步骤
- `get_mise_step_builder() -> &mut MiseStepBuilder`（懒初始化）
- `new_local_layer() -> Layer` — 创建本地源码层（filter 通过 Layer 本身携带，无 `new_local_layer_with_filter`）
- `get_step_name(name: &str) -> String` — 加子上下文前缀（如 `"sub:install"` → `"node:install"`）
- `get_step_by_name(name: &str) -> Option<&dyn StepBuilder>` — 按名查找步骤
- `enter_sub_context(name)` / `exit_sub_context()`
- `resolve_packages() -> Result<HashMap<String, ResolvedPackage>>` — 委托 Resolver 批量解析
- `generate() -> Result<(BuildPlan, HashMap<String, ResolvedPackage>)>`：应用 Config 覆盖 → 解析包版本 → 各 StepBuilder.build() → 组装 Deploy → 返回 (BuildPlan, resolved_packages)

> **注意：** generate() 返回元组 `(BuildPlan, HashMap<String, ResolvedPackage>)` 而非仅 BuildPlan。resolved_packages 传递给 BuildResult。

**测试要求：**
- 构造 GenerateContext 后通过 new_command_step 添加步骤，调用 generate() 验证产出 BuildPlan 的完整性
- Config 覆盖生效测试
- 子上下文命名空间前缀测试（get_step_name 加前缀正确）
- get_step_by_name 查找正确/未找到返回 None
- Metadata set/get 正确性

---

### T2.7 Provider trait + 注册表 + 检测编排

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.1, Arch§8.3 |
| **railpack 参考** | `rp:core/providers/provider.go`, `rp:core/core.go`（getProviders）|
| **依赖** | T2.6 |

**描述：** 定义 Provider trait 和注册表。

> **重要：** railpack 没有 `GetMatchingProvider` 函数。检测逻辑在 `core.go` 的未导出 `getProviders()` 中。arcpack 应将 Provider 注册和检测编排分开设计。

**交付文件：**
- `src/provider/mod.rs`

**Provider trait：**
```rust
pub trait Provider {
    fn name(&self) -> &str;
    fn detect(&self, app: &App, env: &Environment) -> bool;
    fn initialize(&self, _ctx: &mut GenerateContext, _app: &App) -> Result<()> { Ok(()) }
    fn plan(&self, ctx: &mut GenerateContext, app: &App) -> Result<()>;
    fn cleanse_plan(&self, _plan: &mut BuildPlan) -> Result<()> { Ok(()) }
    fn start_command_help(&self) -> Option<String> { None }
}
```

**注册函数：**
- `get_all_providers() -> Vec<Box<dyn Provider>>` — 按优先级排序返回所有 Provider（Phase 2 暂只注册 Node）。顺序应对齐 railpack：Go → Java → Rust → Python → **Node** → StaticFile → Shell
- `get_provider(name: &str) -> Option<Box<dyn Provider>>` — 按名获取特定 Provider

**检测编排逻辑在 `src/lib.rs` 的 `generate_build_plan()` 中：**
```
let providers = get_all_providers();
let matched: Vec<_> = providers.iter()
    .filter(|p| p.detect(&app, &env))
    .collect();
// 支持 Config 中 provider 字段强制指定
// 全未匹配返回 NoProviderMatched 错误
```

**测试要求：**
- 用 MockProvider 测试单匹配/无匹配/Config 强制指定三种场景
- Provider trait 默认方法（initialize、cleanse_plan、start_command_help）不 panic 测试
- get_all_providers() 返回顺序正确
- get_provider("node") 正确返回 NodeProvider

---

### T2.8 Node.js Provider 完整实现

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.1, Arch 附录 A, Arch 附录 B |
| **railpack 参考** | `rp:core/providers/node/node.go`, `rp:core/providers/node/package_manager.go` |
| **依赖** | T2.7 |

**描述：** 首个完整 Provider，覆盖 Node.js 五种包管理器。

> **设计偏差说明：** railpack 将所有包管理器逻辑放在 `package_manager.go` 单文件中。arcpack 选择按包管理器拆分为独立文件（npm.rs / yarn.rs / pnpm.rs / bun.rs），这是 Rust 模块化的合理设计选择。每个文件实现统一的 PackageManager trait。

**交付文件：**
- `src/provider/node/mod.rs` — NodeProvider：detect 检测 package.json；initialize 解析 package.json（scripts/dependencies/engines）；plan 创建 packages/install/build 步骤 + deploy 配置；cleanse_plan 清理
- `src/provider/node/package_json.rs` — PackageJson 解析结构体（scripts/dependencies/devDependencies/engines/packageManager/workspaces）
- `src/provider/node/detect.rs` — 包管理器检测（npm/yarn1/yarnberry/pnpm/bun 通过 lockfile 判断）
- `src/provider/node/npm.rs` — npm install 命令、缓存目录（`/root/.npm`）
- `src/provider/node/yarn.rs` — yarn1 + yarnberry 两种实现
- `src/provider/node/pnpm.rs` — pnpm install 命令、缓存目录
- `src/provider/node/bun.rs` — bun install 命令、缓存目录
- `src/provider/node/workspace.rs` — Workspace 支持（monorepo 检测和处理）

**五种包管理器及缓存路径：**

| 包管理器 | Lockfile | 缓存目录 | CacheType |
|---------|----------|---------|-----------|
| npm | `package-lock.json` | `/root/.npm` | Default |
| yarn1 | `yarn.lock`（无 `.yarnrc.yml`） | `/usr/local/share/.cache/yarn` | Locked |
| yarnberry | `yarn.lock` + `.yarnrc.yml` | `/app/.yarn/cache` | Default |
| pnpm | `pnpm-lock.yaml` | `/root/.local/share/pnpm/store/v3` | Default |
| bun | `bun.lockb` / `bun.lock` | `/root/.bun/install/cache` | Default |

**后续迭代项（不在 Phase 2 范围内）：**
- 框架检测：isNext, isNuxt, isRemix, isVite, isAstro, isAngular, isCRA 等
- SPA 部署路径（Caddyfile.template 生成静态文件服务配置）
- 自定义构建输出目录检测

**测试要求：**
- 使用 fixture 目录验证 detect 正确识别
- 五种包管理器的 install 命令和缓存路径测试
- yarn1 vs yarnberry 正确区分（.yarnrc.yml 存在与否）
- plan() 后 GenerateContext 中步骤数量和内容测试
- PackageJson 解析测试（scripts、engines、workspaces 字段）

---

### T2.9 generate_build_plan() 编排 + 快照测试

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§6.1, Arch§8.7 |
| **railpack 参考** | `rp:core/core.go`（GenerateBuildPlan 函数）, `rp:core/core_test.go` |
| **依赖** | T2.8 |

**描述：** 顶层编排函数和快照测试。

#### GenerateBuildPlanOptions

```rust
pub struct GenerateBuildPlanOptions {
    /// 覆盖构建命令
    pub build_command: Option<String>,
    /// 覆盖启动命令
    pub start_command: Option<String>,
    /// 上次构建的包版本（缓存优化）
    pub previous_versions: HashMap<String, String>,
    /// 配置文件路径
    pub config_file_path: Option<String>,
    /// 环境变量
    pub env_vars: HashMap<String, String>,
}
```

#### BuildResult

```rust
pub struct BuildResult {
    /// 构建计划
    pub plan: BuildPlan,
    /// 解析后的包版本映射
    pub resolved_packages: HashMap<String, ResolvedPackage>,
    /// 构建元数据（如检测到的框架、Node 版本等）
    pub metadata: HashMap<String, String>,
    /// 匹配到的 Provider 名称列表
    pub detected_providers: Vec<String>,
    /// 构建日志
    pub logs: Vec<LogMessage>,
    /// 构建是否成功
    pub success: bool,
}
```

#### 编排流程

**交付文件：**
- `src/lib.rs` 中的 `generate_build_plan(source: &str, options: GenerateBuildPlanOptions) -> Result<BuildResult>`

**流程（严格对齐 railpack core.go）：**
```
1. App::new(source)
2. Environment::new(options.env_vars)
3. Config::load(options.config_file_path)
4. let providers = get_all_providers()
5. let matched = providers.filter(|p| p.detect(&app, &env))
6. GenerateContext::new(app, env, config)
7. for provider in &matched {
       provider.initialize(&mut ctx, &app)?;
   }
8. for provider in &matched {
       provider.plan(&mut ctx, &app)?;
   }
9. // ** Procfile 二次通过 **
   // ProcfileProvider 不在语言 Provider 列表中，
   // 始终在主 Provider plan() 之后独立运行一次。
   // 若项目有 Procfile，它会覆盖 start command。
   procfile_provider.plan(&mut ctx, &app)?;
10. let (plan, resolved_packages) = ctx.generate()?;
11. for provider in &matched {
        provider.cleanse_plan(&mut plan)?;
    }
12. validate_plan(&plan)?;  // 检查 start command 等必要字段
13. 返回 BuildResult { plan, resolved_packages, metadata, ... }
```

> **注意：** 步骤 9 的 Procfile 二次通过是关键——确保 Procfile 中的命令优先级高于 Provider 自动检测的命令。步骤 12 的 ValidatePlan 确保 plan 至少有可用的 start command。

**validate_plan() 检查项：**
- start command 存在
- 至少有一个构建步骤
- Step DAG 无环
- 缓存键引用有效

**测试 fixture：**
- `tests/fixtures/node-npm/package.json`
- `tests/fixtures/node-yarn/package.json` + `yarn.lock`
- `tests/fixtures/node-pnpm/package.json` + `pnpm-lock.yaml`
- `tests/fixtures/node-bun/package.json` + `bun.lockb`
- `tests/fixtures/node-yarn-berry/package.json` + `yarn.lock` + `.yarnrc.yml`

**快照测试：**
- `tests/snapshot_tests.rs` — 对每个 fixture 调用 `generate_build_plan()` 并使用 `insta` crate 做 JSON 快照断言

**测试要求：**
- 5 个 Node.js fixture 的快照测试全部通过（新增 yarn-berry）
- BuildPlan 结构与 railpack 语义对齐（步骤名、依赖关系、缓存键名对齐，参照 Arch§8.7）
- Procfile 覆盖 start command 测试
- ValidatePlan 对无 start command 的 plan 返回错误
- BuildResult 包含正确的 resolved_packages 和 metadata

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
- [x] `cargo check` 无错误无警告
- [x] `cargo test` 全部通过（预计 80+ 个测试用例）
- [x] `generate_build_plan("tests/fixtures/node-npm")` 返回包含 packages:mise/install/build 步骤的 BuildPlan
- [x] Node.js Provider 正确检测 npm/yarn1/yarnberry/pnpm/bun 五种包管理器
- [x] MiseStepBuilder 产出 `"packages:mise"` 步骤，包含 node 运行时包
- [x] CommandStepBuilder 链式 API 可流畅使用
- [x] GenerateContext.generate() 正确应用 Config 覆盖
- [x] 5 个 Node.js fixture 的 `insta` 快照测试全部通过
- [x] BuildPlan 的 Step DAG 依赖关系正确（packages:mise <- install <- build）
- [x] 缓存键名已验证（当前实现：npm-install / yarn-install / pnpm-install）
- [x] PackageRef 模式在 MiseStepBuilder/ImageStepBuilder/InstallBinBuilder 中一致使用
- [x] Resolver 累积注册 + 批量解析流程正确
- [x] Procfile 二次通过按评审决议延期（不属于当前 Phase 2 范围）
- [x] yarn1/yarnberry 两种 PM 均可检测
- [x] ValidatePlan 检查 start command
- [x] BuildResult 包含 resolved_packages 和 metadata

---

## 修正日志

本文档经过对照 railpack 源码的系统性审查和修正，涵盖以下关键变更：

| 编号 | 级别 | 任务 | 修正内容 |
|------|------|------|---------|
| C1 | Critical | T2.3 | MiseStepBuilder：步骤名 "packages:mise"，PackageRef 系统，完整字段和方法 |
| C2 | Critical | T2.3 | ImageStepBuilder：闭包模式 resolve_step_image，Resolver 集成 |
| C3 | Critical | T2.5 | Resolver：从单函数改为累积注册+批量解析模式，PackageRef/ResolvedPackage 类型 |
| C4 | Critical | T2.9 | 编排流程增加 Procfile 二次通过 + ValidatePlan |
| I1 | Important | T2.1 | BuildStepOptions 增加 resolved_packages/caches 字段和 new_apt_install_command |
| I2 | Important | T2.2 | CommandStepBuilder 补充 add_inputs/add_commands/add_variables/add_env_vars/add_paths/use_secrets 等方法 |
| I3 | Important | T2.2 | CommandStepBuilder 移除 deploy_outputs 字段 |
| I4 | Important | T2.4 | DeployBuilder 增加 base 字段，build() 返回 void，apt 创建 runtime 中间步骤 |
| I5 | Important | T2.4 | CacheContext 补充完整 API（add_cache_with_type/set_cache/get_cache/get_apt_caches） |
| I6 | Important | T2.5 | Mise 补充 get_latest_version/get_all_versions/generate_mise_toml，沙箱隔离说明 |
| I7 | Important | T2.6 | GenerateContext 增加 dockerignore_ctx，generate() 返回元组，移除 new_local_layer_with_filter |
| I8 | Important | T2.7 | 移除 get_matching_provider，改为 get_all_providers + get_provider，编排逻辑放 lib.rs |
| I9 | Important | T2.8 | yarn1/yarnberry 拆分（5 种 PM），增加 PackageJson/Workspace 文件，缓存路径修正 |
| I10 | Important | T2.9 | BuildResult 增加 resolved_packages/metadata/logs 等，增加 GenerateBuildPlanOptions |
| S1 | Suggestion | T2.4 | InstallBinBuilder 补充 bin_dir/PackageRef/方法详细描述 |
| S2 | Suggestion | T2.8 | 列出框架检测和 SPA 支持作为后续迭代项 |
| S3 | Suggestion | T2.8 | 修正所有 5 种 PM 的缓存路径 |
| S4 | Suggestion | T2.6 | 增加 Metadata 类型定义 |

**验证参考源文件：**
`core/generate/context.go`, `command_step_builder.go`, `mise_step_builder.go`, `image_step_builder.go`, `deploy_builder.go`, `cache_context.go`, `install_bin_builder.go`, `core/providers/provider.go`, `core/providers/node/node.go`, `core/providers/node/package_manager.go`, `core/core.go`, `core/resolver/resolver.go`, `core/resolver/version.go`, `core/mise/mise.go`
