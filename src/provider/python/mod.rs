/// Python Provider：支持 pip/poetry/uv/pipenv/pdm 五种包管理器
///
/// 对齐 railpack `core/providers/python/python.go`
/// 自动检测 Django/FastAPI/Flask/FastHTML 框架，
/// 自动处理 APT 构建/运行时依赖和 secrets 前缀过滤。

pub mod django;
pub mod frameworks;
pub mod package_manager;

use std::collections::HashMap;

use crate::app::App;
use crate::app::environment::Environment;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::{self, MiseStepBuilder};
use crate::generate::GenerateContext;
use crate::plan::{Command, Filter, Layer};
use crate::provider::Provider;
use crate::Result;

use package_manager::PythonPackageManager;

/// 默认 Python 版本
const DEFAULT_PYTHON_VERSION: &str = "3.13";

/// Python 入口文件（用于 detect）
const PYTHON_ENTRY_FILES: &[&str] = &[
    "main.py", "app.py", "start.py", "bot.py", "hello.py", "server.py",
];

/// APT 构建依赖映射
const BUILD_APT_DEPS: &[(&str, &str)] = &[
    ("pycairo", "libcairo2-dev"),
    ("psycopg2", "libpq-dev"),
    ("psycopg", "libpq-dev"),
    ("mysqlclient", "default-libmysqlclient-dev"),
];

/// APT 运行时依赖映射
const RUNTIME_APT_DEPS: &[(&str, &str)] = &[
    ("pycairo", "libcairo2"),
    ("pdf2image", "poppler-utils"),
    ("pydub", "ffmpeg"),
    ("psycopg2", "libpq5"),
    ("psycopg", "libpq5"),
    ("mysqlclient", "default-mysql-client"),
];

/// Python Provider
pub struct PythonProvider {
    package_manager: PythonPackageManager,
    dependencies: Vec<String>,
    framework: Option<frameworks::PythonFramework>,
    python_version: String,
}

impl PythonProvider {
    pub fn new() -> Self {
        Self {
            package_manager: PythonPackageManager::Pip,
            dependencies: Vec::new(),
            framework: None,
            python_version: DEFAULT_PYTHON_VERSION.to_string(),
        }
    }

    /// 读取依赖列表（从各种来源）
    fn read_dependencies(app: &App, pm: &PythonPackageManager) -> Vec<String> {
        match pm {
            PythonPackageManager::Pip => {
                if let Ok(content) = app.read_file("requirements.txt") {
                    return content
                        .lines()
                        .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
                        .map(|l| {
                            // 提取包名（去除版本限制符）
                            let name = l.trim()
                                .split(&['=', '>', '<', '!', '~', '['][..])
                                .next()
                                .unwrap_or("")
                                .trim()
                                .to_lowercase();
                            name
                        })
                        .filter(|n| !n.is_empty() && !n.starts_with('-'))
                        .collect();
                }
                Vec::new()
            }
            PythonPackageManager::Pipenv => {
                if let Ok(content) = app.read_file("Pipfile") {
                    // 简单提取 [packages] 下的包名
                    let mut in_packages = false;
                    return content
                        .lines()
                        .filter_map(|line| {
                            let line = line.trim();
                            if line == "[packages]" {
                                in_packages = true;
                                return None;
                            }
                            if line.starts_with('[') {
                                in_packages = false;
                                return None;
                            }
                            if in_packages && !line.is_empty() && !line.starts_with('#') {
                                let name = line.split('=').next()?.trim().to_lowercase();
                                if !name.is_empty() {
                                    return Some(name);
                                }
                            }
                            None
                        })
                        .collect();
                }
                Vec::new()
            }
            _ => {
                // pyproject.toml 基础依赖提取
                if let Ok(content) = app.read_file("pyproject.toml") {
                    return Self::extract_pyproject_deps(&content);
                }
                Vec::new()
            }
        }
    }

    /// 从 pyproject.toml 提取依赖名
    fn extract_pyproject_deps(content: &str) -> Vec<String> {
        let mut deps = Vec::new();
        let mut in_deps = false;
        for line in content.lines() {
            let trimmed = line.trim();
            // [project.dependencies] 或 [tool.poetry.dependencies]
            if trimmed == "dependencies = [" || trimmed.contains("dependencies") && trimmed.contains('[') {
                in_deps = true;
                continue;
            }
            if in_deps {
                if trimmed == "]" || (trimmed.starts_with('[') && !trimmed.starts_with('"')) {
                    in_deps = false;
                    continue;
                }
                // 提取引号中的包名
                let trimmed = trimmed.trim_matches(|c: char| c == '"' || c == '\'' || c == ',');
                let name = trimmed
                    .split(&['=', '>', '<', '!', '~', ' ', '[', ';'][..])
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_lowercase();
                if !name.is_empty() && !name.starts_with('#') {
                    deps.push(name);
                }
            }
        }
        deps
    }

