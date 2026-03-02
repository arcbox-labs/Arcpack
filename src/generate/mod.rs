pub mod cache_context;
pub mod command_step_builder;
pub mod deploy_builder;
pub mod image_step_builder;
pub mod install_bin_builder;
pub mod log_collector;
pub mod metadata;
pub mod mise_step_builder;

use std::any::Any;
use std::collections::HashMap;

use crate::app::environment::Environment;
use crate::app::App;
use crate::config::Config;
use crate::plan::dockerignore::DockerignoreContext;
use crate::plan::{
    spread, spread_strings, BuildPlan, Command, Filter, Layer, ARCPACK_BUILDER_IMAGE,
};
use crate::resolver::{ResolvedPackage, Resolver, VersionResolver};
use crate::Result;

use cache_context::CacheContext;
use command_step_builder::CommandStepBuilder;
use deploy_builder::DeployBuilder;
use image_step_builder::ImageStepBuilder;
use log_collector::LogCollector;
use metadata::Metadata;
use mise_step_builder::MiseStepBuilder;

/// 构建步骤选项（传递给 StepBuilder::build）
pub struct BuildStepOptions {
    pub resolved_packages: HashMap<String, ResolvedPackage>,
    pub caches: CacheContext,
}

impl BuildStepOptions {
    /// 生成 apt-get install 命令（去重 + 排序）
    pub fn new_apt_install_command(packages: &[String]) -> Command {
        let mut pkgs: Vec<String> = packages.to_vec();
        pkgs.sort();
        pkgs.dedup();

        let pkg_list = pkgs.join(" ");
        Command::Exec(crate::plan::command::ExecCommand {
            cmd: format!("sh -c 'apt-get update && apt-get install -y {}'", pkg_list),
            custom_name: Some(format!("install apt packages: {}", pkg_list)),
        })
    }
}

/// StepBuilder trait：构建步骤的抽象接口
pub trait StepBuilder: Send + Sync {
    fn name(&self) -> &str;
    fn build(&self, plan: &mut BuildPlan, options: &mut BuildStepOptions) -> Result<()>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// GenerateContext：Provider 操作的核心上下文对象
pub struct GenerateContext {
    pub app: App,
    pub env: Environment,
    pub config: Config,
    pub dockerignore_ctx: DockerignoreContext,
    pub base_image: String,
    pub steps: Vec<Box<dyn StepBuilder>>,
    pub deploy: DeployBuilder,
    pub caches: CacheContext,
    pub secrets: Vec<String>,
    pub sub_contexts: Vec<String>,
    pub metadata: Metadata,
    pub logs: LogCollector,
    pub resolver: Resolver,
    pub mise_step_builder: Option<MiseStepBuilder>,
    /// 额外的命名 MiseStepBuilder（如 Java 运行时 JDK）
    pub additional_mise_builders: Vec<(String, MiseStepBuilder)>,
}

impl GenerateContext {
    /// 创建新的 GenerateContext
    pub fn new(
        app: App,
        env: Environment,
        config: Config,
        version_resolver: Box<dyn VersionResolver>,
    ) -> Result<Self> {
        let dockerignore_ctx = DockerignoreContext::new(app.source())?;

        let mut ctx = Self {
            app,
            env,
            config,
            dockerignore_ctx: dockerignore_ctx.clone(),
            base_image: ARCPACK_BUILDER_IMAGE.to_string(),
            steps: Vec::new(),
            deploy: DeployBuilder::new(),
            caches: CacheContext::new(),
            secrets: Vec::new(),
            sub_contexts: Vec::new(),
            metadata: Metadata::new(),
            logs: LogCollector::new(),
            resolver: Resolver::new(version_resolver),
            mise_step_builder: None,
            additional_mise_builders: Vec::new(),
        };

        if dockerignore_ctx.has_file {
            ctx.metadata.set_bool("dockerIgnore", true);
        }

        ctx.apply_packages_from_config();

        Ok(ctx)
    }

