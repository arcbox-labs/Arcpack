use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use tracing::warn;

use crate::buildkit::proto::control::{self, Exporter, SolveRequest};
use crate::buildkit::proto::pb;

/// 缓存导入/导出配置（如 type=gha,url=...,token=...）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CacheConfig {
    /// 缓存类型（registry / local / gha / s3 等）
    pub cache_type: String,
    /// 附加属性（mode, url, token 等）
    pub attrs: HashMap<String, String>,
}

impl CacheConfig {
    /// 解析缓存配置字符串
    ///
    /// 格式：`type=gha,url=...,token=...`（逗号分隔的 key=value）
    /// `type` 键提取为 `cache_type`，其余进入 `attrs`。
    /// 对齐 railpack `parseKeyValue()` in `buildkit/build.go`。
    pub fn parse(s: &str) -> Self {
        let mut cache_type = String::new();
        let mut attrs = HashMap::new();

        for part in s.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some((key, value)) = part.split_once('=') {
                if key == "type" {
                    cache_type = value.to_string();
                } else {
                    attrs.insert(key.to_string(), value.to_string());
                }
            } else {
                warn!(entry = part, "忽略格式错误的缓存配置条目（缺少 '='）");
            }
        }

        Self { cache_type, attrs }
    }

    /// 转换为 protobuf CacheOptionsEntry
    fn to_entry(&self) -> control::CacheOptionsEntry {
        control::CacheOptionsEntry {
            r#type: self.cache_type.clone(),
            attrs: self.attrs.clone(),
        }
    }
}

/// Solve RPC 配置
pub struct SolveConfig {
    /// LLB Definition（DAG 序列化结果）
    pub definition: pb::Definition,
    /// 输出策略
    pub exporter: ExportConfig,
    /// Session ID（关联 filesync/secrets provider）
    pub session_id: Option<String>,
    /// 前端属性（如 containerimage.config）
    pub frontend_attrs: HashMap<String, String>,
    /// 缓存导入配置
    pub cache_imports: Vec<CacheConfig>,
    /// 缓存导出配置
    pub cache_exports: Vec<CacheConfig>,
}

/// 输出策略：镜像 / 本地目录 / Docker tar
#[derive(Debug, Clone)]
pub enum ExportConfig {
    /// 输出为 OCI 镜像
    Image { name: String, push: bool },
    /// 输出到本地目录
    Local { dest: PathBuf },
    /// 输出为 Docker tar（docker load 兼容）
    DockerTar { name: String, dest: PathBuf },
}

/// 构造 SolveRequest protobuf 消息（纯函数，便于测试）
pub fn build_solve_request(config: &SolveConfig) -> Result<SolveRequest> {
    let (exporter_type, exporter_attrs) = match &config.exporter {
        ExportConfig::Image { name, push } => {
            let mut attrs = HashMap::new();
            attrs.insert("name".to_string(), name.clone());
            attrs.insert("push".to_string(), push.to_string());
            ("image".to_string(), attrs)
        }
        ExportConfig::Local { dest } => {
            let mut attrs = HashMap::new();
            attrs.insert(
                "dest".to_string(),
                dest.to_string_lossy().into_owned(),
            );
            ("local".to_string(), attrs)
        }
        ExportConfig::DockerTar { name, dest } => {
            let mut attrs = HashMap::new();
            attrs.insert("name".to_string(), name.clone());
            attrs.insert(
                "dest".to_string(),
                dest.to_string_lossy().into_owned(),
            );
            ("docker".to_string(), attrs)
        }
    };

    // 使用新版 Exporters 字段（非 deprecated 字段）
    let exporter = Exporter {
        r#type: exporter_type,
        attrs: exporter_attrs,
    };

    let session = config.session_id.clone().unwrap_or_default();

    // 缓存配置
    let cache = if config.cache_imports.is_empty() && config.cache_exports.is_empty() {
        None
    } else {
        Some(control::CacheOptions {
            imports: config.cache_imports.iter().map(CacheConfig::to_entry).collect(),
            exports: config.cache_exports.iter().map(CacheConfig::to_entry).collect(),
            ..Default::default()
        })
    };

    Ok(SolveRequest {
        r#ref: generate_solve_ref(),
        definition: Some(config.definition.clone()),
        session,
        frontend_attrs: config.frontend_attrs.clone(),
        exporters: vec![exporter],
        cache,
        ..Default::default()
    })
}

