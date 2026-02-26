use std::any::Any;
use std::collections::HashMap;

use crate::app::environment::Environment;
use crate::plan::{BuildPlan, Command, Layer, Step, spread, spread_strings};
use crate::Result;

use super::{BuildStepOptions, StepBuilder};

/// CommandStepBuilder：通用命令步骤构建器
///
/// 对齐 railpack `core/generate/command_step_builder.go`
pub struct CommandStepBuilder {
    pub display_name: String,
    pub commands: Vec<Command>,
    pub inputs: Vec<Layer>,
    pub assets: HashMap<String, String>,
    pub variables: HashMap<String, String>,
    pub caches: Vec<String>,
    pub secrets: Vec<String>,
}

impl CommandStepBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            display_name: name.to_string(),
            commands: Vec::new(),
            inputs: Vec::new(),
            assets: HashMap::new(),
            variables: HashMap::new(),
            caches: Vec::new(),
            secrets: vec!["*".to_string()],
        }
    }

    pub fn add_input(&mut self, input: Layer) {
        self.inputs.push(input);
    }

    pub fn add_inputs(&mut self, inputs: &[Layer]) {
        self.inputs.extend_from_slice(inputs);
    }

    pub fn add_command(&mut self, command: Command) {
        self.commands.push(command);
    }

    pub fn add_commands(&mut self, commands: &[Command]) {
        self.commands.extend_from_slice(commands);
    }

    pub fn add_variables(&mut self, variables: &HashMap<String, String>) {
        for (k, v) in variables {
            self.variables.insert(k.clone(), v.clone());
        }
    }

    pub fn add_cache(&mut self, name: &str) {
        if name.is_empty() {
            return;
        }
        self.caches.push(name.to_string());
    }

    pub fn add_paths(&mut self, paths: &[String]) {
        for path in paths {
            self.commands.push(Command::new_path(path));
        }
    }

    /// 从环境变量中筛选特定前缀的 secret（仅取 key）
    pub fn use_secrets_with_prefix(&mut self, env: &Environment, prefix: &str) {
        let secrets = env.get_secrets_with_prefix(prefix);
        self.secrets.extend(secrets.into_keys());
    }

    /// 在 CI 环境下添加指定 secret
    pub fn use_secrets(&mut self, env: &Environment, secrets: &[String]) {
        if env.get_variable("CI").is_some() {
            self.secrets.extend_from_slice(secrets);
        }
    }

    /// 从配置步骤合并属性
    pub fn merge_from_config_step(&mut self, step: &Step) {
        self.inputs = spread(step.inputs.clone(), self.inputs.clone());
        self.commands = spread(step.commands.clone(), self.commands.clone());
        self.secrets = spread_strings(step.secrets.clone(), self.secrets.clone());
        self.caches = spread_strings(step.caches.clone(), self.caches.clone());
        self.add_variables(&step.variables);
        for (k, v) in &step.assets {
            self.assets.insert(k.clone(), v.clone());
        }
    }
}

impl StepBuilder for CommandStepBuilder {
    fn name(&self) -> &str {
        &self.display_name
    }

    fn build(&self, plan: &mut BuildPlan, _options: &mut BuildStepOptions) -> Result<()> {
        let mut step = Step::new(&self.display_name);
        step.inputs = self.inputs.clone();
        step.commands = self.commands.clone();
        step.assets = self.assets.clone();
        step.caches = self.caches.clone();
        step.variables = self.variables.clone();
        step.secrets = self.secrets.clone();

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
    use super::*;

    #[test]
    fn test_command_step_builder_default_secrets() {
        let csb = CommandStepBuilder::new("install");
        assert_eq!(csb.secrets, vec!["*"]);
    }

    #[test]
    fn test_command_step_builder_build_writes_step() {
        let mut csb = CommandStepBuilder::new("install");
        csb.add_command(Command::new_exec("npm ci"));

        let mut plan = BuildPlan::new();
        let mut options = BuildStepOptions {
            resolved_packages: HashMap::new(),
            caches: super::super::cache_context::CacheContext::new(),
        };

        csb.build(&mut plan, &mut options).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].name, Some("install".to_string()));
        assert_eq!(plan.steps[0].commands.len(), 1);
    }

    #[test]
    fn test_add_paths_creates_path_commands() {
        let mut csb = CommandStepBuilder::new("setup");
        csb.add_paths(&["/usr/local/bin".to_string(), "/app/bin".to_string()]);
        assert_eq!(csb.commands.len(), 2);
        assert_eq!(csb.commands[0].command_type(), "globalPath");
    }

    #[test]
    fn test_add_cache_ignores_empty() {
        let mut csb = CommandStepBuilder::new("install");
        csb.add_cache("");
        assert!(csb.caches.is_empty());
    }

    #[test]
    fn test_add_cache_adds_name() {
        let mut csb = CommandStepBuilder::new("install");
        csb.add_cache("npm");
        assert_eq!(csb.caches, vec!["npm"]);
    }

    #[test]
    fn test_use_secrets_with_prefix() {
        let mut env = Environment::new(HashMap::new());
        env.set_variable("NPM_TOKEN", "secret123");
        env.set_variable("OTHER_VAR", "value");

        let mut csb = CommandStepBuilder::new("install");
        csb.use_secrets_with_prefix(&env, "NPM_");
        assert!(csb.secrets.contains(&"NPM_TOKEN".to_string()));
        assert!(!csb.secrets.contains(&"OTHER_VAR".to_string()));
    }
}
