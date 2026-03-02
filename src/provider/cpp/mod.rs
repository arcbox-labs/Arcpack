use crate::app::environment::Environment;
/// C++ Provider：CMake/Meson 构建系统检测
///
/// 对齐 railpack `core/providers/cpp/cpp.go`, `cmake.go`, `meson.go`
/// 支持 CMake + Ninja 和 Meson + Ninja 双路径。
use crate::app::App;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::{self, MiseStepBuilder};
use crate::generate::GenerateContext;
use crate::plan::{Command, Filter, Layer};
use crate::provider::Provider;
use crate::Result;

/// 默认版本
const DEFAULT_CMAKE_VERSION: &str = "latest";
const DEFAULT_MESON_VERSION: &str = "latest";
const DEFAULT_NINJA_VERSION: &str = "latest";

/// 构建系统类型
#[derive(Debug, Clone, PartialEq)]
enum BuildSystem {
    CMake,
    Meson,
}

/// C++ Provider
pub struct CppProvider {
    build_system: BuildSystem,
}

impl CppProvider {
    pub fn new() -> Self {
        Self {
            build_system: BuildSystem::CMake,
        }
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

impl Provider for CppProvider {
    fn name(&self) -> &str {
        "cpp"
    }

    fn detect(&self, app: &App, _env: &Environment) -> Result<bool> {
        Ok(app.has_file("CMakeLists.txt") || app.has_file("meson.build"))
    }

    fn initialize(&mut self, ctx: &mut GenerateContext) -> Result<()> {
        // CMake 优先
        if ctx.app.has_file("CMakeLists.txt") {
            self.build_system = BuildSystem::CMake;
        } else if ctx.app.has_file("meson.build") {
            self.build_system = BuildSystem::Meson;
        }
        Ok(())
    }

    fn plan(&self, ctx: &mut GenerateContext) -> Result<()> {
        // 元数据
        ctx.metadata.set(
            "cppBuildSystem",
            match self.build_system {
                BuildSystem::CMake => "cmake",
                BuildSystem::Meson => "meson",
            },
        );

        // === mise 步骤：安装构建工具 ===
        Self::ensure_mise_step_builder(ctx);

        {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            match self.build_system {
                BuildSystem::CMake => {
                    mise.default_package(&mut ctx.resolver, "cmake", DEFAULT_CMAKE_VERSION);
                    mise.default_package(&mut ctx.resolver, "ninja", DEFAULT_NINJA_VERSION);
                }
                BuildSystem::Meson => {
                    mise.default_package(&mut ctx.resolver, "meson", DEFAULT_MESON_VERSION);
                    mise.default_package(&mut ctx.resolver, "ninja", DEFAULT_NINJA_VERSION);
                }
            }
        }

        let mise_step_name = ctx
            .mise_step_builder
            .as_ref()
            .map(|m| m.name().to_string())
            .unwrap_or_else(|| mise_step_builder::MISE_STEP_NAME.to_string());

        // === build 步骤 ===
        let build = ctx.new_command_step("build");
        build.add_input(Layer::new_step_layer(&mise_step_name, None));
        {
            let local_layer = ctx.new_local_layer();
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_input(local_layer);
            build.add_command(Command::new_exec("mkdir /build"));

            match self.build_system {
                BuildSystem::CMake => {
                    build.add_command(Command::new_exec("cmake -B /build -GNinja /app"));
                    build.add_command(Command::new_exec("cmake --build /build"));
                }
                BuildSystem::Meson => {
                    build.add_command(Command::new_exec("meson setup /build"));
                    build.add_command(Command::new_exec("meson compile -C /build"));
                }
            }
        }

        // === Deploy 配置 ===
        // 使用 app 目录名作为默认二进制名
        let app_name = ctx
            .app
            .source()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("app")
            .to_string();

        ctx.deploy.start_cmd = Some(format!("/build/{}", app_name));

        // deploy inputs: build 步骤输出（仅 /build 目录）
        let build_layer = Layer::new_step_layer(
            "build",
            Some(Filter::include_only(vec!["/build/".to_string()])),
        );

        ctx.deploy.add_inputs(&[build_layer]);

        Ok(())
    }

    fn start_command_help(&self) -> Option<String> {
        Some(
            "To configure your start command, arcpack will:\n\n\
             1. Build with CMake or Meson (auto-detected)\n\
             2. Output binary to /build/<project_name>\n\
             3. Use /build/<project_name> as the start command"
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

    // === detect 测试 ===

    #[test]
    fn test_detect_cmake() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.20)",
        )
        .unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = CppProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_meson() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("meson.build"), "project('myapp', 'cpp')").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = CppProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_empty() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = CppProvider::new();
        assert!(!provider.detect(&app, &env).unwrap());
    }

    // === 构建系统检测测试 ===

    #[test]
    fn test_cmake_priority_over_meson() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("CMakeLists.txt"), "").unwrap();
        fs::write(dir.path().join("meson.build"), "").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = CppProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.build_system, BuildSystem::CMake);
    }

    #[test]
    fn test_meson_only() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("meson.build"), "project('app', 'cpp')").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = CppProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.build_system, BuildSystem::Meson);
    }

    // === plan 测试 ===

    #[test]
    fn test_plan_cmake() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.20)\nproject(myapp)\nadd_executable(myapp main.cpp)",
        )
        .unwrap();
        fs::write(dir.path().join("main.cpp"), "int main() { return 0; }").unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = CppProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        let step_names: Vec<&str> = ctx.steps.iter().map(|s| s.name()).collect();
        assert!(step_names.contains(&"build"));

        assert_eq!(ctx.metadata.get("cppBuildSystem"), Some("cmake"));

        // start_cmd 包含 /build/
        assert!(ctx
            .deploy
            .start_cmd
            .as_deref()
            .unwrap()
            .starts_with("/build/"));
    }

    #[test]
    fn test_plan_meson() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("meson.build"), "project('myapp', 'cpp')").unwrap();
        fs::write(dir.path().join("main.cpp"), "int main() { return 0; }").unwrap();

        let mut ctx = make_ctx(&dir);
        let mut provider = CppProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        assert_eq!(ctx.metadata.get("cppBuildSystem"), Some("meson"));
    }
}
