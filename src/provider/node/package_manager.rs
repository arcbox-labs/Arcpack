use std::collections::HashMap;

use crate::app::App;
use crate::generate::cache_context::CacheContext;
use crate::generate::command_step_builder::CommandStepBuilder;
use crate::generate::mise_step_builder::MiseStepBuilder;
use crate::plan::cache::CacheType;
use crate::plan::Command;
use crate::resolver::Resolver;

use super::package_json::PackageJson;

/// 包管理器类型
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PackageManagerKind {
    Npm,
    Yarn1,
    YarnBerry,
    Pnpm,
    Bun,
}

/// 默认 pnpm 版本
pub const DEFAULT_PNPM_VERSION: &str = "9";

impl PackageManagerKind {
    /// 包管理器显示名
    pub fn name(&self) -> &str {
        match self {
            PackageManagerKind::Npm => "npm",
            PackageManagerKind::Pnpm => "pnpm",
            PackageManagerKind::Bun => "bun",
            PackageManagerKind::Yarn1 | PackageManagerKind::YarnBerry => "yarn",
        }
    }

    /// 运行脚本命令 (e.g. "npm run build")
    pub fn run_cmd(&self, script: &str) -> String {
        format!("{} run {}", self.name(), script)
    }

    /// 运行脚本/文件命令
    pub fn run_script_command(&self, cmd: &str) -> String {
        if *self == PackageManagerKind::Bun {
            format!("bun {}", cmd)
        } else {
            format!("node {}", cmd)
        }
    }

    /// lockfile 名称
    pub fn lockfile_name(&self) -> &str {
        match self {
            PackageManagerKind::Npm => "package-lock.json",
            PackageManagerKind::Yarn1 | PackageManagerKind::YarnBerry => "yarn.lock",
            PackageManagerKind::Pnpm => "pnpm-lock.yaml",
            PackageManagerKind::Bun => "bun.lockb",
        }
    }

    /// 缓存目录
    pub fn cache_dir(&self) -> &str {
        match self {
            PackageManagerKind::Npm => "/root/.npm",
            PackageManagerKind::Pnpm => "/root/.local/share/pnpm/store/v3",
            PackageManagerKind::Bun => "/root/.bun/install/cache",
            PackageManagerKind::Yarn1 => "/usr/local/share/.cache/yarn",
            PackageManagerKind::YarnBerry => "/app/.yarn/cache",
        }
    }

    /// 缓存类型
    pub fn cache_type(&self) -> CacheType {
        match self {
            PackageManagerKind::Yarn1 => CacheType::Locked,
            _ => CacheType::Shared,
        }
    }

    /// 获取安装缓存名
    pub fn get_install_cache(&self, caches: &mut CacheContext) -> String {
        let cache_name = format!("{}-install", self.name());
        caches.add_cache_with_type(&cache_name, self.cache_dir(), self.cache_type())
    }

    /// 获取安装目录（用于 deploy 层）
    pub fn get_install_folder(&self, app: &App) -> Vec<String> {
        match self {
            PackageManagerKind::YarnBerry => {
                let mut folders = vec!["/app/.yarn".to_string()];
                let global_folder = get_yarn_berry_global_folder(app);
                folders.push(global_folder);
                if get_yarn_berry_node_linker(app) == "node-modules" {
                    folders.push("/app/node_modules".to_string());
                }
                folders
            }
            _ => vec!["/app/node_modules".to_string()],
        }
    }

    /// 安装依赖命令
    pub fn install_deps(
        &self,
        app: &App,
        caches: &mut CacheContext,
        install: &mut CommandStepBuilder,
        using_corepack: bool,
    ) {
        let cache_name = self.get_install_cache(caches);
        install.add_cache(&cache_name);

        match self {
            PackageManagerKind::Npm => {
                let has_lockfile = app.has_file("package-lock.json");
                if has_lockfile {
                    install.add_command(Command::new_exec("npm ci"));
                } else {
                    install.add_command(Command::new_exec("npm install"));
                }
            }
            PackageManagerKind::Pnpm => {
                if !using_corepack {
                    install.add_variables(&HashMap::from([(
                        "PNPM_HOME".to_string(),
                        "/pnpm".to_string(),
                    )]));
                    install.add_paths(&["/pnpm".to_string()]);
                    install.add_command(Command::new_exec("pnpm add -g node-gyp"));
                }
                let has_lockfile = app.has_file("pnpm-lock.yaml");
                if has_lockfile {
                    install.add_command(Command::new_exec(
                        "pnpm install --frozen-lockfile --prefer-offline",
                    ));
                } else {
                    install.add_command(Command::new_exec("pnpm install"));
                }
            }
            PackageManagerKind::Bun => {
                let has_lockfile = app.has_file("bun.lockb") || app.has_file("bun.lock");
                if has_lockfile {
                    install.add_command(Command::new_exec("bun install --frozen-lockfile"));
                } else {
                    install.add_command(Command::new_exec("bun install"));
                }
            }
            PackageManagerKind::Yarn1 => {
                let has_lockfile = app.has_file("yarn.lock");
                if has_lockfile {
                    install.add_command(Command::new_exec("yarn install --frozen-lockfile"));
                } else {
                    install.add_command(Command::new_exec("yarn install"));
                }
            }
            PackageManagerKind::YarnBerry => {
                install.add_command(Command::new_exec("yarn install --check-cache"));
            }
        }
    }

