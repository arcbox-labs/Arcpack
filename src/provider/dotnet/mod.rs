/// .NET Provider：*.csproj 检测 + dotnet publish
///
/// 对齐 railpack `core/providers/dotnet/dotnet.go`
/// 支持 TargetFramework 版本解析、global.json SDK 版本、NuGet restore。
use regex::Regex;

use crate::app::environment::Environment;
use crate::app::App;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::{self, MiseStepBuilder};
use crate::generate::GenerateContext;
use crate::plan::{Command, Filter, Layer};
use crate::provider::Provider;
use crate::Result;

/// 默认 .NET 版本
const DEFAULT_DOTNET_VERSION: &str = "8.0";

/// .NET Provider
pub struct DotnetProvider {
    /// 项目名（从 .csproj 文件名提取）
    project_name: Option<String>,
    /// .NET 版本
    dotnet_version: String,
    /// csproj 文件路径
    csproj_path: Option<String>,
}

impl DotnetProvider {
    pub fn new() -> Self {
        Self {
            project_name: None,
            dotnet_version: DEFAULT_DOTNET_VERSION.to_string(),
            csproj_path: None,
        }
    }

    /// 从 TargetFramework 或 TargetFrameworks（复数）提取 .NET 版本
    fn parse_target_framework(content: &str) -> Option<String> {
        // 单一目标框架
        let re =
            Regex::new(r"<TargetFramework>(?:net|netcoreapp)(\d+\.\d+)</TargetFramework>").ok()?;
        if let Some(caps) = re.captures(content) {
            return caps.get(1).map(|m| m.as_str().to_string());
        }

        // 多目标框架（分号分隔，取第一个）
        let re_plural = Regex::new(r"<TargetFrameworks>([^<]+)</TargetFrameworks>").ok()?;
        if let Some(caps) = re_plural.captures(content) {
            let frameworks = caps.get(1)?.as_str();
            let first = frameworks.split(';').next()?;
            let re_ver = Regex::new(r"(?:net|netcoreapp)(\d+\.\d+)").ok()?;
            return re_ver
                .captures(first)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string());
        }

