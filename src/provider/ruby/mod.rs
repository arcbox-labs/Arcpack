/// Ruby Provider：Gemfile 检测 + bundle install + Rails 支持
///
/// 对齐 railpack `core/providers/ruby/ruby.go`
/// 支持 Ruby 版本解析、Bundler 版本、Rails 资产编译、gem 依赖 APT 包。
use std::collections::HashMap;

use regex::Regex;

use crate::app::environment::Environment;
use crate::app::App;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::{self, MiseStepBuilder};
use crate::generate::GenerateContext;
use crate::plan::{Command, Filter, Layer};
use crate::provider::node::NodeProvider;
use crate::provider::Provider;
use crate::Result;

/// 默认 Ruby 版本
const DEFAULT_RUBY_VERSION: &str = "3.4.6";

/// Ruby Provider
pub struct RubyProvider {
    /// Ruby 版本
    ruby_version: String,
    /// Bundler 版本
    bundler_version: Option<String>,
    /// 已检测的 gem 列表
    gems: Vec<String>,
    /// 是否为 Rails 应用
    is_rails: bool,
    /// 是否有 sprockets/propshaft（资产管理）
    has_asset_pipeline: bool,
    /// 是否有 bootsnap
    has_bootsnap: bool,
    /// 是否有 config.ru（Rack 应用）
    has_config_ru: bool,
    /// 是否有 bin/rails
    has_bin_rails: bool,
    /// 是否有 rails 可执行文件
    has_rails_script: bool,
    /// 是否有 config/environment.rb
    has_config_environment: bool,
    /// 是否有 script 目录
    has_script_dir: bool,
    /// 是否有 Rakefile
    has_rakefile: bool,
}

impl RubyProvider {
    pub fn new() -> Self {
        Self {
            ruby_version: DEFAULT_RUBY_VERSION.to_string(),
            bundler_version: None,
            gems: Vec::new(),
            is_rails: false,
            has_asset_pipeline: false,
            has_bootsnap: false,
            has_config_ru: false,
            has_bin_rails: false,
            has_rails_script: false,
            has_config_environment: false,
            has_script_dir: false,
            has_rakefile: false,
        }
    }

