use crate::app::environment::Environment;
/// Laravel 支持
///
/// 对齐 railpack `core/providers/php/php.go` Laravel 部分
/// 检测 artisan 文件、根目录覆盖、构建缓存命令。
use crate::app::App;

/// 检测 Laravel 应用
pub fn detect_laravel(app: &App) -> bool {
    app.has_file("artisan")
}

/// 获取 Laravel 根目录
pub fn get_root_dir(env: &Environment) -> String {
    if let (Some(root), _) = env.get_config_variable("PHP_ROOT_DIR") {
        return root;
    }
    "/app/public".to_string()
}

/// 获取 Laravel 构建命令列表
pub fn get_build_commands(env: &Environment) -> Vec<String> {
    let mut commands = Vec::new();

    // storage 目录初始化（确保运行时目录存在且可写）
    commands.push(
        "mkdir -p storage/framework/{sessions,views,cache,testing} storage/logs bootstrap/cache && chmod -R ug+rw storage"
            .to_string(),
    );

    // 缓存配置
    commands.push("php artisan config:cache".to_string());
    commands.push("php artisan event:cache".to_string());
    commands.push("php artisan route:cache".to_string());
    commands.push("php artisan view:cache".to_string());

    // 可选迁移
    if let (Some(skip), _) = env.get_config_variable("SKIP_MIGRATIONS") {
        if skip == "true" || skip == "1" {
            return commands;
        }
    }

    commands
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_detect_laravel() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("artisan"), "#!/usr/bin/env php").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert!(detect_laravel(&app));
    }

    #[test]
    fn test_detect_not_laravel() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert!(!detect_laravel(&app));
    }

    #[test]
    fn test_get_root_dir_default() {
        let env = Environment::new(HashMap::new());
        assert_eq!(get_root_dir(&env), "/app/public");
    }

    #[test]
    fn test_get_root_dir_custom() {
        let env = Environment::new(HashMap::from([(
            "ARCPACK_PHP_ROOT_DIR".to_string(),
            "/app/web".to_string(),
        )]));
        assert_eq!(get_root_dir(&env), "/app/web");
    }

    #[test]
    fn test_get_build_commands() {
        let env = Environment::new(HashMap::new());
        let cmds = get_build_commands(&env);
        assert!(cmds.iter().any(|c| c.contains("config:cache")));
        assert!(cmds.iter().any(|c| c.contains("route:cache")));
    }
}
