use super::package_json::PackageJson;
use crate::app::environment::Environment;
/// Node.js 框架检测模块
///
/// 对齐 railpack `core/providers/node/frameworks.go`
/// 检测 12 种主流框架，决定部署模式（SSR 进程 vs SPA 静态）
use crate::app::App;

/// 部署模式
#[derive(Debug, Clone, PartialEq)]
pub enum DeployMode {
    /// 服务端渲染（Node.js 进程）
    Ssr,
    /// 单页应用（Caddy 静态服务）
    Spa,
}

/// 框架信息
#[derive(Debug, Clone)]
pub struct FrameworkInfo {
    /// 框架名称
    pub name: String,
    /// 部署模式
    pub mode: DeployMode,
    /// 启动命令（SSR 模式）
    pub start_cmd: Option<String>,
    /// 静态输出目录（SPA 模式）
    pub output_dir: Option<String>,
    /// 框架特定缓存目录
    pub cache_dirs: Vec<String>,
}

/// 检测所有匹配的框架
///
/// 返回按优先级排序的框架列表（第一个为主要框架）
pub fn detect_frameworks(
    app: &App,
    env: &Environment,
    pkg: &PackageJson,
    pkg_path: &str,
) -> Vec<FrameworkInfo> {
    // ARCPACK_NO_SPA=true 禁用 SPA 检测
    let no_spa = env.is_config_variable_truthy("NO_SPA");

    // ARCPACK_SPA_OUTPUT_DIR 强制 SPA 模式
    let (forced_spa_dir, _) = env.get_config_variable("SPA_OUTPUT_DIR");

    if let Some(dir) = forced_spa_dir {
        return vec![FrameworkInfo {
            name: "custom-spa".to_string(),
            mode: DeployMode::Spa,
            start_cmd: None,
            output_dir: Some(dir),
            cache_dirs: vec![],
        }];
    }

    let mut frameworks = Vec::new();

    // 按优先级顺序检测
    if let Some(fw) = detect_nextjs(pkg, pkg_path) {
        frameworks.push(fw);
    }
    if let Some(fw) = detect_nuxt(pkg) {
        frameworks.push(fw);
    }
    if let Some(fw) = detect_remix(pkg) {
        frameworks.push(fw);
    }
    if let Some(fw) = detect_tanstack_start(pkg) {
        frameworks.push(fw);
    }
    if let Some(fw) = detect_astro(app, pkg, pkg_path) {
        frameworks.push(fw);
    }
    if let Some(fw) = detect_react_router(app, pkg, pkg_path) {
        frameworks.push(fw);
    }
    if let Some(fw) = detect_vite(app, pkg, no_spa, pkg_path) {
        frameworks.push(fw);
    }
    if let Some(fw) = detect_cra(pkg) {
        frameworks.push(fw);
    }
    if let Some(fw) = detect_angular(app, pkg) {
        frameworks.push(fw);
    }

    // 如果 no_spa，过滤掉所有 SPA 框架
    if no_spa {
        frameworks.retain(|fw| fw.mode != DeployMode::Spa);
    }

    frameworks
}

/// 检测 Next.js
fn detect_nextjs(pkg: &PackageJson, pkg_path: &str) -> Option<FrameworkInfo> {
    if !pkg.has_dependency("next") {
        return None;
    }

    let cache_path = if pkg_path.is_empty() {
        "/app/.next/cache".to_string()
    } else {
        format!("/app/{}/.next/cache", pkg_path)
    };

    Some(FrameworkInfo {
        name: "next".to_string(),
        mode: DeployMode::Ssr,
        start_cmd: None,
        output_dir: None,
        cache_dirs: vec![cache_path],
    })
}

/// 检测 Nuxt
fn detect_nuxt(pkg: &PackageJson) -> Option<FrameworkInfo> {
    if !pkg.has_dependency("nuxt") {
        return None;
    }

    Some(FrameworkInfo {
        name: "nuxt".to_string(),
        mode: DeployMode::Ssr,
        start_cmd: Some("node .output/server/index.mjs".to_string()),
        output_dir: None,
        cache_dirs: vec![],
    })
}

/// 检测 Remix
fn detect_remix(pkg: &PackageJson) -> Option<FrameworkInfo> {
    if !pkg.has_dependency("@remix-run/node") {
        return None;
    }

    Some(FrameworkInfo {
        name: "remix".to_string(),
        mode: DeployMode::Ssr,
        start_cmd: None,
        output_dir: None,
        cache_dirs: vec!["/app/.cache".to_string()],
    })
}

