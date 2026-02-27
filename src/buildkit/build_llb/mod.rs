pub mod build_env;
pub mod cache_store;
pub mod layers;
pub mod step_node;

use std::collections::HashMap;

use crate::graph::Graph;
use crate::plan::{BuildPlan, Command, Deploy, ARCPACK_RUNTIME_IMAGE};
use crate::Result;

use build_env::BuildEnvironment;
use cache_store::BuildKitCacheStore;
use step_node::StepNode;

use super::platform::Platform;

#[cfg(feature = "llb")]
use crate::buildkit::proto::pb;

/// Dockerfile 生成输出
pub struct BuildGraphOutput {
    pub dockerfile: String,
    pub output_env: BuildEnvironment,
}

/// 构建图 —— 将 BuildPlan 转换为 Dockerfile
///
/// 对齐 railpack `buildkit/build_llb/build_graph.go`
pub struct BuildGraph {
    graph: Graph<StepNode>,
    cache_store: BuildKitCacheStore,
    plan: BuildPlan,
    secrets_hash: Option<String>,
    #[allow(dead_code)]
    platform: Platform,
}

impl BuildGraph {
    /// 从 BuildPlan 构建图
    ///
    /// 1. 为每个 step 创建 StepNode 并注册到图中
    /// 2. 根据 step.inputs 中的 step 引用建立有向边
    /// 3. 执行传递归约
    pub fn new(
        plan: BuildPlan,
        cache_store: BuildKitCacheStore,
        secrets_hash: Option<String>,
        platform: Platform,
    ) -> Result<Self> {
        let mut graph = Graph::new();

        // 注册节点
        for step in &plan.steps {
            graph.add_node(StepNode::new(step.clone()));
        }

        // 建立边：input.step -> step.name（parent -> child）
        for step in &plan.steps {
            let step_name = match &step.name {
                Some(name) => name.clone(),
                None => continue,
            };
            for input in &step.inputs {
                if let Some(ref parent_name) = input.step {
                    graph.add_edge(parent_name, &step_name);
                }
            }
        }

        // 传递归约
        graph.compute_transitive_dependencies();

        Ok(Self {
            graph,
            cache_store,
            plan,
            secrets_hash,
            platform,
        })
    }

    /// 将构建图转换为 Dockerfile 文本
    ///
    /// 1. 拓扑排序获取处理顺序
    /// 2. 依次处理每个节点
    /// 3. 生成 deploy 阶段
    /// 4. 合并所有阶段并添加 syntax header
    pub fn to_dockerfile(&mut self) -> Result<BuildGraphOutput> {
        let order = self.graph.compute_processing_order()?;

        // 处理所有节点
        for name in &order {
            self.process_node(name)?;
        }

        // 收集所有阶段的 dockerfile 片段
        let mut stages: Vec<String> = Vec::new();
        for name in &order {
            if let Some(node) = self.graph.get_node(name) {
                if !node.dockerfile_stage.is_empty() {
                    stages.push(node.dockerfile_stage.clone());
                }
            }
        }

        // 生成 deploy 阶段
        let deploy_stage = self.generate_deploy_stage(&order)?;
        stages.push(deploy_stage);

        // 收集最终输出环境（合并所有节点的 output_env）
        let output_env = self.collect_output_env(&order);

        // 拼接最终 Dockerfile
        let mut dockerfile = String::from("# syntax=docker/dockerfile:1\n\n");
        dockerfile.push_str(&stages.join("\n\n"));
        dockerfile.push('\n');

        Ok(BuildGraphOutput {
            dockerfile,
            output_env,
        })
    }

    /// 递归处理节点
    ///
    /// 1. 跳过已处理节点
    /// 2. 检测循环
    /// 3. 先递归处理所有父节点
    /// 4. 合并父节点的 output_env 到本节点的 input_env
    /// 5. 转换为 Dockerfile 阶段
    fn process_node(&mut self, name: &str) -> Result<()> {
        // 检查是否已处理
        {
            let node = self.graph.get_node(name).ok_or_else(|| {
                anyhow::anyhow!("节点未找到: {}", name)
            })?;
            if node.processed {
                return Ok(());
            }
            if node.in_progress {
                return Err(crate::ArcpackError::CycleDetected {
                    node: name.to_string(),
                });
            }
        }

        // 获取父节点列表（clone 避免借用冲突）
        let parent_names: Vec<String> = self.graph.get_parents(name).to_vec();

        // 递归处理父节点
        for parent_name in &parent_names {
            self.process_node(parent_name)?;
        }

        // 合并父节点的 output_env 到本节点的 input_env
        {
            let mut merged_env = BuildEnvironment::new();
            for parent_name in &parent_names {
                if let Some(parent_node) = self.graph.get_node(parent_name) {
                    merged_env.merge(&parent_node.output_env);
                }
            }
            if let Some(node) = self.graph.get_node_mut(name) {
                node.input_env = merged_env;
                node.in_progress = true;
            }
        }

        // 转换节点为 Dockerfile 阶段
        self.convert_node_to_dockerfile(name)?;

        // 标记完成
        if let Some(node) = self.graph.get_node_mut(name) {
            node.processed = true;
            node.in_progress = false;
        }

        Ok(())
    }

