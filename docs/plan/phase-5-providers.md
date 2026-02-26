# Phase 5: 更多 Provider

> [← 返回目录](./README.md) | 上一阶段：[← Phase 4](./phase-4-buildkit.md)

**目标：** 补齐 Go / Python / Rust / Java / StaticFile / Shell+Procfile 六大语言/场景的 Provider，覆盖主流应用类型。

**前置条件：** Phase 2 全部完成（Provider 框架已就绪）。Phase 3/4 为运行时依赖，但 Provider 开发本身只依赖 Phase 2 的 GenerateContext + StepBuilder。

## 任务依赖图

```
（Phase 2 完成后，以下任务可并行开发）

T5.1 (Go Provider)         ──┐
T5.2 (Python Provider)     ──┤
T5.3 (Rust Provider)       ──┤──► T5.7 (注册表更新 + 全量快照测试)
T5.4 (Java Provider)       ──┤
T5.5 (StaticFile Provider) ──┤
T5.6 (Shell + Procfile)    ──┘
```

## 任务列表

### T5.1 Go Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P0 |
| **设计文档** | Arch 附录 A, Arch 附录 B, Arch§6.2 |
| **railpack 参考** | `rp:core/providers/golang/golang.go` |
| **依赖** | Phase 2 |

**描述：** 检测 go.mod 的 Go 项目构建支持。

**交付文件：**
- `src/provider/golang.rs` — GoProvider：detect（`go.mod`）→ initialize（解析 go.mod 获取 Go 版本和 module 名）→ plan（packages[go runtime via mise] / install[go mod download] / build[go build -o app]）→ deploy（start_cmd = "./app"）。缓存：go-mod（shared）+ go-build（shared）
- `tests/fixtures/go-basic/go.mod` + `main.go`

**测试要求：**
- detect 对含/无 go.mod 目录的双向测试
- Step DAG 结构验证（packages <- install <- build）
- 缓存键名 go-mod / go-build 对齐 railpack
- 快照测试

---

### T5.2 Python Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P0 |
| **设计文档** | Arch 附录 A, Arch 附录 B |
| **railpack 参考** | `rp:core/providers/python/python.go`, `rp:core/providers/python/django.go` |
| **依赖** | Phase 2 |

**描述：** 支持 pip/poetry/pipenv 等多种包管理器，自动检测 Django 框架。

**交付文件：**
- `src/provider/python.rs` — PythonProvider：detect（requirements.txt / pyproject.toml / Pipfile / setup.py）→ initialize（确定包管理器 + Python 版本）→ plan（packages[python runtime] / install[pip install 等]）→ deploy（Django 检测时设 gunicorn 启动命令）。缓存：pip-cache（shared）
- `tests/fixtures/python-pip/requirements.txt`
- `tests/fixtures/python-poetry/pyproject.toml`

**测试要求：**
- detect 对各种 Python 标记文件的覆盖测试
- pip/poetry 两种安装命令正确性测试
- Django 检测测试（含 manage.py + django 依赖时设置 gunicorn）
- 快照测试

---

### T5.3 Rust Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **设计文档** | Arch 附录 A, Arch 附录 B |
| **railpack 参考** | `rp:core/providers/rust/rust.go` |
| **依赖** | Phase 2 |

**描述：** Cargo.toml 检测 + cargo build --release。

**交付文件：**
- `src/provider/rust_lang.rs` — RustProvider：detect（Cargo.toml）→ initialize（解析 package name + rust-version）→ plan（packages[rust runtime] / build[cargo build --release]）→ deploy（start_cmd = "./target/release/{name}"）。缓存：cargo-registry（shared）+ cargo-target（**locked**）
- `tests/fixtures/rust-basic/Cargo.toml` + `src/main.rs`

**测试要求：**
- cargo-target 缓存类型为 locked 而非 shared
- 二进制名称从 Cargo.toml 正确提取
- 快照测试

---

### T5.4 Java Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **设计文档** | Arch 附录 A, Arch 附录 B |
| **railpack 参考** | `rp:core/providers/java/java.go`, `rp:core/providers/java/maven.go`, `rp:core/providers/java/gradle.go` |
| **依赖** | Phase 2 |

**描述：** Maven/Gradle 双路径检测。

**交付文件：**
- `src/provider/java.rs` — JavaProvider：detect（pom.xml / build.gradle / build.gradle.kts）→ initialize（判断 Maven/Gradle + Java 版本）→ plan（packages[java runtime] / build[mvn package 或 gradle build]）→ deploy（start_cmd = "java -jar target/*.jar"）。缓存：maven-repo / gradle-cache（shared）
- `tests/fixtures/java-maven/pom.xml`