    /// 获取或懒创建 MiseStepBuilder
    pub fn get_mise_step_builder(&mut self) -> &mut MiseStepBuilder {
        if self.mise_step_builder.is_none() {
            self.mise_step_builder = Some(MiseStepBuilder::new(
                mise_step_builder::MISE_STEP_NAME,
                &self.config,
            ));
        }
        self.mise_step_builder.as_mut().unwrap()
    }

    /// 创建命名的 MiseStepBuilder（用于 Java 运行时 JDK 等场景）
    pub fn new_named_mise_step_builder(&mut self, name: &str) -> &mut MiseStepBuilder {
        self.additional_mise_builders
            .push((name.to_string(), MiseStepBuilder::new(name, &self.config)));
        &mut self.additional_mise_builders.last_mut().unwrap().1
    }

    /// 进入子上下文
    pub fn enter_sub_context(&mut self, name: &str) {
        self.sub_contexts.push(name.to_string());
    }

    /// 退出子上下文
    pub fn exit_sub_context(&mut self) {
        self.sub_contexts.pop();
    }

    /// 获取步骤名（加子上下文后缀）
    pub fn get_step_name(&self, name: &str) -> String {
        if self.sub_contexts.is_empty() {
            name.to_string()
        } else {
            format!("{}:{}", name, self.sub_contexts.join(":"))
        }
    }

    /// 按名称查找步骤
    pub fn get_step_by_name(&self, name: &str) -> Option<&dyn StepBuilder> {
        self.steps
            .iter()
            .find(|s| s.name() == name)
            .map(|s| s.as_ref())
    }

    /// 创建新的 CommandStepBuilder（同名替换旧步骤）
    pub fn new_command_step(&mut self, name: &str) -> &mut CommandStepBuilder {
        let step_name = self.get_step_name(name);

        // 移除同名步骤
        self.steps.retain(|s| s.name() != step_name);

        let builder = CommandStepBuilder::new(&step_name);
        self.steps.push(Box::new(builder));

        // 返回最后一个元素的可变引用
        let last = self.steps.last_mut().unwrap();
        last.as_any_mut()
            .downcast_mut::<CommandStepBuilder>()
            .unwrap()
    }

    /// 创建新的 ImageStepBuilder
    pub fn new_image_step(
        &mut self,
        name: &str,
        resolve_fn: Box<dyn Fn(&BuildStepOptions) -> String + Send + Sync>,
    ) -> &mut ImageStepBuilder {
        let step_name = self.get_step_name(name);
        let builder = ImageStepBuilder::new(&step_name, resolve_fn);
        self.steps.push(Box::new(builder));

        let last = self.steps.last_mut().unwrap();
        last.as_any_mut()
            .downcast_mut::<ImageStepBuilder>()
            .unwrap()
    }

    /// 创建本地层（应用 dockerignore 过滤）
    pub fn new_local_layer(&self) -> Layer {
        let mut layer = Layer::new_local_layer();

        if !self.dockerignore_ctx.includes.is_empty() {
            layer
                .filter
                .include
                .extend(self.dockerignore_ctx.includes.clone());
        }
        if !self.dockerignore_ctx.excludes.is_empty() {
            layer
                .filter
                .exclude
                .extend(self.dockerignore_ctx.excludes.clone());
        }

        layer
    }

    /// 从 Config 应用包配置到 MiseStepBuilder
    /// 注意：为避免 borrow checker 问题，先收集配置数据再操作
    fn apply_packages_from_config(&mut self) {
        if self.config.packages.is_empty() {
            return;
        }

        // 先收集包信息，避免同时借用 self.config 和 self.get_mise_step_builder()
        let mut sorted_packages: Vec<(String, String)> = self
            .config
            .packages
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        sorted_packages.sort_by(|a, b| a.0.cmp(&b.0));

        // 确保 mise_step_builder 存在
        if self.mise_step_builder.is_none() {
            self.mise_step_builder = Some(MiseStepBuilder::new(
                mise_step_builder::MISE_STEP_NAME,
                &self.config,
            ));
        }

        for (pkg, version) in sorted_packages {
            let pkg_ref = self.mise_step_builder.as_mut().unwrap().default_package(
                &mut self.resolver,
                &pkg,
                &version,
            );
            self.mise_step_builder.as_mut().unwrap().version(
                &mut self.resolver,
                &pkg_ref,
                &version,
                "custom config",
            );
        }
    }

