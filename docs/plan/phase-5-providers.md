# Phase 5: 更多 Provider

> [← 返回目录](./README.md) | 上一阶段：[← Phase 4](./phase-4-buildkit.md)

**目标：** 补齐 Go / Python / Rust / Java / StaticFile / Shell / Procfile 七大语言/场景的 Provider，覆盖主流应用类型。每个 Provider 需实现完整的跨领域关注点（APT 依赖、secrets 前缀过滤、metadata、环境变量覆盖）。

**前置条件：** Phase 2 全部完成（Provider 框架已就绪）。Phase 3/4 为运行时依赖，但 Provider 开发本身只依赖 Phase 2 的 GenerateContext + StepBuilder。

## 跨领域关注点（Cross-Cutting Concerns）

所有语言 Provider 必须处理以下共性关注点，与 railpack 对齐：

| 关注点 | 说明 | railpack 参考 |
|--------|------|--------------|
| **APT 构建依赖** | 根据依赖文件检测 C 扩展需求，添加 `-dev` 包到 mise step | Python: `libpq-dev`/`libcairo2-dev`；Go: `gcc`/`g++`/`libc6-dev` |
| **APT 运行时依赖** | deploy 阶段添加运行时库（非 `-dev` 版） | Python: `libpq5`/`libcairo2`/`poppler-utils`/`ffmpeg` |
| **Secrets 前缀过滤** | `use_secrets_with_prefixes()` 按语言过滤环境变量注入 | Python: `["PYTHON","PIP","PIPX","UV","PDM","POETRY"]` |
| **Metadata** | `ctx.metadata.set()` 记录检测结果，供 CLI info 命令展示 | `pythonPackageManager`/`goWorkspace`/`javaFramework` 等 |
| **环境变量覆盖** | `RAILPACK_{LANG}_VERSION` 覆盖语言版本 | `RAILPACK_GO_VERSION`/`RAILPACK_PYTHON_VERSION`/`RAILPACK_RUST_VERSION` 等 |
| **start_command_help()** | 当 deploy.start_cmd 为空时，输出帮助信息 | 所有 Provider 均实现 |
| **Config.deploy 输出** | Provider 控制 deploy 阶段的 inputs/paths/env_vars/apt_packages | 每个 Provider 需显式配置 |

## 任务依赖图

```
（Phase 2 完成后，以下任务可并行开发）

T5.1 (Go Provider)         ──┐
T5.2 (Python Provider)     ──┤
T5.3 (Rust Provider)       ──┤
T5.4 (Java Provider)       ──┤──► T5.8 (注册表更新 + 全量快照测试)
T5.5 (StaticFile Provider) ──┤
T5.6 (Shell Provider)      ──┤
T5.7 (Procfile Provider)   ──┘
```

> **重要：** T5.7 Procfile 是特殊的后处理 Provider，不在语言 Provider 检测列表中。
> 它始终在主 Provider plan() 之后独立运行，仅用于覆盖 start command。

## 任务列表

### T5.1 Go Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P0 |
| **设计文档** | Arch 附录 A, Arch 附录 B, Arch§6.2 |
| **railpack 参考** | `rp:core/providers/golang/golang.go`（275 行） |
| **依赖** | Phase 2 |

**描述：** 支持 go.mod / go.work 的 Go 项目构建，包括 workspace、CGO 检测、cmd/ 子目录解析。

**交付文件：**
- `src/provider/golang.rs` — GoProvider 完整实现

**检测逻辑（detect）：**
- `go.mod` 存在，**或** `go.work` 存在（workspace），**或** 根目录有 `main.go`

**版本解析（initialize）：**
- 优先级：`go.mod` 的 `go` 指令 → `RAILPACK_GO_VERSION` 环境变量 → `.tool-versions` → 默认 `1.25`

**构建计划（plan）：**

| Step | 名称 | 输入 | 行为 |
|------|------|------|------|
| mise | `packages:mise` | — | 安装 Go 运行时；若 `CGO_ENABLED=1` 则附加 APT 包 `gcc`/`g++`/`libc6-dev` |
| install | `install` | mise | 设置 `GOPATH=/go`、`GOBIN=/go/bin`，PATH 追加 `/go/bin`；复制 `go.mod`/`go.sum`（workspace 模式还复制 `go.work`/`go.work.sum` 及子模块的 go.mod/go.sum）；运行 `go mod download`；非 CGO 模式设 `CGO_ENABLED=0` |
| build | `build` | install + local | 构建命令决策树（见下方）；`-ldflags="-w -s"` 压缩二进制，输出到 `out` |

