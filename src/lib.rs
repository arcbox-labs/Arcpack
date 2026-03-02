pub mod app;
pub mod buildkit;
pub mod cli;
pub mod config;
pub mod error;
pub mod generate;
pub mod graph;
pub mod mise;
pub mod plan;
pub mod provider;
pub mod resolver;

pub use error::ArcpackError;

/// arcpack 统一 Result 类型
pub type Result<T> = std::result::Result<T, ArcpackError>;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use app::environment::Environment;
use app::App;
use config::Config;
use generate::GenerateContext;
use plan::BuildPlan;
use resolver::ResolvedPackage;

/// 日志级别
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// 构建日志消息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogMsg {
    pub level: LogLevel,
    pub message: String,
}

/// 构建计划生成选项
///
/// 对齐 railpack `core/core.go GenerateBuildPlanOptions`
pub struct GenerateBuildPlanOptions {
    /// 默认构建命令（优先级低于环境变量和配置文件）
    pub build_command: Option<String>,
    /// 默认启动命令（优先级低于环境变量和配置文件）
    pub start_command: Option<String>,
    /// 上次构建的包版本（用于版本固定）
    pub previous_versions: HashMap<String, String>,
    /// 配置文件路径（默认 arcpack.json）
    pub config_file_path: Option<String>,
    /// 启动命令缺失时是否报错
    pub error_missing_start_command: bool,
}

impl Default for GenerateBuildPlanOptions {
    fn default() -> Self {
        Self {
            build_command: None,
            start_command: None,
            previous_versions: HashMap::new(),
            config_file_path: None,
            error_missing_start_command: false,
        }
    }
}

/// 构建结果
///
/// 对齐 railpack `core/core.go BuildResult`
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildResult {
    /// arcpack 版本
    pub arcpack_version: String,
    /// 构建计划
    pub plan: Option<BuildPlan>,
    /// 解析后的包版本
    pub resolved_packages: HashMap<String, ResolvedPackage>,
    /// 元数据
    pub metadata: HashMap<String, String>,
    /// 检测到的 Provider 列表
    pub detected_providers: Vec<String>,
    /// 构建日志
    pub logs: Vec<LogMsg>,
    /// 是否成功
    pub success: bool,
}

/// 核心入口：从源码生成构建计划
///
/// 对齐 railpack `core/core.go GenerateBuildPlan`
/// 编排流程：
/// 1. App::new(source)
/// 2. Environment::new(env_vars)
/// 3. Config::load(&app, &env, options_config, config_file_path)
/// 4. 检测 Provider（含 Config 强制指定）
/// 5. GenerateContext::new(app, env, config, version_resolver)
/// 6. provider.initialize(&mut ctx)
/// 7. provider.plan(&mut ctx)
/// 8. ctx.generate() → (plan, resolved_packages)
/// 9. provider.cleanse_plan(&mut plan)
/// 10. 验证 plan（检查 start_cmd）
/// 11. 返回 BuildResult
pub fn generate_build_plan(
    source: &str,
    env_vars: HashMap<String, String>,
    options: &GenerateBuildPlanOptions,
) -> Result<BuildResult> {
    let app = App::new(source)?;
    let env = Environment::new(env_vars);
    let options_config = Config::from_options(&options.build_command, &options.start_command);

    // 配置文件路径：CLI --config-file > ARCPACK_CONFIG_FILE 环境变量 > 默认 arcpack.json
    let config_file_path = options
        .config_file_path
        .clone()
        .or_else(|| env.get_config_variable("CONFIG_FILE").0);
    let config = Config::load(&app, &env, options_config, &config_file_path)?;

    // 检测 Provider（允许无匹配：后续由 plan 校验统一报错）
    let (mut provider_to_use, detected_provider_name) = resolve_provider(&app, &env, &config)?;

    // 创建版本解析器
    let cache_dir = std::env::temp_dir().join(format!("arcpack/mise-{}", std::process::id()));
    let cache_dir_str = cache_dir
        .to_str()
        .ok_or_else(|| ArcpackError::ConfigError {
            message: format!("缓存路径包含非 UTF-8 字符: {:?}", cache_dir),
        })?;
    let version_resolver = Box::new(mise::Mise::new(cache_dir_str)?);

    // 创建 GenerateContext
    let mut ctx = GenerateContext::new(app, env, config, version_resolver)?;

    // 注入上次构建版本
    for (pkg, version) in &options.previous_versions {
        ctx.resolver.set_previous_version(pkg, version);
    }

    // Provider 生命周期
    if let Some(ref mut provider) = provider_to_use {
        provider.initialize(&mut ctx)?;
        provider.plan(&mut ctx)?;
    }

    // Procfile 后处理：覆盖 start_cmd（如果 Procfile 存在）
    let procfile_provider = provider::procfile::ProcfileProvider::new();
    procfile_provider.plan(&mut ctx)?;

    // 生成构建计划
    let (mut plan, resolved_packages) = ctx.generate()?;

    // 后处理
    if let Some(ref provider) = provider_to_use {
        provider.cleanse_plan(&mut plan);
    }

    // 验证
    validate_plan(&plan, provider_to_use.as_deref(), options)?;

    // 收集元数据和日志
    let metadata = ctx.metadata.to_map();
    let logs = ctx.logs.into_logs();

    let detected_providers = detected_provider_name
        .or_else(|| provider_to_use.as_ref().map(|p| p.name().to_string()))
        .into_iter()
        .collect();

    Ok(BuildResult {
        arcpack_version: env!("CARGO_PKG_VERSION").to_string(),
        plan: Some(plan),
        resolved_packages,
        metadata,
        detected_providers,
        logs,
        success: true,
    })
}

