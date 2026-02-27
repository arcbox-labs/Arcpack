# Phase B-1: LLB 原语与协议层

> [← 返回目录](./README.md) | 上一阶段：[← Phase 4 (Phase A)](./phase-4-buildkit.md) | 下一阶段：[Phase B-2 →](./phase-b2-llb-conversion.md)

**目标：** 建立 LLB protobuf 协议基础，实现 BuildKit LLB DAG 的 Rust 原语类型，提供与 Go `llb.State` 等价的构建 API。

**前置条件：** Phase 4 (Phase A) 全部完成

## 设计决策

| 决策点 | 选择 | 理由 |
|--------|------|------|
| Proto 来源 | 从 BuildKit 仓库获取 `.proto` + `tonic-build 0.12` 重新生成 | 获得最新定义，完全兼容项目 prost 0.13 |
| LLB State builder | 自建，参考 `buildkit-llb` (denzp) API 风格 | 现有 crate prost 版本 (0.6) 与项目 (0.13) 不兼容 |
| Feature gate | `#[cfg(feature = "llb")]`（B-1/B-2）; `#[cfg(feature = "grpc")]`（B-3/B-4） | 最小依赖原则：LLB 序列化只需 prost，不拉入整个 gRPC 栈 |

## 任务依赖图

```
TB1.1 (Proto 获取 + tonic-build 生成)
 └──► TB1.2 (LLB 原语类型：OperationOutput/SerializedOp/digest)
       └──► TB1.3 (Source：Image/Local/Scratch)
             └──► TB1.4 (Exec：Run + Mount 枚举)
                   └──► TB1.5 (File：Copy/Mkfile/Mkdir + Merge)
                         └──► TB1.6 (Terminal：marshal → pb::Definition)
```

## 任务列表

### TB1.1 Proto 获取 + tonic-build 代码生成

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | `rp:buildkit/graph/graph.go`（Go 直接使用 BuildKit SDK，无需 proto） |
| **依赖** | Phase 4 (Phase A) |

**描述：** 从 BuildKit 仓库获取 `ops.proto` 定义文件，配置 `tonic-build` 在编译期生成 Rust 绑定。`ops.proto` 是自包含的纯 proto3 文件，无 gogoproto 扩展依赖，可直接使用。所有生成代码通过 `#[cfg(feature = "llb")]` 条件编译，不影响 Phase A 功能。

**Proto 路径约定：** proto 文件按 BuildKit 仓库原始路径存放（`proto/moby/buildkit/v1/ops.proto`），代码中通过 `tonic::include_proto!("pb")` 按 proto package name 引用。目录结构与 package name 是独立的——目录结构保持与源仓库一致便于更新，package name 由 proto 文件内 `package pb;` 声明决定。

**交付文件：**

- `proto/moby/buildkit/v1/ops.proto` — 从 BuildKit 仓库获取

  > 来源：`github.com/moby/buildkit/solver/pb/ops.proto`
  > 自包含：纯 proto3 定义，无 gogoproto 依赖
  > 包含：`Op`、`ExecOp`、`SourceOp`、`FileOp`、`CopyOp`、`MergeOp`、`Platform` 等核心消息类型

- `build.rs` — tonic-build 编译配置

  ```rust
  fn main() {
      #[cfg(feature = "llb")]
      {
          tonic_build::configure()
              .build_server(true)  // 提前生成 server 代码，B-3 Session 需要；B-1 不使用但无副作用
              .compile_protos(
                  &["proto/moby/buildkit/v1/ops.proto"],
                  &["proto/"],
              )
              .expect("Failed to compile protos");
      }
  }
  ```

- `src/buildkit/proto.rs` — 生成代码的 re-export 模块

  ```rust
  #[cfg(feature = "llb")]
  pub mod pb {
      tonic::include_proto!("pb");  // 按 proto 文件中的 `package pb;` 声明引用
  }
  ```

- `Cargo.toml` 更新 — 新增依赖和 feature

  ```toml
  [features]
  llb = ["dep:prost", "dep:tonic-build"]    # LLB 原语 + protobuf 序列化（B-1/B-2）
  grpc = ["llb", "dep:tonic", "dep:hyper-util", "dep:tower"]  # gRPC 客户端 + Session（B-3/B-4）

  [dependencies]
  prost = { version = "0.13", optional = true }
  tonic = { version = "0.12", optional = true }
  hyper-util = { version = "0.1", optional = true, features = ["tokio"] }
  tower = { version = "0.5", optional = true }

  [build-dependencies]
  tonic-build = { version = "0.12", optional = true }
  ```

