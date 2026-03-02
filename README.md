# Arcpack

零配置应用构建器 — 自动检测源码的语言/框架 → 生成构建计划 → 转译为 LLB → 通过 BuildKit 输出 OCI 镜像，无需编写 Dockerfile。

ArcBox PaaS 平台的核心构建组件。

## 核心架构

```
源码 → Provider 检测 → BuildPlan → LLB 转译 → BuildKit 构建 → OCI 镜像
```

- **Source Analyzer** — 读取源码目录，提供文件系统抽象（glob 缓存 + JSONC 解析）
- **Provider** — 语言/框架检测器，多个可同时匹配（detect → initialize → plan → cleanse_plan）
- **BuildPlan** — 聚合所有 Provider 输出的构建蓝图（Step DAG + Layer + Filter + Cache + Command）
- **LLB Generator** — 将 BuildPlan 转译为 BuildKit DAG，实施 OCI 分层策略
- **BuildKit Client** — gRPC 通信（Session + FileSend + Secrets），支持子进程/外部 daemon

## 快速开始

```bash
# 生成构建计划（预览）
arcpack plan /path/to/your/app

# 完整构建 → OCI 镜像
arcpack build /path/to/your/app

# 查看构建元数据
arcpack info /path/to/your/app

# 输出 arcpack.json JSON Schema
arcpack schema
```

## Roadmap

### 核心流水线

| 状态 | 功能 | 说明 |
|------|------|------|
| ✅ | Source Analyzer | App 文件系统抽象 + glob 缓存 + JSONC 解析 |
| ✅ | Provider 框架 | detect → initialize → plan → cleanse_plan 生命周期 |
| ✅ | BuildPlan 数据结构 | Step DAG、Layer、Filter、Cache、Command |
| ✅ | Plan 验证 | commands / step-inputs / deploy-base / start-command |
| ✅ | DAG 拓扑排序 | 传递依赖消除 |
| ✅ | LLB 原语 | exec / file / merge / source / terminal |
| ✅ | BuildPlan → LLB 转换 | 直接转换路径 |
| ✅ | BuildPlan → Dockerfile | 兼容路径 |
| ✅ | BuildKit gRPC 客户端 | Session + FileSend + Secrets |
| ✅ | buildkitd 子进程管理 | SubprocessDaemonManager |
| ✅ | 外部 buildkitd 连接 | ExternalDaemonManager via BUILDKIT_HOST |

### CLI 命令

| 状态 | 命令 | 说明 |
|------|------|------|
| ✅ | `arcpack plan` | 生成 BuildPlan JSON |
| ✅ | `arcpack build` | 完整构建 → OCI 镜像 |
| ✅ | `arcpack info` | 构建元数据输出 |
| ✅ | `arcpack schema` | arcpack.json JSON Schema |
| ✅ | `arcpack prepare` | 写入 plan + info JSON 文件 |
| ✅ | `arcpack frontend` | BuildKit frontend 模式 |

### Provider / 处理器（15 个）

| 状态 | Provider | 说明 |
|------|----------|------|
| ✅ | Node.js | npm/pnpm/yarn/bun + 9 个框架 + SPA + workspace + Corepack |
| ✅ | Python | pip/uv/poetry/pdm/pipenv + Django/FastAPI/Flask/FastHTML |
| ✅ | Go | go modules + workspace + CGO 检测 + Gin 元数据 |
| ✅ | Rust | Cargo + workspace + WASM 检测 + 7 级版本解析 |
| ✅ | Java | Maven/Gradle + Spring Boot + wrapper 支持 |
| ✅ | PHP | Composer + Laravel + FrankenPHP + PHP 扩展检测 + Node.js 双语构建 |
| ✅ | Ruby | Bundler + Rails + YJIT + Node.js/ExecJS 集成 |
| ✅ | Elixir | Mix/Hex + Phoenix + Erlang 版本兼容映射 + Node.js assets |
| ✅ | Gleam | gleam.toml + Erlang shipment |
| ✅ | Deno | deno.json/deno.jsonc + 主文件检测 |
| ✅ | .NET | NuGet + dotnet publish + 多目标框架 |
| ✅ | C++ | CMake/Meson + Ninja |
| ✅ | 静态网站 | Staticfile/public/index.html + Caddy |
| ✅ | Shell | shebang 解析 + 多 shell 支持 |
| ✅ | Procfile | 后处理器，web > worker > first |

### 配置系统

| 状态 | 功能 | 说明 |
|------|------|------|
| ✅ | arcpack.json | 项目配置文件 |
| ✅ | 环境变量覆盖 | ARCPACK_* 前缀 |
| ✅ | JSON Schema 生成 | schemars |
| ✅ | .dockerignore 支持 | 构建上下文过滤 |
| ✅ | Secrets 管理 | SHA256 hash + GITHUB_TOKEN 自动注入 |

### 工具集成

| 状态 | 功能 | 说明 |
|------|------|------|
| ✅ | mise | 版本管理器集成 |
| ✅ | Caddy | Web 服务器（SPA + 静态网站） |
| ✅ | BuildKit 缓存 | cache-import / cache-export / cache-key |

### 测试

| 状态 | 功能 | 说明 |
|------|------|------|
| ✅ | 单元测试 | 80+ 个 `#[cfg(test)]` 模块 |
| ✅ | insta 快照测试 | 32 个 fixture 快照 |
| ✅ | 集成测试框架 | tests/ 目录 |
| 🚧 | 集成测试覆盖 | 仅 8 个 Node.js fixture 有 test.json（共 33 个 fixture） |

### 待实现功能（P0 对齐项）

| 状态 | 编号 | 说明 |
|------|------|------|
| ✅ | P0-01 | Frontend plan-file 读取路径（有 filename 时读 plan 文件，未传时回退检测） |
| ⬜ | P0-02 | docker-container:// BuildKit 连接协议 |
| ✅ | P0-03a | 默认镜像命名（从源码目录名派生） |
| ⬜ | P0-03b | docker load 导出 |
| 🚧 | P0-05 | CLI 语义对齐：--env bare KEY、--error-missing-start flag |
| 🚧 | P0-06 | ARCPACK_CONFIG_FILE 已接入配置优先级，railpack.json 兼容待做 |
| ⬜ | P0-08 | 回归测试补强：fixture 补充到 railpack 级别 104 个 |
| ⬜ | P0-09 | CI/CD（GitHub Actions workflow） |

### 潜在改进项

| 状态 | 说明 |
|------|------|
| ⬜ | C++ 二进制名称从 CMakeLists.txt 解析（当前用目录名启发式） |
| ⬜ | Gleam 版本固定（当前 latest） |
| ⬜ | 更多 Node.js fixture（turborepo/prisma/puppeteer 等） |
| ⬜ | Python fixture 补充（fastapi/flask/pdm 等） |
| ⬜ | tty 进度显示（当前仅 plain text） |

## 技术栈

- **语言**: Rust
- **构建工具**: BuildKit (buildkitd + buildctl)
- **通信方式**: gRPC（Unix Socket / TCP）
- **关键 crate**: tokio（异步）、tonic（gRPC）、clap（CLI）、serde（序列化）

## 许可证

Private — ArcBox 内部项目
