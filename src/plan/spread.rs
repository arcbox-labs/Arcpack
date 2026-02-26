/// Spread 操作符 —— 处理配置数组中的 "..." 展开
///
/// 对齐 railpack `core/plan/spread.go`。
/// `"..."` 在 right 中表示保留 left 的值，在其前/后追加用户自定义项。

/// 可展开 trait，由 Command 和 Layer 实现
pub trait Spreadable {
    /// 是否为展开占位符（值为 "..."）
    fn is_spread(&self) -> bool;
}

/// 泛型展开函数
///
/// 遍历 `left`，当遇到 `is_spread() == true` 的元素时，将整个 `right` 插入该位置。
/// 如果 `left` 为空，直接返回 `right`。
pub fn spread<T: Spreadable + Clone>(left: Vec<T>, right: Vec<T>) -> Vec<T> {
    if left.is_empty() {
        return right;
    }

    let mut result = Vec::new();
    for item in &left {
        if item.is_spread() {
            result.extend(right.clone());
        } else {
            result.push(item.clone());
        }
    }
    result
}

/// 字符串便捷展开函数
///
/// `"..."` 字符串触发展开。
pub fn spread_strings(left: Vec<String>, right: Vec<String>) -> Vec<String> {
    if left.is_empty() {
        return right;
    }

    let mut result = Vec::new();
    for item in &left {
        if item == "..." {
            result.extend(right.clone());
        } else {
            result.push(item.clone());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试辅助：简单的 Spreadable 实现
    #[derive(Debug, Clone, PartialEq)]
    struct TestItem(String);

    impl Spreadable for TestItem {
        fn is_spread(&self) -> bool {
            self.0 == "..."
        }
    }

    #[test]
    fn test_spread_empty_left_returns_right() {
        let left: Vec<TestItem> = vec![];
        let right = vec![TestItem("a".into()), TestItem("b".into())];
        let result = spread(left, right.clone());
        assert_eq!(result, right);
    }

    #[test]
    fn test_spread_no_placeholder_returns_left_only() {
        let left = vec![TestItem("x".into()), TestItem("y".into())];
        let right = vec![TestItem("a".into()), TestItem("b".into())];
        let result = spread(left.clone(), right);
        assert_eq!(result, left);
    }

    #[test]
    fn test_spread_placeholder_at_head() {
        let left = vec![TestItem("...".into()), TestItem("x".into())];
        let right = vec![TestItem("a".into()), TestItem("b".into())];
        let result = spread(left, right);
        assert_eq!(
            result,
            vec![
                TestItem("a".into()),
                TestItem("b".into()),
                TestItem("x".into()),
            ]
        );
    }

    #[test]
    fn test_spread_placeholder_at_middle() {
        let left = vec![
            TestItem("x".into()),
            TestItem("...".into()),
            TestItem("y".into()),
        ];
        let right = vec![TestItem("a".into()), TestItem("b".into())];
        let result = spread(left, right);
        assert_eq!(
            result,
            vec![
                TestItem("x".into()),
                TestItem("a".into()),
                TestItem("b".into()),
                TestItem("y".into()),
            ]
        );
    }

    #[test]
    fn test_spread_placeholder_at_tail() {
        let left = vec![TestItem("x".into()), TestItem("...".into())];
        let right = vec![TestItem("a".into()), TestItem("b".into())];
        let result = spread(left, right);
        assert_eq!(
            result,
            vec![
                TestItem("x".into()),
                TestItem("a".into()),
                TestItem("b".into()),
            ]
        );
    }

    #[test]
    fn test_spread_strings_placeholder_at_middle() {
        let left = vec!["pre".to_string(), "...".to_string(), "post".to_string()];
        let right = vec!["a".to_string(), "b".to_string()];
        let result = spread_strings(left, right);
        assert_eq!(result, vec!["pre", "a", "b", "post"]);
    }

    #[test]
    fn test_spread_strings_empty_left_returns_right() {
        let left: Vec<String> = vec![];
        let right = vec!["a".to_string(), "b".to_string()];
        let result = spread_strings(left, right.clone());
        assert_eq!(result, right);
    }

    #[test]
    fn test_spread_strings_no_placeholder() {
        let left = vec!["x".to_string(), "y".to_string()];
        let right = vec!["a".to_string(), "b".to_string()];
        let result = spread_strings(left.clone(), right);
        assert_eq!(result, left);
    }
}
