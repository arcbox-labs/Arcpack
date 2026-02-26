/// DockerignoreContext —— .dockerignore 解析
///
/// 对齐 railpack `core/plan/dockerignore.go`。
/// 解析 .dockerignore 文件，分离 exclude 和 include（! 否定）模式。
use std::collections::HashSet;
use std::path::Path;

use crate::Result;

/// .dockerignore 上下文
#[derive(Debug, Clone)]
pub struct DockerignoreContext {
    pub excludes: Vec<String>,
    pub includes: Vec<String>,
    pub has_file: bool,
}

impl DockerignoreContext {
    /// 从源码目录构造 DockerignoreContext
    pub fn new(source: &Path) -> Result<Self> {
        let dockerignore_path = source.join(".dockerignore");

        if !dockerignore_path.exists() {
            return Ok(Self {
                excludes: Vec::new(),
                includes: Vec::new(),
                has_file: false,
            });
        }

        let content = std::fs::read_to_string(&dockerignore_path)?;
        let (excludes, includes) = parse_dockerignore(&content);

        Ok(Self {
            excludes: dedup(excludes),
            includes: dedup(includes),
            has_file: true,
        })
    }
}

/// 解析 .dockerignore 内容，分离 exclude 和 include 模式
///
/// 规则：
/// - 空行和 # 开头的注释行被忽略
/// - ! 开头的行为 include（否定模式）
/// - 其余为 exclude 模式
///
/// 注意：分离为两个独立列表后，原始规则的相对顺序信息会丢失。
/// Docker 原生语义中，后出现的规则覆盖先出现的规则（如 exclude 后跟 !include 再跟 exclude）。
/// 此为对齐 railpack `separatePatterns` 的设计——BuildKit 接口分别接收 exclude/include 列表，
/// 故此处同样拆分。
fn parse_dockerignore(content: &str) -> (Vec<String>, Vec<String>) {
    let mut excludes = Vec::new();
    let mut includes = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // 跳过空行和注释
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if let Some(pattern) = trimmed.strip_prefix('!') {
            // ! 否定模式 → include
            let pattern = pattern.trim();
            if !pattern.is_empty() {
                includes.push(pattern.to_string());
            }
        } else {
            excludes.push(trimmed.to_string());
        }
    }

    (excludes, includes)
}

/// 去重（保持顺序）
fn dedup(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_dockerignore_no_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        let ctx = DockerignoreContext::new(dir.path()).unwrap();
        assert!(!ctx.has_file);
        assert!(ctx.excludes.is_empty());
        assert!(ctx.includes.is_empty());
    }

    #[test]
    fn test_dockerignore_basic_excludes() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join(".dockerignore"),
            "node_modules\n.git\n*.log\n",
        )
        .unwrap();

        let ctx = DockerignoreContext::new(dir.path()).unwrap();
        assert!(ctx.has_file);
        assert_eq!(ctx.excludes, vec!["node_modules", ".git", "*.log"]);
        assert!(ctx.includes.is_empty());
    }

    #[test]
    fn test_dockerignore_negation_patterns() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join(".dockerignore"),
            "**/*.md\n!README.md\nnode_modules\n!important.log\n",
        )
        .unwrap();

        let ctx = DockerignoreContext::new(dir.path()).unwrap();
        assert_eq!(ctx.excludes, vec!["**/*.md", "node_modules"]);
        assert_eq!(ctx.includes, vec!["README.md", "important.log"]);
    }

    #[test]
    fn test_dockerignore_comments_and_empty_lines_skipped() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join(".dockerignore"),
            "# 这是注释\n\nnode_modules\n\n# 另一个注释\n.git\n",
        )
        .unwrap();

        let ctx = DockerignoreContext::new(dir.path()).unwrap();
        assert_eq!(ctx.excludes, vec!["node_modules", ".git"]);
    }

    #[test]
    fn test_dockerignore_deduplicates_patterns() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join(".dockerignore"),
            "node_modules\n.git\nnode_modules\n.git\n",
        )
        .unwrap();

        let ctx = DockerignoreContext::new(dir.path()).unwrap();
        assert_eq!(ctx.excludes, vec!["node_modules", ".git"]);
    }

    #[test]
    fn test_dockerignore_glob_patterns() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join(".dockerignore"),
            "**/*.test.js\n**/dist\n!dist/important.js\n",
        )
        .unwrap();

        let ctx = DockerignoreContext::new(dir.path()).unwrap();
        assert_eq!(ctx.excludes, vec!["**/*.test.js", "**/dist"]);
        assert_eq!(ctx.includes, vec!["dist/important.js"]);
    }
}