    /// 应用用户配置到上下文
    fn apply_config(&mut self) {
        self.apply_packages_from_config();

        // 对齐 railpack：只要存在自定义步骤，也会生成 packages:mise 基础步骤。
        if !self.config.steps.is_empty() && self.mise_step_builder.is_none() {
            self.mise_step_builder = Some(MiseStepBuilder::new(
                mise_step_builder::MISE_STEP_NAME,
                &self.config,
            ));
        }

        // 合并缓存配置
        for (k, v) in &self.config.caches {
            self.caches.set_cache(k, v.clone());
        }

        // 合并 secrets
        self.secrets = spread_strings(self.config.secrets.clone(), self.secrets.clone());

        // 合并 deploy 配置
        if let Some(ref deploy_config) = self.config.deploy {
            if let Some(ref start_cmd) = deploy_config.start_cmd {
                if !start_cmd.is_empty() {
                    self.deploy.start_cmd = Some(start_cmd.clone());
                }
            }

            self.deploy.apt_packages = spread_strings(
                deploy_config.apt_packages.clone(),
                self.deploy.apt_packages.clone(),
            );
            self.deploy.deploy_inputs = spread(
                deploy_config.inputs.clone(),
                self.deploy.deploy_inputs.clone(),
            );
            self.deploy.paths =
                spread_strings(deploy_config.paths.clone(), self.deploy.paths.clone());

            for (k, v) in &deploy_config.variables {
                self.deploy.variables.insert(k.clone(), v.clone());
            }
        }

        // 应用步骤配置
        let step_names: Vec<String> = {
            let mut names: Vec<String> = self.config.steps.keys().cloned().collect();
            names.sort_by(|a, b| {
                let priority = |name: &str| match name {
                    "install" => 0_u8,
                    "build" => 1_u8,
                    _ => 2_u8,
                };

                priority(a).cmp(&priority(b)).then_with(|| a.cmp(b))
            });
            names
        };
        let has_install_config = self.config.steps.contains_key("install");

        for name in step_names {
            let config_step = self.config.steps[&name].clone();
            let needs_local_input = config_step.step.inputs.is_empty()
                && (config_step.needs_local_input
                    || (self.config.auto_local_copy_steps
                        && step_uses_local_copy(&config_step.step)));
            let local_layer = needs_local_input.then(|| self.new_local_layer());

            // 查找或创建 CommandStepBuilder
            let existing_idx = self.steps.iter().position(|s| s.name() == name);

            if let Some(idx) = existing_idx {
                if let Some(csb) = self.steps[idx]
                    .as_any_mut()
                    .downcast_mut::<CommandStepBuilder>()
                {
                    csb.merge_from_config_step(&config_step.step);

                    if let Some(ref layer) = local_layer {
                        if !command_step_has_local_input(csb) {
                            csb.add_input(layer.clone());
                        }
                    }

                    if name == "build" && has_install_config {
                        ensure_build_depends_on_install(csb);
                    }
                }
            } else {
                // 创建新的 CommandStepBuilder
                let mut csb = CommandStepBuilder::new(&name);
                // 兼容 railpack：新建配置步骤默认至少有一个基础输入
                // 优先依赖 mise 步骤；若不存在则退回 builder base image。
                if let Some(ref mise) = self.mise_step_builder {
                    csb.add_input(Layer::new_step_layer(mise.name(), None));
                } else {
                    csb.add_input(Layer::new_image_layer(&self.base_image, None));
                }

                if let Some(ref layer) = local_layer {
                    csb.add_input(layer.clone());
                }

                csb.merge_from_config_step(&config_step.step);

                if name == "build" && has_install_config {
                    ensure_build_depends_on_install(&mut csb);
                }

                // 对齐 railpack：当配置新增 install 且已有 build 时，install 应位于 build 之前。
                if name == "install" {
                    if let Some(build_idx) = self.steps.iter().position(|s| s.name() == "build") {
                        self.steps.insert(build_idx, Box::new(csb));
                    } else {
                        self.steps.push(Box::new(csb));
                    }
                } else {
                    self.steps.push(Box::new(csb));
                }
            }

            // 转换 deploy outputs 为 layers
            let output_filters = if config_step.deploy_outputs.is_empty() {
                vec![Filter::include_only(vec![".".to_string()])]
            } else {
                config_step.deploy_outputs.clone()
            };

            for filter in &output_filters {
                let mut already_covered = false;
                for inc in &filter.include {
                    if self.deploy.has_include_for_step(&name, inc) {
                        already_covered = true;
                        break;
                    }
                }
                if !already_covered {
                    self.deploy
                        .add_inputs(&[Layer::new_step_layer(&name, Some(filter.clone()))]);
                }
            }
        }
    }