**Go 构建命令决策树（对齐 railpack）：**
1. `RAILPACK_GO_WORKSPACE_MODULE` 环境变量 → `go build ./指定模块`
2. `RAILPACK_GO_BIN` 环境变量 → `go build ./cmd/<name>`
3. 有 `go.mod` + 根目录有 `.go` 文件 → `go build`
4. 有 `cmd/*` 子目录 → `go build ./cmd/<第一个目录>`
5. 仅有 `go.mod` → `go build`
6. workspace 模式 → 扫描各 package 的 `main.go`
7. 根目录有 `main.go` → `go build main.go`

**Deploy 配置：**
- `start_cmd`: `./out`
- APT 运行时包：`tzdata`（始终）；CGO 模式追加 `libc6`
- deploy inputs: 仅 build 步骤输出（过滤到 `.`）

**缓存：**
- `go-build` → `/root/.cache/go-build`（shared）

**Metadata：** `goMod`、`goWorkspace`、`goRootFile`、`goGin`、`goCGO`（均为 bool）

**测试 fixture：**
- `tests/fixtures/go-basic/go.mod` + `main.go` — 基础项目
- `tests/fixtures/go-workspace/go.work` + 子模块 — workspace 项目
- `tests/fixtures/go-cgo/go.mod` + CGO 代码 — CGO 场景

**测试要求：**
- detect 对含/无 go.mod、go.work、main.go 三种标记的多向测试
- go.mod `go` 指令版本解析测试
- `RAILPACK_GO_VERSION` 环境变量覆盖测试
- 构建命令决策树各分支覆盖测试（至少覆盖分支 1/3/4/7）
- workspace 模式 go.work 复制逻辑测试
- CGO 检测 → APT 包添加测试（构建和运行时均覆盖）
- 缓存键名 `go-build` 对齐 railpack
- Metadata 设置正确性测试
- 快照测试

---

### T5.2 Python Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P0 |
| **设计文档** | Arch 附录 A, Arch 附录 B |
| **railpack 参考** | `rp:core/providers/python/python.go`（561 行）, `rp:core/providers/python/django.go` |
| **依赖** | Phase 2 |

**描述：** 支持 pip/poetry/uv/pipenv/pdm 五种包管理器，自动检测 Django/FastAPI/Flask/FastHTML 四种框架，自动处理 APT 构建/运行时依赖和 secrets 前缀过滤。

**交付文件：**
- `src/provider/python/mod.rs` — PythonProvider 核心
- `src/provider/python/package_manager.rs` — 五种包管理器的安装逻辑
- `src/provider/python/django.rs` — Django 检测 + WSGI 模块解析
- `src/provider/python/frameworks.rs` — FastAPI/Flask/FastHTML 检测

**检测逻辑（detect）：**
- 任一存在：`main.py` / `app.py` / `start.py` / `bot.py` / `hello.py` / `server.py`，**或** `requirements.txt`，**或** `pyproject.toml`，**或** `Pipfile`

**版本解析：**
- 优先级：`RAILPACK_PYTHON_VERSION` → `runtime.txt` → `Pipfile` 的 `python_version`/`python_full_version` → 默认 `3.13`
- mise 设置：`MISE_PYTHON_COMPILE=false`

**构建计划（plan）：**

| Step | 名称 | 输入 | 行为 |
|------|------|------|------|
| mise | `packages:mise` | — | 安装 Python；按包管理器安装工具（uv: `uv latest`；poetry: `pipx:poetry`；pdm: `pipx:pdm`；pipenv: `pipx:pipenv`）；检测 APT 构建依赖 |
| install | `install` | mise | 创建 venv，按包管理器分发安装命令（见下方）；secrets 前缀过滤 |
| build | `build` | install + local | uv 模式运行 `uv sync --locked --no-dev --no-editable`；其他模式无额外命令 |

**五种包管理器安装命令（对齐 railpack）：**

| 包管理器 | 触发条件 | 安装命令 | 缓存键 → 目录 |
|---------|---------|---------|--------------|
| pip | `requirements.txt` | `python -m venv .venv && pip install -r requirements.txt` | `pip` → `/opt/pip-cache` |
| uv | `pyproject.toml` + `uv.lock` | `uv sync --locked --no-dev --no-install-project` | `uv` → `/opt/uv-cache` |
| poetry | `pyproject.toml` + `poetry.lock` | `poetry install --no-interaction --no-ansi --only main --no-root` | — |
| pdm | `pyproject.toml` + `pdm.lock` | `pdm install --check --prod --no-editable` | — |
| pipenv | `Pipfile` | `pipenv install --deploy --ignore-pipfile`（有 lock）/ `--skip-lock`（无 lock） | — |

**Venv 配置（所有包管理器共用）：**
- `VIRTUAL_ENV=/app/.venv`
- PATH 追加 `/app/.venv/bin`

**Secrets 前缀过滤：**
- `use_secrets_with_prefixes(["PYTHON", "PIP", "PIPX", "UV", "PDM", "POETRY"])`

