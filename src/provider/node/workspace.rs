/// Workspace / Monorepo 支持
///
/// 对齐 railpack `core/providers/node/workspace.go`
/// 解析 package.json workspaces 和 pnpm-workspace.yaml
use crate::app::App;
use crate::Result;

use super::package_json::PackageJson;

/// Workspace 子包信息
#[derive(Debug, Clone)]
pub struct WorkspacePackage {
    /// 子包相对路径
    pub path: String,
    /// 子包 package.json
    pub package_json: PackageJson,
}

/// 解析 workspace 子包
///
/// 支持：
/// - package.json workspaces 字段（数组或 { packages: [...] }）
/// - pnpm-workspace.yaml packages 字段
pub fn resolve_workspace_packages(app: &App) -> Result<Vec<WorkspacePackage>> {
    let globs = collect_workspace_globs(app)?;

    if globs.is_empty() {
        return Ok(Vec::new());
    }

    let mut packages = Vec::new();

    for glob_pattern in &globs {
        // 转换为文件系统 glob：workspace glob + package.json
        let pattern = if glob_pattern.ends_with('/') {
            format!("{}package.json", glob_pattern)
        } else {
            format!("{}/package.json", glob_pattern)
        };

        let matches = app.find_files(&pattern).unwrap_or_default();

        for matched_file in matches {
            // 提取子包目录路径
            let pkg_dir = matched_file
                .strip_suffix("/package.json")
                .unwrap_or(&matched_file);

            if pkg_dir.is_empty() || pkg_dir == "package.json" {
                continue; // 跳过根 package.json
            }

            if let Ok(pkg_json) = app.read_json::<PackageJson>(&matched_file) {
                packages.push(WorkspacePackage {
                    path: pkg_dir.to_string(),
                    package_json: pkg_json,
                });
            }
        }
    }

    // 按路径排序保证确定性
    packages.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(packages)
}

/// 收集 workspace glob 模式
fn collect_workspace_globs(app: &App) -> Result<Vec<String>> {
    let mut globs = Vec::new();

    // 1. 从 package.json workspaces 字段读取
    if app.has_file("package.json") {
        if let Ok(pkg) = app.read_json::<PackageJson>("package.json") {
            globs.extend(pkg.workspaces);
        }
    }

    // 2. 从 pnpm-workspace.yaml 读取
    if app.has_file("pnpm-workspace.yaml") {
        if let Ok(config) = app.read_yaml::<PnpmWorkspaceConfig>("pnpm-workspace.yaml") {
            globs.extend(config.packages);
        }
    }

    // 去重
    globs.sort();
    globs.dedup();

    Ok(globs)
}

/// pnpm-workspace.yaml 结构
#[derive(Debug, serde::Deserialize)]
struct PnpmWorkspaceConfig {
    #[serde(default)]
    packages: Vec<String>,
}

/// 生成 workspace 子包的缓存名称
///
/// 如 `packages/web` → `next-packages-web`
pub fn workspace_cache_name(prefix: &str, pkg_path: &str) -> String {
    let sanitized = pkg_path.replace('/', "-");
    format!("{}-{}", prefix, sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_app(dir: &TempDir) -> App {
        App::new(dir.path().to_str().unwrap()).unwrap()
    }

    #[test]
    fn test_resolve_workspace_packages_from_package_json_array() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"workspaces": ["packages/*"]}"#,
        )
        .unwrap();

        // 创建子包
        fs::create_dir_all(dir.path().join("packages/web")).unwrap();
        fs::write(
            dir.path().join("packages/web/package.json"),
            r#"{"name": "@app/web"}"#,
        )
        .unwrap();

        fs::create_dir_all(dir.path().join("packages/api")).unwrap();
        fs::write(
            dir.path().join("packages/api/package.json"),
            r#"{"name": "@app/api"}"#,
        )
        .unwrap();

        let app = make_app(&dir);
        let packages = resolve_workspace_packages(&app).unwrap();
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].path, "packages/api");
        assert_eq!(packages[1].path, "packages/web");
    }

    #[test]
    fn test_resolve_workspace_packages_from_package_json_object() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"workspaces": {"packages": ["apps/*"]}}"#,
        )
        .unwrap();

        fs::create_dir_all(dir.path().join("apps/frontend")).unwrap();
        fs::write(
            dir.path().join("apps/frontend/package.json"),
            r#"{"name": "frontend"}"#,
        )
        .unwrap();

        let app = make_app(&dir);
        let packages = resolve_workspace_packages(&app).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].path, "apps/frontend");
        assert_eq!(packages[0].package_json.name, "frontend");
    }

    #[test]
    fn test_resolve_workspace_packages_from_pnpm_workspace() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("package.json"), r#"{"name": "root"}"#).unwrap();
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\n",
        )
        .unwrap();

        fs::create_dir_all(dir.path().join("packages/lib")).unwrap();
        fs::write(
            dir.path().join("packages/lib/package.json"),
            r#"{"name": "@app/lib"}"#,
        )
        .unwrap();

        let app = make_app(&dir);
        let packages = resolve_workspace_packages(&app).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].path, "packages/lib");
    }

    #[test]
    fn test_resolve_workspace_packages_empty() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("package.json"), r#"{"name": "test"}"#).unwrap();

        let app = make_app(&dir);
        let packages = resolve_workspace_packages(&app).unwrap();
        assert!(packages.is_empty());
    }

    #[test]
    fn test_workspace_cache_name() {
        assert_eq!(
            workspace_cache_name("next", "packages/web"),
            "next-packages-web"
        );
        assert_eq!(
            workspace_cache_name("vite", "apps/frontend"),
            "vite-apps-frontend"
        );
    }
}