    /// 从 Gemfile 解析 Ruby 版本
    fn parse_ruby_version_from_gemfile(content: &str) -> Option<String> {
        let re = Regex::new(r#"ruby\s+["'](\d+\.\d+\.\d+)["']"#).ok()?;
        re.captures(content)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }

    /// 从 Gemfile.lock 的 RUBY VERSION 段解析版本
    fn parse_ruby_version_from_lockfile(content: &str) -> Option<String> {
        let re = Regex::new(r"RUBY VERSION\s+ruby\s+(\d+\.\d+\.\d+)").ok()?;
        re.captures(content)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }

    /// 从 Gemfile.lock 的 BUNDLED WITH 段解析 Bundler 版本
    fn parse_bundler_version(content: &str) -> Option<String> {
        let re = Regex::new(r"BUNDLED WITH\s+(\d+\.\d+\.\d+)").ok()?;
        re.captures(content)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }

    /// 从 Gemfile.lock 提取 gem 名列表
    fn parse_gems(content: &str) -> Vec<String> {
        let mut gems = Vec::new();
        let re = Regex::new(r"^\s{4}(\S+)\s").unwrap();
        let mut in_specs = false;
        for line in content.lines() {
            if line.trim() == "specs:" {
                in_specs = true;
                continue;
            }
            if in_specs {
                if line.starts_with("    ") && !line.starts_with("      ") {
                    if let Some(caps) = re.captures(line) {
                        if let Some(name) = caps.get(1) {
                            gems.push(name.as_str().to_string());
                        }
                    }
                }
                // 空行或新段落结束 specs
                if !line.starts_with(' ') && !line.is_empty() {
                    in_specs = false;
                }
            }
        }
        gems
    }

    /// 解析 Gemfile 中的 path: 引用（本地 gem 路径）
    fn parse_local_gem_paths(content: &str) -> Vec<String> {
        let mut paths = Vec::new();
        let re = Regex::new(
            r#"(?:gem\s+['"][^'"]+['"]\s*,.*path:\s*['"]([^'"]+)['"]|path\s+['"]([^'"]+)['"])"#,
        )
        .unwrap();
        for caps in re.captures_iter(content) {
            if let Some(m) = caps.get(1).or(caps.get(2)) {
                let path = m.as_str().to_string();
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
        paths
    }

    /// 检测 Rails 应用
    fn detect_rails(app: &App) -> bool {
        if let Ok(content) = app.read_file("config/application.rb") {
            return content.contains("Rails::Application");
        }
        false
    }

    /// 获取 start command
    fn get_start_command(&self) -> String {
        if self.is_rails {
            if self.has_rails_script {
                return "bundle exec rails server -b 0.0.0.0 -p ${PORT:-3000}".to_string();
            }
            if self.has_bin_rails {
                return "bundle exec bin/rails server -b 0.0.0.0 -p ${PORT:-3000} -e $RAILS_ENV"
                    .to_string();
            }
            return "bundle exec bin/rails server -b 0.0.0.0 -p ${PORT:-3000} -e $RAILS_ENV"
                .to_string();
        }

        if self.has_config_environment && self.has_script_dir {
            return "bundle exec ruby script/server -p ${PORT:-3000}".to_string();
        }

        if self.has_config_ru {
            return "bundle exec rackup config.ru -p ${PORT:-3000}".to_string();
        }
        if self.has_rakefile {
            return "bundle exec rake".to_string();
        }

        String::new()
    }

    fn get_ruby_env_vars() -> HashMap<String, String> {
        HashMap::from([
            ("BUNDLE_GEMFILE".to_string(), "/app/Gemfile".to_string()),
            ("GEM_HOME".to_string(), "/usr/local/bundle".to_string()),
            ("GEM_PATH".to_string(), "/usr/local/bundle".to_string()),
            ("MALLOC_ARENA_MAX".to_string(), "2".to_string()),
            (
                "LD_PRELOAD".to_string(),
                "/usr/lib/x86_64-linux-gnu/libjemalloc.so".to_string(),
            ),
        ])
    }

    fn uses_dep(ctx: &GenerateContext, dep: &str) -> bool {
        for file in ["Gemfile", "Gemfile.lock"] {
            if let Ok(content) = ctx.app.read_file(file) {
                if content.contains(dep) {
                    return true;
                }
            }
        }
        false
    }

    fn get_runtime_apt_packages(&self, ctx: &GenerateContext) -> Vec<String> {
        let mut packages = vec!["libyaml-dev".to_string(), "libjemalloc-dev".to_string()];

        if Self::uses_dep(ctx, "pg") {
            packages.push("libpq-dev".to_string());
        }
        if Self::uses_dep(ctx, "mysql") {
            packages.push("default-libmysqlclient-dev".to_string());
        }
        if Self::uses_dep(ctx, "magick") {
            packages.push("libmagickwand-dev".to_string());
        }
        if Self::uses_dep(ctx, "vips") {
            packages.push("libvips-dev".to_string());
        }
        if Self::uses_dep(ctx, "charlock_holmes") {
            packages.push("libicu-dev".to_string());
            packages.push("libxml2-dev".to_string());
            packages.push("libxslt-dev".to_string());
        }

        packages.sort();
        packages.dedup();
        packages
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

impl Provider for RubyProvider {
    fn name(&self) -> &str {
        "ruby"
    }

    fn detect(&self, app: &App, _env: &Environment) -> Result<bool> {
        Ok(app.has_file("Gemfile"))
    }

    fn initialize(&mut self, ctx: &mut GenerateContext) -> Result<()> {
        // 从 Gemfile 解析 Ruby 版本
        if let Ok(content) = ctx.app.read_file("Gemfile") {
            if let Some(version) = Self::parse_ruby_version_from_gemfile(&content) {
                self.ruby_version = version;
            }
        }

        // 从 Gemfile.lock 解析
        if let Ok(content) = ctx.app.read_file("Gemfile.lock") {
            // Ruby 版本（覆盖 Gemfile 的）
            if let Some(version) = Self::parse_ruby_version_from_lockfile(&content) {
                self.ruby_version = version;
            }
            // Bundler 版本
            self.bundler_version = Self::parse_bundler_version(&content);
            // gem 列表
            self.gems = Self::parse_gems(&content);
        }

        // 环境变量版本覆盖
        if let (Some(version), _) = ctx.env.get_config_variable("RUBY_VERSION") {
            self.ruby_version = version;
        }

        // Rails 检测
        self.is_rails = Self::detect_rails(&ctx.app);
        self.has_config_ru = ctx.app.has_file("config.ru");
        self.has_bin_rails = ctx.app.has_file("bin/rails");
        self.has_rails_script = ctx.app.has_file("rails");
        self.has_config_environment = ctx.app.has_file("config/environment.rb");
        self.has_script_dir = ctx.app.has_match("script");
        self.has_rakefile = ctx.app.has_file("Rakefile");

        self.has_asset_pipeline =
            Self::uses_dep(ctx, "sprockets") || Self::uses_dep(ctx, "propshaft");
        self.has_bootsnap = Self::uses_dep(ctx, "bootsnap");

        Ok(())
    }

    fn plan(&self, ctx: &mut GenerateContext) -> Result<()> {
        // 元数据
        ctx.metadata.set("rubyVersion", &self.ruby_version);
        ctx.metadata.set_bool("rubyRails", self.is_rails);
        ctx.metadata
            .set_bool("rubyAssetPipeline", self.has_asset_pipeline);
        ctx.metadata.set_bool("rubyBootsnap", self.has_bootsnap);

        // === mise 步骤：安装 Ruby ===
        Self::ensure_mise_step_builder(ctx);

        let ruby_ref = {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            let r = mise.default_package(&mut ctx.resolver, "ruby", DEFAULT_RUBY_VERSION);
            mise.version(&mut ctx.resolver, &r, &self.ruby_version, "resolved");

            // 构建依赖
            mise.add_supporting_apt_package("libyaml-dev");
            mise.add_supporting_apt_package("libjemalloc-dev");

            // YJIT 支持：Ruby >= 3.2 需要 rustc 和 cargo
            {
                let parts: Vec<&str> = self.ruby_version.splitn(3, '.').collect();
                let major_minor = parts
                    .first()
                    .and_then(|a| a.parse::<u32>().ok())
                    .zip(parts.get(1).and_then(|b| b.parse::<u32>().ok()));
                if major_minor.map_or(false, |(maj, min)| (maj, min) >= (3, 2)) {
                    mise.add_supporting_apt_package("rustc");
                    mise.add_supporting_apt_package("cargo");
                }
            }

            r
        };
        let _ = ruby_ref;

        // 环境变量版本覆盖
        if let (Some(env_version), var_name) = ctx.env.get_config_variable("RUBY_VERSION") {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            mise.version(&mut ctx.resolver, &ruby_ref, &env_version, &var_name);
        }

        let mise_step_name = ctx
            .mise_step_builder
            .as_ref()
            .map(|m| m.name().to_string())
            .unwrap_or_else(|| mise_step_builder::MISE_STEP_NAME.to_string());
        let ruby_env_vars = Self::get_ruby_env_vars();

        // === install 步骤 ===
        let install = ctx.new_command_step("install");
        install.add_input(Layer::new_step_layer(&mise_step_name, None));
        {
            let install = Self::get_command_step(&mut ctx.steps, "install");
            install.secrets = vec![];
            install.use_secrets_with_prefix(&ctx.env, "RUBY");
            install.use_secrets_with_prefix(&ctx.env, "GEM");
            install.use_secrets_with_prefix(&ctx.env, "BUNDLE");
            install.add_variables(&ruby_env_vars);

            // 安装 bundler
            if let Some(ref bundler_ver) = self.bundler_version {
                install.add_command(Command::new_exec(format!(
                    "gem install -N bundler:{}",
                    bundler_ver
                )));
            } else {
                install.add_command(Command::new_exec("gem install -N bundler"));
            }

            // 复制 Gemfile + Gemfile.lock
            install.add_command(Command::new_copy("Gemfile", "Gemfile"));
            install.add_command(Command::new_copy("Gemfile.lock", "Gemfile.lock"));

            // 复制本地 gem 路径
            if let Ok(gemfile_content) = ctx.app.read_file("Gemfile") {
                for gem_path in Self::parse_local_gem_paths(&gemfile_content) {
                    install.add_command(Command::new_copy(&gem_path, &gem_path));
                }
            }

            // bundle install
            install.add_command(Command::new_exec("bundle install"));

            if self.has_bootsnap {
                install.add_command(Command::new_exec("bundle exec bootsnap precompile --gemfile"));
            }

            install.add_command(Command::new_path("/usr/local/bundle"));
        }

        // === Node.js 集成 ===
        let mut node = NodeProvider::new();
        let node_detected = node.detect(&ctx.app, &ctx.env)?;
        if node_detected || Self::uses_dep(ctx, "execjs") {
            node.install_mise_packages(ctx)?;
        }

        if node_detected {
            node.initialize(ctx)?;
            node.install_mise_packages(ctx)?;

            let install_node = ctx.new_command_step("install:node");
            install_node.add_input(Layer::new_step_layer(&mise_step_name, None));
            node.install_node_deps(ctx, "install:node")?;

            let prune_node = ctx.new_command_step("prune:node");
            prune_node.add_input(Layer::new_step_layer("install:node", None));
            node.prune_node_deps(ctx, "prune:node")?;

            let mut install_node_include = vec![".".to_string()];
            if let Some(mise) = ctx.mise_step_builder.as_ref() {
                install_node_include.extend(mise.get_output_paths());
            }
            let build_node = ctx.new_command_step("build:node");
            build_node.add_input(Layer::new_step_layer("install", None));
            build_node.add_input(Layer::new_step_layer(
                "install:node",
                Some(Filter::include_only(install_node_include)),
            ));
            node.build_node(ctx, "build:node")?;
        }

        // === build 步骤（Rails 资产编译等） ===
        let build = ctx.new_command_step("build");
        build.add_input(Layer::new_step_layer("install", None));
        {
            let local_layer = ctx.new_local_layer();
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.secrets = vec![];
            build.add_input(local_layer);
            build.add_variables(&ruby_env_vars);

            // build Secrets 前缀
            build.use_secrets_with_prefix(&ctx.env, "RAILS");
            build.use_secrets_with_prefix(&ctx.env, "BUNDLE");
            build.use_secrets_with_prefix(&ctx.env, "BOOTSNAP");
            build.use_secrets_with_prefix(&ctx.env, "SPROCKETS");
            build.use_secrets_with_prefix(&ctx.env, "WEBPACKER");
            build.use_secrets_with_prefix(&ctx.env, "ASSET");
            build.use_secrets_with_prefix(&ctx.env, "DISABLE_SPRING");

            if self.is_rails {
                // 资产编译
                if self.has_asset_pipeline {
                    build.add_command(Command::new_exec("bundle exec rake assets:precompile"));
                }
                // bootsnap 预编译
                if self.has_bootsnap {
                    build.add_command(Command::new_exec(
                        "bundle exec bootsnap precompile app/ lib/",
                    ));
                }
            }
        }

        // === Deploy 配置 ===
        let start_cmd = self.get_start_command();
        if !start_cmd.is_empty() {
            ctx.deploy.start_cmd = Some(start_cmd);
        }

        for (k, v) in &ruby_env_vars {
            ctx.deploy.variables.insert(k.clone(), v.clone());
        }

        // 运行时 APT 包
        let runtime_apt = self.get_runtime_apt_packages(ctx);
        ctx.deploy.add_apt_packages(&runtime_apt);

        // deploy inputs
        let mise_layer = ctx
            .mise_step_builder
            .as_ref()
            .map(|m| m.get_layer())
            .unwrap_or_default();

        let install_layer = Layer::new_step_layer(
            "install",
            Some(Filter::include_only(vec!["/usr/local/bundle".to_string()])),
        );
        let mut ruby_build_outputs = vec!["/app".to_string()];
        if self.is_rails && self.has_bootsnap {
            ruby_build_outputs.push("lib/".to_string());
        }
        let ruby_build_layer = Layer::new_step_layer(
            "build",
            Some(Filter::include_only(ruby_build_outputs)),
        );
        ctx.deploy
            .add_inputs(&[mise_layer, install_layer, ruby_build_layer]);

        if node_detected {
            let node_modules_layer = Layer::new_step_layer(
                "prune:node",
                Some(Filter::include_only(vec!["/app/node_modules".to_string()])),
            );
            let build_node_layer = Layer::new_step_layer(
                "build:node",
                Some(Filter {
                    include: vec![".".to_string()],
                    exclude: vec!["node_modules".to_string(), ".yarn".to_string()],
                }),
            );
            ctx.deploy.add_inputs(&[node_modules_layer, build_node_layer]);
        }

        Ok(())
    }

    fn start_command_help(&self) -> Option<String> {
        Some(
            "To configure your start command, arcpack will:\n\n\
             For Rails: bundle exec bin/rails server\n\
             For Rack: bundle exec rackup\n\
             Default: bundle exec rake"
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

    fn basic_gemfile() -> &'static str {
        "source 'https://rubygems.org'\nruby '3.3.0'\ngem 'rails'\ngem 'pg'\n"
    }

    fn basic_lockfile() -> &'static str {
        r#"GEM
  remote: https://rubygems.org/
  specs:
    rails (7.1.3)
    pg (1.5.4)
    bootsnap (1.17.0)
    sprockets-rails (3.4.2)

PLATFORMS
  ruby

RUBY VERSION
   ruby 3.3.0p0

BUNDLED WITH
   2.5.6
"#
    }

    // === detect 测试 ===

    #[test]
    fn test_detect_with_gemfile() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Gemfile"), basic_gemfile()).unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = RubyProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_without_gemfile() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = RubyProvider::new();
        assert!(!provider.detect(&app, &env).unwrap());
    }

    // === 版本解析测试 ===

    #[test]
    fn test_parse_ruby_version_from_gemfile() {
        assert_eq!(
            RubyProvider::parse_ruby_version_from_gemfile("ruby '3.3.0'\n"),
            Some("3.3.0".to_string())
        );
        assert_eq!(
            RubyProvider::parse_ruby_version_from_gemfile("ruby \"3.2.2\"\n"),
            Some("3.2.2".to_string())
        );
        assert_eq!(
            RubyProvider::parse_ruby_version_from_gemfile("gem 'rails'\n"),
            None
        );
    }

    #[test]
    fn test_parse_ruby_version_from_lockfile() {
        assert_eq!(
            RubyProvider::parse_ruby_version_from_lockfile(basic_lockfile()),
            Some("3.3.0".to_string())
        );
    }

    #[test]
    fn test_parse_bundler_version() {
        assert_eq!(
            RubyProvider::parse_bundler_version(basic_lockfile()),
            Some("2.5.6".to_string())
        );
    }

    #[test]
    fn test_parse_gems() {
        let gems = RubyProvider::parse_gems(basic_lockfile());
        assert!(gems.contains(&"rails".to_string()));
        assert!(gems.contains(&"pg".to_string()));
        assert!(gems.contains(&"bootsnap".to_string()));
        assert!(gems.contains(&"sprockets-rails".to_string()));
    }

    // === Rails 检测测试 ===

    #[test]
    fn test_detect_rails() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(
            dir.path().join("config/application.rb"),
            "module MyApp\n  class Application < Rails::Application\n  end\nend",
        )
        .unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert!(RubyProvider::detect_rails(&app));
    }

    #[test]
    fn test_detect_not_rails() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert!(!RubyProvider::detect_rails(&app));
    }

    // === APT 依赖测试 ===

    #[test]
    fn test_apt_deps_for_pg_gem() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("Gemfile"),
            "source 'https://rubygems.org'\ngem 'pg'\n",
        )
        .unwrap();
        let ctx = make_ctx(&dir);
        let p = RubyProvider::new();
        let runtime_deps = p.get_runtime_apt_packages(&ctx);
        assert!(runtime_deps.contains(&"libpq-dev".to_string()));
    }

