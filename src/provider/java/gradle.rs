/// Gradle 构建路径
///
/// 对齐 railpack `core/providers/java/gradle.go`

use crate::app::App;
use crate::app::environment::Environment;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::plan::Command;
use crate::resolver::Resolver;
use crate::generate::mise_step_builder::MiseStepBuilder;

/// 默认 Gradle 版本
const DEFAULT_GRADLE_VERSION: &str = "8";

/// 从 gradle-wrapper.properties 提取 Gradle 版本
pub fn parse_gradle_version(app: &App) -> Option<String> {
    let content = app
        .read_file("gradle/wrapper/gradle-wrapper.properties")
        .ok()?;
    // 匹配 distributionUrl 中的版本号
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("distributionUrl") {
            // 提取 gradle-X.Y.Z 中的版本
            if let Some(pos) = line.find("gradle-") {
                let after = &line[pos + 7..];
                if let Some(end) = after.find('-') {
                    return Some(after[..end].to_string());
                }
                if let Some(end) = after.find(".zip") {
                    return Some(after[..end].to_string());
                }
            }
        }
    }
    None
}

/// 判断 Gradle 版本是否 <= 5（强制 JDK 8）
pub fn is_gradle_legacy(version: &str) -> bool {
    version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .map_or(false, |major| major <= 5)
}

/// 获取 Gradle 版本
pub fn get_gradle_version(app: &App, env: &Environment) -> String {
    // 环境变量覆盖
    if let (Some(v), _) = env.get_config_variable("GRADLE_VERSION") {
        return v;
    }
    // wrapper properties
    if let Some(v) = parse_gradle_version(app) {
        return v;
    }
    DEFAULT_GRADLE_VERSION.to_string()
}

/// 配置 Gradle 构建 mise 包
pub fn setup_gradle_packages(
    mise: &mut MiseStepBuilder,
    resolver: &mut Resolver,
    _env: &Environment,
) {
    let gradle_ref = mise.default_package(resolver, "gradle", DEFAULT_GRADLE_VERSION);
    let _ = gradle_ref;
}

/// 配置 Gradle 构建步骤
pub fn setup_gradle_build(
    build: &mut CommandStepBuilder,
    app: &App,
    caches: &mut crate::generate::cache_context::CacheContext,
) {
    // chmod +x gradlew
    if app.has_file("gradlew") {
        build.add_command(Command::new_exec("chmod +x gradlew"));
    }

    // 构建命令
    let build_cmd = if app.has_file("gradlew") {
        "./gradlew clean build -x check -x test -Pproduction"
    } else {
        "gradle clean build -x check -x test -Pproduction"
    };
    build.add_command(Command::new_exec(build_cmd));

    // 缓存
    let cache_name = caches.add_cache("gradle", "/root/.gradle");
    build.add_cache(&cache_name);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_gradle_legacy_v5() {
        assert!(is_gradle_legacy("5.6.4"));
    }

    #[test]
    fn test_is_gradle_legacy_v4() {
        assert!(is_gradle_legacy("4.10.3"));
    }

    #[test]
    fn test_is_gradle_not_legacy_v8() {
        assert!(!is_gradle_legacy("8.5"));
    }

    #[test]
    fn test_parse_gradle_version_from_wrapper() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("gradle/wrapper")).unwrap();
        std::fs::write(
            dir.path().join("gradle/wrapper/gradle-wrapper.properties"),
            "distributionUrl=https\\://services.gradle.org/distributions/gradle-8.5-bin.zip",
        )
        .unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(parse_gradle_version(&app), Some("8.5".to_string()));
    }
}
