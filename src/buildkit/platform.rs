use std::fmt;

/// 平台信息
///
/// 对齐 railpack `platform.go`
#[derive(Debug, Clone)]
pub struct Platform {
    pub os: String,
    pub arch: String,
    pub variant: Option<String>,
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.variant {
            Some(v) => write!(f, "{}/{}/{}", self.os, self.arch, v),
            None => write!(f, "{}/{}", self.os, self.arch),
        }
    }
}

impl Default for Platform {
    fn default() -> Self {
        detect_host_platform()
    }
}

/// 解析平台字符串，空字符串默认宿主架构
///
/// 对齐 railpack `ParsePlatformWithDefaults()`
pub fn parse_platform_with_defaults(platform_str: &str) -> crate::Result<Platform> {
    if platform_str.is_empty() {
        return Ok(detect_host_platform());
    }
    parse_platform(platform_str)
}

/// 检测宿主机平台
fn detect_host_platform() -> Platform {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        "x86" => "386",
        other => other,
    };

    let variant = match arch {
        "arm64" => Some("v8".to_string()),
        _ => None,
    };

    Platform {
        os: "linux".to_string(),
        arch: arch.to_string(),
        variant,
    }
}

/// 解析平台字符串
fn parse_platform(s: &str) -> crate::Result<Platform> {
    let parts: Vec<&str> = s.split('/').collect();
    match parts.len() {
        2 => Ok(Platform {
            os: parts[0].to_string(),
            arch: parts[1].to_string(),
            variant: None,
        }),
        3 => Ok(Platform {
            os: parts[0].to_string(),
            arch: parts[1].to_string(),
            variant: Some(parts[2].to_string()),
        }),
        _ => Err(anyhow::anyhow!(
            "invalid platform format: '{}', expected 'os/arch' or 'os/arch/variant'",
            s
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_platform_linux_amd64() {
        let p = parse_platform("linux/amd64").unwrap();
        assert_eq!(p.os, "linux");
        assert_eq!(p.arch, "amd64");
        assert!(p.variant.is_none());
    }

    #[test]
    fn test_parse_platform_linux_arm64_v8() {
        let p = parse_platform("linux/arm64/v8").unwrap();
        assert_eq!(p.os, "linux");
        assert_eq!(p.arch, "arm64");
        assert_eq!(p.variant, Some("v8".to_string()));
    }

    #[test]
    fn test_parse_platform_invalid() {
        let result = parse_platform("invalid");
        assert!(result.is_err(), "单段字符串应返回错误");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("invalid platform format"),
            "错误信息应包含 'invalid platform format'，实际: {}",
            err_msg
        );
    }

    #[test]
    fn test_parse_platform_defaults_not_empty() {
        let p = parse_platform_with_defaults("").unwrap();
        assert_eq!(p.os, "linux", "默认 os 应为 linux");
        assert!(!p.arch.is_empty(), "默认 arch 不应为空");
    }

    #[test]
    fn test_platform_to_string() {
        let p = Platform {
            os: "linux".to_string(),
            arch: "amd64".to_string(),
            variant: None,
        };
        assert_eq!(p.to_string(), "linux/amd64");

        let p_with_variant = Platform {
            os: "linux".to_string(),
            arch: "arm64".to_string(),
            variant: Some("v8".to_string()),
        };
        assert_eq!(p_with_variant.to_string(), "linux/arm64/v8");
    }
}
