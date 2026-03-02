/// Step —— 构建步骤
///
/// 对齐 railpack `core/plan/step.go`。
/// 每个 Step 包含名称、输入层、命令列表、secrets、assets、变量和缓存引用。
use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::command::Command;
use super::layer::Layer;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Layer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<Command>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub assets: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub variables: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caches: Vec<String>,
}

impl Step {
    /// 创建新步骤（对齐 railpack 的 NewStep）
    ///
    /// 默认 secrets = ["*"]（授予所有 secret 访问权）
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            inputs: Vec::new(),
            commands: Vec::new(),
            secrets: vec!["*".to_string()],
            assets: HashMap::new(),
            variables: HashMap::new(),
            caches: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::command::Command;
    use super::super::filter::Filter;
    use super::super::layer::Layer;
    use super::*;

    #[test]
    fn test_step_new_defaults_secrets_to_wildcard() {
        let step = Step::new("install");
        assert_eq!(step.secrets, vec!["*"]);
    }

    #[test]
    fn test_step_new_initializes_empty_collections() {
        let step = Step::new("build");
        assert_eq!(step.name, Some("build".to_string()));
        assert!(step.inputs.is_empty());
        assert!(step.commands.is_empty());
        assert!(step.assets.is_empty());
        assert!(step.variables.is_empty());
        assert!(step.caches.is_empty());
    }

    #[test]
    fn test_step_json_roundtrip_with_all_fields() {
        let mut step = Step::new("build");
        step.inputs.push(Layer::new_step_layer("install", None));
        step.inputs.push(Layer::new_local_layer());
        step.commands.push(Command::new_exec("go build -o app ."));
        step.assets
            .insert("config.toml".to_string(), "[settings]".to_string());
        step.variables
            .insert("GOFLAGS".to_string(), "-trimpath".to_string());
        step.caches.push("go-build".to_string());

        let json = serde_json::to_string_pretty(&step).unwrap();
        let parsed: Step = serde_json::from_str(&json).unwrap();
        assert_eq!(step, parsed);
    }

    #[test]
    fn test_step_empty_fields_skipped_in_json() {
        let step = Step::new("packages");
        let json = serde_json::to_string(&step).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        // 应有 name 和 secrets，其余跳过
        assert!(value.get("name").is_some());
        assert!(value.get("secrets").is_some());
        assert!(value.get("inputs").is_none());
        assert!(value.get("commands").is_none());
        assert!(value.get("assets").is_none());
        assert!(value.get("variables").is_none());
        assert!(value.get("caches").is_none());
    }

    #[test]
    fn test_step_json_with_filter_on_input() {
        let mut step = Step::new("deploy-copy");
        step.inputs.push(Layer::new_step_layer(
            "build",
            Some(Filter::include_only(vec![".".to_string()])),
        ));
        let json = serde_json::to_string(&step).unwrap();
        let parsed: Step = serde_json::from_str(&json).unwrap();
        assert_eq!(step, parsed);
    }
}
