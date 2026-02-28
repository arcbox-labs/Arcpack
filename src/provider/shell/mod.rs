/// Shell Provider：可执行脚本支持
///
/// 对齐 railpack `core/providers/shell/shell.go`
/// 自动检测 shebang 确定解释器，支持 ARCPACK_SHELL_SCRIPT 指定自定义脚本。

use crate::app::App;
use crate::app::environment::Environment;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::{self, MiseStepBuilder};
use crate::generate::GenerateContext;
use crate::plan::{Command, Filter, Layer};
use crate::provider::Provider;
use crate::Result;

/// 默认脚本文件名
const DEFAULT_SCRIPT: &str = "start.sh";

/// Shell Provider
pub struct ShellProvider {
    /// 脚本文件名
    script_name: String,
    /// 解释器（bash/sh/zsh）
    interpreter: String,
}

impl ShellProvider {
    pub fn new() -> Self {
        Self {
            script_name: DEFAULT_SCRIPT.to_string(),
            interpreter: "sh".to_string(),
        }
    }

    /// 从脚本内容解析 shebang，返回解释器名
    fn parse_shebang(content: &str) -> &'static str {
        let first_line = match content.lines().next() {
            Some(line) => line.trim(),
            None => return "sh",
        };

        if !first_line.starts_with("#!") {
            return "sh";
        }

        // 提取 shebang 后的路径/命令
        let shebang = first_line.trim_start_matches("#!");
        let shebang = shebang.trim();

        // #!/usr/bin/env <cmd> 格式
        if shebang.starts_with("/usr/bin/env ") {
            let cmd = shebang.strip_prefix("/usr/bin/env ").unwrap().trim();
            // 取第一个 token（可能有参数如 -S）
            let cmd = cmd.split_whitespace().next().unwrap_or("sh");
            return Self::map_interpreter(cmd);
        }

        // #!/bin/<cmd> 或 #!/usr/bin/<cmd> 格式
        let basename = shebang.rsplit('/').next().unwrap_or("sh");
        // 取第一个 token（basename 可能含参数）
        let basename = basename.split_whitespace().next().unwrap_or("sh");
        Self::map_interpreter(basename)
    }

    /// 将 shebang 中的命令映射到解释器
    fn map_interpreter(cmd: &str) -> &'static str {
        match cmd {
            "bash" => "bash",
            "zsh" => "zsh",
            "sh" => "sh",
            "dash" => "sh",
            // 不常见的 shell 回退到 bash 并警告
            "mksh" | "ksh" | "fish" => "bash",
            _ => "sh",
        }
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

impl Provider for ShellProvider {
    fn name(&self) -> &str {
        "shell"
    }

    fn detect(&self, app: &App, env: &Environment) -> Result<bool> {
        // ARCPACK_SHELL_SCRIPT 环境变量指定的文件
        if let (Some(script), _) = env.get_config_variable("SHELL_SCRIPT") {
            return Ok(app.has_file(&script));
        }
        // 默认 start.sh
        Ok(app.has_file(DEFAULT_SCRIPT))
    }

    fn initialize(&mut self, ctx: &mut GenerateContext) -> Result<()> {
        // 确定脚本名
        if let (Some(script), var_name) = ctx.env.get_config_variable("SHELL_SCRIPT") {
            if !ctx.app.has_file(&script) {
                return Err(anyhow::anyhow!(
                    "{} 指定的脚本文件 '{}' 不存在",
                    var_name,
                    script
                )
                .into());
            }
            self.script_name = script;
        }

        // 读取脚本内容，解析 shebang
        if let Ok(content) = ctx.app.read_file(&self.script_name) {
            self.interpreter = Self::parse_shebang(&content).to_string();
        }

        Ok(())
    }

