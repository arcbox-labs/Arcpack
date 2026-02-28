/// Elixir Provider：mix.exs 检测 + mix release
///
/// 对齐 railpack `core/providers/elixir/elixir.go`
/// 支持 Elixir/Erlang 版本映射、Phoenix 检测、mix release。

use std::collections::HashMap;

use regex::Regex;

use crate::app::App;
use crate::app::environment::Environment;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::{self, MiseStepBuilder};
use crate::generate::GenerateContext;
use crate::plan::{Command, Filter, Layer};
use crate::provider::Provider;
use crate::provider::node::NodeProvider;
use crate::Result;

/// 默认版本
const DEFAULT_ELIXIR_VERSION: &str = "1.18";
const DEFAULT_ERLANG_VERSION: &str = "27.3";

/// Elixir → Erlang/OTP 最低版本映射
/// 每个 Elixir minor 版本对应的最低 OTP 版本
const ELIXIR_ERLANG_MAP: &[(u32, u32, &str)] = &[
    (1, 0, "18"),
    (1, 1, "18"),
    (1, 2, "19"),
    (1, 3, "19"),
    (1, 4, "20"),
    (1, 5, "20"),
    (1, 6, "21"),
    (1, 7, "22"),
    (1, 8, "22"),
    (1, 9, "22"),
    (1, 10, "23"),
    (1, 11, "24"),
    (1, 12, "24"),
    (1, 13, "25"),
    (1, 14, "26"),
    (1, 15, "26"),
    (1, 16, "26"),
    (1, 17, "27"),
    (1, 18, "27"),
    (1, 19, "28"),
];

/// Elixir Provider
pub struct ElixirProvider {
    /// Elixir 版本
    elixir_version: String,
    /// Erlang/OTP 版本
    erlang_version: String,
    /// 应用名（从 mix.exs 解析）
    app_name: Option<String>,
    /// 是否为 Phoenix 应用
    is_phoenix: bool,
    /// 是否有 assets 目录
    has_assets: bool,
}

impl ElixirProvider {
    pub fn new() -> Self {
        Self {
            elixir_version: DEFAULT_ELIXIR_VERSION.to_string(),
            erlang_version: DEFAULT_ERLANG_VERSION.to_string(),
            app_name: None,
            is_phoenix: false,
            has_assets: false,
        }
    }

    /// 从 mix.exs 解析 app name
    fn parse_app_name(content: &str) -> Option<String> {
        let re = Regex::new(r"app:\s*:(\w+)").ok()?;
        re.captures(content)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }

