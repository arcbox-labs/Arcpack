pub mod error;
pub mod plan;
pub mod app;
pub mod config;
pub mod resolver;
pub mod mise;
pub mod generate;
pub mod provider;
pub mod cli;
pub mod graph;
pub mod buildkit;

pub use error::ArcpackError;

/// arcpack 统一 Result 类型
pub type Result<T> = std::result::Result<T, ArcpackError>;

use std::collections::HashMap;

use serde::{Serialize, Deserialize};

use app::App;
use app::environment::Environment;
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
            error_missing_start_command: true,
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
    let config = Config::load(&app, &env, options_config, &options.config_file_path)?;

    // 检测 Provider
    let mut provider_to_use = detect_provider(&app, &env, &config)?;

    // 创建版本解析器
    let cache_dir = std::env::temp_dir().join(format!("arcpack/mise-{}", std::process::id()));
    let cache_dir_str = cache_dir.to_str().ok_or_else(|| {
        ArcpackError::ConfigError {
            message: format!("缓存路径包含非 UTF-8 字符: {:?}", cache_dir),
        }
    })?;
    let version_resolver = Box::new(mise::Mise::new(cache_dir_str)?);

    // 创建 GenerateContext
    let mut ctx = GenerateContext::new(app, env, config, version_resolver)?;

    // 注入上次构建版本
    for (pkg, version) in &options.previous_versions {
        ctx.resolver.set_previous_version(pkg, version);
    }

    // Provider 生命周期
    provider_to_use.initialize(&mut ctx)?;
    provider_to_use.plan(&mut ctx)?;

    // 生成构建计划
    let (mut plan, resolved_packages) = ctx.generate()?;

    // 后处理
    provider_to_use.cleanse_plan(&mut plan);

    // 验证
    validate_plan(&plan, &*provider_to_use, options)?;

    // 收集元数据
    let metadata = ctx.metadata.to_map();

    Ok(BuildResult {
        arcpack_version: env!("CARGO_PKG_VERSION").to_string(),
        plan: Some(plan),
        resolved_packages,
        metadata,
        detected_providers: vec![provider_to_use.name().to_string()],
        logs: Vec::new(),
        success: true,
    })
}

/// 检测匹配的 Provider
///
/// 优先级：Config 强制指定 > 自动检测（第一个匹配）
fn detect_provider(
    app: &App,
    env: &Environment,
    config: &Config,
) -> Result<Box<dyn provider::Provider>> {
    // Config 强制指定
    if let Some(ref provider_name) = config.provider {
        return provider::get_provider(provider_name).ok_or_else(|| {
            ArcpackError::UnknownProvider { name: provider_name.clone() }
        });
    }

    // 自动检测
    for p in provider::get_all_providers() {
        if p.detect(app, env)? {
            return Ok(p);
        }
    }

    Err(ArcpackError::NoProviderMatched)
}

/// 验证构建计划
fn validate_plan(
    plan: &BuildPlan,
    provider: &dyn provider::Provider,
    options: &GenerateBuildPlanOptions,
) -> Result<()> {
    if options.error_missing_start_command {
        if plan.deploy.start_cmd.is_none() {
            return Err(ArcpackError::NoStartCommand {
                help: provider.start_command_help(),
            });
        }
    }
    Ok(())
}
