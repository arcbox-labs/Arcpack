use std::collections::HashMap;

/// PackageJson 解析
///
/// 对齐 railpack `core/providers/node/package_json.go`
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct PackageJson {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub scripts: HashMap<String, String>,
    #[serde(rename = "packageManager")]
    pub package_manager: Option<String>,
    #[serde(default)]
    pub dependencies: HashMap<String, String>,
    #[serde(default, rename = "devDependencies")]
    pub dev_dependencies: HashMap<String, String>,
    #[serde(default)]
    pub engines: HashMap<String, String>,
    #[serde(default)]
    pub main: Option<String>,
    /// 自定义反序列化：支持 string[] 或 { packages: [...] }
    #[serde(default, deserialize_with = "deserialize_workspaces")]
    pub workspaces: Vec<String>,
}

impl PackageJson {
    pub fn has_script(&self, name: &str) -> bool {
        self.scripts.get(name).is_some_and(|s| !s.is_empty())
    }

    pub fn get_script(&self, name: &str) -> Option<&str> {
        self.scripts.get(name).map(|s| s.as_str())
    }

    pub fn has_dependency(&self, name: &str) -> bool {
        self.dependencies.contains_key(name) || self.dev_dependencies.contains_key(name)
    }

    pub fn has_local_dependency(&self) -> bool {
        self.dependencies
            .values()
            .chain(self.dev_dependencies.values())
            .any(|v| v.starts_with("file:"))
    }

    /// 解析 packageManager 字段 "name@version" 格式
    /// 返回 (name, version)，解析失败返回 ("", "")
    pub fn get_package_manager_info(&self) -> (String, String) {
        if let Some(ref pm) = self.package_manager {
            let pm = pm.trim();
            let parts: Vec<&str> = pm.split('@').collect();
            if parts.len() == 2 {
                let version_parts: Vec<&str> = parts[1].split('+').collect();
                return (
                    parts[0].trim().to_string(),
                    version_parts[0].trim().to_string(),
                );
            }
        }
        (String::new(), String::new())
    }
}

/// 自定义 workspaces 反序列化
/// 支持：
/// - string[]: ["packages/*", "apps/*"]
/// - { packages: ["packages/*"] }
fn deserialize_workspaces<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct WorkspacesVisitor;

    impl<'de> de::Visitor<'de> for WorkspacesVisitor {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an array of strings or an object with 'packages' field")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Vec<String>, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            let mut result = Vec::new();
            while let Some(v) = seq.next_element::<String>()? {
                result.push(v);
            }
            Ok(result)
        }

        fn visit_map<M>(self, mut map: M) -> Result<Vec<String>, M::Error>
        where
            M: de::MapAccess<'de>,
        {
            let mut packages = Vec::new();
            while let Some(key) = map.next_key::<String>()? {
                if key == "packages" {
                    packages = map.next_value()?;
                } else {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
            Ok(packages)
        }

        fn visit_unit<E>(self) -> Result<Vec<String>, E>
        where
            E: de::Error,
        {
            Ok(Vec::new())
        }
    }

    deserializer.deserialize_any(WorkspacesVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_package_json() {
        let json = r#"{
            "name": "my-app",
            "version": "1.0.0",
            "scripts": { "start": "node index.js", "build": "tsc" },
            "dependencies": { "express": "^4.18.0" }
        }"#;
        let pkg: PackageJson = serde_json::from_str(json).unwrap();
        assert_eq!(pkg.name, "my-app");
        assert!(pkg.has_script("start"));
        assert!(pkg.has_dependency("express"));
    }

    #[test]
    fn test_workspaces_array() {
        let json = r#"{ "workspaces": ["packages/*", "apps/*"] }"#;
        let pkg: PackageJson = serde_json::from_str(json).unwrap();
        assert_eq!(pkg.workspaces, vec!["packages/*", "apps/*"]);
    }

    #[test]
    fn test_workspaces_object() {
        let json = r#"{ "workspaces": { "packages": ["packages/*"] } }"#;
        let pkg: PackageJson = serde_json::from_str(json).unwrap();
        assert_eq!(pkg.workspaces, vec!["packages/*"]);
    }

    #[test]
    fn test_workspaces_missing() {
        let json = r#"{ "name": "test" }"#;
        let pkg: PackageJson = serde_json::from_str(json).unwrap();
        assert!(pkg.workspaces.is_empty());
    }

    #[test]
    fn test_package_manager_info_npm() {
        let json = r#"{ "packageManager": "npm@10.2.0" }"#;
        let pkg: PackageJson = serde_json::from_str(json).unwrap();
        let (name, version) = pkg.get_package_manager_info();
        assert_eq!(name, "npm");
        assert_eq!(version, "10.2.0");
    }

    #[test]
    fn test_package_manager_info_with_hash() {
        let json = r#"{ "packageManager": "pnpm@9.0.0+sha1.abc" }"#;
        let pkg: PackageJson = serde_json::from_str(json).unwrap();
        let (name, version) = pkg.get_package_manager_info();
        assert_eq!(name, "pnpm");
        assert_eq!(version, "9.0.0");
    }

    #[test]
    fn test_package_manager_info_none() {
        let json = r#"{ "name": "test" }"#;
        let pkg: PackageJson = serde_json::from_str(json).unwrap();
        let (name, version) = pkg.get_package_manager_info();
        assert!(name.is_empty());
        assert!(version.is_empty());
    }

    #[test]
    fn test_has_dependency_in_dev() {
        let json = r#"{ "devDependencies": { "typescript": "^5.0" } }"#;
        let pkg: PackageJson = serde_json::from_str(json).unwrap();
        assert!(pkg.has_dependency("typescript"));
        assert!(!pkg.has_dependency("express"));
    }

    #[test]
    fn test_has_local_dependency() {
        let json = r#"{ "dependencies": { "my-lib": "file:./lib" } }"#;
        let pkg: PackageJson = serde_json::from_str(json).unwrap();
        assert!(pkg.has_local_dependency());
    }

    #[test]
    fn test_no_local_dependency() {
        let json = r#"{ "dependencies": { "express": "^4.18.0" } }"#;
        let pkg: PackageJson = serde_json::from_str(json).unwrap();
        assert!(!pkg.has_local_dependency());
    }

    #[test]
    fn test_engines_field() {
        let json = r#"{ "engines": { "node": ">=18", "pnpm": ">=8" } }"#;
        let pkg: PackageJson = serde_json::from_str(json).unwrap();
        assert_eq!(pkg.engines.get("node").unwrap(), ">=18");
    }

    #[test]
    fn test_has_script_empty_returns_false() {
        let json = r#"{ "scripts": { "start": "" } }"#;
        let pkg: PackageJson = serde_json::from_str(json).unwrap();
        assert!(!pkg.has_script("start"));
    }
}