**APT 构建依赖检测（pythonBuildDepRequirements）：**

| 依赖库 | 条件 | APT 包 |
|--------|------|--------|
| pycairo | `pycairo` 在依赖中 | `libcairo2-dev` |
| psycopg2 | `psycopg2`/`psycopg`（非 `-binary`）在依赖中 | `libpq-dev` |
| mysqlclient | `mysqlclient` 在依赖中 | `default-libmysqlclient-dev` |

**APT 运行时依赖检测（pythonRuntimeDepRequirements）：**

| 依赖库 | APT 包 |
|--------|--------|
| pycairo | `libcairo2` |
| pdf2image | `poppler-utils` |
| pydub | `ffmpeg` |
| psycopg2（非 binary） | `libpq5` |
| mysqlclient | `default-mysql-client` |

**框架检测与 StartCmd：**

| 框架 | 检测条件 | StartCmd |
|------|---------|---------|
| Django | `manage.py` + `django` 在依赖中 | `python manage.py migrate && gunicorn --bind 0.0.0.0:${PORT:-8000} <wsgi_module>:application` |
| FastAPI | `fastapi` 在依赖中 + `uvicorn` 在依赖中 | `uvicorn main:app --host 0.0.0.0 --port ${PORT:-8000}` |
| FastHTML | `python-fasthtml` 在依赖中 + `uvicorn` 在依赖中 | `uvicorn main:app --host 0.0.0.0 --port ${PORT:-8000}` |
| Flask | `flask` 在依赖中 + `gunicorn` 在依赖中 | `gunicorn --bind 0.0.0.0:${PORT:-8000} main:app` |
| 回退 | 无框架匹配 | `python <入口文件>`（首个匹配的 main.py/app.py/...） |

**Django WSGI 模块解析（`django.rs`）：**
- `RAILPACK_DJANGO_APP_NAME` 环境变量 → 直接使用
- 扫描 `**/*.py` 匹配 `WSGI_APPLICATION = "xxx.application"` 正则

**Deploy 环境变量（始终设置）：**
```
PYTHONFAULTHANDLER=1, PYTHONUNBUFFERED=1, PYTHONHASHSEED=random,
PYTHONDONTWRITEBYTECODE=1, PIP_DISABLE_PIP_VERSION_CHECK=1, PIP_DEFAULT_TIMEOUT=100
```

**Deploy inputs：** mise 层 + venv（过滤）+ 源码（排除 `.venv`）

**Metadata：** `pythonPackageManager`（pip/poetry/pdm/uv/pipenv）, `pythonRuntime`（django/flask/fastapi/fasthtml/python）

**install 阶段文件复制优化：**
- 默认只复制 lock/manifest 文件
- 若检测到本地路径引用（requirements.txt 中的 `-e ./local_pkg`，pyproject.toml 中的 `uv.workspace`），则复制全部文件

**测试 fixture：**
- `tests/fixtures/python-pip/requirements.txt`
- `tests/fixtures/python-poetry/pyproject.toml` + `poetry.lock`
- `tests/fixtures/python-uv/pyproject.toml` + `uv.lock`
- `tests/fixtures/python-pdm/pyproject.toml` + `pdm.lock`
- `tests/fixtures/python-pipenv/Pipfile` + `Pipfile.lock`
- `tests/fixtures/python-django/manage.py` + `requirements.txt`（含 django）

**测试要求：**
- detect 对各种 Python 标记文件的覆盖测试
- 五种包管理器的安装命令和缓存路径正确性测试
- venv 配置（VIRTUAL_ENV、PATH）对所有包管理器一致
- secrets 前缀过滤测试（正确过滤、不泄漏无关 secrets）
- APT 构建依赖检测测试（pycairo→libcairo2-dev、psycopg2→libpq-dev）
- APT 运行时依赖检测测试
- Django 检测 + WSGI 模块解析测试
- FastAPI/Flask 框架检测 → 正确 StartCmd 测试
- Deploy 环境变量完整性测试
- `RAILPACK_PYTHON_VERSION` 覆盖测试
- install 阶段文件复制优化测试（本地路径引用检测）
- Metadata 设置正确性测试
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

**描述：** 支持 Cargo.toml 检测 + cargo build --release，包括 workspace 支持、WASM 目标检测、依赖预编译优化、多种版本来源。

**交付文件：**
- `src/provider/rust_lang.rs` — RustProvider 完整实现

**检测逻辑（detect）：** `Cargo.toml` 存在

**版本解析（优先级顺序）：**
1. `Cargo.toml` `edition` 字段 → 最低版本映射（2015→1.30, 2018→1.55, 2021→1.84, 2024→1.85.1）
2. `RAILPACK_RUST_VERSION` 环境变量
3. `rust-version.txt` 或 `.rust-version` 文件
4. `Cargo.toml` `package.rust-version` 字段
5. `rust-toolchain.toml` 的 `toolchain.channel`
6. `rust-toolchain` 文件（纯文本 channel）
7. 默认 `1.89`

