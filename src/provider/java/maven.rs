/// Maven 构建路径
///
/// 对齐 railpack `core/providers/java/maven.go`
use crate::app::App;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::MiseStepBuilder;
use crate::plan::Command;
use crate::resolver::Resolver;

/// 检测是否有 Maven wrapper
pub fn has_maven_wrapper(app: &App) -> bool {
    app.has_file("mvnw") && app.has_file(".mvn/wrapper/maven-wrapper.properties")
}

/// 获取 Maven 命令（mvn 或 ./mvnw）
pub fn get_maven_command(app: &App) -> &'static str {
    if has_maven_wrapper(app) {
        "./mvnw"
    } else {
        "mvn"
    }
}

/// 配置 Maven 构建 mise 包
pub fn setup_maven_packages(mise: &mut MiseStepBuilder, resolver: &mut Resolver) {
    let maven_ref = mise.default_package(resolver, "maven", "latest");
    let _ = maven_ref;
}

/// 配置 Maven 构建步骤
pub fn setup_maven_build(
    build: &mut CommandStepBuilder,
    app: &App,
    caches: &mut crate::generate::cache_context::CacheContext,
) {
    let mvn = get_maven_command(app);

    // chmod +x mvnw
    if has_maven_wrapper(app) {
        build.add_command(Command::new_exec("chmod +x mvnw"));
    }

    // 构建命令
    let build_cmd = format!(
        "{} -DoutputFile=target/mvn-dependency-list.log -B -DskipTests clean dependency:list install -Pproduction",
        mvn
    );
    build.add_command(Command::new_exec(build_cmd));

    // 缓存
    let cache_name = caches.add_cache("maven", ".m2/repository");
    build.add_cache(&cache_name);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_has_maven_wrapper_both_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("mvnw"), "#!/bin/bash").unwrap();
        fs::create_dir_all(dir.path().join(".mvn/wrapper")).unwrap();
        fs::write(
            dir.path().join(".mvn/wrapper/maven-wrapper.properties"),
            "distributionUrl=...",
        )
        .unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert!(has_maven_wrapper(&app));
    }

    #[test]
    fn test_has_maven_wrapper_no_mvnw() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert!(!has_maven_wrapper(&app));
    }

    #[test]
    fn test_get_maven_command_with_wrapper() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("mvnw"), "#!/bin/bash").unwrap();
        fs::create_dir_all(dir.path().join(".mvn/wrapper")).unwrap();
        fs::write(dir.path().join(".mvn/wrapper/maven-wrapper.properties"), "").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(get_maven_command(&app), "./mvnw");
    }

    #[test]
    fn test_get_maven_command_without_wrapper() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(get_maven_command(&app), "mvn");
    }
}
