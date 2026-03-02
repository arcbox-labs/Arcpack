pub mod cpp;
pub mod deno;
pub mod dotnet;
pub mod elixir;
pub mod gleam;
pub mod golang;
pub mod java;
pub mod node;
pub mod php;
pub mod procfile;
pub mod python;
pub mod ruby;
pub mod rust_lang;
pub mod shell;
pub mod staticfile;

use crate::app::environment::Environment;
use crate::app::App;
use crate::generate::GenerateContext;
use crate::plan::BuildPlan;
use crate::Result;

/// Provider trait：语言/框架检测器
///
/// 对齐 railpack `core/providers/provider.go`
/// 生命周期：detect → initialize → plan → cleanse_plan
pub trait Provider: Send + Sync {
    /// 返回 Provider 名称
    fn name(&self) -> &str;

    /// 检测源码是否匹配该 Provider
    /// detect() 在 GenerateContext 创建前调用，因此取 App + Environment
    fn detect(&self, app: &App, env: &Environment) -> Result<bool>;

    /// 初始化阶段（可写入自身状态）
    fn initialize(&mut self, _ctx: &mut GenerateContext) -> Result<()> {
        Ok(())
    }

    /// 生成构建计划
    fn plan(&self, ctx: &mut GenerateContext) -> Result<()>;

    /// 后处理：清理构建计划
    fn cleanse_plan(&self, _plan: &mut BuildPlan) {}

    /// 启动命令帮助信息
    fn start_command_help(&self) -> Option<String> {
        None
    }
}

/// 获取所有语言 Provider（按检测优先级排序）
///
/// 优先级对齐 railpack：
/// PHP > Go > Java > Rust > Ruby > Elixir > Python > Deno > .NET > Node > Gleam > C++ > StaticFile > Shell
pub fn get_all_providers() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(php::PhpProvider::new()),
        Box::new(golang::GoProvider::new()),
        Box::new(java::JavaProvider::new()),
        Box::new(rust_lang::RustProvider::new()),
        Box::new(ruby::RubyProvider::new()),
        Box::new(elixir::ElixirProvider::new()),
        Box::new(python::PythonProvider::new()),
        Box::new(deno::DenoProvider::new()),
        Box::new(dotnet::DotnetProvider::new()),
        Box::new(node::NodeProvider::new()),
        Box::new(gleam::GleamProvider::new()),
        Box::new(cpp::CppProvider::new()),
        Box::new(staticfile::StaticFileProvider::new()),
        Box::new(shell::ShellProvider::new()),
    ]
}

/// 按名称获取 Provider
pub fn get_provider(name: &str) -> Option<Box<dyn Provider>> {
    get_all_providers().into_iter().find(|p| p.name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_all_providers_priority_order() {
        let providers = get_all_providers();
        let names: Vec<&str> = providers.iter().map(|p| p.name()).collect();
        assert_eq!(
            names,
            vec![
                "php",
                "golang",
                "java",
                "rust",
                "ruby",
                "elixir",
                "python",
                "deno",
                "dotnet",
                "node",
                "gleam",
                "cpp",
                "staticfile",
                "shell"
            ]
        );
    }

    #[test]
    fn test_get_provider_by_name() {
        for name in &[
            "php",
            "golang",
            "java",
            "rust",
            "ruby",
            "elixir",
            "python",
            "deno",
            "dotnet",
            "node",
            "gleam",
            "cpp",
            "staticfile",
            "shell",
        ] {
            assert!(
                get_provider(name).is_some(),
                "provider '{}' not found",
                name
            );
        }
    }

    #[test]
    fn test_get_provider_unknown_returns_none() {
        assert!(get_provider("xxx").is_none());
    }

    #[test]
    fn test_default_trait_methods_no_panic() {
        struct DummyProvider;
        impl Provider for DummyProvider {
            fn name(&self) -> &str {
                "dummy"
            }
            fn detect(&self, _app: &App, _env: &Environment) -> Result<bool> {
                Ok(false)
            }
            fn plan(&self, _ctx: &mut GenerateContext) -> Result<()> {
                Ok(())
            }
        }

        let p = DummyProvider;
        assert!(p.start_command_help().is_none());

        let mut plan = BuildPlan::new();
        p.cleanse_plan(&mut plan);
    }
}