        None
    }

    /// 从 global.json 解析 SDK 版本
    fn parse_global_json_version(app: &App) -> Option<String> {
        let content = app.read_file("global.json").ok()?;
        let json: serde_json::Value = serde_json::from_str(&content).ok()?;
        json.get("sdk")
            .and_then(|sdk| sdk.get("version"))
            .and_then(|v| v.as_str())
            .map(|v| {
                // 提取主版本号 "8.0.100" → "8.0"
                let parts: Vec<&str> = v.split('.').collect();
                if parts.len() >= 2 {
                    format!("{}.{}", parts[0], parts[1])
                } else {
                    v.to_string()
                }
            })
    }

    /// 从 .csproj 文件名提取项目名
    fn extract_project_name(csproj_path: &str) -> String {
        let path = std::path::Path::new(csproj_path);
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("app")
            .to_string()
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

impl Provider for DotnetProvider {
    fn name(&self) -> &str {
        "dotnet"
    }

    fn detect(&self, app: &App, _env: &Environment) -> Result<bool> {
        if let Ok(files) = app.find_files("*.csproj") {
            return Ok(!files.is_empty());
        }
        Ok(false)
    }

    fn initialize(&mut self, ctx: &mut GenerateContext) -> Result<()> {
        // 查找第一个 .csproj 文件
        if let Ok(files) = ctx.app.find_files("*.csproj") {
            if let Some(csproj) = files.first() {
                self.csproj_path = Some(csproj.clone());
                self.project_name = Some(Self::extract_project_name(csproj));

                // 从 TargetFramework 解析版本
                if let Ok(content) = ctx.app.read_file(csproj) {
                    if let Some(version) = Self::parse_target_framework(&content) {
                        self.dotnet_version = version;
                    }
                }
            }
        }

        // global.json 版本覆盖
        if let Some(version) = Self::parse_global_json_version(&ctx.app) {
            self.dotnet_version = version;
        }

        // 环境变量版本覆盖（最高优先级）
        if let (Some(version), _) = ctx.env.get_config_variable("DOTNET_VERSION") {
            self.dotnet_version = version;
        }

        Ok(())
    }

    fn plan(&self, ctx: &mut GenerateContext) -> Result<()> {
        let project_name = self.project_name.as_deref().unwrap_or("app");

        // 元数据
        ctx.metadata.set("dotnetVersion", &self.dotnet_version);
        ctx.metadata.set("dotnetProjectName", project_name);

        // === mise 步骤：安装 .NET SDK ===
        Self::ensure_mise_step_builder(ctx);

        let dotnet_ref = {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            let r = mise.default_package(&mut ctx.resolver, "dotnet", DEFAULT_DOTNET_VERSION);
            mise.version(&mut ctx.resolver, &r, &self.dotnet_version, "resolved");
            r
        };
        let _ = dotnet_ref;

        let mise_step_name = ctx
            .mise_step_builder
            .as_ref()
            .map(|m| m.name().to_string())
            .unwrap_or_else(|| mise_step_builder::MISE_STEP_NAME.to_string());

        // === install 步骤：dotnet restore ===
        let install = ctx.new_command_step("install");
        install.add_input(Layer::new_step_layer(&mise_step_name, None));
        {
            let install = Self::get_command_step(&mut ctx.steps, "install");

            // 复制项目文件
            if let Some(ref csproj) = self.csproj_path {
                install.add_command(Command::new_copy(csproj, csproj));
            }
            // 复制可选的 global.json 和 nuget.config
            if ctx.app.has_file("global.json") {
                install.add_command(Command::new_copy("global.json", "global.json"));
            }
            if ctx.app.has_file("nuget.config") {
                install.add_command(Command::new_copy("nuget.config", "nuget.config"));
            }

            install.add_command(Command::new_exec("dotnet restore"));
        }

        // NuGet 缓存
        let cache_name = ctx
            .caches
            .add_cache("nuget-packages", "/root/.nuget/packages");
        {
            let install = Self::get_command_step(&mut ctx.steps, "install");
            install.add_cache(&cache_name);
        }

        // === build 步骤：dotnet publish ===
        let build = ctx.new_command_step("build");
        build.add_input(Layer::new_step_layer("install", None));
        {
            let local_layer = ctx.new_local_layer();
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_input(local_layer);

            build.add_command(Command::new_exec(
                "dotnet publish --no-restore -c Release -o out",
            ));
        }

        // === Deploy 配置 ===
        ctx.deploy.start_cmd = Some(format!(
            "ASPNETCORE_URLS=http://0.0.0.0:${{PORT:-3000}} ./out/{}",
            project_name
        ));

        // Deploy 环境变量
        ctx.deploy.variables.insert(
            "ASPNETCORE_ENVIRONMENT".to_string(),
            "Production".to_string(),
        );
        ctx.deploy
            .variables
            .insert("DOTNET_CLI_TELEMETRY_OPTOUT".to_string(), "1".to_string());
        ctx.deploy
            .variables
            .insert("DOTNET_NOLOGO".to_string(), "1".to_string());
        ctx.deploy
            .variables
            .insert("ASPNETCORE_CONTENTROOT".to_string(), "/app/out".to_string());

        // 运行时 APT 包
        ctx.deploy.add_apt_packages(&["libicu-dev".to_string()]);

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
            "To configure your start command, arcpack will:\n\n\
             1. Restore NuGet packages with `dotnet restore`\n\
             2. Publish with `dotnet publish -c Release -o out`\n\
             3. Use `./out/<ProjectName>` as the start command"
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

    fn basic_csproj() -> &'static str {
        r#"<Project Sdk="Microsoft.NET.Sdk.Web">
  <PropertyGroup>
    <TargetFramework>net8.0</TargetFramework>
  </PropertyGroup>
</Project>"#
    }

    // === detect 测试 ===

    #[test]
    fn test_detect_with_csproj() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("MyApp.csproj"), basic_csproj()).unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = DotnetProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_without_csproj() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = DotnetProvider::new();
        assert!(!provider.detect(&app, &env).unwrap());
    }

    // === 版本解析测试 ===

    #[test]
    fn test_parse_target_framework_net8() {
        let content = "<TargetFramework>net8.0</TargetFramework>";
        assert_eq!(
            DotnetProvider::parse_target_framework(content),
            Some("8.0".to_string())
        );
    }

    #[test]
    fn test_parse_target_framework_netcoreapp() {
        let content = "<TargetFramework>netcoreapp3.1</TargetFramework>";
        assert_eq!(
            DotnetProvider::parse_target_framework(content),
            Some("3.1".to_string())
        );
    }

    #[test]
    fn test_parse_target_framework_missing() {
        let content = "<Project></Project>";
        assert_eq!(DotnetProvider::parse_target_framework(content), None);
    }

    #[test]
    fn test_extract_project_name() {
        assert_eq!(
            DotnetProvider::extract_project_name("MyApp.csproj"),
            "MyApp"
        );
        assert_eq!(
            DotnetProvider::extract_project_name("src/WebApi.csproj"),
            "WebApi"
        );
    }

    #[test]
    fn test_version_from_csproj() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("MyApp.csproj"), basic_csproj()).unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = DotnetProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.dotnet_version, "8.0");
    }

    #[test]
    fn test_version_from_global_json() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("MyApp.csproj"),
            "<Project><PropertyGroup></PropertyGroup></Project>",
        )
        .unwrap();
        fs::write(
            dir.path().join("global.json"),
            r#"{"sdk": {"version": "9.0.100"}}"#,
        )
        .unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = DotnetProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.dotnet_version, "9.0");
    }

    #[test]
    fn test_version_from_env() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("MyApp.csproj"), basic_csproj()).unwrap();
        let mut ctx = make_ctx_with_env(
            &dir,
            HashMap::from([("ARCPACK_DOTNET_VERSION".to_string(), "7.0".to_string())]),
        );
        let mut provider = DotnetProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.dotnet_version, "7.0");
    }

    // === plan 测试 ===

    #[test]
    fn test_plan_basic() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("MyApp.csproj"), basic_csproj()).unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = DotnetProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        let step_names: Vec<&str> = ctx.steps.iter().map(|s| s.name()).collect();
        assert!(step_names.contains(&"install"));
        assert!(step_names.contains(&"build"));

        assert!(ctx
            .deploy
            .start_cmd
            .as_deref()
            .unwrap()
            .contains("out/MyApp"));
        assert!(ctx.deploy.apt_packages.contains(&"libicu-dev".to_string()));
        assert_eq!(
            ctx.deploy
                .variables
                .get("ASPNETCORE_ENVIRONMENT")
                .map(|s| s.as_str()),
            Some("Production")
        );
        assert!(ctx.caches.get_cache("nuget-packages").is_some());
    }
}
