pub mod package_json;
pub mod package_manager;
pub mod detect;
pub mod frameworks;
pub mod spa;
pub mod prune;
pub mod workspace;

use std::collections::HashMap;

use crate::app::App;
use crate::app::environment::Environment;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::{self, MiseStepBuilder};
use crate::generate::GenerateContext;
use crate::plan::{Command, Filter, Layer};
use crate::provider::Provider;
use crate::Result;

use package_json::PackageJson;
use package_manager::PackageManagerKind;

/// 默认 Node.js 版本
const DEFAULT_NODE_VERSION: &str = "22";
/// 默认 Bun 版本
const DEFAULT_BUN_VERSION: &str = "latest";
/// Corepack 主目录
const COREPACK_HOME: &str = "/opt/corepack";
/// node_modules 缓存目录
const NODE_MODULES_CACHE: &str = "/app/node_modules/.cache";

/// Puppeteer/Chromium 所需的 APT 包列表
const PUPPETEER_APT_PACKAGES: &[&str] = &[
    "xvfb", "gconf-service", "libasound2", "libatk1.0-0", "libc6",
    "libcairo2", "libcups2", "libdbus-1-3", "libexpat1", "libfontconfig1",
    "libgbm1", "libgcc1", "libgconf-2-4", "libgdk-pixbuf2.0-0",
    "libglib2.0-0", "libgtk-3-0", "libnspr4", "libpango-1.0-0",
    "libpangocairo-1.0-0", "libstdc++6", "libx11-6", "libx11-xcb1",
    "libxcb1", "libxcomposite1", "libxcursor1", "libxdamage1", "libxext6",
    "libxfixes3", "libxi6", "libxrandr2", "libxrender1", "libxss1",
    "libxtst6", "ca-certificates", "fonts-liberation", "libappindicator1",
    "libnss3", "lsb-release", "xdg-utils", "wget",
];

/// Node.js Provider
///
/// 对齐 railpack `core/providers/node/node.go`
/// 集成框架检测、SPA 部署、Prune 步骤、Workspace 支持
pub struct NodeProvider {
    package_json: Option<PackageJson>,
    package_manager: PackageManagerKind,
    workspace_packages: Vec<workspace::WorkspacePackage>,
}

impl NodeProvider {
    pub fn new() -> Self {
        Self {
            package_json: None,
            package_manager: PackageManagerKind::Npm,
            workspace_packages: Vec::new(),
        }
    }

    /// 获取 package.json
    fn get_package_json(&self, app: &App) -> Result<PackageJson> {
        if !app.has_file("package.json") {
            return Ok(PackageJson::default());
        }
        app.read_json::<PackageJson>("package.json")
            .map_err(|e| anyhow::anyhow!("error reading package.json: {}", e).into())
    }

    /// 是否使用 corepack（packageManager 字段存在且非 bun）
    fn uses_corepack(&self) -> bool {
        self.package_json.as_ref().map_or(false, |pkg| {
            pkg.package_manager.is_some() && self.package_manager != PackageManagerKind::Bun
        })
    }

    /// Node 运行时是否必需
    fn requires_node(&self) -> bool {
        if self.package_manager != PackageManagerKind::Bun {
            return true;
        }
        self.package_json.as_ref().map_or(false, |pkg| {
            pkg.package_manager.is_some()
                || pkg.scripts.values().any(|s| s.contains("node"))
        })
    }

    /// Bun 运行时是否必需
    fn requires_bun(&self) -> bool {
        self.package_manager == PackageManagerKind::Bun
    }

    /// 获取启动命令
    fn get_start_command(&self, ctx: &GenerateContext) -> Option<String> {
        let pkg = self.package_json.as_ref()?;

        // 1. start 脚本
        if pkg.has_script("start") {
            return Some(self.package_manager.run_cmd("start"));
        }

        // 2. main 字段
        if let Some(ref main) = pkg.main {
            if !main.is_empty() {
                return Some(self.package_manager.run_script_command(main));
            }
        }

        // 3. index.js / index.ts
        if let Ok(files) = ctx.app.find_files("{index.js,index.ts}") {
            if let Some(file) = files.first() {
                return Some(self.package_manager.run_script_command(file));
            }
        }

        None
    }