**测试要求：**
- Maven/Gradle 双路径检测测试
- 构建命令差异测试（mvn vs gradle）
- JDK 版本解析测试
- 快照测试

---

### T5.5 StaticFile Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P2 |
| **设计文档** | Arch 附录 A |
| **railpack 参考** | `rp:core/providers/staticfile/staticfile.go` |
| **依赖** | Phase 2 |

**描述：** 纯静态网站，使用 Caddy 作为文件服务器。

**交付文件：**
- `src/provider/staticfile.rs` — StaticFileProvider：detect（index.html 且不存在任何语言标记文件）→ plan（文件复制步骤 + Caddyfile 模板 asset）→ deploy（使用 caddy/nginx 作为服务器）
- `tests/fixtures/staticfile/index.html`

**测试要求：**
- detect 仅有 index.html 返回 true，同时有 package.json 返回 false
- Caddyfile 模板内容正确性测试
- 快照测试

---

### T5.6 Shell Provider + Procfile Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P2 |
| **设计文档** | Arch 附录 A |
| **railpack 参考** | `rp:core/providers/shell/shell.go`, `rp:core/providers/procfile/procfile.go` |
| **依赖** | Phase 2 |

**描述：** 可执行脚本和 Procfile 支持。

**交付文件：**
- `src/provider/shell.rs` — ShellProvider：detect（可执行脚本文件）→ plan（文件复制 + deploy start_cmd）
- `src/provider/procfile.rs` — ProcfileProvider：detect（Procfile）→ 解析 Procfile 提取 web 进程启动命令
- `tests/fixtures/with-procfile/Procfile`

**测试要求：**
- Shell detect 对含可执行脚本的目录返回 true
- Procfile 解析 "web: node server.js" 提取启动命令
- 快照测试

---

### T5.7 注册表更新 + 全量快照测试

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch 附录 A, Arch§8.3 |
| **railpack 参考** | `rp:core/providers/provider.go`（GetLanguageProviders 函数） |
| **依赖** | T5.1, T5.2, T5.3, T5.4, T5.5, T5.6 |

**描述：** 注册所有 Provider 并完成全量快照测试。

**交付文件：**
- `src/provider/mod.rs`（更新）— `get_all_providers()` 按优先级顺序注册全部 7 个 Provider：Go → Java → Rust → Python → Node → StaticFile → Shell
- `tests/snapshot_tests.rs`（扩展）— 为每个新 Provider 的 fixture 添加快照测试
- `tests/integration_providers.rs` — Provider 端到端测试：对每种 fixture 调用 generate_build_plan() 验证完整流程
- `tests/fixtures/`（补充）— 确保每种语言有完整 fixture

**测试要求：**
- 全部 fixture 快照测试通过
- Provider 检测优先级正确（Go 优先于 Python，Node 优先于 StaticFile）
- `get_all_providers()` 返回顺序与 Arch 附录 A 一致
- integration_providers.rs 端到端测试通过

---

## Phase 5 Gate

**执行命令：**
```bash
cargo check
cargo test
cargo test -- snapshot                     # 全量快照测试
cargo insta review                         # 审查新增快照
./target/release/arcpack plan tests/fixtures/go-basic
./target/release/arcpack plan tests/fixtures/python-pip
./target/release/arcpack plan tests/fixtures/rust-basic
./target/release/arcpack plan tests/fixtures/java-maven
./target/release/arcpack plan tests/fixtures/staticfile
./target/release/arcpack plan tests/fixtures/with-procfile
cargo test -- --ignored                    # 集成测试（需 buildkitd 环境）
```

**验收清单：**
- [ ] `cargo check` 无错误无警告
- [ ] `cargo test` 全部通过（预计 200+ 个测试用例）
- [ ] 7 个 Provider 全部在 `get_all_providers()` 中注册，顺序正确
- [ ] 每个 Provider 有独立的 detect / plan 单元测试
- [ ] 每个 Provider 有对应 fixture 的 `insta` 快照测试
- [ ] `arcpack plan` 对每种语言/场景输出正确 BuildPlan
- [ ] 缓存键名和缓存目录与 railpack 对齐（参照 Arch 附录 B）
- [ ] Provider 检测优先级：语言特定 Provider 优先于通用 Provider（StaticFile / Shell 排最后）
- [ ] Procfile 启动命令提取正确
- [ ] 集成测试（`#[ignore]`）对多种语言 fixture 构建通过
