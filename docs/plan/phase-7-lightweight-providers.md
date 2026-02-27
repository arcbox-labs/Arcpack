# Phase 7: 轻量级 Provider（Deno / Gleam / C++）

> [← 返回目录](./README.md) | 上一阶段：[← Phase 6](./phase-6-integration-tests-node-deep.md) | 下一阶段：[Phase 8 →](./phase-8-ruby-elixir.md)

**目标：** 新增 3 个低复杂度 Provider（Deno / Gleam / C++），每个均含集成测试。

**前置条件：** Phase 6 T6.1（集成测试框架已就绪）+ Phase 5（Provider 框架已稳定）

## 任务依赖图

```
（以下任务可并行开发）

T7.1 (Deno Provider)    ──┐
T7.2 (Gleam Provider)   ──┤──► T7.4 (注册表更新 + 测试)
T7.3 (C++ Provider)     ──┘
```

## 任务列表

### T7.1 Deno Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **railpack 参考** | `rp:core/providers/deno/deno.go`（1 文件，约 120 行，LOW 复杂度） |
| **依赖** | Phase 2 |

**描述：** 支持 Deno 项目的检测和构建，通过 mise 安装 Deno 运行时。

**交付文件：** `src/provider/deno.rs`

**检测逻辑（detect）：**
- `deno.json` 存在 **或** `deno.jsonc` 存在

**版本解析（initialize）：**
- 优先级：`ARCPACK_DENO_VERSION` 环境变量 → mise `.tool-versions` → 默认 `2`

**构建计划（plan）：**

| Step | 名称 | 输入 | 行为 |
|------|------|------|------|
| mise | `packages:mise` | — | 安装 `deno` 运行时 |
| build | `build` | mise + local | `deno cache {main_file}` 预编译依赖；复制完整本地上下文 |

**Main file 发现逻辑（对齐 railpack）：**

按优先级依次查找：
1. `main.ts`
2. `main.js`
3. `main.mjs`
4. `main.mts`
5. 首个匹配的 `**/*.ts` 文件
6. 首个匹配的 `**/*.js` 文件

若无法找到任何匹配文件，报错。

**Deploy 配置：**
- `start_cmd`: `deno run --allow-all {main_file}`
- deploy inputs: mise 二进制层 + 完整源码
- 环境变量：无额外设置

**缓存：**
- `deno-cache` → `/root/.cache/deno`（shared）

**Metadata：** `denoVersion`、`denoMainFile`

**Secrets 前缀：** `DENO`

**start_command_help()：**
```
No main file found. Create a main.ts or main.js file, or set the entry point manually.
```

**测试 fixture：**
- `tests/fixtures/deno-basic/`
  - `deno.json`: `{}`
  - `main.ts`: `console.log("Hello from Deno");`
  - `test.json`: `[{"expectedOutput": "Hello from Deno"}]`

**测试要求：**
- detect 对含/无 deno.json、deno.jsonc 的测试
- main file 发现逻辑各优先级测试
- `ARCPACK_DENO_VERSION` 环境变量覆盖测试
- start_cmd 正确性测试
- 缓存键名 `deno-cache` 对齐 railpack
- 快照测试

---

### T7.2 Gleam Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **railpack 参考** | `rp:core/providers/gleam/gleam.go`（1 文件，约 100 行，LOW 复杂度） |
| **依赖** | Phase 2 |

**描述：** 支持 Gleam 项目的检测和构建，通过 mise 安装 Gleam 和 Erlang。

**交付文件：** `src/provider/gleam.rs`

**检测逻辑（detect）：**
- `gleam.toml` 存在

**版本解析（initialize）：**
- Gleam 版本：`ARCPACK_GLEAM_VERSION` → mise `.tool-versions` → 默认 `latest`
- Erlang 版本：`ARCPACK_ERLANG_VERSION` → mise `.tool-versions` → 默认 `latest`

