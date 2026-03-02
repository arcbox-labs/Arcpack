/// Rust Provider：Cargo.toml 检测 + cargo build --release
///
/// 对齐 railpack `core/providers/rust/rust.go`
/// 支持 workspace、WASM 目标、依赖预编译优化、多种版本来源。
use std::collections::HashSet;
use std::path::Path;

use regex::Regex;
use serde::Deserialize;

use crate::app::environment::Environment;
use crate::app::App;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::{self, MiseStepBuilder};
use crate::generate::GenerateContext;
use crate::plan::cache::CacheType;
use crate::plan::command::FileCommand;
use crate::plan::{Command, Filter, Layer};
use crate::provider::Provider;
use crate::Result;

/// 默认 Rust 版本
const DEFAULT_RUST_VERSION: &str = "1.89";

/// Cargo.toml 结构（轻量提取）
#[derive(Debug, Deserialize, Default)]
struct CargoToml {
    package: Option<CargoPackage>,
    lib: Option<CargoLib>,
    workspace: Option<CargoWorkspace>,
    bin: Option<Vec<CargoBin>>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
struct CargoPackage {
    name: Option<String>,
    edition: Option<String>,
    rust_version: Option<String>,
    default_run: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct CargoLib {
    name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
struct CargoWorkspace {
    members: Option<Vec<String>>,
    default_members: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Default)]
struct CargoBin {
    #[allow(dead_code)]
    name: Option<String>,
}

/// rust-toolchain.toml 结构
#[derive(Debug, Deserialize, Default)]
struct RustToolchain {
    toolchain: Option<RustToolchainSection>,
}

#[derive(Debug, Deserialize, Default)]
struct RustToolchainSection {
    channel: Option<String>,
    version: Option<String>,
}

/// Rust Provider
pub struct RustProvider {
    cargo_toml: Option<CargoToml>,
    is_wasm: bool,
}

impl RustProvider {
    pub fn new() -> Self {
        Self {
            cargo_toml: None,
            is_wasm: false,
        }
    }

    /// edition 到最低 Rust 版本的映射
    fn edition_to_version(edition: &str) -> Option<&'static str> {
        match edition {
            "2015" => Some("1.30.0"),
            "2018" => Some("1.55.0"),
            "2021" => Some("1.84.0"),
            "2024" => Some("1.85.1"),
            _ => None,
        }
    }

    fn extract_semver(value: &str) -> Option<String> {
        let re = Regex::new(r"(\d+\.\d+(?:\.\d+)?)").ok()?;
        let caps = re.captures(value)?;
        caps.get(1).map(|m| m.as_str().to_string())
    }

    /// 7 级版本解析
    fn resolve_version(&self, app: &App, env: &Environment) -> String {
        let mut resolved = DEFAULT_RUST_VERSION.to_string();

        // 1. Cargo.toml edition 映射
        if let Some(ref cargo) = self.cargo_toml {
            if let Some(ref pkg) = cargo.package {
                if let Some(ref edition) = pkg.edition {
                    if let Some(v) = Self::edition_to_version(edition) {
                        resolved = v.to_string();
                    }
                }
            }
        }

        // 2. ARCPACK_RUST_VERSION
        if let (Some(v), _) = env.get_config_variable("RUST_VERSION") {
            if !v.is_empty() {
                resolved = v;
            }
        }

        // 3. rust-version.txt 或 .rust-version
        for file in &["rust-version.txt", ".rust-version"] {
            if let Ok(content) = app.read_file(file) {
                if let Some(v) = Self::extract_semver(content.trim()) {
                    resolved = v;
                }
            }
        }

        // 4. Cargo.toml package.rust-version
        if let Some(ref cargo) = self.cargo_toml {
            if let Some(ref pkg) = cargo.package {
                if let Some(ref v) = pkg.rust_version {
                    if let Some(v) = Self::extract_semver(v) {
                        resolved = v;
                    }
                }
            }
        }

        // 5. rust-toolchain.toml
        if let Ok(content) = app.read_file("rust-toolchain.toml") {
            if let Ok(tc) = toml::from_str::<RustToolchain>(&content) {
                if let Some(toolchain) = tc.toolchain {
                    let source = toolchain.channel.or(toolchain.version).unwrap_or_default();
                    if let Some(v) = Self::extract_semver(&source) {
                        resolved = v;
                    }
                }
            }
        }

        // 6. rust-toolchain 文件
        if let Ok(content) = app.read_file("rust-toolchain") {
            if let Some(v) = Self::extract_semver(content.trim()) {
                resolved = v;
            }
        }

        resolved
    }