**测试要求：**
- `cargo check --features llb` 编译通过，proto 生成无错误
- `pb::Op` / `pb::ExecOp` / `pb::SourceOp` / `pb::FileOp` 类型可实例化
- `pb::Platform` 字段与 `buildkit::platform::Platform` 可互转
- Feature gate 验证：`cargo check`（无 llb feature）不编译 proto 相关代码

---

### TB1.2 LLB 原语类型：OperationOutput / SerializedOp / digest

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | 无直接对应（Go 使用 `llb.State` 内部封装；arcpack 显式建模） |
| **依赖** | TB1.1 |

**描述：** LLB DAG 的核心抽象类型。每个 LLB 操作（Source/Exec/File/Merge）最终产出一个 `OperationOutput`，表示 DAG 中某个操作的某个输出端口。`SerializedOp` 封装序列化后的 protobuf `Op`，并计算 content-addressable digest。这些类型是所有后续 LLB 构建原语的基础。

**交付文件：**

- `src/buildkit/llb/mod.rs` — LLB 模块入口

  ```rust
  #[cfg(feature = "llb")]
  pub mod operation;
  #[cfg(feature = "llb")]
  pub mod source;
  #[cfg(feature = "llb")]
  pub mod exec;
  #[cfg(feature = "llb")]
  pub mod file;
  #[cfg(feature = "llb")]
  pub mod merge;
  #[cfg(feature = "llb")]
  pub mod terminal;

  // re-exports
  #[cfg(feature = "llb")]
  pub use operation::*;
  #[cfg(feature = "llb")]
  pub use source::*;
  #[cfg(feature = "llb")]
  pub use exec::*;
  #[cfg(feature = "llb")]
  pub use file::*;
  #[cfg(feature = "llb")]
  pub use merge::*;
  #[cfg(feature = "llb")]
  pub use terminal::*;
  ```

- `src/buildkit/llb/operation.rs` — 核心原语类型

  **OperationOutput**（LLB DAG 的边）：

  ```rust
  /// LLB DAG 中某个操作的某个输出端口。
  /// 对齐 Go `llb.State` 的核心概念 —— State 本质上就是 (Op, OutputIndex) 的引用。
  #[derive(Clone, Debug)]
  pub struct OperationOutput {
      pub serialized_op: Arc<SerializedOp>,
      pub output_index: i64,
  }
  ```

  **SerializedOp**（序列化后的操作）：

  ```rust
  /// 封装序列化后的 protobuf Op 及其 content-addressable digest。
  #[derive(Debug)]
  pub struct SerializedOp {
      pub bytes: Vec<u8>,           // prost 序列化后的 pb::Op 字节
      pub digest: String,           // "sha256:{hex}" 格式
      pub metadata: OpMetadata,     // 描述信息（description 等）
      pub inputs: Vec<OperationOutput>,  // 此操作依赖的输入
  }
  ```

  **OpMetadata**：

  ```rust
  #[derive(Clone, Debug, Default)]
  pub struct OpMetadata {
      pub description: HashMap<String, String>,
      pub caps: HashMap<String, bool>,
  }
  ```

  **辅助函数**：

  | 函数 | 签名 | 说明 |
  |------|------|------|
  | `digest_of` | `(bytes: &[u8]) -> String` | 计算 `sha256:{hex}` 格式的 content digest |
  | `serialize_op` | `(op: &pb::Op, metadata: OpMetadata, inputs: Vec<OperationOutput>) -> SerializedOp` | 序列化 Op + 计算 digest |
  | `make_output` | `(serialized_op: SerializedOp, output_index: i64) -> OperationOutput` | 构造 OperationOutput |

**测试要求：**
- `digest_of()` 对相同输入产生相同 digest
- `digest_of()` 对不同输入产生不同 digest
- `digest_of()` 输出格式为 `sha256:{64位hex}`
- `serialize_op()` 对空 Op 和带字段 Op 都能正确序列化
- `OperationOutput` clone 后 digest 不变
- `SerializedOp` 的 inputs 正确记录依赖关系

---

### TB1.3 Source 操作：Image / Local / Scratch

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | Go `llb.Image()`, `llb.Local()`, `llb.Scratch()` |
| **依赖** | TB1.2 |

