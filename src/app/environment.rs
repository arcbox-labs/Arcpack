/// Environment —— 环境变量管理
///
/// 对齐 railpack `core/app/environment.go`。
/// 管理环境变量，支持 ARCPACK_ 前缀的配置变量。
use std::collections::HashMap;

/// 配置变量前缀
const CONFIG_PREFIX: &str = "ARCPACK_";

#[derive(Debug, Clone, Default)]
pub struct Environment {
    pub variables: HashMap<String, String>,
}

impl Environment {
    /// 从 HashMap 构造
    pub fn new(variables: HashMap<String, String>) -> Self {
        Self { variables }
    }

    /// 从 "KEY=VALUE" 字符串列表构造
    ///
    /// 对齐 railpack 的 FromEnvs：
    /// - 有 = 号：使用提供的值
    /// - 无 = 号或值为空：从 OS 环境变量取值
    pub fn from_envs(envs: Vec<String>) -> Self {
        let mut variables = HashMap::new();

        for env_str in envs {
            if let Some(eq_pos) = env_str.find('=') {
                let name = &env_str[..eq_pos];
                let value = &env_str[eq_pos + 1..];

                if value.is_empty() {
                    // 值为空，从 OS 环境变量取值
                    if let Ok(os_val) = std::env::var(name) {
                        variables.insert(name.to_string(), os_val);
                    }
                } else {
                    variables.insert(name.to_string(), value.to_string());
                }
            } else {
                // 无 = 号，从 OS 环境变量取值
                if let Ok(os_val) = std::env::var(&env_str) {
                    variables.insert(env_str, os_val);
                }
            }
        }

        Self { variables }
    }

    /// 获取变量值
    pub fn get_variable(&self, name: &str) -> Option<&String> {
        self.variables.get(name)
    }

    /// 设置变量
    pub fn set_variable(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.variables.insert(name.into(), value.into());
    }

    /// 返回 ARCPACK_ 前缀变量名
    pub fn config_variable(name: &str) -> String {
        format!("{}{}", CONFIG_PREFIX, name)
    }

    /// 获取 ARCPACK_ 前缀的配置变量
    ///
    /// 返回 (Option<值>, 完整变量名) 二元组，用于日志追踪来源。
    /// 对齐 railpack 的 GetConfigVariable 双返回值语义。
    pub fn get_config_variable(&self, name: &str) -> (Option<String>, String) {
        let config_var = Self::config_variable(name);

        if let Some(val) = self.variables.get(&config_var) {
            (Some(val.trim().to_string()), config_var)
        } else {
            (None, config_var)
        }
    }

    /// 获取空格分隔的配置变量列表
    pub fn get_config_variable_list(&self, name: &str) -> (Vec<String>, String) {
        let (val, config_var) = self.get_config_variable(name);

        match val {
            Some(v) if !v.is_empty() => {
                let list: Vec<String> = v.split_whitespace().map(String::from).collect();
                (list, config_var)
            }
            _ => (Vec::new(), config_var),
        }
    }

    /// 检查配置变量是否为 truthy（"1" 或 "true"）
    pub fn is_config_variable_truthy(&self, name: &str) -> bool {
        let (val, _) = self.get_config_variable(name);
        match val {
            Some(v) => {
                let lower = v.to_lowercase();
                lower == "1" || lower == "true"
            }
            None => false,
        }
    }

    /// 按前缀过滤变量名
    ///
    /// 对齐 railpack 的 GetSecretsWithPrefix。
    pub fn get_secrets_with_prefix(&self, prefix: &str) -> HashMap<String, String> {
        self.variables
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_envs_parses_key_value_pairs() {
        let envs = vec![
            "NODE_VERSION=18".to_string(),
            "GO_VERSION=1.21".to_string(),
        ];
        let env = Environment::from_envs(envs);
        assert_eq!(env.get_variable("NODE_VERSION"), Some(&"18".to_string()));
        assert_eq!(env.get_variable("GO_VERSION"), Some(&"1.21".to_string()));
    }

    #[test]
    fn test_from_envs_missing_value_falls_back_to_os_env() {
        // 设置一个 OS 环境变量用于测试
        std::env::set_var("ARCPACK_TEST_FROM_ENVS", "test_value");

        let envs = vec!["ARCPACK_TEST_FROM_ENVS".to_string()];
        let env = Environment::from_envs(envs);
        assert_eq!(
            env.get_variable("ARCPACK_TEST_FROM_ENVS"),
            Some(&"test_value".to_string())
        );

        std::env::remove_var("ARCPACK_TEST_FROM_ENVS");
    }

    #[test]
    fn test_get_config_variable_returns_value_and_name() {
        let mut env = Environment::default();
        env.set_variable("ARCPACK_NODE_VERSION", "18");

        let (val, name) = env.get_config_variable("NODE_VERSION");
        assert_eq!(val, Some("18".to_string()));
        assert_eq!(name, "ARCPACK_NODE_VERSION");
    }

    #[test]
    fn test_get_config_variable_missing_returns_none_and_name() {
        let env = Environment::default();
        let (val, name) = env.get_config_variable("NODE_VERSION");
        assert_eq!(val, None);
        assert_eq!(name, "ARCPACK_NODE_VERSION");
    }

    #[test]
    fn test_get_config_variable_trims_whitespace() {
        let mut env = Environment::default();
        env.set_variable("ARCPACK_VERSION", "  18  ");

        let (val, _) = env.get_config_variable("VERSION");
        assert_eq!(val, Some("18".to_string()));
    }

    #[test]
    fn test_get_config_variable_list_splits_by_whitespace() {
        let mut env = Environment::default();
        env.set_variable("ARCPACK_PACKAGES", "curl git wget");

        let (list, name) = env.get_config_variable_list("PACKAGES");
        assert_eq!(list, vec!["curl", "git", "wget"]);
        assert_eq!(name, "ARCPACK_PACKAGES");
    }

    #[test]
    fn test_is_config_variable_truthy() {
        let mut env = Environment::default();
        env.set_variable("ARCPACK_ENABLE_CACHE", "true");
        env.set_variable("ARCPACK_DEBUG", "1");
        env.set_variable("ARCPACK_VERBOSE", "false");

        assert!(env.is_config_variable_truthy("ENABLE_CACHE"));
        assert!(env.is_config_variable_truthy("DEBUG"));
        assert!(!env.is_config_variable_truthy("VERBOSE"));
        assert!(!env.is_config_variable_truthy("NONEXISTENT"));
    }

    #[test]
    fn test_get_secrets_with_prefix() {
        let mut env = Environment::default();
        env.set_variable("DATABASE_URL", "postgres://...");
        env.set_variable("DATABASE_PASSWORD", "secret");
        env.set_variable("REDIS_URL", "redis://...");

        let secrets = env.get_secrets_with_prefix("DATABASE_");
        assert_eq!(secrets.len(), 2);
        assert!(secrets.contains_key("DATABASE_URL"));
        assert!(secrets.contains_key("DATABASE_PASSWORD"));
        assert!(!secrets.contains_key("REDIS_URL"));
    }
}
