use std::collections::HashMap;

use crate::plan::cache::{Cache, CacheType};

pub const APT_CACHE_KEY: &str = "apt";
pub const MISE_CACHE_KEY: &str = "mise";

/// 缓存上下文：管理构建过程中的缓存定义
#[derive(Debug, Clone, Default)]
pub struct CacheContext {
    pub caches: HashMap<String, Cache>,
}

impl CacheContext {
    pub fn new() -> Self {
        Self {
            caches: HashMap::new(),
        }
    }

    /// 添加共享类型缓存，返回规范化后的缓存名
    pub fn add_cache(&mut self, name: &str, directory: &str) -> String {
        self.add_cache_with_type(name, directory, CacheType::Shared)
    }

    /// 添加指定类型的缓存，返回规范化后的缓存名
    pub fn add_cache_with_type(
        &mut self,
        name: &str,
        directory: &str,
        cache_type: CacheType,
    ) -> String {
        let sanitized = sanitize_cache_name(name);
        self.caches.insert(
            sanitized.clone(),
            Cache {
                directory: directory.to_string(),
                cache_type,
            },
        );
        sanitized
    }

    /// 直接设置缓存条目
    pub fn set_cache(&mut self, name: &str, cache: Cache) {
        self.caches.insert(name.to_string(), cache);
    }

    /// 获取缓存条目
    pub fn get_cache(&self, name: &str) -> Option<&Cache> {
        self.caches.get(name)
    }

    /// 获取 apt 缓存键列表（自动注册 apt + apt-lists）
    pub fn get_apt_caches(&mut self) -> Vec<String> {
        if !self.caches.contains_key(APT_CACHE_KEY) {
            self.caches.insert(
                APT_CACHE_KEY.to_string(),
                Cache {
                    directory: "/var/cache/apt".to_string(),
                    cache_type: CacheType::Locked,
                },
            );
        }

        let apt_lists_key = "apt-lists";
        if !self.caches.contains_key(apt_lists_key) {
            self.caches.insert(
                apt_lists_key.to_string(),
                Cache {
                    directory: "/var/lib/apt/lists".to_string(),
                    cache_type: CacheType::Locked,
                },
            );
        }

        vec![APT_CACHE_KEY.to_string(), apt_lists_key.to_string()]
    }
}

/// 规范化缓存名：去首尾斜杠，/ 替换为 -
fn sanitize_cache_name(name: &str) -> String {
    let name = name.strip_prefix('/').unwrap_or(name);
    let name = name.strip_suffix('/').unwrap_or(name);
    name.replace('/', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_cache_name_strips_leading_slash() {
        assert_eq!(sanitize_cache_name("/foo/bar"), "foo-bar");
    }

    #[test]
    fn test_sanitize_cache_name_strips_trailing_slash() {
        assert_eq!(sanitize_cache_name("foo/bar/"), "foo-bar");
    }

    #[test]
    fn test_sanitize_cache_name_strips_both_slashes() {
        assert_eq!(sanitize_cache_name("/foo/bar/"), "foo-bar");
    }

    #[test]
    fn test_sanitize_cache_name_replaces_slashes() {
        assert_eq!(sanitize_cache_name("a/b/c"), "a-b-c");
    }

    #[test]
    fn test_sanitize_cache_name_no_slashes() {
        assert_eq!(sanitize_cache_name("npm"), "npm");
    }

    #[test]
    fn test_add_cache_returns_sanitized_name() {
        let mut ctx = CacheContext::new();
        let name = ctx.add_cache("/root/.npm/", "/root/.npm");
        assert_eq!(name, "root-.npm");
        assert!(ctx.caches.contains_key("root-.npm"));
    }

    #[test]
    fn test_add_cache_default_shared_type() {
        let mut ctx = CacheContext::new();
        ctx.add_cache("npm", "/root/.npm");
        let cache = ctx.get_cache("npm").unwrap();
        assert_eq!(cache.cache_type, CacheType::Shared);
    }

    #[test]
    fn test_add_cache_with_type_locked() {
        let mut ctx = CacheContext::new();
        ctx.add_cache_with_type("yarn", "/usr/local/yarn", CacheType::Locked);
        let cache = ctx.get_cache("yarn").unwrap();
        assert_eq!(cache.cache_type, CacheType::Locked);
    }

    #[test]
    fn test_get_apt_caches_registers_both() {
        let mut ctx = CacheContext::new();
        let keys = ctx.get_apt_caches();
        assert_eq!(keys, vec!["apt", "apt-lists"]);
        assert!(ctx.caches.contains_key("apt"));
        assert!(ctx.caches.contains_key("apt-lists"));
    }

    #[test]
    fn test_get_apt_caches_locked_type() {
        let mut ctx = CacheContext::new();
        ctx.get_apt_caches();
        assert_eq!(ctx.get_cache("apt").unwrap().cache_type, CacheType::Locked);
        assert_eq!(
            ctx.get_cache("apt-lists").unwrap().cache_type,
            CacheType::Locked
        );
    }

    #[test]
    fn test_get_apt_caches_idempotent() {
        let mut ctx = CacheContext::new();
        let keys1 = ctx.get_apt_caches();
        let keys2 = ctx.get_apt_caches();
        assert_eq!(keys1, keys2);
        assert_eq!(ctx.caches.len(), 2);
    }
}
