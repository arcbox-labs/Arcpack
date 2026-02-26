# ArcPack 技术架构设计 Prompt

## 角色设定

你是一名资深的云原生基础设施架构师，精通容器构建工具链（BuildKit、Dockerfile、OCI 规范）、Rust 系统编程以及 PaaS 平台架构设计。你的任务是帮助我设计一个名为 **ArcPack** 的项目的完整技术架构和技术方案。

## 项目背景

### ArcPack 是什么

ArcPack 是一个使用 Rust 编写的**零配置应用构建器**，灵感来源于 [Railpack](https://github.com/railwayapp/railpack)。它的核心目标是：

- 接收用户的源代码仓库作为输入
- 自动检测项目的语言、框架和依赖
- 零配置地生成优化的 OCI 容器镜像
- 无需用户编写 Dockerfile

ArcPack 是 **ArcBox**（一个类似于 Railway / Fly.io 的部署平台）的核心构建组件。ArcBox 提供双层存储（本地 NVMe 高性能块存储 + S3 兼容对象存储）的部署服务。

### 参考架构：Railpack 的核心流程

我已经深入研究了 Railpack，以下是它的核心架构流程：

```
源代码输入
  → Provider(s) 检测项目类型（detect）并贡献构建信息（contribute）
    → 多个 Provider 可以同时匹配（如 Node.js + Python monorepo）
    → Provider 接收 Context（包含项目文件系统信息 + BuildPlan 的可变引用）
    → Provider 通过 Context 向 BuildPlan 写入构建步骤
  → BuildPlan 聚合所有 Provider 的输出
    → 包含 setup steps、install steps、build steps、start command
    → 包含系统依赖声明、环境变量等
  → Railpack 自身将 BuildPlan 转译为 LLB（Low-Level Build definition）
    → LLB 是 BuildKit 能理解的 DAG 格式
  → 通过 gRPC 提交给 BuildKit daemon 执行
  → BuildKit 执行构建 → 输出 OCI 镜像
```

### 关键设计概念

- **Context**：运行时的工作台/上下文对象，持有项目的文件系统信息（哪些文件存在、配置文件内容等）以及对 BuildPlan 的可变引用。Provider 在执行 detect 和 contribute 时接收 Context。
- **Provider**：语言/框架检测器 + 构建规划器。职责包括：检测项目类型、声明安装/构建/启动命令、声明系统级依赖。
- **BuildPlan**：所有 Provider 输出的聚合产物，结构化描述完整的构建蓝图。
- **LLB 转译**：将高层的 BuildPlan 转化为 BuildKit 能理解的底层 DAG 表示。
- **分层策略**：将构建产物分成多个 OCI 层（系统依赖层 → 语言运行时层 → 应用依赖层 → 应用代码层），利用层缓存加速增量构建。

## 设计需求

请帮我完成以下技术架构和方案设计：

### 1. 整体架构设计

- 绘制 ArcPack 的完整架构图（使用 Mermaid 或文字描述）
- 明确各模块的边界和职责
- 描述数据在各模块之间的流转方式
- 说明 ArcPack 作为 ArcBox 平台子组件时的集成方式

### 2. 核心模块设计

对以下每个模块，请给出：
- 模块职责与接口定义（Rust trait 设计）
- 关键数据结构（Rust struct）
- 模块间交互方式

#### 2.1 Source Analyzer（源码分析器）
- 如何接收和读取源代码目录
- 文件系统抽象层设计（支持本地目录 / Git 仓库 / 远程归档）

#### 2.2 Provider System（提供者系统）
- Provider trait 设计（detect / contribute 方法签名）
- 多 Provider 同时匹配的机制
- Provider 的优先级和冲突解决策略
- 需要支持的 Provider 列表（按优先级排序）：
  - Node.js (npm / yarn / pnpm / bun)
  - Python (pip / poetry / uv)
  - Go
  - Rust
  - Java (Maven / Gradle)
  - 静态网站（HTML/CSS/JS）
  - Dockerfile（回退方案）

#### 2.3 Context（上下文）
- Context 的数据结构设计
- 如何向 Provider 暴露项目信息
- 如何持有和管理 BuildPlan 的可变引用

#### 2.4 BuildPlan（构建计划）
- BuildPlan 的完整数据结构
- 如何表示构建阶段（setup / install / build / start）
- 系统依赖、环境变量、缓存路径的表示方式

#### 2.5 LLB Generator（LLB 生成器）
- 将 BuildPlan 转译为 LLB 的策略
- OCI 镜像分层策略的实现
- 缓存挂载（cache mounts）的设计

#### 2.6 BuildKit Client（BuildKit 客户端）
  [通信技术方案](./arcpack-buildkit-subprocess-design.md)

### 3. 构建流水线设计

- 完整的构建流水线（pipeline）设计
- 错误处理和日志策略
- 构建进度的实时反馈机制（streaming logs）
- 构建超时和资源限制

### 4. 缓存策略

- 镜像层缓存（利用 OCI 层复用）
- 依赖缓存（node_modules、pip cache 等的跨构建复用）
- BuildKit 内置缓存的利用方式
- 缓存失效策略

### 5. 与 ArcBox 平台的集成

- ArcPack 作为独立 CLI 工具的使用方式
- ArcPack 作为 ArcBox 平台内部服务的集成方式
- API 设计（暴露给 ArcBox 的接口）
- 构建结果的输出格式（OCI 镜像推送到 registry）

### 6. 技术选型建议

请为以下方面给出 Rust 生态的具体技术选型建议：
- gRPC 客户端（tonic / 其他）
- 序列化（serde / protobuf）
- 异步运行时（tokio）
- 文件系统操作
- 日志框架
- CLI 框架（clap / 其他）
- 测试框架和策略

### 7. 与 Railpack 的差异化(前期先保证与Railpack能力对齐)

ArcPack 相对于 Railpack 可以考虑的差异化方向（请评估可行性）：
- 更好的 monorepo 支持
- 构建缓存的持久化和跨机器共享
- 插件化的 Provider 系统（用户自定义 Provider）
- 更丰富的构建钩子（pre-build / post-build hooks）
- 与 ArcBox 平台的深度集成（如直接利用 NVMe 存储加速构建缓存）

## 输出要求

1. 使用清晰的层次结构组织文档
2. 关键设计给出 Rust 代码示例（trait 定义、核心 struct、关键函数签名）
3. 使用 Mermaid 图表展示架构和流程
4. 对有争议的设计决策给出多个方案对比，并给出推荐
5. 标注设计中的风险点和需要原型验证的部分
