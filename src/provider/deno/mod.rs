/// Deno Provider：deno.json/deno.jsonc 检测 + deno cache/run
///
/// 对齐 railpack `core/providers/deno/deno.go`
/// 自动检测主文件，支持 ARCPACK_DENO_VERSION 版本覆盖。

use crate::app::App;
use crate::app::environment::Environment;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::{self, MiseStepBuilder};
use crate::generate::GenerateContext;
use crate::plan::{Command, Filter, Layer};
use crate::provider::Provider;
use crate::Result;

/// 默认 Deno 版本
const DEFAULT_DENO_VERSION: &str = "2";

/// 主文件搜索优先级
const MAIN_FILE_CANDIDATES: &[&str] = &[
    "main.ts", "main.js", "main.mjs", "main.mts",
];

/// Deno Provider
pub struct DenoProvider {
    /// 检测到的主文件路径
    main_file: Option<String>,
}

impl DenoProvider {
    pub fn new() -> Self {
        Self { main_file: None }
    }

    /// 搜索主文件：按优先级检查候选文件，回退到首个 ts/js 匹配
    fn find_main_file(app: &App) -> Option<String> {
        // 固定候选文件
        for candidate in MAIN_FILE_CANDIDATES {
            if app.has_file(candidate) {
                return Some(candidate.to_string());
            }
        }
        // 回退：搜索首个 ts/js 文件
        for pattern in &["**/*.ts", "**/*.js"] {
            if let Ok(files) = app.find_files(pattern) {
                if let Some(first) = files.first() {
                    return Some(first.clone());
                }
            }
        }
        None
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

impl Provider for DenoProvider {
    fn name(&self) -> &str {
        "deno"
    }

    fn detect(&self, app: &App, _env: &Environment) -> Result<bool> {
        Ok(app.has_file("deno.json") || app.has_file("deno.jsonc"))
    }

    fn initialize(&mut self, ctx: &mut GenerateContext) -> Result<()> {
        self.main_file = Self::find_main_file(&ctx.app);
        Ok(())
    }

    fn plan(&self, ctx: &mut GenerateContext) -> Result<()> {
        // 元数据
        if let Some(ref main_file) = self.main_file {
            ctx.metadata.set("denoMainFile", main_file);
        }

        // === mise 步骤：安装 Deno ===
        Self::ensure_mise_step_builder(ctx);

        let deno_ref = {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            mise.default_package(&mut ctx.resolver, "deno", DEFAULT_DENO_VERSION)
        };

        // 环境变量版本覆盖
        if let (Some(env_version), var_name) = ctx.env.get_config_variable("DENO_VERSION") {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            mise.version(&mut ctx.resolver, &deno_ref, &env_version, &var_name);
        }

        let mise_step_name = ctx
            .mise_step_builder
            .as_ref()
            .map(|m| m.name().to_string())
            .unwrap_or_else(|| mise_step_builder::MISE_STEP_NAME.to_string());

        // === build 步骤：deno cache ===
        let build = ctx.new_command_step("build");
        build.add_input(Layer::new_step_layer(&mise_step_name, None));
        {
            let local_layer = ctx.new_local_layer();
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_input(local_layer);

            // 仅主文件存在时执行 deno cache
            if let Some(ref main_file) = self.main_file {
                build.add_command(Command::new_exec(format!("deno cache {}", main_file)));
            }
        }

        // 缓存：Deno 全局缓存目录
        let cache_name = ctx.caches.add_cache("deno-cache", "/root/.cache");
        {
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_cache(&cache_name);
        }

        // === Deploy 配置 ===
        if let Some(ref main_file) = self.main_file {
            ctx.deploy.start_cmd =
                Some(format!("deno run --allow-all {}", main_file));
        }

        // deploy inputs: mise 层 + build 步骤输出
        let mise_layer = ctx
            .mise_step_builder
            .as_ref()
            .map(|m| m.get_layer())
            .unwrap_or_default();

        let build_layer = Layer::new_step_layer(
            "build",
            Some(Filter::include_only(vec![".".to_string()])),
        );

        ctx.deploy.add_inputs(&[mise_layer, build_layer]);

        Ok(())
    }

    fn start_command_help(&self) -> Option<String> {
        Some(
            "To configure your start command, arcpack will:\n\n\
             1. Detect your main file (main.ts, main.js, etc.)\n\
             2. Cache dependencies with `deno cache`\n\
             3. Use `deno run --allow-all <main_file>` as the start command"
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
    fn test_detect_with_deno_json() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("deno.json"), "{}").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = DenoProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_with_deno_jsonc() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("deno.jsonc"), "{}").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = DenoProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_without_deno_config() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = DenoProvider::new();
        assert!(!provider.detect(&app, &env).unwrap());
    }

    // === 主文件搜索测试 ===

    #[test]
    fn test_find_main_file_main_ts() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.ts"), "console.log('hi')").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(DenoProvider::find_main_file(&app), Some("main.ts".to_string()));
    }

    #[test]
    fn test_find_main_file_main_js() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.js"), "console.log('hi')").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(DenoProvider::find_main_file(&app), Some("main.js".to_string()));
    }

    #[test]
    fn test_find_main_file_fallback_glob() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("app.ts"), "console.log('hi')").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(DenoProvider::find_main_file(&app), Some("app.ts".to_string()));
    }

    #[test]
    fn test_find_main_file_none() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(DenoProvider::find_main_file(&app), None);
    }

    // === plan 测试 ===

    #[test]
    fn test_plan_basic() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("deno.json"), "{}").unwrap();
        fs::write(dir.path().join("main.ts"), "console.log('hello')").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = DenoProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        let step_names: Vec<&str> = ctx.steps.iter().map(|s| s.name()).collect();
        assert!(step_names.contains(&"build"));

        assert_eq!(
            ctx.deploy.start_cmd.as_deref(),
            Some("deno run --allow-all main.ts")
        );

        assert_eq!(ctx.metadata.get("denoMainFile"), Some("main.ts"));
        assert!(ctx.caches.get_cache("deno-cache").is_some());
    }

    #[test]
    fn test_plan_no_main_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("deno.json"), "{}").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = DenoProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        // 无主文件时不设置 start_cmd
        assert!(ctx.deploy.start_cmd.is_none());
    }

    #[test]
    fn test_plan_env_version_override() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("deno.json"), "{}").unwrap();
        fs::write(dir.path().join("main.ts"), "").unwrap();
        let mut ctx = make_ctx_with_env(
            &dir,
            HashMap::from([("ARCPACK_DENO_VERSION".to_string(), "1.40".to_string())]),
        );
        let mut provider = DenoProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        let requested = ctx.resolver.get("deno").unwrap();
        assert_eq!(requested.version, "1.40");
    }
}
