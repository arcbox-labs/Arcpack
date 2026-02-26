/// PlanPackages —— 构建包声明
///
/// 对齐 railpack `core/plan/packages.go`。
/// 包含 apt 系统包和 mise 运行时包两类。
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanPackages {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apt: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub mise: HashMap<String, String>,
}

impl PlanPackages {
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加 apt 包
    pub fn add_apt_package(&mut self, pkg: impl Into<String>) {
        self.apt.push(pkg.into());
    }

    /// 添加 mise 包（语言运行时）
    pub fn add_mise_package(&mut self, pkg: impl Into<String>, version: impl Into<String>) {
        self.mise.insert(pkg.into(), version.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_packages_new_is_empty() {
        let pkgs = PlanPackages::new();
        assert!(pkgs.apt.is_empty());
        assert!(pkgs.mise.is_empty());
    }

    #[test]
    fn test_add_apt_package() {
        let mut pkgs = PlanPackages::new();
        pkgs.add_apt_package("curl");
        pkgs.add_apt_package("git");
        assert_eq!(pkgs.apt, vec!["curl", "git"]);
    }

    #[test]
    fn test_add_mise_package() {
        let mut pkgs = PlanPackages::new();
        pkgs.add_mise_package("node", "18");
        pkgs.add_mise_package("go", "1.21");
        assert_eq!(pkgs.mise.get("node"), Some(&"18".to_string()));
        assert_eq!(pkgs.mise.get("go"), Some(&"1.21".to_string()));
    }

    #[test]
    fn test_plan_packages_empty_fields_skipped_in_json() {
        let pkgs = PlanPackages::new();
        let json = serde_json::to_string(&pkgs).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn test_plan_packages_json_roundtrip() {
        let mut pkgs = PlanPackages::new();
        pkgs.add_apt_package("curl");
        pkgs.add_mise_package("node", "20");

        let json = serde_json::to_string(&pkgs).unwrap();
        let parsed: PlanPackages = serde_json::from_str(&json).unwrap();
        assert_eq!(pkgs, parsed);
    }
}