    /// 检测 WASM 目标
    fn detect_wasm(app: &App) -> bool {
        if let Ok(content) = app.read_file(".cargo/config.toml") {
            return content.contains("wasm32-wasi");
        }
        if let Ok(content) = app.read_file(".cargo/config") {
            return content.contains("wasm32-wasi");
        }
        false
    }

    fn get_target(&self) -> Option<&'static str> {
        if self.is_wasm {
            return Some("wasm32-wasi");
        }
        None
    }

    fn get_bin_suffix(&self) -> &'static str {
        if self.is_wasm {
            ".wasm"
        } else {
            ""
        }
    }

    fn get_app_name(&self) -> String {
        self.cargo_toml
            .as_ref()
            .and_then(|cargo| cargo.package.as_ref())
            .and_then(|pkg| pkg.name.clone())
            .unwrap_or_default()
    }

    fn resolve_workspace_binary(&self, app: &App, env: &Environment) -> Option<String> {
        if let (Some(name), _) = env.get_config_variable("CARGO_WORKSPACE") {
            if !name.is_empty() {
                return Some(name);
            }
        }

        let workspace = self
            .cargo_toml
            .as_ref()
            .and_then(|cargo| cargo.workspace.as_ref())?;

        let mut seen = HashSet::new();
        let mut members = Vec::new();
        if let Some(default_members) = &workspace.default_members {
            members.extend(default_members.clone());
        }
        if let Some(all_members) = &workspace.members {
            members.extend(all_members.clone());
        }

        let excludes: HashSet<String> = workspace
            .exclude
            .as_ref()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();

        for member in members {
            if excludes.contains(&member) || !seen.insert(member.clone()) {
                continue;
            }

            let candidates = if member.contains('*') || member.contains('?') {
                app.find_directories(&member).unwrap_or_default()
            } else {
                vec![member]
            };

            for candidate in candidates {
                if let Some(bin) = Self::find_binary_in_workspace_member(app, &candidate) {
                    return Some(bin);
                }
            }
        }

        None
    }

    fn find_binary_in_workspace_member(app: &App, member_dir: &str) -> Option<String> {
        let manifest_path = format!("{member_dir}/Cargo.toml");
        let manifest = app.read_toml::<CargoToml>(&manifest_path).ok()?;
        let package_name = manifest
            .package
            .as_ref()
            .and_then(|pkg| pkg.name.as_ref())
            .cloned()?;

        let has_main = app.has_file(&format!("{member_dir}/src/main.rs"));
        let has_bin_dir = app.has_match(&format!("{member_dir}/src/bin"));
        let has_manifest_bins = manifest.bin.as_ref().is_some_and(|bins| !bins.is_empty());
        let has_lib = app.has_file(&format!("{member_dir}/src/lib.rs"));

        if has_main || has_bin_dir || has_manifest_bins || !has_lib {
            return Some(package_name);
        }

        None
    }

    fn get_bins(&self, app: &App) -> Vec<String> {
        let mut bins = Vec::new();

        let app_name = self.get_app_name();
        if !app_name.is_empty() && app.has_file("src/main.rs") {
            bins.push(app_name);
        }

        if app.has_match("src/bin") {
            let find_bins = app.find_files("src/bin/*").unwrap_or_default();
            for bin_path in find_bins {
                let Some(filename) = Path::new(&bin_path).file_name().and_then(|s| s.to_str())
                else {
                    continue;
                };
                let parts: Vec<&str> = filename.split('.').collect();
                if parts.len() <= 1 {
                    continue;
                }

                let name = parts[..parts.len() - 1].join(".");
                if !name.is_empty() {
                    bins.push(name);
                }
            }
        }

        bins
    }

    fn get_start_bin(&self, app: &App, env: &Environment) -> Option<String> {
        let bins = self.get_bins(app);
        if bins.is_empty() {
            return None;
        }

        let mut selected = String::new();
        if bins.len() == 1 {
            selected = bins[0].clone();
        } else if let (Some(bin), _) = env.get_config_variable("RUST_BIN") {
            if bins.iter().any(|b| b == &bin) {
                selected = bin;
            }
        } else if let Some(default_run) = self
            .cargo_toml
            .as_ref()
            .and_then(|cargo| cargo.package.as_ref())
            .and_then(|pkg| pkg.default_run.as_ref())
        {
            selected = default_run.clone();
        }

        if selected.is_empty() {
            return None;
        }

        Some(format!("./bin/{}{}", selected, self.get_bin_suffix()))
    }

    fn get_start_command(&self, app: &App, env: &Environment) -> Option<String> {
        let target = self.get_target();
        let workspace = self.resolve_workspace_binary(app, env);

        if target.is_some() {
            if let Some(workspace) = workspace {
                return Some(format!("./bin/{workspace}"));
            }
            return self.get_start_bin(app, env);
        }

        if let Some(workspace) = workspace {
            return Some(format!("./bin/{workspace}"));
        }

        self.get_start_bin(app, env)
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

impl Provider for RustProvider {
    fn name(&self) -> &str {
        "rust"
    }

    fn detect(&self, app: &App, _env: &Environment) -> Result<bool> {
        Ok(app.has_file("Cargo.toml"))
    }

    fn initialize(&mut self, ctx: &mut GenerateContext) -> Result<()> {
        // 解析 Cargo.toml
        if let Ok(cargo) = ctx.app.read_toml::<CargoToml>("Cargo.toml") {
            self.cargo_toml = Some(cargo);
        }

        self.is_wasm = Self::detect_wasm(&ctx.app);
        Ok(())
    }

    fn plan(&self, ctx: &mut GenerateContext) -> Result<()> {
        // === mise 步骤：安装 Rust ===
        Self::ensure_mise_step_builder(ctx);

        let version = self.resolve_version(&ctx.app, &ctx.env);

        let rust_ref = {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            let r = mise.default_package(&mut ctx.resolver, "rust", DEFAULT_RUST_VERSION);
            mise.version(&mut ctx.resolver, &r, &version, "resolved");
            r
        };
        let _ = rust_ref;

        let mise_step_name = ctx
            .mise_step_builder
            .as_ref()
            .map(|m| m.name().to_string())
            .unwrap_or_else(|| mise_step_builder::MISE_STEP_NAME.to_string());
        let workspace_binary = self.resolve_workspace_binary(&ctx.app, &ctx.env);
        let app_name = self.get_app_name();
        let target_arg = self
            .get_target()
            .map(|target| format!(" --target {target}"))
            .unwrap_or_default();
        let target_path = self
            .get_target()
            .map(|target| format!("{target}/"))
            .unwrap_or_default();
        let bin_suffix = self.get_bin_suffix();

        let cargo_registry_cache = ctx
            .caches
            .add_cache("cargo_registry", "/root/.cargo/registry");
        let cargo_git_cache = ctx.caches.add_cache("cargo_git", "/root/.cargo/git");
        let cargo_target_cache = if !app_name.is_empty() {
            Some(
                ctx.caches
                    .add_cache_with_type("cargo_target", "target", CacheType::Shared),
            )
        } else {
            None
        };

        // === install 步骤（依赖预编译） ===
        let install = ctx.new_command_step("install");
        install.add_input(Layer::new_step_layer(&mise_step_name, None));

        {
            let install = Self::get_command_step(&mut ctx.steps, "install");
            install.add_cache(&cargo_registry_cache);
            install.add_cache(&cargo_git_cache);

            install.add_command(Command::new_copy("Cargo.toml*", "."));
            install.add_command(Command::new_copy("Cargo.lock*", "."));

            if let Some(target) = self.get_target() {
                install.add_command(Command::new_exec(format!("rustup target add {target}")));
            }

            // workspace 场景不做依赖预编译
            if workspace_binary.is_none() {
                install
                    .assets
                    .insert("main.rs".to_string(), "fn main() { }".to_string());
                let has_lib = self
                    .cargo_toml
                    .as_ref()
                    .and_then(|cargo| cargo.lib.as_ref())
                    .and_then(|lib| lib.name.as_ref())
                    .is_some();
                if has_lib {
                    install
                        .assets
                        .insert("lib.rs".to_string(), "fn main() { }".to_string());
                }

                install.add_command(Command::new_exec("mkdir -p src"));
                install.add_command(Command::File(FileCommand {
                    path: "src/main.rs".to_string(),
                    name: "main.rs".to_string(),
                    mode: None,
                    custom_name: Some("compile dependencies".to_string()),
                }));
                if has_lib {
                    install.add_command(Command::new_file("src/lib.rs", "lib.rs"));
                }

                install.add_command(Command::new_exec(format!(
                    "cargo build --release{target_arg}"
                )));
                install.add_command(Command::new_exec(format!(
                    "rm -rf src target/{}release/{}*",
                    target_path, app_name
                )));
            }
        }

        // === build 步骤 ===
        let build = ctx.new_command_step("build");
        build.add_input(Layer::new_step_layer(&mise_step_name, None));
        build.add_input(Layer::new_step_layer(
            "install",
            Some(Filter {
                include: vec![],
                exclude: vec!["/app/".to_string()],
            }),
        ));
        {
            let local_layer = ctx.new_local_layer();
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_input(local_layer);
            if let Some(cache) = &cargo_target_cache {
                build.add_cache(cache);
            }
            build.add_command(Command::new_exec("mkdir -p bin"));

            if let Some(target) = self.get_target() {
                build.add_command(Command::new_exec(format!("rustup target add {target}")));
            }

            if let Some(workspace) = workspace_binary {
                build.add_command(Command::new_exec(format!(
                    "cargo build --release --package {}{}",
                    workspace, target_arg
                )));
                build.add_command(Command::new_exec(format!(
                    "cp target/{}release/{}{} bin",
                    target_path, workspace, bin_suffix
                )));
            } else {
                let bins = self.get_bins(&ctx.app);
                if !bins.is_empty() {
                    build.add_command(Command::new_exec(format!(
                        "cargo build --release{}",
                        target_arg
                    )));
                    for bin in bins {
                        build.add_command(Command::new_exec(format!(
                            "cp target/{}release/{}{} bin",
                            target_path, bin, bin_suffix
                        )));
                    }
                }
            }
        }

        // === Deploy 配置 ===
        ctx.deploy.start_cmd = self.get_start_command(&ctx.app, &ctx.env);

        // Deploy 环境变量
        ctx.deploy
            .variables
            .insert("ROCKET_ADDRESS".to_string(), "0.0.0.0".to_string());

        // deploy inputs: build 步骤输出（排除 target/）
        let build_layer = Layer::new_step_layer(
            "build",
            Some(Filter {
                include: vec![".".to_string()],
                exclude: vec!["target".to_string()],
            }),
        );
        ctx.deploy.add_inputs(&[build_layer]);

        Ok(())
    }

    fn start_command_help(&self) -> Option<String> {
        Some(
            "To configure your start command, arcpack will:\n\n\
             1. Build your Rust binary with `cargo build --release`\n\
             2. Copy the binary to ./bin/<name>\n\
             3. Use ./bin/<name> as the start command\n\n\
             The binary name is detected from Cargo.toml (package.name or [[bin]])"
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

    fn basic_cargo_toml() -> &'static str {
        r#"[package]
name = "my-app"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "my-app"
path = "src/main.rs"
"#
    }

    // === detect 测试 ===

    #[test]
    fn test_detect_with_cargo_toml() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), basic_cargo_toml()).unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = RustProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_without_cargo_toml() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = RustProvider::new();
        assert!(!provider.detect(&app, &env).unwrap());
    }

    // === 版本解析测试 ===

    #[test]
    fn test_edition_to_version() {
        assert_eq!(RustProvider::edition_to_version("2015"), Some("1.30.0"));
        assert_eq!(RustProvider::edition_to_version("2018"), Some("1.55.0"));
        assert_eq!(RustProvider::edition_to_version("2021"), Some("1.84.0"));
        assert_eq!(RustProvider::edition_to_version("2024"), Some("1.85.1"));
        assert_eq!(RustProvider::edition_to_version("unknown"), None);
    }

    #[test]
    fn test_version_from_edition() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), basic_cargo_toml()).unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = RustProvider::new();
        provider.initialize(&mut ctx).unwrap();
        let version = provider.resolve_version(&ctx.app, &ctx.env);
        assert_eq!(version, "1.84.0"); // edition 2021
    }

    #[test]
    fn test_version_from_env() {
        let dir = TempDir::new().unwrap();
        // 无 edition 的 Cargo.toml
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let mut ctx = make_ctx_with_env(
            &dir,
            HashMap::from([("ARCPACK_RUST_VERSION".to_string(), "1.75".to_string())]),
        );
        let mut provider = RustProvider::new();
        provider.initialize(&mut ctx).unwrap();
        let version = provider.resolve_version(&ctx.app, &ctx.env);
        assert_eq!(version, "1.75");
    }

    #[test]
    fn test_version_from_rust_version_field() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nrust-version = \"1.70\"\n",
        )
        .unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = RustProvider::new();
        provider.initialize(&mut ctx).unwrap();
        let version = provider.resolve_version(&ctx.app, &ctx.env);
        // 无 edition → 跳到 rust-version
        assert_eq!(version, "1.70");
    }

    #[test]
    fn test_version_from_toolchain_toml() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"1.72.0\"\n",
        )
        .unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = RustProvider::new();
        provider.initialize(&mut ctx).unwrap();
        let version = provider.resolve_version(&ctx.app, &ctx.env);
        assert_eq!(version, "1.72.0");
    }

    #[test]
    fn test_version_default() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = RustProvider::new();
        provider.initialize(&mut ctx).unwrap();
        let version = provider.resolve_version(&ctx.app, &ctx.env);
        assert_eq!(version, DEFAULT_RUST_VERSION);
    }

    // === workspace 测试 ===

    #[test]
    fn test_workspace_detection() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crate-a\"]\n",
        )
        .unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = RustProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert!(provider
            .cargo_toml
            .as_ref()
            .and_then(|c| c.workspace.as_ref())
            .is_some());
    }

    // === binary name 测试 ===

    #[test]
    fn test_binary_name_from_bin() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), basic_cargo_toml()).unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = RustProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.get_app_name(), "my-app".to_string());
    }

    #[test]
    fn test_binary_name_from_package() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"server\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = RustProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.get_app_name(), "server".to_string());
    }

    // === plan 测试 ===

    #[test]
    fn test_plan_basic() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), basic_cargo_toml()).unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = RustProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        // 验证步骤
        let step_names: Vec<&str> = ctx.steps.iter().map(|s| s.name()).collect();
        assert!(step_names.contains(&"install"));
        assert!(step_names.contains(&"build"));

        // 验证 start_cmd
        assert_eq!(ctx.deploy.start_cmd.as_deref(), Some("./bin/my-app"));

        // 验证缓存
        assert!(ctx.caches.get_cache("cargo_registry").is_some());
        assert!(ctx.caches.get_cache("cargo_git").is_some());
        assert!(ctx.caches.get_cache("cargo_target").is_some());

        // 验证 ROCKET_ADDRESS
        assert_eq!(
            ctx.deploy
                .variables
                .get("ROCKET_ADDRESS")
                .map(|s| s.as_str()),
            Some("0.0.0.0")
        );
    }

    #[test]
    fn test_plan_cargo_target_shared_cache() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), basic_cargo_toml()).unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = RustProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        let cache = ctx.caches.get_cache("cargo_target").unwrap();
        assert_eq!(cache.cache_type, CacheType::Shared);
    }

    #[test]
    fn test_workspace_plan_skips_cargo_target_cache() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"[workspace]
members = ["a"]"#,
        )
        .unwrap();
        fs::create_dir(dir.path().join("a")).unwrap();
        fs::write(
            dir.path().join("a/Cargo.toml"),
            r#"[package]
name = "a"
version = "0.1.0"
edition = "2021""#,
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("a/src")).unwrap();
        fs::write(dir.path().join("a/src/main.rs"), "fn main() {}").unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = RustProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        assert!(ctx.caches.get_cache("cargo_target").is_none());
    }
}