**描述：** LLB 的三种数据源操作。`Image` 从 registry 拉取基础镜像；`Local` 挂载本地构建上下文；`Scratch` 创建空文件系统。这三个函数是所有 LLB DAG 的叶节点（无输入依赖），是构建图的起点。

**交付文件：**

- `src/buildkit/llb/source.rs`

  **顶层构造函数**（对齐 Go `llb` 包的公开函数）：

  | 函数 | 签名 | Go 对应 | 说明 |
  |------|------|---------|------|
  | `image` | `(reference: &str) -> OperationOutput` | `llb.Image(ref)` | 拉取基础镜像，生成 `SourceOp { identifier: "docker-image://..." }` |
  | `image_with_platform` | `(reference: &str, platform: &Platform) -> OperationOutput` | `llb.Image(ref, llb.Platform(...))` | 指定平台的镜像拉取 |
  | `local` | `(name: &str) -> OperationOutput` | `llb.Local(name)` | 挂载本地目录，`name` 对应 buildctl `--local {name}=...` |
  | `local_with_opts` | `(name: &str, opts: LocalOpts) -> OperationOutput` | `llb.Local(name, opts...)` | 带 include/exclude 过滤的本地挂载 |
  | `scratch` | `() -> OperationOutput` | `llb.Scratch()` | 空文件系统，`SourceOp { identifier: "" }` + 无输出 |

  **LocalOpts**：

  ```rust
  #[derive(Clone, Debug, Default)]
  pub struct LocalOpts {
      pub include_patterns: Vec<String>,
      pub exclude_patterns: Vec<String>,
      pub shared_key_hint: String,
  }
  ```

  **SourceOp 构造细节**（内部，对齐 Go `llb/source.go`）：

  ```
  image("node:20"):
    Op {
        op: Some(SourceOp {
            identifier: "docker-image://docker.io/library/node:20"
        }),
        platform: Some(Platform { os: "linux", architecture: "amd64", ... }),
    }

  local("context"):
    Op {
        op: Some(SourceOp {
            identifier: "local://context"
        }),
        // attrs: include/exclude 模式
    }

  scratch():
    Op {
        op: Some(SourceOp {
            identifier: ""  // 空标识符 = scratch
        }),
    }
    // output_index = 0，但 Op 无实际输出定义
  ```

**测试要求：**
- `image("node:20")` 生成正确的 `docker-image://` identifier
- `image("node:20")` 的 digest 稳定（相同输入 → 相同 digest）
- `image_with_platform()` 在 Op 中设置正确的 Platform 字段
- `local("context")` 生成 `local://context` identifier
- `local_with_opts()` 正确设置 include/exclude attrs
- `scratch()` 生成空 identifier 的 SourceOp
- 所有 Source 操作的 `output_index` 为 0
- 不同镜像引用产生不同 digest

---

### TB1.4 Exec 操作：Run + Mount 枚举

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | Go `llb.State.Run()`, `llb.AddMount()`, `llb.AddCacheMountOption()` |
| **依赖** | TB1.3 |

**描述：** LLB Exec 操作，对应容器内执行命令。使用 builder pattern 构建，支持多种 Mount 类型（缓存挂载、Secret 挂载、只读层挂载）。Exec 是 LLB DAG 中最常用的操作，构建步骤中的 `RUN` 指令都映射为 Exec。

**交付文件：**

