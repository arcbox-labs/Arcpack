use std::any::Any;

use crate::plan::{BuildPlan, Layer, Step};
use crate::resolver::{PackageRef, Resolver};
use crate::Result;

use super::{BuildStepOptions, StepBuilder};

/// ImageStepBuilder：基于镜像的步骤构建器
///
/// 对齐 railpack `core/generate/image_step_builder.go`
/// 用于需要特定 Docker 镜像的构建步骤
pub struct ImageStepBuilder {
    pub display_name: String,
    pub packages: Vec<PackageRef>,
    pub resolve_step_image: Box<dyn Fn(&BuildStepOptions) -> String + Send + Sync>,
    pub apt_packages: Vec<String>,
}

impl ImageStepBuilder {
    pub fn new(
        name: &str,
        resolve_fn: Box<dyn Fn(&BuildStepOptions) -> String + Send + Sync>,
    ) -> Self {
        Self {
            display_name: name.to_string(),
            packages: Vec::new(),
            resolve_step_image: resolve_fn,
            apt_packages: Vec::new(),
        }
    }

    /// 注册默认包版本
    pub fn default_package(
        &mut self,
        resolver: &mut Resolver,
        name: &str,
        default_version: &str,
    ) -> PackageRef {
        for pkg in &self.packages {
            if pkg.name == name {
                return pkg.clone();
            }
        }
        let pkg_ref = resolver.default_package(name, default_version);
        self.packages.push(pkg_ref.clone());
        pkg_ref
    }

    /// 更新包版本
    pub fn version(
        &self,
        resolver: &mut Resolver,
        pkg_ref: &PackageRef,
        version: &str,
        source: &str,
    ) {
        resolver.version(pkg_ref, version, source);
    }

    /// 设置版本可用性检查
    pub fn set_version_available(
        &self,
        resolver: &mut Resolver,
        pkg_ref: &PackageRef,
        is_available: Box<dyn Fn(&str) -> bool + Send + Sync>,
    ) {
        resolver.set_version_available(pkg_ref, is_available);
    }
}

impl StepBuilder for ImageStepBuilder {
    fn name(&self) -> &str {
        &self.display_name
    }

    fn build(&self, plan: &mut BuildPlan, options: &mut BuildStepOptions) -> Result<()> {
        let image = (self.resolve_step_image)(options);

        let mut step = Step::new(&self.display_name);
        step.secrets = vec![];
        step.inputs = vec![Layer::new_image_layer(image, None)];

        if !self.apt_packages.is_empty() {
            step.commands = vec![BuildStepOptions::new_apt_install_command(
                &self.apt_packages,
            )];
        }

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

    #[allow(dead_code)]
    struct MockResolver;
    impl VersionResolver for MockResolver {
        fn get_latest_version(&self, _pkg: &str, version: &str) -> Result<String> {
            Ok(format!("{}.0.0", version))
        }
        fn get_all_versions(&self, _pkg: &str, _version: &str) -> Result<Vec<String>> {
            Ok(vec!["1.0.0".to_string()])
        }
    }

    fn make_options() -> BuildStepOptions {
        BuildStepOptions {
            resolved_packages: HashMap::new(),
            caches: CacheContext::new(),
        }
    }

    #[test]
    fn test_image_step_builder_resolves_image() {
        let builder = ImageStepBuilder::new(
            "packages:golang",
            Box::new(|_opts| "golang:1.21".to_string()),
        );

        let mut plan = BuildPlan::new();
        let mut options = make_options();
        builder.build(&mut plan, &mut options).unwrap();

        assert_eq!(plan.steps.len(), 1);
        assert_eq!(
            plan.steps[0].inputs[0].image.as_deref(),
            Some("golang:1.21")
        );
    }

    #[test]
    fn test_image_step_builder_dynamic_resolution() {
        let builder = ImageStepBuilder::new(
            "packages:node",
            Box::new(|opts| {
                if let Some(pkg) = opts.resolved_packages.get("node") {
                    if let Some(ref v) = pkg.resolved_version {
                        return format!("node:{}", v);
                    }
                }
                "node:latest".to_string()
            }),
        );

        let mut plan = BuildPlan::new();
        let mut options = BuildStepOptions {
            resolved_packages: {
                let mut m = HashMap::new();
                m.insert(
                    "node".to_string(),
                    ResolvedPackage {
                        name: "node".to_string(),
                        requested_version: Some("22".to_string()),
                        resolved_version: Some("22.0.0".to_string()),
                        source: "default".to_string(),
                    },
                );
                m
            },
            caches: CacheContext::new(),
        };

        builder.build(&mut plan, &mut options).unwrap();
        assert_eq!(
            plan.steps[0].inputs[0].image.as_deref(),
            Some("node:22.0.0")
        );
    }

    #[test]
    fn test_image_step_builder_with_apt() {
        let mut builder =
            ImageStepBuilder::new("packages:custom", Box::new(|_| "ubuntu:22.04".to_string()));
        builder.apt_packages = vec!["curl".to_string()];

        let mut plan = BuildPlan::new();
        let mut options = make_options();
        builder.build(&mut plan, &mut options).unwrap();

        assert!(!plan.steps[0].commands.is_empty());
    }
}
