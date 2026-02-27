use std::collections::HashMap;

use tonic::{Request, Response, Status};

use crate::buildkit::proto::secrets::{
    secrets_server, GetSecretRequest, GetSecretResponse,
};

/// Secret 提供者——响应 buildkitd 通过 Session 回调的 Secret 请求
///
/// 对齐 Go `secretsprovider.FromMap(secretsMap)`
pub struct SecretsProvider {
    secrets: HashMap<String, String>,
}

impl SecretsProvider {
    pub fn new(secrets: HashMap<String, String>) -> Self {
        Self { secrets }
    }
}

#[tonic::async_trait]
impl secrets_server::Secrets for SecretsProvider {
    async fn get_secret(
        &self,
        request: Request<GetSecretRequest>,
    ) -> Result<Response<GetSecretResponse>, Status> {
        let id = &request.get_ref().id;
        match self.secrets.get(id) {
            Some(value) => Ok(Response::new(GetSecretResponse {
                data: value.as_bytes().to_vec(),
            })),
            None => Err(Status::not_found(format!("secret not found: {id}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider(pairs: &[(&str, &str)]) -> SecretsProvider {
        let secrets = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        SecretsProvider::new(secrets)
    }

    #[tokio::test]
    async fn test_secrets_get_existing() {
        let provider = make_provider(&[("DB_URL", "postgres://localhost/mydb")]);
        let req = Request::new(GetSecretRequest {
            id: "DB_URL".to_string(),
            annotations: HashMap::new(),
        });

        let resp = secrets_server::Secrets::get_secret(&provider, req)
            .await
            .unwrap();
        assert_eq!(resp.get_ref().data, b"postgres://localhost/mydb");
    }

    #[tokio::test]
    async fn test_secrets_get_missing() {
        let provider = make_provider(&[("DB_URL", "value")]);
        let req = Request::new(GetSecretRequest {
            id: "NOT_EXIST".to_string(),
            annotations: HashMap::new(),
        });

        let err = secrets_server::Secrets::get_secret(&provider, req)
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
        assert!(err.message().contains("NOT_EXIST"));
    }

    #[tokio::test]
    async fn test_secrets_empty_map() {
        let provider = make_provider(&[]);
        let req = Request::new(GetSecretRequest {
            id: "ANY_KEY".to_string(),
            annotations: HashMap::new(),
        });

        let err = secrets_server::Secrets::get_secret(&provider, req)
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn test_secrets_value_as_bytes() {
        // 验证含 Unicode 的 secret 正确编码为 UTF-8 bytes
        let provider = make_provider(&[("TOKEN", "密钥abc123")]);
        let req = Request::new(GetSecretRequest {
            id: "TOKEN".to_string(),
            annotations: HashMap::new(),
        });

        let resp = secrets_server::Secrets::get_secret(&provider, req)
            .await
            .unwrap();
        let value = String::from_utf8(resp.get_ref().data.clone()).unwrap();
        assert_eq!(value, "密钥abc123");
    }

    #[tokio::test]
    async fn test_secrets_multiple_keys() {
        let provider = make_provider(&[
            ("KEY_A", "value_a"),
            ("KEY_B", "value_b"),
            ("KEY_C", "value_c"),
        ]);

        for (key, expected) in [("KEY_A", "value_a"), ("KEY_B", "value_b"), ("KEY_C", "value_c")] {
            let req = Request::new(GetSecretRequest {
                id: key.to_string(),
                annotations: HashMap::new(),
            });
            let resp = secrets_server::Secrets::get_secret(&provider, req)
                .await
                .unwrap();
            assert_eq!(resp.get_ref().data, expected.as_bytes());
        }
    }
}