    /// 将节点转换为 Dockerfile 阶段
    ///
    /// 1. 从 layers 获取 FROM 和 COPY 指令
    /// 2. 添加 WORKDIR、ENV 指令
    /// 3. 逐条转换 commands
    /// 4. 更新 output_env
    fn convert_node_to_dockerfile(&mut self, name: &str) -> Result<()> {
        // 提取所需数据（避免借用冲突）
        let (step, input_env) = {
            let node = self.graph.get_node(name).ok_or_else(|| {
                anyhow::anyhow!("节点未找到: {}", name)
            })?;
            (node.step.clone(), node.input_env.clone())
        };

        // 从 layers 获取 FROM / COPY 指令
        let layer_result = layers::get_full_state_from_layers(&step.inputs, name);

        // 构建 Dockerfile 阶段
        let mut lines: Vec<String> = Vec::new();

        // FROM 指令
        if let Some(from) = &layer_result.from_instruction {
            lines.push(from.clone());
        }

        // WORKDIR
        lines.push("WORKDIR /app".to_string());

        // 初始化 output_env（从 input_env 开始累积）
        let mut output_env = input_env.clone();

        // 合并 step.variables 到 output_env
        for (key, value) in &step.variables {
            output_env.add_env_var(key, value);
        }

        // ENV 指令：input_env.env_vars + step.variables（后者覆盖）
        let mut env_map = input_env.env_vars.clone();
        env_map.extend(step.variables.clone());
        let mut env_pairs: Vec<_> = env_map.into_iter().collect();
        env_pairs.sort_by(|a, b| a.0.cmp(&b.0));
        for (key, value) in &env_pairs {
            lines.push(format!("ENV {}={}", key, value));
        }

        // PATH 指令（如果 input_env 有 path_list）
        if !input_env.path_list.is_empty() {
            let paths = input_env.path_list.join(":");
            lines.push(format!("ENV PATH={}:$PATH", paths));
        }

        // COPY 指令（来自 layers）
        for copy_instr in &layer_result.copy_instructions {
            lines.push(copy_instr.clone());
        }

        // 提取 secrets_hash 避免借用冲突（convert_exec_command 需要 &mut self）
        let secrets_hash = self.secrets_hash.clone();

        // 逐条转换 commands
        for cmd in &step.commands {
            match cmd {
                Command::Exec(exec) => {
                    let line = self.convert_exec_command(
                        &exec.cmd,
                        &step.caches,
                        &step.secrets,
                        &secrets_hash,
                    );
                    lines.push(line);
                }
                Command::Path(path_cmd) => {
                    output_env.push_path(&path_cmd.path);
                    let all_paths = output_env.path_list.join(":");
                    lines.push(format!("ENV PATH={}:$PATH", all_paths));
                }
                Command::Copy(copy_cmd) => {
                    let line = Self::convert_copy_command(copy_cmd);
                    lines.push(line);
                }
                Command::File(file_cmd) => {
                    let line = Self::convert_file_command(file_cmd, &step.assets);
                    lines.push(line);
                }
            }
        }

        let dockerfile_stage = lines.join("\n");

        // 写回节点
        if let Some(node) = self.graph.get_node_mut(name) {
            node.dockerfile_stage = dockerfile_stage;
            node.output_env = output_env;
        }

        Ok(())
    }

    /// 转换 Exec 命令为 RUN 指令
    ///
    /// 生成格式：RUN [--mount=type=cache,...] [--mount=type=secret,...] {cmd}
    fn convert_exec_command(
        &mut self,
        cmd: &str,
        step_caches: &[String],
        step_secrets: &[String],
        secrets_hash: &Option<String>,
    ) -> String {
        let mut mounts: Vec<String> = Vec::new();

        // 缓存挂载
        for cache_key in step_caches {
            if let Some(plan_cache) = self.plan.caches.get(cache_key) {
                let mount_opt =
                    self.cache_store
                        .get_cache_mount_option(cache_key, plan_cache);
                mounts.push(mount_opt);
            }
        }

        // Secret 挂载
        let has_secrets = match step_secrets {
            [] => false,
            s if s.contains(&"*".to_string()) => !self.plan.secrets.is_empty(),
            _ => true,
        };

        if has_secrets {
            let secret_names = if step_secrets.contains(&"*".to_string()) {
                self.plan.secrets.clone()
            } else {
                step_secrets.to_vec()
            };
            for secret_name in &secret_names {
                mounts.push(format!("--mount=type=secret,id={}", secret_name));
            }
        }

        // 构建 RUN 行
        let mut line = String::from("RUN");
        for mount in &mounts {
            line.push(' ');
            line.push_str(mount);
        }

        // 如果有 secrets 且 secrets_hash 存在，添加失效注释
        if has_secrets {
            if let Some(ref hash) = secrets_hash {
                line.push_str(&format!(" SECRETS_HASH={}", hash));
            }
        }

        line.push(' ');
        line.push_str(cmd);
        line
    }

    /// 转换 Copy 命令为 COPY 指令
    fn convert_copy_command(copy_cmd: &crate::plan::command::CopyCommand) -> String {
        if let Some(ref image) = copy_cmd.image {
            format!("COPY --from={} {} {}", image, copy_cmd.src, copy_cmd.dest)
        } else {
            format!("COPY {} {}", copy_cmd.src, copy_cmd.dest)
        }
    }

    /// 转换 File 命令为 heredoc COPY 指令
    fn convert_file_command(
        file_cmd: &crate::plan::command::FileCommand,
        step_assets: &HashMap<String, String>,
    ) -> String {
        let content = step_assets
            .get(&file_cmd.name)
            .map(|s| s.as_str())
            .unwrap_or("");
        // 检查内容是否含 EOF 行，避免 heredoc 定界符冲突
        let delimiter = if content.lines().any(|l| l.trim() == "EOF") {
            "ARCPACK_FILE_END"
        } else {
            "EOF"
        };
        format!(
            "COPY <<'{}' {}\n{}\n{}",
            delimiter, file_cmd.path, content, delimiter
        )
    }

