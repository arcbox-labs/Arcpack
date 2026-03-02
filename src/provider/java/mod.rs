/// Java Provider：Maven/Gradle 双路径检测
///
/// 对齐 railpack `core/providers/java/java.go`
/// 支持 wrapper（mvnw/gradlew）、Spring Boot 检测、运行时 JDK 分离。
pub mod gradle;
pub mod maven;

use crate::app::environment::Environment;
use crate::app::App;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::{self, MiseStepBuilder};
use crate::generate::GenerateContext;
use crate::plan::{Filter, Layer};
use crate::provider::Provider;
use crate::Result;

/// 默认 JDK 版本
const DEFAULT_JDK_VERSION: &str = "21";

/// 构建工具类型
#[derive(Debug, Clone, PartialEq)]
enum BuildTool {
    Maven,
    Gradle,
}

/// Java Provider
pub struct JavaProvider {
    build_tool: BuildTool,
    is_spring_boot: bool,
    jdk_version: String,
}

impl JavaProvider {
    pub fn new() -> Self {
        Self {
            build_tool: BuildTool::Maven,
            is_spring_boot: false,
            jdk_version: DEFAULT_JDK_VERSION.to_string(),
        }
    }

    /// 检测构建工具
    fn detect_build_tool(app: &App) -> Option<BuildTool> {
        if app.has_file("gradlew")
            || app.has_file("build.gradle")
            || app.has_file("build.gradle.kts")
        {
            return Some(BuildTool::Gradle);
        }
        if app.has_match("pom.{xml,atom,clj,groovy,rb,scala,yaml,yml}") || app.has_file("pom.xml") {
            return Some(BuildTool::Maven);
        }
        None
    }

    /// 检测 Spring Boot
    fn detect_spring_boot(app: &App) -> bool {
        // 检查 build.gradle 或 pom.xml 中是否引用 spring-boot
        if let Ok(content) = app.read_file("build.gradle") {
            if content.contains("spring-boot") || content.contains("org.springframework.boot") {
                return true;
            }
        }
        if let Ok(content) = app.read_file("build.gradle.kts") {
            if content.contains("spring-boot") || content.contains("org.springframework.boot") {
                return true;
            }
        }
        if let Ok(content) = app.read_file("pom.xml") {
            if content.contains("spring-boot") || content.contains("org.springframework.boot") {
                return true;
            }
        }
        false
    }

    /// 获取 StartCmd
    fn get_start_command(&self) -> String {
        match (&self.build_tool, self.is_spring_boot) {
            (BuildTool::Gradle, true) => {
                // 先尝试子目录 jar（多模块），回退到根 build/libs
                "java $JAVA_OPTS -Dserver.port=$PORT -jar $(ls -1 */build/libs/*jar build/libs/*jar 2>/dev/null | grep -v plain | head -1)"
                    .to_string()
            }
            (BuildTool::Gradle, false) => {
                "java $JAVA_OPTS -jar $(ls -1 */build/libs/*jar build/libs/*jar 2>/dev/null | grep -v plain | head -1)".to_string()
            }
            (BuildTool::Maven, true) => {
                "java -Dserver.port=$PORT $JAVA_OPTS -jar target/*jar".to_string()
            }
            (BuildTool::Maven, false) => {
                "java $JAVA_OPTS -jar target/*jar".to_string()
            }
        }
    }

    /// 确保 mise_step_builder 已初始化
    fn ensure_mise_step_builder(ctx: &mut GenerateContext) {
        if ctx.mise_step_builder.is_none() {
            ctx.mise_step_builder = Some(MiseStepBuilder::new(
                mise_step_builder::MISE_STEP_NAME,
                &ctx.config,
            ));
        }
    }

    /// 获取指定名称的 CommandStepBuilder 可变引用
    fn get_command_step<'a>(
        steps: &'a mut [Box<dyn crate::generate::StepBuilder>],
        name: &str,
    ) -> &'a mut CommandStepBuilder {
        let idx = steps.iter().position(|s| s.name() == name).unwrap();
        steps[idx]
            .as_any_mut()
            .downcast_mut::<CommandStepBuilder>()
            .unwrap()
    }
}

impl Provider for JavaProvider {
    fn name(&self) -> &str {
        "java"
    }

    fn detect(&self, app: &App, _env: &Environment) -> Result<bool> {
        Ok(Self::detect_build_tool(app).is_some())
    }

    fn initialize(&mut self, ctx: &mut GenerateContext) -> Result<()> {
        if let Some(tool) = Self::detect_build_tool(&ctx.app) {
            self.build_tool = tool;
        }

        self.is_spring_boot = Self::detect_spring_boot(&ctx.app);

        // JDK 版本
        self.jdk_version = DEFAULT_JDK_VERSION.to_string();
        if let (Some(v), _) = ctx.env.get_config_variable("JDK_VERSION") {
            self.jdk_version = v;
        }

        // Gradle 低版本强制 JDK 8
        if self.build_tool == BuildTool::Gradle {
            let gradle_version = gradle::get_gradle_version(&ctx.app, &ctx.env);
            if gradle::is_gradle_legacy(&gradle_version) {
                self.jdk_version = "8".to_string();
            }
        }

        Ok(())
    }

