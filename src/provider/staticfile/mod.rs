/// StaticFile Provider：纯静态网站，使用 Caddy 作为文件服务器
///
/// 对齐 railpack `core/providers/staticfile/staticfile.go`
/// 支持多种根目录检测方式和 Caddyfile 模板。
use serde::Deserialize;

use crate::app::environment::Environment;
use crate::app::App;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::{self, MiseStepBuilder};
use crate::generate::GenerateContext;
use crate::plan::{Command, Filter, Layer};
use crate::provider::Provider;
use crate::Result;

/// Caddy 默认版本
const DEFAULT_CADDY_VERSION: &str = "latest";

/// 默认 Caddyfile 模板
const CADDYFILE_TEMPLATE: &str = include_str!("templates/Caddyfile.template");

/// Staticfile YAML 配置
#[derive(Debug, Deserialize, Default)]
struct StaticfileConfig {
    root: Option<String>,
}

/// StaticFile Provider
pub struct StaticFileProvider {
    /// 静态文件根目录
    root_dir: String,
}

impl StaticFileProvider {
    pub fn new() -> Self {
        Self {
            root_dir: ".".to_string(),
        }
    }

    /// 检测根目录（四步级联）
    /// 返回 Some(root_dir) 或 None（不匹配）
    fn detect_root(app: &App, env: &Environment) -> Option<String> {
        // 1. ARCPACK_STATIC_FILE_ROOT 环境变量
        if let (Some(root), _) = env.get_config_variable("STATIC_FILE_ROOT") {
            return Some(root);
        }

        // 2. Staticfile 文件（YAML 格式）
        if app.has_file("Staticfile") {
            if let Ok(content) = app.read_file("Staticfile") {
                let content = content.trim();
                if content.is_empty() {
                    return Some(".".to_string());
                }
                if let Ok(config) = serde_yaml::from_str::<StaticfileConfig>(content) {
                    return Some(config.root.unwrap_or_else(|| ".".to_string()));
                }
            }
            return Some(".".to_string());
        }

        // 3. public/ 目录
        if app.has_match("public") {
            return Some("public".to_string());
        }

        // 4. index.html 在根目录
        if app.has_file("index.html") {
            return Some(".".to_string());
        }

        None
    }

    /// 确保 mise_step_builder 已初始化
    fn ensure_mise_step_builder(ctx: &mut GenerateContext) {
        if ctx.mise_step_builder.is_none() {
            ctx.mise_step_builder = Some(MiseStepBuilder::new(
                mise_step_builder::MISE_STEP_NAME,
                &ctx.config,
            ));
        }
    }

    /// 获取指定名称的 CommandStepBuilder 可变引用
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

impl Provider for StaticFileProvider {
    fn name(&self) -> &str {
        "staticfile"
    }

    fn detect(&self, app: &App, env: &Environment) -> Result<bool> {
        Ok(Self::detect_root(app, env).is_some())
    }

    fn initialize(&mut self, ctx: &mut GenerateContext) -> Result<()> {
        if let Some(root) = Self::detect_root(&ctx.app, &ctx.env) {
            self.root_dir = root;
        }
        Ok(())
    }