**构建计划（plan）：**

| Step | 名称 | 输入 | 行为 |
|------|------|------|------|
| mise | `packages:mise` | — | 安装 Rust 运行时 |
| install | `install` | mise | 复制 `Cargo.toml*`、`Cargo.lock*`；**依赖预编译**（见下方） |
| build | `build` | mise + install（排除 `/app/`）+ local | 编译项目，复制二进制到 `bin/` |

**依赖预编译优化（install 步骤 — 对齐 railpack）：**
- **非 workspace 项目：** 注入空 `src/main.rs`（`fn main() { }`）作为 dummy 源文件；若 `Cargo.toml` 含 `[lib]` 则追加空 `src/lib.rs`；运行 `cargo build --release` 仅编译依赖；然后清理 `src/` 和 target 中的 app 二进制，强制后续 build 步骤重新编译真实源码
- **workspace 项目：** 跳过依赖预编译

**Build 步骤命令：**
- 非 workspace：`cargo build --release`，然后 `cp` 各二进制到 `bin/`
- workspace：`cargo build --release --package <workspace-binary>`，然后 `cp` 到 `bin/`
- WASM 目标：检测 `.cargo/config.toml` 中 `target = "wasm32-wasi"` → 添加 `--target wasm32-wasi`，复制 `.wasm` 文件

**StartCmd 解析顺序：**
1. `RAILPACK_CARGO_WORKSPACE` 环境变量 → `./bin/<name>`
2. workspace 检测 → `./bin/<workspace-binary>`
3. 单二进制 → `./bin/<name>`
4. 多二进制 + `RAILPACK_RUST_BIN` → `./bin/<matching-name>`
5. `package.default-run` 字段
6. WASM 目标：后缀 `.wasm`

**Deploy 配置：**
- 环境变量：`ROCKET_ADDRESS=0.0.0.0`
- deploy inputs: build 步骤过滤到 `.`，排除 `target/`

**缓存：**
- `cargo_registry` → `/root/.cargo/registry`（shared）
- `cargo_git` → `/root/.cargo/git`（shared）
- `cargo_target` → `target`（relative，**locked**）

**测试 fixture：**
- `tests/fixtures/rust-basic/Cargo.toml` + `src/main.rs` — 基础项目
- `tests/fixtures/rust-workspace/Cargo.toml` + 子 crate — workspace 项目

**测试要求：**
- detect 对含/无 Cargo.toml 的双向测试
- 版本解析优先级测试（edition 映射、rust-toolchain.toml、环境变量覆盖）
- 依赖预编译逻辑测试（dummy main.rs 注入 + 清理）
- workspace 项目跳过预编译测试
- 二进制名称从 Cargo.toml 正确提取
- StartCmd 解析顺序各分支测试
- `cargo_registry` / `cargo_git` 为 shared，`cargo_target` 为 locked
- WASM 目标检测测试
- Deploy 环境变量 `ROCKET_ADDRESS` 测试
- 快照测试

---

### T5.4 Java Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **设计文档** | Arch 附录 A, Arch 附录 B |
| **railpack 参考** | `rp:core/providers/java/java.go`, `rp:core/providers/java/maven.go`, `rp:core/providers/java/gradle.go`, `rp:core/providers/java/jdk.go` |
| **依赖** | Phase 2 |

**描述：** Maven/Gradle 双路径检测，支持 wrapper（mvnw/gradlew）、Spring Boot 检测、运行时 JDK 分离。

**交付文件：**
- `src/provider/java/mod.rs` — JavaProvider 核心
- `src/provider/java/maven.rs` — Maven 构建路径
- `src/provider/java/gradle.rs` — Gradle 构建路径

**检测逻辑（detect）：**
- `pom.{xml,atom,clj,groovy,rb,scala,yaml,yml}` glob 匹配，**或** `gradlew` 存在

**版本解析：**
- JDK 默认：`21`
- `RAILPACK_JDK_VERSION` 环境变量覆盖
- Gradle 版本 ≤ 5 时强制 JDK 8

**构建计划（plan）：**

| Step | 名称 | 输入 | 行为 |
|------|------|------|------|
| mise（构建） | `packages:mise` | — | 安装 JDK + 构建工具（Maven 或 Gradle） |
| build | `build` | mise + local | 运行构建命令（见下方） |
| mise（运行时） | `packages:mise:runtime` | — | **单独的运行时 JDK 步骤**（不含构建工具） |

