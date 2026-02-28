/// PHP Provider：FrankenPHP (Caddy) 镜像 + Composer 安装
///
/// 对齐 railpack `core/providers/php/php.go`
/// 使用 ImageStepBuilder（基于 dunglas/frankenphp 镜像），
/// 支持 Laravel、PHP 扩展自动检测、Caddyfile 模板。

pub mod extensions;
pub mod laravel;

use std::collections::HashMap;

use regex::Regex;

use crate::app::App;
use crate::app::environment::Environment;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::GenerateContext;
use crate::plan::{Command, Filter, Layer};
use crate::provider::Provider;
use crate::Result;

/// 默认 PHP 版本
const DEFAULT_PHP_VERSION: &str = "8.4";

/// 嵌入模板
const CADDYFILE_TEMPLATE: &str = include_str!("templates/Caddyfile");
const PHP_INI_TEMPLATE: &str = include_str!("templates/php.ini");
const START_CONTAINER_SCRIPT: &str = include_str!("templates/start-container.sh");

/// PHP Provider
pub struct PhpProvider {
    /// PHP 版本
    php_version: String,
    /// 是否为 Laravel 应用
    is_laravel: bool,
    /// 根目录（Web root）
    root_dir: String,
    /// 检测到的 PHP 扩展
    extensions: Vec<String>,
}

impl PhpProvider {
    pub fn new() -> Self {
        Self {
            php_version: DEFAULT_PHP_VERSION.to_string(),
            is_laravel: false,
            root_dir: "/app".to_string(),
            extensions: Vec::new(),
        }
    }

    /// 从 composer.json 解析 PHP 版本
    fn parse_php_version(app: &App) -> Option<String> {
        let content = app.read_file("composer.json").ok()?;
        let json: serde_json::Value = serde_json::from_str(&content).ok()?;
        let php_constraint = json.get("require")
            .and_then(|r| r.get("php"))
            .and_then(|v| v.as_str())?;

        // 从约束中提取版本号，如 "^8.2" → "8.2", ">=8.1" → "8.1"
        let re = Regex::new(r"(\d+\.\d+)").ok()?;
        re.captures(php_constraint)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
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

    /// 生成 Caddyfile 内容
    fn render_caddyfile(root_dir: &str, port: &str) -> String {
        CADDYFILE_TEMPLATE
            .replace("{{ROOT_DIR}}", root_dir)
            .replace("{{PORT}}", port)
    }
}

impl Provider for PhpProvider {
    fn name(&self) -> &str {
        "php"
    }

    fn detect(&self, app: &App, _env: &Environment) -> Result<bool> {
        Ok(app.has_file("composer.json") || app.has_file("index.php"))
    }

    fn initialize(&mut self, ctx: &mut GenerateContext) -> Result<()> {
        // PHP 版本
        if let Some(version) = Self::parse_php_version(&ctx.app) {
            self.php_version = version;
        }
        if let (Some(version), _) = ctx.env.get_config_variable("PHP_VERSION") {
            self.php_version = version;
        }

        // Laravel 检测
        self.is_laravel = laravel::detect_laravel(&ctx.app);

        // 根目录
        if self.is_laravel {
            self.root_dir = laravel::get_root_dir(&ctx.env);
        } else if let (Some(root), _) = ctx.env.get_config_variable("PHP_ROOT_DIR") {
            self.root_dir = root;
        }

        // PHP 扩展
        self.extensions = extensions::detect_extensions(&ctx.app, &ctx.env, self.is_laravel);

        Ok(())
    }

