/// Go Provider：支持 go.mod / go.work 的 Go 项目构建
///
/// 对齐 railpack `core/providers/golang/golang.go`
/// 支持 workspace、CGO 检测、cmd/ 子目录解析。

use std::collections::HashMap;

use crate::app::App;
use crate::app::environment::Environment;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::{self, MiseStepBuilder};
use crate::generate::GenerateContext;
use crate::plan::{Command, Filter, Layer};
use crate::provider::Provider;
use crate::Result;

/// 默认 Go 版本
const DEFAULT_GO_VERSION: &str = "1.25";

/// Go Provider
pub struct GoProvider {
    /// go.mod 中的 Go 版本
    go_mod_version: Option<String>,
    /// 是否为 workspace 模式
    is_workspace: bool,
    /// 是否有 CGO
    cgo_enabled: bool,
    /// 根目录是否有 .go 文件
    has_root_go_files: bool,
    /// cmd/ 子目录名列表
    cmd_dirs: Vec<String>,
    /// go.mod 是否存在
    has_go_mod: bool,
}

impl GoProvider {
    pub fn new() -> Self {
        Self {
            go_mod_version: None,
            is_workspace: false,
            cgo_enabled: false,
            has_root_go_files: false,
            cmd_dirs: Vec::new(),
            has_go_mod: false,
        }
    }

