/// 版本约束 → 模糊版本（对齐 railpack resolveToFuzzyVersion）
///
/// 转换规则：
/// - "" / "*" → "latest"
/// - ">=22 <23" → "22"（取 >= 后的主版本号）
/// - "^18.2.0" → "18"（caret → 仅保留主版本）
/// - "~18.2.0" → "18.2.0"（去掉 ~ 前缀）
/// - "v18" → "18"（去掉 v 前缀）
/// - "14.x" → "14"（去掉 .x 后缀）
pub fn resolve_to_fuzzy_version(version: &str) -> String {
    let version = version.trim();

    // 空串和 "*" 返回 "latest"
    if version.is_empty() || version == "*" {
        return "latest".to_string();
    }

    // 处理范围表示法 (e.g. ">=22 <23" 或 ">= 22" 或 ">=20.0.0")
    if version.contains(">=") || version.contains('<') {
        let parts: Vec<&str> = version.split_whitespace().collect();
        for (i, part) in parts.iter().enumerate() {
            if let Some(v) = part.strip_prefix(">=") {
                // 版本号在 >= 之后本部分，或在下一部分
                let v = if v.is_empty() && i + 1 < parts.len() {
                    parts[i + 1]
                } else {
                    v
                };
                // 只保留主版本号
                return v.trim().split('.').next().unwrap_or(v).to_string();
            }
        }
    }

    // 处理 caret 表示法：只保留主版本号
    if let Some(v) = version.strip_prefix('^') {
        return v.split('.').next().unwrap_or(v).to_string();
    }

    // 移除 ~ 和 v 前缀
    let version = version.strip_prefix('~').unwrap_or(version);
    let version = version.strip_prefix('v').unwrap_or(version);

    // 替换 .x 为空
    let version = version.replace(".x", "");

    // 移除尾部的点号
    version.trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_to_fuzzy_version_empty_returns_latest() {
        assert_eq!(resolve_to_fuzzy_version(""), "latest");
    }

    #[test]
    fn test_resolve_to_fuzzy_version_star_returns_latest() {
        assert_eq!(resolve_to_fuzzy_version("*"), "latest");
    }

    #[test]
    fn test_resolve_to_fuzzy_version_whitespace_only_returns_latest() {
        assert_eq!(resolve_to_fuzzy_version("  "), "latest");
    }

    #[test]
    fn test_resolve_to_fuzzy_version_caret_major_only() {
        assert_eq!(resolve_to_fuzzy_version("^18.2.0"), "18");
        assert_eq!(resolve_to_fuzzy_version("^18.4"), "18");
        assert_eq!(resolve_to_fuzzy_version("^18"), "18");
    }

    #[test]
    fn test_resolve_to_fuzzy_version_tilde_keeps_version() {
        assert_eq!(resolve_to_fuzzy_version("~16.0"), "16.0");
        assert_eq!(resolve_to_fuzzy_version("~16.0.1"), "16.0.1");
    }

    #[test]
    fn test_resolve_to_fuzzy_version_range_notation() {
        assert_eq!(resolve_to_fuzzy_version(">=22 <23"), "22");
        assert_eq!(resolve_to_fuzzy_version(">= 22"), "22");
        assert_eq!(resolve_to_fuzzy_version(">=20.0.0"), "20");
    }

    #[test]
    fn test_resolve_to_fuzzy_version_v_prefix_stripped() {
        assert_eq!(resolve_to_fuzzy_version("v18"), "18");
        assert_eq!(resolve_to_fuzzy_version("v18.4.1"), "18.4.1");
    }

    #[test]
    fn test_resolve_to_fuzzy_version_dot_x_removed() {
        assert_eq!(resolve_to_fuzzy_version("14.x"), "14");
        assert_eq!(resolve_to_fuzzy_version("3.x"), "3");
    }

    #[test]
    fn test_resolve_to_fuzzy_version_exact_preserved() {
        assert_eq!(resolve_to_fuzzy_version("18.4.1"), "18.4.1");
        assert_eq!(resolve_to_fuzzy_version("22"), "22");
    }

    #[test]
    fn test_resolve_to_fuzzy_version_lts_latest_preserved() {
        assert_eq!(resolve_to_fuzzy_version("lts"), "lts");
        assert_eq!(resolve_to_fuzzy_version("latest"), "latest");
    }
}
