/// test.json 配置解析
///
/// 每个 fixture 目录可包含 test.json 文件，定义集成测试行为。

use std::collections::HashMap;

/// 测试配置
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct TestConfig {
    /// 目标平台（如 "linux/amd64"）
    pub platform: Option<String>,
    /// 预期输出字符串（容器 stdout 中包含）
    pub expected_output: Option<String>,
    /// 额外环境变量
    pub envs: Option<HashMap<String, String>>,
    /// 仅构建，不运行容器
    pub just_build: Option<bool>,
    /// 预期构建失败
    pub should_fail: Option<bool>,
    /// HTTP 健康检查
    pub http_check: Option<HttpCheck>,
}

/// HTTP 健康检查配置
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpCheck {
    /// 检查路径（如 "/"）
    pub path: String,
    /// 预期 HTTP 状态码
    pub expected_status: u16,
    /// 预期响应体包含的字符串
    pub expected_body: Option<String>,
    /// 超时（秒）
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// 重试次数
    #[serde(default = "default_retries")]
    pub retries: u32,
}

fn default_timeout() -> u64 {
    30
}

fn default_retries() -> u32 {
    10
}

impl TestConfig {
    /// 从文件加载
    pub fn load(path: &str) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {}", path, e))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse {}: {}", path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_expected_output() {
        let json = r#"{"expectedOutput": "Hello"}"#;
        let config: TestConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.expected_output, Some("Hello".to_string()));
        assert!(config.just_build.is_none());
    }

    #[test]
    fn test_parse_just_build() {
        let json = r#"{"justBuild": true}"#;
        let config: TestConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.just_build, Some(true));
    }

    #[test]
    fn test_parse_http_check() {
        let json = r#"{"httpCheck": {"path": "/", "expectedStatus": 200}}"#;
        let config: TestConfig = serde_json::from_str(json).unwrap();
        let check = config.http_check.unwrap();
        assert_eq!(check.path, "/");
        assert_eq!(check.expected_status, 200);
        assert_eq!(check.timeout_secs, 30);
        assert_eq!(check.retries, 10);
    }

    #[test]
    fn test_parse_with_envs() {
        let json = r#"{"envs": {"NODE_ENV": "test"}, "justBuild": true}"#;
        let config: TestConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            config.envs.as_ref().unwrap().get("NODE_ENV").unwrap(),
            "test"
        );
    }
}
