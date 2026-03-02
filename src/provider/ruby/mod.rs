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

/// gem → APT 构建依赖映射
const GEM_BUILD_APT_DEPS: &[(&str, &[&str])] = &[
    ("pg", &["libpq-dev"]),
    ("mysql2", &["default-libmysqlclient-dev"]),
    ("magick", &["libmagickwand-dev"]),
    ("mini_magick", &["libmagickwand-dev"]),
    ("rmagick", &["libmagickwand-dev"]),
    ("ruby-vips", &["libvips-dev"]),
    ("vips", &["libvips-dev"]),
    (
        "charlock_holmes",
        &["libicu-dev", "libxml2-dev", "libxslt-dev"],
    ),
    ("nokogiri", &["libxml2-dev", "libxslt-dev"]),
    ("sqlite3", &["libsqlite3-dev"]),
];

/// gem → APT 运行时依赖映射
const GEM_RUNTIME_APT_DEPS: &[(&str, &[&str])] = &[
    ("pg", &["libpq5"]),
    ("mysql2", &["default-libmysqlclient-dev"]),
    ("magick", &["libmagickwand-dev"]),
    ("mini_magick", &["libmagickwand-dev"]),
    ("rmagick", &["libmagickwand-dev"]),
    ("ruby-vips", &["libvips42"]),
    ("vips", &["libvips42"]),
    ("charlock_holmes", &["libicu-dev"]),
    ("sqlite3", &["libsqlite3-0"]),
];

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
        if self.has_bin_rails {
            return "bundle exec bin/rails server -b 0.0.0.0 -p ${PORT:-3000}".to_string();
        }
        if self.has_config_ru {
            return "bundle exec rackup -o 0.0.0.0 -p ${PORT:-3000}".to_string();
        }
        "bundle exec rake".to_string()
    }

    /// 根据 gem 列表检测构建时 APT 依赖
    fn get_build_apt_packages(&self) -> Vec<String> {
        let mut packages = Vec::new();
        for (gem, deps) in GEM_BUILD_APT_DEPS {
            if self.gems.iter().any(|g| g == *gem) {
                for dep in *deps {
                    if !packages.contains(&dep.to_string()) {
                        packages.push(dep.to_string());
                    }
                }
            }
        }
        packages
    }

    /// 根据 gem 列表检测运行时 APT 依赖
    fn get_runtime_apt_packages(&self) -> Vec<String> {
        let mut packages = Vec::new();
        for (gem, deps) in GEM_RUNTIME_APT_DEPS {
            if self.gems.iter().any(|g| g == *gem) {
                for dep in *deps {
                    if !packages.contains(&dep.to_string()) {
                        packages.push(dep.to_string());
                    }
                }
            }
        }
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

        // 资产管道检测
        if self.is_rails {
            self.has_asset_pipeline = self
                .gems
                .iter()
                .any(|g| g == "sprockets" || g == "sprockets-rails" || g == "propshaft");
            self.has_bootsnap = self.gems.iter().any(|g| g == "bootsnap");
        }

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
            mise.add_supporting_apt_package("zlib1g-dev");
            mise.add_supporting_apt_package("libffi-dev");
            mise.add_supporting_apt_package("procps");

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

            // gem 特定构建依赖
            for pkg in self.get_build_apt_packages() {
                mise.add_supporting_apt_package(&pkg);
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

        // === install 步骤 ===
        let install = ctx.new_command_step("install");
        install.add_input(Layer::new_step_layer(&mise_step_name, None));
        {
            let install = Self::get_command_step(&mut ctx.steps, "install");

            // 安装 bundler
            if let Some(ref bundler_ver) = self.bundler_version {
                install.add_command(Command::new_exec(format!(
                    "gem install bundler:{}",
                    bundler_ver
                )));
            } else {
                install.add_command(Command::new_exec("gem install bundler"));
            }

            // 复制 Gemfile + Gemfile.lock
            install.add_command(Command::new_copy("Gemfile", "Gemfile"));
            if ctx.app.has_file("Gemfile.lock") {
                install.add_command(Command::new_copy("Gemfile.lock", "Gemfile.lock"));
            }

            // .ruby-version 如果存在
            if ctx.app.has_file(".ruby-version") {
                install.add_command(Command::new_copy(".ruby-version", ".ruby-version"));
            }

            // 复制本地 gem 路径
            if let Ok(gemfile_content) = ctx.app.read_file("Gemfile") {
                for gem_path in Self::parse_local_gem_paths(&gemfile_content) {
                    if ctx.app.has_match(&gem_path) {
                        install.add_command(Command::new_copy(&gem_path, &gem_path));
                    }
                }
            }

            // bundle install
            install.add_command(Command::new_exec(
                "bundle install --jobs=4 --retry=3 --without development test",
            ));

            // Secrets 前缀
            install.use_secrets_with_prefix(&ctx.env, "RUBY");
            install.use_secrets_with_prefix(&ctx.env, "GEM");
            install.use_secrets_with_prefix(&ctx.env, "BUNDLE");
        }

        // gem 缓存
        let cache_name = ctx
            .caches
            .add_cache("bundle-cache", "/usr/local/bundle/cache");
        {
            let install = Self::get_command_step(&mut ctx.steps, "install");
            install.add_cache(&cache_name);
        }

        // === build 步骤（Rails 资产编译等） ===
        let build = ctx.new_command_step("build");
        build.add_input(Layer::new_step_layer("install", None));
        {
            let local_layer = ctx.new_local_layer();
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_input(local_layer);

            // build Secrets 前缀
            build.use_secrets_with_prefix(&ctx.env, "RAILS");
            build.use_secrets_with_prefix(&ctx.env, "BUNDLE");
            build.use_secrets_with_prefix(&ctx.env, "BOOTSNAP");

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

        // === Node.js 集成 ===
        let has_package_json = ctx.app.has_file("package.json");
        let has_execjs = self.gems.iter().any(|g| g == "execjs");
        let has_node = has_package_json || has_execjs;

        if has_node {
            let mut node = NodeProvider::new();
            node.initialize(ctx)?;

            // 注册 Node.js 到 mise（execjs 仅需运行时，不需要 npm install）
            node.install_mise_packages(ctx)?;

            // 有 package.json 时才运行完整 Node.js 构建流水线
            if has_package_json {
                // install:node 步骤
                let install_node = ctx.new_command_step("install:node");
                install_node.add_input(Layer::new_step_layer("install", None));
                node.install_node_deps(ctx, "install:node")?;

                // build:node 步骤
                let build_node = ctx.new_command_step("build:node");
                build_node.add_input(Layer::new_step_layer("install:node", None));
                node.build_node(ctx, "build:node")?;

                // prune:node 步骤（移除 devDependencies）
                let prune_node = ctx.new_command_step("prune:node");
                prune_node.add_input(Layer::new_step_layer("build:node", None));
                node.prune_node_deps(ctx, "prune:node")?;
            }
        }

        // === Deploy 配置 ===
        ctx.deploy.start_cmd = Some(self.get_start_command());

        // Deploy 环境变量
        let deploy_vars = HashMap::from([
            ("BUNDLE_GEMFILE".to_string(), "/app/Gemfile".to_string()),
            ("GEM_HOME".to_string(), "/usr/local/bundle".to_string()),
            ("GEM_PATH".to_string(), "/usr/local/bundle".to_string()),
            ("MALLOC_ARENA_MAX".to_string(), "2".to_string()),
            ("LD_PRELOAD".to_string(), "libjemalloc.so.2".to_string()),
            ("RAILS_ENV".to_string(), "production".to_string()),
            ("RAILS_LOG_TO_STDOUT".to_string(), "enabled".to_string()),
            ("RAILS_SERVE_STATIC_FILES".to_string(), "true".to_string()),
        ]);
        for (k, v) in deploy_vars {
            ctx.deploy.variables.insert(k, v);
        }

        ctx.deploy.paths.push("/usr/local/bundle/bin".to_string());

        // 运行时 APT 包
        let mut runtime_apt = vec!["libjemalloc2".to_string(), "libyaml-0-2".to_string()];
        runtime_apt.extend(self.get_runtime_apt_packages());
        ctx.deploy.add_apt_packages(&runtime_apt);

        // deploy inputs
        let mise_layer = ctx
            .mise_step_builder
            .as_ref()
            .map(|m| m.get_layer())
            .unwrap_or_default();

        let build_step_name = if has_package_json {
            "prune:node"
        } else {
            "build"
        };
        let build_layer = Layer::new_step_layer(
            build_step_name,
            Some(Filter::include_only(vec![".".to_string()])),
        );

        ctx.deploy.add_inputs(&[mise_layer, build_layer]);

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
        let mut p = RubyProvider::new();
        p.gems = vec!["pg".to_string()];
        let build_deps = p.get_build_apt_packages();
        assert!(build_deps.contains(&"libpq-dev".to_string()));
        let runtime_deps = p.get_runtime_apt_packages();
        assert!(runtime_deps.contains(&"libpq5".to_string()));
    }

    // === start_cmd 测试 ===

    #[test]
    fn test_start_cmd_rails() {
        let mut p = RubyProvider::new();
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
        let p = RubyProvider::new();
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

        // 运行时 APT（pg → libpq5）
        assert!(ctx.deploy.apt_packages.contains(&"libpq5".to_string()));
        assert!(ctx
            .deploy
            .apt_packages
            .contains(&"libjemalloc2".to_string()));
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
