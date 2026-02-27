use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use tonic::transport::Channel;

use crate::buildkit::proto::control::{
    control_client::ControlClient, Exporter, SolveRequest, SolveResponse,
};
use crate::buildkit::proto::pb;

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

/// Solve RPC 结果
pub struct SolveResult {
    pub exporter_response: HashMap<String, String>,
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

    Ok(SolveRequest {
        r#ref: generate_solve_ref(),
        definition: Some(config.definition.clone()),
        session,
        frontend_attrs: config.frontend_attrs.clone(),
        exporters: vec![exporter],
        ..Default::default()
    })
}

/// 发送 Solve RPC 到 buildkitd
///
/// 接受已构造的 SolveRequest，避免重复生成 ref 导致
/// 进度订阅 ref 与实际构建 ref 不一致。
pub async fn solve(
    client: &mut ControlClient<Channel>,
    request: SolveRequest,
) -> Result<SolveResult> {
    let response = client
        .solve(request)
        .await
        .context("Solve RPC failed")?
        .into_inner();

    Ok(parse_solve_response(response))
}

fn parse_solve_response(resp: SolveResponse) -> SolveResult {
    SolveResult {
        exporter_response: resp.exporter_response,
    }
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
        };

        let req = build_solve_request(&config).unwrap();
        assert!(!req.r#ref.is_empty());
        assert!(req.r#ref.starts_with("arcpack-"));
    }

    #[test]
    fn test_parse_solve_response() {
        let mut exporter_response = HashMap::new();
        exporter_response.insert(
            "containerimage.digest".to_string(),
            "sha256:abc123".to_string(),
        );

        let resp = SolveResponse { exporter_response };
        let result = parse_solve_response(resp);

        assert_eq!(
            result.exporter_response.get("containerimage.digest").unwrap(),
            "sha256:abc123"
        );
    }
}
