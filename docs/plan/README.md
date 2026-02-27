# ArcPack 实施路线图（Implementation Roadmap）

> 版本：v1.0.0 | 日期：2026-02-26 | 状态：待执行
>
> 本文档基于 [arcpack-architecture.md](../design/arcpack-architecture.md) 第 7 节的分阶段实施路线，
> 将每个 Phase 拆解为可独立交付的任务（Task），标注依赖关系、验收标准和 railpack 参考文件。
>
> 所有任务遵循 TDD 工作流：先写测试 → 再写实现 → 测试通过 → 提交。

## 约定

| 项目 | 说明 |
|------|------|
| **任务 ID** | `T{phase}.{seq}`，如 `T1.3` 表示 Phase 1 的第 3 个任务；Phase B 使用 `TB{sub}.{seq}`，如 `TB1.3` |
| **状态** | `pending` 待开始 / `in_progress` 进行中 / `completed` 已完成 |
| **设计文档引用** | `Arch§N.M` = arcpack-architecture.md §N.M；`BK§N` = arcpack-buildkit-subprocess-design.md §N |
| **railpack 引用** | `rp:path/to/file.go` = railpack 仓库中的对应文件 |
| **TDD** | 每个任务的"测试要求"中明确了应先编写的测试用例 |

## Phase 目录

| Phase | 文档 | 任务数 | 目标 |
|-------|------|--------|------|
| [Phase 1: 基础数据结构](./phase-1-data-structures.md) | `phase-1-data-structures.md` | 7 | 核心数据结构可编译可序列化 |
| [Phase 2: Provider 框架 + Node.js](./phase-2-provider-framework.md) | `phase-2-provider-framework.md` | 9 | Node.js BuildPlan 生成成功 |
| [Phase 3: CLI](./phase-3-cli.md) | `phase-3-cli.md` | 6 | CLI 可独立使用 |
| [Phase 4: BuildKit 集成 (Phase A)](./phase-4-buildkit.md) | `phase-4-buildkit.md` | 9 | OCI 镜像端到端构建成功（Dockerfile + buildctl） |
| [Phase 5: 更多 Provider](./phase-5-providers.md) | `phase-5-providers.md` | 8 | 覆盖 7 种语言/场景 + Procfile |

### Phase B: LLB 原生路径（消除 Dockerfile 中间层）

| Phase | 文档 | 任务数 | 目标 |
|-------|------|--------|------|
| [Phase B-1: LLB 原语与协议层](./phase-b1-llb-primitives.md) | `phase-b1-llb-primitives.md` | 6 | LLB protobuf 原语 + Rust builder API |
| [Phase B-2: BuildPlan → LLB 转换](./phase-b2-llb-conversion.md) | `phase-b2-llb-conversion.md` | 6 | BuildPlan 直接生成 LLB Definition |
| [Phase B-3: gRPC 客户端与 Session](./phase-b3-grpc-session.md) | `phase-b3-grpc-session.md` | 6 | tonic gRPC Solve + Session 协议 |
| [Phase B-4: CLI 集成与验证](./phase-b4-cli-integration.md) | `phase-b4-cli-integration.md` | 4 | 双路径切换 + 等价性验证 + Frontend |

**附录：** [arcpack → railpack 文件映射速查表](./appendix-file-mapping.md)

## 任务统计

| Phase | 任务数 | 预计测试用例 | 关键里程碑 |
|-------|--------|------------|----------|
| Phase 1: 基础数据结构 | 7 | ~35 | 核心数据结构可编译可序列化 |
| Phase 2: Provider 框架 + Node.js | 9 | ~50 | Node.js BuildPlan 生成成功 |
| Phase 3: CLI | 6 | ~30 | CLI 可独立使用 |
| Phase 4: BuildKit 集成 (Phase A) | 9 | ~55 | OCI 镜像端到端构建成功（Dockerfile + buildctl） |
| Phase 5: 更多 Provider | 8 | ~75 | 覆盖 7 种语言/场景 + Procfile |
| **Phase A 合计** | **39** | **~245** | |
| Phase B-1: LLB 原语与协议层 | 6 | ~30 | LLB protobuf 原语 + Rust builder API |
| Phase B-2: BuildPlan → LLB 转换 | 6 | ~25 | BuildPlan 直接生成 LLB Definition |
| Phase B-3: gRPC 客户端与 Session | 6 | ~20 | tonic gRPC Solve + Session 协议 |
| Phase B-4: CLI 集成与验证 | 4 | ~15 | 双路径切换 + 等价性验证 + Frontend |
| **Phase B 合计** | **22** | **~90** | |
| **总计** | **61** | **~335** | |

## 全局风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| mise CLI 不可用或版本不兼容 | Phase 2 版本解析无法工作 | Resolver 设计为离线可降级（直接返回原始版本字符串），不强依赖 mise 二进制 |
| buildkitd 权限问题 | Phase 4 集成测试失败 | 提供 rootless 模式和 Docker-in-Docker 两种测试环境配置 |
| railpack API 变更 | 快照测试基准失效 | 快照测试通过 insta crate 管理，变更时 `cargo insta review` 审查 |
| BuildKit cache mount 语法兼容性 | Dockerfile Phase A 在旧版 BuildKit 失败 | Dockerfile 头部声明 `# syntax=docker/dockerfile:1` 确保最新语法 |
| 跨 Provider 缓存键名冲突 | 构建缓存串扰 | 缓存键名统一使用 `{lang}-{purpose}` 格式，与 railpack 对齐 |
| prost 版本不兼容 | Phase B LLB 原语无法使用现有 `buildkit-llb` crate | 自建 LLB 原语，基于 tonic-build 0.12 + prost 0.13 重新生成 proto 绑定 |
| BuildKit Session 协议复杂度（**高风险**） | Phase B-3 Session 使用 HTTP/2 连接劫持而非标准 gRPC 双向流，tonic 可能无法直接实现 | 前置研究任务（见 TB3.3）；降级方案：退化为 LLB + buildctl stdin（TB2.6 已验证）；初版文件传输使用完整传输（非 diff） |
| Phase A / Phase B 行为不一致 | 迁移 Phase B 后产出不同镜像 | TB4.3 等价性验证工具自动化回归测试，所有 fixture 必须通过 |
| gRPC Unix socket 适配 | tonic 不原生支持 Unix socket | 通过 tower `service_fn` + `hyper-util` TokioIo 适配器解决 |
