/// Config 模块 —— arcpack.json 配置加载与合并
///
/// 对齐 railpack `core/config/config.go` + `core/core.go` 中的配置加载逻辑。
/// 支持从文件、环境变量加载并合并配置。
use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::app::{App, Environment};
use crate::plan::{Cache, Command, Filter, Layer, Step};
use crate::Result;

/// 默认配置文件名
const DEFAULT_CONFIG_FILE: &str = "arcpack.json";

/// JSON Schema URL
pub const SCHEMA_URL: &str = "https://schema.arcpack.dev";

/// 部署配置
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeployConfig {
    /// 运行时额外 apt 包
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apt_packages: Vec<String>,

    /// 运行时基础镜像
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<Layer>,

    /// deploy 输入层
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Layer>,

    /// 启动命令
    #[serde(rename = "startCommand", skip_serializing_if = "Option::is_none")]
    pub start_cmd: Option<String>,

    /// 运行时环境变量
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub variables: HashMap<String, String>,

    /// 运行时 PATH 条目
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

/// 步骤配置覆盖
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StepConfig {
    /// 内嵌 Step 的字段
    #[serde(flatten)]
    pub step: Step,

    /// 部署输出过滤
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deploy_outputs: Vec<Filter>,
}

/// 顶层配置（对应 arcpack.json）
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// 强制指定 Provider（跳过自动检测）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    /// 构建阶段额外 apt 包
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build_apt_packages: Vec<String>,

    /// 步骤配置覆盖（步骤名 → 覆盖配置）
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub steps: HashMap<String, StepConfig>,

    /// 部署配置覆盖
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deploy: Option<DeployConfig>,

    /// mise 包版本覆盖（包名 → 版本）
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub packages: HashMap<String, String>,

    /// 额外缓存定义
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub caches: HashMap<String, Cache>,

    /// Secret 引用
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
}

impl Config {
    /// 创建空配置
    pub fn empty() -> Self {
        Self::default()
    }

    /// 从 CLI 选项生成配置
    ///
    /// 对齐 railpack `core/core.go GenerateConfigFromOptions`
    pub fn from_options(
        build_command: &Option<String>,
        start_command: &Option<String>,
    ) -> Self {
        let mut config = Self::empty();

        if let Some(ref cmd) = build_command {
            if !cmd.is_empty() {
                let step_config = StepConfig {
                    step: Step {
                        commands: vec![
                            Command::new_copy(".", "."),
                            Command::new_exec_shell(cmd),
                        ],
                        ..Step::new("build")
                    },
                    ..Default::default()
                };
                config.steps.insert("build".to_string(), step_config);
            }
        }

        if let Some(ref cmd) = start_command {
            if !cmd.is_empty() {
                config
                    .deploy
                    .get_or_insert_with(DeployConfig::default)
                    .start_cmd = Some(cmd.clone());
            }
        }

        config
    }

    /// 从 App + Environment + 可选配置文件路径加载配置
    ///
    /// 三路合并：options < env < file（文件优先级最高）。
    /// 对齐 railpack `core/core.go GetConfig`。
    /// 缺失配置文件返回默认空 Config。
    pub fn load(
        app: &App,
        env: &Environment,
        options_config: Self,
        config_file_path: &Option<String>,
    ) -> Result<Self> {
        let file_config = Self::load_from_file(app, config_file_path)?;
        let env_config = Self::load_from_environment(env);

        // 三路合并：options < env < file
        let merged = Self::merge(&options_config, &env_config);
        Ok(Self::merge(&merged, &file_config))
    }

    /// 从配置文件加载
    ///
    /// 支持自定义配置文件路径。未指定时使用默认 arcpack.json。
    /// 明确指定不存在的配置文件会报错；默认文件不存在返回空 Config。
    fn load_from_file(app: &App, config_file_path: &Option<String>) -> Result<Self> {
        let (file_name, is_custom) = match config_file_path {
            Some(ref path) if !path.is_empty() => (path.as_str(), true),
            _ => (DEFAULT_CONFIG_FILE, false),
        };

        if !app.has_file(file_name) {
            if is_custom {
                return Err(crate::ArcpackError::ConfigParse {
                    path: file_name.to_string(),
                    message: "配置文件不存在".to_string(),
                });
            }
            return Ok(Self::empty());
        }

        app.read_json(file_name)
    }

