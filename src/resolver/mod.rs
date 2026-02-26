pub mod version;

use std::collections::HashMap;

use crate::Result;
use version::resolve_to_fuzzy_version;

/// 默认来源标识
pub const DEFAULT_SOURCE: &str = "arcpack default";

/// 包引用（用于链式 API）
#[derive(Debug, Clone)]
pub struct PackageRef {
    pub name: String,
}

/// 请求安装的包
pub struct RequestedPackage {
    pub name: String,
    pub version: String,
    pub source: String,
    /// 自定义版本校验回调（用于检查特定版本是否可用）
    pub is_version_available: Option<Box<dyn Fn(&str) -> bool + Send + Sync>>,
    /// 跳过 mise 安装（仅解析版本号用于展示）
    pub skip_mise_install: bool,
}

impl RequestedPackage {
    pub fn new(name: &str, default_version: &str) -> Self {
        Self {
            name: name.to_string(),
            version: default_version.to_string(),
            source: DEFAULT_SOURCE.to_string(),
            is_version_available: None,
            skip_mise_install: false,
        }
    }

    pub fn set_version(&mut self, version: &str, source: &str) {
        self.version = version.to_string();
        self.source = source.to_string();
    }
}

/// 已解析的包版本信息
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResolvedPackage {
    pub name: String,
    #[serde(rename = "requestedVersion", skip_serializing_if = "Option::is_none")]
    pub requested_version: Option<String>,
    #[serde(rename = "resolvedVersion", skip_serializing_if = "Option::is_none")]
    pub resolved_version: Option<String>,
    pub source: String,
}

/// 版本解析后端 trait（单元测试用 mock 实现）
pub trait VersionResolver: Send + Sync {
    /// 获取指定包的最新匹配版本
    fn get_latest_version(&self, pkg: &str, version: &str) -> Result<String>;
    /// 获取指定包的所有匹配版本列表
    fn get_all_versions(&self, pkg: &str, version: &str) -> Result<Vec<String>>;
}

/// 包版本解析器
pub struct Resolver {
    version_resolver: Box<dyn VersionResolver>,
    packages: HashMap<String, RequestedPackage>,
    previous_versions: HashMap<String, String>,
}

impl Resolver {
    pub fn new(version_resolver: Box<dyn VersionResolver>) -> Self {
        Self {
            version_resolver,
            packages: HashMap::new(),
            previous_versions: HashMap::new(),
        }
    }

    /// 注册一个包的默认版本，返回 PackageRef
    /// 如果存在 previous_version 则覆盖版本号
    pub fn default_package(&mut self, name: &str, default_version: &str) -> PackageRef {
        self.packages
            .insert(name.to_string(), RequestedPackage::new(name, default_version));

        // 如果存在历史版本且不同于默认版本，则使用历史版本
        if let Some(prev) = self.previous_versions.get(name) {
            if prev != default_version {
                let prev = prev.clone();
                self.version(
                    &PackageRef {
                        name: name.to_string(),
                    },
                    &prev,
                    "previous installed version",
                );
            }
        }

        PackageRef {
            name: name.to_string(),
        }
    }

    /// 更新已注册包的版本和来源
    pub fn version(&mut self, pkg_ref: &PackageRef, version: &str, source: &str) -> PackageRef {
        if let Some(pkg) = self.packages.get_mut(&pkg_ref.name) {
            pkg.set_version(version.trim(), source);
        }
        pkg_ref.clone()
    }

    /// 设置历史版本（用于增量构建）
    pub fn set_previous_version(&mut self, name: &str, version: &str) {
        self.previous_versions
            .insert(name.to_string(), version.to_string());
    }

    /// 设置版本可用性检查回调
    pub fn set_version_available(
        &mut self,
        pkg_ref: &PackageRef,
        is_version_available: Box<dyn Fn(&str) -> bool + Send + Sync>,
    ) {
        if let Some(pkg) = self.packages.get_mut(&pkg_ref.name) {
            pkg.is_version_available = Some(is_version_available);
        }
    }

    /// 设置跳过 mise 安装标志
    pub fn set_skip_mise_install(&mut self, pkg_ref: &PackageRef, skip: bool) {
        if let Some(pkg) = self.packages.get_mut(&pkg_ref.name) {
            pkg.skip_mise_install = skip;
        }
    }

    /// 获取已注册的包
    pub fn get(&self, name: &str) -> Option<&RequestedPackage> {
        self.packages.get(name)
    }

    /// 获取所有已注册包的可变引用（用于配置阶段）
    pub fn packages_mut(&mut self) -> &mut HashMap<String, RequestedPackage> {
        &mut self.packages
    }