/// 检测 TanStack Start
fn detect_tanstack_start(pkg: &PackageJson) -> Option<FrameworkInfo> {
    if !pkg.has_dependency("@tanstack/react-start") {
        return None;
    }

    Some(FrameworkInfo {
        name: "tanstack-start".to_string(),
        mode: DeployMode::Ssr,
        start_cmd: None,
        output_dir: None,
        cache_dirs: vec![],
    })
}

/// 检测 Astro（SSR 或 SPA）
fn detect_astro(app: &App, pkg: &PackageJson, pkg_path: &str) -> Option<FrameworkInfo> {
    if !pkg.has_dependency("astro") {
        return None;
    }

    // 检查是否有 server output 配置或 adapter
    let is_ssr = has_astro_ssr_config(app, pkg);

    let cache_path = if pkg_path.is_empty() {
        "/app/node_modules/.astro".to_string()
    } else {
        format!("/app/{}/node_modules/.astro", pkg_path)
    };

    if is_ssr {
        Some(FrameworkInfo {
            name: "astro".to_string(),
            mode: DeployMode::Ssr,
            start_cmd: None,
            output_dir: None,
            cache_dirs: vec![cache_path],
        })
    } else {
        let out_dir = read_astro_output_dir(app).unwrap_or_else(|| "dist".to_string());
        Some(FrameworkInfo {
            name: "astro".to_string(),
            mode: DeployMode::Spa,
            start_cmd: None,
            output_dir: Some(out_dir),
            cache_dirs: vec![cache_path],
        })
    }
}

