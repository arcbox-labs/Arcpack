pub mod install;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::resolver::VersionResolver;
use crate::Result;

/// mise 支持的配置文件名
pub const MISE_CONFIG_FILES: &[&str] = &[
    "mise.toml",
    ".tool-versions",
    ".python-version",
    ".node-version",
    ".nvmrc",
];

/// mise idiomatic 版本文件支持的工具列表
pub const IDIOMATIC_VERSION_FILE_TOOLS: &str = "python,node,ruby,elixir,go,java,yarn";

/// mise TOML 配置中的工具条目
#[derive(serde::Serialize, serde::Deserialize)]
struct MisePackage {
    version: String,
}

/// mise TOML 配置结构（使用 BTreeMap 保证输出排序稳定）
#[derive(serde::Serialize, serde::Deserialize)]
struct MiseConfig {
    tools: BTreeMap<String, MisePackage>,
}

/// 生成 mise.toml 内容（[tools] 节）
pub fn generate_mise_toml(packages: &HashMap<String, String>) -> Result<String> {
    let config = MiseConfig {
        tools: packages
            .iter()
            .map(|(name, version)| {
                (
                    name.clone(),
                    MisePackage {
                        version: version.clone(),
                    },
                )
            })
            .collect(),
    };

    let toml_str = toml::to_string(&config)
        .map_err(|e| anyhow::anyhow!("failed to generate mise.toml: {}", e))?;

    Ok(toml_str)
}

/// Mise 工具封装（调用宿主机 mise 二进制）
pub struct Mise {
    binary_path: PathBuf,
    cache_dir: PathBuf,
    github_token: Option<String>,
}

impl Mise {
    /// 创建 Mise 实例，确保 mise 已安装
    pub fn new(cache_dir: &str) -> Result<Self> {
        let binary_path = ensure_installed(cache_dir)?;
        let github_token = std::env::var("GITHUB_TOKEN").ok();

        Ok(Self {
            binary_path,
            cache_dir: PathBuf::from(cache_dir),
            github_token,
        })
    }

    /// 执行 mise 命令并附加环境变量
    fn run_cmd_with_env(&self, extra_env: &[(&str, &str)], args: &[&str]) -> Result<String> {
        let cache_dir = self.cache_dir.join("cache");
        let data_dir = self.cache_dir.join("data");
        let state_dir = self.cache_dir.join("state");
        let system_dir = self.cache_dir.join("system");

        let mut cmd = Command::new(&self.binary_path);
        cmd.args(args);

        // 隔离环境，避免宿主机配置干扰
        cmd.env_clear();
        cmd.env("HOME", &self.cache_dir);
        cmd.env("MISE_CACHE_DIR", &cache_dir);
        cmd.env("MISE_DATA_DIR", &data_dir);
        cmd.env("MISE_STATE_DIR", &state_dir);
        cmd.env("MISE_SYSTEM_DIR", &system_dir);
        cmd.env("MISE_HTTP_TIMEOUT", "60s");
        cmd.env("MISE_FETCH_REMOTE_VERSIONS_TIMEOUT", "60s");
        cmd.env("MISE_HTTP_RETRIES", "5");

        // 防止读取宿主配置（对齐 railpack mise.go 的隔离策略）
        cmd.env("MISE_PARANOID", "1");
        cmd.env("MISE_NO_CONFIG", "1");

        // 防止向上查找 .tool-versions（设置为 cache_dir 的父目录）
        if let Some(parent) = self.cache_dir.parent() {
            cmd.env("MISE_CEILING_PATHS", parent);
        }

        // 继承 PATH
        if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }

        if let Some(ref token) = self.github_token {
            cmd.env("GITHUB_TOKEN", token);
        }

        for (key, value) in extra_env {
            cmd.env(key, value);
        }