**Gradle 路径：**
- Gradle 默认版本：`8`；`RAILPACK_GRADLE_VERSION` 覆盖
- 读取 `gradle/wrapper/gradle-wrapper.properties` 提取版本（正则匹配）
- `gradlew` 设为可执行（若非已可执行）
- 构建命令：`./gradlew clean build -x check -x test -Pproduction`
- 缓存：`gradle` → `/root/.gradle`

**Maven 路径：**
- mise 安装 `maven latest`
- 若 `mvnw` + `.mvn/wrapper/maven-wrapper.properties` 存在则使用 `./mvnw`，否则 `mvn`
- `mvnw` 设为可执行
- 构建命令：`mvn -DoutputFile=target/mvn-dependency-list.log -B -DskipTests clean dependency:list install -Pproduction`
- 缓存：`maven` → `.m2/repository`

**Spring Boot 检测：** 检查 `**/spring-boot*.jar`、`**/spring-boot*.class`、`**/org/springframework/boot/**` glob 匹配

**Deploy 配置：**
- deploy inputs: 运行时 mise 层 + build 输出（Maven: `target/.`；Gradle: `.`）
- StartCmd 按条件选择：

| 条件 | StartCmd |
|------|---------|
| Gradle + Spring Boot | `java $JAVA_OPTS -Dserver.port=$PORT -jar $(ls -1 */build/libs/*jar \| grep -v plain)` |
| Gradle | `java $JAVA_OPTS -jar $(ls -1 */build/libs/*jar \| grep -v plain)` |
| Maven + WildFly Swarm | `java -Dswarm.http.port=$PORT $JAVA_OPTS -jar target/*jar` |
| Maven + Spring Boot | `java -Dserver.port=$PORT $JAVA_OPTS -jar target/*jar` |
| Maven | `java $JAVA_OPTS -jar target/*jar` |

**Metadata：** `javaPackageManager`（gradle/maven）, `javaFramework`（spring-boot 或空）

**测试 fixture：**
- `tests/fixtures/java-maven/pom.xml` + `src/main/java/` — Maven 项目
- `tests/fixtures/java-gradle/gradlew` + `build.gradle` — Gradle 项目

**测试要求：**
- Maven/Gradle 双路径检测测试
- pom.xml 各种扩展名匹配测试
- Gradle wrapper 版本解析测试
- Maven wrapper 检测测试（mvnw 存在/不存在）
- Spring Boot 检测测试
- StartCmd 按条件选择测试（覆盖 Gradle+SpringBoot / Maven / Maven+SpringBoot）
- 运行时 JDK 分离测试（`packages:mise:runtime` 不含构建工具）
- `RAILPACK_JDK_VERSION` 覆盖测试
- Gradle 低版本强制 JDK 8 测试
- Metadata 设置正确性测试
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

**描述：** 纯静态网站，使用 Caddy 作为文件服务器。支持多种根目录检测方式和 Caddyfile 模板。

> **注意：** 此 Provider 独立于 Node SPA 部署（Node Provider 内的框架检测也使用 Caddy，但属于 Node Provider 范围）。

**交付文件：**
- `src/provider/staticfile.rs` — StaticFileProvider 完整实现

**检测逻辑 + 根目录解析（detect 和 initialize 共用逻辑）：**
1. `RAILPACK_STATIC_FILE_ROOT` 环境变量 → 使用指定目录
2. `Staticfile` 文件存在 → 解析 YAML `root:` 键
3. `public/` 目录存在 → 使用 `public/`
4. `index.html` 在根目录存在 → 使用 `.`
5. 以上均不匹配 → detect 返回 false

**构建计划（plan）：**

| Step | 名称 | 输入 | 行为 |
|------|------|------|------|
| mise | `packages:mise` | — | 安装 `caddy latest` |
| build | `build` | mise + local | 写入 Caddyfile（从嵌入模板或用户的 `Caddyfile`/`Caddyfile.template`）；运行 `caddy fmt --overwrite Caddyfile` |

**Caddyfile 模板数据：** `STATIC_FILE_ROOT` → 解析后的根目录

**Deploy 配置：**
- `start_cmd`: `caddy run --config Caddyfile --adapter caddyfile 2>&1`
- deploy inputs: mise 层 + build 步骤全部输出

**测试 fixture：**
- `tests/fixtures/staticfile/index.html` — 基础静态网站
- `tests/fixtures/staticfile-yaml/Staticfile` + `dist/index.html` — YAML 配置指定根目录
- `tests/fixtures/staticfile-public/public/index.html` — public/ 目录

**测试要求：**
- detect 四种触发路径覆盖测试（环境变量 / Staticfile YAML / public/ / index.html）
- detect 全不匹配返回 false 测试
- 仅有 index.html 返回 true，同时有 package.json 返回 false（被 Node Provider 优先匹配）
- Staticfile YAML `root:` 解析测试
- Caddyfile 模板内容正确性测试（STATIC_FILE_ROOT 替换）
- 用户自定义 Caddyfile 优先于模板测试
- 快照测试