    /// 获取 Node.js 环境变量
    fn get_node_env_vars(&self) -> HashMap<String, String> {
        let mut env_vars = HashMap::from([
            ("NODE_ENV".to_string(), "production".to_string()),
            ("NPM_CONFIG_PRODUCTION".to_string(), "false".to_string()),
            ("NPM_CONFIG_UPDATE_NOTIFIER".to_string(), "false".to_string()),
            ("NPM_CONFIG_FUND".to_string(), "false".to_string()),
            ("CI".to_string(), "true".to_string()),
        ]);

        if self.package_manager == PackageManagerKind::Yarn1 {
            env_vars.insert("YARN_PRODUCTION".to_string(), "false".to_string());
        }

        env_vars
    }

    /// 检查是否有 lifecycle 脚本（preinstall/postinstall/prepare）
    fn has_lifecycle_scripts(&self) -> bool {
        self.package_json.as_ref().map_or(false, |pkg| {
            pkg.has_script("preinstall")
                || pkg.has_script("postinstall")
                || pkg.has_script("prepare")
                || pkg.has_local_dependency()
        })
    }

    /// 检测 Puppeteer 依赖
    fn has_puppeteer(pkg: &PackageJson) -> bool {
        pkg.has_dependency("puppeteer")
            || pkg.has_dependency("puppeteer-core")
            || pkg.has_dependency("puppeteer-extra")
    }