/// 生成随机 Solve ref（用于关联 Status 流）
fn generate_solve_ref() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("arcpack-{ts:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_definition() -> pb::Definition {
        pb::Definition::default()
    }

    #[test]
    fn test_build_solve_request_image_export() {
        let config = SolveConfig {
            definition: empty_definition(),
            exporter: ExportConfig::Image {
                name: "myapp:latest".to_string(),
                push: true,
            },
            session_id: Some("sess-123".to_string()),
            frontend_attrs: HashMap::new(),
            cache_imports: vec![],
            cache_exports: vec![],
        };

        let req = build_solve_request(&config).unwrap();
        assert_eq!(req.exporters.len(), 1);
        assert_eq!(req.exporters[0].r#type, "image");
        assert_eq!(req.exporters[0].attrs.get("name").unwrap(), "myapp:latest");
        assert_eq!(req.exporters[0].attrs.get("push").unwrap(), "true");
        assert_eq!(req.session, "sess-123");
    }

    #[test]
    fn test_build_solve_request_image_no_push() {
        let config = SolveConfig {
            definition: empty_definition(),
            exporter: ExportConfig::Image {
                name: "myapp:v1".to_string(),
                push: false,
            },
            session_id: None,
            frontend_attrs: HashMap::new(),
            cache_imports: vec![],
            cache_exports: vec![],
        };

        let req = build_solve_request(&config).unwrap();
        assert_eq!(req.exporters[0].attrs.get("push").unwrap(), "false");
    }

    #[test]
    fn test_build_solve_request_local_export() {
        let config = SolveConfig {
            definition: empty_definition(),
            exporter: ExportConfig::Local {
                dest: PathBuf::from("/tmp/output"),
            },
            session_id: None,
            frontend_attrs: HashMap::new(),
            cache_imports: vec![],
            cache_exports: vec![],
        };

        let req = build_solve_request(&config).unwrap();
        assert_eq!(req.exporters.len(), 1);
        assert_eq!(req.exporters[0].r#type, "local");
        assert_eq!(req.exporters[0].attrs.get("dest").unwrap(), "/tmp/output");
    }

    #[test]
    fn test_build_solve_request_docker_tar() {
        let config = SolveConfig {
            definition: empty_definition(),
            exporter: ExportConfig::DockerTar {
                name: "myapp:latest".to_string(),
                dest: PathBuf::from("/tmp/myapp.tar"),
            },
            session_id: Some("sess-456".to_string()),
            frontend_attrs: HashMap::new(),
            cache_imports: vec![],
            cache_exports: vec![],
        };

        let req = build_solve_request(&config).unwrap();
        assert_eq!(req.exporters.len(), 1);
        assert_eq!(req.exporters[0].r#type, "docker");
        assert_eq!(req.exporters[0].attrs.get("name").unwrap(), "myapp:latest");
        assert_eq!(
            req.exporters[0].attrs.get("dest").unwrap(),
            "/tmp/myapp.tar"
        );
        assert_eq!(req.session, "sess-456");
    }

    #[test]
    fn test_build_solve_request_session_id() {
        let config = SolveConfig {
            definition: empty_definition(),
            exporter: ExportConfig::Image {
                name: "test".to_string(),
                push: false,
            },
            session_id: Some("my-session".to_string()),
            frontend_attrs: HashMap::new(),
            cache_imports: vec![],
            cache_exports: vec![],
        };

        let req = build_solve_request(&config).unwrap();
        assert_eq!(req.session, "my-session");
    }

    #[test]
    fn test_build_solve_request_empty_session() {
        let config = SolveConfig {
            definition: empty_definition(),
            exporter: ExportConfig::Image {
                name: "test".to_string(),
                push: false,
            },
            session_id: None,
            frontend_attrs: HashMap::new(),
            cache_imports: vec![],
            cache_exports: vec![],
        };

        let req = build_solve_request(&config).unwrap();
        assert_eq!(req.session, "");
    }

    #[test]
    fn test_build_solve_request_frontend_attrs() {
        let mut attrs = HashMap::new();
        attrs.insert(
            "containerimage.config".to_string(),
            r#"{"Env":["PATH=/usr/bin"]}"#.to_string(),
        );

        let config = SolveConfig {
            definition: empty_definition(),
            exporter: ExportConfig::Image {
                name: "test".to_string(),
                push: false,
            },
            session_id: None,
            frontend_attrs: attrs,
            cache_imports: vec![],
            cache_exports: vec![],
        };

        let req = build_solve_request(&config).unwrap();
        assert_eq!(
            req.frontend_attrs.get("containerimage.config").unwrap(),
            r#"{"Env":["PATH=/usr/bin"]}"#
        );
    }

    #[test]
    fn test_build_solve_request_has_definition() {
        let config = SolveConfig {
            definition: empty_definition(),
            exporter: ExportConfig::Image {
                name: "test".to_string(),
                push: false,
            },
            session_id: None,
            frontend_attrs: HashMap::new(),
            cache_imports: vec![],
            cache_exports: vec![],
        };

        let req = build_solve_request(&config).unwrap();
        assert!(req.definition.is_some());
    }

    #[test]
    fn test_build_solve_request_ref_not_empty() {
        let config = SolveConfig {
            definition: empty_definition(),
            exporter: ExportConfig::Image {
                name: "test".to_string(),
                push: false,
            },
            session_id: None,
            frontend_attrs: HashMap::new(),
            cache_imports: vec![],
            cache_exports: vec![],
        };

        let req = build_solve_request(&config).unwrap();
        assert!(!req.r#ref.is_empty());
        assert!(req.r#ref.starts_with("arcpack-"));
    }

    // === CacheConfig 解析测试 ===

    #[test]
    fn test_parse_cache_config_gha() {
        let c = CacheConfig::parse("type=gha,url=https://example.com,token=abc123");
        assert_eq!(c.cache_type, "gha");
        assert_eq!(c.attrs.get("url").unwrap(), "https://example.com");
        assert_eq!(c.attrs.get("token").unwrap(), "abc123");
    }

    #[test]
    fn test_parse_cache_config_registry() {
        let c = CacheConfig::parse("type=registry,ref=example.com/cache:latest");
        assert_eq!(c.cache_type, "registry");
        assert_eq!(c.attrs.get("ref").unwrap(), "example.com/cache:latest");
    }

    #[test]
    fn test_parse_cache_config_local() {
        let c = CacheConfig::parse("type=local,dest=/tmp/cache");
        assert_eq!(c.cache_type, "local");
        assert_eq!(c.attrs.get("dest").unwrap(), "/tmp/cache");
    }

    #[test]
    fn test_parse_cache_config_empty() {
        let c = CacheConfig::parse("");
        assert!(c.cache_type.is_empty());
        assert!(c.attrs.is_empty());
    }

    #[test]
    fn test_build_solve_request_no_cache() {
        let config = SolveConfig {
            definition: empty_definition(),
            exporter: ExportConfig::Image {
                name: "test".to_string(),
                push: false,
            },
            session_id: None,
            frontend_attrs: HashMap::new(),
            cache_imports: vec![],
            cache_exports: vec![],
        };

        let req = build_solve_request(&config).unwrap();
        assert!(req.cache.is_none());
    }

    #[test]
    fn test_build_solve_request_with_cache() {
        let config = SolveConfig {
            definition: empty_definition(),
            exporter: ExportConfig::Image {
                name: "test".to_string(),
                push: false,
            },
            session_id: None,
            frontend_attrs: HashMap::new(),
            cache_imports: vec![CacheConfig::parse("type=gha,url=https://ex.com")],
            cache_exports: vec![CacheConfig::parse("type=gha,mode=max")],
        };

        let req = build_solve_request(&config).unwrap();
        let cache = req.cache.unwrap();
        assert_eq!(cache.imports.len(), 1);
        assert_eq!(cache.imports[0].r#type, "gha");
        assert_eq!(cache.imports[0].attrs.get("url").unwrap(), "https://ex.com");
        assert_eq!(cache.exports.len(), 1);
        assert_eq!(cache.exports[0].r#type, "gha");
        assert_eq!(cache.exports[0].attrs.get("mode").unwrap(), "max");
    }

}
