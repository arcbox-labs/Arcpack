# 附录: arcpack → railpack 文件映射速查表

> [← 返回目录](./README.md)

为方便开发时快速定位 railpack 参考实现，以下列出关键文件的对应关系。

## 核心数据结构

| arcpack | railpack | 说明 |
|---------|----------|------|
| `src/error.rs` | (分散) | railpack 无统一错误类型 |
| `src/plan/mod.rs` | `rp:core/plan/plan.go` | BuildPlan + Deploy |
| `src/plan/step.rs` | `rp:core/plan/step.go` | Step |
| `src/plan/command.rs` | `rp:core/plan/command.go` | Command (Go interface → Rust enum) |
| `src/plan/layer.rs` | `rp:core/plan/layer.go` | Layer |
| `src/plan/filter.rs` | `rp:core/plan/filters.go` | Filter |
| `src/plan/cache.rs` | `rp:core/plan/cache.go` | Cache |
| `src/plan/spread.rs` | `rp:core/plan/spread.go` | Spread |
| `src/plan/packages.rs` | `rp:core/plan/packages.go` | PlanPackages |
| `src/plan/dockerignore.rs` | `rp:core/plan/dockerignore.go` | .dockerignore 解析 |

## 文件系统 & 配置

| arcpack | railpack | 说明 |
|---------|----------|------|
| `src/app/mod.rs` | `rp:core/app/app.go` | App |
| `src/app/environment.rs` | `rp:core/app/environment.go` | Environment |
| `src/config/mod.rs` | `rp:core/config/config.go` | Config |

## 编排 & 构建器

| arcpack | railpack | 说明 |
|---------|----------|------|
| `src/generate/mod.rs` | `rp:core/generate/context.go` | GenerateContext |
| `src/generate/command_step_builder.rs` | `rp:core/generate/command_step_builder.go` | CommandStepBuilder |
| `src/generate/mise_step_builder.rs` | `rp:core/generate/mise_step_builder.go` | MiseStepBuilder |
| `src/generate/image_step_builder.rs` | `rp:core/generate/image_step_builder.go` | ImageStepBuilder |
| `src/generate/deploy_builder.rs` | `rp:core/generate/deploy_builder.go` | DeployBuilder |
| `src/generate/install_bin_builder.rs` | `rp:core/generate/install_bin_builder.go` | InstallBinBuilder |
| `src/generate/cache_context.rs` | `rp:core/generate/cache_context.go` | CacheContext |
| `src/resolver/mod.rs` | `rp:core/resolver/resolver.go` | Resolver |
| `src/mise/mod.rs` | `rp:core/mise/mise.go` | Mise |
| `src/mise/install.rs` | `rp:core/mise/install.go` | Mise install 脚本 |

## Provider

| arcpack | railpack | 说明 |
|---------|----------|------|
| `src/provider/mod.rs` | `rp:core/providers/provider.go` | Provider trait + registry |
| `src/provider/node/mod.rs` | `rp:core/providers/node/node.go` | NodeProvider |
| `src/provider/node/detect.rs` | `rp:core/providers/node/package_manager.go` | 包管理器检测 |
| `src/provider/golang.rs` | `rp:core/providers/golang/golang.go` | GoProvider |
| `src/provider/python.rs` | `rp:core/providers/python/python.go` | PythonProvider |
| `src/provider/rust_lang.rs` | `rp:core/providers/rust/rust.go` | RustProvider |
| `src/provider/java.rs` | `rp:core/providers/java/java.go` | JavaProvider |
| `src/provider/staticfile.rs` | `rp:core/providers/staticfile/staticfile.go` | StaticFileProvider |
| `src/provider/shell.rs` | `rp:core/providers/shell/shell.go` | ShellProvider |
| `src/provider/procfile.rs` | `rp:core/providers/procfile/procfile.go` | ProcfileProvider |

## BuildKit

| arcpack | railpack | 说明 |
|---------|----------|------|
| `src/graph/mod.rs` | `rp:buildkit/graph/graph.go` | Graph<T> |
| `src/buildkit/build_llb/mod.rs` | `rp:buildkit/build_llb/build_graph.go` | BuildGraph |
| `src/buildkit/build_llb/step_node.rs` | `rp:buildkit/build_llb/step_node.go` | StepNode |
| `src/buildkit/build_llb/build_env.rs` | `rp:buildkit/build_llb/build_env.go` | BuildEnvironment |
| `src/buildkit/build_llb/layers.rs` | `rp:buildkit/build_llb/layers.go` | Layer 合并策略 |
| `src/buildkit/build_llb/cache_store.rs` | `rp:buildkit/build_llb/cache_store.go` | CacheStore |
| `src/buildkit/convert.rs` | `rp:buildkit/convert.go` | 转换入口 |
| `src/buildkit/build.rs` | `rp:buildkit/build.go` | 构建主流程 |
| `src/buildkit/daemon.rs` | (arcpack 独有) | DaemonManager |
| `src/buildkit/client.rs` | (arcpack 独有) | BuildKitClient (buildctl) |
| `src/buildkit/image.rs` | `rp:buildkit/image.go` | OCI Image config |
| `src/buildkit/platform.rs` | `rp:buildkit/platform.go` | 平台解析 |

## CLI & 入口

| arcpack | railpack | 说明 |
|---------|----------|------|
| `src/lib.rs` | `rp:core/core.go` | generate_build_plan() |
| `src/main.rs` | `rp:cmd/cli/main.go` | CLI 入口 |
| `src/cli/build.rs` | `rp:cli/build.go` | build 命令 |
| `src/cli/plan.rs` | `rp:cli/plan.go` | plan 命令 |
| `src/cli/info.rs` | `rp:cli/info.go` | info 命令 |
| `src/cli/schema.rs` | `rp:cli/schema.go` | schema 命令 |

## 测试

| arcpack | railpack | 说明 |
|---------|----------|------|
| `tests/snapshot_tests.rs` | `rp:core/core_test.go` | BuildPlan 快照测试 |
| `tests/integration_buildkit.rs` | `rp:buildkit/build_test.go` | BuildKit 集成测试 |
