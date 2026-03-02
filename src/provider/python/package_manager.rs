/// Python 包管理器检测和安装逻辑
///
/// 对齐 railpack `core/providers/python/python.go` 中的包管理器部分
use crate::app::App;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::MiseStepBuilder;
use crate::plan::Command;
use crate::resolver::Resolver;

/// 包管理器类型
#[derive(Debug, Clone, PartialEq)]
pub enum PythonPackageManager {
    Pip,
    Uv,
    Poetry,
    Pdm,
    Pipenv,
}

impl std::fmt::Display for PythonPackageManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pip => write!(f, "pip"),
            Self::Uv => write!(f, "uv"),
            Self::Poetry => write!(f, "poetry"),
            Self::Pdm => write!(f, "pdm"),
            Self::Pipenv => write!(f, "pipenv"),
        }
    }
}

/// 检测包管理器类型
pub fn detect_package_manager(app: &App) -> PythonPackageManager {
    // uv: pyproject.toml + uv.lock
    if app.has_file("pyproject.toml") && app.has_file("uv.lock") {
        return PythonPackageManager::Uv;
    }

    // poetry: pyproject.toml + poetry.lock
    if app.has_file("pyproject.toml") && app.has_file("poetry.lock") {
        return PythonPackageManager::Poetry;
    }

    // pdm: pyproject.toml + pdm.lock
    if app.has_file("pyproject.toml") && app.has_file("pdm.lock") {
        return PythonPackageManager::Pdm;
    }

    // pipenv: Pipfile
    if app.has_file("Pipfile") {
        return PythonPackageManager::Pipenv;
    }

    // pip: 默认（requirements.txt 或 pyproject.toml 无对应 lock 文件）
    PythonPackageManager::Pip
}

/// 配置包管理器的 mise 包
pub fn setup_mise_packages(
    pm: &PythonPackageManager,
    mise: &mut MiseStepBuilder,
    resolver: &mut Resolver,
) {
    match pm {
        PythonPackageManager::Uv => {
            let uv_ref = mise.default_package(resolver, "uv", "latest");
            let _ = uv_ref;
        }
        PythonPackageManager::Poetry => {
            let poetry_ref = mise.default_package(resolver, "pipx:poetry", "latest");
            let _ = poetry_ref;
        }
        PythonPackageManager::Pdm => {
            let pdm_ref = mise.default_package(resolver, "pipx:pdm", "latest");
            let _ = pdm_ref;
        }
        PythonPackageManager::Pipenv => {
            let pipenv_ref = mise.default_package(resolver, "pipx:pipenv", "latest");
            let _ = pipenv_ref;
        }
        PythonPackageManager::Pip => {
            // pip 随 Python 内置
        }
    }
}

/// 添加安装命令到 install 步骤
pub fn add_install_commands(
    pm: &PythonPackageManager,
    install: &mut CommandStepBuilder,
    app: &App,
    caches: &mut crate::generate::cache_context::CacheContext,
) {
    match pm {
        PythonPackageManager::Pip => {
            install.add_command(Command::new_exec("python -m venv /app/.venv"));
            install.add_command(Command::new_exec("pip install -r requirements.txt"));
            let cache_name = caches.add_cache("pip", "/opt/pip-cache");
            install.add_cache(&cache_name);
        }
        PythonPackageManager::Uv => {
            install.add_command(Command::new_exec(
                "uv sync --locked --no-dev --no-install-project",
            ));
            let cache_name = caches.add_cache("uv", "/opt/uv-cache");
            install.add_cache(&cache_name);
        }
        PythonPackageManager::Poetry => {
            install.add_command(Command::new_exec(
                "poetry install --no-interaction --no-ansi --only main --no-root",
            ));
        }
        PythonPackageManager::Pdm => {
            install.add_command(Command::new_exec(
                "pdm install --check --prod --no-editable",
            ));
        }
        PythonPackageManager::Pipenv => {
            let has_lock = app.has_file("Pipfile.lock");
            if has_lock {
                install.add_command(Command::new_copy("Pipfile.lock", "Pipfile.lock"));
                install.add_command(Command::new_exec(
                    "pipenv install --deploy --ignore-pipfile",
                ));
            } else {
                install.add_command(Command::new_exec("pipenv install --skip-lock"));
            }
        }
    }
}

/// 获取安装步骤需要复制的文件
pub fn get_install_files(pm: &PythonPackageManager) -> Vec<&'static str> {
    match pm {
        PythonPackageManager::Pip => vec!["requirements.txt"],
        PythonPackageManager::Uv => vec!["pyproject.toml", "uv.lock"],
        PythonPackageManager::Poetry => vec!["pyproject.toml", "poetry.lock"],
        PythonPackageManager::Pdm => vec!["pyproject.toml", "pdm.lock"],
        PythonPackageManager::Pipenv => vec!["Pipfile"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_detect_pip_default() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("requirements.txt"), "flask==2.0").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(detect_package_manager(&app), PythonPackageManager::Pip);
    }

    #[test]
    fn test_detect_uv() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "[project]").unwrap();
        fs::write(dir.path().join("uv.lock"), "").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(detect_package_manager(&app), PythonPackageManager::Uv);
    }

    #[test]
    fn test_detect_poetry() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "[tool.poetry]").unwrap();
        fs::write(dir.path().join("poetry.lock"), "").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(detect_package_manager(&app), PythonPackageManager::Poetry);
    }

    #[test]
    fn test_detect_pdm() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "[project]").unwrap();
        fs::write(dir.path().join("pdm.lock"), "").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(detect_package_manager(&app), PythonPackageManager::Pdm);
    }

    #[test]
    fn test_detect_pipenv() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Pipfile"), "[packages]").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(detect_package_manager(&app), PythonPackageManager::Pipenv);
    }
}