/// 检测匹配的 Provider
///
/// 对齐 railpack：即使 Config 强制指定 Provider，也先做自动检测用于记录 detected provider。
///
/// 返回值：
/// - provider_to_use: 最终实际执行 plan 的 Provider（允许 None）
/// - detected_provider_name: 自动检测命中的 Provider 名称（仅元数据）
fn resolve_provider(
    app: &App,
    env: &Environment,
    config: &Config,
) -> Result<(Option<Box<dyn provider::Provider>>, Option<String>)> {
    let mut detected_provider_name: Option<String> = None;
    let mut detected_provider: Option<Box<dyn provider::Provider>> = None;

    // 自动检测（第一个匹配）
    for p in provider::get_all_providers() {
        if p.detect(app, env)? {
            detected_provider_name = Some(p.name().to_string());
            detected_provider = Some(p);
            break;
        }
    }

    // Config 强制指定
    if let Some(ref provider_name) = config.provider {
        let configured_provider =
            provider::get_provider(provider_name).ok_or_else(|| ArcpackError::UnknownProvider {
                name: provider_name.clone(),
            })?;
        return Ok((Some(configured_provider), detected_provider_name));
    }

    Ok((detected_provider, detected_provider_name))
}

/// 验证构建计划
fn validate_plan(
    plan: &BuildPlan,
    provider: Option<&dyn provider::Provider>,
    options: &GenerateBuildPlanOptions,
) -> Result<()> {
    validate_commands(plan)?;

    for step in &plan.steps {
        validate_step_inputs(step)?;
    }

    validate_deploy_base(plan)?;

    if options.error_missing_start_command {
        validate_start_command(plan, provider)?;
    }

    Ok(())
}

/// 校验：构建计划至少包含一条命令
fn validate_commands(plan: &BuildPlan) -> Result<()> {
    let has_any_command = plan.steps.iter().any(|step| !step.commands.is_empty());
    if !has_any_command {
        return Err(ArcpackError::InvalidPlan {
            message: "build plan has no commands".to_string(),
        });
    }
    Ok(())
}

/// 校验：每个步骤的输入结构合法
fn validate_step_inputs(step: &plan::Step) -> Result<()> {
    let step_name = step.name.as_deref().unwrap_or("<unnamed>");

    if step.inputs.is_empty() {
        return Err(ArcpackError::InvalidPlan {
            message: format!("step '{}' has no inputs", step_name),
        });
    }

    let first_input = &step.inputs[0];
    if first_input.image.is_none() && first_input.step.is_none() {
        return Err(ArcpackError::InvalidPlan {
            message: format!(
                "step '{}' first input must reference an image or step",
                step_name
            ),
        });
    }

    Ok(())
}