    /// 核心编排：生成 BuildPlan
    pub fn generate(&mut self) -> Result<(BuildPlan, HashMap<String, ResolvedPackage>)> {
        self.apply_config();

        // 批量解析版本
        let resolved_packages = self.resolver.resolve_packages()?;

        let mut plan = BuildPlan::new();
        let mut options = BuildStepOptions {
            resolved_packages: resolved_packages.clone(),
            caches: self.caches.clone(),
        };

        // 若有 mise_step_builder，先 build 它
        if let Some(ref mise_builder) = self.mise_step_builder {
            mise_builder.build(
                &mut plan,
                &mut options,
                &self.resolver,
                &self.app,
                &self.env,
            )?;
        }

        // 遍历 steps，逐个 build
        for step in &self.steps {
            step.build(&mut plan, &mut options)?;
        }

        // 构建额外的命名 mise builders（如 Java/Gleam 运行时包）。
        // 对齐 railpack：运行时 mise 步骤应位于业务 build 之后。
        for (_, mise_builder) in &self.additional_mise_builders {
            mise_builder.build(
                &mut plan,
                &mut options,
                &self.resolver,
                &self.app,
                &self.env,
            )?;
        }

        // 写入 secrets
        let mut secrets = self.secrets.clone();
        secrets.sort();
        secrets.dedup();
        plan.secrets = secrets;

        // 构建 deploy
        self.deploy.build(&mut plan, &mut options);

        // deploy 构建阶段可能新增缓存（如 apt），需在 deploy 后回写到 plan。
        plan.caches = options.caches.caches.clone();

        // 规范化
        plan.normalize();

        Ok((plan, resolved_packages))
    }
}

fn step_uses_local_copy(step: &crate::plan::Step) -> bool {
    step.commands
        .iter()
        .any(|cmd| matches!(cmd, Command::Copy(copy) if copy.image.is_none()))
}

fn command_step_has_local_input(csb: &CommandStepBuilder) -> bool {
    csb.inputs.iter().any(|input| input.local == Some(true))
}

fn ensure_build_depends_on_install(csb: &mut CommandStepBuilder) {
    let mut next_inputs = vec![Layer::new_step_layer("install", None)];

    for input in &csb.inputs {
        if input.step.as_deref() == Some("install") {
            continue;
        }

        let keep = if input.local == Some(true) || input.image.is_some() {
            true
        } else if let Some(step_name) = input.step.as_deref() {
            !step_name.starts_with("packages:")
        } else {
            true
        };

        if keep && !next_inputs.contains(input) {
            next_inputs.push(input.clone());
        }
    }

    csb.inputs = next_inputs;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StepConfig;
    use crate::plan::Step;
    use crate::resolver::VersionResolver;
    use tempfile::TempDir;

    struct MockVersionResolver;
    impl VersionResolver for MockVersionResolver {
        fn get_latest_version(&self, _pkg: &str, version: &str) -> Result<String> {
            Ok(format!("{}.0.0", version))
        }
        fn get_all_versions(&self, _pkg: &str, _version: &str) -> Result<Vec<String>> {
            Ok(vec!["1.0.0".to_string()])
        }
    }

    fn make_test_ctx() -> (TempDir, GenerateContext) {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let config = Config::empty();
        let ctx = GenerateContext::new(app, env, config, Box::new(MockVersionResolver)).unwrap();
        (dir, ctx)
    }

    #[test]
    fn test_get_step_name_no_sub_context() {
        let (_dir, ctx) = make_test_ctx();
        assert_eq!(ctx.get_step_name("install"), "install");
    }

    #[test]
    fn test_get_step_name_with_sub_context() {
        let (_dir, mut ctx) = make_test_ctx();
        ctx.enter_sub_context("node");
        assert_eq!(ctx.get_step_name("install"), "install:node");
        ctx.exit_sub_context();
        assert_eq!(ctx.get_step_name("install"), "install");
    }

    #[test]
    fn test_new_command_step_replaces_same_name() {
        let (_dir, mut ctx) = make_test_ctx();
        ctx.new_command_step("install");
        ctx.new_command_step("install");
        assert_eq!(ctx.steps.len(), 1);
    }

    #[test]
    fn test_new_apt_install_command_sorts_and_dedupes() {
        let cmd = BuildStepOptions::new_apt_install_command(&[
            "git".to_string(),
            "curl".to_string(),
            "git".to_string(),
        ]);
        if let Command::Exec(exec) = &cmd {
            assert!(exec.cmd.contains("curl git"));
            assert!(exec.cmd.contains("apt-get update && apt-get install -y"));
        } else {
            panic!("expected Exec command");
        }
    }

    #[test]
    fn test_generate_empty_produces_empty_plan() {
        let (_dir, mut ctx) = make_test_ctx();
        let (plan, resolved) = ctx.generate().unwrap();
        assert!(plan.steps.is_empty());
        assert!(resolved.is_empty());
    }

    #[test]
    fn test_new_local_layer_basic() {
        let (_dir, ctx) = make_test_ctx();
        let layer = ctx.new_local_layer();
        assert_eq!(layer.local, Some(true));
    }

    #[test]
    fn test_generate_with_command_step() {
        let (_dir, mut ctx) = make_test_ctx();
        let step = ctx.new_command_step("install");
        step.add_command(Command::new_exec("npm ci"));
        step.add_input(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));

        // 配置 deploy 引用该步骤
        ctx.deploy.add_inputs(&[Layer::new_step_layer(
            "install",
            Some(Filter::include_only(vec![".".to_string()])),
        )]);

        let (plan, _) = ctx.generate().unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].name, Some("install".to_string()));
    }

    #[test]
    fn test_generate_deploy_apt_registers_top_level_caches() {
        let (_dir, mut ctx) = make_test_ctx();
        ctx.deploy.add_apt_packages(&["curl".to_string()]);

        let (plan, _) = ctx.generate().unwrap();
        assert!(plan.caches.contains_key("apt"));
        assert!(plan.caches.contains_key("apt-lists"));
    }

    #[test]
    fn test_generate_config_only_step_gets_mise_input() {
        let (_dir, mut ctx) = make_test_ctx();

        ctx.config.steps.insert(
            "custom".to_string(),
            StepConfig {
                step: Step {
                    commands: vec![Command::new_exec("echo hi")],
                    ..Step::new("custom")
                },
                ..Default::default()
            },
        );

        let (plan, _) = ctx.generate().unwrap();
        let custom = plan
            .steps
            .iter()
            .find(|s| s.name.as_deref() == Some("custom"))
            .expect("custom step should exist");

        assert!(!custom.inputs.is_empty());
        assert_eq!(custom.inputs[0].step.as_deref(), Some("packages:mise"));
    }
}