**构建计划（plan）：**

| Step | 名称 | 输入 | 行为 |
|------|------|------|------|
| mise (build) | `packages:mise` | — | 安装 `gleam` + `erlang`（构建时都需要） |
| build | `build` | mise + local | `gleam export erlang-shipment` → 输出 `build/erlang-shipment/` |
| mise (runtime) | `packages:mise:runtime` | — | 仅安装 `erlang`（运行时不需要 gleam 编译器） |

**关键设计：两个 mise 步骤**
- 构建时 mise step：安装 `gleam` + `erlang`（gleam 编译需要两者）
- 运行时 mise step：名为 `packages:mise:runtime`，仅安装 `erlang`（Erlang shipment 只需要 erlang VM）
- deploy inputs 引用运行时 mise step 而非构建时 mise step

**Deploy 配置：**
- `start_cmd`: `./build/erlang-shipment/entrypoint.sh run`
- deploy inputs: 运行时 mise 层（仅 erlang）+ `build/erlang-shipment/` 目录
- 环境变量：无额外设置

**GLEAM_INCLUDE_SOURCE 环境变量：**
- 若 `GLEAM_INCLUDE_SOURCE` 为 truthy 值，deploy inputs 额外包含完整源码
- 默认不包含源码（Erlang shipment 已自包含）

**Metadata：** `gleamVersion`、`erlangVersion`

**Secrets 前缀：** `GLEAM`、`HEX`

**测试 fixture：**
- `tests/fixtures/gleam-basic/`
  - `gleam.toml`: `[project]\nname = "hello"\nversion = "1.0.0"`
  - `src/hello.gleam`: 基础 Hello World
  - `test.json`: `[{"expectedOutput": "Hello from Gleam"}]`

**测试要求：**
- detect 对含/无 gleam.toml 的测试
- 版本解析测试（环境变量覆盖）
- 两个 mise 步骤正确性测试（构建 gleam+erlang，运行时仅 erlang）
- `GLEAM_INCLUDE_SOURCE` 条件包含源码测试
- start_cmd 正确性测试
- 快照测试

---

### T7.3 C++ Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **railpack 参考** | `rp:core/providers/cpp/`（4 文件，约 350 行总计，LOW-MEDIUM 复杂度） |
| **依赖** | Phase 2 |

**描述：** 支持 CMake 和 Meson 两种构建系统的 C++ 项目。

**交付文件：**
- `src/provider/cpp/mod.rs` — CppProvider 核心 + detect + plan
- `src/provider/cpp/cmake.rs` — CMake 构建逻辑
- `src/provider/cpp/meson.rs` — Meson 构建逻辑

**检测逻辑（detect）：**
- `CMakeLists.txt` 存在 → CMake 模式
- `meson.build` 存在 → Meson 模式
- 两者都存在 → CMake 优先

**版本解析（initialize）：**
- CMake 版本：`ARCPACK_CMAKE_VERSION` → 默认 `latest`
- Meson 版本：`ARCPACK_MESON_VERSION` → 默认 `latest`
- Ninja 版本：默认 `latest`（始终安装）

**核心类型：**

```rust
pub struct CppProvider {
    build_system: BuildSystem,
}

enum BuildSystem {
    Cmake,
    Meson,
}
```

**CMake 构建计划：**

| Step | 名称 | 输入 | 行为 |
|------|------|------|------|
| mise | `packages:mise` | — | 安装 `cmake` + `ninja`；APT 包 `build-essential`、`pkg-config` |
| build | `build` | mise + local | `cmake -B /build -GNinja /app` → `cmake --build /build` |

**Meson 构建计划：**

| Step | 名称 | 输入 | 行为 |
|------|------|------|------|
| mise | `packages:mise` | — | 安装 `meson` + `ninja`；APT 包 `build-essential`、`pkg-config`、`python3` |
| build | `build` | mise + local | `meson setup /build` → `meson compile -C /build` |