    /// 解析 Python 版本
    fn resolve_version(app: &App, env: &Environment) -> String {
        // ARCPACK_PYTHON_VERSION
        if let (Some(v), _) = env.get_config_variable("PYTHON_VERSION") {
            return v;
        }

        // runtime.txt
        if let Ok(content) = app.read_file("runtime.txt") {
            let v = content.trim();
            // 格式：python-3.11.x
            if let Some(stripped) = v.strip_prefix("python-") {
                return stripped.to_string();
            }
            if !v.is_empty() {
                return v.to_string();
            }
        }

        // Pipfile python_version
        if let Ok(content) = app.read_file("Pipfile") {
            for line in content.lines() {
                let line = line.trim();
                if line.starts_with("python_version") || line.starts_with("python_full_version") {
                    if let Some(v) = line.split('"').nth(1) {
                        return v.to_string();
                    }
                    if let Some(v) = line.split('\'').nth(1) {
                        return v.to_string();
                    }
                }
            }
        }

        DEFAULT_PYTHON_VERSION.to_string()
    }

    /// 检测 APT 构建依赖
    fn detect_build_apt_deps(dependencies: &[String]) -> Vec<String> {
        let mut apt_deps = Vec::new();
        for (pkg, apt_pkg) in BUILD_APT_DEPS {
            if dependencies.iter().any(|d| d == *pkg) {
                // psycopg2-binary 不需要 libpq-dev
                if *pkg == "psycopg2"
                    && dependencies.iter().any(|d| d == "psycopg2-binary")
                {
                    continue;
                }
                apt_deps.push(apt_pkg.to_string());
            }
        }
        apt_deps
    }

