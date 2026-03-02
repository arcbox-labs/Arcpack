/// Rust Provider：Cargo.toml 检测 + cargo build --release
///
/// 对齐 railpack `core/providers/rust/rust.go`
/// 支持 workspace、WASM 目标、依赖预编译优化、多种版本来源。
use serde::Deserialize;

use crate::app::environment::Environment;
use crate::app::App;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::{self, MiseStepBuilder};
use crate::generate::GenerateContext;
use crate::plan::cache::CacheType;
use crate::plan::{Command, Filter, Layer};
use crate::provider::Provider;
use crate::Result;

/// 默认 Rust 版本
const DEFAULT_RUST_VERSION: &str = "1.89";

/// Cargo.toml 结构（轻量提取）
#[derive(Debug, Deserialize, Default)]
struct CargoToml {
    package: Option<CargoPackage>,
    workspace: Option<CargoWorkspace>,
    bin: Option<Vec<CargoBin>>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
struct CargoPackage {
    name: Option<String>,
    edition: Option<String>,
    rust_version: Option<String>,
    #[allow(dead_code)]
    default_run: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct CargoWorkspace {
    #[allow(dead_code)]
    members: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Default)]
struct CargoBin {
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
}

/// Rust Provider
pub struct RustProvider {
    cargo_toml: Option<CargoToml>,
    is_workspace: bool,
    is_wasm: bool,
    binary_name: Option<String>,
}

impl RustProvider {
    pub fn new() -> Self {
        Self {
            cargo_toml: None,
            is_workspace: false,
            is_wasm: false,
            binary_name: None,
        }
    }

    /// edition 到最低 Rust 版本的映射
    fn edition_to_version(edition: &str) -> Option<&'static str> {
        match edition {
            "2015" => Some("1.30"),
            "2018" => Some("1.55"),
            "2021" => Some("1.84"),
            "2024" => Some("1.85.1"),
            _ => None,
        }
    }