    fn plan(&self, ctx: &mut GenerateContext) -> Result<()> {
        // 元数据
        ctx.metadata
            .set("detectedShellInterpreter", &self.interpreter);

        // mise 步骤：仅 apply_packages_from_config（无语言特定包）
        Self::ensure_mise_step_builder(ctx);

        let mise_step_name = ctx
            .mise_step_builder
            .as_ref()
            .map(|m| m.name().to_string())
            .unwrap_or_else(|| mise_step_builder::MISE_STEP_NAME.to_string());

        // build 步骤：chmod +x <script>
        let build = ctx.new_command_step("build");
        build.add_input(Layer::new_step_layer(&mise_step_name, None));
        {
            let local_layer = ctx.new_local_layer();
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_input(local_layer);
            build.add_command(Command::new_exec(format!(
                "chmod +x {}",
                self.script_name
            )));
        }

        // Deploy 配置
        ctx.deploy.start_cmd = Some(format!("{} {}", self.interpreter, self.script_name));

        // zsh 需要额外 APT 包
        if self.interpreter == "zsh" {
            ctx.deploy.add_apt_packages(&["zsh".to_string()]);
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
            "To configure your start command, arcpack will look for:\n\n\
             1. A \"start.sh\" file in your project root\n\
             2. The ARCPACK_SHELL_SCRIPT environment variable pointing to your script"
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

    // === detect 测试 ===

    #[test]
    fn test_detect_with_start_sh() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("start.sh"), "#!/bin/bash\necho hi").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = ShellProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_without_start_sh() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = ShellProvider::new();
        assert!(!provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_with_env_custom_script() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("run.sh"), "#!/bin/bash\necho hi").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::from([(
            "ARCPACK_SHELL_SCRIPT".to_string(),
            "run.sh".to_string(),
        )]));
        let provider = ShellProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_with_env_missing_script() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::from([(
            "ARCPACK_SHELL_SCRIPT".to_string(),
            "nonexistent.sh".to_string(),
        )]));
        let provider = ShellProvider::new();
        assert!(!provider.detect(&app, &env).unwrap());
    }

    // === shebang 解析测试 ===

    #[test]
    fn test_parse_shebang_bash() {
        assert_eq!(ShellProvider::parse_shebang("#!/bin/bash\necho hi"), "bash");
    }

    #[test]
    fn test_parse_shebang_env_bash() {
        assert_eq!(
            ShellProvider::parse_shebang("#!/usr/bin/env bash\necho hi"),
            "bash"
        );
    }

    #[test]
    fn test_parse_shebang_zsh() {
        assert_eq!(ShellProvider::parse_shebang("#!/bin/zsh\necho hi"), "zsh");
    }

    #[test]
    fn test_parse_shebang_env_zsh() {
        assert_eq!(
            ShellProvider::parse_shebang("#!/usr/bin/env zsh\necho hi"),
            "zsh"
        );
    }

    #[test]
    fn test_parse_shebang_sh() {
        assert_eq!(ShellProvider::parse_shebang("#!/bin/sh\necho hi"), "sh");
    }

    #[test]
    fn test_parse_shebang_dash() {
        assert_eq!(ShellProvider::parse_shebang("#!/bin/dash\necho hi"), "sh");
    }

    #[test]
    fn test_parse_shebang_fish_fallback() {
        assert_eq!(
            ShellProvider::parse_shebang("#!/bin/fish\necho hi"),
            "bash"
        );
    }

    #[test]
    fn test_parse_shebang_no_shebang() {
        assert_eq!(ShellProvider::parse_shebang("echo hi"), "sh");
    }

    #[test]
    fn test_parse_shebang_empty() {
        assert_eq!(ShellProvider::parse_shebang(""), "sh");
    }

    // === initialize 测试 ===

    #[test]
    fn test_initialize_reads_shebang() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("start.sh"), "#!/bin/bash\necho hello").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = ShellProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.interpreter, "bash");
        assert_eq!(provider.script_name, "start.sh");
    }

    #[test]
    fn test_initialize_custom_script() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("run.sh"), "#!/bin/zsh\necho hello").unwrap();
        let mut ctx = make_ctx_with_env(
            &dir,
            HashMap::from([("ARCPACK_SHELL_SCRIPT".to_string(), "run.sh".to_string())]),
        );
        let mut provider = ShellProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.interpreter, "zsh");
        assert_eq!(provider.script_name, "run.sh");
    }

    #[test]
    fn test_initialize_custom_script_not_exists_error() {
        let dir = TempDir::new().unwrap();
        let mut ctx = make_ctx_with_env(
            &dir,
            HashMap::from([(
                "ARCPACK_SHELL_SCRIPT".to_string(),
                "nonexistent.sh".to_string(),
            )]),
        );
        let mut provider = ShellProvider::new();
        assert!(provider.initialize(&mut ctx).is_err());
    }

    // === plan 测试 ===

    #[test]
    fn test_plan_basic() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("start.sh"), "#!/bin/bash\necho hello").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = ShellProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        // 验证步骤
        let step_names: Vec<&str> = ctx.steps.iter().map(|s| s.name()).collect();
        assert!(step_names.contains(&"build"));

        // 验证 start_cmd
        assert_eq!(
            ctx.deploy.start_cmd.as_deref(),
            Some("bash start.sh")
        );

        // 验证 metadata
        assert_eq!(
            ctx.metadata.get("detectedShellInterpreter"),
            Some("bash")
        );
    }

    #[test]
    fn test_plan_zsh_adds_apt_package() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("start.sh"), "#!/bin/zsh\necho hello").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = ShellProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        assert!(ctx.deploy.apt_packages.contains(&"zsh".to_string()));
        assert_eq!(
            ctx.deploy.start_cmd.as_deref(),
            Some("zsh start.sh")
        );
    }

    #[test]
    fn test_plan_no_shebang_uses_sh() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("start.sh"), "echo hello").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = ShellProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        assert_eq!(
            ctx.deploy.start_cmd.as_deref(),
            Some("sh start.sh")
        );
    }
}
