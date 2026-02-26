use crate::app::App;

use super::package_json::PackageJson;
use super::package_manager::{parse_yarn_package_manager, PackageManagerKind};

/// 检测包管理器类型
///
/// 对齐 railpack `core/providers/node/node.go` getPackageManager
/// 优先级：packageManager 字段 → lockfile → engines → 默认 npm
pub fn detect_package_manager(app: &App, package_json: &PackageJson) -> PackageManagerKind {
    // 1. 检查 packageManager 字段
    if package_json.package_manager.is_some() {
        let (pm_name, pm_version) = package_json.get_package_manager_info();
        match pm_name.as_str() {
            "yarn" if !pm_version.is_empty() => {
                return parse_yarn_package_manager(&pm_version);
            }
            "pnpm" => return PackageManagerKind::Pnpm,
            "npm" => return PackageManagerKind::Npm,
            "bun" => return PackageManagerKind::Bun,
            _ => {} // 未知包管理器，继续检测
        }
    }

    // 2. 基于 lockfile 检测
    if app.has_file("pnpm-lock.yaml") {
        return PackageManagerKind::Pnpm;
    }
    if app.has_file("bun.lockb") || app.has_file("bun.lock") {
        return PackageManagerKind::Bun;
    }
    if app.has_file(".yarnrc.yml") || app.has_file(".yarnrc.yaml") {
        return PackageManagerKind::YarnBerry;
    }
    if app.has_file("yarn.lock") {
        return PackageManagerKind::Yarn1;
    }

    // 3. 基于 engines 字段检测
    if let Some(engine) = package_json.engines.get("pnpm") {
        if !engine.trim().is_empty() {
            return PackageManagerKind::Pnpm;
        }
    }
    if let Some(engine) = package_json.engines.get("bun") {
        if !engine.trim().is_empty() {
            return PackageManagerKind::Bun;
        }
    }
    if let Some(engine) = package_json.engines.get("yarn") {
        if !engine.trim().is_empty() {
            return parse_yarn_package_manager(engine);
        }
    }

    // 4. 默认 npm
    PackageManagerKind::Npm
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    fn make_app(dir: &TempDir) -> App {
        App::new(dir.path().to_str().unwrap()).unwrap()
    }

    fn make_pkg_json(json: &str) -> PackageJson {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn test_detect_npm_default() {
        let dir = TempDir::new().unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg_json(r#"{ "name": "test" }"#);
        assert_eq!(detect_package_manager(&app, &pkg), PackageManagerKind::Npm);
    }

    #[test]
    fn test_detect_pnpm_by_lockfile() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("pnpm-lock.yaml"), "lockfileVersion: '9.0'").unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg_json(r#"{ "name": "test" }"#);
        assert_eq!(detect_package_manager(&app, &pkg), PackageManagerKind::Pnpm);
    }

    #[test]
    fn test_detect_bun_by_lockfile() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("bun.lockb"), "").unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg_json(r#"{ "name": "test" }"#);
        assert_eq!(detect_package_manager(&app, &pkg), PackageManagerKind::Bun);
    }

    #[test]
    fn test_detect_bun_by_bun_lock() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("bun.lock"), "").unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg_json(r#"{ "name": "test" }"#);
        assert_eq!(detect_package_manager(&app, &pkg), PackageManagerKind::Bun);
    }

    #[test]
    fn test_detect_yarn1_by_lockfile() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("yarn.lock"), "").unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg_json(r#"{ "name": "test" }"#);
        assert_eq!(detect_package_manager(&app, &pkg), PackageManagerKind::Yarn1);
    }

    #[test]
    fn test_detect_yarn_berry_by_yarnrc() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".yarnrc.yml"), "nodeLinker: node-modules").unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg_json(r#"{ "name": "test" }"#);
        assert_eq!(detect_package_manager(&app, &pkg), PackageManagerKind::YarnBerry);
    }

    #[test]
    fn test_detect_from_package_manager_field_pnpm() {
        let dir = TempDir::new().unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg_json(r#"{ "packageManager": "pnpm@9.0.0" }"#);
        assert_eq!(detect_package_manager(&app, &pkg), PackageManagerKind::Pnpm);
    }

    #[test]
    fn test_detect_from_package_manager_field_yarn1() {
        let dir = TempDir::new().unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg_json(r#"{ "packageManager": "yarn@1.22.0" }"#);
        assert_eq!(detect_package_manager(&app, &pkg), PackageManagerKind::Yarn1);
    }

    #[test]
    fn test_detect_from_package_manager_field_yarn_berry() {
        let dir = TempDir::new().unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg_json(r#"{ "packageManager": "yarn@4.0.0" }"#);
        assert_eq!(detect_package_manager(&app, &pkg), PackageManagerKind::YarnBerry);
    }

    #[test]
    fn test_detect_from_engines_pnpm() {
        let dir = TempDir::new().unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg_json(r#"{ "engines": { "pnpm": ">=8" } }"#);
        assert_eq!(detect_package_manager(&app, &pkg), PackageManagerKind::Pnpm);
    }

    #[test]
    fn test_package_manager_field_takes_priority() {
        let dir = TempDir::new().unwrap();
        // 即使有 yarn.lock，packageManager 字段优先
        fs::write(dir.path().join("yarn.lock"), "").unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg_json(r#"{ "packageManager": "pnpm@9.0.0" }"#);
        assert_eq!(detect_package_manager(&app, &pkg), PackageManagerKind::Pnpm);
    }
}