    /// 从 mix.exs 解析 Elixir 版本约束
    fn parse_elixir_version(content: &str) -> Option<String> {
        // elixir: "~> 1.17" 或 elixir: ">= 1.14.0"
        let re = Regex::new(r#"elixir:\s*"[~><=\s]*(\d+\.\d+)"#).ok()?;
        re.captures(content)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }

    /// 根据 Elixir 版本查找对应的最低 Erlang/OTP 版本
    fn elixir_to_erlang(elixir_version: &str) -> Option<String> {
        let parts: Vec<&str> = elixir_version.split('.').collect();
        if parts.len() < 2 {
            return None;
        }
        let major: u32 = parts[0].parse().ok()?;
        let minor: u32 = parts[1].parse().ok()?;

        // 从映射表查找
        for &(m, n, otp) in ELIXIR_ERLANG_MAP.iter().rev() {
            if major > m || (major == m && minor >= n) {
                return Some(otp.to_string());
            }
        }
        None
    }

    /// 检测 Phoenix 框架
    fn detect_phoenix(app: &App) -> bool {
        if let Ok(content) = app.read_file("mix.exs") {
            return content.contains(":phoenix");
        }
        false
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

impl Provider for ElixirProvider {
    fn name(&self) -> &str {
        "elixir"
    }

    fn detect(&self, app: &App, _env: &Environment) -> Result<bool> {
        Ok(app.has_file("mix.exs"))
    }

    fn initialize(&mut self, ctx: &mut GenerateContext) -> Result<()> {
        // mix.lock 缺失警告
        if !ctx.app.has_file("mix.lock") {
            ctx.logs.warn("mix.lock not found. It is recommended to commit mix.lock to version control for reproducible builds.");
        }

        // 从 mix.exs 解析
        if let Ok(content) = ctx.app.read_file("mix.exs") {
            self.app_name = Self::parse_app_name(&content);

            if let Some(version) = Self::parse_elixir_version(&content) {
                self.elixir_version = version;
            }
        }

        // Erlang 版本映射
        if let Some(otp) = Self::elixir_to_erlang(&self.elixir_version) {
            self.erlang_version = otp;
        }

        // 文件版本覆盖
        if let Ok(content) = ctx.app.read_file(".elixir-version") {
            let v = content.trim().to_string();
            if !v.is_empty() {
                self.elixir_version = v;
            }
        }
        if let Ok(content) = ctx.app.read_file(".erlang-version") {
            let v = content.trim().to_string();
            if !v.is_empty() {
                self.erlang_version = v;
            }
        }

        // 环境变量版本覆盖（最高优先级）
        if let (Some(version), _) = ctx.env.get_config_variable("ELIXIR_VERSION") {
            self.elixir_version = version;
        }
        if let (Some(version), _) = ctx.env.get_config_variable("ERLANG_VERSION") {
            self.erlang_version = version;
        }

        // Phoenix 检测
        self.is_phoenix = Self::detect_phoenix(&ctx.app);
        self.has_assets = ctx.app.has_file("assets/package.json")
            || ctx.app.has_match("assets");

        Ok(())
    }

    fn plan(&self, ctx: &mut GenerateContext) -> Result<()> {
        let app_name = self.app_name.as_deref().unwrap_or("app");

        // 元数据
        ctx.metadata.set("elixirVersion", &self.elixir_version);
        ctx.metadata.set("erlangVersion", &self.erlang_version);
        ctx.metadata.set("elixirAppName", app_name);
        ctx.metadata.set_bool("elixirPhoenix", self.is_phoenix);

        // === mise 步骤（构建期）：elixir + erlang ===
        Self::ensure_mise_step_builder(ctx);

        {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            let elixir_ref = mise.default_package(
                &mut ctx.resolver,
                "elixir",
                DEFAULT_ELIXIR_VERSION,
            );
            mise.version(
                &mut ctx.resolver,
                &elixir_ref,
                &self.elixir_version,
                "resolved",
            );

            let erlang_ref = mise.default_package(
                &mut ctx.resolver,
                "erlang",
                DEFAULT_ERLANG_VERSION,
            );
            mise.version(
                &mut ctx.resolver,
                &erlang_ref,
                &self.erlang_version,
                "resolved",
            );

            // 构建依赖
            mise.add_supporting_apt_package("build-essential");
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

            // Hex + Rebar
            install.add_variables(&HashMap::from([
                ("MIX_ENV".to_string(), "prod".to_string()),
                ("MIX_HOME".to_string(), "/root/.mix".to_string()),
            ]));

            // 预创建构建目录
            install.add_command(Command::new_exec("mkdir -p config deps _build"));

            install.add_command(Command::new_exec("mix local.hex --force"));
            install.add_command(Command::new_exec("mix local.rebar --force"));

            // 复制依赖文件
            install.add_command(Command::new_copy("mix.exs", "mix.exs"));
            if ctx.app.has_file("mix.lock") {
                install.add_command(Command::new_copy("mix.lock", "mix.lock"));
            }
            if ctx.app.has_match("config/**") {
                install.add_command(Command::new_copy("config/", "config/"));
            }

            // 安装依赖
            install.add_command(Command::new_exec("mix deps.get --only prod"));
            install.add_command(Command::new_exec("mix deps.compile"));

            // secrets 前缀
            install.use_secrets_with_prefix(&ctx.env, "MIX");
            install.use_secrets_with_prefix(&ctx.env, "ERL");
            install.use_secrets_with_prefix(&ctx.env, "ELIXIR");
            install.use_secrets_with_prefix(&ctx.env, "OTP");
        }

        // === Node.js 集成（assets/ 子目录）===
        let has_assets_node = ctx.app.has_file("assets/package.json");
        if has_assets_node {
            // 注册 Node.js 到 mise
            // TODO: NodeProvider::new() 未调用 initialize()，不会检测 Bun 等替代包管理器；
            //       当前仅用于 mise 注册和依赖安装，影响有限，后续如需 Bun 支持需补充初始化。
            let node = NodeProvider::new();
            node.install_mise_packages(ctx)?;

            // install:node 步骤
            let install_node = ctx.new_command_step("install:node");
            install_node.add_input(Layer::new_step_layer("install", None));
            {
                let has_npm_lock = ctx.app.has_file("assets/package-lock.json");
                let has_yarn_lock = ctx.app.has_file("assets/yarn.lock");
                let step = Self::get_command_step(&mut ctx.steps, "install:node");

                // 复制 assets 目录的 Node.js 文件
                step.add_command(Command::new_copy("assets/package.json", "assets/package.json"));
                if has_npm_lock {
                    step.add_command(Command::new_copy(
                        "assets/package-lock.json",
                        "assets/package-lock.json",
                    ));
                }
                if has_yarn_lock {
                    step.add_command(Command::new_copy(
                        "assets/yarn.lock",
                        "assets/yarn.lock",
                    ));
                }

                // 在 assets/ 子目录安装 Node.js 依赖
                if has_npm_lock {
                    step.add_command(Command::new_exec_shell("cd assets && npm ci"));
                } else if has_yarn_lock {
                    step.add_command(Command::new_exec_shell(
                        "cd assets && yarn install --frozen-lockfile",
                    ));
                } else {
                    step.add_command(Command::new_exec_shell("cd assets && npm install"));
                }
            }
        }

        // === build 步骤 ===
        let build_input = if has_assets_node { "install:node" } else { "install" };
        let build = ctx.new_command_step("build");
        build.add_input(Layer::new_step_layer(build_input, None));
        {
            let local_layer = ctx.new_local_layer();
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_input(local_layer);

            build.add_variables(&HashMap::from([
                ("MIX_ENV".to_string(), "prod".to_string()),
                ("MIX_HOME".to_string(), "/root/.mix".to_string()),
            ]));

            // 编译
            build.add_command(Command::new_exec("mix compile"));

            // Phoenix 资产部署（检查 mix.exs 中是否定义了对应别名）
            if self.is_phoenix {
                if let Ok(mix_content) = ctx.app.read_file("mix.exs") {
                    for alias in ["assets.setup", "assets.deploy", "ecto.deploy"] {
                        if mix_content.contains(alias) {
                            build.add_command(Command::new_exec(format!("mix {alias}")));
                        }
                    }
                }
            }

            // Mix release
            build.add_command(Command::new_exec("mix release"));
        }

        // 缓存
        let cache_name = ctx.caches.add_cache("mix-deps", "/app/deps");
        {
            let install = Self::get_command_step(&mut ctx.steps, "install");
            install.add_cache(&cache_name);
        }

        let cache_name = ctx.caches.add_cache("mix-build", "/app/_build");
        {
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_cache(&cache_name);
        }

        // === Deploy 配置 ===
        ctx.deploy.start_cmd = Some(format!(
            "/app/_build/prod/rel/{}/bin/{} start",
            app_name, app_name
        ));

        // Deploy 环境变量
        let deploy_vars = HashMap::from([
            ("LANG".to_string(), "en_US.UTF-8".to_string()),
            ("LANGUAGE".to_string(), "en_US:en".to_string()),
            ("LC_ALL".to_string(), "en_US.UTF-8".to_string()),
            ("MIX_ENV".to_string(), "prod".to_string()),
            ("MIX_HOME".to_string(), "/root/.mix".to_string()),
            ("MIX_ARCHIVES".to_string(), "/root/.mix/archives".to_string()),
            (
                "ELIXIR_ERL_OPTIONS".to_string(),
                "+fnu".to_string(),
            ),
        ]);
        for (k, v) in deploy_vars {
            ctx.deploy.variables.insert(k, v);
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
             1. Install dependencies with `mix deps.get`\n\
             2. Build a release with `mix release`\n\
             3. Use `/app/_build/prod/rel/<app>/bin/<app> start` as the start command"
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

    fn basic_mix_exs() -> &'static str {
        r#"defmodule MyApp.MixProject do
  use Mix.Project

  def project do
    [
      app: :my_app,
      version: "0.1.0",
      elixir: "~> 1.17",
      start_permanent: Mix.env() == :prod,
      deps: deps()
    ]
  end

  defp deps do
    []
  end
end
"#
    }

    // === detect 测试 ===

    #[test]
    fn test_detect_with_mix_exs() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("mix.exs"), basic_mix_exs()).unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = ElixirProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_without_mix_exs() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = ElixirProvider::new();
        assert!(!provider.detect(&app, &env).unwrap());
    }

    // === 解析测试 ===

    #[test]
    fn test_parse_app_name() {
        assert_eq!(
            ElixirProvider::parse_app_name("app: :my_app,"),
            Some("my_app".to_string())
        );
        assert_eq!(
            ElixirProvider::parse_app_name("nothing here"),
            None
        );
    }

    #[test]
    fn test_parse_elixir_version() {
        assert_eq!(
            ElixirProvider::parse_elixir_version(r#"elixir: "~> 1.17""#),
            Some("1.17".to_string())
        );
        assert_eq!(
            ElixirProvider::parse_elixir_version(r#"elixir: ">= 1.14.0""#),
            Some("1.14".to_string())
        );
    }

    // === 版本映射测试 ===

    #[test]
    fn test_elixir_to_erlang() {
        assert_eq!(ElixirProvider::elixir_to_erlang("1.17"), Some("27".to_string()));
        assert_eq!(ElixirProvider::elixir_to_erlang("1.14"), Some("26".to_string()));
        assert_eq!(ElixirProvider::elixir_to_erlang("1.10"), Some("23".to_string()));
        assert_eq!(ElixirProvider::elixir_to_erlang("1.6"), Some("21".to_string()));
        assert_eq!(ElixirProvider::elixir_to_erlang("1.0"), Some("18".to_string()));
    }

    // === Phoenix 检测测试 ===

    #[test]
    fn test_detect_phoenix() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("mix.exs"),
            "defp deps do\n  [{:phoenix, \"~> 1.7\"}]\nend",
        )
        .unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert!(ElixirProvider::detect_phoenix(&app));
    }

    #[test]
    fn test_detect_not_phoenix() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("mix.exs"), basic_mix_exs()).unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert!(!ElixirProvider::detect_phoenix(&app));
    }