- `src/buildkit/llb/exec.rs`

  **ExecBuilder**（builder pattern，对齐 Go `llb.ExecInfo` + `RunOption`）：

  ```rust
  pub struct ExecBuilder {
      input: OperationOutput,         // 执行环境（rootfs）
      args: Vec<String>,              // 命令参数 ["/bin/sh", "-c", "npm install"]
      env: Vec<(String, String)>,     // 环境变量
      cwd: String,                    // 工作目录
      mounts: Vec<MountSpec>,         // 额外挂载
      metadata: OpMetadata,           // 描述信息
  }
  ```

  **ExecBuilder 方法**：

  | 方法 | 签名 | 说明 | Go 对应 |
  |------|------|------|---------|
  | `new` | `(input: OperationOutput, args: Vec<String>) -> Self` | 构造 builder | `state.Run(...)` |
  | `env` | `(mut self, key, value) -> Self` | 添加环境变量 | `llb.AddEnv()` |
  | `cwd` | `(mut self, dir: &str) -> Self` | 设置工作目录 | `llb.Dir()` |
  | `add_mount` | `(mut self, mount: MountSpec) -> Self` | 添加挂载 | `llb.AddMount()` |
  | `add_cache_mount` | `(mut self, target: &str, cache_id: &str, sharing: CacheSharingMode) -> Self` | 添加缓存挂载 | `llb.AddCacheMountOption()` |
  | `add_secret_env` | `(mut self, name: &str, env_name: &str) -> Self` | 添加 Secret 环境变量 | `llb.AddSecret()` |
  | `description` | `(mut self, desc: &str) -> Self` | 设置描述信息 | `llb.WithCustomName()` |
  | `root` | `(self) -> OperationOutput` | 构建 ExecOp → 返回 rootfs 输出 | `.Root()` |

  **MountSpec 枚举**（对齐 Go `pb.Mount` 的各种模式）：

  ```rust
  pub enum MountSpec {
      /// 缓存挂载：持久化目录，跨构建复用
      Cache {
          target: String,                // 挂载目标路径
          cache_id: String,              // 缓存键
          sharing: CacheSharingMode,     // 共享模式
      },
      /// Secret 环境变量挂载
      SecretEnv {
          name: String,                  // secret 名称
          env_name: String,              // 环境变量名
      },
      /// 只读层挂载：从其他操作的输出挂载为只读
      ReadOnlyLayer {
          input: OperationOutput,        // 源操作输出
          target: String,                // 挂载目标路径
      },
  }
  ```

  **CacheSharingMode**（对齐 Go `pb.CacheSharingOpt`）：

  ```rust
  pub enum CacheSharingMode {
      Shared,   // 多个构建步骤可同时读写（默认）
      Locked,   // 互斥锁定，同一时间只有一个步骤可访问
  }
  ```

  **ExecOp 生成细节**（内部，对齐 Go `ExecOp` protobuf）：

  ```
  ExecBuilder::new(base, ["sh", "-c", "npm install"])
      .cwd("/app")
      .env("NODE_ENV", "production")
      .add_cache_mount("/root/.npm", "npm-cache", Shared)
      .root()

  →

  Op {
      inputs: [{ digest: base.digest, index: base.output_index }],
      op: Some(ExecOp {
          meta: Some(Meta {
              args: ["/bin/sh", "-c", "npm install"],
              env: ["NODE_ENV=production"],
              cwd: "/app",
          }),
          mounts: [
              Mount { input: 0, dest: "/", output: 0 },       // rootfs
              Mount {
                  mount_type: CACHE,
                  cache_opt: Some(CacheOpt { id: "npm-cache", sharing: SHARED }),
                  dest: "/root/.npm",
              },
          ],
      }),
  }
  ```

**测试要求：**
- `ExecBuilder` 基本构造：args 和 cwd 正确设置
- 环境变量添加：多次 `.env()` 调用累积
- 缓存挂载生成正确的 `pb::Mount` 类型和 `CacheOpt`
- Secret 环境变量挂载：`mount_type = SECRET`，`env` 字段设置
- 只读层挂载：正确引用输入操作的 digest
- `root()` 返回的 `OperationOutput` 的 `output_index` 为 0
- 多个挂载组合：cache + secret 同时存在于同一 ExecOp
- 空环境变量列表和非空列表都正确序列化

---

### TB1.5 File 操作 + Merge

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | Go `llb.Copy()`, `llb.Mkfile()`, `llb.Mkdir()`, `llb.Merge()` |
| **依赖** | TB1.4 |

**描述：** LLB 的文件操作和合并操作。File 操作用于在容器文件系统中创建文件、目录或从其他源复制文件。Merge 操作用于将多个文件系统层合并为一个，是 Phase A layers 合并策略在 LLB 中的等价物。

**交付文件：**