    fn plan(&self, ctx: &mut GenerateContext) -> Result<()> {
        // 元数据
        ctx.metadata.set("phpVersion", &self.php_version);
        ctx.metadata.set_bool("phpLaravel", self.is_laravel);
        ctx.metadata.set("phpRootDir", &self.root_dir);

        // === image 步骤：FrankenPHP 基础镜像 ===
        let php_version = self.php_version.clone();
        let image_step = ctx.new_image_step(
            "packages:php",
            Box::new(move |_opts| {
                format!(
                    "dunglas/frankenphp:php{}-bookworm",
                    php_version
                )
            }),
        );
        image_step.apt_packages = vec![
            "git".to_string(),
            "zip".to_string(),
            "unzip".to_string(),
            "ca-certificates".to_string(),
        ];

        // === extensions 步骤：安装 PHP 扩展 ===
        if let Some(ext_cmd) = extensions::install_command(&self.extensions) {
            let ext_step = ctx.new_command_step("extensions");
            ext_step.add_input(Layer::new_step_layer("packages:php", None));
            {
                let ext_step = Self::get_command_step(&mut ctx.steps, "extensions");
                ext_step.add_command(Command::new_exec(ext_cmd));
            }
        }

        // === prepare 步骤：写入配置文件 ===
        let prev_step = if self.extensions.is_empty() {
            "packages:php"
        } else {
            "extensions"
        };

        let prepare = ctx.new_command_step("prepare");
        prepare.add_input(Layer::new_step_layer(prev_step, None));
        {
            let prepare = Self::get_command_step(&mut ctx.steps, "prepare");

            // Caddyfile
            let caddyfile = Self::render_caddyfile(&self.root_dir, "${PORT:-8080}");
            prepare.add_command(Command::new_file("/etc/caddy/Caddyfile", caddyfile));

            // php.ini
            prepare.add_command(Command::new_file(
                "/usr/local/etc/php/conf.d/99-arcpack.ini",
                PHP_INI_TEMPLATE.to_string(),
            ));

            // start-container.sh
            prepare.add_command(Command::new_file(
                "/start-container.sh",
                START_CONTAINER_SCRIPT.to_string(),
            ));
            prepare.add_command(Command::new_exec("chmod +x /start-container.sh"));
        }

        // === install:composer 步骤 ===
        if ctx.app.has_file("composer.json") {
            let install = ctx.new_command_step("install:composer");
            install.add_input(Layer::new_step_layer("prepare", None));
            {
                let local_layer = ctx.new_local_layer();
                let install = Self::get_command_step(&mut ctx.steps, "install:composer");
                install.add_input(local_layer);

                // 从 composer 镜像复制 composer 二进制
                install.add_input(Layer::new_image_layer(
                    "composer:latest".to_string(),
                    Some(Filter::include_only(vec![
                        "/usr/bin/composer".to_string(),
                    ])),
                ));

                install.add_command(Command::new_exec(
                    "composer install --optimize-autoloader --no-scripts --no-interaction",
                ));
            }

            // Composer 缓存
            let cache_name = ctx.caches.add_cache("composer-cache", "/root/.composer/cache");
            {
                let install = Self::get_command_step(&mut ctx.steps, "install:composer");
                install.add_cache(&cache_name);
            }
        }

        // === build 步骤（Laravel 优化命令） ===
        if self.is_laravel {
            let composer_step = if ctx.app.has_file("composer.json") {
                "install:composer"
            } else {
                "prepare"
            };

            let build = ctx.new_command_step("build");
            build.add_input(Layer::new_step_layer(composer_step, None));
            {
                let build = Self::get_command_step(&mut ctx.steps, "build");
                for cmd in laravel::get_build_commands(&ctx.env) {
                    build.add_command(Command::new_exec(cmd));
                }
            }
        }

        // === Deploy 配置 ===
        ctx.deploy.start_cmd = Some("/start-container.sh".to_string());

        // Deploy 环境变量
        let deploy_vars = HashMap::from([
            ("APP_ENV".to_string(), "production".to_string()),
            ("SERVER_NAME".to_string(), ":${PORT:-8080}".to_string()),
        ]);
        for (k, v) in deploy_vars {
            ctx.deploy.variables.insert(k, v);
        }

        // deploy inputs: image 层 + 最后步骤输出
        let last_step = if self.is_laravel {
            "build"
        } else if ctx.app.has_file("composer.json") {
            "install:composer"
        } else {
            "prepare"
        };

        let output_layer = Layer::new_step_layer(
            last_step,
            Some(Filter::include_only(vec![".".to_string()])),
        );

        ctx.deploy.add_inputs(&[output_layer]);

        Ok(())
    }

