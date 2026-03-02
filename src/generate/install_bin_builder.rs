use std::any::Any;

use crate::plan::{BuildPlan, Command, Filter, Layer, Step, ARCPACK_BUILDER_IMAGE};
use crate::resolver::{PackageRef, Resolver};
use crate::Result;

use super::{BuildStepOptions, StepBuilder};

/// 容器内二进制安装目录
pub const BIN_DIR: &str = "/railpack";

/// InstallBinBuilder：独立二进制安装步骤构建器
///
/// 对齐 railpack `core/generate/install_bin_builder.go`
/// 用于通过 mise 安装独立二进制（如 caddy 用于 SPA）
pub struct InstallBinBuilder {
    pub display_name: String,
    pub package: Option<PackageRef>,
}

impl InstallBinBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            display_name: name.to_string(),
            package: None,
        }
    }

    /// 注册默认包版本
    pub fn default_package(
        &mut self,
        resolver: &mut Resolver,
        name: &str,
        default_version: &str,
    ) -> PackageRef {
        let pkg_ref = resolver.default_package(name, default_version);
        self.package = Some(pkg_ref.clone());
        pkg_ref
    }

    /// 获取输出路径
    pub fn get_output_paths(&self) -> Vec<String> {
        if let Some(ref pkg) = self.package {
            vec![format!("{}/{}", BIN_DIR, pkg.name)]
        } else {
            vec![]
        }
    }

    /// 获取输出层
    pub fn get_layer(&self) -> Layer {
        let paths = self.get_output_paths();
        if paths.is_empty() {
            return Layer::default();
        }
        Layer::new_step_layer(&self.display_name, Some(Filter::include_only(paths)))
    }

    fn get_bin_path(&self) -> String {
        if let Some(ref pkg) = self.package {
            format!("{}/{}", BIN_DIR, pkg.name)
        } else {
            BIN_DIR.to_string()
        }
    }
}

impl StepBuilder for InstallBinBuilder {
    fn name(&self) -> &str {
        &self.display_name
    }

    fn build(&self, plan: &mut BuildPlan, options: &mut BuildStepOptions) -> Result<()> {
        let pkg = self
            .package
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("InstallBinBuilder: no package set"))?;

        let resolved = options.resolved_packages.get(&pkg.name).ok_or_else(|| {
            anyhow::anyhow!("package {} not found in resolved packages", pkg.name)
        })?;

        let version = resolved
            .resolved_version
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("package {} has no resolved version", pkg.name))?;

        let mut step = Step::new(&self.display_name);
        step.secrets = vec![];
        step.inputs = vec![Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None)];

        let bin_path = self.get_bin_path();
        step.commands = vec![
            Command::new_exec(format!(
                "mise install-into {}@{} {}",
                pkg.name, version, bin_path
            )),
            Command::new_path(&bin_path),
            Command::new_path(format!("{}/bin", bin_path)),
        ];

        plan.add_step(step);
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::super::cache_context::CacheContext;
    use super::*;
    use crate::resolver::{ResolvedPackage, VersionResolver};
    use std::collections::HashMap;

    struct MockResolver;
    impl VersionResolver for MockResolver {
        fn get_latest_version(&self, _pkg: &str, version: &str) -> Result<String> {
            Ok(format!("{}.0.0", version))
        }
        fn get_all_versions(&self, _pkg: &str, _version: &str) -> Result<Vec<String>> {
            Ok(vec!["1.0.0".to_string()])
        }
    }

    #[test]
    fn test_install_bin_builder_default_package() {
        let mut resolver = Resolver::new(Box::new(MockResolver));
        let mut builder = InstallBinBuilder::new("packages:caddy");
        let pkg_ref = builder.default_package(&mut resolver, "caddy", "2");
        assert_eq!(pkg_ref.name, "caddy");
        assert_eq!(builder.get_output_paths(), vec!["/railpack/caddy"]);
    }

    #[test]
    fn test_install_bin_builder_get_layer() {
        let mut resolver = Resolver::new(Box::new(MockResolver));
        let mut builder = InstallBinBuilder::new("packages:caddy");
        builder.default_package(&mut resolver, "caddy", "2");
        let layer = builder.get_layer();
        assert_eq!(layer.step.as_deref(), Some("packages:caddy"));
        assert!(layer
            .filter
            .include
            .contains(&"/railpack/caddy".to_string()));
    }

    #[test]
    fn test_install_bin_builder_build() {
        let mut resolver = Resolver::new(Box::new(MockResolver));
        let mut builder = InstallBinBuilder::new("packages:caddy");
        builder.default_package(&mut resolver, "caddy", "2");

        let mut plan = BuildPlan::new();
        let mut options = BuildStepOptions {
            resolved_packages: {
                let mut m = HashMap::new();
                m.insert(
                    "caddy".to_string(),
                    ResolvedPackage {
                        name: "caddy".to_string(),
                        requested_version: Some("2".to_string()),
                        resolved_version: Some("2.0.0".to_string()),
                        source: "default".to_string(),
                    },
                );
                m
            },
            caches: CacheContext::new(),
        };

        builder.build(&mut plan, &mut options).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].name, Some("packages:caddy".to_string()));
        // 验证命令包含 mise install-into
        let cmd = &plan.steps[0].commands[0];
        if let Command::Exec(exec) = cmd {
            assert!(exec.cmd.contains("mise install-into"));
            assert!(exec.cmd.contains("caddy@2.0.0"));
        } else {
            panic!("expected Exec command");
        }
    }
}