- `src/buildkit/llb/file.rs` — File 操作

  **顶层构造函数**（对齐 Go `llb` 包的公开函数）：

  | 函数 | 签名 | Go 对应 | 说明 |
  |------|------|---------|------|
  | `copy` | `(src: OperationOutput, src_path: &str, dest: OperationOutput, dest_path: &str) -> OperationOutput` | `llb.Copy(src, srcPath, dest, destPath)` | 从 src 复制文件到 dest |
  | `copy_with_opts` | `(src, src_path, dest, dest_path, opts: CopyOpts) -> OperationOutput` | `llb.Copy(src, srcPath, dest, destPath, opts...)` | 带选项的复制 |
  | `make_file` | `(dest: OperationOutput, path: &str, content: &[u8], mode: i32) -> OperationOutput` | `llb.Mkfile(path, mode, content)` | 在 dest 上创建文件 |
  | `make_dir` | `(dest: OperationOutput, path: &str, mode: i32) -> OperationOutput` | `llb.Mkdir(path, mode)` | 在 dest 上创建目录 |

  **CopyOpts**：

  ```rust
  #[derive(Clone, Debug, Default)]
  pub struct CopyOpts {
      pub create_dest_path: bool,           // 自动创建目标目录
      pub allow_wildcard: bool,             // 允许通配符匹配
      pub allow_empty_wildcard: bool,       // 通配符无匹配时不报错
      pub follow_symlinks: bool,            // 跟随符号链接
  }
  ```

  **FileOp 生成细节**（内部，对齐 Go `pb.FileOp`）：

  ```
  copy(src, "/app/dist", dest, "/app/dist"):
    Op {
        inputs: [src.input_ref, dest.input_ref],
        op: Some(FileOp {
            actions: [
                FileAction {
                    action: Some(Copy(FileActionCopy {
                        src: src.output_index,
                        dest: dest.output_index,
                        src_path: "/app/dist",
                        dest_path: "/app/dist",
                        ...
                    }))
                }
            ],
        }),
    }
  ```

- `src/buildkit/llb/merge.rs` — Merge 操作

  **顶层构造函数**：

  | 函数 | 签名 | Go 对应 | 说明 |
  |------|------|---------|------|
  | `merge` | `(inputs: Vec<OperationOutput>) -> OperationOutput` | `llb.Merge(inputs)` | 合并多个文件系统层 |

  **MergeOp 生成细节**（内部，对齐 Go `pb.MergeOp`）：

  ```
  merge([layer1, layer2, layer3]):
    Op {
        inputs: [layer1.ref, layer2.ref, layer3.ref],
        op: Some(MergeOp {
            inputs: [
                MergeInput { input: 0 },
                MergeInput { input: 1 },
                MergeInput { input: 2 },
            ],
        }),
    }
  ```

  > Merge 对应 Phase A 中 `should_merge() == true` 时的 multi-stage COPY 策略。
  > 在 LLB 中 Merge 是原生操作，比 Dockerfile 的 COPY 更高效。

**测试要求：**
- `copy()` 正确设置 src/dest 输入引用和路径
- `copy_with_opts()` 选项正确传递到 `FileActionCopy`
- `make_file()` 创建的 `FileActionMkFile` 包含正确内容和 mode
- `make_dir()` 创建的 `FileActionMkDir` 包含正确路径和 mode
- `merge()` 单输入退化为直接传递（无 MergeOp）
- `merge()` 多输入正确设置 inputs 列表和 `MergeInput` 索引
- `merge()` 空输入返回错误
- File 操作链式组合：`make_dir → copy → make_file` 依赖链正确

---

### TB1.6 Terminal：marshal → pb::Definition

| 字段 | 值 |
|------|---|
| **状态** | `completed` |
| **设计文档** | Arch§3.10 |
| **railpack 参考** | Go `llb.State.Marshal()` → `solver/pb.Definition` |
| **依赖** | TB1.5 |

**描述：** LLB DAG 的终结操作——将 `OperationOutput` 引用的整个操作图序列化为 `pb::Definition`，即 BuildKit Solve RPC 的输入格式。`marshal` 执行 BFS 遍历 DAG，收集所有可达操作，按拓扑序生成 `Definition { def: Vec<Vec<u8>>, metadata: HashMap<String, OpMetadata> }`。

**交付文件：**