**Deploy 配置（两种模式通用）：**
- `start_cmd`: `/build/{project_dir_basename}`
  - `project_dir_basename` 是项目目录名（如 `/app/myproject` → `myproject`）
  - CMake 的实际可执行文件名可能与目录名不同，此处使用约定
- deploy inputs: 仅 `/build` 目录的构建产物
- APT 运行时包：`libstdc++6`

**环境变量：**
- `ARCPACK_CPP_BUILD_DIR`: 覆盖构建输出目录（默认 `/build`）
- `ARCPACK_CPP_BINARY_NAME`: 覆盖可执行文件名

**缓存：**
- `cpp-build` → `/build`（locked — C++ 增量编译缓存）

**Metadata：** `cppBuildSystem`（`cmake` 或 `meson`）

**start_command_help()：**
```
Could not determine the binary name. Set ARCPACK_CPP_BINARY_NAME or ensure your project directory name matches the output binary.
```

**测试 fixture：**
- `tests/fixtures/cpp-cmake/`
  - `CMakeLists.txt`:
    ```cmake
    cmake_minimum_required(VERSION 3.10)
    project(hello)
    add_executable(hello main.cpp)
    ```
  - `main.cpp`: `#include <iostream>\nint main() { std::cout << "Hello from C++"; }`
  - `test.json`: `[{"expectedOutput": "Hello from C++"}]`
- `tests/fixtures/cpp-meson/`
  - `meson.build`:
    ```meson
    project('hello', 'cpp')
    executable('hello', 'main.cpp')
    ```
  - `main.cpp`: 同上
  - `test.json`: `[{"expectedOutput": "Hello from C++"}]`

**测试要求：**
- detect 对 CMakeLists.txt / meson.build / 两者都有的测试
- CMake 构建命令正确性测试
- Meson 构建命令正确性测试
- `ARCPACK_CPP_BINARY_NAME` 覆盖测试
- 缓存键名 `cpp-build` 测试
- 快照测试（cmake + meson 各一份）

---

### T7.4 注册表更新 + 测试

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **依赖** | T7.1 ~ T7.3 |

**描述：** 将 Deno / Gleam / C++ 加入 `get_all_providers()` 注册表，更新全量快照测试。

**注册顺序更新（对齐 railpack）：**

```rust
pub fn get_all_providers() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(GoProvider::new()),
        Box::new(JavaProvider::new()),
        Box::new(RustProvider::new()),
        Box::new(PythonProvider::new()),
        Box::new(DenoProvider::new()),        // NEW
        Box::new(NodeProvider::new()),
        Box::new(GleamProvider::new()),       // NEW
        Box::new(CppProvider::new()),         // NEW
        Box::new(StaticFileProvider::new()),
        Box::new(ShellProvider::new()),
    ]
}
```

> **注册顺序说明：** 检测优先级从上到下。Deno 在 Node.js 之前（若项目有 `deno.json`，优先检测为 Deno）。Gleam 和 C++ 在 Node.js 之后（这些语言的检测标记不会与 Node.js 冲突）。

**快照测试更新：**
- 新增 `deno-basic`、`gleam-basic`、`cpp-cmake`、`cpp-meson` 四份快照

**验证：**
```bash
cargo check
cargo test
cargo test -- snapshot
cargo insta review
```

---

## 验证清单

Phase 7 完成后：

```bash
cargo check                                            # 编译无错误
cargo test                                             # 全部单元测试通过
cargo test -- snapshot                                 # 快照测试通过（含新增 fixture）
cargo insta review                                     # 审查新增快照
cargo test --test integration_tests -- --ignored       # 集成测试通过
cargo test --test integration_tests -- --ignored deno  # Deno 集成测试
cargo test --test integration_tests -- --ignored gleam # Gleam 集成测试
cargo test --test integration_tests -- --ignored cpp   # C++ 集成测试
```
