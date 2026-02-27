# Phase 6: 集成测试框架 + Node.js Provider 深度补全

> [← 返回目录](./README.md) | 上一阶段：[← Phase 5](./phase-5-providers.md) | 下一阶段：[Phase 7 →](./phase-7-lightweight-providers.md)

**目标：** 建立端到端集成测试体系（对齐 railpack integration test）；Node.js Provider 覆盖主流框架检测、SPA 部署、monorepo 支持和依赖裁剪。

**前置条件：** Phase 5 全部完成（7 种基础 Provider 已就绪）

## 任务依赖图

```
T6.1 (集成测试框架) ──────────────────────────┐
 │                                            │
 ├──► T6.7 (现有 fixture 集成测试)            │
 │                                            │
T6.2 (框架检测)                               │
 ├──► T6.3 (SPA 部署/Caddy)                   │
 │     └──► T6.6 (Puppeteer/杂项)             │
 ├──► T6.5 (Workspace/Monorepo)               │
 └──► T6.4 (依赖裁剪/Prune)                   │
                                              ▼
T6.8 (注册表更新 + 快照测试) ◄── 全部完成后 ──┘
```

## 任务列表

### T6.1 集成测试框架

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P0 |
| **railpack 参考** | `rp:integration/` 目录 |
| **依赖** | Phase 5 |

**描述：** 建立端到端集成测试基础设施，后续所有 Phase 共用。扫描 fixture 目录，读取 test.json 配置，通过 BuildKit 构建镜像后 docker run 验证输出或 HTTP 健康检查。

**交付文件：**
- `tests/integration_tests/mod.rs` — 测试运行器主入口
- `tests/integration_tests/test_config.rs` — `TestConfig` 结构体（解析 test.json）
- `tests/integration_tests/http_check.rs` — HTTP 健康检查（重试 + 超时）
- `tests/integration_tests/docker_compose.rs` — docker-compose 服务管理

**test.json 格式（对齐 railpack）：**

```json
[
  {
    "platform": "linux/amd64",
    "expectedOutput": "Hello world",
    "envs": { "KEY": "VALUE" },
    "justBuild": false,
    "shouldFail": false,
    "httpCheck": {
      "path": "/",
      "expected": 200,
      "internalPort": 3000,
      "expectedOutput": "text in body"
    }
  }
]
```

