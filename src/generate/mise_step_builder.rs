use std::collections::HashMap;

use crate::app::App;
use crate::app::environment::Environment;
use crate::config::Config;
use crate::mise;
use crate::plan::{BuildPlan, Command, Filter, Layer, Step, ARCPACK_BUILDER_IMAGE};
use crate::resolver::{PackageRef, Resolver};
use crate::Result;

use super::BuildStepOptions;

/// mise 步骤名常量
pub const MISE_STEP_NAME: &str = "packages:mise";

/// mise install 命令
const MISE_INSTALL_COMMAND: &str = "mise install";

/// MiseStepBuilder：mise 包安装步骤构建器
///
/// 对齐 railpack `core/generate/mise_step_builder.go`
/// 注意：MiseStepBuilder 不存入 GenerateContext.steps，
/// 而是单独持有于 ctx.mise_step_builder，generate() 时先 build 它再 build 其余 steps
pub struct MiseStepBuilder {
    pub display_name: String,
    pub mise_packages: Vec<PackageRef>,
    pub supporting_apt_packages: Vec<String>,
    pub assets: HashMap<String, String>,
    pub inputs: Vec<Layer>,
    pub variables: HashMap<String, String>,
}

impl MiseStepBuilder {
    pub fn new(display_name: &str, config: &Config) -> Self {
        Self {
            display_name: display_name.to_string(),
            mise_packages: Vec::new(),
            supporting_apt_packages: config.build_apt_packages.clone(),
            assets: HashMap::new(),
            inputs: Vec::new(),
            variables: HashMap::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.display_name
    }

    /// 注册默认包（避免重复注册同名包）
    pub fn default_package(
        &mut self,
        resolver: &mut Resolver,
        name: &str,
        default_version: &str,
    ) -> PackageRef {
        // 检查是否已注册
        for pkg in &self.mise_packages {
            if pkg.name == name {
                return pkg.clone();
            }
        }

        let pkg_ref = resolver.default_package(name, default_version);
        self.mise_packages.push(pkg_ref.clone());
        pkg_ref
    }

    /// 更新包版本
    pub fn version(
        &mut self,
        resolver: &mut Resolver,
        pkg_ref: &PackageRef,
        version: &str,
        source: &str,
    ) {
        resolver.version(pkg_ref, version, source);
    }

    /// 标记跳过 mise 安装
    pub fn skip_mise_install(&self, resolver: &mut Resolver, pkg_ref: &PackageRef) {
        resolver.set_skip_mise_install(pkg_ref, true);
    }

    /// 添加构建时 apt 依赖包
    pub fn add_supporting_apt_package(&mut self, name: &str) {
        self.supporting_apt_packages.push(name.to_string());
    }

    /// 添加输入层
    pub fn add_input(&mut self, input: Layer) {
        self.inputs.push(input);
    }

    /// 获取输出路径列表
    pub fn get_output_paths(&self) -> Vec<String> {
        if self.mise_packages.is_empty() {
            return vec![];
        }
        vec![
            "/mise/shims".to_string(),
            "/mise/installs".to_string(),
            "/usr/local/bin/mise".to_string(),
            "/etc/mise/config.toml".to_string(),
            "/root/.local/state/mise".to_string(),
        ]
    }

    /// 获取输出层引用
    pub fn get_layer(&self) -> Layer {
        let paths = self.get_output_paths();
        if paths.is_empty() {
            return Layer::default();
        }
        Layer::new_step_layer(
            &self.display_name,
            Some(Filter::include_only(paths)),
        )
    }

    /// 构建 mise 步骤到 BuildPlan
    ///
    /// 注意：此方法不实现 StepBuilder trait，因为 MiseStepBuilder 单独于 steps 列表持有，
    /// 需要额外的 resolver/app/env 引用
    pub fn build(
        &self,
        plan: &mut BuildPlan,
        options: &mut BuildStepOptions,
        resolver: &Resolver,
        app: &App,
        env: &Environment,
    ) -> Result<()> {
        let mut base_layer = Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None);

        // 如果有构建时 apt 包，先创建 apt 安装步骤
        if !self.supporting_apt_packages.is_empty() {
            let mut apt_step = Step::new("packages:apt:build");
            apt_step.inputs = vec![base_layer.clone()];
            apt_step.commands = vec![
                BuildStepOptions::new_apt_install_command(&self.supporting_apt_packages),
            ];
            apt_step.caches = options.caches.get_apt_caches();
            apt_step.secrets = vec![];

            plan.add_step(apt_step);
            base_layer = Layer::new_step_layer("packages:apt:build", None);
        }

        let mut step = Step::new(&self.display_name);
        step.inputs = vec![base_layer];

        if !self.mise_packages.is_empty() {
            // 添加 mise shims PATH
            step.commands.push(Command::new_path("/mise/shims"));

            // 设置 mise 环境变量
            step.variables.insert("MISE_DATA_DIR".to_string(), "/mise".to_string());
            step.variables.insert("MISE_CONFIG_DIR".to_string(), "/mise".to_string());
            step.variables.insert("MISE_CACHE_DIR".to_string(), "/mise/cache".to_string());
            step.variables.insert("MISE_SHIMS_DIR".to_string(), "/mise/shims".to_string());
            step.variables.insert("MISE_INSTALLS_DIR".to_string(), "/mise/installs".to_string());
            step.variables.insert("MISE_NODE_VERIFY".to_string(), "false".to_string());
            step.variables.insert("MISE_PARANOID".to_string(), "1".to_string());
            step.variables.insert("MISE_TRUSTED_CONFIG_PATHS".to_string(), "/app".to_string());
            step.variables.insert(
                "MISE_IDIOMATIC_VERSION_FILE_ENABLE_TOOLS".to_string(),
                mise::IDIOMATIC_VERSION_FILE_TOOLS.to_string(),
            );

            // 合并自定义变量
            for (k, v) in &self.variables {
                step.variables.insert(k.clone(), v.clone());
            }

            // 传递 MISE_VERBOSE
            if let Some(verbose) = env.get_variable("MISE_VERBOSE") {
                step.variables.insert("MISE_VERBOSE".to_string(), verbose.to_string());
            }

            // 复制用户 mise 配置文件
            for file in mise::MISE_CONFIG_FILES {
                if app.has_file(file) {
                    step.commands.push(Command::new_copy(*file, *file));
                }
            }

            // 生成 mise.toml asset
            let mut packages_to_install = HashMap::new();
            for pkg in &self.mise_packages {
                if let Some(resolved) = options.resolved_packages.get(&pkg.name) {
                    if let Some(ref version) = resolved.resolved_version {
                        // 检查是否跳过安装
                        if let Some(requested) = resolver.get(&pkg.name) {
                            if !requested.skip_mise_install {
                                packages_to_install.insert(pkg.name.clone(), version.clone());
                            }
                        }
                    }
                }
            }

            let mise_toml = mise::generate_mise_toml(&packages_to_install)?;

            let mut pkg_names: Vec<&String> = packages_to_install.keys().collect();
            pkg_names.sort();
            let pkg_list: Vec<String> = pkg_names.iter().map(|s| s.to_string()).collect();

            step.commands.push(Command::File(crate::plan::command::FileCommand {
                path: "/etc/mise/config.toml".to_string(),
                name: "mise.toml".to_string(),
                mode: None,
                custom_name: Some("create mise config".to_string()),
            }));
            step.commands.push(Command::Exec(crate::plan::command::ExecCommand {
                cmd: MISE_INSTALL_COMMAND.to_string(),
                custom_name: Some(format!("install mise packages: {}", pkg_list.join(", "))),
            }));

            step.assets.insert("mise.toml".to_string(), mise_toml);
        }

        // 合并额外的 assets
        for (k, v) in &self.assets {
            step.assets.insert(k.clone(), v.clone());
        }

        step.secrets = vec![];
        plan.add_step(step);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::cache_context::CacheContext;
    use crate::resolver::{ResolvedPackage, VersionResolver};

    struct MockResolver;
    impl VersionResolver for MockResolver {
        fn get_latest_version(&self, _pkg: &str, version: &str) -> Result<String> {
            Ok(format!("{}.0.0", version))
        }
        fn get_all_versions(&self, _pkg: &str, _version: &str) -> Result<Vec<String>> {
            Ok(vec!["1.0.0".to_string()])
        }
    }

    fn make_test_resolver() -> Resolver {
        Resolver::new(Box::new(MockResolver))
    }

    #[test]
    fn test_mise_step_builder_default_name() {
        let config = Config::empty();
        let msb = MiseStepBuilder::new(MISE_STEP_NAME, &config);
        assert_eq!(msb.name(), MISE_STEP_NAME);
    }

    #[test]
    fn test_default_package_registers_once() {
        let config = Config::empty();
        let mut msb = MiseStepBuilder::new(MISE_STEP_NAME, &config);
        let mut resolver = make_test_resolver();

        let ref1 = msb.default_package(&mut resolver, "node", "22");
        let ref2 = msb.default_package(&mut resolver, "node", "22");
        assert_eq!(ref1.name, ref2.name);
        assert_eq!(msb.mise_packages.len(), 1);
    }

    #[test]
    fn test_get_output_paths_empty_if_no_packages() {
        let config = Config::empty();
        let msb = MiseStepBuilder::new(MISE_STEP_NAME, &config);
        assert!(msb.get_output_paths().is_empty());
    }

    #[test]
    fn test_get_output_paths_with_packages() {
        let config = Config::empty();
        let mut msb = MiseStepBuilder::new(MISE_STEP_NAME, &config);
        let mut resolver = make_test_resolver();
        msb.default_package(&mut resolver, "node", "22");
        let paths = msb.get_output_paths();
        assert!(paths.contains(&"/mise/shims".to_string()));
    }

    #[test]
    fn test_get_layer_with_packages() {
        let config = Config::empty();
        let mut msb = MiseStepBuilder::new(MISE_STEP_NAME, &config);
        let mut resolver = make_test_resolver();
        msb.default_package(&mut resolver, "node", "22");
        let layer = msb.get_layer();
        assert_eq!(layer.step.as_deref(), Some(MISE_STEP_NAME));
    }

    #[test]
    fn test_build_with_packages() {
        let config = Config::empty();
        let mut msb = MiseStepBuilder::new(MISE_STEP_NAME, &config);
        let mut resolver = make_test_resolver();
        msb.default_package(&mut resolver, "node", "22");

        let dir = tempfile::TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());

        let mut plan = BuildPlan::new();
        let mut options = BuildStepOptions {
            resolved_packages: {
                let mut m = HashMap::new();
                m.insert(
                    "node".to_string(),
                    ResolvedPackage {
                        name: "node".to_string(),
                        requested_version: Some("22".to_string()),
                        resolved_version: Some("22.0.0".to_string()),
                        source: "default".to_string(),
                    },
                );
                m
            },
            caches: CacheContext::new(),
        };

        msb.build(&mut plan, &mut options, &resolver, &app, &env).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].name, Some(MISE_STEP_NAME.to_string()));
        // 验证 mise.toml asset 存在
        assert!(plan.steps[0].assets.contains_key("mise.toml"));
    }

    #[test]
    fn test_build_with_apt_packages_creates_apt_step() {
        let config = Config::empty();
        let mut msb = MiseStepBuilder::new(MISE_STEP_NAME, &config);
        let mut resolver = make_test_resolver();
        msb.default_package(&mut resolver, "node", "22");
        msb.add_supporting_apt_package("build-essential");

        let dir = tempfile::TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());

        let mut plan = BuildPlan::new();
        let mut options = BuildStepOptions {
            resolved_packages: {
                let mut m = HashMap::new();
                m.insert(
                    "node".to_string(),
                    ResolvedPackage {
                        name: "node".to_string(),
                        requested_version: Some("22".to_string()),
                        resolved_version: Some("22.0.0".to_string()),
                        source: "default".to_string(),
                    },
                );
                m
            },
            caches: CacheContext::new(),
        };

        msb.build(&mut plan, &mut options, &resolver, &app, &env).unwrap();
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].name, Some("packages:apt:build".to_string()));
        assert_eq!(plan.steps[1].name, Some(MISE_STEP_NAME.to_string()));
    }
}