    /// 批量解析所有已注册包的版本
    pub fn resolve_packages(&self) -> Result<HashMap<String, ResolvedPackage>> {
        let mut resolved = HashMap::new();

        for (name, pkg) in &self.packages {
            let fuzzy_version = resolve_to_fuzzy_version(&pkg.version);

            let latest_version = if let Some(ref is_available) = pkg.is_version_available {
                // 有自定义校验：获取所有版本，从新到旧找第一个可用的
                let versions = self.version_resolver.get_all_versions(name, &fuzzy_version)?;
                let mut found = None;
                for v in versions.iter().rev() {
                    if is_available(v) {
                        found = Some(v.clone());
                        break;
                    }
                }
                found.ok_or_else(|| {
                    anyhow::anyhow!("no version available for {} {}", name, pkg.version)
                })?
            } else {
                // 直接获取最新版本
                match self.version_resolver.get_latest_version(name, &fuzzy_version) {
                    Ok(v) => v,
                    Err(e) => {
                        if pkg.skip_mise_install {
                            // 跳过 mise 安装时，解析失败不报错
                            fuzzy_version
                        } else {
                            return Err(e);
                        }
                    }
                }
            };

            resolved.insert(
                name.clone(),
                ResolvedPackage {
                    name: name.clone(),
                    requested_version: Some(pkg.version.clone()),
                    resolved_version: Some(latest_version),
                    source: pkg.source.clone(),
                },
            );
        }

        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock 版本解析器
    struct MockVersionResolver {
        versions: HashMap<String, Vec<String>>,
    }

    impl MockVersionResolver {
        fn new() -> Self {
            Self {
                versions: HashMap::new(),
            }
        }

        fn add_versions(&mut self, pkg: &str, versions: Vec<&str>) {
            self.versions.insert(
                pkg.to_string(),
                versions.into_iter().map(|s| s.to_string()).collect(),
            );
        }
    }

    impl VersionResolver for MockVersionResolver {
        fn get_latest_version(&self, pkg: &str, _version: &str) -> Result<String> {
            self.versions
                .get(pkg)
                .and_then(|v| v.last().cloned())
                .ok_or_else(|| anyhow::anyhow!("package not found: {}", pkg).into())
        }

        fn get_all_versions(&self, pkg: &str, _version: &str) -> Result<Vec<String>> {
            self.versions
                .get(pkg)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("package not found: {}", pkg).into())
        }
    }

    #[test]
    fn test_resolver_default_and_resolve() {
        let mut mock = MockVersionResolver::new();
        mock.add_versions("node", vec!["18.0.0", "18.4.0", "22.0.0"]);

        let mut resolver = Resolver::new(Box::new(mock));
        resolver.default_package("node", "22");

        let resolved = resolver.resolve_packages().unwrap();
        let node = resolved.get("node").unwrap();
        assert_eq!(node.name, "node");
        assert_eq!(node.requested_version.as_deref(), Some("22"));
        assert_eq!(node.resolved_version.as_deref(), Some("22.0.0"));
    }

    #[test]
    fn test_resolver_version_updates_package() {
        let mut mock = MockVersionResolver::new();
        mock.add_versions("node", vec!["18.0.0", "18.4.0", "22.0.0"]);

        let mut resolver = Resolver::new(Box::new(mock));
        let pkg_ref = resolver.default_package("node", "18");
        resolver.version(&pkg_ref, "22", "user config");

        let pkg = resolver.get("node").unwrap();
        assert_eq!(pkg.version, "22");
        assert_eq!(pkg.source, "user config");
    }

    #[test]
    fn test_resolver_previous_version_overrides_default() {
        let mut mock = MockVersionResolver::new();
        mock.add_versions("node", vec!["18.0.0", "20.0.0"]);

        let mut resolver = Resolver::new(Box::new(mock));
        resolver.set_previous_version("node", "20");
        resolver.default_package("node", "18");

        let pkg = resolver.get("node").unwrap();
        assert_eq!(pkg.version, "20");
        assert_eq!(pkg.source, "previous installed version");
    }

    #[test]
    fn test_resolver_previous_version_same_as_default_no_override() {
        let mut mock = MockVersionResolver::new();
        mock.add_versions("node", vec!["18.0.0"]);

        let mut resolver = Resolver::new(Box::new(mock));
        resolver.set_previous_version("node", "18");
        resolver.default_package("node", "18");

        let pkg = resolver.get("node").unwrap();
        assert_eq!(pkg.source, DEFAULT_SOURCE);
    }

    #[test]
    fn test_resolver_version_available_callback() {
        let mut mock = MockVersionResolver::new();
        mock.add_versions("node", vec!["18.0.0", "18.4.0", "20.0.0"]);

        let mut resolver = Resolver::new(Box::new(mock));
        let pkg_ref = resolver.default_package("node", "18");
        resolver.set_version_available(
            &pkg_ref,
            Box::new(|v: &str| v.starts_with("18.")),
        );

        let resolved = resolver.resolve_packages().unwrap();
        let node = resolved.get("node").unwrap();
        // 从后往前找，第一个匹配 18.x 的是 18.4.0
        assert_eq!(node.resolved_version.as_deref(), Some("18.4.0"));
    }

    #[test]
    fn test_resolver_skip_mise_install_fallback() {
        // 模拟版本解析失败
        struct FailResolver;
        impl VersionResolver for FailResolver {
            fn get_latest_version(&self, _pkg: &str, _version: &str) -> Result<String> {
                Err(anyhow::anyhow!("resolve failed").into())
            }
            fn get_all_versions(&self, _pkg: &str, _version: &str) -> Result<Vec<String>> {
                Err(anyhow::anyhow!("resolve failed").into())
            }
        }

        let mut resolver = Resolver::new(Box::new(FailResolver));
        let pkg_ref = resolver.default_package("bun", "1.0");
        resolver.set_skip_mise_install(&pkg_ref, true);

        let resolved = resolver.resolve_packages().unwrap();
        let bun = resolved.get("bun").unwrap();
        // skip_mise_install 时解析失败回退到 fuzzy version
        assert_eq!(bun.resolved_version.as_deref(), Some("1.0"));
    }

    #[test]
    fn test_resolver_multiple_packages() {
        let mut mock = MockVersionResolver::new();
        mock.add_versions("node", vec!["22.0.0"]);
        mock.add_versions("pnpm", vec!["8.0.0", "9.0.0"]);

        let mut resolver = Resolver::new(Box::new(mock));
        resolver.default_package("node", "22");
        resolver.default_package("pnpm", "9");

        let resolved = resolver.resolve_packages().unwrap();
        assert_eq!(resolved.len(), 2);
        assert!(resolved.contains_key("node"));
        assert!(resolved.contains_key("pnpm"));
    }
}