    /// 从 go.mod 内容中解析 Go 版本（`go X.XX` 指令）
    fn parse_go_mod_version(content: &str) -> Option<String> {
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("go ") {
                let version = line.strip_prefix("go ")?.trim();
                // 跳过行内注释
                let version = version.split("//").next()?.trim();
                if !version.is_empty() {
                    return Some(version.to_string());
                }
            }
        }
        None
    }

    /// 检测 CGO：检查环境变量或代码中的 import "C"
    fn detect_cgo(app: &App, env: &Environment) -> bool {
        // CGO_ENABLED 环境变量显式设置
        if let Some(val) = env.get_variable("CGO_ENABLED") {
            return val == "1";
        }
        // 扫描 .go 文件中的 import "C"
        if let Ok(re) = regex::Regex::new(r#"import\s+"C""#) {
            let files = app.find_files_with_content("*.go", &re);
            return !files.is_empty();
        }
        false
    }

    /// 获取构建命令（对齐 railpack 决策树）
    fn get_build_command(&self, env: &Environment) -> String {
        let ldflags = "-ldflags=\"-w -s\"";
        let output = "-o out";

        // 1. ARCPACK_GO_WORKSPACE_MODULE 环境变量
        if let (Some(module), _) = env.get_config_variable("GO_WORKSPACE_MODULE") {
            return format!("go build {} {} ./{}", ldflags, output, module);
        }

        // 2. ARCPACK_GO_BIN 环境变量
        if let (Some(bin_name), _) = env.get_config_variable("GO_BIN") {
            return format!("go build {} {} ./cmd/{}", ldflags, output, bin_name);
        }

        // 3. 有 go.mod + 根目录有 .go 文件
        if self.has_go_mod && self.has_root_go_files {
            return format!("go build {} {} .", ldflags, output);
        }

        // 4. 有 cmd/* 子目录
        if !self.cmd_dirs.is_empty() {
            let first_cmd = &self.cmd_dirs[0];
            return format!("go build {} {} ./cmd/{}", ldflags, output, first_cmd);
        }

        // 5. 仅有 go.mod
        if self.has_go_mod {
            return format!("go build {} {} .", ldflags, output);
        }

        // 6. workspace 模式（简化处理：go build）
        if self.is_workspace {
            return format!("go build {} {} .", ldflags, output);
        }

        // 7. 根目录有 main.go
        if self.has_root_go_files {
            return format!("go build {} {} main.go", ldflags, output);
        }

        // 默认
        format!("go build {} {} .", ldflags, output)
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

impl Provider for GoProvider {
    fn name(&self) -> &str {
        "golang"
    }

    fn detect(&self, app: &App, _env: &Environment) -> Result<bool> {
        Ok(app.has_file("go.mod") || app.has_file("go.work") || app.has_file("main.go"))
    }

    fn initialize(&mut self, ctx: &mut GenerateContext) -> Result<()> {
        self.has_go_mod = ctx.app.has_file("go.mod");
        self.is_workspace = ctx.app.has_file("go.work");

        // 解析 go.mod 版本
        if self.has_go_mod {
            if let Ok(content) = ctx.app.read_file("go.mod") {
                self.go_mod_version = Self::parse_go_mod_version(&content);
            }
        }

        // 检测根目录 .go 文件
        if let Ok(files) = ctx.app.find_files("*.go") {
            self.has_root_go_files = !files.is_empty();
        }

        // 检测 cmd/ 子目录
        if let Ok(dirs) = ctx.app.find_directories("cmd/*") {
            self.cmd_dirs = dirs;
        }

        // 检测 CGO
        self.cgo_enabled = Self::detect_cgo(&ctx.app, &ctx.env);

        Ok(())
    }

    fn plan(&self, ctx: &mut GenerateContext) -> Result<()> {
        // 元数据
        ctx.metadata.set_bool("goMod", self.has_go_mod);
        ctx.metadata.set_bool("goWorkspace", self.is_workspace);
        ctx.metadata.set_bool("goRootFile", self.has_root_go_files);
        ctx.metadata.set_bool("goCGO", self.cgo_enabled);

        // Gin 框架检测
        if self.has_go_mod {
            if let Ok(content) = ctx.app.read_file("go.mod") {
                if content.contains("github.com/gin-gonic/gin") {
                    ctx.metadata.set_bool("goGin", true);
                }
            }
        }

        // === mise 步骤：安装 Go ===
        Self::ensure_mise_step_builder(ctx);

        let go_ref = {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            mise.default_package(&mut ctx.resolver, "go", DEFAULT_GO_VERSION)
        };

        // go.mod 版本覆盖
        if let Some(ref version) = self.go_mod_version {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            mise.version(&mut ctx.resolver, &go_ref, version, "go.mod");
        }

        // 环境变量覆盖
        if let (Some(env_version), var_name) = ctx.env.get_config_variable("GO_VERSION") {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            mise.version(&mut ctx.resolver, &go_ref, &env_version, &var_name);
        }

        // CGO 构建依赖
        if self.cgo_enabled {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            mise.add_supporting_apt_package("gcc");
            mise.add_supporting_apt_package("g++");
            mise.add_supporting_apt_package("libc6-dev");
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

            // Go 环境变量
            install.add_variables(&HashMap::from([
                ("GOPATH".to_string(), "/go".to_string()),
                ("GOBIN".to_string(), "/go/bin".to_string()),
            ]));
            install.add_paths(&["/go/bin".to_string()]);

            // 非 CGO 设置
            if !self.cgo_enabled {
                install.add_variables(&HashMap::from([(
                    "CGO_ENABLED".to_string(),
                    "0".to_string(),
                )]));
            }

            // 复制 go.mod/go.sum
            if self.has_go_mod {
                install.add_command(Command::new_copy("go.mod", "go.mod"));
                if ctx.app.has_file("go.sum") {
                    install.add_command(Command::new_copy("go.sum", "go.sum"));
                }
            }

            // workspace 模式：额外复制文件
            if self.is_workspace {
                install.add_command(Command::new_copy("go.work", "go.work"));
                if ctx.app.has_file("go.work.sum") {
                    install.add_command(Command::new_copy("go.work.sum", "go.work.sum"));
                }
            }

            install.add_command(Command::new_exec("go mod download"));
        }

        // === build 步骤 ===
        let build = ctx.new_command_step("build");
        build.add_input(Layer::new_step_layer("install", None));
        {
            let local_layer = ctx.new_local_layer();
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_input(local_layer);

            let build_cmd = self.get_build_command(&ctx.env);
            build.add_command(Command::new_exec(build_cmd));

            // 缓存
            let cache_name = ctx.caches.add_cache("go-build", "/root/.cache/go-build");
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_cache(&cache_name);
        }

        // === Deploy 配置 ===
        ctx.deploy.start_cmd = Some("./out".to_string());

        // 运行时 APT 包
        ctx.deploy.add_apt_packages(&["tzdata".to_string()]);
        if self.cgo_enabled {
            ctx.deploy.add_apt_packages(&["libc6".to_string()]);
        }

        // deploy inputs: build 步骤输出
        let build_layer = Layer::new_step_layer(
            "build",
            Some(Filter::include_only(vec![".".to_string()])),
        );
        ctx.deploy.add_inputs(&[build_layer]);

        Ok(())
    }

    fn start_command_help(&self) -> Option<String> {
        Some(
            "To configure your start command, arcpack will:\n\n\
             1. Build your Go binary to ./out\n\
             2. Use ./out as the start command\n\n\
             You can customize with ARCPACK_GO_BIN or ARCPACK_GO_WORKSPACE_MODULE"
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
    fn test_detect_with_go_mod() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("go.mod"), "module example.com/app\ngo 1.22").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = GoProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_with_go_work() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("go.work"), "go 1.22\nuse .").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = GoProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_with_main_go() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.go"), "package main\nfunc main() {}").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = GoProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_empty() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = GoProvider::new();
        assert!(!provider.detect(&app, &env).unwrap());
    }

    // === go.mod 版本解析测试 ===

    #[test]
    fn test_parse_go_mod_version() {
        let content = "module example.com/app\n\ngo 1.22\n\nrequire ()";
        assert_eq!(
            GoProvider::parse_go_mod_version(content),
            Some("1.22".to_string())
        );
    }

    #[test]
    fn test_parse_go_mod_version_with_comment() {
        let content = "module example.com/app\ngo 1.21 // comment";
        assert_eq!(
            GoProvider::parse_go_mod_version(content),
            Some("1.21".to_string())
        );
    }

    #[test]
    fn test_parse_go_mod_version_missing() {
        let content = "module example.com/app\nrequire ()";
        assert_eq!(GoProvider::parse_go_mod_version(content), None);
    }

    // === 构建命令测试 ===

    #[test]
    fn test_build_command_root_go_files() {
        let mut p = GoProvider::new();
        p.has_go_mod = true;
        p.has_root_go_files = true;
        let env = Environment::new(HashMap::new());
        let cmd = p.get_build_command(&env);
        assert!(cmd.contains("go build"));
        assert!(cmd.contains("-o out"));
        assert!(cmd.ends_with(" ."));
    }

    #[test]
    fn test_build_command_cmd_dir() {
        let mut p = GoProvider::new();
        p.has_go_mod = true;
        p.cmd_dirs = vec!["server".to_string()];
        let env = Environment::new(HashMap::new());
        let cmd = p.get_build_command(&env);
        assert!(cmd.contains("./cmd/server"));
    }

    #[test]
    fn test_build_command_env_go_bin() {
        let p = GoProvider::new();
        let env = Environment::new(HashMap::from([(
            "ARCPACK_GO_BIN".to_string(),
            "api".to_string(),
        )]));
        let cmd = p.get_build_command(&env);
        assert!(cmd.contains("./cmd/api"));
    }

    // === plan 测试 ===

    #[test]
    fn test_plan_basic() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("go.mod"),
            "module example.com/app\n\ngo 1.22",
        )
        .unwrap();
        fs::write(
            dir.path().join("main.go"),
            "package main\nfunc main() {}",
        )
        .unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = GoProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        // 验证步骤
        let step_names: Vec<&str> = ctx.steps.iter().map(|s| s.name()).collect();
        assert!(step_names.contains(&"install"));
        assert!(step_names.contains(&"build"));

        // 验证 start_cmd
        assert_eq!(ctx.deploy.start_cmd.as_deref(), Some("./out"));

        // 验证 metadata
        assert_eq!(ctx.metadata.get("goMod"), Some("true"));
        assert_eq!(ctx.metadata.get("goRootFile"), Some("true"));

        // 验证缓存
        assert!(ctx.caches.get_cache("go-build").is_some());

        // 验证运行时 APT 包
        assert!(ctx.deploy.apt_packages.contains(&"tzdata".to_string()));
    }

    #[test]
    fn test_plan_cgo_adds_apt_packages() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("go.mod"),
            "module example.com/app\ngo 1.22",
        )
        .unwrap();
        fs::write(
            dir.path().join("main.go"),
            "package main\nimport \"C\"\nfunc main() {}",
        )
        .unwrap();

        let mut ctx = make_ctx_with_env(
            &dir,
            HashMap::from([("CGO_ENABLED".to_string(), "1".to_string())]),
        );
        let mut provider = GoProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert!(provider.cgo_enabled);

        provider.plan(&mut ctx).unwrap();
        // CGO 运行时包
        assert!(ctx.deploy.apt_packages.contains(&"libc6".to_string()));

        // CGO 构建包（在 mise step builder）
        let mise = ctx.mise_step_builder.as_ref().unwrap();
        assert!(mise.supporting_apt_packages.contains(&"gcc".to_string()));
    }

    #[test]
    fn test_plan_workspace_metadata() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("go.work"), "go 1.22\nuse .").unwrap();
        fs::write(
            dir.path().join("go.mod"),
            "module example.com/app\ngo 1.22",
        )
        .unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = GoProvider::new();
        provider.initialize(&mut ctx).unwrap();

        assert!(provider.is_workspace);
        assert_eq!(ctx.metadata.get("goWorkspace"), None); // 未 plan 前无 metadata

        provider.plan(&mut ctx).unwrap();
        assert_eq!(ctx.metadata.get("goWorkspace"), Some("true"));
    }

    #[test]
    fn test_plan_env_version_override() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("go.mod"),
            "module example.com/app\ngo 1.22",
        )
        .unwrap();
        let mut ctx = make_ctx_with_env(
            &dir,
            HashMap::from([("ARCPACK_GO_VERSION".to_string(), "1.21".to_string())]),
        );
        let mut provider = GoProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        // 环境变量覆盖应生效（后写入的版本覆盖前者）
        let requested = ctx.resolver.get("go").unwrap();
        assert_eq!(requested.version, "1.21");
    }
}