    /// 安装包管理器特定版本到 mise
    pub fn get_package_manager_packages(
        &self,
        app: &App,
        package_json: &PackageJson,
        mise_step: &mut MiseStepBuilder,
        resolver: &mut Resolver,
    ) {
        let (pm_name, pm_version) = package_json.get_package_manager_info();

        match self {
            PackageManagerKind::Pnpm => {
                let pnpm = mise_step.default_package(resolver, "pnpm", DEFAULT_PNPM_VERSION);

                // engines 字段优先
                if let Some(engine_version) = package_json.engines.get("pnpm") {
                    if !engine_version.is_empty() {
                        mise_step.version(
                            resolver,
                            &pnpm,
                            engine_version,
                            "package.json > engines > pnpm",
                        );
                    }
                }

                // pnpm-lock.yaml 版本推断
                if let Ok(lockfile) = app.read_file("pnpm-lock.yaml") {
                    if lockfile.starts_with("lockfileVersion: 5.3") {
                        mise_step.version(resolver, &pnpm, "6", "pnpm-lock.yaml");
                    } else if lockfile.starts_with("lockfileVersion: 5.4") {
                        mise_step.version(resolver, &pnpm, "7", "pnpm-lock.yaml");
                    } else if lockfile.starts_with("lockfileVersion: '6.0'") {
                        mise_step.version(resolver, &pnpm, "8", "pnpm-lock.yaml");
                    }
                }

                if pm_name == "pnpm" && !pm_version.is_empty() {
                    mise_step.version(
                        resolver,
                        &pnpm,
                        &pm_version,
                        "package.json > packageManager",
                    );
                    mise_step.skip_mise_install(resolver, &pnpm);
                }
            }
            PackageManagerKind::Yarn1 | PackageManagerKind::YarnBerry => {
                let default_major = if *self == PackageManagerKind::Yarn1 {
                    "1"
                } else {
                    "2"
                };
                let yarn = mise_step.default_package(resolver, "yarn", default_major);

                if *self == PackageManagerKind::Yarn1 {
                    mise_step.add_supporting_apt_package("tar");
                    mise_step.add_supporting_apt_package("gpg");
                }

                if let Some(engine_version) = package_json.engines.get("yarn") {
                    if !engine_version.is_empty() {
                        mise_step.version(
                            resolver,
                            &yarn,
                            engine_version,
                            "package.json > engines > yarn",
                        );
                    }
                }

                if pm_name == "yarn" && !pm_version.is_empty() {
                    let major = pm_version.split('.').next().unwrap_or("1");
                    let yarn = mise_step.default_package(resolver, "yarn", major);
                    mise_step.version(
                        resolver,
                        &yarn,
                        &pm_version,
                        "package.json > packageManager",
                    );
                    mise_step.skip_mise_install(resolver, &yarn);
                }
            }
            PackageManagerKind::Bun => {
                let bun = mise_step.default_package(resolver, "bun", "latest");

                if let Some(engine_version) = package_json.engines.get("bun") {
                    if !engine_version.is_empty() {
                        mise_step.version(
                            resolver,
                            &bun,
                            engine_version,
                            "package.json > engines > bun",
                        );
                    }
                }

                if pm_name == "bun" && !pm_version.is_empty() {
                    mise_step.version(resolver, &bun, &pm_version, "package.json > packageManager");
                }
            }
            PackageManagerKind::Npm => {
                // npm 通过 Node.js 自带，不需要额外安装
            }
        }
    }

    /// 获取安装依赖所需的支持文件列表
    pub fn supporting_install_files(&self, app: &App) -> Vec<String> {
        let pattern = "**/{package.json,package-lock.json,pnpm-workspace.yaml,yarn.lock,pnpm-lock.yaml,bun.lockb,bun.lock,.yarn,.pnp.*,.yarnrc.yml,.npmrc,.node-version,.nvmrc,patches,.pnpm-patches,prisma}";

        let mut all_files = Vec::new();

        if let Ok(files) = app.find_files(pattern) {
            for file in files {
                if !file.starts_with("node_modules/") {
                    all_files.push(file);
                }
            }
        }

        if let Ok(dirs) = app.find_directories(pattern) {
            all_files.extend(dirs);
        }

        all_files
    }
}

impl std::fmt::Display for PackageManagerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// .yarnrc.yml 配置结构体
#[derive(serde::Deserialize, Default)]
struct YarnRc {
    #[serde(rename = "globalFolder", default)]
    global_folder: Option<String>,
    #[serde(rename = "nodeLinker", default)]
    node_linker: Option<String>,
}

/// Yarn Berry 全局目录
fn get_yarn_berry_global_folder(app: &App) -> String {
    if let Ok(rc) = app.read_yaml::<YarnRc>(".yarnrc.yml") {
        if let Some(ref folder) = rc.global_folder {
            if !folder.is_empty() {
                return folder.clone();
            }
        }
    }
    "/root/.yarn".to_string()
}

