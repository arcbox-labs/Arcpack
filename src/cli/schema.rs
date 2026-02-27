/// schema 命令 —— 输出 arcpack.json 的 JSON Schema
///
/// 对齐 railpack `cli/schema.go`

use crate::config::Config;

/// 执行 schema 命令
pub fn run_schema() -> crate::Result<()> {
    let schema = Config::json_schema();
    let json = serde_json::to_string_pretty(&schema)?;
    println!("{}", json);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_output_is_valid_json_with_type_object() {
        let schema = Config::json_schema();
        let json = serde_json::to_string_pretty(&schema).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["type"], "object");
    }

    #[test]
    fn test_schema_contains_config_fields() {
        let schema = Config::json_schema();
        let json = serde_json::to_string_pretty(&schema).unwrap();
        // Config 顶层字段名
        assert!(json.contains("provider"));
        assert!(json.contains("buildAptPackages"));
        assert!(json.contains("steps"));
        assert!(json.contains("deploy"));
        assert!(json.contains("packages"));
        assert!(json.contains("caches"));
        assert!(json.contains("secrets"));
    }
}
