/// App —— 源码目录的只读文件系统抽象
///
/// 对齐 railpack `core/app/app.go`。
/// 封装源码目录的只读访问，提供 glob 缓存。
pub mod environment;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use globset::GlobBuilder;
use regex::Regex;
use serde::de::DeserializeOwned;
use walkdir::WalkDir;

use crate::error::ArcpackError;
use crate::Result;

pub use environment::Environment;

pub struct App {
    /// 源码根目录（绝对路径）
    source: PathBuf,
    /// glob 匹配结果缓存
    glob_cache: Mutex<HashMap<String, Vec<String>>>,
}

impl App {
    /// 构造 App，接收源码根目录路径
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let source = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()?.join(&path)
        };

        // 规范化路径
        let source = source
            .canonicalize()
            .map_err(|_| ArcpackError::SourceNotAccessible {
                path: source.display().to_string(),
            })?;

        if !source.is_dir() {
            return Err(ArcpackError::SourceNotAccessible {
                path: source.display().to_string(),
            });
        }

        Ok(Self {
            source,
            glob_cache: Mutex::new(HashMap::new()),
        })
    }

    /// 获取源码根目录
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// 检查文件是否存在
    pub fn has_file(&self, path: &str) -> bool {
        self.source.join(path).exists()
    }

    /// glob 匹配是否有结果（文件或目录）
    ///
    /// 对齐 railpack `HasMatch`：glob 出错时返回 false 而非传播错误。
    /// 错误通过 tracing::warn 记录，便于调试。
    pub fn has_match(&self, pattern: &str) -> bool {
        let files = match self.find_files(pattern) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(pattern, error = %e, "has_match: glob 文件匹配失败");
                return false;
            }
        };
        if !files.is_empty() {
            return true;
        }
        let dirs = match self.find_directories(pattern) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(pattern, error = %e, "has_match: glob 目录匹配失败");
                return false;
            }
        };
        !dirs.is_empty()
    }

    /// 读取文本文件（规范化换行符）
    pub fn read_file(&self, name: &str) -> Result<String> {
        let path = self.source.join(name);
        let data = std::fs::read_to_string(&path)?;
        Ok(data.replace("\r\n", "\n"))
    }

    /// 读取并解析 JSON 文件（支持 JSONC：单行 // 和多行 /* */ 注释）
    ///
    /// 对齐 railpack 使用 hujson 的行为。此处用状态机实现注释剥离，
    /// 正确处理字符串中的转义引号（`\"`）和嵌套转义。
    pub fn read_json<T: DeserializeOwned>(&self, name: &str) -> Result<T> {
        let content = self.read_file(name)?;
        let cleaned = strip_jsonc_comments(&content);
        let cleaned = strip_trailing_commas(&cleaned);

        serde_json::from_str(&cleaned).map_err(|e| ArcpackError::ConfigParse {
            path: name.to_string(),
            message: e.to_string(),
        })
    }

    /// 读取并解析 YAML 文件
    pub fn read_yaml<T: DeserializeOwned>(&self, name: &str) -> Result<T> {
        let content = self.read_file(name)?;
        serde_yaml::from_str(&content).map_err(|e| ArcpackError::ConfigParse {
            path: name.to_string(),
            message: e.to_string(),
        })
    }

    /// 读取并解析 TOML 文件
    pub fn read_toml<T: DeserializeOwned>(&self, name: &str) -> Result<T> {
        let content = self.read_file(name)?;
        toml::from_str(&content).map_err(|e| ArcpackError::ConfigParse {
            path: name.to_string(),
            message: e.to_string(),
        })
    }

    /// glob 匹配文件（结果缓存）
    pub fn find_files(&self, pattern: &str) -> Result<Vec<String>> {
        self.find_matches(pattern, false)
    }

    /// glob 匹配目录
    pub fn find_directories(&self, pattern: &str) -> Result<Vec<String>> {
        self.find_matches(pattern, true)
    }

    /// glob + 内容正则双重匹配
    ///
    /// 对齐 railpack `FindFilesWithContent`：出错时返回空列表。
    /// 错误通过 tracing::warn 记录，便于调试。
    pub fn find_files_with_content(&self, pattern: &str, regex: &Regex) -> Vec<String> {
        let files = match self.find_files(pattern) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(pattern, error = %e, "find_files_with_content: glob 匹配失败");
                return Vec::new();
            }
        };

        files
            .into_iter()
            .filter(|file| match self.read_file(file) {
                Ok(content) => regex.is_match(&content),
                Err(e) => {
                    tracing::warn!(file, error = %e, "find_files_with_content: 读取文件失败");
                    false
                }
            })
            .collect()
    }

    /// 检查文件是否有可执行权限
    pub fn is_file_executable(&self, name: &str) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = self.source.join(name);
            if let Ok(meta) = std::fs::metadata(&path) {
                if meta.is_file() {
                    return meta.permissions().mode() & 0o111 != 0;
                }
            }
            false
        }
        #[cfg(not(unix))]
        {
            let _ = name;
            false
        }
    }

    /// 内部方法：glob 匹配（带缓存）
    fn find_matches(&self, pattern: &str, is_dir: bool) -> Result<Vec<String>> {
        let all_matches = self.find_glob(pattern)?;

        let mut result = Vec::new();
        for m in all_matches {
            let full_path = self.source.join(&m);
            if let Ok(meta) = std::fs::metadata(&full_path) {
                if meta.is_dir() == is_dir {
                    result.push(m);
                }
            }
        }

        Ok(result)
    }

    /// 内部方法：执行 glob 匹配并缓存结果
    fn find_glob(&self, pattern: &str) -> Result<Vec<String>> {
        {
            let cache = self.glob_cache.lock().unwrap();
            if let Some(cached) = cache.get(pattern) {
                return Ok(cached.clone());
            }
        }

        let glob = GlobBuilder::new(pattern)
            .literal_separator(false)
            .build()
            .map_err(|e| ArcpackError::Other(anyhow::anyhow!("无效的 glob 模式: {}", e)))?;
        let matcher = glob.compile_matcher();

        let mut matches = Vec::new();
        for entry in WalkDir::new(&self.source)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if let Ok(relative) = path.strip_prefix(&self.source) {
                let relative_str = relative.to_string_lossy().to_string();
                if !relative_str.is_empty() && matcher.is_match(&relative_str) {
                    matches.push(relative_str);
                }
            }
        }

        matches.sort();

        {
            let mut cache = self.glob_cache.lock().unwrap();
            cache.insert(pattern.to_string(), matches.clone());
        }

        Ok(matches)
    }
}