    fn plan(&self, ctx: &mut GenerateContext) -> Result<()> {
        // mise 步骤：安装 caddy
        Self::ensure_mise_step_builder(ctx);
        {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            let _ = mise.default_package(&mut ctx.resolver, "caddy", DEFAULT_CADDY_VERSION);
        }

        // 检查用户自定义 Caddyfile
        let has_custom_caddyfile =
            ctx.app.has_file("Caddyfile") || ctx.app.has_file("Caddyfile.template");
        let local_layer = ctx.new_local_layer();

        // build 步骤：写入 Caddyfile + caddy fmt
        let build = ctx.new_command_step("build");
        build.add_input(Layer::new_step_layer(
            mise_step_builder::MISE_STEP_NAME,
            None,
        ));
        {
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_input(local_layer);

            if !has_custom_caddyfile {
                let caddyfile_content =
                    CADDYFILE_TEMPLATE.replace("{{.STATIC_FILE_ROOT}}", &self.root_dir);
                build
                    .assets
                    .insert("Caddyfile".to_string(), caddyfile_content);
                build.add_command(Command::new_file("Caddyfile", "Caddyfile"));
            }

            build.add_command(Command::new_exec("caddy fmt --overwrite Caddyfile"));
        }

        // Deploy 配置
        ctx.deploy.start_cmd =
            Some("caddy run --config Caddyfile --adapter caddyfile 2>&1".to_string());

        // deploy inputs: mise 层 + build 步骤输出
        let mise_layer = ctx
            .mise_step_builder
            .as_ref()
            .map(|m| m.get_layer())
            .unwrap_or_default();
        let build_layer =
            Layer::new_step_layer("build", Some(Filter::include_only(vec![".".to_string()])));

        ctx.deploy.add_inputs(&[mise_layer, build_layer]);

        Ok(())
    }

    fn start_command_help(&self) -> Option<String> {
        Some(
            "To deploy a static site, arcpack will look for:\n\n\
             1. The ARCPACK_STATIC_FILE_ROOT environment variable\n\
             2. A \"Staticfile\" with a root: directive\n\
             3. A \"public/\" directory\n\
             4. An \"index.html\" in the project root"
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
    fn test_detect_with_index_html() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("index.html"), "<h1>Hello</h1>").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = StaticFileProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_with_public_dir() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("public")).unwrap();
        fs::write(dir.path().join("public/index.html"), "<h1>Hello</h1>").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = StaticFileProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_with_staticfile() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Staticfile"), "root: dist").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = StaticFileProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_with_env_var() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::from([(
            "ARCPACK_STATIC_FILE_ROOT".to_string(),
            "build".to_string(),
        )]));
        let provider = StaticFileProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_none_match() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = StaticFileProvider::new();
        assert!(!provider.detect(&app, &env).unwrap());
    }

    // === 根目录解析测试 ===

    #[test]
    fn test_root_from_staticfile_yaml() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Staticfile"), "root: dist").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        assert_eq!(
            StaticFileProvider::detect_root(&app, &env),
            Some("dist".to_string())
        );
    }

    #[test]
    fn test_root_from_public_dir() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("public")).unwrap();
        fs::write(dir.path().join("public/index.html"), "").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        assert_eq!(
            StaticFileProvider::detect_root(&app, &env),
            Some("public".to_string())
        );
    }

    #[test]
    fn test_root_from_index_html() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("index.html"), "").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        assert_eq!(
            StaticFileProvider::detect_root(&app, &env),
            Some(".".to_string())
        );
    }

    // === Caddyfile 模板测试 ===

    #[test]
    fn test_caddyfile_template_substitution() {
        let content = CADDYFILE_TEMPLATE.replace("{{.STATIC_FILE_ROOT}}", "dist");
        assert!(content.contains("root * dist"));
    }

    #[test]
    fn test_caddyfile_template_has_health() {
        assert!(CADDYFILE_TEMPLATE.contains("/health"));
    }

    // === plan 测试 ===

    #[test]
    fn test_plan_basic() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("index.html"), "<h1>Hello</h1>").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = StaticFileProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        assert_eq!(
            ctx.deploy.start_cmd.as_deref(),
            Some("caddy run --config Caddyfile --adapter caddyfile 2>&1")
        );
    }

    #[test]
    fn test_plan_with_custom_root() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Staticfile"), "root: dist").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = StaticFileProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.root_dir, "dist");
    }

    #[test]
    fn test_plan_env_root_override() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("index.html"), "").unwrap();
        let mut ctx = make_ctx_with_env(
            &dir,
            HashMap::from([("ARCPACK_STATIC_FILE_ROOT".to_string(), "build".to_string())]),
        );
        let mut provider = StaticFileProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.root_dir, "build");
    }
}