/// 校验：deploy.base 必须存在且引用 image 或 step
fn validate_deploy_base(plan: &BuildPlan) -> Result<()> {
    let has_valid_base = plan
        .deploy
        .base
        .as_ref()
        .map(|base| base.image.is_some() || base.step.is_some())
        .unwrap_or(false);

    if !has_valid_base {
        return Err(ArcpackError::InvalidPlan {
            message: "deploy.base is required".to_string(),
        });
    }

    Ok(())
}

/// 校验：开启 strict start 校验时，必须存在 start command
fn validate_start_command(
    plan: &BuildPlan,
    provider: Option<&dyn provider::Provider>,
) -> Result<()> {
    if plan.deploy.start_cmd.is_none() {
        return Err(ArcpackError::NoStartCommand {
            help: provider.and_then(|p| p.start_command_help()),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Command, Filter, Layer, Step};
    use tempfile::TempDir;

    fn make_valid_plan() -> BuildPlan {
        let mut plan = BuildPlan::new();

        let mut step = Step::new("build");
        step.inputs = vec![Layer::new_image_layer("ubuntu:22.04", None)];
        step.commands = vec![Command::new_exec("echo hello")];
        plan.steps.push(step);

        plan.deploy.base = Some(Layer::new_image_layer("ubuntu:22.04", None));
        plan.deploy.inputs.push(Layer::new_step_layer(
            "build",
            Some(Filter::include_only(vec![".".to_string()])),
        ));
        plan.deploy.start_cmd = Some("echo start".to_string());

        plan
    }

    #[test]
    fn test_generate_build_plan_options_default_no_error_missing_start() {
        let options = GenerateBuildPlanOptions::default();
        assert!(!options.error_missing_start_command);
    }

    #[test]
    fn test_resolve_provider_no_match_returns_none() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path()).unwrap();
        let env = Environment::new(HashMap::new());
        let config = Config::empty();

        let (provider, detected_name) = resolve_provider(&app, &env, &config).unwrap();
        assert!(provider.is_none());
        assert!(detected_name.is_none());
    }

    #[test]
    fn test_validate_plan_rejects_no_commands() {
        let mut plan = BuildPlan::new();
        plan.deploy.base = Some(Layer::new_image_layer("ubuntu:22.04", None));

        let err = validate_plan(&plan, None, &GenerateBuildPlanOptions::default()).unwrap_err();
        assert!(matches!(err, ArcpackError::InvalidPlan { .. }));
        assert!(err.to_string().contains("no commands"));
    }

    #[test]
    fn test_validate_plan_rejects_step_without_inputs() {
        let mut plan = make_valid_plan();
        plan.steps[0].inputs.clear();

        let err = validate_plan(&plan, None, &GenerateBuildPlanOptions::default()).unwrap_err();
        assert!(matches!(err, ArcpackError::InvalidPlan { .. }));
        assert!(err.to_string().contains("has no inputs"));
    }

    #[test]
    fn test_validate_plan_rejects_missing_deploy_base() {
        let mut plan = make_valid_plan();
        plan.deploy.base = None;

        let err = validate_plan(&plan, None, &GenerateBuildPlanOptions::default()).unwrap_err();
        assert!(matches!(err, ArcpackError::InvalidPlan { .. }));
        assert!(err.to_string().contains("deploy.base"));
    }

    #[test]
    fn test_validate_plan_start_command_respects_option() {
        let mut plan = make_valid_plan();
        plan.deploy.start_cmd = None;

        // 默认关闭：不报错
        assert!(validate_plan(&plan, None, &GenerateBuildPlanOptions::default()).is_ok());

        // 显式开启：报错
        let strict_options = GenerateBuildPlanOptions {
            error_missing_start_command: true,
            ..Default::default()
        };
        let err = validate_plan(&plan, None, &strict_options).unwrap_err();
        assert!(matches!(err, ArcpackError::NoStartCommand { .. }));
    }
}
