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
        let mut output_env = BuildEnvironment::new();
        for name in &order {
            if let Some(node) = self.graph.get_node(name) {
                output_env.merge(&node.output_env);
            }
        }

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
                        .get_cache_mount_option(cache_key, &plan_cache.clone());
                mounts.push(mount_opt);
            }
        }

        // Secret 挂载
        let has_secrets = match step_secrets {
            s if s.is_empty() => false,
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
            let from_ref: Option<String> = if let Some(ref step) = input.step {
                Some(layers::sanitize_stage_name(step))
            } else {
                input.image.clone()
            };
            if let Some(ref name) = from_ref {
                let copies = layers::copy_layer_paths(Some(name), &input.filter, false);
                lines.extend(copies);
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
}
