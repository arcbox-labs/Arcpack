use std::collections::HashMap;

/// 构建元数据：记录构建过程中的键值信息
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Metadata {
    pub properties: HashMap<String, String>,
}

impl Metadata {
    pub fn new() -> Self {
        Self {
            properties: HashMap::new(),
        }
    }

    /// 设置键值（空值不存储）
    pub fn set(&mut self, key: &str, value: &str) {
        if value.is_empty() {
            return;
        }
        self.properties.insert(key.to_string(), value.to_string());
    }

    /// 设置布尔值（true 存为 "true"，false 不存储）
    pub fn set_bool(&mut self, key: &str, value: bool) {
        if value {
            self.properties.insert(key.to_string(), "true".to_string());
        }
    }

    /// 获取值
    pub fn get(&self, key: &str) -> Option<&str> {
        self.properties.get(key).map(|s| s.as_str())
    }

    /// 转为 HashMap
    pub fn to_map(&self) -> HashMap<String, String> {
        self.properties.clone()
    }
}

impl Default for Metadata {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_set_and_get() {
        let mut m = Metadata::new();
        m.set("framework", "next.js");
        assert_eq!(m.get("framework"), Some("next.js"));
    }

    #[test]
    fn test_metadata_set_empty_value_ignored() {
        let mut m = Metadata::new();
        m.set("key", "");
        assert_eq!(m.get("key"), None);
    }

    #[test]
    fn test_metadata_set_bool_true() {
        let mut m = Metadata::new();
        m.set_bool("spa", true);
        assert_eq!(m.get("spa"), Some("true"));
    }

    #[test]
    fn test_metadata_set_bool_false_not_stored() {
        let mut m = Metadata::new();
        m.set_bool("spa", false);
        assert_eq!(m.get("spa"), None);
    }

    #[test]
    fn test_metadata_get_nonexistent_returns_none() {
        let m = Metadata::new();
        assert_eq!(m.get("missing"), None);
    }

    #[test]
    fn test_metadata_overwrite_value() {
        let mut m = Metadata::new();
        m.set("key", "old");
        m.set("key", "new");
        assert_eq!(m.get("key"), Some("new"));
    }
}