/// 检查 Astro 是否配置为 SSR 模式
fn has_astro_ssr_config(app: &App, pkg: &PackageJson) -> bool {
    // 检查是否有 SSR adapter 依赖
    let ssr_adapters = [
        "@astrojs/node",
        "@astrojs/vercel",
        "@astrojs/netlify",
        "@astrojs/cloudflare",
        "@astrojs/deno",
    ];

    for adapter in &ssr_adapters {
        if pkg.has_dependency(adapter) {
            return true;
        }
    }

    // 检查 astro.config.* 中的 output: 'server' 或 output: 'hybrid'
    let config_regex = regex::Regex::new(r#"output\s*:\s*['"](?:server|hybrid)['"]"#).unwrap();
    let configs = app.find_files_with_content("astro.config.*", &config_regex);
    !configs.is_empty()
}

/// 读取 Astro 配置中的 outDir
fn read_astro_output_dir(app: &App) -> Option<String> {
    let out_dir_regex = regex::Regex::new(r#"outDir\s*:\s*['"]([^'"]+)['"]"#).unwrap();
    for config_file in ["astro.config.mjs", "astro.config.js", "astro.config.ts"] {
        if let Ok(content) = app.read_file(config_file) {
            if let Some(captures) = out_dir_regex.captures(&content) {
                return captures.get(1).map(|m| m.as_str().to_string());
            }
        }
    }
    None
}

/// 检测 React Router SPA
fn detect_react_router(app: &App, pkg: &PackageJson, pkg_path: &str) -> Option<FrameworkInfo> {
    let has_config = app.has_file("react-router.config.ts")
        || app.has_file("react-router.config.js")
        || app.has_file("react-router.config.mjs");
    let has_dep = pkg.has_dependency("@react-router/dev");

    if !has_config && !has_dep {
        return None;
    }

    let cache_path = if pkg_path.is_empty() {
        "/app/.react-router".to_string()
    } else {
        format!("/app/{}/.react-router", pkg_path)
    };

    // 读取 buildDirectory 配置
    let out_dir =
        read_react_router_build_dir(app).unwrap_or_else(|| "build/client/".to_string());

    let mode = if pkg.has_script("start") {
        DeployMode::Ssr
    } else {
        DeployMode::Spa
    };
    let output_dir = if mode == DeployMode::Spa {
        Some(out_dir)
    } else {
        None
    };

    Some(FrameworkInfo {
        name: "react-router".to_string(),
        mode,
        start_cmd: None,
        output_dir,
        cache_dirs: vec![cache_path],
    })
}

/// 读取 React Router 配置中的 buildDirectory
fn read_react_router_build_dir(app: &App) -> Option<String> {
    let build_dir_regex = regex::Regex::new(r#"buildDirectory\s*:\s*['"]([^'"]+)['"]"#).unwrap();
    for config_file in [
        "react-router.config.ts",
        "react-router.config.js",
        "react-router.config.mjs",
    ] {
        if let Ok(content) = app.read_file(config_file) {
            if let Some(captures) = build_dir_regex.captures(&content) {
                return captures.get(1).map(|m| m.as_str().to_string());
            }
        }
    }
    None
}

/// 检测 Vite（SSR 或 SPA）
///
/// SvelteKit 使用 vite，但不应被归类为 Vite SPA
fn detect_vite(
    app: &App,
    pkg: &PackageJson,
    no_spa: bool,
    pkg_path: &str,
) -> Option<FrameworkInfo> {
    if !pkg.has_dependency("vite") {
        return None;
    }

    // SvelteKit 排除——它有自己的 SSR 模式
    if pkg.has_dependency("@sveltejs/kit") {
        return None;
    }

    let cache_path = if pkg_path.is_empty() {
        "/app/node_modules/.vite".to_string()
    } else {
        format!("/app/{}/node_modules/.vite", pkg_path)
    };

    // 检查是否有 SSR 配置
    let ssr_regex = regex::Regex::new(r#"ssr\s*:\s*\{"#).unwrap();
    let has_ssr = !app
        .find_files_with_content("vite.config.*", &ssr_regex)
        .is_empty();

    if has_ssr {
        return Some(FrameworkInfo {
            name: "vite".to_string(),
            mode: DeployMode::Ssr,
            start_cmd: None,
            output_dir: None,
            cache_dirs: vec![cache_path],
        });
    }

    // 非 SSR：检查是否有 build 脚本（确认是 SPA 而非纯库）
    if no_spa || !pkg.has_script("build") {
        return None;
    }

    let out_dir = read_vite_output_dir(app).unwrap_or_else(|| "dist".to_string());

    Some(FrameworkInfo {
        name: "vite".to_string(),
        mode: DeployMode::Spa,
        start_cmd: None,
        output_dir: Some(out_dir),
        cache_dirs: vec![cache_path],
    })
}

/// 读取 Vite 配置中的 outDir
fn read_vite_output_dir(app: &App) -> Option<String> {
    let out_dir_regex = regex::Regex::new(r#"outDir\s*:\s*['"]([^'"]+)['"]"#).unwrap();
    for config_file in ["vite.config.ts", "vite.config.js", "vite.config.mjs"] {
        if let Ok(content) = app.read_file(config_file) {
            if let Some(captures) = out_dir_regex.captures(&content) {
                return captures.get(1).map(|m| m.as_str().to_string());
            }
        }
    }
    None
}

/// 检测 CRA (Create React App)
fn detect_cra(pkg: &PackageJson) -> Option<FrameworkInfo> {
    if !pkg.has_dependency("react-scripts") {
        return None;
    }

    // 确认有 react-scripts build 脚本
    if let Some(build_script) = pkg.get_script("build") {
        if !build_script.contains("react-scripts") {
            return None;
        }
    }

    Some(FrameworkInfo {
        name: "cra".to_string(),
        mode: DeployMode::Spa,
        start_cmd: None,
        output_dir: Some("build".to_string()),
        cache_dirs: vec![],
    })
}

/// 检测 Angular
fn detect_angular(app: &App, pkg: &PackageJson) -> Option<FrameworkInfo> {
    if !pkg.has_dependency("@angular/core") {
        return None;
    }

    if !app.has_file("angular.json") {
        return None;
    }

    let out_dir = read_angular_output_path(app).unwrap_or_else(|| "dist".to_string());

    Some(FrameworkInfo {
        name: "angular".to_string(),
        mode: DeployMode::Spa,
        start_cmd: None,
        output_dir: Some(out_dir),
        cache_dirs: vec![],
    })
}

/// 读取 Angular 配置中的 outputPath
fn read_angular_output_path(app: &App) -> Option<String> {
    let content = app.read_file("angular.json").ok()?;
    let config: serde_json::Value = serde_json::from_str(&content).ok()?;

    // 遍历 projects 找第一个 build architect 的 outputPath
    let projects = config.get("projects")?.as_object()?;
    for project in projects.values() {
        if let Some(output_path) = project
            .get("architect")
            .and_then(|a| a.get("build"))
            .and_then(|b| b.get("options"))
            .and_then(|o| o.get("outputPath"))
            .and_then(|p| p.as_str())
        {
            // Angular 17+ 新格式在 outputPath 下有 /browser 子目录
            let path = output_path.to_string();
            // 检查是否有 browser 子目录标识（新版 Angular）
            if let Some(builder) = project
                .get("architect")
                .and_then(|a| a.get("build"))
                .and_then(|b| b.get("builder"))
                .and_then(|b| b.as_str())
            {
                if builder.contains("application") {
                    return Some(format!("{}/browser", path));
                }
            }
            return Some(path);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn make_app(dir: &TempDir) -> App {
        App::new(dir.path().to_str().unwrap()).unwrap()
    }

    fn make_env(vars: &[(&str, &str)]) -> Environment {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Environment::new(map)
    }

    fn make_pkg(json: &str) -> PackageJson {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn test_detect_nextjs() {
        let pkg = make_pkg(r#"{"dependencies": {"next": "14.0.0"}}"#);
        let fw = detect_nextjs(&pkg, "");
        assert!(fw.is_some());
        let fw = fw.unwrap();
        assert_eq!(fw.name, "next");
        assert_eq!(fw.mode, DeployMode::Ssr);
        assert!(fw.cache_dirs[0].contains(".next/cache"));
    }

    #[test]
    fn test_detect_nextjs_monorepo_path() {
        let pkg = make_pkg(r#"{"dependencies": {"next": "14.0.0"}}"#);
        let fw = detect_nextjs(&pkg, "packages/web").unwrap();
        assert!(fw.cache_dirs[0].contains("packages/web/.next/cache"));
    }

    #[test]
    fn test_detect_nuxt() {
        let pkg = make_pkg(r#"{"dependencies": {"nuxt": "3.0.0"}}"#);
        let fw = detect_nuxt(&pkg).unwrap();
        assert_eq!(fw.name, "nuxt");
        assert_eq!(fw.mode, DeployMode::Ssr);
        assert_eq!(
            fw.start_cmd,
            Some("node .output/server/index.mjs".to_string())
        );
    }

    #[test]
    fn test_detect_remix() {
        let pkg = make_pkg(r#"{"dependencies": {"@remix-run/node": "2.0.0"}}"#);
        let fw = detect_remix(&pkg).unwrap();
        assert_eq!(fw.name, "remix");
        assert_eq!(fw.mode, DeployMode::Ssr);
    }

    #[test]
    fn test_detect_tanstack_start() {
        let pkg = make_pkg(r#"{"dependencies": {"@tanstack/react-start": "1.0.0"}}"#);
        let fw = detect_tanstack_start(&pkg).unwrap();
        assert_eq!(fw.name, "tanstack-start");
    }

    #[test]
    fn test_detect_vite_spa() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("vite.config.ts"),
            r#"export default { build: {} }"#,
        )
        .unwrap();
        let app = make_app(&dir);
        let pkg =
            make_pkg(r#"{"dependencies": {"vite": "5.0.0"}, "scripts": {"build": "vite build"}}"#);
        let fw = detect_vite(&app, &pkg, false, "").unwrap();
        assert_eq!(fw.name, "vite");
        assert_eq!(fw.mode, DeployMode::Spa);
        assert_eq!(fw.output_dir, Some("dist".to_string()));
    }

    #[test]
    fn test_detect_vite_custom_outdir() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("vite.config.ts"),
            r#"export default { build: { outDir: 'public' } }"#,
        )
        .unwrap();
        let app = make_app(&dir);
        let pkg =
            make_pkg(r#"{"dependencies": {"vite": "5.0.0"}, "scripts": {"build": "vite build"}}"#);
        let fw = detect_vite(&app, &pkg, false, "").unwrap();
        assert_eq!(fw.output_dir, Some("public".to_string()));
    }

    #[test]
    fn test_detect_vite_ssr() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("vite.config.ts"),
            r#"export default { ssr: { } }"#,
        )
        .unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg(r#"{"dependencies": {"vite": "5.0.0"}}"#);
        let fw = detect_vite(&app, &pkg, false, "").unwrap();
        assert_eq!(fw.mode, DeployMode::Ssr);
    }

    #[test]
    fn test_detect_vite_sveltekit_excluded() {
        let dir = TempDir::new().unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg(
            r#"{"dependencies": {"vite": "5.0.0", "@sveltejs/kit": "2.0.0"}, "scripts": {"build": "vite build"}}"#,
        );
        let fw = detect_vite(&app, &pkg, false, "");
        assert!(fw.is_none(), "SvelteKit 不应被检测为 Vite SPA");
    }

    #[test]
    fn test_detect_cra() {
        let pkg = make_pkg(
            r#"{"dependencies": {"react-scripts": "5.0.0"}, "scripts": {"build": "react-scripts build"}}"#,
        );
        let fw = detect_cra(&pkg).unwrap();
        assert_eq!(fw.name, "cra");
        assert_eq!(fw.mode, DeployMode::Spa);
        assert_eq!(fw.output_dir, Some("build".to_string()));
    }

    #[test]
    fn test_detect_angular() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("angular.json"),
            r#"{"projects": {"app": {"architect": {"build": {"builder": "@angular-devkit/build-angular:browser", "options": {"outputPath": "dist/app"}}}}}}"#,
        )
        .unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg(r#"{"dependencies": {"@angular/core": "17.0.0"}}"#);
        let fw = detect_angular(&app, &pkg).unwrap();
        assert_eq!(fw.name, "angular");
        assert_eq!(fw.mode, DeployMode::Spa);
        assert_eq!(fw.output_dir, Some("dist/app".to_string()));
    }

    #[test]
    fn test_detect_angular_v17_application_builder() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("angular.json"),
            r#"{"projects": {"app": {"architect": {"build": {"builder": "@angular-devkit/build-angular:application", "options": {"outputPath": "dist/app"}}}}}}"#,
        )
        .unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg(r#"{"dependencies": {"@angular/core": "17.0.0"}}"#);
        let fw = detect_angular(&app, &pkg).unwrap();
        assert_eq!(fw.output_dir, Some("dist/app/browser".to_string()));
    }

    #[test]
    fn test_detect_astro_spa() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("astro.config.mjs"), r#"export default {}"#).unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg(r#"{"dependencies": {"astro": "4.0.0"}}"#);
        let fw = detect_astro(&app, &pkg, "").unwrap();
        assert_eq!(fw.name, "astro");
        assert_eq!(fw.mode, DeployMode::Spa);
        assert_eq!(fw.output_dir, Some("dist".to_string()));
    }

    #[test]
    fn test_detect_astro_ssr_with_adapter() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("astro.config.mjs"), r#"export default {}"#).unwrap();
        let app = make_app(&dir);
        let pkg = make_pkg(r#"{"dependencies": {"astro": "4.0.0", "@astrojs/node": "8.0.0"}}"#);
        let fw = detect_astro(&app, &pkg, "").unwrap();
        assert_eq!(fw.mode, DeployMode::Ssr);
    }

    #[test]
    fn test_forced_spa_output_dir() {
        let dir = TempDir::new().unwrap();
        let app = make_app(&dir);
        let env = make_env(&[("ARCPACK_SPA_OUTPUT_DIR", "custom/out")]);
        let pkg = make_pkg(r#"{"name": "test"}"#);
        let frameworks = detect_frameworks(&app, &env, &pkg, "");
        assert_eq!(frameworks.len(), 1);
        assert_eq!(frameworks[0].name, "custom-spa");
        assert_eq!(frameworks[0].output_dir, Some("custom/out".to_string()));
    }

    #[test]
    fn test_no_spa_filters_spa_frameworks() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("vite.config.ts"), r#"export default {}"#).unwrap();
        let app = make_app(&dir);
        let env = make_env(&[("ARCPACK_NO_SPA", "true")]);
        let pkg =
            make_pkg(r#"{"dependencies": {"vite": "5.0.0"}, "scripts": {"build": "vite build"}}"#);
        let frameworks = detect_frameworks(&app, &env, &pkg, "");
        assert!(
            frameworks.iter().all(|fw| fw.mode != DeployMode::Spa),
            "NO_SPA 应过滤所有 SPA 框架"
        );
    }

    #[test]
    fn test_no_framework_detected_for_plain_node() {
        let dir = TempDir::new().unwrap();
        let app = make_app(&dir);
        let env = make_env(&[]);
        let pkg = make_pkg(r#"{"dependencies": {"express": "4.0.0"}}"#);
        let frameworks = detect_frameworks(&app, &env, &pkg, "");
        assert!(frameworks.is_empty());
    }
}
