/// Django 检测和 WSGI 模块解析
///
/// 对齐 railpack `core/providers/python/django.go`

use crate::app::App;
use crate::app::environment::Environment;

/// 检测是否为 Django 项目
pub fn is_django(app: &App, dependencies: &[String]) -> bool {
    app.has_file("manage.py")
        && dependencies.iter().any(|d| d == "django" || d == "Django")
}

/// 解析 Django WSGI 模块名
pub fn get_wsgi_module(app: &App, env: &Environment) -> Option<String> {
    // 1. ARCPACK_DJANGO_APP_NAME 环境变量
    if let (Some(name), _) = env.get_config_variable("DJANGO_APP_NAME") {
        return Some(format!("{}.wsgi", name));
    }

    // 2. 正则扫描 WSGI_APPLICATION 设置
    if let Ok(re) = regex::Regex::new(r#"WSGI_APPLICATION\s*=\s*['"]([^'"]+)\.application['"]"#) {
        let files = app.find_files_with_content("**/*.py", &re);
        for file in &files {
            if let Ok(content) = app.read_file(file) {
                if let Some(captures) = re.captures(&content) {
                    if let Some(module) = captures.get(1) {
                        return Some(module.as_str().to_string());
                    }
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_is_django_with_manage_py_and_dep() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("manage.py"), "").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let deps = vec!["django".to_string(), "gunicorn".to_string()];
        assert!(is_django(&app, &deps));
    }

    #[test]
    fn test_is_django_without_manage_py() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let deps = vec!["django".to_string()];
        assert!(!is_django(&app, &deps));
    }

    #[test]
    fn test_is_django_without_dep() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("manage.py"), "").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let deps = vec!["flask".to_string()];
        assert!(!is_django(&app, &deps));
    }

    #[test]
    fn test_wsgi_module_from_env() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::from([(
            "ARCPACK_DJANGO_APP_NAME".to_string(),
            "myapp".to_string(),
        )]));
        assert_eq!(get_wsgi_module(&app, &env), Some("myapp.wsgi".to_string()));
    }

    #[test]
    fn test_wsgi_module_from_settings() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("myproject")).unwrap();
        fs::write(
            dir.path().join("myproject/settings.py"),
            "WSGI_APPLICATION = 'myproject.wsgi.application'\n",
        )
        .unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        assert_eq!(
            get_wsgi_module(&app, &env),
            Some("myproject.wsgi".to_string())
        );
    }
}