    fn start_command_help(&self) -> Option<String> {
        Some(
            "To configure your start command, arcpack will:\n\n\
             1. Use FrankenPHP (Caddy) as the PHP server\n\
             2. Install Composer dependencies\n\
             3. For Laravel: run artisan optimization commands\n\
             4. Use /start-container.sh as the start command"
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::resolver::VersionResolver;
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
    fn test_detect_with_composer_json() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("composer.json"), "{}").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = PhpProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_with_index_php() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("index.php"), "<?php echo 'hi'; ?>").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = PhpProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_empty() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = PhpProvider::new();
        assert!(!provider.detect(&app, &env).unwrap());
    }

    // === 版本解析测试 ===

    #[test]
    fn test_parse_php_version() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("composer.json"),
            r#"{"require": {"php": "^8.2"}}"#,
        )
        .unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(PhpProvider::parse_php_version(&app), Some("8.2".to_string()));
    }

    #[test]
    fn test_parse_php_version_gte() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("composer.json"),
            r#"{"require": {"php": ">=8.1"}}"#,
        )
        .unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(PhpProvider::parse_php_version(&app), Some("8.1".to_string()));
    }

    #[test]
    fn test_version_from_env() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("composer.json"), "{}").unwrap();
        let mut ctx = make_ctx_with_env(
            &dir,
            HashMap::from([("ARCPACK_PHP_VERSION".to_string(), "8.3".to_string())]),
        );
        let mut provider = PhpProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.php_version, "8.3");
    }

    // === Laravel 检测测试 ===

    #[test]
    fn test_laravel_detection() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("composer.json"), "{}").unwrap();
        fs::write(dir.path().join("artisan"), "#!/usr/bin/env php").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = PhpProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert!(provider.is_laravel);
        assert_eq!(provider.root_dir, "/app/public");
    }

    // === template 测试 ===

    #[test]
    fn test_render_caddyfile() {
        let result = PhpProvider::render_caddyfile("/app/public", "${PORT:-8080}");
        assert!(result.contains("/app/public"));
        assert!(result.contains("${PORT:-8080}"));
        assert!(result.contains("frankenphp"));
    }

    // === plan 测试 ===

    #[test]
    fn test_plan_basic_php() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("index.php"), "<?php echo 'hi'; ?>").unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = PhpProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        let step_names: Vec<&str> = ctx.steps.iter().map(|s| s.name()).collect();
        assert!(step_names.contains(&"packages:php"));
        assert!(step_names.contains(&"prepare"));

        assert_eq!(
            ctx.deploy.start_cmd.as_deref(),
            Some("/start-container.sh")
        );
    }

    #[test]
    fn test_plan_with_composer() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("composer.json"),
            r#"{"require": {"php": "^8.2"}}"#,
        )
        .unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = PhpProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        let step_names: Vec<&str> = ctx.steps.iter().map(|s| s.name()).collect();
        assert!(step_names.contains(&"install:composer"));
        assert!(ctx.caches.get_cache("composer-cache").is_some());
    }

    #[test]
    fn test_plan_laravel() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("composer.json"),
            r#"{"require": {"php": "^8.2"}}"#,
        )
        .unwrap();
        fs::write(dir.path().join("artisan"), "#!/usr/bin/env php").unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = PhpProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        let step_names: Vec<&str> = ctx.steps.iter().map(|s| s.name()).collect();
        assert!(step_names.contains(&"build"));
        assert_eq!(ctx.metadata.get("phpLaravel"), Some("true"));
    }
}
