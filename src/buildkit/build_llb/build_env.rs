use std::collections::HashMap;

/// 构建环境 —— 累积环境变量和 PATH 条目
///
/// 对齐 railpack `build_env.go`
#[derive(Debug, Clone, Default)]
pub struct BuildEnvironment {
    /// PATH 条目列表（按 push 顺序排列）
    pub path_list: Vec<String>,
    /// 环境变量映射
    pub env_vars: HashMap<String, String>,
}

impl BuildEnvironment {
    /// 创建空环境
    pub fn new() -> Self {
        Self::default()
    }

    /// 合并另一个环境：追加 path_list（去重），深拷贝合并 env_vars
    pub fn merge(&mut self, other: &BuildEnvironment) {
        // 追加 path_list，跳过已存在的
        for path in &other.path_list {
            if !self.path_list.contains(path) {
                self.path_list.push(path.clone());
            }
        }
        // 合并 env_vars（后者覆盖前者）
        for (key, value) in &other.env_vars {
            self.env_vars.insert(key.clone(), value.clone());
        }
    }

    /// 添加 PATH 条目（去重后 push 到末尾）
    ///
    /// 对齐 railpack 行为——先检查是否已存在，push 到末尾
    pub fn push_path(&mut self, path: impl Into<String>) {
        let path = path.into();
        if !self.path_list.contains(&path) {
            self.path_list.push(path);
        }
    }

    /// 设置环境变量
    pub fn add_env_var(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.env_vars.insert(key.into(), value.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_creates_empty_environment() {
        let env = BuildEnvironment::new();
        assert!(env.path_list.is_empty(), "path_list should be empty");
        assert!(env.env_vars.is_empty(), "env_vars should be empty");
    }

    #[test]
    fn test_push_path_deduplicates() {
        let mut env = BuildEnvironment::new();
        env.push_path("/a");
        env.push_path("/a");
        assert_eq!(env.path_list.len(), 1, "duplicate path should not be added");
        assert_eq!(env.path_list[0], "/a");
    }

    #[test]
    fn test_push_path_preserves_order() {
        let mut env = BuildEnvironment::new();
        env.push_path("/a");
        env.push_path("/b");
        assert_eq!(env.path_list, vec!["/a", "/b"]);
    }

    #[test]
    fn test_merge_deep_copy() {
        // 合并后修改源环境，目标环境不受影响
        let mut target = BuildEnvironment::new();
        target.push_path("/target");
        target.add_env_var("KEY", "target_value");

        let mut source = BuildEnvironment::new();
        source.push_path("/source");
        source.add_env_var("SRC_KEY", "src_value");

        target.merge(&source);

        // 修改源环境
        source.push_path("/new_source");
        source.add_env_var("SRC_KEY", "modified");

        // 目标环境不受影响
        assert_eq!(target.path_list, vec!["/target", "/source"]);
        assert_eq!(target.env_vars.get("SRC_KEY").unwrap(), "src_value");
        assert!(!target.path_list.contains(&"/new_source".to_string()));
    }

    #[test]
    fn test_merge_env_vars_override() {
        let mut target = BuildEnvironment::new();
        target.add_env_var("KEY", "old_value");
        target.add_env_var("KEEP", "keep_value");

        let mut source = BuildEnvironment::new();
        source.add_env_var("KEY", "new_value");

        target.merge(&source);

        assert_eq!(
            target.env_vars.get("KEY").unwrap(),
            "new_value",
            "later env vars should override earlier ones"
        );
        assert_eq!(
            target.env_vars.get("KEEP").unwrap(),
            "keep_value",
            "unrelated env vars should remain unchanged"
        );
    }
}