    /// 生成 deploy 阶段
    ///
    /// 1. FROM 基础镜像
    /// 2. COPY --from 输入层
    /// 3. WORKDIR /app
    /// 4. ENV 环境变量和 PATH
    /// 5. ENTRYPOINT + CMD
    fn generate_deploy_stage(&self, order: &[String]) -> Result<String> {
        let deploy: &Deploy = &self.plan.deploy;
        let mut lines: Vec<String> = Vec::new();

        // FROM 指令
        let from_line = if let Some(ref base) = deploy.base {
            if let Some(ref image) = base.image {
                format!("FROM {} AS deploy", image)
            } else if let Some(ref step_name) = base.step {
                // 步骤引用需要 sanitize
                let safe_name = layers::sanitize_stage_name(step_name);
                format!("FROM {} AS deploy", safe_name)
            } else {
                format!("FROM {} AS deploy", ARCPACK_RUNTIME_IMAGE)
            }
        } else {
            format!("FROM {} AS deploy", ARCPACK_RUNTIME_IMAGE)
        };
        lines.push(from_line);

        // COPY --from 输入层
        for input in &deploy.inputs {
            // 步骤引用需要 sanitize，镜像引用保持原样
            let from_ref = input
                .step
                .as_ref()
                .map(|s| layers::sanitize_stage_name(s))
                .or_else(|| input.image.clone());
            if let Some(ref name) = from_ref {
                lines.extend(layers::copy_layer_paths(Some(name), &input.filter, false));
            }
        }

        // WORKDIR
        lines.push("WORKDIR /app".to_string());

        // 收集所有节点的 output_env 合并到 deploy 环境
        let mut deploy_env = BuildEnvironment::new();
        for name in order {
            if let Some(node) = self.graph.get_node(name) {
                deploy_env.merge(&node.output_env);
            }
        }

        // 合并 deploy.variables
        for (key, value) in &deploy.variables {
            deploy_env.add_env_var(key, value);
        }

        // 合并 deploy.paths
        for path in &deploy.paths {
            deploy_env.push_path(path);
        }

        // ENV 指令
        let mut env_pairs: Vec<(String, String)> = deploy_env.env_vars.into_iter().collect();
        env_pairs.sort_by(|a, b| a.0.cmp(&b.0));
        for (key, value) in &env_pairs {
            lines.push(format!("ENV {}={}", key, value));
        }

        // PATH 指令
        if !deploy_env.path_list.is_empty() {
            let paths = deploy_env.path_list.join(":");
            lines.push(format!("ENV PATH={}:$PATH", paths));
        }

        // ENTRYPOINT + CMD
        lines.push(r#"ENTRYPOINT ["/bin/bash", "-c"]"#.to_string());
        if let Some(ref start_cmd) = deploy.start_cmd {
            lines.push(format!(r#"CMD ["{}"]"#, start_cmd));
        } else {
            lines.push(r#"CMD ["/bin/bash"]"#.to_string());
        }

        Ok(lines.join("\n"))
    }

    /// 收集所有节点的 output_env 合并结果
    fn collect_output_env(&self, order: &[String]) -> BuildEnvironment {
        let mut output_env = BuildEnvironment::new();
        for name in order {
            if let Some(node) = self.graph.get_node(name) {
                output_env.merge(&node.output_env);
            }
        }
        output_env
    }

    /// 将构建图转换为 LLB Definition
    ///
    /// 1. 拓扑排序获取处理顺序
    /// 2. 依次处理每个节点（生成 LLB 操作）
    /// 3. 生成 deploy 阶段 LLB
    /// 4. 返回 (Definition, BuildEnvironment)
    #[cfg(feature = "llb")]
    pub fn to_llb(&mut self) -> Result<(pb::Definition, BuildEnvironment)> {
        let order = self.graph.compute_processing_order()?;
        for node_name in &order {
            self.process_node_llb(node_name)?;
        }
        let definition = self.build_deploy_llb(&order)?;
        let output_env = self.collect_output_env(&order);
        Ok((definition, output_env))
    }

    /// LLB 版递归处理节点
    ///
    /// 对齐 `process_node()`，差异：
    /// - 无 WORKDIR/ENV 指令（LLB 在 ExecOp.Meta 中设置）
    /// - State 以 OperationOutput 传递
    /// - 每个 Exec 必须携带完整环境变量
    #[cfg(feature = "llb")]
    fn process_node_llb(&mut self, name: &str) -> Result<()> {

        // 检查是否已处理 / 循环检测
        {
            let node = self.graph.get_node(name).ok_or_else(|| {
                anyhow::anyhow!("节点未找到: {}", name)
            })?;
            if node.processed {
                return Ok(());
            }
            if node.in_progress {
                return Err(crate::ArcpackError::CycleDetected {
                    node: name.to_string(),
                });
            }
        }

        // 获取父节点列表
        let parent_names: Vec<String> = self.graph.get_parents(name).to_vec();

        // 递归处理父节点
        for parent_name in &parent_names {
            self.process_node_llb(parent_name)?;
        }

        // 合并父节点 output_env → input_env
        {
            let mut merged_env = BuildEnvironment::new();
            for parent_name in &parent_names {
                if let Some(parent_node) = self.graph.get_node(parent_name) {
                    merged_env.merge(&parent_node.output_env);
                }
            }
            if let Some(node) = self.graph.get_node_mut(name) {
                node.input_env = merged_env;
                node.in_progress = true;
            }
        }

        // 执行可失败的转换逻辑；出错时复位 in_progress
        let result = self.do_process_node_llb(name);
        if result.is_err() {
            if let Some(node) = self.graph.get_node_mut(name) {
                node.in_progress = false;
            }
        }
        result
    }

    /// process_node_llb 的核心逻辑（拆分以便 in_progress 复位）
    #[cfg(feature = "llb")]
    fn do_process_node_llb(&mut self, name: &str) -> Result<()> {
        // 提取数据（避免借用冲突）
        let (step, input_env) = {
            let node = self.graph.get_node(name).ok_or_else(|| {
                anyhow::anyhow!("节点未找到: {}", name)
            })?;
            (node.step.clone(), node.input_env.clone())
        };

        // 获取起始 LLB state
        let starting_state = self.get_node_starting_state_llb(&step.inputs)?;
        let mut current_state = starting_state;

        // 初始化 output_env
        let mut output_env = input_env.clone();
        for (key, value) in &step.variables {
            output_env.add_env_var(key, value);
        }

        // 提取 secrets_hash（避免 &mut self 借用冲突）
        let secrets_hash = self.secrets_hash.clone();

        // 逐条转换 commands
        for cmd in &step.commands {
            match cmd {
                Command::Exec(exec) => {
                    current_state = self.convert_exec_command_llb(
                        current_state,
                        exec,
                        &step,
                        &output_env,
                        secrets_hash.as_deref(),
                    )?;
                }
                Command::Path(path_cmd) => {
                    output_env.push_path(&path_cmd.path);
                }
                Command::Copy(copy_cmd) => {
                    current_state = Self::convert_copy_command_llb(current_state, copy_cmd);
                }
                Command::File(file_cmd) => {
                    current_state = Self::convert_file_command_llb(
                        current_state,
                        file_cmd,
                        &step.assets,
                    );
                }
            }
        }

        // 存储 llb_state + output_env
        if let Some(node) = self.graph.get_node_mut(name) {
            node.set_llb_state(current_state);
            node.output_env = output_env;
            node.processed = true;
            node.in_progress = false;
        }

        Ok(())
    }

    /// 获取节点的起始 LLB state（从 layers 构建）
    #[cfg(feature = "llb")]
    fn get_node_starting_state_llb(
        &self,
        inputs: &[crate::plan::Layer],
    ) -> Result<crate::buildkit::llb::OperationOutput> {
        use crate::buildkit::llb;

        // 收集所有 step_nodes 的引用
        let step_nodes: HashMap<String, &StepNode> = self
            .graph
            .get_nodes()
            .iter()
            .map(|(name, node)| (name.clone(), node))
            .collect();

        let base_image = llb::image(crate::plan::ARCPACK_BUILDER_IMAGE);
        layers::get_full_state_from_layers_llb(inputs, &step_nodes, &base_image)
    }

    /// 系统默认 PATH（LLB ExecOp 需要显式设置）
    #[cfg(feature = "llb")]
    const DEFAULT_SYSTEM_PATH: &'static str =
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

    /// 转换 Exec 命令为 LLB ExecOp
    #[cfg(feature = "llb")]
    fn convert_exec_command_llb(
        &mut self,
        state: crate::buildkit::llb::OperationOutput,
        exec: &crate::plan::command::ExecCommand,
        step: &crate::plan::Step,
        current_env: &BuildEnvironment,
        secrets_hash: Option<&str>,
    ) -> Result<crate::buildkit::llb::OperationOutput> {
        use crate::buildkit::llb::exec::ExecBuilder;

        let args = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            exec.cmd.clone(),
        ];
        let mut builder = ExecBuilder::new(state, args).cwd("/app");

        // 注入环境变量：current_env.env_vars + step.variables
        let mut env_map = current_env.env_vars.clone();
        env_map.extend(step.variables.clone());
        let mut env_pairs: Vec<_> = env_map.into_iter().collect();
        env_pairs.sort_by(|a, b| a.0.cmp(&b.0));
        for (key, value) in &env_pairs {
            builder = builder.env(key, value);
        }

        // PATH = path_list + 系统默认
        let path_value = if current_env.path_list.is_empty() {
            Self::DEFAULT_SYSTEM_PATH.to_string()
        } else {
            format!("{}:{}", current_env.path_list.join(":"), Self::DEFAULT_SYSTEM_PATH)
        };
        builder = builder.env("PATH", &path_value);

        // 缓存挂载
        for cache_key in &step.caches {
            if let Some(plan_cache) = self.plan.caches.get(cache_key) {
                let mount_spec =
                    self.cache_store
                        .get_cache_mount_spec(cache_key, plan_cache);
                builder = builder.add_mount(mount_spec);
            }
        }

        // Secret 挂载
        let has_secrets = match step.secrets.as_slice() {
            [] => false,
            s if s.iter().any(|s| s == "*") => !self.plan.secrets.is_empty(),
            _ => true,
        };

        if has_secrets {
            let secret_names = if step.secrets.iter().any(|s| s == "*") {
                self.plan.secrets.clone()
            } else {
                step.secrets.clone()
            };
            for secret_name in &secret_names {
                builder = builder.add_secret_env(secret_name, secret_name);
            }

            // 有 secrets 且有 secrets_hash → 注入 _SECRET_HASH env
            if let Some(hash) = secrets_hash {
                builder = builder.env("_SECRET_HASH", hash);
            }
        }

        // 自定义描述
        if let Some(ref custom_name) = exec.custom_name {
            builder = builder.description(custom_name);
        }

        Ok(builder.root())
    }

    /// 转换 Copy 命令为 LLB copy 操作
    #[cfg(feature = "llb")]
    fn convert_copy_command_llb(
        state: crate::buildkit::llb::OperationOutput,
        copy_cmd: &crate::plan::command::CopyCommand,
    ) -> crate::buildkit::llb::OperationOutput {
        use crate::buildkit::llb;

        let src = if let Some(ref image_name) = copy_cmd.image {
            llb::image(image_name)
        } else {
            llb::local("context")
        };
        llb::copy(src, &copy_cmd.src, state, &copy_cmd.dest)
    }

    /// 转换 File 命令为 LLB make_file 操作
    #[cfg(feature = "llb")]
    fn convert_file_command_llb(
        state: crate::buildkit::llb::OperationOutput,
        file_cmd: &crate::plan::command::FileCommand,
        step_assets: &HashMap<String, String>,
    ) -> crate::buildkit::llb::OperationOutput {
        use crate::buildkit::llb;

        let content = step_assets
            .get(&file_cmd.name)
            .map(|s| s.as_bytes())
            .unwrap_or(b"");
        let mode = file_cmd.mode.map(|m| m as i32).unwrap_or(0o644);
        llb::make_file(state, &file_cmd.path, content, mode)
    }

    /// 生成 deploy 阶段 LLB 并序列化为 Definition
    ///
    /// 1. 创建 deploy 基础镜像 State
    /// 2. 遍历 deploy.inputs，从对应 step 的 llb_state copy 到 deploy state
    /// 3. marshal 序列化为 pb::Definition
    ///
    /// 注意：ENV/WORKDIR/CMD/ENTRYPOINT 不在 LLB 中表达，
    /// 由 build_image_config() 生成 OCI ImageConfig。
    #[cfg(feature = "llb")]
    fn build_deploy_llb(&self, _order: &[String]) -> Result<pb::Definition> {
        use crate::buildkit::llb;

        let deploy = &self.plan.deploy;

        // 1. 创建 deploy 基础镜像 State
        let mut state = if let Some(ref base) = deploy.base {
            if let Some(ref image_name) = base.image {
                llb::image(image_name)
            } else if let Some(ref step_name) = base.step {
                // 从 step 的 llb_state 获取
                let node = self.graph.get_node(step_name).ok_or_else(|| {
                    anyhow::anyhow!("deploy.base 引用不存在的 step: {}", step_name)
                })?;
                node.get_llb_state().cloned().ok_or_else(|| {
                    anyhow::anyhow!("deploy.base step {} 尚未生成 llb_state", step_name)
                })?
            } else {
                llb::image(ARCPACK_RUNTIME_IMAGE)
            }
        } else {
            llb::image(ARCPACK_RUNTIME_IMAGE)
        };

        // 2. 遍历 deploy.inputs，从对应 step/image 的 state copy 到 deploy state
        for input in &deploy.inputs {
            let src_state = if let Some(ref step_name) = input.step {
                let node = self.graph.get_node(step_name).ok_or_else(|| {
                    anyhow::anyhow!("deploy.input 引用不存在的 step: {}", step_name)
                })?;
                node.get_llb_state().cloned().ok_or_else(|| {
                    anyhow::anyhow!("deploy.input step {} 尚未生成 llb_state", step_name)
                })?
            } else if let Some(ref image_name) = input.image {
                llb::image(image_name)
            } else {
                continue;
            };

            // 有 filter → 按 resolve_paths 逐路径 copy；无 filter → copy /app → /app
            if input.filter.include.is_empty() {
                state = llb::copy(src_state, "/app", state, "/app");
            } else {
                let is_local = input.local == Some(true);
                for path in &input.filter.include {
                    let (src_path, dest_path) = layers::resolve_paths(path, is_local);
                    state = llb::copy(src_state.clone(), &src_path, state, &dest_path);
                }
            }
        }

        // 3. 序列化为 Definition
        llb::marshal(&state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{
        BuildPlan, Command, Deploy, Filter, Layer, Step,
        ARCPACK_BUILDER_IMAGE, ARCPACK_RUNTIME_IMAGE,
    };

    /// 辅助函数：创建默认 Platform
    fn default_platform() -> Platform {
        Platform::default()
    }

    /// 辅助函数：创建空 cache_store
    fn empty_cache_store() -> BuildKitCacheStore {
        BuildKitCacheStore::new("")
    }

    // 1. 验证 BuildGraph::new 创建节点
    #[test]
    fn test_build_graph_new_creates_nodes() {
        let mut plan = BuildPlan::new();
        plan.add_step(Step::new("packages"));
        plan.add_step(Step::new("install"));

        let bg = BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
        assert!(bg.graph.get_node("packages").is_some(), "packages 节点应存在");
        assert!(bg.graph.get_node("install").is_some(), "install 节点应存在");
    }

    // 2. 验证 BuildGraph::new 根据 inputs 添加边
    #[test]
    fn test_build_graph_new_adds_edges() {
        let mut plan = BuildPlan::new();
        plan.add_step(Step::new("packages"));

        let mut install = Step::new("install");
        install.inputs.push(Layer::new_step_layer("packages", None));
        plan.add_step(install);

        let bg = BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
        let parents = bg.graph.get_parents("install");
        assert!(
            parents.contains(&"packages".to_string()),
            "install 的父节点应包含 packages"
        );
    }

    // 3. 三步 plan 生成有效 Dockerfile
    #[test]
    fn test_to_dockerfile_simple_plan() {
        let mut plan = BuildPlan::new();

        // packages 步骤：FROM builder image
        let mut packages = Step::new("packages");
        packages
            .inputs
            .push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
        packages.commands.push(Command::new_path("/usr/local/bin"));
        plan.add_step(packages);

        // install 步骤：依赖 packages
        let mut install = Step::new("install");
        install
            .inputs
            .push(Layer::new_step_layer("packages", None));
        install.commands.push(Command::new_exec("npm install"));
        plan.add_step(install);

        // build 步骤：依赖 install + local
        let mut build = Step::new("build");
        build
            .inputs
            .push(Layer::new_step_layer("install", None));
        build.inputs.push(Layer::new_local_layer());
        build.commands.push(Command::new_exec("npm run build"));
        plan.add_step(build);

        // deploy 配置
        plan.deploy = Deploy {
            base: Some(Layer::new_image_layer(ARCPACK_RUNTIME_IMAGE, None)),
            inputs: vec![Layer::new_step_layer(
                "build",
                Some(Filter::include_only(vec![".".to_string()])),
            )],
            start_cmd: Some("node server.js".to_string()),
            variables: HashMap::new(),
            paths: vec![],
        };

        let mut bg =
            BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
        let output = bg.to_dockerfile().unwrap();

        // 验证关键内容
        assert!(
            output.dockerfile.contains("FROM"),
            "Dockerfile 应包含 FROM 指令"
        );
        assert!(
            output.dockerfile.contains("RUN npm install"),
            "Dockerfile 应包含 npm install"
        );
        assert!(
            output.dockerfile.contains("RUN npm run build"),
            "Dockerfile 应包含 npm run build"
        );
        assert!(
            output.dockerfile.contains("AS deploy"),
            "Dockerfile 应包含 deploy 阶段"
        );
        assert!(
            output.dockerfile.contains("node server.js"),
            "Dockerfile 应包含启动命令"
        );
    }

    // 4. 输出以 syntax header 开头
    #[test]
    fn test_dockerfile_has_syntax_header() {
        let mut plan = BuildPlan::new();
        let mut step = Step::new("setup");
        step.inputs
            .push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
        plan.add_step(step);

        let mut bg =
            BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
        let output = bg.to_dockerfile().unwrap();
        assert!(
            output.dockerfile.starts_with("# syntax=docker/dockerfile:1"),
            "Dockerfile 应以 syntax header 开头"
        );
    }

    // 5. Exec 命令生成 RUN 指令
    #[test]
    fn test_exec_command_generates_run() {
        let mut plan = BuildPlan::new();
        let mut step = Step::new("install");
        step.inputs
            .push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
        step.commands.push(Command::new_exec("apt-get update"));
        plan.add_step(step);

        let mut bg =
            BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
        let output = bg.to_dockerfile().unwrap();
        assert!(
            output.dockerfile.contains("RUN apt-get update"),
            "应包含 RUN apt-get update"
        );
    }

    // 6. Copy 命令生成 COPY --from 指令
    #[test]
    fn test_copy_command_generates_copy() {
        let mut plan = BuildPlan::new();
        let mut step = Step::new("setup");
        step.inputs
            .push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
        step.commands.push(Command::Copy(crate::plan::command::CopyCommand {
            image: Some("golang:1.21".to_string()),
            src: "/usr/local/go".to_string(),
            dest: "/usr/local/go".to_string(),
        }));
        plan.add_step(step);

        let mut bg =
            BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
        let output = bg.to_dockerfile().unwrap();
        assert!(
            output.dockerfile.contains("COPY --from=golang:1.21 /usr/local/go /usr/local/go"),
            "应包含 COPY --from 指令"
        );
    }

    // 7. Path 命令生成 ENV PATH 指令
    #[test]
    fn test_path_command_generates_env() {
        let mut plan = BuildPlan::new();
        let mut step = Step::new("setup");
        step.inputs
            .push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
        step.commands
            .push(Command::new_path("/usr/local/go/bin"));
        plan.add_step(step);

        let mut bg =
            BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
        let output = bg.to_dockerfile().unwrap();
        assert!(
            output.dockerfile.contains("ENV PATH=/usr/local/go/bin:$PATH"),
            "应包含 ENV PATH 指令"
        );
    }

    // 8. File 命令生成 heredoc COPY
    #[test]
    fn test_file_command_generates_heredoc() {
        let mut plan = BuildPlan::new();
        let mut step = Step::new("config");
        step.inputs
            .push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
        step.commands
            .push(Command::new_file("/etc/config.toml", "myconfig"));
        step.assets
            .insert("myconfig".to_string(), "[settings]\nkey = \"value\"".to_string());
        plan.add_step(step);

        let mut bg =
            BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
        let output = bg.to_dockerfile().unwrap();
        assert!(
            output.dockerfile.contains("COPY <<'EOF' /etc/config.toml"),
            "应包含 heredoc COPY 指令"
        );
        assert!(
            output.dockerfile.contains("[settings]"),
            "应包含文件内容"
        );
        assert!(
            output.dockerfile.contains("EOF"),
            "应包含 EOF 终止符"
        );
    }

    // 9. 父节点的 PATH 传播到子节点
    #[test]
    fn test_env_propagation_parent_to_child() {
        let mut plan = BuildPlan::new();

        // 父节点设置 PATH
        let mut parent = Step::new("parent");
        parent
            .inputs
            .push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
        parent.commands.push(Command::new_path("/custom/bin"));
        plan.add_step(parent);

        // 子节点依赖父节点
        let mut child = Step::new("child");
        child.inputs.push(Layer::new_step_layer("parent", None));
        child.commands.push(Command::new_exec("echo hello"));
        plan.add_step(child);

        let mut bg =
            BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
        let output = bg.to_dockerfile().unwrap();

        // 在 child 阶段中应能看到父节点传播的 PATH
        let child_stage_start = output.dockerfile.find("AS child").expect("应有 child 阶段");
        let child_stage = &output.dockerfile[child_stage_start..];
        assert!(
            child_stage.contains("ENV PATH=/custom/bin:$PATH"),
            "子节点应继承父节点的 PATH，实际内容:\n{}",
            child_stage
        );
    }

    // 10. deploy 阶段包含 CMD
    #[test]
    fn test_deploy_stage_has_cmd() {
        let mut plan = BuildPlan::new();
        let mut step = Step::new("build");
        step.inputs
            .push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
        plan.add_step(step);

        plan.deploy = Deploy {
            base: Some(Layer::new_image_layer(ARCPACK_RUNTIME_IMAGE, None)),
            inputs: vec![],
            start_cmd: Some("node server.js".to_string()),
            variables: HashMap::new(),
            paths: vec![],
        };

        let mut bg =
            BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
        let output = bg.to_dockerfile().unwrap();
        assert!(
            output.dockerfile.contains(r#"CMD ["node server.js"]"#),
            "deploy 阶段应包含 CMD"
        );
    }

    // 11. 无 start_cmd 时默认 /bin/bash
    #[test]
    fn test_deploy_stage_no_start_cmd_defaults_bash() {
        let mut plan = BuildPlan::new();
        let mut step = Step::new("build");
        step.inputs
            .push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
        plan.add_step(step);

        plan.deploy = Deploy {
            base: Some(Layer::new_image_layer(ARCPACK_RUNTIME_IMAGE, None)),
            inputs: vec![],
            start_cmd: None,
            variables: HashMap::new(),
            paths: vec![],
        };

        let mut bg =
            BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
        let output = bg.to_dockerfile().unwrap();
        assert!(
            output.dockerfile.contains(r#"CMD ["/bin/bash"]"#),
            "无 start_cmd 时应默认 CMD [\"/bin/bash\"]"
        );
    }

    // 12. File 命令内容含 EOF 时使用备选定界符
    #[test]
    fn test_file_command_with_eof_in_content_uses_alternative_delimiter() {
        let mut plan = BuildPlan::new();
        let mut step = Step::new("config");
        step.inputs
            .push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
        step.commands
            .push(Command::new_file("/etc/script.sh", "myscript"));
        step.assets.insert(
            "myscript".to_string(),
            "#!/bin/bash\ncat <<EOF\nhello\nEOF\necho done".to_string(),
        );
        plan.add_step(step);

        let mut bg =
            BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
        let output = bg.to_dockerfile().unwrap();
        assert!(
            output.dockerfile.contains("ARCPACK_FILE_END"),
            "内容含 EOF 时应使用 ARCPACK_FILE_END 定界符"
        );
        assert!(
            !output.dockerfile.contains("COPY <<'EOF'"),
            "不应使用 EOF 作为定界符"
        );
    }

    // === LLB 测试 ===

    #[cfg(feature = "llb")]
    mod llb_tests {
        use super::*;
        use crate::plan::Cache;
        use prost::Message;

        /// 辅助函数：创建单步 plan + graph 并执行 process_node_llb
        fn build_single_step_graph(step: Step) -> BuildGraph {
            let mut plan = BuildPlan::new();
            plan.add_step(step);
            BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap()
        }

        #[test]
        fn test_process_node_llb_sets_state() {
            let mut step = Step::new("install");
            step.inputs.push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
            step.commands.push(Command::new_exec("npm install"));

            let mut bg = build_single_step_graph(step);
            bg.process_node_llb("install").unwrap();

            let node = bg.graph.get_node("install").unwrap();
            assert!(node.llb_state.is_some(), "处理后 llb_state 应为 Some");
            assert!(node.processed, "处理后 processed 应为 true");
        }

        #[test]
        fn test_process_node_llb_env_propagation() {
            let mut plan = BuildPlan::new();

            // 父节点设置 PATH
            let mut parent = Step::new("parent");
            parent.inputs.push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
            parent.commands.push(Command::new_path("/custom/bin"));
            plan.add_step(parent);

            // 子节点依赖父节点
            let mut child = Step::new("child");
            child.inputs.push(Layer::new_step_layer("parent", None));
            child.commands.push(Command::new_exec("echo hello"));
            plan.add_step(child);

            let mut bg = BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
            bg.process_node_llb("parent").unwrap();
            bg.process_node_llb("child").unwrap();

            let child_node = bg.graph.get_node("child").unwrap();
            assert!(
                child_node.output_env.path_list.contains(&"/custom/bin".to_string()),
                "子节点应继承父节点的 PATH"
            );
        }

        #[test]
        fn test_process_node_llb_cycle_detection() {
            let mut plan = BuildPlan::new();
            let mut step_a = Step::new("a");
            step_a.inputs.push(Layer::new_step_layer("b", None));
            plan.add_step(step_a);

            let mut step_b = Step::new("b");
            step_b.inputs.push(Layer::new_step_layer("a", None));
            plan.add_step(step_b);

            let mut bg = BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
            // 手动设置 in_progress 模拟循环
            if let Some(node) = bg.graph.get_node_mut("b") {
                node.in_progress = true;
            }
            let result = bg.process_node_llb("b");
            assert!(result.is_err(), "循环应返回错误");
        }

        #[test]
        fn test_exec_llb_basic_args() {
            let mut step = Step::new("install");
            step.inputs.push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
            step.commands.push(Command::new_exec("npm install"));

            let mut bg = build_single_step_graph(step);
            bg.process_node_llb("install").unwrap();

            let node = bg.graph.get_node("install").unwrap();
            let state = node.get_llb_state().unwrap();
            let op = crate::buildkit::proto::pb::Op::decode(
                state.serialized_op.bytes.as_slice()
            ).unwrap();
            if let Some(crate::buildkit::proto::pb::op::Op::Exec(exec_op)) = &op.op {
                let meta = exec_op.meta.as_ref().unwrap();
                assert_eq!(meta.args, vec!["/bin/sh", "-c", "npm install"]);
                assert_eq!(meta.cwd, "/app");
            } else {
                panic!("应为 ExecOp");
            }
        }

        #[test]
        fn test_exec_llb_env_vars() {
            let mut step = Step::new("build");
            step.inputs.push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
            step.variables.insert("NODE_ENV".to_string(), "production".to_string());
            step.commands.push(Command::new_exec("npm run build"));

            let mut bg = build_single_step_graph(step);
            bg.process_node_llb("build").unwrap();

            let node = bg.graph.get_node("build").unwrap();
            let state = node.get_llb_state().unwrap();
            let op = crate::buildkit::proto::pb::Op::decode(
                state.serialized_op.bytes.as_slice()
            ).unwrap();
            if let Some(crate::buildkit::proto::pb::op::Op::Exec(exec_op)) = &op.op {
                let meta = exec_op.meta.as_ref().unwrap();
                assert!(
                    meta.env.contains(&"NODE_ENV=production".to_string()),
                    "应包含 NODE_ENV env var"
                );
            } else {
                panic!("应为 ExecOp");
            }
        }

        #[test]
        fn test_exec_llb_path_env() {
            let mut plan = BuildPlan::new();
            let mut step = Step::new("setup");
            step.inputs.push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
            step.commands.push(Command::new_path("/custom/bin"));
            step.commands.push(Command::new_exec("echo test"));
            plan.add_step(step);

            let mut bg = BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
            bg.process_node_llb("setup").unwrap();

            let node = bg.graph.get_node("setup").unwrap();
            let state = node.get_llb_state().unwrap();
            let op = crate::buildkit::proto::pb::Op::decode(
                state.serialized_op.bytes.as_slice()
            ).unwrap();
            if let Some(crate::buildkit::proto::pb::op::Op::Exec(exec_op)) = &op.op {
                let meta = exec_op.meta.as_ref().unwrap();
                let path_env = meta.env.iter()
                    .find(|e| e.starts_with("PATH="))
                    .expect("应有 PATH 环境变量");
                assert!(
                    path_env.contains("/custom/bin"),
                    "PATH 应包含 /custom/bin"
                );
                assert!(
                    path_env.contains("/usr/local/bin"),
                    "PATH 应包含系统默认路径"
                );
            } else {
                panic!("应为 ExecOp");
            }
        }

        #[test]
        fn test_exec_llb_cache_mount() {
            let mut plan = BuildPlan::new();
            plan.caches.insert("npm".to_string(), Cache::new("/root/.npm"));

            let mut step = Step::new("install");
            step.inputs.push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
            step.caches.push("npm".to_string());
            step.commands.push(Command::new_exec("npm install"));
            plan.add_step(step);

            let mut bg = BuildGraph::new(
                plan,
                BuildKitCacheStore::new("test"),
                None,
                default_platform(),
            ).unwrap();
            bg.process_node_llb("install").unwrap();

            let node = bg.graph.get_node("install").unwrap();
            let state = node.get_llb_state().unwrap();
            let op = crate::buildkit::proto::pb::Op::decode(
                state.serialized_op.bytes.as_slice()
            ).unwrap();
            if let Some(crate::buildkit::proto::pb::op::Op::Exec(exec_op)) = &op.op {
                // rootfs mount + cache mount = 2
                assert!(exec_op.mounts.len() >= 2, "应有至少 2 个 mount（rootfs + cache）");
                let cache_mount = exec_op.mounts.iter()
                    .find(|m| m.mount_type == crate::buildkit::proto::pb::MountType::Cache as i32)
                    .expect("应有 Cache mount");
                assert_eq!(cache_mount.dest, "/root/.npm");
            } else {
                panic!("应为 ExecOp");
            }
        }

        #[test]
        fn test_exec_llb_secret_mount() {
            let mut plan = BuildPlan::new();
            plan.secrets.push("MY_TOKEN".to_string());

            let mut step = Step::new("deploy");
            step.inputs.push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
            // 默认 secrets = ["*"]
            step.commands.push(Command::new_exec("deploy"));
            plan.add_step(step);

            let mut bg = BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
            bg.process_node_llb("deploy").unwrap();

            let node = bg.graph.get_node("deploy").unwrap();
            let state = node.get_llb_state().unwrap();
            let op = crate::buildkit::proto::pb::Op::decode(
                state.serialized_op.bytes.as_slice()
            ).unwrap();
            if let Some(crate::buildkit::proto::pb::op::Op::Exec(exec_op)) = &op.op {
                assert_eq!(exec_op.secretenv.len(), 1, "应有 1 个 secret env");
                assert_eq!(exec_op.secretenv[0].id, "MY_TOKEN");
                assert_eq!(exec_op.secretenv[0].name, "MY_TOKEN");
            } else {
                panic!("应为 ExecOp");
            }
        }

        #[test]
        fn test_exec_llb_secret_wildcard() {
            let mut plan = BuildPlan::new();
            plan.secrets.push("SECRET_A".to_string());
            plan.secrets.push("SECRET_B".to_string());

            let mut step = Step::new("run");
            step.inputs.push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
            // 默认 secrets = ["*"]，会展开为所有 plan secrets
            step.commands.push(Command::new_exec("run"));
            plan.add_step(step);

            let mut bg = BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap();
            bg.process_node_llb("run").unwrap();

            let node = bg.graph.get_node("run").unwrap();
            let state = node.get_llb_state().unwrap();
            let op = crate::buildkit::proto::pb::Op::decode(
                state.serialized_op.bytes.as_slice()
            ).unwrap();
            if let Some(crate::buildkit::proto::pb::op::Op::Exec(exec_op)) = &op.op {
                assert_eq!(exec_op.secretenv.len(), 2, "* 应展开为 2 个 secrets");
            } else {
                panic!("应为 ExecOp");
            }
        }

        #[test]
        fn test_exec_llb_secret_hash() {
            let mut plan = BuildPlan::new();
            plan.secrets.push("TOKEN".to_string());

            let mut step = Step::new("run");
            step.inputs.push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
            step.commands.push(Command::new_exec("run"));
            plan.add_step(step);

            let mut bg = BuildGraph::new(
                plan,
                empty_cache_store(),
                Some("abc123".to_string()),
                default_platform(),
            ).unwrap();
            bg.process_node_llb("run").unwrap();

            let node = bg.graph.get_node("run").unwrap();
            let state = node.get_llb_state().unwrap();
            let op = crate::buildkit::proto::pb::Op::decode(
                state.serialized_op.bytes.as_slice()
            ).unwrap();
            if let Some(crate::buildkit::proto::pb::op::Op::Exec(exec_op)) = &op.op {
                let meta = exec_op.meta.as_ref().unwrap();
                assert!(
                    meta.env.contains(&"_SECRET_HASH=abc123".to_string()),
                    "有 secrets 时应注入 _SECRET_HASH"
                );
            } else {
                panic!("应为 ExecOp");
            }
        }

        #[test]
        fn test_copy_llb_from_image() {
            let mut step = Step::new("setup");
            step.inputs.push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
            step.commands.push(Command::Copy(crate::plan::command::CopyCommand {
                image: Some("golang:1.21".to_string()),
                src: "/usr/local/go".to_string(),
                dest: "/usr/local/go".to_string(),
            }));

            let mut bg = build_single_step_graph(step);
            bg.process_node_llb("setup").unwrap();

            let node = bg.graph.get_node("setup").unwrap();
            let state = node.get_llb_state().unwrap();
            let op = crate::buildkit::proto::pb::Op::decode(
                state.serialized_op.bytes.as_slice()
            ).unwrap();
            assert!(matches!(op.op, Some(crate::buildkit::proto::pb::op::Op::File(_))),
                "Copy 命令应生成 FileOp");
        }

        #[test]
        fn test_copy_llb_from_local() {
            let mut step = Step::new("setup");
            step.inputs.push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
            step.commands.push(Command::Copy(crate::plan::command::CopyCommand {
                image: None,
                src: "package.json".to_string(),
                dest: "/app/package.json".to_string(),
            }));

            let mut bg = build_single_step_graph(step);
            bg.process_node_llb("setup").unwrap();

            let node = bg.graph.get_node("setup").unwrap();
            assert!(node.llb_state.is_some(), "应设置 llb_state");
        }

        #[test]
        fn test_file_llb_content_and_mode() {
            let mut step = Step::new("config");
            step.inputs.push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
            step.commands.push(Command::File(crate::plan::command::FileCommand {
                path: "/app/config.toml".to_string(),
                name: "config".to_string(),
                mode: Some(0o755),
                custom_name: None,
            }));
            step.assets.insert("config".to_string(), "key = \"value\"".to_string());

            let mut bg = build_single_step_graph(step);
            bg.process_node_llb("config").unwrap();

            let node = bg.graph.get_node("config").unwrap();
            let state = node.get_llb_state().unwrap();
            let op = crate::buildkit::proto::pb::Op::decode(
                state.serialized_op.bytes.as_slice()
            ).unwrap();
            if let Some(crate::buildkit::proto::pb::op::Op::File(file_op)) = &op.op {
                if let Some(crate::buildkit::proto::pb::file_action::Action::Mkfile(mkfile)) =
                    &file_op.actions[0].action
                {
                    assert_eq!(mkfile.path, "/app/config.toml");
                    assert_eq!(mkfile.data, b"key = \"value\"");
                    assert_eq!(mkfile.mode, 0o755);
                } else {
                    panic!("应为 MkFile action");
                }
            } else {
                panic!("应为 FileOp");
            }
        }

        // === deploy LLB 测试 ===

        /// 辅助函数：创建完整 plan（packages → install → build → deploy）并运行 to_llb
        fn build_full_plan_llb(deploy: Deploy) -> BuildGraph {
            let mut plan = BuildPlan::new();

            let mut packages = Step::new("packages");
            packages.inputs.push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
            plan.add_step(packages);

            let mut install = Step::new("install");
            install.inputs.push(Layer::new_step_layer("packages", None));
            install.commands.push(Command::new_exec("npm install"));
            plan.add_step(install);

            let mut build = Step::new("build");
            build.inputs.push(Layer::new_step_layer("install", None));
            build.inputs.push(Layer::new_local_layer());
            build.commands.push(Command::new_exec("npm run build"));
            plan.add_step(build);

            plan.deploy = deploy;
            BuildGraph::new(plan, empty_cache_store(), None, default_platform()).unwrap()
        }

        #[test]
        fn test_deploy_llb_default_runtime_image() {
            let deploy = Deploy {
                base: None,
                inputs: vec![Layer::new_step_layer("build", None)],
                start_cmd: Some("node server.js".to_string()),
                ..Default::default()
            };
            let mut bg = build_full_plan_llb(deploy);
            let (def, _env) = bg.to_llb().unwrap();

            // Definition 不为空
            assert!(!def.def.is_empty(), "Definition.def 不应为空");

            // 应包含 SourceOp 引用 ARCPACK_RUNTIME_IMAGE
            let has_runtime_image = def.def.iter().any(|bytes| {
                if let Ok(op) = crate::buildkit::proto::pb::Op::decode(bytes.as_slice()) {
                    if let Some(crate::buildkit::proto::pb::op::Op::Source(src)) = &op.op {
                        return src.identifier.contains("railpack-runtime");
                    }
                }
                false
            });
            assert!(has_runtime_image, "无 base 时应使用 ARCPACK_RUNTIME_IMAGE");
        }

        #[test]
        fn test_deploy_llb_custom_base_image() {
            let deploy = Deploy {
                base: Some(Layer::new_image_layer("ubuntu:22.04", None)),
                inputs: vec![Layer::new_step_layer("build", None)],
                start_cmd: Some("node server.js".to_string()),
                ..Default::default()
            };
            let mut bg = build_full_plan_llb(deploy);
            let (def, _env) = bg.to_llb().unwrap();

            let has_ubuntu = def.def.iter().any(|bytes| {
                if let Ok(op) = crate::buildkit::proto::pb::Op::decode(bytes.as_slice()) {
                    if let Some(crate::buildkit::proto::pb::op::Op::Source(src)) = &op.op {
                        return src.identifier.contains("ubuntu:22.04");
                    }
                }
                false
            });
            assert!(has_ubuntu, "应使用自定义 base image ubuntu:22.04");
        }

        #[test]
        fn test_deploy_llb_copies_from_step() {
            let deploy = Deploy {
                base: Some(Layer::new_image_layer(ARCPACK_RUNTIME_IMAGE, None)),
                inputs: vec![Layer::new_step_layer("build", None)],
                start_cmd: Some("node server.js".to_string()),
                ..Default::default()
            };
            let mut bg = build_full_plan_llb(deploy);
            let (def, _env) = bg.to_llb().unwrap();

            // 应包含 FileOp（copy 操作）
            let has_copy = def.def.iter().any(|bytes| {
                if let Ok(op) = crate::buildkit::proto::pb::Op::decode(bytes.as_slice()) {
                    return matches!(op.op, Some(crate::buildkit::proto::pb::op::Op::File(_)));
                }
                false
            });
            assert!(has_copy, "deploy 应包含从 step 复制的 FileOp");
        }

        #[test]
        fn test_deploy_llb_with_filter() {
            let deploy = Deploy {
                base: Some(Layer::new_image_layer(ARCPACK_RUNTIME_IMAGE, None)),
                inputs: vec![Layer::new_step_layer(
                    "build",
                    Some(Filter::include_only(vec![".".to_string()])),
                )],
                start_cmd: Some("node server.js".to_string()),
                ..Default::default()
            };
            let mut bg = build_full_plan_llb(deploy);
            let (def, _env) = bg.to_llb().unwrap();
            assert!(!def.def.is_empty(), "有 filter 的 deploy 也应生成非空 Definition");
        }

        #[test]
        fn test_deploy_llb_marshal_non_empty() {
            let deploy = Deploy {
                base: Some(Layer::new_image_layer(ARCPACK_RUNTIME_IMAGE, None)),
                inputs: vec![Layer::new_step_layer(
                    "build",
                    Some(Filter::include_only(vec![".".to_string()])),
                )],
                start_cmd: Some("node server.js".to_string()),
                ..Default::default()
            };
            let mut bg = build_full_plan_llb(deploy);
            let (def, _env) = bg.to_llb().unwrap();

            // 验证 Definition 可被 prost 序列化
            let encoded = def.encode_to_vec();
            assert!(!encoded.is_empty(), "序列化后的 Definition 不应为空");

            // 验证可反序列化
            let decoded = crate::buildkit::proto::pb::Definition::decode(
                encoded.as_slice()
            ).unwrap();
            assert_eq!(decoded.def.len(), def.def.len());
        }
    }
}
