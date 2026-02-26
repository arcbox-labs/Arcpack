/// Filter —— 文件过滤器（include/exclude）
///
/// 对齐 railpack `core/plan/filters.go`。
/// 通过 serde flatten 内嵌到 Layer 结构体中。
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Filter {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

impl Filter {
    /// 创建包含 include 和 exclude 的过滤器
    pub fn new(include: Vec<String>, exclude: Vec<String>) -> Self {
        Self { include, exclude }
    }

    /// 创建仅包含 include 的过滤器
    pub fn include_only(include: Vec<String>) -> Self {
        Self {
            include,
            exclude: Vec::new(),
        }
    }

    /// 检查过滤器是否为空（无 include 也无 exclude）
    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_new_creates_both_fields() {
        let f = Filter::new(
            vec!["src/**".to_string()],
            vec!["*.test.ts".to_string()],
        );
        assert_eq!(f.include, vec!["src/**"]);
        assert_eq!(f.exclude, vec!["*.test.ts"]);
    }

    #[test]
    fn test_filter_include_only_leaves_exclude_empty() {
        let f = Filter::include_only(vec![".".to_string()]);
        assert_eq!(f.include, vec!["."]);
        assert!(f.exclude.is_empty());
    }

    #[test]
    fn test_filter_empty_fields_skipped_in_json() {
        let f = Filter::default();
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn test_filter_json_roundtrip() {
        let f = Filter::new(
            vec!["*.go".to_string()],
            vec!["vendor/**".to_string()],
        );
        let json = serde_json::to_string(&f).unwrap();
        let parsed: Filter = serde_json::from_str(&json).unwrap();
        assert_eq!(f, parsed);
    }

    #[test]
    fn test_filter_is_empty() {
        assert!(Filter::default().is_empty());
        assert!(!Filter::include_only(vec![".".to_string()]).is_empty());
    }
}
