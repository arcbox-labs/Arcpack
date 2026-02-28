/// Python 框架检测
///
/// 对齐 railpack `core/providers/python/python.go` 中的框架检测部分

use crate::app::App;
use crate::app::environment::Environment;

/// 检测到的框架
#[derive(Debug, Clone)]
pub struct PythonFramework {
    pub name: String,
    pub start_cmd: String,
}

/// Python 入口文件列表
const ENTRY_FILES: &[&str] = &[
    "main.py", "app.py", "start.py", "bot.py", "hello.py", "server.py",
];

/// 检测入口文件
pub fn detect_entry_file(app: &App) -> Option<String> {
    for file in ENTRY_FILES {
        if app.has_file(file) {
            return Some(file.to_string());
        }
    }
    None
}

/// 检测框架并返回启动命令
pub fn detect_framework(
    app: &App,
    env: &Environment,
    dependencies: &[String],
) -> Option<PythonFramework> {
    // Django
    if super::django::is_django(app, dependencies) {
        let wsgi_module = super::django::get_wsgi_module(app, env)
            .unwrap_or_else(|| "config.wsgi".to_string());
        return Some(PythonFramework {
            name: "django".to_string(),
            start_cmd: format!(
                "python manage.py migrate && gunicorn --bind 0.0.0.0:${{PORT:-8000}} {}:application",
                wsgi_module
            ),
        });
    }

    // FastAPI
    if has_dep(dependencies, "fastapi") && has_dep(dependencies, "uvicorn") {
        return Some(PythonFramework {
            name: "fastapi".to_string(),
            start_cmd: "uvicorn main:app --host 0.0.0.0 --port ${PORT:-8000}".to_string(),
        });
    }

    // FastHTML
    if has_dep(dependencies, "python-fasthtml") && has_dep(dependencies, "uvicorn") {
        return Some(PythonFramework {
            name: "fasthtml".to_string(),
            start_cmd: "uvicorn main:app --host 0.0.0.0 --port ${PORT:-8000}".to_string(),
        });
    }

    // Flask
    if has_dep(dependencies, "flask") && has_dep(dependencies, "gunicorn") {
        return Some(PythonFramework {
            name: "flask".to_string(),
            start_cmd: "gunicorn --bind 0.0.0.0:${PORT:-8000} main:app".to_string(),
        });
    }

    // 回退：python <entry_file>
    if let Some(entry) = detect_entry_file(app) {
        return Some(PythonFramework {
            name: "python".to_string(),
            start_cmd: format!("python {}", entry),
        });
    }

    None
}

fn has_dep(dependencies: &[String], name: &str) -> bool {
    dependencies.iter().any(|d| d == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_detect_entry_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.py"), "").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(detect_entry_file(&app), Some("main.py".to_string()));
    }

    #[test]
    fn test_detect_entry_file_app_py() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("app.py"), "").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(detect_entry_file(&app), Some("app.py".to_string()));
    }

    #[test]
    fn test_detect_entry_file_none() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(detect_entry_file(&app), None);
    }

    #[test]
    fn test_detect_fastapi() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let deps = vec!["fastapi".to_string(), "uvicorn".to_string()];
        let fw = detect_framework(&app, &env, &deps).unwrap();
        assert_eq!(fw.name, "fastapi");
        assert!(fw.start_cmd.contains("uvicorn"));
    }

    #[test]
    fn test_detect_flask() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let deps = vec!["flask".to_string(), "gunicorn".to_string()];
        let fw = detect_framework(&app, &env, &deps).unwrap();
        assert_eq!(fw.name, "flask");
        assert!(fw.start_cmd.contains("gunicorn"));
    }

    #[test]
    fn test_detect_fallback_entry() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.py"), "print('hi')").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let deps = vec!["requests".to_string()];
        let fw = detect_framework(&app, &env, &deps).unwrap();
        assert_eq!(fw.name, "python");
        assert_eq!(fw.start_cmd, "python main.py");
    }

    #[test]
    fn test_detect_no_framework() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let deps = vec!["requests".to_string()];
        assert!(detect_framework(&app, &env, &deps).is_none());
    }
}