    // === start_cmd 测试 ===

    #[test]
    fn test_start_cmd_rails() {
        let mut p = RubyProvider::new();
        p.is_rails = true;
        p.has_bin_rails = true;
        assert!(p.get_start_command().contains("bin/rails server"));
    }

    #[test]
    fn test_start_cmd_rack() {
        let mut p = RubyProvider::new();
        p.has_config_ru = true;
        assert!(p.get_start_command().contains("rackup"));
    }

    #[test]
    fn test_start_cmd_default() {
        let mut p = RubyProvider::new();
        p.has_rakefile = true;
        assert_eq!(p.get_start_command(), "bundle exec rake");
    }

    // === plan 测试 ===

    #[test]
    fn test_plan_basic() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Gemfile"), basic_gemfile()).unwrap();
        fs::write(dir.path().join("Gemfile.lock"), basic_lockfile()).unwrap();
        fs::write(dir.path().join("config.ru"), "run MyApp").unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = RubyProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        let step_names: Vec<&str> = ctx.steps.iter().map(|s| s.name()).collect();
        assert!(step_names.contains(&"install"));
        assert!(step_names.contains(&"build"));

        // Rack 应用
        assert!(ctx.deploy.start_cmd.as_deref().unwrap().contains("rackup"));

        // 运行时 APT（pg → libpq-dev）
        assert!(ctx.deploy.apt_packages.contains(&"libpq-dev".to_string()));
        assert!(ctx
            .deploy
            .apt_packages
            .contains(&"libjemalloc-dev".to_string()));
    }

    #[test]
    fn test_plan_rails() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Gemfile"), basic_gemfile()).unwrap();
        fs::write(dir.path().join("Gemfile.lock"), basic_lockfile()).unwrap();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(
            dir.path().join("config/application.rb"),
            "class Application < Rails::Application; end",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("bin")).unwrap();
        fs::write(dir.path().join("bin/rails"), "#!/usr/bin/env ruby").unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = RubyProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert!(provider.is_rails);
        assert!(provider.has_asset_pipeline);
        assert!(provider.has_bootsnap);

        provider.plan(&mut ctx).unwrap();
        assert!(ctx
            .deploy
            .start_cmd
            .as_deref()
            .unwrap()
            .contains("bin/rails server"));
    }

    #[test]
    fn test_version_from_env() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Gemfile"), basic_gemfile()).unwrap();
        let mut ctx = make_ctx_with_env(
            &dir,
            HashMap::from([("ARCPACK_RUBY_VERSION".to_string(), "3.2.0".to_string())]),
        );
        let mut provider = RubyProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.ruby_version, "3.2.0");
    }
}
