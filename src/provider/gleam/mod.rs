/// Gleam Provider：gleam.toml 检测 + gleam export erlang-shipment
///
/// 对齐 railpack `core/providers/gleam/gleam.go`
/// 构建期用 gleam + erlang，运行时仅需 erlang。
use serde::Deserialize;

use crate::app::environment::Environment;
use crate::app::App;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::{self, MiseStepBuilder};
use crate::generate::GenerateContext;
use crate::plan::{Command, Filter, Layer};
use crate::provider::Provider;
use crate::Result;

/// 默认版本
const DEFAULT_GLEAM_VERSION: &str = "latest";
const DEFAULT_ERLANG_VERSION: &str = "latest";

/// gleam.toml 结构（轻量提取）
#[derive(Debug, Deserialize, Default)]
struct GleamToml {
    name: Option<String>,
}

/// Gleam Provider
pub struct GleamProvider {
    /// 项目名（从 gleam.toml 解析）
    app_name: Option<String>,
    /// 是否包含源码到部署镜像
    include_source: bool,
}

impl GleamProvider {
    pub fn new() -> Self {
        Self {
            app_name: None,
            include_source: false,
        }
    }

    /// 从 gleam.toml 解析项目名
    fn parse_app_name(app: &App) -> Option<String> {
        let content = app.read_file("gleam.toml").ok()?;
        let toml: GleamToml = toml::from_str(&content).ok()?;
        toml.name
    }

    fn ensure_mise_step_builder(ctx: &mut GenerateContext) {
        if ctx.mise_step_builder.is_none() {
            ctx.mise_step_builder = Some(MiseStepBuilder::new(
                mise_step_builder::MISE_STEP_NAME,
                &ctx.config,
            ));
        }
    }

    fn get_command_step<'a>(
        steps: &'a mut [Box<dyn crate::generate::StepBuilder>],
        name: &str,
    ) -> &'a mut CommandStepBuilder {
        let idx = steps.iter().position(|s| s.name() == name).unwrap();
        steps[idx]
            .as_any_mut()
            .downcast_mut::<CommandStepBuilder>()
            .unwrap()
    }
}

impl Provider for GleamProvider {
    fn name(&self) -> &str {
        "gleam"
    }

    fn detect(&self, app: &App, _env: &Environment) -> Result<bool> {
        Ok(app.has_file("gleam.toml"))
    }

    fn initialize(&mut self, ctx: &mut GenerateContext) -> Result<()> {
        self.app_name = Self::parse_app_name(&ctx.app);

        // ARCPACK_GLEAM_INCLUDE_SOURCE 控制是否包含源码
        if let (Some(val), _) = ctx.env.get_config_variable("GLEAM_INCLUDE_SOURCE") {
            self.include_source = val == "true" || val == "1";
        }

        Ok(())
    }

    fn plan(&self, ctx: &mut GenerateContext) -> Result<()> {
        // 元数据
        if let Some(ref name) = self.app_name {
            ctx.metadata.set("gleamAppName", name);
        }

        // === mise 步骤（构建期）：安装 gleam + erlang ===
        Self::ensure_mise_step_builder(ctx);

        {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            mise.default_package(&mut ctx.resolver, "gleam", DEFAULT_GLEAM_VERSION);
            mise.default_package(&mut ctx.resolver, "erlang", DEFAULT_ERLANG_VERSION);
        }

        let mise_step_name = ctx
            .mise_step_builder
            .as_ref()
            .map(|m| m.name().to_string())
            .unwrap_or_else(|| mise_step_builder::MISE_STEP_NAME.to_string());

        // === build 步骤 ===
        let build = ctx.new_command_step("build");
        build.add_input(Layer::new_step_layer(&mise_step_name, None));
        {
            let local_layer = ctx.new_local_layer();
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_input(local_layer);
            build.add_command(Command::new_exec("gleam export erlang-shipment"));
        }

        // === 运行时 mise（仅 erlang） ===
        // 直接字段访问避免双重可变借用
        ctx.additional_mise_builders.push((
            "packages:mise:runtime".to_string(),
            MiseStepBuilder::new("packages:mise:runtime", &ctx.config),
        ));
        {
            let runtime_mise = &mut ctx.additional_mise_builders.last_mut().unwrap().1;
            runtime_mise.default_package(&mut ctx.resolver, "erlang", DEFAULT_ERLANG_VERSION);
        }

        // === Deploy 配置 ===
        let bin_name = self.app_name.as_deref().unwrap_or("app");

        ctx.deploy.start_cmd = Some(format!("./build/erlang-shipment/entrypoint.sh run"));

        ctx.metadata.set("gleamBinName", bin_name);

        // deploy inputs
        let runtime_mise_layer = Layer::new_step_layer(
            "packages:mise:runtime",
            Some(Filter::include_only(vec![
                "/mise/shims".to_string(),
                "/mise/installs".to_string(),
                "/usr/local/bin/mise".to_string(),
                "/etc/mise/config.toml".to_string(),
                "/root/.local/state/mise".to_string(),
            ])),
        );

        let mut build_filter = Filter::include_only(vec!["build/erlang-shipment/.".to_string()]);

        if self.include_source {
            // 包含完整源码
            build_filter = Filter::include_only(vec![".".to_string()]);
        }

        let build_layer = Layer::new_step_layer("build", Some(build_filter));

        ctx.deploy.add_inputs(&[runtime_mise_layer, build_layer]);

        Ok(())
    }

