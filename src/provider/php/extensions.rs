/// PHP 扩展检测与安装
///
/// 对齐 railpack `core/providers/php/php.go` 扩展安装部分
/// 解析 composer.json、环境变量、Laravel 必需扩展。

use crate::app::App;
use crate::app::environment::Environment;

/// Laravel 必需扩展
const LARAVEL_REQUIRED_EXTENSIONS: &[&str] = &[
    "ctype",
    "curl",
    "dom",
    "fileinfo",
    "filter",
    "hash",
    "mbstring",
    "openssl",
    "pcre",
    "pdo",
    "session",
    "tokenizer",
    "xml",
];

/// 检测所有需要安装的 PHP 扩展
pub fn detect_extensions(
    app: &App,
    env: &Environment,
    is_laravel: bool,
) -> Vec<String> {
    let mut extensions = Vec::new();

    // 1. 从 composer.json require 中的 ext-* 键
    if let Ok(content) = app.read_file("composer.json") {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(require) = json.get("require").and_then(|r| r.as_object()) {
                for key in require.keys() {
                    if let Some(ext_name) = key.strip_prefix("ext-") {
                        if !extensions.contains(&ext_name.to_string()) {
                            extensions.push(ext_name.to_string());
                        }
                    }
                }
            }
        }
    }

    // 2. ARCPACK_PHP_EXTENSIONS 环境变量
    if let (Some(exts), _) = env.get_config_variable("PHP_EXTENSIONS") {
        for ext in exts.split([',', ' ']) {
            let ext = ext.trim().to_string();
            if !ext.is_empty() && !extensions.contains(&ext) {
                extensions.push(ext);
            }
        }
    }

    // 3. Laravel 必需扩展
    if is_laravel {
        for ext in LARAVEL_REQUIRED_EXTENSIONS {
            if !extensions.contains(&ext.to_string()) {
                extensions.push(ext.to_string());
            }
        }
    }

    // 4. DB 扩展（基于环境变量）
    if let Some(db) = env.get_variable("DB_CONNECTION") {
        match db.as_str() {
            "mysql" | "mariadb" => {
                if !extensions.contains(&"pdo_mysql".to_string()) {
                    extensions.push("pdo_mysql".to_string());
                }
            }
            "pgsql" | "postgres" | "postgresql" => {
                if !extensions.contains(&"pdo_pgsql".to_string()) {
                    extensions.push("pdo_pgsql".to_string());
                }
            }
            "sqlite" => {
                if !extensions.contains(&"pdo_sqlite".to_string()) {
                    extensions.push("pdo_sqlite".to_string());
                }
            }
            _ => {}
        }
    }

    // 5. Redis 扩展
    let needs_redis = env.get_variable("REDIS_HOST").is_some()
        || env.get_variable("REDIS_URL").is_some()
        || env.get_variable("CACHE_DRIVER").map_or(false, |v| v == "redis")
        || env.get_variable("SESSION_DRIVER").map_or(false, |v| v == "redis")
        || env.get_variable("QUEUE_CONNECTION").map_or(false, |v| v == "redis");

    if needs_redis && !extensions.contains(&"redis".to_string()) {
        extensions.push("redis".to_string());
    }

    extensions
}

/// 生成 install-php-extensions 命令
pub fn install_command(extensions: &[String]) -> Option<String> {
    if extensions.is_empty() {
        return None;
    }
    Some(format!(
        "install-php-extensions {}",
        extensions.join(" ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_detect_from_composer_json() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("composer.json"),
            r#"{"require": {"php": "^8.2", "ext-gd": "*", "ext-intl": "*"}}"#,
        )
        .unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let exts = detect_extensions(&app, &env, false);
        assert!(exts.contains(&"gd".to_string()));
        assert!(exts.contains(&"intl".to_string()));
    }

    #[test]
    fn test_detect_from_env_variable() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::from([(
            "ARCPACK_PHP_EXTENSIONS".to_string(),
            "gd,intl,zip".to_string(),
        )]));
        let exts = detect_extensions(&app, &env, false);
        assert!(exts.contains(&"gd".to_string()));
        assert!(exts.contains(&"intl".to_string()));
        assert!(exts.contains(&"zip".to_string()));
    }

    #[test]
    fn test_detect_laravel_required() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let exts = detect_extensions(&app, &env, true);
        assert!(exts.contains(&"mbstring".to_string()));
        assert!(exts.contains(&"pdo".to_string()));
        assert!(exts.contains(&"xml".to_string()));
    }

    #[test]
    fn test_detect_db_extensions() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::from([(
            "DB_CONNECTION".to_string(),
            "mysql".to_string(),
        )]));
        let exts = detect_extensions(&app, &env, false);
        assert!(exts.contains(&"pdo_mysql".to_string()));
    }

    #[test]
    fn test_detect_redis_extension() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::from([(
            "REDIS_HOST".to_string(),
            "localhost".to_string(),
        )]));
        let exts = detect_extensions(&app, &env, false);
        assert!(exts.contains(&"redis".to_string()));
    }

    #[test]
    fn test_install_command() {
        let exts = vec!["gd".to_string(), "intl".to_string()];
        assert_eq!(
            install_command(&exts),
            Some("install-php-extensions gd intl".to_string())
        );
    }

    #[test]
    fn test_install_command_empty() {
        assert_eq!(install_command(&[]), None);
    }
}
