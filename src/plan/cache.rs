/// Cache —— 构建缓存定义
///
/// 对齐 railpack `core/plan/cache.go`。
/// 支持 shared（共享）和 locked（互斥）两种模式。
use serde::{Deserialize, Serialize};

/// 缓存类型
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CacheType {
    #[default]
    Shared,
    Locked,
}

/// 缓存定义
///
/// directory 和 cache_type 均为必填字段，避免构造无效缓存（无目录或无类型）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Cache {
    pub directory: String,
    #[serde(rename = "type", default)]
    pub cache_type: CacheType,
}

impl Cache {
    /// 创建新缓存（默认 shared 类型）
    pub fn new(directory: impl Into<String>) -> Self {
        Self {
            directory: directory.into(),
            cache_type: CacheType::Shared,
        }
    }

    /// 创建 locked 类型缓存
    pub fn new_locked(directory: impl Into<String>) -> Self {
        Self {
            directory: directory.into(),
            cache_type: CacheType::Locked,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_new_defaults_to_shared() {
        let cache = Cache::new("/var/cache/apt");
        assert_eq!(cache.cache_type, CacheType::Shared);
        assert_eq!(cache.directory, "/var/cache/apt");
    }

    #[test]
    fn test_cache_new_locked() {
        let cache = Cache::new_locked("/app/target");
        assert_eq!(cache.cache_type, CacheType::Locked);
    }

    #[test]
    fn test_cache_type_serializes_lowercase() {
        let shared = serde_json::to_string(&CacheType::Shared).unwrap();
        assert_eq!(shared, r#""shared""#);
        let locked = serde_json::to_string(&CacheType::Locked).unwrap();
        assert_eq!(locked, r#""locked""#);
    }

    #[test]
    fn test_cache_type_json_roundtrip() {
        let shared: CacheType = serde_json::from_str(r#""shared""#).unwrap();
        assert_eq!(shared, CacheType::Shared);
        let locked: CacheType = serde_json::from_str(r#""locked""#).unwrap();
        assert_eq!(locked, CacheType::Locked);
    }

    #[test]
    fn test_cache_json_roundtrip() {
        let cache = Cache::new("/root/.npm");
        let json = serde_json::to_string(&cache).unwrap();
        let parsed: Cache = serde_json::from_str(&json).unwrap();
        assert_eq!(cache, parsed);
        // 验证 type 字段名（非 cache_type）
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value.get("type").unwrap(), "shared");
        assert!(value.get("cacheType").is_none());
    }
}