---

### T5.6 Shell Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P2 |
| **设计文档** | Arch 附录 A |
| **railpack 参考** | `rp:core/providers/shell/shell.go` |
| **依赖** | Phase 2 |

**描述：** 可执行脚本支持，自动检测 shebang 确定解释器。

**交付文件：**
- `src/provider/shell.rs` — ShellProvider 完整实现

**检测逻辑（detect）：**
- `start.sh` 存在，**或** `RAILPACK_SHELL_SCRIPT` 环境变量指定的文件存在

**Initialize：** 存储脚本名称。若 `RAILPACK_SHELL_SCRIPT` 指定的文件不存在则报错。

**构建计划（plan）：**

| Step | 名称 | 输入 | 行为 |
|------|------|------|------|
| mise | `packages:mise` | — | 仅安装 `RAILPACK_PACKAGES` 配置的包（无语言特定包） |
| install | `install`（可选） | mise + local | 仅在 `RAILPACK_INSTALL_CMD` 配置时创建 |
| build | `build` | install/mise + local | `chmod +x <script>` |

**Shebang 解释器检测：**

| Shebang | 解释器 | 运行时 APT 包 |
|---------|--------|-------------|
| `#!/bin/bash` / `#!/usr/bin/env bash` | `bash` | — |
| `#!/bin/zsh` / `#!/usr/bin/env zsh` | `zsh` | `zsh` |
| `#!/bin/sh` / `#!/usr/bin/env sh` | `sh` | — |
| `#!/bin/dash` | `sh` | — |
| `#!/bin/mksh` / `#!/bin/ksh` / `#!/bin/fish` | `bash`（回退 + 警告） | — |
| 无 shebang | `sh` | — |

**Deploy 配置：**
- `start_cmd`: `<解释器> <脚本名>`
- deploy inputs: mise 层 + build 步骤输出

**Metadata：** `detectedShellInterpreter`

**测试 fixture：**
- `tests/fixtures/shell-basic/start.sh` — bash 脚本
- `tests/fixtures/shell-custom/run.sh` — 配合 `RAILPACK_SHELL_SCRIPT=run.sh`

**测试要求：**
- detect 对 start.sh 存在/不存在的双向测试
- `RAILPACK_SHELL_SCRIPT` 环境变量指定自定义脚本测试
- `RAILPACK_SHELL_SCRIPT` 指定不存在文件报错测试
- shebang 解释器检测测试（覆盖 bash/zsh/sh/无 shebang）
- zsh 脚本追加 APT 包测试
- install 步骤条件创建测试
- Metadata `detectedShellInterpreter` 测试
- 快照测试

---

### T5.7 Procfile Provider（特殊后处理 Provider）

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **设计文档** | Arch 附录 A |
| **railpack 参考** | `rp:core/providers/procfile/procfile.go`, `rp:core/core.go`（步骤 5：始终调用 procfile） |
| **依赖** | Phase 2 |

**描述：** Procfile 是特殊的后处理 Provider，**不在语言 Provider 检测注册表中**。它始终在主 Provider `plan()` 之后独立运行，仅用于覆盖 start command。

> **架构要点（对齐 railpack `core.go` 第 5 步）：**
> ```
> // 主 Provider plan() 之后
> procfileProvider.Plan(ctx)  // 始终调用，无条件
> ```
> Procfile 的 `detect()` 方法不会被 Provider 注册表调用。其内部 plan() 自行检查 Procfile 是否存在，不存在则直接返回 Ok(())。

**交付文件：**
- `src/provider/procfile.rs` — ProcfileProvider

**行为：**
- 读取 `Procfile`（YAML 格式解析）
- 按优先级选择进程：`web` > `worker` > 任意第一个 key
- 设置 `ctx.deploy.set_start_cmd(命令)`
- 若项目无 Procfile 则 plan() 直接返回 Ok(())，不做任何修改

**在 `src/lib.rs` 中的调用位置：**
```rust
// generate_build_plan() 内部：
// ... provider.plan(&mut ctx, &app)?;  // 主 Provider
// 始终在主 Provider 之后调用 Procfile
let procfile = ProcfileProvider::new();
procfile.plan(&mut ctx, &app)?;
// ... ctx.generate()?;
```

**测试 fixture：**
- `tests/fixtures/with-procfile/Procfile` — 含 web 进程
- `tests/fixtures/procfile-worker/Procfile` — 无 web，有 worker 进程

**测试要求：**
- Procfile YAML 解析测试（`web: node server.js` 提取命令）
- 优先级测试：web > worker > 任意
- 无 Procfile 时 plan() 无副作用测试
- Procfile 覆盖已有 start_cmd 测试
- **Procfile 不出现在 `get_all_providers()` 返回列表中**的测试
- 快照测试