/// 移除 JSONC 注释（单行 `//` 和多行 `/* */`）
///
/// 使用状态机正确处理字符串上下文和转义序列（`\"`），
/// 避免截断字符串内的 `//` 或 `/*`。
fn strip_jsonc_comments(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;

    while let Some(&ch) = chars.peek() {
        if in_string {
            if ch == '\\' {
                // 转义序列：原样保留两个字符
                result.push(chars.next().unwrap());
                if let Some(&next) = chars.peek() {
                    result.push(chars.next().unwrap());
                    let _ = next;
                }
            } else if ch == '"' {
                result.push(chars.next().unwrap());
                in_string = false;
            } else {
                result.push(chars.next().unwrap());
            }
        } else {
            if ch == '"' {
                result.push(chars.next().unwrap());
                in_string = true;
            } else if ch == '/' {
                chars.next(); // 消费 '/'
                match chars.peek() {
                    Some(&'/') => {
                        // 单行注释：跳到行尾
                        for c in chars.by_ref() {
                            if c == '\n' {
                                result.push('\n');
                                break;
                            }
                        }
                    }
                    Some(&'*') => {
                        // 多行注释：跳到 */
                        chars.next(); // 消费 '*'
                        let mut prev = '\0';
                        for c in chars.by_ref() {
                            if prev == '*' && c == '/' {
                                break;
                            }
                            prev = c;
                        }
                    }
                    _ => {
                        // 普通 '/' 字符
                        result.push('/');
                    }
                }
            } else {
                result.push(chars.next().unwrap());
            }
        }
    }

    result
}