    /// 从环境变量加载
    ///
    /// 对齐 railpack 的 GenerateConfigFromEnvironment
    fn load_from_environment(env: &Environment) -> Self {
        let mut config = Self::empty();

        // ARCPACK_INSTALL_CMD
        if let (Some(cmd), _) = env.get_config_variable("INSTALL_CMD") {
            let step_config = StepConfig {
                step: Step {
                    commands: vec![
                        Command::new_copy(".", "."),
                        Command::new_exec_shell(&cmd),
                    ],
                    ..Step::new("install")
                },
                ..Default::default()
            };
            config.steps.insert("install".to_string(), step_config);
        }

        // ARCPACK_BUILD_CMD
        if let (Some(cmd), _) = env.get_config_variable("BUILD_CMD") {
            let step_config = StepConfig {
                step: Step {
                    commands: vec![
                        Command::new_copy(".", "."),
                        Command::new_exec_shell(&cmd),
                    ],
                    ..Step::new("build")
                },
                ..Default::default()
            };
            config.steps.insert("build".to_string(), step_config);
        }

        // ARCPACK_START_CMD
        if let (Some(cmd), _) = env.get_config_variable("START_CMD") {
            config
                .deploy
                .get_or_insert_with(DeployConfig::default)
                .start_cmd = Some(cmd);
        }

        // ARCPACK_PACKAGES（空格分隔的 "pkg@version" 列表）
        let (packages, _) = env.get_config_variable_list("PACKAGES");
        if !packages.is_empty() {
            for pkg in packages {
                if let Some((name, version)) = pkg.split_once('@') {
                    config.packages.insert(name.to_string(), version.to_string());
                } else {
                    config.packages.insert(pkg, "*".to_string());
                }
            }
        }

        // ARCPACK_BUILD_APT_PACKAGES
        let (apt_pkgs, _) = env.get_config_variable_list("BUILD_APT_PACKAGES");
        if !apt_pkgs.is_empty() {
            config.build_apt_packages = apt_pkgs;
        }

        // ARCPACK_DEPLOY_APT_PACKAGES
        let (deploy_apt, _) = env.get_config_variable_list("DEPLOY_APT_PACKAGES");
        if !deploy_apt.is_empty() {
            config
                .deploy
                .get_or_insert_with(DeployConfig::default)
                .apt_packages = deploy_apt;
        }

        config
    }

    /// 合并两个配置（right 优先级高于 left）
    ///
    /// deploy 做字段级合并，避免 right 只设了 start_cmd 就覆盖 left 的全部 deploy 字段。
    fn merge(left: &Config, right: &Config) -> Config {
        let mut result = left.clone();

        if right.provider.is_some() {
            result.provider = right.provider.clone();
        }

        if !right.build_apt_packages.is_empty() {
            result.build_apt_packages = right.build_apt_packages.clone();
        }

        for (name, step) in &right.steps {
            result.steps.insert(name.clone(), step.clone());
        }

        // deploy 字段级合并：仅覆盖 right 中非空的字段
        if let Some(ref right_deploy) = right.deploy {
            let result_deploy = result.deploy.get_or_insert_with(DeployConfig::default);
            if !right_deploy.apt_packages.is_empty() {
                result_deploy.apt_packages = right_deploy.apt_packages.clone();
            }
            if right_deploy.base.is_some() {
                result_deploy.base = right_deploy.base.clone();
            }
            if !right_deploy.inputs.is_empty() {
                result_deploy.inputs = right_deploy.inputs.clone();
            }
            if right_deploy.start_cmd.is_some() {
                result_deploy.start_cmd = right_deploy.start_cmd.clone();
            }
            if !right_deploy.variables.is_empty() {
                for (k, v) in &right_deploy.variables {
                    result_deploy.variables.insert(k.clone(), v.clone());
                }
            }
            if !right_deploy.paths.is_empty() {
                result_deploy.paths = right_deploy.paths.clone();
            }
        }

        for (name, version) in &right.packages {
            result.packages.insert(name.clone(), version.clone());
        }

        for (name, cache) in &right.caches {
            result.caches.insert(name.clone(), cache.clone());
        }

        if !right.secrets.is_empty() {
            result.secrets = right.secrets.clone();
        }

        result
    }