**TestConfig 结构体：**

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestConfig {
    pub platform: Option<String>,          // 默认 "linux/amd64"
    pub expected_output: Option<String>,   // stdout 匹配模式
    pub envs: Option<HashMap<String, String>>,
    pub just_build: Option<bool>,          // 仅构建不运行
    pub should_fail: Option<bool>,         // 期望构建失败
    pub http_check: Option<HttpCheck>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpCheck {
    pub path: String,                      // HTTP 请求路径
    pub expected: u16,                     // 期望状态码
    pub internal_port: Option<u16>,        // 容器内端口，默认 3000
    pub expected_output: Option<String>,   // body 匹配
}
```

**测试流程：**
1. 扫描 `tests/fixtures/*/test.json`，为每个目录生成 `#[test] #[ignore]` 测试
2. 调用 `generate_build_plan()` 生成 BuildPlan
3. 调用 `BuildKitClient::build()` 构建镜像（tag: `arcpack-test-{fixture_name}:latest`）
4. `shouldFail` 模式：断言构建失败
5. `justBuild` 模式：构建成功即通过
6. `expectedOutput` 模式：`docker run --rm {envs} {image}` → 扫描 stdout 包含指定字符串
7. `httpCheck` 模式：`docker run -d -p {random_port}:{internal_port} {envs} {image}` → HTTP 轮询（最多 35s，300ms 间隔）→ 断言状态码和 body → `docker stop` 清理
8. 若 fixture 目录含 `docker-compose.yml`：先 `docker compose up -d --wait`，测试容器加入同一网络，测试结束后 `docker compose down`

**运行命令：**
```bash
cargo test --test integration_tests -- --ignored          # 全部集成测试
cargo test --test integration_tests -- --ignored node-npm # 单个 fixture
```

**HTTP 检查重试逻辑：**
```rust
async fn http_check(port: u16, check: &HttpCheck) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("http://localhost:{}{}", port, check.path);
    let max_attempts = 117;  // ~35s at 300ms interval
    let interval = Duration::from_millis(300);

    for attempt in 0..max_attempts {
        match client.get(&url).send().await {
            Ok(resp) => {
                if resp.status().as_u16() == check.expected {
                    if let Some(expected) = &check.expected_output {
                        let body = resp.text().await?;
                        assert!(body.contains(expected),
                            "body does not contain '{expected}'");
                    }
                    return Ok(());
                }
            }
            Err(_) if attempt < max_attempts - 1 => {
                tokio::time::sleep(interval).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(anyhow!("HTTP check timed out after 35s"))
}
```

**docker-compose 管理：**
```rust
pub struct DockerComposeContext {
    fixture_dir: PathBuf,
    network_name: String,
    running: bool,
}

impl DockerComposeContext {
    pub async fn start(&mut self) -> Result<()>;   // docker compose up -d --wait
    pub fn network_name(&self) -> &str;             // 传给 docker run --network
    pub async fn stop(&mut self) -> Result<()>;     // docker compose down -v
}

impl Drop for DockerComposeContext {
    fn drop(&mut self) {
        // 确保测试失败时也清理
        if self.running {
            std::process::Command::new("docker")
                .args(["compose", "-f", ..., "down", "-v"])
                .output().ok();
        }
    }
}
```

**测试要求：**
- test.json 解析正确性测试（各字段组合：仅 expectedOutput、仅 httpCheck、shouldFail、justBuild）
- test.json 数组格式（多个 test case）解析测试
- HTTP check 重试逻辑测试（mock HTTP server，先返回 503 再返回 200）
- docker-compose 生命周期管理测试
- fixture 目录扫描逻辑测试（含/无 test.json 的目录过滤）

---

### T6.2 Node.js 框架检测

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P0 |
| **railpack 参考** | `rp:core/providers/node/frameworks.go`（约 400 行，12 种框架） |
| **依赖** | Phase 2（Node.js Provider 已存在） |

**描述：** 在 Node.js Provider 中新增主流框架自动检测，决定部署模式（SSR 进程 vs SPA 静态）。框架检测结果影响 start_cmd、缓存配置和 SPA 部署流程。

**交付文件：**
- `src/provider/node/frameworks.rs` — `FrameworkInfo` + `detect_frameworks()`

**核心类型：**

```rust
#[derive(Debug, Clone)]
pub enum DeployMode {
    Ssr,    // 运行 Node.js 进程
    Spa,    // 静态文件 + Caddy
}

#[derive(Debug, Clone)]
pub struct FrameworkInfo {
    pub name: String,                    // 如 "nextjs", "vite", "cra"
    pub deploy_mode: DeployMode,
    pub start_cmd: Option<String>,       // SSR 模式的启动命令
    pub output_dir: Option<String>,      // SPA 模式的构建输出目录
    pub cache_dirs: Vec<String>,         // 框架特定缓存目录
    pub build_cmd: Option<String>,       // 覆盖默认 build 命令（少数框架需要）
}

/// 扫描 package.json 和配置文件，返回检测到的框架信息
pub fn detect_framework(
    app: &App,
    env: &Environment,
    pkg: &PackageJson,
    pkg_path: &str,          // monorepo 子包路径，根目录为 ""
) -> Result<Option<FrameworkInfo>>;
```

**检测清单（按优先级排列）：**

| # | 框架 | 检测条件 | 部署模式 | Start Cmd | 输出目录 |
|---|------|---------|---------|-----------|---------|
| 1 | Next.js | `next` 在 dependencies 或 devDependencies | SSR | `npm start`（内含 `next start`） | — |
| 2 | Nuxt | `nuxt` 依赖 | SSR | `node .output/server/index.mjs` | — |
| 3 | Remix | `@remix-run/node` 依赖 | SSR | `npm start` | — |
| 4 | TanStack Start | `@tanstack/react-start` 依赖 | SSR | `npm start` | — |
| 5 | Vite (SSR) | `vite` 依赖 + `vite.config.*` 含 `ssr` 字样 | SSR | `npm start` | — |
| 6 | Astro (SSR) | `astro` 依赖 + `astro.config.*` 含 `output: 'server'` 或 adapter 包依赖 | SSR | `npm start` | — |
| 7 | React Router (SPA) | `react-router.config.*` 存在 或 `@react-router/dev` 依赖 | SPA | Caddy | `build/client/`（默认） |
| 8 | Vite (SPA) | `vite` 依赖 + `scripts.build` 含 `vite build` + 无 SSR 标记 + 不是 SvelteKit | SPA | Caddy | `dist`（默认） |
| 9 | Astro (SPA) | `astro` 依赖 + 无 server output | SPA | Caddy | `dist`（默认） |
| 10 | CRA | `react-scripts` 依赖 + `scripts.build` 含 `react-scripts build` | SPA | Caddy | `build` |
| 11 | Angular | `@angular/core` 依赖 + `angular.json` 存在 + `scripts.build` 含 `ng build` | SPA | Caddy | 解析 `angular.json` |
| 12 | Bun (runtime) | bun 包管理器 + 无以上 SPA 框架 | SSR | `bun run start` | — |

> **重要排除：** SvelteKit（`@sveltejs/kit` 依赖）不应被判定为 Vite SPA，因为 SvelteKit 使用 Vite 但有自己的 SSR 部署方式。检测 Vite SPA 时需排除 `@sveltejs/kit` 在 dependencies 中的情况。

**输出目录检测（SPA 框架）：**

| 框架 | 配置文件 | 配置字段 | 默认值 |
|------|---------|---------|-------|
| Vite | `vite.config.{js,ts,mjs,mts}` | `build.outDir` 正则 | `dist` |
| Astro | `astro.config.{js,ts,mjs,mts}` | `outDir` 正则 | `dist` |
| CRA | — | — | `build`（固定） |
| Angular | `angular.json` | `projects.*.architect.build.options.outputPath` JSON 解析 | `dist/{project_name}/browser` |
| React Router | `react-router.config.{js,ts,mjs,mts}` | `buildDirectory` 正则 | `build/client/` |

Angular 特殊处理：新版 Angular（v17+）的 outputPath 已包含 `/browser` 后缀；旧版需手动追加。检测方式：解析 `angular.json` 的 outputPath，若不以 `/browser` 结尾则追加。

**环境变量控制：**
- `ARCPACK_SPA_OUTPUT_DIR`: 强制 SPA 模式 + 覆盖输出目录（设置此变量即意味着 SPA 模式，无论框架检测结果）
- `ARCPACK_NO_SPA`: 设为 truthy 值时禁用 SPA 检测，所有框架按 SSR 模式处理

**框架特定缓存目录：**

| 框架 | 缓存目录 |
|------|---------|
| Next.js | `/app/{pkg_path}/.next/cache` |
| Remix | `/app/{pkg_path}/.cache` |
| Vite | `/app/{pkg_path}/node_modules/.vite` |
| Astro | `/app/{pkg_path}/node_modules/.astro` |
| React Router | `/app/{pkg_path}/.react-router` |

**与 NodeProvider 集成：**
- `plan()` 中调用 `detect_framework()` 获取 `FrameworkInfo`
- SSR 模式：正常设置 `start_cmd`，追加缓存目录到 build step
- SPA 模式：调用 T6.3 的 `deploy_as_spa()` 替代默认 deploy 配置
- 设置 metadata：`ctx.metadata.set("nodeFramework", info.name)`

**测试要求：**
- 各框架检测条件覆盖测试（12 种框架 × 正向/反向）
- SSR vs SPA 模式判断正确性测试
- 输出目录检测测试（各框架默认值 + 自定义值）
- `ARCPACK_SPA_OUTPUT_DIR` 强制 SPA + 覆盖目录测试
- `ARCPACK_NO_SPA` 禁用 SPA 测试
- SvelteKit 排除在 Vite SPA 之外的测试
- 框架优先级测试（同时存在 next 和 vite 依赖，应检测为 Next.js）
- 缓存目录正确性测试（含 monorepo pkg_path）
- 快照测试（新增 node-next、node-vite-spa fixture）

---

### T6.3 SPA 部署（Caddy）

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P0 |
| **railpack 参考** | `rp:core/providers/node/spa.go`、`rp:core/providers/node/Caddyfile.template` |
| **依赖** | T6.2 |

**描述：** SPA 框架使用 Caddy 作为静态文件服务器部署，包括 Caddyfile 模板和健康检查端点。

**交付文件：**
- `src/provider/node/spa.rs` — `deploy_as_spa()` 函数
- `src/provider/node/caddyfile.template` — 嵌入的 Caddyfile 模板（`include_str!`）

**deploy_as_spa() 函数签名：**

```rust
/// 将当前 Provider 的 deploy 配置改为 SPA 模式
/// - 安装 caddy 二进制
/// - 生成 Caddyfile（模板或用户覆盖）
/// - 设置 deploy inputs 为 caddy + 静态文件
pub fn deploy_as_spa(
    ctx: &mut GenerateContext,
    output_dir: &str,        // SPA 构建输出目录（如 "dist"、"build"）
) -> Result<()>;
```

**SPA 部署流程：**
1. 通过 `MiseStepBuilder` 安装 `caddy`（或从 caddy:latest 镜像 COPY 二进制 — 参考 railpack 实现）
2. 创建 `caddy` 命令步骤：
   - 检查用户项目根目录是否有 `Caddyfile` 或 `Caddyfile.template`
   - 若有：复制用户文件到 `/app/Caddyfile`（模板文件中替换 `{{DIST_DIR}}` 占位符）
   - 若无：使用嵌入的默认模板，替换 `{{DIST_DIR}}` 为实际输出目录
3. deploy 配置：
   - `start_cmd`: `caddy run --config /app/Caddyfile --adapter caddyfile`
   - `inputs`: caddy 二进制层 + Caddyfile + 构建输出目录

**默认 Caddyfile 模板：**

```caddyfile
:3000 {
    root * /app/{{DIST_DIR}}
    encode gzip zstd

    # SPA 路由回退
    try_files {path} {path}.html {path}/index.html /index.html

    # 健康检查
    handle /health {
        respond "OK" 200
    }

    file_server

    # 安全响应头
    header {
        X-Content-Type-Options nosniff
        X-Frame-Options DENY
        Referrer-Policy strict-origin-when-cross-origin
    }
}
```

**Caddy 安装方式（两种，对齐 railpack）：**
- 方式一（推荐）：使用 `ImageStepBuilder` 从 `caddy:2-alpine` 镜像 COPY `/usr/bin/caddy` 二进制
- 方式二：使用 mise 安装 `caddy` — 作为后备，如果方式一不可用

```rust
// 方式一示例
let caddy_step = ImageStepBuilder::new("caddy:install")
    .image("caddy:2-alpine")
    .copy("/usr/bin/caddy", "/usr/bin/caddy");
```

**测试要求：**
- Caddyfile 模板渲染正确性测试（`{{DIST_DIR}}` 替换为 `dist`、`build`、`build/client` 等）
- 用户自定义 Caddyfile 优先级测试（项目根目录有 Caddyfile 时使用用户文件）
- 用户自定义 Caddyfile.template 模板替换测试
- deploy inputs 包含 caddy 二进制层 + Caddyfile + 输出目录
- start_cmd 正确性测试

**集成测试 fixture：**
- `tests/fixtures/node-vite-spa/` — Vite SPA 项目
  - `package.json`（`vite` 依赖 + `"build": "vite build"`）
  - `index.html`、`src/main.js`
  - `test.json`: `[{"httpCheck": {"path": "/", "expected": 200}}]`
- `tests/fixtures/node-cra/` — Create React App 项目
  - `package.json`（`react-scripts` 依赖 + `"build": "react-scripts build"`）
  - `public/index.html`、`src/index.js`
  - `test.json`: `[{"httpCheck": {"path": "/", "expected": 200}}]`

---

### T6.4 依赖裁剪（Prune）

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **railpack 参考** | `rp:core/providers/node/prune.go`（约 150 行） |
| **依赖** | Phase 2 |

**描述：** 通过 `ARCPACK_PRUNE_DEPS=true` 触发，构建后删除 devDependencies 以减小镜像体积。裁剪步骤作为 build 之后的独立步骤插入。

**交付文件：**
- `src/provider/node/prune.rs` — `PruneStep` + 各包管理器裁剪命令

**核心类型：**

```rust
/// 创建依赖裁剪步骤
/// 在 build step 之后插入，移除 devDependencies
pub fn create_prune_step(
    ctx: &mut GenerateContext,
    pm: &PackageManagerKind,
    build_step_name: &str,     // prune step 依赖的前置步骤
) -> Result<Option<String>>;   // 返回 prune step 名称（若启用）
```

**启用条件：**
- `ARCPACK_PRUNE_DEPS` 环境变量为 truthy（`true`/`1`/`yes`）
- 若未设置，不创建裁剪步骤

**各包管理器裁剪命令：**

| 包管理器 | 裁剪命令 | 备注 |
|---------|---------|------|
| npm | `npm prune --omit=dev --ignore-scripts` | |
| pnpm | `pnpm prune --prod --ignore-scripts` | pnpm v8.15.6+ 支持 `--ignore-scripts` |
| bun | `rm -rf node_modules && bun install --production --ignore-scripts` | bun 无原生 prune，需重装 |
| yarn 1 | `yarn install --production=true` | |
| yarn berry v3 | `yarn install --check-cache` | v3 特殊行为 |
| yarn berry v2/v4+ | `yarn workspaces focus --production --all` | 使用 workspace 插件 |

> **pnpm 版本检测：** 需从 `packageManager` 字段或 pnpm lockfile 推断版本。若无法确定版本，默认使用不带 `--ignore-scripts` 的命令。

**自定义裁剪命令：**
- `ARCPACK_NODE_PRUNE_CMD` 环境变量：完全覆盖裁剪命令
- 设置后忽略包管理器自动检测

**CleansePlan 集成：**
裁剪步骤创建后，`NodeProvider::cleanse_plan()` 需要进行以下后处理：
- 从 `npm ci` / `pnpm install` 等 install step 的 cache mount 列表中移除 `node_modules` 缓存
- 原因：Docker 缓存键基于 mount 内容哈希，裁剪后 node_modules 变化导致缓存失效循环
- 仅在 prune 启用时执行此清理

**Deploy inputs 调整：**
- 裁剪启用时：deploy 中 node_modules 的 input layer 应来自 prune step 而非 build step
- 确保裁剪后的精简 node_modules 进入最终镜像

**测试要求：**
- 各包管理器裁剪命令正确性测试（6 种 PM）
- `ARCPACK_PRUNE_DEPS` 未设置时不创建裁剪步骤
- `ARCPACK_NODE_PRUNE_CMD` 自定义命令覆盖测试
- CleansePlan 缓存挂载移除逻辑测试
- 裁剪后 deploy inputs 来自 prune step（非 build step）
- 快照测试（node-npm + prune 场景）

---

### T6.5 Workspace / Monorepo 支持

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **railpack 参考** | `rp:core/providers/node/workspace.go`（约 200 行） |
| **依赖** | T6.2 |

**描述：** 解析 package.json workspaces 字段和 pnpm-workspace.yaml，支持多包项目。在 monorepo 项目中正确检测子包的框架和配置。

**交付文件：**
- `src/provider/node/workspace.rs` — `Workspace` 结构体 + glob 解析

**核心类型：**

```rust
#[derive(Debug, Clone)]
pub struct Workspace {
    pub root_dir: String,          // workspace 根目录
    pub packages: Vec<WorkspacePackage>,
}

#[derive(Debug, Clone)]
pub struct WorkspacePackage {
    pub name: String,              // 包名（来自 package.json.name）
    pub path: String,              // 相对路径（如 "packages/web"）
    pub package_json: PackageJson, // 子包的 package.json
}

/// 解析 workspace 配置，返回所有子包信息
pub fn detect_workspaces(app: &App) -> Result<Option<Workspace>>;
```

**Workspace 检测来源：**

1. **package.json `workspaces` 字段（数组格式）：**
```json
{
  "workspaces": ["packages/*", "apps/*"]
}
```

2. **package.json `workspaces` 字段（对象格式）：**
```json
{
  "workspaces": {
    "packages": ["packages/*", "apps/*"]
  }
}
```

3. **pnpm-workspace.yaml：**
```yaml
packages:
  - 'packages/*'
  - 'apps/*'
  - '!**/test/**'
```

**Glob 解析逻辑：**
- 将 workspace glob 模式转换为实际目录列表
- 支持 `*` 通配符（匹配单级目录）
- 支持 `**` 通配符（匹配多级目录）
- 支持 `!` 排除模式
- 每个匹配目录须包含 `package.json` 才算有效子包
- 读取子包的 `package.json` 填充 `WorkspacePackage`

**与框架检测集成：**
- 框架检测在所有子包上运行（不仅仅是根目录）
- 若多个子包检测到不同框架，选择第一个匹配的 SSR 框架（优先级高于 SPA）
- 缓存目录按子包路径命名（如 `next-packages-web` 而非 `next-cache`）

**与 NodeProvider 集成：**
- `initialize()` 阶段调用 `detect_workspaces()`
- `plan()` 阶段：install step 需复制所有子包的 `package.json`
- build step 的 cwd 可能需要切换到特定子包目录
- metadata 记录：`nodeWorkspace: true`、`nodeWorkspacePackages: ["pkg1", "pkg2"]`

**测试要求：**
- package.json workspaces 数组格式解析测试
- package.json workspaces object 格式解析测试
- pnpm-workspace.yaml 解析测试
- glob 匹配正确性测试（`*`、`**`、`!` 排除）
- 无效目录过滤测试（匹配但无 package.json 的目录）
- 子包 package.json 读取测试
- 框架检测在子包上运行的测试
- 缓存命名包含子包路径的测试

**集成测试 fixture：**
- `tests/fixtures/node-monorepo/` — pnpm workspace + Next.js 子包
  - `package.json`（workspace 配置）
  - `pnpm-workspace.yaml`
  - `pnpm-lock.yaml`
  - `packages/web/package.json`（next 依赖）
  - `packages/web/pages/index.js`
  - `packages/shared/package.json`（共享库）
  - `packages/shared/index.js`
  - `test.json`: `[{"expectedOutput": "Hello from monorepo"}]`

---

### T6.6 Puppeteer / 原生依赖 + 杂项改进

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **railpack 参考** | `rp:core/providers/node/node.go`（puppeteer 检测部分） |
| **依赖** | Phase 2 |

**描述：** 自动检测 Puppeteer 依赖并添加 Chromium APT 包；补充其他细节改进。

**Puppeteer 检测：**
- 检查 `dependencies` 或 `devDependencies` 中是否有 `puppeteer`（注意：不检测 `puppeteer-core`，后者不需要 Chromium）
- 检测到后在 `packages:apt` 步骤中添加以下 APT 包：

```
xvfb gconf-service libasound2 libatk1.0-0 libc6 libcairo2 libcups2
libdbus-1-3 libexpat1 libfontconfig1 libgbm1 libgcc1 libgconf-2-4
libgdk-pixbuf2.0-0 libglib2.0-0 libgtk-3-0 libnspr4 libpango-1.0-0
libpangocairo-1.0-0 libstdc++6 libx11-6 libx11-xcb1 libxcb1
libxcomposite1 libxcursor1 libxdamage1 libxext6 libxfixes3 libxi6
libxrandr2 libxrender1 libxss1 libxtst6 ca-certificates
fonts-liberation libappindicator1 libnss3 lsb-release xdg-utils wget
```

- 设置环境变量 `PUPPETEER_SKIP_CHROMIUM_DOWNLOAD=true`（使用系统 Chromium）

**额外安装模式（ARCPACK_NODE_INSTALL_PATTERNS）：**
- 环境变量 `ARCPACK_NODE_INSTALL_PATTERNS`：逗号分隔的 glob 模式列表
- 这些文件在 install step 中额外复制到构建上下文
- 用途：需要在 npm install 之前存在的文件（如 `.npmrc`、patches 目录等）

**Install 步骤文件列表扩展：**
当前 install step 复制 lockfile + package.json。扩展为同时复制以下目录/文件（若存在）：
- `**/prisma/` — Prisma schema（`postinstall` 时生成客户端）
- `**/patches/` — patch-package / pnpm patch 文件
- `**/.pnpm-patches/` — pnpm v8+ patch 文件
- `.npmrc` — npm 配置

**Node v25+ libatomic1：**
- 检测 Node.js 版本 ≥ 25
- 自动添加 `libatomic1` 到 APT 运行时包
- 原因：Node v25 切换到更新的 V8 引擎，需要 libatomic

**Corepack 配置：**
- 设置 `COREPACK_HOME=/opt/corepack` 环境变量
- 确保 corepack 在 mise install 之后正确初始化

**测试要求：**
- Puppeteer 依赖检测 → APT 包添加测试
- `puppeteer-core` 不触发 APT 包添加的测试
- `ARCPACK_NODE_INSTALL_PATTERNS` 额外 glob 解析测试
- Prisma/patches 目录自动复制测试
- Node v25+ libatomic1 条件添加测试
- Node v24 不添加 libatomic1 的反向测试
- `COREPACK_HOME` 环境变量设置测试

---

### T6.7 现有 fixture 集成测试

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **依赖** | T6.1 + Phase 5 |

**描述：** 为 Phase 5 的所有 Provider fixture 和 Phase 6 新增的 fixture 补充 test.json，跑通第一批集成测试。

**test.json 添加清单：**

| Fixture | 测试类型 | 断言 | 备注 |
|---------|---------|------|------|
| `node-npm` | `expectedOutput` | `"Hello"` | Phase 2 已有 fixture |
| `node-pnpm` | `expectedOutput` | `"Hello"` | Phase 2 已有 fixture |
| `go-basic` | `expectedOutput` | `"Hello"` | Phase 5 fixture |
| `python-pip` | `expectedOutput` | `"Hello"` | Phase 5 fixture |
| `rust-basic` | `expectedOutput` | `"Hello"` | Phase 5 fixture |
| `java-maven` | `expectedOutput` | `"Hello"` | Phase 5 fixture |
| `staticfile` | `httpCheck` | `GET / → 200` | Phase 5 fixture |
| `node-vite-spa` | `httpCheck` | `GET / → 200` | Phase 6 新增 |
| `node-cra` | `httpCheck` | `GET / → 200` | Phase 6 新增 |
| `node-monorepo` | `expectedOutput` | `"Hello from monorepo"` | Phase 6 新增 |

**验收标准：**
```bash
cargo test --test integration_tests -- --ignored   # 全部通过
```

**注意事项：**
- 每个 fixture 的 index.js / main.go / main.py 等入口文件需输出对应的 expectedOutput 字符串
- httpCheck fixture 需有 HTTP 服务器代码或使用 SPA Caddy 部署
- 集成测试需要运行中的 buildkitd 和 docker daemon

---

### T6.8 注册表更新 + 快照测试

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **依赖** | T6.2 ~ T6.6 |

**描述：** 更新 Node.js Provider 的快照测试，新增框架检测、SPA、workspace 相关 fixture。

**新增快照 fixture：**
- `tests/fixtures/node-next/` — Next.js 项目
  - `package.json`（`next` + `react` + `react-dom` 依赖，`"build": "next build"`，`"start": "next start"`）
  - `package-lock.json`（或 mock lockfile）
  - `pages/index.js`
- `tests/fixtures/node-vite-spa/` — Vite SPA 项目（与 T6.3 集成测试共用）
  - 快照验证 deploy 配置包含 Caddy 相关步骤
- `tests/fixtures/node-monorepo/` — pnpm monorepo（与 T6.5 集成测试共用）
  - 快照验证 workspace 检测和子包处理

**快照验证要点：**
- `node-next` 快照：deploy.start_cmd 为 `npm start`，含 `.next/cache` 缓存
- `node-vite-spa` 快照：deploy.start_cmd 含 `caddy`，deploy inputs 含静态文件层
- `node-monorepo` 快照：install step 复制多个 package.json，metadata 含 workspace 信息

**snapshot_tests.rs 更新：**
```rust
#[test]
fn node_next_plan() {
    let result = generate_plan("tests/fixtures/node-next");
    insta::assert_json_snapshot!(result);
}

#[test]
fn node_vite_spa_plan() {
    let result = generate_plan("tests/fixtures/node-vite-spa");
    insta::assert_json_snapshot!(result);
}

#[test]
fn node_monorepo_plan() {
    let result = generate_plan("tests/fixtures/node-monorepo");
    insta::assert_json_snapshot!(result);
}
```

---

## 验证清单

Phase 6 完成后：

```bash
cargo check                                            # 编译无错误
cargo test                                             # 全部单元测试通过
cargo test -- snapshot                                 # 快照测试通过（含新增 fixture）
cargo insta review                                     # 审查新增快照
cargo test --test integration_tests -- --ignored       # 集成测试通过（需 buildkitd + docker）
```
