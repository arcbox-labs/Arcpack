use std::collections::HashMap;

use super::build_llb::build_env::BuildEnvironment;
use super::platform::Platform;
use crate::plan::Deploy;

/// OCI Image 配置
///
/// 对齐 railpack `image.go`
#[derive(Debug, Clone)]
pub struct ImageConfig {
    pub env: Vec<String>,
    pub working_dir: String,
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    pub platform: Platform,
}

/// 系统默认 PATH
const DEFAULT_SYSTEM_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// 构建 Image 配置
///
/// ENV 合并顺序：
/// 1. deploy.paths — Provider 声明的运行时 PATH
/// 2. graph_env.path_list — 构建过程中累积的 PATH
/// 3. 系统默认 PATH
/// → 拼接、去重为单个 PATH=... 条目
///
/// 4. graph_env.env_vars — 构建过程中累积的变量
/// 5. deploy.variables — Provider 声明的运行时变量
/// → 合并为 KEY=value 列表，排序
/// 向 path_parts 追加条目，跳过已存在项
fn push_unique(path_parts: &mut Vec<String>, items: impl IntoIterator<Item = String>) {
    for p in items {
        if !path_parts.contains(&p) {
            path_parts.push(p);
        }
    }
}

pub fn build_image_config(
    graph_env: &BuildEnvironment,
    deploy: &Deploy,
    platform: &Platform,
) -> ImageConfig {
    let mut env = Vec::new();

    // 构建 PATH（deploy.paths → graph_env.path_list → 系统默认，去重）
    let mut path_parts: Vec<String> = Vec::new();
    push_unique(&mut path_parts, deploy.paths.iter().cloned());
    push_unique(&mut path_parts, graph_env.path_list.iter().cloned());
    push_unique(
        &mut path_parts,
        DEFAULT_SYSTEM_PATH.split(':').map(String::from),
    );

    if !path_parts.is_empty() {
        env.push(format!("PATH={}", path_parts.join(":")));
    }

    // 合并环境变量（graph_env.env_vars + deploy.variables，后者覆盖）
    let mut env_map: HashMap<String, String> = graph_env.env_vars.clone();
    env_map.extend(deploy.variables.clone());

    // 排序后添加
    let mut sorted_keys: Vec<&String> = env_map.keys().collect();
    sorted_keys.sort();
    for key in sorted_keys {
        env.push(format!("{}={}", key, env_map[key]));
    }

    // CMD
    let cmd = match &deploy.start_cmd {
        Some(cmd) => vec![cmd.clone()],
        None => vec!["/bin/bash".to_string()],
    };

    ImageConfig {
        env,
        working_dir: "/app".to_string(),
        entrypoint: vec!["/bin/bash".to_string(), "-c".to_string()],
        cmd,
        platform: platform.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 辅助函数：创建默认 Platform
    fn test_platform() -> Platform {
        Platform {
            os: "linux".to_string(),
            arch: "amd64".to_string(),
            variant: None,
        }
    }

    #[test]
    fn test_build_image_config_path_merge_dedup() {
        let mut graph_env = BuildEnvironment::new();
        graph_env.push_path("/custom/bin");
        graph_env.push_path("/usr/local/bin"); // 与系统默认重复

        let deploy = Deploy {
            paths: vec!["/deploy/bin".to_string(), "/custom/bin".to_string()],
            ..Default::default()
        };

        let config = build_image_config(&graph_env, &deploy, &test_platform());

        // 查找 PATH 条目
        let path_entry = config
            .env
            .iter()
            .find(|e| e.starts_with("PATH="))
            .expect("应包含 PATH 条目");

        // deploy.paths 排在最前
        assert!(
            path_entry.starts_with("PATH=/deploy/bin:"),
            "deploy.paths 应排在最前，实际: {}",
            path_entry
        );

        // /custom/bin 不应重复出现
        let path_value = path_entry.strip_prefix("PATH=").unwrap();
        let parts: Vec<&str> = path_value.split(':').collect();
        let custom_count = parts.iter().filter(|&&p| p == "/custom/bin").count();
        assert_eq!(
            custom_count, 1,
            "/custom/bin 不应重复，实际出现 {} 次",
            custom_count
        );

        // /usr/local/bin 不应重复出现
        let usr_local_count = parts.iter().filter(|&&p| p == "/usr/local/bin").count();
        assert_eq!(
            usr_local_count, 1,
            "/usr/local/bin 不应重复，实际出现 {} 次",
            usr_local_count
        );
    }

    #[test]
    fn test_build_image_config_env_vars_sorted() {
        let mut graph_env = BuildEnvironment::new();
        graph_env.add_env_var("ZZZ_VAR", "z_value");
        graph_env.add_env_var("AAA_VAR", "a_value");

        let deploy = Deploy::default();
        let config = build_image_config(&graph_env, &deploy, &test_platform());

        // 跳过 PATH 条目，收集其余环境变量
        let env_vars: Vec<&String> = config
            .env
            .iter()
            .filter(|e| !e.starts_with("PATH="))
            .collect();

        assert_eq!(env_vars.len(), 2, "应有 2 个非 PATH 环境变量");
        assert!(
            env_vars[0].starts_with("AAA_VAR="),
            "第一个变量应为 AAA_VAR，实际: {}",
            env_vars[0]
        );
        assert!(
            env_vars[1].starts_with("ZZZ_VAR="),
            "第二个变量应为 ZZZ_VAR，实际: {}",
            env_vars[1]
        );
    }

    #[test]
    fn test_build_image_config_no_start_cmd_defaults_bash() {
        let graph_env = BuildEnvironment::new();
        let deploy = Deploy {
            start_cmd: None,
            ..Default::default()
        };

        let config = build_image_config(&graph_env, &deploy, &test_platform());
        assert_eq!(
            config.cmd,
            vec!["/bin/bash"],
            "无 start_cmd 时应默认 /bin/bash"
        );
    }

    #[test]
    fn test_build_image_config_with_start_cmd() {
        let graph_env = BuildEnvironment::new();
        let deploy = Deploy {
            start_cmd: Some("node server.js".to_string()),
            ..Default::default()
        };

        let config = build_image_config(&graph_env, &deploy, &test_platform());
        assert_eq!(config.cmd, vec!["node server.js"]);
        assert_eq!(
            config.entrypoint,
            vec!["/bin/bash", "-c"],
            "entrypoint 应为 [/bin/bash, -c]"
        );
        assert_eq!(config.working_dir, "/app");
    }

    #[test]
    fn test_build_image_config_deploy_vars_override_graph() {
        let mut graph_env = BuildEnvironment::new();
        graph_env.add_env_var("NODE_ENV", "development");
        graph_env.add_env_var("KEEP_ME", "original");

        let mut variables = HashMap::new();
        variables.insert("NODE_ENV".to_string(), "production".to_string());

        let deploy = Deploy {
            variables,
            ..Default::default()
        };

        let config = build_image_config(&graph_env, &deploy, &test_platform());

        // NODE_ENV 应被 deploy 覆盖为 production
        let node_env = config
            .env
            .iter()
            .find(|e| e.starts_with("NODE_ENV="))
            .expect("应包含 NODE_ENV");
        assert_eq!(
            node_env, "NODE_ENV=production",
            "deploy.variables 应覆盖 graph_env 中的同名变量"
        );

        // KEEP_ME 应保持不变
        let keep_me = config
            .env
            .iter()
            .find(|e| e.starts_with("KEEP_ME="))
            .expect("应保留未被覆盖的变量");
        assert_eq!(keep_me, "KEEP_ME=original");
    }
}