    // === plan 测试 ===

    #[test]
    fn test_plan_basic() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("mix.exs"), basic_mix_exs()).unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = ElixirProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        let step_names: Vec<&str> = ctx.steps.iter().map(|s| s.name()).collect();
        assert!(step_names.contains(&"install"));
        assert!(step_names.contains(&"build"));

        assert_eq!(
            ctx.deploy.start_cmd.as_deref(),
            Some("/app/_build/prod/rel/my_app/bin/my_app start")
        );

        assert_eq!(ctx.metadata.get("elixirAppName"), Some("my_app"));
        assert_eq!(ctx.metadata.get("elixirVersion"), Some("1.17"));
        assert_eq!(ctx.metadata.get("erlangVersion"), Some("27"));

        assert_eq!(
            ctx.deploy.variables.get("MIX_ENV").map(|s| s.as_str()),
            Some("prod")
        );
    }

    #[test]
    fn test_version_from_env() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("mix.exs"), basic_mix_exs()).unwrap();

        let mut ctx = make_ctx_with_env(
            &dir,
            HashMap::from([
                ("ARCPACK_ELIXIR_VERSION".to_string(), "1.16".to_string()),
                ("ARCPACK_ERLANG_VERSION".to_string(), "26".to_string()),
            ]),
        );
        let mut provider = ElixirProvider::new();
        provider.initialize(&mut ctx).unwrap();

        assert_eq!(provider.elixir_version, "1.16");
        assert_eq!(provider.erlang_version, "26");
    }
}