    fn start_command_help(&self) -> Option<String> {
        Some(
            "To configure your start command, arcpack will:\n\n\
             1. Build with `gleam export erlang-shipment`\n\
             2. Use `./build/erlang-shipment/entrypoint.sh run` as the start command"
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::resolver::VersionResolver;
    use std::collections::HashMap;
    use std::fs;
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

    fn make_ctx(dir: &TempDir) -> GenerateContext {
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let config = Config::empty();
        GenerateContext::new(app, env, config, Box::new(MockVersionResolver)).unwrap()
    }

    fn make_ctx_with_env(dir: &TempDir, env_vars: HashMap<String, String>) -> GenerateContext {
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(env_vars);
        let config = Config::empty();
        GenerateContext::new(app, env, config, Box::new(MockVersionResolver)).unwrap()
    }

    // === detect 测试 ===

    #[test]
    fn test_detect_with_gleam_toml() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("gleam.toml"), "name = \"myapp\"").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = GleamProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_without_gleam_toml() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = GleamProvider::new();
        assert!(!provider.detect(&app, &env).unwrap());
    }

    // === 名称解析测试 ===

    #[test]
    fn test_parse_app_name() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("gleam.toml"),
            "name = \"hello_gleam\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(
            GleamProvider::parse_app_name(&app),
            Some("hello_gleam".to_string())
        );
    }

    #[test]
    fn test_parse_app_name_missing() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("gleam.toml"), "version = \"1.0.0\"\n").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(GleamProvider::parse_app_name(&app), None);
    }

    // === plan 测试 ===

    #[test]
    fn test_plan_basic() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("gleam.toml"),
            "name = \"myapp\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/main.gleam"),
            "pub fn main() { io.println(\"Hello!\") }",
        )
        .unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = GleamProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        let step_names: Vec<&str> = ctx.steps.iter().map(|s| s.name()).collect();
        assert!(step_names.contains(&"build"));

        assert_eq!(
            ctx.deploy.start_cmd.as_deref(),
            Some("./build/erlang-shipment/entrypoint.sh run")
        );

        assert_eq!(ctx.metadata.get("gleamAppName"), Some("myapp"));
        assert_eq!(ctx.metadata.get("gleamBinName"), Some("myapp"));

        // 验证运行时 mise builder 存在
        assert!(ctx
            .additional_mise_builders
            .iter()
            .any(|(name, _)| name == "packages:mise:runtime"));
    }

    #[test]
    fn test_plan_include_source() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("gleam.toml"), "name = \"myapp\"\n").unwrap();

        let mut ctx = make_ctx_with_env(
            &dir,
            HashMap::from([(
                "ARCPACK_GLEAM_INCLUDE_SOURCE".to_string(),
                "true".to_string(),
            )]),
        );
        let mut provider = GleamProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert!(provider.include_source);
    }
}