    fn plan(&self, ctx: &mut GenerateContext) -> Result<()> {
        // 元数据
        ctx.metadata.set(
            "javaPackageManager",
            match self.build_tool {
                BuildTool::Maven => "maven",
                BuildTool::Gradle => "gradle",
            },
        );
        if self.is_spring_boot {
            ctx.metadata.set("javaFramework", "spring-boot");
        }

        // === 构建期 mise 步骤：JDK + 构建工具 ===
        Self::ensure_mise_step_builder(ctx);

        {
            let mise = ctx.mise_step_builder.as_mut().unwrap();
            let java_ref = mise.default_package(&mut ctx.resolver, "java", &self.jdk_version);
            mise.version(&mut ctx.resolver, &java_ref, &self.jdk_version, "default");

            match self.build_tool {
                BuildTool::Gradle => {
                    gradle::setup_gradle_packages(mise, &mut ctx.resolver, &ctx.env);
                }
                BuildTool::Maven => {
                    maven::setup_maven_packages(mise, &mut ctx.resolver);
                }
            }
        }

        let mise_step_name = ctx
            .mise_step_builder
            .as_ref()
            .map(|m| m.name().to_string())
            .unwrap_or_else(|| mise_step_builder::MISE_STEP_NAME.to_string());

        // === build 步骤 ===
        let build = ctx.new_command_step("build");
        build.add_input(Layer::new_step_layer(&mise_step_name, None));
        {
            let local_layer = ctx.new_local_layer();
            let build = Self::get_command_step(&mut ctx.steps, "build");
            build.add_input(local_layer);

            match self.build_tool {
                BuildTool::Gradle => {
                    gradle::setup_gradle_build(build, &ctx.app, &mut ctx.caches);
                }
                BuildTool::Maven => {
                    maven::setup_maven_build(build, &ctx.app, &mut ctx.caches);
                }
            }
        }

        // === 运行时 JDK 步骤（独立的 mise builder，不含构建工具） ===
        // 使用直接字段访问避免双重可变借用
        ctx.additional_mise_builders.push((
            "packages:mise:runtime".to_string(),
            MiseStepBuilder::new("packages:mise:runtime", &ctx.config),
        ));
        {
            let jdk_version = self.jdk_version.clone();
            let runtime_mise = &mut ctx.additional_mise_builders.last_mut().unwrap().1;
            let java_ref = runtime_mise.default_package(&mut ctx.resolver, "java", &jdk_version);
            runtime_mise.version(&mut ctx.resolver, &java_ref, &jdk_version, "runtime");
        }

        // === Deploy 配置 ===
        ctx.deploy.start_cmd = Some(self.get_start_command());

        // deploy inputs: 运行时 mise 层 + build 步骤输出
        let runtime_mise_layer = Layer::new_step_layer(
            "packages:mise:runtime",
            Some(Filter::include_only(vec![
                "/mise/shims".to_string(),
                "/mise/installs".to_string(),
                "/usr/local/bin/mise".to_string(),
                "/etc/mise/config.toml".to_string(),
                "/root/.local/state/mise".to_string(),
            ])),
        );

        let build_output = match self.build_tool {
            BuildTool::Maven => "target/.".to_string(),
            BuildTool::Gradle => ".".to_string(),
        };
        let build_layer =
            Layer::new_step_layer("build", Some(Filter::include_only(vec![build_output])));

        ctx.deploy.add_inputs(&[runtime_mise_layer, build_layer]);

        Ok(())
    }