    /// 7 级版本解析
    fn resolve_version(&self, app: &App, env: &Environment) -> String {
        // 1. Cargo.toml edition 映射
        if let Some(ref cargo) = self.cargo_toml {
            if let Some(ref pkg) = cargo.package {
                if let Some(ref edition) = pkg.edition {
                    if let Some(v) = Self::edition_to_version(edition) {
                        return v.to_string();
                    }
                }
            }
        }

        // 2. ARCPACK_RUST_VERSION
        if let (Some(v), _) = env.get_config_variable("RUST_VERSION") {
            return v;
        }

        // 3. rust-version.txt 或 .rust-version
        for file in &["rust-version.txt", ".rust-version"] {
            if let Ok(content) = app.read_file(file) {
                let v = content.trim().to_string();
                if !v.is_empty() {
                    return v;
                }
            }
        }

        // 4. Cargo.toml package.rust-version
        if let Some(ref cargo) = self.cargo_toml {
            if let Some(ref pkg) = cargo.package {
                if let Some(ref v) = pkg.rust_version {
                    return v.clone();
                }
            }
        }

        // 5. rust-toolchain.toml
        if let Ok(content) = app.read_file("rust-toolchain.toml") {
            if let Ok(tc) = toml::from_str::<RustToolchain>(&content) {
                if let Some(toolchain) = tc.toolchain {
                    if let Some(channel) = toolchain.channel {
                        return channel;
                    }
                }
            }
        }

        // 6. rust-toolchain 文件
        if let Ok(content) = app.read_file("rust-toolchain") {
            let v = content.trim().to_string();
            if !v.is_empty() {
                return v;
            }
        }

        // 7. 默认
        DEFAULT_RUST_VERSION.to_string()
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

    /// 获取 start command
    fn get_start_command(&self, env: &Environment) -> Option<String> {
        // 1. ARCPACK_CARGO_WORKSPACE
        if let (Some(name), _) = env.get_config_variable("CARGO_WORKSPACE") {
            return Some(format!("./bin/{}", name));
        }

        // 2. workspace 二进制
        // 简化：workspace 场景使用 binary_name
        if self.is_workspace {
            if let Some(ref name) = self.binary_name {
                return Some(format!("./bin/{}", name));
            }
        }

        // 3. 单二进制
        if let Some(ref name) = self.binary_name {
            if self.is_wasm {
                return Some(format!("./bin/{}.wasm", name));
            }
            return Some(format!("./bin/{}", name));
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
            self.is_workspace = cargo.workspace.is_some();
            self.binary_name = Self::get_binary_name_from_cargo(&cargo);
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

        // === install 步骤（依赖预编译） ===
        let install = ctx.new_command_step("install");
        install.add_input(Layer::new_step_layer(&mise_step_name, None));

        {
            let install = Self::get_command_step(&mut ctx.steps, "install");

            // 复制 Cargo.toml 和 Cargo.lock
            install.add_command(Command::new_copy("Cargo.toml", "Cargo.toml"));
            if ctx.app.has_file("Cargo.lock") {
                install.add_command(Command::new_copy("Cargo.lock", "Cargo.lock"));
            }

            // 依赖预编译（非 workspace）
            if !self.is_workspace {
                // 注入空 src/main.rs
                install.add_command(Command::new_exec("mkdir -p src"));
                install.add_command(Command::new_exec("echo 'fn main() {}' > src/main.rs"));

                // 检查是否有 [lib] 节
                let has_lib = self.cargo_toml.as_ref().map_or(false, |_| {
                    // 简化处理：默认不添加空 lib.rs
                    false
                });
                if has_lib {
                    install.add_command(Command::new_exec("touch src/lib.rs"));
                }

                install.add_command(Command::new_exec("cargo build --release"));
                // 清理注入的文件，强制重编译真实源码
                install.add_command(Command::new_exec("rm -rf src"));
            }
        }

        // === build 步骤 ===
        let build = ctx.new_command_step("build");
        build.add_input(Layer::new_step_layer(
            "install",
            Some(Filter {
                include: vec![".".to_string()],
                exclude: vec![
                    "/app/src".to_string(),
                    "/app/benches".to_string(),
                    "/app/examples".to_string(),
                ],
            }),
        ));
        {
            let local_layer = ctx.new_local_layer();
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_input(local_layer);

            // 构建命令
            let mut build_cmd = "cargo build --release".to_string();
            if self.is_wasm {
                build_cmd.push_str(" --target wasm32-wasi");
            }
            build.add_command(Command::new_exec(&build_cmd));

            // 复制二进制到 bin/
            build.add_command(Command::new_exec("mkdir -p bin"));
            if let Some(ref name) = self.binary_name {
                if self.is_wasm {
                    build.add_command(Command::new_exec(format!(
                        "cp target/wasm32-wasi/release/{}.wasm bin/",
                        name
                    )));
                } else {
                    build.add_command(Command::new_exec(format!(
                        "cp target/release/{} bin/",
                        name
                    )));
                }
            }

            // src/bin/*.rs 额外二进制检测
            if let Ok(bin_files) = ctx.app.find_files("src/bin/*.rs") {
                let (target_dir, ext) = if self.is_wasm {
                    ("wasm32-wasi/release", ".wasm")
                } else {
                    ("release", "")
                };
                for bin_file in &bin_files {
                    let bin_name = std::path::Path::new(bin_file)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    // 跳过与主二进制同名的
                    if !bin_name.is_empty() && self.binary_name.as_deref() != Some(bin_name) {
                        build.add_command(Command::new_exec(format!(
                            "cp target/{target_dir}/{bin_name}{ext} bin/"
                        )));
                    }
                }
            }
        }

        // 缓存
        ctx.caches
            .add_cache("cargo_registry", "/root/.cargo/registry");
        ctx.caches.add_cache("cargo_git", "/root/.cargo/git");
        ctx.caches
            .add_cache_with_type("cargo_target", "target", CacheType::Locked);

        {
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_cache("cargo_registry");
            build.add_cache("cargo_git");
            build.add_cache("cargo_target");
        }

        // === Deploy 配置 ===
        ctx.deploy.start_cmd = self.get_start_command(&ctx.env);

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

impl RustProvider {
    fn get_binary_name_from_cargo(cargo: &CargoToml) -> Option<String> {
        // [[bin]] 中的第一个
        if let Some(ref bins) = cargo.bin {
            if let Some(bin) = bins.first() {
                if let Some(ref name) = bin.name {
                    return Some(name.clone());
                }
            }
        }
        // package.name
        if let Some(ref pkg) = cargo.package {
            if let Some(ref name) = pkg.name {
                return Some(name.clone());
            }
        }
        None
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
        assert_eq!(RustProvider::edition_to_version("2015"), Some("1.30"));
        assert_eq!(RustProvider::edition_to_version("2018"), Some("1.55"));
        assert_eq!(RustProvider::edition_to_version("2021"), Some("1.84"));
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
        assert_eq!(version, "1.84"); // edition 2021
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
        assert!(provider.is_workspace);
    }

    // === binary name 测试 ===

    #[test]
    fn test_binary_name_from_bin() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), basic_cargo_toml()).unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = RustProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.binary_name, Some("my-app".to_string()));
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
        assert_eq!(provider.binary_name, Some("server".to_string()));
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
    fn test_plan_cargo_target_locked_cache() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), basic_cargo_toml()).unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = RustProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        let cache = ctx.caches.get_cache("cargo_target").unwrap();
        assert_eq!(cache.cache_type, CacheType::Locked);
    }
}