    /// 获取或创建步骤配置
    pub fn get_or_create_step(&mut self, name: &str) -> &mut StepConfig {
        self.steps
            .entry(name.to_string())
            .or_insert_with(|| StepConfig {
                step: Step::new(name),
                ..Default::default()
            })
    }

    /// 生成 JSON Schema
    pub fn json_schema() -> schemars::schema::RootSchema {
        schemars::schema_for!(Config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArcpackError;
    use tempfile::TempDir;

    #[test]
    fn test_load_valid_json() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("arcpack.json"),
            r#"{
                "provider": "node",
                "buildAptPackages": ["curl", "git"],
                "packages": {"node": "20"},
                "deploy": {
                    "startCommand": "npm start"
                }
            }"#,
        )
        .unwrap();

        let app = App::new(dir.path()).unwrap();
        let env = Environment::default();
        let config = Config::load(&app, &env, Config::empty(), &None).unwrap();

        assert_eq!(config.provider, Some("node".to_string()));
        assert_eq!(config.build_apt_packages, vec!["curl", "git"]);
        assert_eq!(config.packages.get("node"), Some(&"20".to_string()));
        assert_eq!(
            config.deploy.as_ref().unwrap().start_cmd,
            Some("npm start".to_string())
        );
    }

    #[test]
    fn test_load_missing_file_returns_default() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path()).unwrap();
        let env = Environment::default();
        let config = Config::load(&app, &env, Config::empty(), &None).unwrap();

        assert_eq!(config, Config::empty());
    }

    #[test]
    fn test_load_invalid_json_returns_config_parse_error() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("arcpack.json"), "{ invalid json }").unwrap();

        let app = App::new(dir.path()).unwrap();
        let env = Environment::default();
        let result = Config::load(&app, &env, Config::empty(), &None);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ArcpackError::ConfigParse { .. }
        ));
    }

    #[test]
    fn test_json_schema_is_non_empty() {
        let schema = Config::json_schema();
        let json = serde_json::to_string_pretty(&schema).unwrap();
        assert!(!json.is_empty());
        // 应包含关键字段定义
        assert!(json.contains("provider"));
        assert!(json.contains("buildAptPackages"));
    }

    #[test]
    fn test_camel_case_field_names_in_json() {
        let mut config = Config::empty();
        config.build_apt_packages = vec!["curl".to_string()];
        config.deploy = Some(DeployConfig {
            start_cmd: Some("npm start".to_string()),
            apt_packages: vec!["ca-certificates".to_string()],
            ..Default::default()
        });

        let json = serde_json::to_string(&config).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        // 验证 camelCase
        assert!(value.get("buildAptPackages").is_some());
        assert!(value.get("build_apt_packages").is_none());

        let deploy = value.get("deploy").unwrap();
        assert!(deploy.get("startCommand").is_some());
        assert!(deploy.get("start_cmd").is_none());
        assert!(deploy.get("aptPackages").is_some());
    }

    #[test]
    fn test_env_config_start_cmd_override() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path()).unwrap();

        let mut env = Environment::default();
        env.set_variable("ARCPACK_START_CMD", "node server.js");

        let config = Config::load(&app, &env, Config::empty(), &None).unwrap();
        assert_eq!(
            config.deploy.as_ref().unwrap().start_cmd,
            Some("node server.js".to_string())
        );
    }

    #[test]
    fn test_get_or_create_step() {
        let mut config = Config::empty();
        {
            let step = config.get_or_create_step("install");
            step.step.commands.push(Command::new_exec("npm install"));
        }

        assert!(config.steps.contains_key("install"));
        assert_eq!(config.steps["install"].step.commands.len(), 1);
    }

    #[test]
    fn test_merge_deploy_field_level_preserves_file_fields() {
        // 文件配置：deploy 有 base + inputs
        // 环境变量：仅设 ARCPACK_START_CMD
        // 合并后：base/inputs 来自文件，start_cmd 来自环境变量，均不丢失
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("arcpack.json"),
            r#"{
                "deploy": {
                    "base": { "image": "ubuntu:22.04" },
                    "inputs": [{ "step": "build" }]
                }
            }"#,
        )
        .unwrap();

        let app = App::new(dir.path()).unwrap();
        let mut env = Environment::default();
        env.set_variable("ARCPACK_START_CMD", "node server.js");

        let config = Config::load(&app, &env, Config::empty(), &None).unwrap();
        let deploy = config.deploy.as_ref().expect("deploy should exist");

        // base 来自文件，未被覆盖
        assert_eq!(
            deploy.base.as_ref().unwrap().image.as_deref(),
            Some("ubuntu:22.04")
        );
        // inputs 来自文件，未被覆盖
        assert_eq!(deploy.inputs.len(), 1);
        assert_eq!(
            deploy.inputs[0].step.as_deref(),
            Some("build")
        );
        // start_cmd 来自环境变量
        assert_eq!(
            deploy.start_cmd.as_deref(),
            Some("node server.js")
        );
    }

    #[test]
    fn test_config_json_roundtrip() {
        let mut config = Config::empty();
        config.provider = Some("node".to_string());
        config.build_apt_packages = vec!["curl".to_string()];
        config.packages.insert("node".to_string(), "20".to_string());
        config
            .caches
            .insert("npm".to_string(), Cache::new("/root/.npm"));
        config.deploy = Some(DeployConfig {
            start_cmd: Some("npm start".to_string()),
            ..Default::default()
        });

        let json = serde_json::to_string_pretty(&config).unwrap();
        let parsed: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn test_from_options_build_command() {
        let config = Config::from_options(
            &Some("make build".to_string()),
            &None,
        );
        assert!(config.steps.contains_key("build"));
        let build_step = &config.steps["build"];
        assert_eq!(build_step.step.commands.len(), 2);
    }

    #[test]
    fn test_from_options_start_command() {
        let config = Config::from_options(
            &None,
            &Some("node server.js".to_string()),
        );
        assert_eq!(
            config.deploy.as_ref().unwrap().start_cmd,
            Some("node server.js".to_string())
        );
    }

    #[test]
    fn test_from_options_empty_strings_ignored() {
        let config = Config::from_options(
            &Some("".to_string()),
            &Some("".to_string()),
        );
        assert!(config.steps.is_empty());
        assert!(config.deploy.is_none());
    }

    #[test]
    fn test_load_custom_config_file_path() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("custom.json"),
            r#"{ "provider": "node" }"#,
        )
        .unwrap();

        let app = App::new(dir.path()).unwrap();
        let env = Environment::default();
        let config = Config::load(
            &app,
            &env,
            Config::empty(),
            &Some("custom.json".to_string()),
        )
        .unwrap();

        assert_eq!(config.provider, Some("node".to_string()));
    }

    #[test]
    fn test_load_custom_config_file_missing_returns_error() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path()).unwrap();
        let env = Environment::default();

        let result = Config::load(
            &app,
            &env,
            Config::empty(),
            &Some("nonexistent.json".to_string()),
        );

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ArcpackError::ConfigParse { .. }
        ));
    }

    #[test]
    fn test_three_way_merge_priority() {
        // options 设置 start_command，file 设置 provider
        // 结果应包含两者（file > env > options）
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("arcpack.json"),
            r#"{ "provider": "node" }"#,
        )
        .unwrap();

        let app = App::new(dir.path()).unwrap();
        let env = Environment::default();
        let options_config = Config::from_options(
            &None,
            &Some("node server.js".to_string()),
        );

        let config = Config::load(&app, &env, options_config, &None).unwrap();

        // provider 来自文件
        assert_eq!(config.provider, Some("node".to_string()));
        // start_cmd 来自 options（文件未设置，不会覆盖）
        assert_eq!(
            config.deploy.as_ref().unwrap().start_cmd,
            Some("node server.js".to_string())
        );
    }

    #[test]
    fn test_file_config_overrides_options() {
        // options 和 file 都设置 start_command，file 胜出
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("arcpack.json"),
            r#"{ "deploy": { "startCommand": "npm start" } }"#,
        )
        .unwrap();

        let app = App::new(dir.path()).unwrap();
        let env = Environment::default();
        let options_config = Config::from_options(
            &None,
            &Some("node server.js".to_string()),
        );

        let config = Config::load(&app, &env, options_config, &None).unwrap();

        // file 优先级更高
        assert_eq!(
            config.deploy.as_ref().unwrap().start_cmd,
            Some("npm start".to_string())
        );
    }
}
