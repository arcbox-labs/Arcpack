/// Procfile Provider：后处理 Provider，覆盖 start command
///
/// 对齐 railpack `core/providers/procfile/procfile.go` + `core/core.go`
/// 特殊说明：Procfile 不在 get_all_providers() 注册表中。
/// 它始终在主 Provider plan() 之后独立运行，仅用于覆盖 start command。

use crate::generate::GenerateContext;
use crate::Result;

/// Procfile Provider
pub struct ProcfileProvider;

impl ProcfileProvider {
    pub fn new() -> Self {
        Self
    }

    /// 解析 Procfile 内容，返回 Vec<(进程名, 命令)>
    fn parse_procfile(content: &str) -> Vec<(String, String)> {
        let mut entries = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim().to_string();
                let value = line[colon_pos + 1..].trim().to_string();
                if !key.is_empty() && !value.is_empty() {
                    entries.push((key, value));
                }
            }
        }
        entries
    }

    /// 从 Procfile 条目中按优先级选择启动命令
    /// 优先级：web > worker > 第一个
    fn select_command(entries: &[(String, String)]) -> Option<String> {
        if entries.is_empty() {
            return None;
        }
        // 优先 web
        if let Some((_, cmd)) = entries.iter().find(|(k, _)| k == "web") {
            return Some(cmd.clone());
        }
        // 其次 worker
        if let Some((_, cmd)) = entries.iter().find(|(k, _)| k == "worker") {
            return Some(cmd.clone());
        }
        // 回退第一个
        Some(entries[0].1.clone())
    }

    /// 执行 Procfile 计划（在主 Provider plan() 之后调用）
    pub fn plan(&self, ctx: &mut GenerateContext) -> Result<()> {
        if !ctx.app.has_file("Procfile") {
            return Ok(());
        }

        let content = ctx.app.read_file("Procfile")?;
        let entries = Self::parse_procfile(&content);

        if let Some(cmd) = Self::select_command(&entries) {
            ctx.deploy.start_cmd = Some(cmd);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::app::environment::Environment;
    use crate::config::Config;
    use crate::generate::GenerateContext;
    use crate::resolver::VersionResolver;
    use std::collections::HashMap;
    use std::fs;
    use tempfile::TempDir;

    struct MockVersionResolver;
    impl VersionResolver for MockVersionResolver {
        fn get_latest_version(&self, _pkg: &str, version: &str) -> Result<String> {
            Ok(format!("{}.0.0", version))
        }
        fn get_all_versions(&self, _pkg: &str, _version: &str) -> Result<Vec<String>> {
            Ok(vec!["1.0.0".to_string()])
        }
    }

    fn make_ctx(dir: &TempDir) -> GenerateContext {
        let app = App::new(dir.path().to_str().unwrap()).unwrap();
        let env = Environment::new(HashMap::new());
        let config = Config::empty();
        GenerateContext::new(app, env, config, Box::new(MockVersionResolver)).unwrap()
    }

    // === 解析测试 ===

    #[test]
    fn test_parse_procfile_basic() {
        let entries = ProcfileProvider::parse_procfile("web: node server.js\nworker: node worker.js");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], ("web".to_string(), "node server.js".to_string()));
        assert_eq!(
            entries[1],
            ("worker".to_string(), "node worker.js".to_string())
        );
    }

    #[test]
    fn test_parse_procfile_ignores_comments_and_empty() {
        let entries = ProcfileProvider::parse_procfile("# comment\n\nweb: node app.js\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "web");
    }

    #[test]
    fn test_parse_procfile_trims_whitespace() {
        let entries = ProcfileProvider::parse_procfile("  web:  node server.js  ");
        assert_eq!(entries[0], ("web".to_string(), "node server.js".to_string()));
    }

    // === 优先级测试 ===

    #[test]
    fn test_select_command_web_priority() {
        let entries = vec![
            ("worker".to_string(), "node worker.js".to_string()),
            ("web".to_string(), "node server.js".to_string()),
        ];
        assert_eq!(
            ProcfileProvider::select_command(&entries),
            Some("node server.js".to_string())
        );
    }

    #[test]
    fn test_select_command_worker_fallback() {
        let entries = vec![
            ("worker".to_string(), "node worker.js".to_string()),
            ("cron".to_string(), "node cron.js".to_string()),
        ];
        assert_eq!(
            ProcfileProvider::select_command(&entries),
            Some("node worker.js".to_string())
        );
    }

    #[test]
    fn test_select_command_first_fallback() {
        let entries = vec![
            ("cron".to_string(), "node cron.js".to_string()),
            ("scheduler".to_string(), "node sched.js".to_string()),
        ];
        assert_eq!(
            ProcfileProvider::select_command(&entries),
            Some("node cron.js".to_string())
        );
    }

    #[test]
    fn test_select_command_empty() {
        assert_eq!(ProcfileProvider::select_command(&[]), None);
    }

    // === plan 测试 ===

    #[test]
    fn test_plan_no_procfile_noop() {
        let dir = TempDir::new().unwrap();
        let mut ctx = make_ctx(&dir);
        ctx.deploy.start_cmd = Some("original".to_string());

        let provider = ProcfileProvider::new();
        provider.plan(&mut ctx).unwrap();

        // 无 Procfile，不应修改
        assert_eq!(ctx.deploy.start_cmd.as_deref(), Some("original"));
    }

    #[test]
    fn test_plan_overrides_start_cmd() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Procfile"), "web: node server.js").unwrap();
        let mut ctx = make_ctx(&dir);
        ctx.deploy.start_cmd = Some("original".to_string());

        let provider = ProcfileProvider::new();
        provider.plan(&mut ctx).unwrap();

        assert_eq!(
            ctx.deploy.start_cmd.as_deref(),
            Some("node server.js")
        );
    }

    #[test]
    fn test_plan_sets_start_cmd_when_none() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Procfile"), "worker: node worker.js").unwrap();
        let mut ctx = make_ctx(&dir);

        let provider = ProcfileProvider::new();
        provider.plan(&mut ctx).unwrap();

        assert_eq!(
            ctx.deploy.start_cmd.as_deref(),
            Some("node worker.js")
        );
    }

    // === 不在注册表中 ===

    #[test]
    fn test_procfile_not_in_get_all_providers() {
        let providers = crate::provider::get_all_providers();
        let names: Vec<&str> = providers.iter().map(|p| p.name()).collect();
        assert!(
            !names.contains(&"procfile"),
            "Procfile 不应在 get_all_providers() 中"
        );
    }
}
