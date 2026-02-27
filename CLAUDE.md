# arcpack

零配置应用构建器（灵感来源于 [Railpack](./railpack/)）。
自动检测源码的语言/框架 → 生成构建计划 → 转译为 LLB → 通过 BuildKit 输出 OCI 镜像，无需用户编写 Dockerfile。
属于 ArcBox（类似 Railway / Fly.io 的 PaaS 平台）的核心构建组件。

## 技术栈

- 语言：Rust
- 构建工具：BuildKit (buildkitd + buildctl)
- 通信方式：Unix Socket gRPC
- 关键 crate：tokio（异步）、tonic（gRPC）、clap（CLI）、serde（序列化）

## 架构要点

核心流水线：`源码 → Provider 检测 → BuildPlan → LLB 转译 → BuildKit 构建 → OCI 镜像`

核心模块：
- **Source Analyzer** — 读取源码目录，提供文件系统抽象
- **Provider** — 语言/框架检测器（detect + contribute），多个可同时匹配
- **Context** — 运行时上下文，向 Provider 暴露项目信息和 BuildPlan 可变引用
- **BuildPlan** — 聚合所有 Provider 输出的构建蓝图（setup / install / build / start）
- **LLB Generator** — 将 BuildPlan 转译为 BuildKit DAG，实施 OCI 分层策略
- **BuildKit Client** — 子进程管理 buildkitd，当前用 buildctl CLI，后续迁移 tonic gRPC

Provider 支持范围：Node.js / Python / Go / Rust / Java / 静态网站 / Dockerfile（回退）

运行模式：独立 CLI 工具 + ArcBox 平台内部服务（Spot 实例，用完即销毁）

## 项目约定

- 只有代码注释使用中文
- commit message 使用英文
- 设计文档放在 `docs/` 目录下
- 重要：方案设计和编码时，始终参考 [Railpack](https://github.com/railwayapp/railpack) 的实现作为基准

### 交互行为约定

1. **直接写代码，不要输出计划** — 收到实现任务时，立即读代码、写代码。不要先输出"我将执行以下步骤"的计划文本。如果需要规划，使用 plan mode，不要在聊天中罗列步骤。
2. **对话式协作，而非自主探索** — 每完成一个文件的修改后，简要说明改了什么、为什么这样改，然后继续。不要一次性沉默地改 10 个文件后才汇报。遇到设计抉择时主动问我。
3. **编辑后验证引用完整性** — 修改函数签名、重命名类型/模块、删除 pub 项时，必须 grep 搜索所有引用点并同步更新，确保 `cargo check` 通过。
4. **默认 Rust 惯用模式** — 优先使用 `Result<T, E>` 而非 panic、`impl Trait` 而非 dyn、`thiserror` 派生错误类型、Builder pattern 构建复杂结构体。不需要我每次指定。
5. **逐文件解释，不要高层概述** — 解释代码变更时，按文件逐个说明具体改动（哪些行、什么逻辑），而不是"我重构了错误处理系统"这样的抽象描述。
6. **改完代码必须跑测试** — 任何代码修改完成后，必须运行 `cargo test` 并报告结果。测试失败时先修复再继续，不要跳过。

## 测试规范

### 分层策略

| 层级 | 位置 | 职责 | 是否需要外部依赖 |
|------|------|------|-----------------|
| 单元测试 | 各源文件底部 `#[cfg(test)] mod tests` | 测试纯逻辑函数（参数解析、配置构建、路径处理等） | 否 |
| 集成测试 | `tests/` 目录 | 测试模块间协作（子进程生命周期、socket 通信、构建流程） | 是（需要 buildkitd） |

### 命名与组织

- 测试函数命名：`test_<被测行为>_<场景>_<预期结果>`，例如 `test_wait_ready_timeout_returns_error`
- 每个 `#[cfg(test)]` 模块只测试当前文件的公开和私有逻辑
- 需要 buildkitd/buildctl 的测试标记 `#[ignore]`，CI 中通过 `cargo test -- --ignored` 单独运行

### 编写原则

- **先写测试，再写实现**（TDD）——新功能和 bug 修复都应先有失败的测试
- **测试行为而非实现**——断言可观测的输出和副作用，不断言内部状态
- **每个测试只验证一件事**——失败时能立即定位原因
- **测试必须可独立运行**——不依赖执行顺序，不共享可变状态
- **使用 `assert!` / `assert_eq!` / `assert_matches!`**——提供清晰的失败信息，避免裸 `unwrap()`

### 外部依赖隔离

- 子进程调用（buildkitd、buildctl）通过 trait 抽象，单元测试中使用 mock 实现
- 文件系统操作使用 `tempfile` crate 创建临时目录，测试结束自动清理
- Unix socket 测试使用临时路径，避免与正在运行的服务冲突

### 运行命令

```bash
cargo test                    # 运行所有单元测试
cargo test -- --ignored       # 运行需要外部依赖的集成测试
cargo test -- --nocapture     # 显示测试中的 println! 输出
```