---

### T5.8 注册表更新 + 全量快照测试

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **设计文档** | Arch 附录 A, Arch§8.3 |
| **railpack 参考** | `rp:core/providers/provider.go`（GetLanguageProviders、GetProvider 函数） |
| **依赖** | T5.1, T5.2, T5.3, T5.4, T5.5, T5.6, T5.7 |

**描述：** 注册所有 Provider 并完成全量快照测试。提供 `get_provider(name)` 按名查找功能。

**交付文件：**
- `src/provider/mod.rs`（更新）— `get_all_providers()` 和 `get_provider(name)`
- `tests/snapshot_tests.rs`（扩展）— 为每个新 Provider 的 fixture 添加快照测试
- `tests/integration_providers.rs` — Provider 端到端测试

**`get_all_providers()` 注册顺序（对齐 railpack 优先级）：**
```rust
// 顺序决定 detect 优先级（first match wins）
vec![
    Box::new(GoProvider::new()),
    Box::new(JavaProvider::new()),
    Box::new(RustProvider::new()),
    Box::new(PythonProvider::new()),
    Box::new(NodeProvider::new()),       // Phase 2 已实现
    Box::new(StaticFileProvider::new()),
    Box::new(ShellProvider::new()),
]
// 注意：ProcfileProvider 不在此列表中
```

> **与 railpack 的已知差异：** railpack 注册了 14 个语言 Provider（php → golang → java → rust → ruby → elixir → python → deno → dotnet → node → gleam → cpp → staticfile → shell）。arcpack Phase 5 实现 7 个，省略了 PHP / Ruby / Elixir / Deno / .NET / Gleam / C++。后续可按需补充。

**`get_provider(name)` 函数：**
```rust
pub fn get_provider(name: &str) -> Option<Box<dyn Provider>> {
    get_all_providers().into_iter().find(|p| p.name() == name)
}
```

**全量快照测试 fixture 清单：**

| 语言/场景 | Fixture 路径 |
|-----------|-------------|
| Go（基础） | `tests/fixtures/go-basic/` |
| Go（workspace） | `tests/fixtures/go-workspace/` |
| Python（pip） | `tests/fixtures/python-pip/` |
| Python（uv） | `tests/fixtures/python-uv/` |
| Python（poetry） | `tests/fixtures/python-poetry/` |
| Python（Django） | `tests/fixtures/python-django/` |
| Rust（基础） | `tests/fixtures/rust-basic/` |
| Rust（workspace） | `tests/fixtures/rust-workspace/` |
| Java（Maven） | `tests/fixtures/java-maven/` |
| Java（Gradle） | `tests/fixtures/java-gradle/` |
| StaticFile | `tests/fixtures/staticfile/` |
| Shell | `tests/fixtures/shell-basic/` |
| Procfile | `tests/fixtures/with-procfile/` |
| Node（npm）（Phase 2 已有） | `tests/fixtures/node-npm/` |
| Node（pnpm）（Phase 2 已有） | `tests/fixtures/node-pnpm/` |

**测试要求：**
- 全部 fixture 快照测试通过
- Provider 检测优先级正确（Go/Java/Rust/Python 优先于 Node，Node 优先于 StaticFile/Shell）
- `get_all_providers()` 返回顺序正确
- `get_provider("golang")` / `get_provider("python")` 等按名查找正确
- `get_provider("procfile")` 返回 None（Procfile 不在注册表中）
- integration_providers.rs 端到端测试：对每种 fixture 调用 `generate_build_plan()` 验证完整流程
- 所有 Provider 的 `start_command_help()` 返回非空帮助信息
- 所有 Provider 的 `cleanse_plan()` 不 panic

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
./target/release/arcpack plan tests/fixtures/python-uv
./target/release/arcpack plan tests/fixtures/rust-basic
./target/release/arcpack plan tests/fixtures/java-maven
./target/release/arcpack plan tests/fixtures/staticfile
./target/release/arcpack plan tests/fixtures/shell-basic
./target/release/arcpack plan tests/fixtures/with-procfile
cargo test -- --ignored                    # 集成测试（需 buildkitd 环境）
```

**验收清单：**
- [ ] `cargo check` 无错误无警告
- [ ] `cargo test` 全部通过（预计 200+ 个测试用例）
- [ ] 7 个语言 Provider 全部在 `get_all_providers()` 中注册，顺序正确
- [ ] ProcfileProvider 不在 `get_all_providers()` 中，但在 `generate_build_plan()` 中始终被调用
- [ ] 每个 Provider 有独立的 detect / plan 单元测试
- [ ] 每个 Provider 有对应 fixture 的 `insta` 快照测试
- [ ] `arcpack plan` 对每种语言/场景输出正确 BuildPlan
- [ ] 缓存键名和缓存目录与 railpack 对齐（参照 Arch 附录 B）
- [ ] Provider 检测优先级：语言特定 Provider 优先于通用 Provider（StaticFile / Shell 排最后）
- [ ] Procfile 始终在主 Provider 之后运行，覆盖 start command 正确
- [ ] Python 支持 5 种包管理器（pip/poetry/uv/pdm/pipenv）
- [ ] Python APT 构建/运行时依赖检测正确
- [ ] Go workspace / CGO 检测正确
- [ ] Rust 依赖预编译优化正确
- [ ] Java 双路径（Maven/Gradle）+ wrapper 支持正确
- [ ] 所有 Provider 的 `start_command_help()` 返回有意义的帮助信息
- [ ] 所有 Provider 设置了对应的 Metadata
- [ ] 集成测试（`#[ignore]`）对多种语言 fixture 构建通过

