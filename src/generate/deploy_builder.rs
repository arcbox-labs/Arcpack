use std::collections::HashMap;

use crate::plan::{BuildPlan, Deploy, Layer, Step, ARCPACK_RUNTIME_IMAGE};

use super::BuildStepOptions;

/// DeployBuilder：部署配置构建器
///
/// 对齐 railpack `core/generate/deploy_builder.go`
pub struct DeployBuilder {
    pub base: Layer,
    pub deploy_inputs: Vec<Layer>,
    pub start_cmd: Option<String>,
    pub variables: HashMap<String, String>,
    pub paths: Vec<String>,
    pub apt_packages: Vec<String>,
}

impl DeployBuilder {
    pub fn new() -> Self {
        Self {
            base: Layer::new_image_layer(ARCPACK_RUNTIME_IMAGE, None),
            deploy_inputs: Vec::new(),
            start_cmd: None,
            variables: HashMap::new(),
            paths: Vec::new(),
            apt_packages: Vec::new(),
        }
    }

    pub fn set_inputs(&mut self, layers: Vec<Layer>) {
        self.deploy_inputs = layers;
    }

    pub fn add_inputs(&mut self, layers: &[Layer]) {
        self.deploy_inputs.extend_from_slice(layers);
    }

    pub fn add_apt_packages(&mut self, packages: &[String]) {
        self.apt_packages.extend_from_slice(packages);
    }

    /// 检查某个步骤是否已有覆盖特定路径的输入
    pub fn has_include_for_step(&self, step_name: &str, path: &str) -> bool {
        for layer in &self.deploy_inputs {
            if layer.step.as_deref() != Some(step_name) {
                continue;
            }
            for inc in &layer.filter.include {
                // 精确匹配或 "." 覆盖全部
                if inc == path || inc == "." {
                    return true;
                }
            }
        }
        false
    }

    /// 构建 deploy 配置到 BuildPlan
    pub fn build(&self, plan: &mut BuildPlan, options: &mut BuildStepOptions) {
        let mut base_layer = self.base.clone();

        // 如果有运行时 apt 包，先创建 apt 安装步骤
        if !self.apt_packages.is_empty() {
            let mut runtime_apt_step = Step::new("packages:apt:runtime");
            runtime_apt_step.inputs = vec![base_layer.clone()];
            runtime_apt_step.commands = vec![
                BuildStepOptions::new_apt_install_command(&self.apt_packages),
            ];
            runtime_apt_step.caches = options.caches.get_apt_caches();
            runtime_apt_step.secrets = vec![];

            let step_name = runtime_apt_step.name.clone().unwrap();
            plan.add_step(runtime_apt_step);
            base_layer = Layer::new_step_layer(step_name, None);
        }

        plan.deploy = Deploy {
            base: Some(base_layer),
            inputs: self.deploy_inputs.clone(),
            start_cmd: self.start_cmd.clone(),
            variables: self.variables.clone(),
            paths: self.paths.clone(),
        };
    }
}

impl Default for DeployBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::cache_context::CacheContext;

    fn make_options() -> BuildStepOptions {
        BuildStepOptions {
            resolved_packages: HashMap::new(),
            caches: CacheContext::new(),
        }
    }

    #[test]
    fn test_deploy_builder_default_base() {
        let db = DeployBuilder::new();
        assert_eq!(db.base.image.as_deref(), Some(ARCPACK_RUNTIME_IMAGE));
    }

    #[test]
    fn test_deploy_builder_build_no_apt() {
        let db = DeployBuilder::new();
        let mut plan = BuildPlan::new();
        let mut options = make_options();

        db.build(&mut plan, &mut options);

        assert!(plan.steps.is_empty());
        assert!(plan.deploy.base.is_some());
    }

    #[test]
    fn test_deploy_builder_build_with_apt_creates_runtime_step() {
        let mut db = DeployBuilder::new();
        db.add_apt_packages(&["curl".to_string()]);

        let mut plan = BuildPlan::new();
        let mut options = make_options();

        db.build(&mut plan, &mut options);

        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].name, Some("packages:apt:runtime".to_string()));
        assert_eq!(
            plan.deploy.base.as_ref().unwrap().step.as_deref(),
            Some("packages:apt:runtime")
        );
    }

    #[test]
    fn test_has_include_for_step_exact_match() {
        let mut db = DeployBuilder::new();
        db.add_inputs(&[Layer::new_step_layer(
            "build",
            Some(crate::plan::Filter::include_only(vec![".".to_string()])),
        )]);
        assert!(db.has_include_for_step("build", "."));
        assert!(db.has_include_for_step("build", "something")); // "." 覆盖全部
        assert!(!db.has_include_for_step("install", "."));
    }

    #[test]
    fn test_deploy_builder_start_cmd() {
        let mut db = DeployBuilder::new();
        db.start_cmd = Some("node index.js".to_string());

        let mut plan = BuildPlan::new();
        let mut options = make_options();
        db.build(&mut plan, &mut options);

        assert_eq!(plan.deploy.start_cmd.as_deref(), Some("node index.js"));
    }
}