/// Yarn Berry node linker 设置
fn get_yarn_berry_node_linker(app: &App) -> String {
    if let Ok(rc) = app.read_yaml::<YarnRc>(".yarnrc.yml") {
        if let Some(ref linker) = rc.node_linker {
            if !linker.is_empty() {
                return linker.clone();
            }
        }
    }
    "pnp".to_string()
}

/// 从 packageManager 版本字符串推断 Yarn 版本
pub fn parse_yarn_package_manager(version: &str) -> PackageManagerKind {
    if version.starts_with("1.") || version == "1" {
        PackageManagerKind::Yarn1
    } else {
        PackageManagerKind::YarnBerry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_manager_name() {
        assert_eq!(PackageManagerKind::Npm.name(), "npm");
        assert_eq!(PackageManagerKind::Pnpm.name(), "pnpm");
        assert_eq!(PackageManagerKind::Bun.name(), "bun");
        assert_eq!(PackageManagerKind::Yarn1.name(), "yarn");
        assert_eq!(PackageManagerKind::YarnBerry.name(), "yarn");
    }

    #[test]
    fn test_run_cmd() {
        assert_eq!(PackageManagerKind::Npm.run_cmd("build"), "npm run build");
        assert_eq!(PackageManagerKind::Pnpm.run_cmd("start"), "pnpm run start");
    }

    #[test]
    fn test_run_script_command() {
        assert_eq!(
            PackageManagerKind::Npm.run_script_command("index.js"),
            "node index.js"
        );
        assert_eq!(
            PackageManagerKind::Bun.run_script_command("index.ts"),
            "bun index.ts"
        );
    }

    #[test]
    fn test_lockfile_name() {
        assert_eq!(PackageManagerKind::Npm.lockfile_name(), "package-lock.json");
        assert_eq!(PackageManagerKind::Pnpm.lockfile_name(), "pnpm-lock.yaml");
        assert_eq!(PackageManagerKind::Bun.lockfile_name(), "bun.lockb");
        assert_eq!(PackageManagerKind::Yarn1.lockfile_name(), "yarn.lock");
    }

    #[test]
    fn test_parse_yarn_package_manager() {
        assert_eq!(
            parse_yarn_package_manager("1.22.0"),
            PackageManagerKind::Yarn1
        );
        assert_eq!(
            parse_yarn_package_manager("2.0.0"),
            PackageManagerKind::YarnBerry
        );
        assert_eq!(
            parse_yarn_package_manager("4.0.0"),
            PackageManagerKind::YarnBerry
        );
    }

    #[test]
    fn test_cache_type() {
        assert_eq!(PackageManagerKind::Npm.cache_type(), CacheType::Shared);
        assert_eq!(PackageManagerKind::Yarn1.cache_type(), CacheType::Locked);
        assert_eq!(
            PackageManagerKind::YarnBerry.cache_type(),
            CacheType::Shared
        );
    }

    /// 辅助函数：将 commands 序列化为可搜索字符串
    fn commands_debug_str(install: &CommandStepBuilder) -> String {
        install
            .commands
            .iter()
            .map(|c| format!("{:?}", c))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn test_bun_install_with_lockfile_uses_frozen() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("bun.lockb"), "").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();

        let mut caches = CacheContext::new();
        let mut install = CommandStepBuilder::new("install");
        PackageManagerKind::Bun.install_deps(&app, &mut caches, &mut install, false);

        assert!(
            commands_debug_str(&install).contains("--frozen-lockfile"),
            "应使用 --frozen-lockfile"
        );
    }

    #[test]
    fn test_bun_install_without_lockfile_no_frozen() {
        let dir = tempfile::TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();

        let mut caches = CacheContext::new();
        let mut install = CommandStepBuilder::new("install");
        PackageManagerKind::Bun.install_deps(&app, &mut caches, &mut install, false);

        assert!(
            !commands_debug_str(&install).contains("--frozen-lockfile"),
            "无锁文件时不应使用 --frozen-lockfile"
        );
    }

    #[test]
    fn test_yarn1_install_with_lockfile_uses_frozen() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("yarn.lock"), "").unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();

        let mut caches = CacheContext::new();
        let mut install = CommandStepBuilder::new("install");
        PackageManagerKind::Yarn1.install_deps(&app, &mut caches, &mut install, false);

        assert!(
            commands_debug_str(&install).contains("--frozen-lockfile"),
            "应使用 --frozen-lockfile"
        );
    }

    #[test]
    fn test_yarn1_install_without_lockfile_no_frozen() {
        let dir = tempfile::TempDir::new().unwrap();
        let app = App::new(dir.path().to_str().unwrap()).unwrap();

        let mut caches = CacheContext::new();
        let mut install = CommandStepBuilder::new("install");
        PackageManagerKind::Yarn1.install_deps(&app, &mut caches, &mut install, false);

        assert!(
            !commands_debug_str(&install).contains("--frozen-lockfile"),
            "无锁文件时不应使用 --frozen-lockfile"
        );
    }
}