/// 移除 JSON 中的尾随逗号（HuJSON/JSONC 兼容）
///
/// 匹配 `},` → `}`, `],` → `]` 以及值后面紧跟 `}` 或 `]` 前的逗号。
/// 在字符串内部的逗号不受影响。
fn strip_trailing_commas(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_string = false;

    while i < len {
        let ch = chars[i];

        if in_string {
            if ch == '\\' && i + 1 < len {
                result.push(ch);
                result.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if ch == '"' {
                in_string = false;
            }
            result.push(ch);
            i += 1;
            continue;
        }

        if ch == '"' {
            in_string = true;
            result.push(ch);
            i += 1;
            continue;
        }

        if ch == ',' {
            // 向前查看下一个非空白字符是否为 } 或 ]
            let mut j = i + 1;
            while j < len && chars[j].is_whitespace() {
                j += 1;
            }
            if j < len && (chars[j] == '}' || chars[j] == ']') {
                // 跳过尾随逗号，保留空白
                i += 1;
                continue;
            }
        }

        result.push(ch);
        i += 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// 创建测试用临时目录和文件
    fn setup_test_app() -> (TempDir, App) {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"name":"test"}"#).unwrap();
        std::fs::write(dir.path().join("README.md"), "# Test").unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/index.ts"), "console.log('hello')").unwrap();

        let app = App::new(dir.path()).unwrap();
        (dir, app)
    }

    #[test]
    fn test_app_new_valid_directory() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path());
        assert!(app.is_ok());
    }

    #[test]
    fn test_app_new_nonexistent_directory_returns_error() {
        let result = App::new("/nonexistent/path/that/does/not/exist");
        assert!(result.is_err());
    }

    #[test]
    fn test_has_file_existing() {
        let (_dir, app) = setup_test_app();
        assert!(app.has_file("package.json"));
        assert!(!app.has_file("nonexistent.txt"));
    }

    #[test]
    fn test_read_file_normalizes_line_endings() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "line1\r\nline2\r\n").unwrap();
        let app = App::new(dir.path()).unwrap();

        let content = app.read_file("test.txt").unwrap();
        assert_eq!(content, "line1\nline2\n");
    }

    #[test]
    fn test_read_json_parses_file() {
        let (_dir, app) = setup_test_app();
        let value: serde_json::Value = app.read_json("package.json").unwrap();
        assert_eq!(value.get("name").unwrap(), "test");
    }

    #[test]
    fn test_find_files_matches_glob_pattern() {
        let (_dir, app) = setup_test_app();
        let files = app.find_files("**/*.ts").unwrap();
        assert!(files.contains(&"src/index.ts".to_string()));
    }

    #[test]
    fn test_find_directories() {
        let (_dir, app) = setup_test_app();
        let dirs = app.find_directories("src").unwrap();
        assert!(dirs.contains(&"src".to_string()));
    }

    #[test]
    fn test_has_match() {
        let (_dir, app) = setup_test_app();
        assert!(app.has_match("**/*.json"));
        assert!(!app.has_match("**/*.py"));
    }

    #[test]
    fn test_glob_cache_returns_same_result() {
        let (_dir, app) = setup_test_app();

        let first = app.find_files("**/*.ts").unwrap();
        let second = app.find_files("**/*.ts").unwrap();
        assert_eq!(first, second);

        // 验证缓存确实被使用
        let cache = app.glob_cache.lock().unwrap();
        assert!(cache.contains_key("**/*.ts"));
    }

    #[test]
    fn test_find_files_with_content() {
        let (_dir, app) = setup_test_app();
        let regex = Regex::new("console").unwrap();
        let matches = app.find_files_with_content("**/*.ts", &regex);
        assert!(matches.contains(&"src/index.ts".to_string()));
    }

    #[test]
    fn test_is_file_executable() {
        let dir = TempDir::new().unwrap();
        let script_path = dir.path().join("run.sh");
        std::fs::write(&script_path, "#!/bin/bash\necho hello").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let app = App::new(dir.path()).unwrap();
        assert!(app.is_file_executable("run.sh"));
        assert!(!app.is_file_executable("nonexistent.sh"));
    }

    #[test]
    fn test_source_returns_path() {
        let dir = TempDir::new().unwrap();
        let app = App::new(dir.path()).unwrap();
        assert!(app.source().is_dir());
    }

    #[test]
    fn test_strip_jsonc_single_line_comments() {
        let input = r#"{
  // 这是注释
  "key": "value"
}"#;
        let result = strip_jsonc_comments(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["key"], "value");
    }

    #[test]
    fn test_strip_jsonc_multi_line_comments() {
        let input = r#"{
  /* 多行
     注释 */
  "key": "value"
}"#;
        let result = strip_jsonc_comments(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["key"], "value");
    }

    #[test]
    fn test_strip_jsonc_preserves_double_slash_in_strings() {
        // 字符串内的 // 不能被当作注释
        let input = r#"{"url": "https://example.com"}"#;
        let result = strip_jsonc_comments(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["url"], "https://example.com");
    }

    #[test]
    fn test_strip_jsonc_handles_escaped_quotes() {
        // 转义引号 \" 不能结束字符串上下文
        let input = r#"{"msg": "say \"hello\" // world"}"#;
        let result = strip_jsonc_comments(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["msg"], r#"say "hello" // world"#);
    }

    #[test]
    fn test_strip_jsonc_preserves_utf8_characters() {
        // 中文和 emoji 不能被字节级遍历破坏
        let input = r#"{
  // 注释
  "name": "你好世界",
  "emoji": "🚀🎉"
}"#;
        let result = strip_jsonc_comments(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["name"], "你好世界");
        assert_eq!(parsed["emoji"], "🚀🎉");
    }

    #[test]
    fn test_read_json_with_jsonc_comments() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            r#"{
  // 单行注释
  "name": "test",
  /* 多行注释 */
  "url": "https://example.com"
}"#,
        )
        .unwrap();

        let app = App::new(dir.path()).unwrap();
        let value: serde_json::Value = app.read_json("config.json").unwrap();
        assert_eq!(value["name"], "test");
        assert_eq!(value["url"], "https://example.com");
    }

    #[test]
    fn test_strip_trailing_commas_object() {
        let input = r#"{"a": 1, "b": 2,}"#;
        let result = strip_trailing_commas(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"], 1);
        assert_eq!(parsed["b"], 2);
    }

    #[test]
    fn test_strip_trailing_commas_array() {
        let input = r#"[1, 2, 3,]"#;
        let result = strip_trailing_commas(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_strip_trailing_commas_nested() {
        let input = r#"{"arr": [1, 2,], "obj": {"x": 1,},}"#;
        let result = strip_trailing_commas(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["arr"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["obj"]["x"], 1);
    }

    #[test]
    fn test_strip_trailing_commas_preserves_strings() {
        let input = r#"{"msg": "hello,}", "ok": true,}"#;
        let result = strip_trailing_commas(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["msg"], "hello,}");
        assert_eq!(parsed["ok"], true);
    }

    #[test]
    fn test_read_json_hujson_full() {
        // 测试注释 + 尾随逗号的完整 HuJSON 支持
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("test.json"),
            r#"{
  // comment
  "name": "test",
  "items": [1, 2,],
  "nested": {
    "key": "value", // inline comment
  },
}"#,
        )
        .unwrap();

        let app = App::new(dir.path()).unwrap();
        let value: serde_json::Value = app.read_json("test.json").unwrap();
        assert_eq!(value["name"], "test");
        assert_eq!(value["items"].as_array().unwrap().len(), 2);
        assert_eq!(value["nested"]["key"], "value");
    }
}