    /// 获取 Puppeteer APT 包列表
    fn get_puppeteer_apt_packages() -> Vec<String> {
        PUPPETEER_APT_PACKAGES.iter().map(|s| s.to_string()).collect()
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

    /// 获取指定名称的 CommandStepBuilder 的可变引用
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

impl Provider for NodeProvider {
    fn name(&self) -> &str {
        "node"
    }

    fn detect(&self, app: &App, _env: &Environment) -> Result<bool> {
        Ok(app.has_file("package.json"))
    }

    fn initialize(&mut self, ctx: &mut GenerateContext) -> Result<()> {
        let package_json = self.get_package_json(&ctx.app)?;
        self.package_manager = detect::detect_package_manager(&ctx.app, &package_json);
        self.workspace_packages = workspace::resolve_workspace_packages(&ctx.app)?;
        self.package_json = Some(package_json);
        Ok(())
    }

    fn plan(&self, ctx: &mut GenerateContext) -> Result<()> {
        if self.package_json.is_none() {
            return Err(anyhow::anyhow!("package.json not found").into());
        }

        // 设置元数据
        ctx.metadata.set("nodePackageManager", &self.package_manager.to_string());
        ctx.metadata.set_bool("nodeUsesCorepack", self.uses_corepack());

        let requires_node = self.requires_node();
        let requires_bun = self.requires_bun();
        let uses_corepack = self.uses_corepack();

        // === 安装 mise 包 ===
        // 使用 ensure + 直接字段访问避免双重可变借用
        Self::ensure_mise_step_builder(ctx);

        if requires_node {
            // 注册 Node.js 包（通过直接字段访问实现不相交借用）
            let node_ref = {
                let mise = ctx.mise_step_builder.as_mut().unwrap();
                mise.default_package(&mut ctx.resolver, "node", DEFAULT_NODE_VERSION)
            };

            // 环境变量覆盖
            if let (Some(env_version), var_name) = ctx.env.get_config_variable("NODE_VERSION") {
                let mise = ctx.mise_step_builder.as_mut().unwrap();
                mise.version(&mut ctx.resolver, &node_ref, &env_version, &var_name);
            }

            // engines.node 覆盖
            if let Some(ref pkg) = self.package_json {
                if let Some(engine_node) = pkg.engines.get("node") {
                    if !engine_node.is_empty() {
                        let mise = ctx.mise_step_builder.as_mut().unwrap();
                        mise.version(
                            &mut ctx.resolver,
                            &node_ref,
                            engine_node,
                            "package.json > engines > node",
                        );
                    }
                }
            }

            ctx.deploy.add_apt_packages(&["libatomic1".to_string()]);
        }

        if requires_bun {
            let bun_ref = {
                let mise = ctx.mise_step_builder.as_mut().unwrap();
                mise.default_package(&mut ctx.resolver, "bun", DEFAULT_BUN_VERSION)
            };

            if let (Some(env_version), var_name) = ctx.env.get_config_variable("BUN_VERSION") {
                let mise = ctx.mise_step_builder.as_mut().unwrap();
                mise.version(&mut ctx.resolver, &bun_ref, &env_version, &var_name);
            }

            // bun 是主 PM 但不需要 node 时，仍安装 node（node-gyp）
            if !requires_node && !ctx.config.packages.contains_key("node") {
                let node_ref = {
                    let mise = ctx.mise_step_builder.as_mut().unwrap();
                    mise.default_package(&mut ctx.resolver, "node", DEFAULT_NODE_VERSION)
                };

                if let (Some(env_version), var_name) = ctx.env.get_config_variable("NODE_VERSION")
                {
                    let mise = ctx.mise_step_builder.as_mut().unwrap();
                    mise.version(&mut ctx.resolver, &node_ref, &env_version, &var_name);
                }

                ctx.deploy.add_apt_packages(&["libatomic1".to_string()]);
            }
        }

        // 安装包管理器特定版本
        if let Some(ref pkg) = self.package_json {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            self.package_manager
                .get_package_manager_packages(&ctx.app, pkg, mise, &mut ctx.resolver);
        }

        if uses_corepack {
            if let Some(ref mut mise) = ctx.mise_step_builder {
                mise.variables
                    .insert("MISE_NODE_COREPACK".to_string(), "true".to_string());
            }
        }

        // === install 步骤 ===
        let mise_step_name = ctx
            .mise_step_builder
            .as_ref()
            .map(|m| m.name().to_string())
            .unwrap_or_else(|| mise_step_builder::MISE_STEP_NAME.to_string());

        let install = ctx.new_command_step("install");
        install.add_input(Layer::new_step_layer(&mise_step_name, None));

        // 设置 install 步骤环境变量和 secrets
        {
            let env_vars = self.get_node_env_vars();
            let install = Self::get_command_step(&mut ctx.steps, "install");

            install.add_variables(&env_vars);
            install.secrets = vec![];
            install.use_secrets_with_prefix(&ctx.env, "NODE");
            install.use_secrets_with_prefix(&ctx.env, "NPM");
            install.use_secrets_with_prefix(&ctx.env, "BUN");
            install.use_secrets_with_prefix(&ctx.env, "PNPM");
            install.use_secrets_with_prefix(&ctx.env, "YARN");
            install.use_secrets_with_prefix(&ctx.env, "CI");
            install.add_paths(&["/app/node_modules/.bin".to_string()]);

            if uses_corepack {
                install.add_variables(&HashMap::from([(
                    "COREPACK_HOME".to_string(),
                    COREPACK_HOME.to_string(),
                )]));
                install.add_command(Command::new_copy("package.json", "package.json"));
                install.add_command(Command::new_exec_shell(
                    "npm i -g corepack@latest && corepack enable && corepack prepare --activate",
                ));
            }

            install.add_command(Command::new_exec(format!("mkdir -p {}", NODE_MODULES_CACHE)));
        }

        // 复制安装所需文件
        {
            let needs_full_source = self.has_lifecycle_scripts();

            if needs_full_source {
                let local_layer = ctx.new_local_layer();
                let install = Self::get_command_step(&mut ctx.steps, "install");
                install.add_input(local_layer);
            } else {
                let files = self.package_manager.supporting_install_files(&ctx.app);
                let install = Self::get_command_step(&mut ctx.steps, "install");
                for file in &files {
                    install.add_command(Command::new_copy(file.as_str(), file.as_str()));
                }
            }
        }

        // 安装依赖
        {
            let install = Self::get_command_step(&mut ctx.steps, "install");
            self.package_manager
                .install_deps(&ctx.app, &mut ctx.caches, install, uses_corepack);
        }

        // === build 步骤 ===
        let build = ctx.new_command_step("build");
        build.add_input(Layer::new_step_layer("install", None));

        {
            let local_layer = ctx.new_local_layer();
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_input(local_layer);

            if let Some(ref pkg) = self.package_json {
                if pkg.has_script("build") {
                    build.add_command(Command::new_exec(self.package_manager.run_cmd("build")));
                }
            }

            let cache_name = ctx.caches.add_cache("node-modules", NODE_MODULES_CACHE);
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_cache(&cache_name);
        }

        // === 框架检测 ===
        let pkg = self.package_json.as_ref().unwrap();

        // 框架检测优先级：根项目 > 第一个按路径排序匹配的 workspace 子包
        // 若 monorepo 有多个子包各自用不同框架，仅取第一个（first-match-wins）
        let primary_framework = frameworks::detect_frameworks(&ctx.app, &ctx.env, pkg, "")
            .first()
            .cloned()
            .or_else(|| {
                self.workspace_packages.iter().find_map(|ws_pkg| {
                    frameworks::detect_frameworks(
                        &ctx.app, &ctx.env, &ws_pkg.package_json, &ws_pkg.path,
                    )
                    .first()
                    .cloned()
                })
            });

        if let Some(ref fw) = primary_framework {
            ctx.metadata.set("nodeFramework", &fw.name);
            ctx.metadata.set("nodeDeployMode", match fw.mode {
                frameworks::DeployMode::Ssr => "ssr",
                frameworks::DeployMode::Spa => "spa",
            });

            // 框架特定缓存目录
            for cache_dir in &fw.cache_dirs {
                let cache_name = cache_dir.trim_start_matches("/app/")
                    .replace('/', "-").replace('.', "");
                ctx.caches.add_cache(&cache_name, cache_dir);
                let build = Self::get_command_step(&mut ctx.steps, "build");
                build.add_cache(&cache_name);
            }
        }

        // === Workspace 元数据 ===
        if !self.workspace_packages.is_empty() {
            ctx.metadata.set_bool("nodeWorkspace", true);
            let pkg_names: Vec<&str> = self.workspace_packages.iter()
                .map(|p| p.path.as_str()).collect();
            ctx.metadata.set("nodeWorkspacePackages", &pkg_names.join(","));
        }

        // === Prune 步骤（可选） ===
        let prune_step_name = prune::create_prune_step(ctx, &self.package_manager, "build");
        let has_prune = prune_step_name.is_some();
        if has_prune {
            ctx.metadata.set_bool("nodePruneDeps", true);
        }

        // node_modules 层的来源步骤
        let node_modules_source_step = if has_prune { "prune" } else { "build" };

        // === Deploy 配置 ===
        let is_spa = primary_framework.as_ref()
            .map(|fw| fw.mode == frameworks::DeployMode::Spa)
            .unwrap_or(false);

        if is_spa {
            // SPA 模式：使用 Caddy 静态服务
            let fw = primary_framework.as_ref().unwrap();
            if let Some(ref output_dir) = fw.output_dir {
                spa::deploy_as_spa(ctx, output_dir, "build")?;
            }
        } else {
            // SSR 或普通 Node.js 模式
            // SSR 框架的 start_cmd 优先于自动检测
            if let Some(ref fw) = primary_framework {
                if let Some(ref start_cmd) = fw.start_cmd {
                    ctx.deploy.start_cmd = Some(start_cmd.clone());
                }
            }

            // 如果框架未提供 start_cmd，使用自动检测
            if ctx.deploy.start_cmd.is_none() {
                if let Some(start_cmd) = self.get_start_command(ctx) {
                    ctx.deploy.start_cmd = Some(start_cmd);
                }
            }

            // Deploy inputs
            let mise_layer = ctx
                .mise_step_builder
                .as_ref()
                .map(|m| m.get_layer())
                .unwrap_or_default();

            let install_folders = self.package_manager.get_install_folder(&ctx.app);
            let node_modules_layer = Layer::new_step_layer(
                node_modules_source_step,
                Some(Filter::include_only(install_folders)),
            );

            let mut build_include_dirs = vec!["/root/.cache".to_string(), ".".to_string()];
            if uses_corepack {
                build_include_dirs.push(COREPACK_HOME.to_string());
            }

            let build_layer = Layer::new_step_layer(
                "build",
                Some(Filter {
                    include: build_include_dirs,
                    exclude: vec!["node_modules".to_string(), ".yarn".to_string()],
                }),
            );

            ctx.deploy
                .add_inputs(&[mise_layer, node_modules_layer, build_layer]);
        }

        // Deploy 环境变量（两种模式都需要）
        let deploy_env_vars = self.get_node_env_vars();
        for (k, v) in &deploy_env_vars {
            ctx.deploy.variables.insert(k.clone(), v.clone());
        }

        // Puppeteer 检测 → 添加 Chromium APT 依赖
        if let Some(ref pkg) = self.package_json {
            if Self::has_puppeteer(pkg) {
                ctx.deploy.add_apt_packages(&Self::get_puppeteer_apt_packages());
                ctx.logs.info("detected Puppeteer dependency, adding Chromium APT packages");
            }
        }

        // COREPACK_HOME 环境变量（deploy）
        if uses_corepack {
            ctx.deploy
                .variables
                .insert("COREPACK_HOME".to_string(), COREPACK_HOME.to_string());
        }

        Ok(())
    }

    fn cleanse_plan(&self, plan: &mut crate::plan::BuildPlan) {
        let has_prune = plan.steps.iter().any(|s| s.name.as_deref() == Some("prune"));
        prune::cleanse_plan_for_prune(plan, has_prune);
    }

    fn start_command_help(&self) -> Option<String> {
        Some(
            "To configure your start command, arcpack will check:\n\n\
             1. A \"start\" script in your package.json:\n\
             2. A \"main\" field in your package.json\n\
             3. An index.js or index.ts file in your project root"
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

    fn setup_node_project(dir: &TempDir, package_json: &str) {
        fs::write(dir.path().join("package.json"), package_json).unwrap();
    }

    fn make_ctx(dir: &TempDir) -> GenerateContext {
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let config = Config::empty();
        GenerateContext::new(app, env, config, Box::new(MockVersionResolver)).unwrap()
    }

    #[test]
    fn test_detect_with_package_json() {
        let dir = TempDir::new().unwrap();
        setup_node_project(&dir, r#"{"name":"test"}"#);
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = NodeProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_without_package_json() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = NodeProvider::new();
        assert!(!provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_initialize_reads_package_json() {
        let dir = TempDir::new().unwrap();
        setup_node_project(
            &dir,
            r#"{"name":"my-app","scripts":{"start":"node index.js"}}"#,
        );
        let mut ctx = make_ctx(&dir);
        let mut provider = NodeProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert!(provider.package_json.is_some());
        assert_eq!(provider.package_json.as_ref().unwrap().name, "my-app");
    }

    #[test]
    fn test_initialize_detects_npm_default() {
        let dir = TempDir::new().unwrap();
        setup_node_project(&dir, r#"{"name":"test"}"#);
        let mut ctx = make_ctx(&dir);
        let mut provider = NodeProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.package_manager, PackageManagerKind::Npm);
    }

    #[test]
    fn test_initialize_detects_pnpm() {
        let dir = TempDir::new().unwrap();
        setup_node_project(&dir, r#"{"name":"test"}"#);
        fs::write(dir.path().join("pnpm-lock.yaml"), "lockfileVersion: 9").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = NodeProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.package_manager, PackageManagerKind::Pnpm);
    }

    #[test]
    fn test_plan_npm_basic() {
        let dir = TempDir::new().unwrap();
        setup_node_project(
            &dir,
            r#"{
            "name": "test-app",
            "scripts": { "start": "node index.js", "build": "tsc" },
            "dependencies": { "express": "^4.18.0" }
        }"#,
        );
        fs::write(dir.path().join("package-lock.json"), "{}").unwrap();
        fs::write(dir.path().join("index.js"), "console.log('hello')").unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = NodeProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        // 验证步骤存在
        let step_names: Vec<&str> = ctx.steps.iter().map(|s| s.name()).collect();
        assert!(step_names.contains(&"install"));
        assert!(step_names.contains(&"build"));

        // 验证 deploy 有启动命令
        assert!(ctx.deploy.start_cmd.is_some());
        assert_eq!(ctx.deploy.start_cmd.as_deref(), Some("npm run start"));
    }

    #[test]
    fn test_plan_pnpm_basic() {
        let dir = TempDir::new().unwrap();
        setup_node_project(
            &dir,
            r#"{
            "name": "test-app",
            "scripts": { "start": "node index.js" }
        }"#,
        );
        fs::write(
            dir.path().join("pnpm-lock.yaml"),
            "lockfileVersion: '9.0'",
        )
        .unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = NodeProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        assert_eq!(provider.package_manager, PackageManagerKind::Pnpm);
    }

    #[test]
    fn test_plan_bun_basic() {
        let dir = TempDir::new().unwrap();
        setup_node_project(
            &dir,
            r#"{
            "name": "test-app",
            "scripts": { "start": "bun run index.ts" }
        }"#,
        );
        fs::write(dir.path().join("bun.lockb"), "").unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = NodeProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        assert_eq!(provider.package_manager, PackageManagerKind::Bun);
    }

    #[test]
    fn test_uses_corepack_with_package_manager() {
        let dir = TempDir::new().unwrap();
        setup_node_project(
            &dir,
            r#"{
            "name": "test",
            "packageManager": "pnpm@9.0.0"
        }"#,
        );
        let mut ctx = make_ctx(&dir);
        let mut provider = NodeProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert!(provider.uses_corepack());
    }

    #[test]
    fn test_no_corepack_for_bun() {
        let dir = TempDir::new().unwrap();
        setup_node_project(
            &dir,
            r#"{
            "name": "test",
            "packageManager": "bun@1.0.0"
        }"#,
        );
        let mut ctx = make_ctx(&dir);
        let mut provider = NodeProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert!(!provider.uses_corepack());
    }

    #[test]
    fn test_start_command_from_scripts() {
        let dir = TempDir::new().unwrap();
        setup_node_project(&dir, r#"{"scripts":{"start":"node server.js"}}"#);
        let mut ctx = make_ctx(&dir);
        let mut provider = NodeProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(
            provider.get_start_command(&ctx),
            Some("npm run start".to_string())
        );
    }

    #[test]
    fn test_start_command_from_main() {
        let dir = TempDir::new().unwrap();
        setup_node_project(&dir, r#"{"main":"src/app.js"}"#);
        let mut ctx = make_ctx(&dir);
        let mut provider = NodeProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(
            provider.get_start_command(&ctx),
            Some("node src/app.js".to_string())
        );
    }

    #[test]
    fn test_start_command_from_index_file() {
        let dir = TempDir::new().unwrap();
        setup_node_project(&dir, r#"{"name":"test"}"#);
        fs::write(dir.path().join("index.js"), "").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = NodeProvider::new();
        provider.initialize(&mut ctx).unwrap();
        let cmd = provider.get_start_command(&ctx);
        assert!(cmd.is_some());
        assert!(cmd.unwrap().contains("index.js"));
    }

    #[test]
    fn test_has_puppeteer() {
        let pkg: PackageJson =
            serde_json::from_str(r#"{"dependencies":{"puppeteer":"^21.0.0"}}"#).unwrap();
        assert!(NodeProvider::has_puppeteer(&pkg));
    }

    #[test]
    fn test_has_puppeteer_core() {
        let pkg: PackageJson =
            serde_json::from_str(r#"{"devDependencies":{"puppeteer-core":"^21.0.0"}}"#).unwrap();
        assert!(NodeProvider::has_puppeteer(&pkg));
    }

    #[test]
    fn test_no_puppeteer() {
        let pkg: PackageJson =
            serde_json::from_str(r#"{"dependencies":{"express":"^4.0.0"}}"#).unwrap();
        assert!(!NodeProvider::has_puppeteer(&pkg));
    }
}