    /// 检测 APT 运行时依赖
    fn detect_runtime_apt_deps(dependencies: &[String]) -> Vec<String> {
        let mut apt_deps = Vec::new();
        for (pkg, apt_pkg) in RUNTIME_APT_DEPS {
            if dependencies.iter().any(|d| d == *pkg) {
                if *pkg == "psycopg2"
                    && dependencies.iter().any(|d| d == "psycopg2-binary")
                {
                    continue;
                }
                apt_deps.push(apt_pkg.to_string());
            }
        }
        apt_deps
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

impl Provider for PythonProvider {
    fn name(&self) -> &str {
        "python"
    }

    fn detect(&self, app: &App, _env: &Environment) -> Result<bool> {
        // 入口文件
        for file in PYTHON_ENTRY_FILES {
            if app.has_file(file) {
                return Ok(true);
            }
        }
        // 包管理文件
        Ok(app.has_file("requirements.txt")
            || app.has_file("pyproject.toml")
            || app.has_file("Pipfile"))
    }

    fn initialize(&mut self, ctx: &mut GenerateContext) -> Result<()> {
        self.package_manager = package_manager::detect_package_manager(&ctx.app);
        self.dependencies =
            Self::read_dependencies(&ctx.app, &self.package_manager);
        self.framework =
            frameworks::detect_framework(&ctx.app, &ctx.env, &self.dependencies);
        self.python_version = Self::resolve_version(&ctx.app, &ctx.env);

        Ok(())
    }

    fn plan(&self, ctx: &mut GenerateContext) -> Result<()> {
        // 元数据
        ctx.metadata
            .set("pythonPackageManager", &self.package_manager.to_string());
        if let Some(ref fw) = self.framework {
            ctx.metadata.set("pythonRuntime", &fw.name);
        } else {
            ctx.metadata.set("pythonRuntime", "python");
        }

        // === mise 步骤 ===
        Self::ensure_mise_step_builder(ctx);

        {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            let python_ref = mise.default_package(
                &mut ctx.resolver,
                "python",
                DEFAULT_PYTHON_VERSION,
            );
            mise.version(
                &mut ctx.resolver,
                &python_ref,
                &self.python_version,
                "resolved",
            );
            mise.variables.insert(
                "MISE_PYTHON_COMPILE".to_string(),
                "false".to_string(),
            );

            // 包管理器工具
            package_manager::setup_mise_packages(
                &self.package_manager,
                mise,
                &mut ctx.resolver,
            );

            // APT 构建依赖
            let build_apt = Self::detect_build_apt_deps(&self.dependencies);
            for pkg in &build_apt {
                mise.add_supporting_apt_package(pkg);
            }
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

            // Venv 配置
            install.add_variables(&HashMap::from([
                ("VIRTUAL_ENV".to_string(), "/app/.venv".to_string()),
            ]));
            install.add_paths(&["/app/.venv/bin".to_string()]);

            // Secrets 前缀过滤
            install.secrets = vec![];
            install.use_secrets_with_prefix(&ctx.env, "PYTHON");
            install.use_secrets_with_prefix(&ctx.env, "PIP");
            install.use_secrets_with_prefix(&ctx.env, "PIPX");
            install.use_secrets_with_prefix(&ctx.env, "UV");
            install.use_secrets_with_prefix(&ctx.env, "PDM");
            install.use_secrets_with_prefix(&ctx.env, "POETRY");

            // 复制安装文件
            let files = package_manager::get_install_files(&self.package_manager);
            for file in files {
                install.add_command(Command::new_copy(file, file));
            }

            // 安装命令
            package_manager::add_install_commands(
                &self.package_manager,
                install,
                &ctx.app,
                &mut ctx.caches,
            );
        }

        // === build 步骤 ===
        let build = ctx.new_command_step("build");
        build.add_input(Layer::new_step_layer("install", None));
        {
            let local_layer = ctx.new_local_layer();
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_input(local_layer);

            // uv 模式：在 build 步骤中运行额外同步
            if self.package_manager == PythonPackageManager::Uv {
                build.add_command(Command::new_exec(
                    "uv sync --locked --no-dev --no-editable",
                ));
            }
        }

        // === Deploy 配置 ===
        if let Some(ref fw) = self.framework {
            ctx.deploy.start_cmd = Some(fw.start_cmd.clone());
        }

        // Deploy 环境变量
        let deploy_vars = HashMap::from([
            ("PYTHONFAULTHANDLER".to_string(), "1".to_string()),
            ("PYTHONUNBUFFERED".to_string(), "1".to_string()),
            ("PYTHONHASHSEED".to_string(), "random".to_string()),
            ("PYTHONDONTWRITEBYTECODE".to_string(), "1".to_string()),
            (
                "PIP_DISABLE_PIP_VERSION_CHECK".to_string(),
                "1".to_string(),
            ),
            ("PIP_DEFAULT_TIMEOUT".to_string(), "100".to_string()),
        ]);
        for (k, v) in &deploy_vars {
            ctx.deploy.variables.insert(k.clone(), v.clone());
        }

        // APT 运行时依赖
        let runtime_apt = Self::detect_runtime_apt_deps(&self.dependencies);
        if !runtime_apt.is_empty() {
            ctx.deploy.add_apt_packages(&runtime_apt);
        }

        // deploy inputs: mise 层 + venv（build 步骤输出）+ 源码
        let mise_layer = ctx
            .mise_step_builder
            .as_ref()
            .map(|m| m.get_layer())
            .unwrap_or_default();

        let build_layer = Layer::new_step_layer(
            "build",
            Some(Filter {
                include: vec![".".to_string()],
                exclude: vec![".venv".to_string()],
            }),
        );

        let venv_layer = Layer::new_step_layer(
            "build",
            Some(Filter::include_only(vec![".venv".to_string()])),
        );

        ctx.deploy.add_inputs(&[mise_layer, venv_layer, build_layer]);

        Ok(())
    }

    fn start_command_help(&self) -> Option<String> {
        Some(
            "To configure your start command, arcpack will detect:\n\n\
             1. Django: python manage.py migrate && gunicorn ...\n\
             2. FastAPI: uvicorn main:app ...\n\
             3. Flask: gunicorn main:app ...\n\
             4. Fallback: python <entry_file> (main.py, app.py, etc.)"
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

    // === detect 测试 ===

    #[test]
    fn test_detect_with_requirements_txt() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("requirements.txt"), "flask==2.0").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = PythonProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_with_pyproject_toml() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "[project]").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = PythonProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_with_main_py() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.py"), "print('hi')").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = PythonProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_with_pipfile() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Pipfile"), "[packages]").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = PythonProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_empty() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = PythonProvider::new();
        assert!(!provider.detect(&app, &env).unwrap());
    }

    // === 版本解析测试 ===

    #[test]
    fn test_version_default() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        assert_eq!(PythonProvider::resolve_version(&app, &env), "3.13");
    }

    #[test]
    fn test_version_from_env() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::from([(
            "ARCPACK_PYTHON_VERSION".to_string(),
            "3.11".to_string(),
        )]));
        assert_eq!(PythonProvider::resolve_version(&app, &env), "3.11");
    }

    #[test]
    fn test_version_from_runtime_txt() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("runtime.txt"), "python-3.10.5").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        assert_eq!(PythonProvider::resolve_version(&app, &env), "3.10.5");
    }

    // === APT 依赖检测测试 ===

    #[test]
    fn test_build_apt_deps_psycopg2() {
        let deps = vec!["psycopg2".to_string(), "flask".to_string()];
        let apt = PythonProvider::detect_build_apt_deps(&deps);
        assert!(apt.contains(&"libpq-dev".to_string()));
    }

    #[test]
    fn test_build_apt_deps_psycopg2_binary_excluded() {
        let deps = vec![
            "psycopg2".to_string(),
            "psycopg2-binary".to_string(),
        ];
        let apt = PythonProvider::detect_build_apt_deps(&deps);
        assert!(!apt.contains(&"libpq-dev".to_string()));
    }

    #[test]
    fn test_runtime_apt_deps_pycairo() {
        let deps = vec!["pycairo".to_string()];
        let apt = PythonProvider::detect_runtime_apt_deps(&deps);
        assert!(apt.contains(&"libcairo2".to_string()));
    }

    // === Deploy 环境变量测试 ===

    #[test]
    fn test_deploy_env_vars() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("requirements.txt"), "flask==2.0").unwrap();
        fs::write(dir.path().join("main.py"), "").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = PythonProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        assert_eq!(
            ctx.deploy.variables.get("PYTHONFAULTHANDLER"),
            Some(&"1".to_string())
        );
        assert_eq!(
            ctx.deploy.variables.get("PYTHONUNBUFFERED"),
            Some(&"1".to_string())
        );
        assert_eq!(
            ctx.deploy.variables.get("PYTHONHASHSEED"),
            Some(&"random".to_string())
        );
    }

    // === plan 测试 ===

    #[test]
    fn test_plan_pip_basic() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("requirements.txt"),
            "flask==2.0\ngunicorn==20.0",
        )
        .unwrap();
        fs::write(dir.path().join("main.py"), "from flask import Flask").unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = PythonProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        let step_names: Vec<&str> = ctx.steps.iter().map(|s| s.name()).collect();
        assert!(step_names.contains(&"install"));
        assert!(step_names.contains(&"build"));

        assert_eq!(
            ctx.metadata.get("pythonPackageManager"),
            Some("pip")
        );
        assert_eq!(ctx.metadata.get("pythonRuntime"), Some("flask"));
    }

    #[test]
    fn test_plan_uv_basic() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "[project]\nname = \"app\"").unwrap();
        fs::write(dir.path().join("uv.lock"), "").unwrap();
        fs::write(dir.path().join("main.py"), "").unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = PythonProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        assert_eq!(
            ctx.metadata.get("pythonPackageManager"),
            Some("uv")
        );
    }

    #[test]
    fn test_plan_django() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("requirements.txt"),
            "django==4.2\ngunicorn==20.0",
        )
        .unwrap();
        fs::write(dir.path().join("manage.py"), "#!/usr/bin/env python").unwrap();
        fs::create_dir(dir.path().join("myproject")).unwrap();
        fs::write(
            dir.path().join("myproject/settings.py"),
            "WSGI_APPLICATION = 'myproject.wsgi.application'\n",
        )
        .unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = PythonProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        assert_eq!(ctx.metadata.get("pythonRuntime"), Some("django"));
        assert!(ctx
            .deploy
            .start_cmd
            .as_ref()
            .unwrap()
            .contains("gunicorn"));
        assert!(ctx
            .deploy
            .start_cmd
            .as_ref()
            .unwrap()
            .contains("myproject.wsgi"));
    }

    #[test]
    fn test_plan_fastapi() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("requirements.txt"),
            "fastapi==0.100\nuvicorn==0.23",
        )
        .unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = PythonProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        assert_eq!(ctx.metadata.get("pythonRuntime"), Some("fastapi"));
        assert!(ctx
            .deploy
            .start_cmd
            .as_ref()
            .unwrap()
            .contains("uvicorn"));
    }
}