- `src/buildkit/llb/terminal.rs`

  **核心函数**（对齐 Go `llb.State.Marshal()` → `solver/pb.Definition`）：

  ```rust
  /// 将 LLB DAG 序列化为 BuildKit 可接受的 Definition。
  /// 从 output 开始 BFS 遍历所有依赖操作，按拓扑序序列化。
  ///
  /// 对齐 Go `State.Marshal(ctx, constraints)` → `Definition`
  pub fn marshal(output: &OperationOutput) -> Result<pb::Definition>
  ```

  **序列化流程**（对齐 Go `MarshalConstraints` + `State.Marshal`）：

  ```
  marshal(output):
    1. BFS 从 output.serialized_op 开始，遍历所有 inputs（递归）
    2. 收集所有唯一 SerializedOp（按 digest 去重）
    3. 按拓扑序排列（叶节点在前，根节点在后）
    4. 生成 terminal Op（空 Op，inputs 指向 output）
    5. 构造 Definition:
       - def: 所有 SerializedOp.bytes + terminal Op bytes（按拓扑序）
       - metadata: digest → OpMetadata 映射

    返回 pb::Definition {
        def: Vec<Vec<u8>>,        // 序列化的操作列表（拓扑序）
        metadata: HashMap<String, pb::OpMetadata>,  // digest → 元数据
    }
  ```

  **Terminal Op**（对齐 Go `Marshal` 的 terminal vertex）：

  ```
  // 最后一个 Op：空操作，仅引用最终输出，作为 Definition 的根
  Op {
      inputs: [Input { digest: output.digest, index: output.output_index }],
      op: None,  // 无实际操作
  }
  ```

  **辅助函数**：

  | 函数 | 签名 | 说明 |
  |------|------|------|
  | `collect_ops` | `(output: &OperationOutput) -> Vec<Arc<SerializedOp>>` | BFS 收集所有可达操作，按 digest 去重 |
  | `topological_sort_ops` | `(ops: Vec<Arc<SerializedOp>>) -> Vec<Arc<SerializedOp>>` | 拓扑排序（叶节点优先） |
  | `build_terminal_op` | `(output: &OperationOutput) -> Vec<u8>` | 构造 terminal Op 的序列化字节 |

**测试要求：**
- 单操作 marshal：`scratch()` → Definition 包含 1 个 SourceOp + 1 个 terminal Op
- 线性链 marshal：`image → exec → copy` → Definition 按拓扑序包含 3 个 Op + terminal
- DAG 去重：两个 Exec 共享同一 Image 输入，marshal 后 Image 只出现一次
- 菱形依赖 marshal：A→B, A→C, B→D, C→D → D 的 marshal 包含 4 个唯一 Op
- Definition.metadata 包含所有 Op 的描述信息
- terminal Op 的 inputs 正确引用最终输出的 digest 和 index
- marshal 后的 Definition 可被 `prost::Message::encode()` 序列化为字节（round-trip 测试）

---

## 与 railpack 的已知差异

| 方面 | railpack (Go) | arcpack Phase B-1 (Rust) | 原因 |
|------|--------------|--------------------------|------|
| LLB SDK | 直接使用 `github.com/moby/buildkit/client/llb` | 自建 LLB 原语，参考 denzp/buildkit-llb API 风格 | Go SDK 无 Rust 等价物；现有 Rust crate prost 版本不兼容 |
| Proto 生成 | Go 内置 protobuf 支持 | tonic-build 0.12 + prost 0.13 重新生成 | 获得最新 proto 定义，完全兼容项目依赖版本 |
| State 抽象 | `llb.State` 隐式封装 Op + OutputIndex | `OperationOutput` 显式持有 `Arc<SerializedOp>` + `output_index` | Rust 显式所有权模型，避免隐式共享状态 |
| Builder pattern | Go 使用 functional options (`RunOption`) | Rust 使用 method chaining builder pattern | 更符合 Rust 惯用风格 |
| Merge | `llb.Merge()` 直接调用 | `merge()` + 单输入退化优化 | 减少不必要的 MergeOp |

---

## Phase B-1 Gate

**执行命令：**
```bash
cargo check --features llb
cargo test --features llb
```

**验收清单：**
- [x] `cargo check --features llb` 无错误无警告
- [x] `cargo check`（无 llb feature）不编译任何 Phase B-1 代码
- [x] `pb::Op` / `pb::ExecOp` / `pb::SourceOp` / `pb::FileOp` / `pb::MergeOp` 类型可用
- [x] `image()` / `local()` / `scratch()` 生成正确的 SourceOp
- [x] `ExecBuilder` 支持 cache mount + secret env + readonly layer
- [x] `copy()` / `make_file()` / `make_dir()` 生成正确的 FileOp
- [x] `merge()` 生成正确的 MergeOp
- [x] `marshal()` 对 DAG 执行拓扑序列化，输出正确的 `pb::Definition`
- [x] `marshal()` 对共享输入正确去重
- [x] 所有 digest 格式为 `sha256:{64位hex}`
- [x] 预计 ~30 个测试用例全部通过