        let output = cmd.output().map_err(|e| {
            anyhow::anyhow!(
                "failed to run mise command '{}': {}",
                args.join(" "),
                e
            )
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(anyhow::anyhow!(
                "mise command '{}' failed:\n{}\n{}",
                args.join(" "),
                stdout,
                stderr
            )
            .into());
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

impl VersionResolver for Mise {
    fn get_latest_version(&self, pkg: &str, version: &str) -> Result<String> {
        let query = format!("{}@{}", pkg, version);
        let output = self.run_cmd_with_env(
            &[("MISE_NO_CONFIG", "1"), ("MISE_PARANOID", "1")],
            &["latest", &query],
        )?;

        let latest = output.trim().to_string();
        if latest.is_empty() {
            return Err(
                anyhow::anyhow!("failed to resolve version {} of {}", version, pkg).into(),
            );
        }

        Ok(latest)
    }

    fn get_all_versions(&self, pkg: &str, version: &str) -> Result<Vec<String>> {
        let query = format!("{}@{}", pkg, version);
        let output = self.run_cmd_with_env(
            &[("MISE_NO_CONFIG", "1"), ("MISE_PARANOID", "1")],
            &["ls-remote", &query],
        )?;

        let versions: Vec<String> = output
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|v| !v.is_empty() && !v.contains("RC"))
            .collect();

        if versions.is_empty() {
            return Err(
                anyhow::anyhow!("failed to resolve version {} of {}", version, pkg).into(),
            );
        }

        Ok(versions)
    }
}

/// 确保 mise 二进制已安装（返回二进制路径）
fn ensure_installed(cache_dir: &str) -> Result<PathBuf> {
    // 先检查 mise 是否在 PATH 中
    if let Ok(output) = Command::new("which").arg("mise").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
    }

    // 检查缓存目录中是否有 mise 二进制
    let bin_path = Path::new(cache_dir).join("bin").join("mise");
    if bin_path.exists() {
        return Ok(bin_path);
    }

    Err(anyhow::anyhow!(
        "mise binary not found. Please install mise: https://mise.jdx.dev/getting-started.html"
    )
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_mise_toml_single_package() {
        let mut packages = HashMap::new();
        packages.insert("node".to_string(), "22".to_string());

        let toml_str = generate_mise_toml(&packages).unwrap();
        assert!(toml_str.contains("[tools.node]"));
        assert!(toml_str.contains("version = \"22\""));
    }

    #[test]
    fn test_generate_mise_toml_multiple_packages() {
        let mut packages = HashMap::new();
        packages.insert("node".to_string(), "22.0.0".to_string());
        packages.insert("pnpm".to_string(), "9.0.0".to_string());

        let toml_str = generate_mise_toml(&packages).unwrap();
        assert!(toml_str.contains("[tools.node]"));
        assert!(toml_str.contains("[tools.pnpm]"));
    }

    #[test]
    fn test_generate_mise_toml_empty_packages() {
        let packages = HashMap::new();
        let toml_str = generate_mise_toml(&packages).unwrap();
        assert!(toml_str.contains("[tools]"));
    }

    #[test]
    fn test_generate_mise_toml_valid_toml() {
        let mut packages = HashMap::new();
        packages.insert("node".to_string(), "22".to_string());

        let toml_str = generate_mise_toml(&packages).unwrap();
        // 验证输出是合法 TOML
        let parsed: toml::Value = toml::from_str(&toml_str).unwrap();
        let tools = parsed.get("tools").unwrap().as_table().unwrap();
        let node = tools.get("node").unwrap().as_table().unwrap();
        assert_eq!(node.get("version").unwrap().as_str().unwrap(), "22");
    }

    #[test]
    #[ignore] // 需要 mise 二进制
    fn test_mise_get_latest_version() {
        let mise = Mise::new("/tmp/arcpack/mise-test").unwrap();
        let version = mise.get_latest_version("node", "22").unwrap();
        assert!(version.starts_with("22."));
    }
}