---

## 与 railpack 的已知差异

| 项目 | railpack | arcpack Phase 5 | 原因 |
|------|---------|-----------------|------|
| Provider 数量 | 14 个语言 + Procfile | 7 个语言 + Procfile | 优先覆盖主流语言，后续按需补充 |
| 缺失 Provider | PHP / Ruby / Elixir / Deno / .NET / Gleam / C++ | — | Phase 5+ 按社区需求逐步添加 |
| PHP 优先级 | 最高（第 1 位） | 未实现 | PHP 最复杂（FrankenPHP + Composer + Laravel + 可选 Node），建议单独 Phase |
| Node SPA 部署 | Node Provider 内的 DeploySPA | Phase 2 已记录为后续迭代项 | 不影响 Phase 5 |

---

## 修正日志

本文档经过对照 railpack 源码的系统性审查和修正，涵盖以下关键变更：

| 编号 | 级别 | 任务 | 修正内容 |
|------|------|------|---------|
| C1 | Critical | T5.7 | **Procfile 架构修正**：从 Shell+Procfile 合并任务拆分为独立任务，明确 Procfile 不在语言 Provider 注册表中，始终作为后处理运行 |
| C2 | Critical | T5.2 | **Python 包管理器扩展**：从 pip/poetry 两种扩展为 pip/poetry/uv/pdm/pipenv 五种，对齐 railpack |
| C3 | Critical | T5.2 | **Python 框架检测**：新增 FastAPI/Flask/FastHTML 框架自动检测和对应 StartCmd |
| I1 | Important | T5.1 | **Go 增强**：新增 go.work workspace 支持、CGO 检测、cmd/ 子目录构建命令决策树、GOPATH/GOBIN 环境变量 |
| I2 | Important | T5.2 | **Python APT 依赖**：新增构建/运行时 APT 自动检测（pycairo/psycopg2/mysqlclient 等）|
| I3 | Important | T5.2 | **Python Secrets**：新增 `use_secrets_with_prefixes()` 前缀过滤 |
| I4 | Important | T5.3 | **Rust 增强**：新增 7 级版本解析优先级、依赖预编译优化、workspace/WASM 支持、多种 StartCmd 解析路径 |
| I5 | Important | T5.4 | **Java 增强**：新增 wrapper 支持、Spring Boot 检测、运行时 JDK 分离（`packages:mise:runtime`）、Gradle 版本解析 |
| I6 | Important | T5.5 | **StaticFile 增强**：新增 4 种根目录检测方式、Staticfile YAML 解析、Caddyfile 模板机制 |
| I7 | Important | T5.6 | **Shell 增强**：新增 shebang 解释器检测、`RAILPACK_SHELL_SCRIPT` 环境变量、条件 install 步骤 |
| I8 | Important | T5.8 | **注册表增强**：新增 `get_provider(name)` 函数，明确 Procfile 不在注册表中 |
| S1 | Suggestion | ALL | **跨领域关注点**：新增文档顶部的 Cross-Cutting Concerns 表格，明确所有 Provider 必须实现的共性关注点 |
| S2 | Suggestion | ALL | **Metadata 补齐**：每个 Provider 明确了应设置的 Metadata 字段 |
| S3 | Suggestion | T5.8 | **已知差异表**：新增与 railpack 的差异对照表，明确缺失 Provider 和后续计划 |

**验证参考源文件：**
`core/providers/provider.go`, `core/providers/procfile/procfile.go`, `core/providers/golang/golang.go`, `core/providers/python/python.go`, `core/providers/python/django.go`, `core/providers/rust/rust.go`, `core/providers/java/java.go`, `core/providers/java/maven.go`, `core/providers/java/gradle.go`, `core/providers/staticfile/staticfile.go`, `core/providers/shell/shell.go`, `core/core.go`
