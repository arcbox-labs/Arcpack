use std::collections::HashMap;

use crate::plan::{Cache, CacheType};

/// BuildKit 缓存条目
#[derive(Debug, Clone)]
pub struct BuildKitCache {
    /// 带前缀的缓存键
    pub cache_key: String,
    /// 原始 Cache 定义
    pub plan_cache: Cache,
}

/// BuildKit 持久化缓存挂载管理
///
/// 对齐 railpack `cache_store.go`
#[derive(Debug)]
pub struct BuildKitCacheStore {
    /// 缓存键前缀（多租户隔离）
    unique_id: String,
    /// 缓存注册表（memoization）
    cache_map: HashMap<String, BuildKitCache>,
}

impl BuildKitCacheStore {
    /// 创建新的缓存存储
    pub fn new(unique_id: impl Into<String>) -> Self {
        Self {
            unique_id: unique_id.into(),
            cache_map: HashMap::new(),
        }
    }

    /// 获取缓存（有则复用，无则创建）
    /// key 自动添加 `{unique_id}-` 前缀
    pub fn get_cache(&mut self, key: &str, plan_cache: &Cache) -> &BuildKitCache {
        if !self.cache_map.contains_key(key) {
            let cache_key = if self.unique_id.is_empty() {
                key.to_string()
            } else {
                format!("{}-{}", self.unique_id, key)
            };
            self.cache_map.insert(
                key.to_string(),
                BuildKitCache {
                    cache_key,
                    plan_cache: plan_cache.clone(),
                },
            );
        }
        self.cache_map.get(key).unwrap()
    }

    /// 生成 cache mount 选项字符串
    ///
    /// 格式：`--mount=type=cache,target={dir},id={key}[,sharing=locked]`
    pub fn get_cache_mount_option(&mut self, key: &str, plan_cache: &Cache) -> String {
        let cache = self.get_cache(key, plan_cache);
        let mut mount = format!(
            "--mount=type=cache,target={},id={}",
            cache.plan_cache.directory, cache.cache_key
        );
        if cache.plan_cache.cache_type == CacheType::Locked {
            mount.push_str(",sharing=locked");
        }
        mount
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Cache;

    #[test]
    fn test_new_creates_empty_store() {
        let store = BuildKitCacheStore::new("app-123");
        assert!(store.cache_map.is_empty());
    }

    #[test]
    fn test_get_cache_memoization() {
        let mut store = BuildKitCacheStore::new("app");
        let cache = Cache::new("/root/.npm");
        let key1 = store.get_cache("npm", &cache).cache_key.clone();
        let key2 = store.get_cache("npm", &cache).cache_key.clone();
        assert_eq!(key1, key2);
        assert_eq!(key1, "app-npm");
    }

    #[test]
    fn test_get_cache_prefix() {
        let mut store = BuildKitCacheStore::new("project-42");
        let cache = Cache::new("/var/cache/apt");
        let result = store.get_cache("apt", &cache);
        assert_eq!(result.cache_key, "project-42-apt");
    }

    #[test]
    fn test_get_cache_empty_unique_id() {
        let mut store = BuildKitCacheStore::new("");
        let cache = Cache::new("/root/.npm");
        let result = store.get_cache("npm", &cache);
        assert_eq!(result.cache_key, "npm");
    }

    #[test]
    fn test_mount_option_shared() {
        let mut store = BuildKitCacheStore::new("app");
        let cache = Cache::new("/root/.npm");
        let opt = store.get_cache_mount_option("npm", &cache);
        assert_eq!(opt, "--mount=type=cache,target=/root/.npm,id=app-npm");
        assert!(!opt.contains("sharing=locked"));
    }

    #[test]
    fn test_mount_option_locked() {
        let mut store = BuildKitCacheStore::new("app");
        let cache = Cache::new_locked("/var/cache/apt");
        let opt = store.get_cache_mount_option("apt", &cache);
        assert_eq!(
            opt,
            "--mount=type=cache,target=/var/cache/apt,id=app-apt,sharing=locked"
        );
    }
}