    fn start_command_help(&self) -> Option<String> {
        Some(
            "To configure your start command, arcpack will:\n\n\
             For Maven: java $JAVA_OPTS -jar target/*jar\n\
             For Gradle: java $JAVA_OPTS -jar $(ls -1 */build/libs/*jar | grep -v plain)\n\n\
             Spring Boot apps will automatically get -Dserver.port=$PORT"
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::resolver::VersionResolver;
    use std::collections::HashMap;
    use std::fs;
    use tempfile::TempDir;

    struct MockVersionResolver;
    impl VersionResolver for MockVersionResolver {
        fn get_latest_version(&self, _pkg: &str, version: &str) -> Result<String> {
            Ok(format!("{}.0.0", version))
        }
        fn get_all_versions(&self, _pkg: &str, _version: &str) -> Result<Vec<String>> {
            Ok(vec!["1.0.0".to_string()])
        }
    }

    fn make_ctx(dir: &TempDir) -> GenerateContext {
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let config = Config::empty();
        GenerateContext::new(app, env, config, Box::new(MockVersionResolver)).unwrap()
    }

    fn make_ctx_with_env(dir: &TempDir, env_vars: HashMap<String, String>) -> GenerateContext {
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(env_vars);
        let config = Config::empty();
        GenerateContext::new(app, env, config, Box::new(MockVersionResolver)).unwrap()
    }

    // === detect 测试 ===

    #[test]
    fn test_detect_with_pom_xml() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("pom.xml"), "<project></project>").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = JavaProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_with_gradlew() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("gradlew"), "#!/bin/bash").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = JavaProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_with_build_gradle() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("build.gradle"), "apply plugin: 'java'").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = JavaProvider::new();
        assert!(provider.detect(&app, &env).unwrap());
    }

    #[test]
    fn test_detect_empty() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let provider = JavaProvider::new();
        assert!(!provider.detect(&app, &env).unwrap());
    }

    // === 构建工具检测 ===

    #[test]
    fn test_build_tool_maven() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("pom.xml"), "<project></project>").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = JavaProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.build_tool, BuildTool::Maven);
    }

    #[test]
    fn test_build_tool_gradle() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("gradlew"), "#!/bin/bash").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = JavaProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.build_tool, BuildTool::Gradle);
    }

    // === Spring Boot 检测 ===

    #[test]
    fn test_spring_boot_detection_pom() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("pom.xml"),
            r#"<project>
                <parent>
                    <artifactId>spring-boot-starter-parent</artifactId>
                </parent>
            </project>"#,
        )
        .unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = JavaProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert!(provider.is_spring_boot);
    }

    #[test]
    fn test_spring_boot_detection_gradle() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("build.gradle"),
            "plugins { id 'org.springframework.boot' version '3.0.0' }",
        )
        .unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = JavaProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert!(provider.is_spring_boot);
    }

    // === StartCmd 测试 ===

    #[test]
    fn test_start_cmd_gradle_spring() {
        let p = JavaProvider {
            build_tool: BuildTool::Gradle,
            is_spring_boot: true,
            jdk_version: "21".to_string(),
        };
        let cmd = p.get_start_command();
        assert!(cmd.contains("-Dserver.port=$PORT"));
        assert!(cmd.contains("build/libs"));
    }

    #[test]
    fn test_start_cmd_maven() {
        let p = JavaProvider {
            build_tool: BuildTool::Maven,
            is_spring_boot: false,
            jdk_version: "21".to_string(),
        };
        let cmd = p.get_start_command();
        assert!(cmd.contains("target/*jar"));
        assert!(!cmd.contains("-Dserver.port"));
    }

    #[test]
    fn test_start_cmd_maven_spring() {
        let p = JavaProvider {
            build_tool: BuildTool::Maven,
            is_spring_boot: true,
            jdk_version: "21".to_string(),
        };
        let cmd = p.get_start_command();
        assert!(cmd.contains("-Dserver.port=$PORT"));
    }

    // === JDK 版本测试 ===

    #[test]
    fn test_jdk_version_default() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("pom.xml"), "<project></project>").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = JavaProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.jdk_version, "21");
    }

    #[test]
    fn test_jdk_version_env_override() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("pom.xml"), "<project></project>").unwrap();
        let mut ctx = make_ctx_with_env(
            &dir,
            HashMap::from([("ARCPACK_JDK_VERSION".to_string(), "17".to_string())]),
        );
        let mut provider = JavaProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.jdk_version, "17");
    }

    #[test]
    fn test_jdk_version_legacy_gradle_forces_8() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("gradlew"), "#!/bin/bash").unwrap();
        fs::create_dir_all(dir.path().join("gradle/wrapper")).unwrap();
        fs::write(
            dir.path().join("gradle/wrapper/gradle-wrapper.properties"),
            "distributionUrl=https\\://services.gradle.org/distributions/gradle-4.10.3-bin.zip",
        )
        .unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = JavaProvider::new();
        provider.initialize(&mut ctx).unwrap();
        assert_eq!(provider.jdk_version, "8");
    }

    // === plan 测试 ===

    #[test]
    fn test_plan_maven_basic() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("pom.xml"), "<project></project>").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = JavaProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        let step_names: Vec<&str> = ctx.steps.iter().map(|s| s.name()).collect();
        assert!(step_names.contains(&"build"));

        assert!(ctx.deploy.start_cmd.is_some());
        assert_eq!(ctx.metadata.get("javaPackageManager"), Some("maven"));

        // 验证运行时 JDK 步骤
        assert!(!ctx.additional_mise_builders.is_empty());
        assert_eq!(ctx.additional_mise_builders[0].0, "packages:mise:runtime");
    }

    #[test]
    fn test_plan_gradle_basic() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("gradlew"), "#!/bin/bash").unwrap();
        fs::write(dir.path().join("build.gradle"), "apply plugin: 'java'").unwrap();
        let mut ctx = make_ctx(&dir);
        let mut provider = JavaProvider::new();
        provider.initialize(&mut ctx).unwrap();
        provider.plan(&mut ctx).unwrap();

        assert_eq!(ctx.metadata.get("javaPackageManager"), Some("gradle"));
    }
}
