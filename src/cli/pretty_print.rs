/// 终端美化输出模块
///
/// 对齐 railpack `core/prettyPrint.go`

use std::io::Write;

use owo_colors::OwoColorize;
use owo_colors::Stream;

use crate::BuildResult;

/// 输出流目标
#[derive(Debug, Clone, Copy)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

impl OutputStream {
    fn to_owo(self) -> Stream {
        match self {
            OutputStream::Stdout => Stream::Stdout,
            OutputStream::Stderr => Stream::Stderr,
        }
    }
}

/// 打印选项
pub struct PrintOptions {
    /// 是否显示元数据
    pub metadata: bool,
    /// arcpack 版本
    pub version: String,
    /// 颜色检测目标流
    pub stream: OutputStream,
}

/// 格式化 BuildResult 为美化文本
pub fn format_build_result(result: &BuildResult, options: &PrintOptions) -> String {
    let mut out = String::new();
    let s = options.stream.to_owo();

    // 版本头
    out.push_str(&format!(
        "\n {} {}\n",
        "arcpack".if_supports_color(s, |t| t.cyan()),
        options.version.if_supports_color(s, |t| t.dimmed()),
    ));

    // Providers
    if !result.detected_providers.is_empty() {
        out.push_str(&format!(
            " {} {}\n",
            "Providers:".if_supports_color(s, |t| t.bold()),
            result.detected_providers.join(", "),
        ));
    }

    // Logs
    for log in &result.logs {
        let prefix = match log.level {
            crate::LogLevel::Info => "INFO".if_supports_color(s, |t| t.blue()).to_string(),
            crate::LogLevel::Warn => "WARN".if_supports_color(s, |t| t.yellow()).to_string(),
            crate::LogLevel::Error => "ERROR".if_supports_color(s, |t| t.red()).to_string(),
        };
        out.push_str(&format!(" [{}] {}\n", prefix, log.message));
    }

    // Packages
    if !result.resolved_packages.is_empty() {
        out.push_str(&format!(
            "\n {}\n",
            "Packages".if_supports_color(s, |t| t.bold()),
        ));
        // 按名称排序确保输出稳定
        let mut packages: Vec<_> = result.resolved_packages.iter().collect();
        packages.sort_by(|a, b| a.0.cmp(b.0));
        for (_, pkg) in &packages {
            let version_str = pkg
                .resolved_version
                .as_deref()
                .unwrap_or("unknown");
            let requested = pkg
                .requested_version
                .as_deref()
                .unwrap_or("");
            out.push_str(&format!(
                "  {} {} {} ({})\n",
                pkg.name.if_supports_color(s, |t| t.green()),
                version_str,
                if !requested.is_empty() {
                    format!("(requested: {})", requested)
                } else {
                    String::new()
                },
                pkg.source.if_supports_color(s, |t| t.dimmed()),
            ));
        }
    }

    // Steps
    if let Some(ref plan) = result.plan {
        if !plan.steps.is_empty() {
            out.push_str(&format!(
                "\n {}\n",
                "Steps".if_supports_color(s, |t| t.bold()),
            ));
            for (i, step) in plan.steps.iter().enumerate() {
                let name = step
                    .name
                    .as_deref()
                    .unwrap_or("unnamed");
                out.push_str(&format!(
                    "  {}. {}\n",
                    (i + 1).if_supports_color(s, |t| t.dimmed()),
                    name,
                ));
            }
        }

        // Deploy
        if let Some(ref start_cmd) = plan.deploy.start_cmd {
            out.push_str(&format!(
                "\n {} {}\n",
                "Start:".if_supports_color(s, |t| t.bold()),
                start_cmd.if_supports_color(s, |t| t.green()),
            ));
        }
    }

    // Metadata
    if options.metadata && !result.metadata.is_empty() {
        out.push_str(&format!(
            "\n {}\n",
            "Metadata".if_supports_color(s, |t| t.bold()),
        ));
        let mut entries: Vec<_> = result.metadata.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (key, value) in entries {
            out.push_str(&format!(
                "  {}={}\n",
                key.if_supports_color(s, |t| t.dimmed()),
                value,
            ));
        }
    }

    out.push('\n');
    out
}

/// 输出 BuildResult 到 stderr
pub fn pretty_print_build_result(result: &BuildResult, options: &PrintOptions) {
    let text = format_build_result(result, options);
    let _ = write!(std::io::stderr(), "{}", text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::plan::BuildPlan;
    use crate::plan::Step;
    use crate::resolver::ResolvedPackage;

    fn make_basic_result() -> BuildResult {
        BuildResult {
            arcpack_version: "0.1.0".to_string(),
            plan: Some(BuildPlan {
                steps: vec![Step::new("install"), Step::new("build")],
                ..BuildPlan::default()
            }),
            resolved_packages: HashMap::new(),
            metadata: HashMap::new(),
            detected_providers: vec!["node".to_string()],
            logs: Vec::new(),
            success: true,
        }
    }

    fn default_options() -> PrintOptions {
        PrintOptions {
            metadata: false,
            version: "0.1.0".to_string(),
            stream: OutputStream::Stderr,
        }
    }

    #[test]
    fn test_format_build_result_shows_providers() {
        let result = make_basic_result();
        let text = format_build_result(&result, &default_options());
        assert!(text.contains("node"));
        assert!(text.contains("Providers:"));
    }

    #[test]
    fn test_format_build_result_shows_packages() {
        let mut result = make_basic_result();
        result.resolved_packages.insert(
            "node".to_string(),
            ResolvedPackage {
                name: "node".to_string(),
                requested_version: Some("22".to_string()),
                resolved_version: Some("22.0.0".to_string()),
                source: "arcpack default".to_string(),
            },
        );
        let text = format_build_result(&result, &default_options());
        assert!(text.contains("Packages"));
        assert!(text.contains("22.0.0"));
    }

    #[test]
    fn test_format_build_result_shows_metadata_when_enabled() {
        let mut result = make_basic_result();
        result
            .metadata
            .insert("framework".to_string(), "express".to_string());
        let options = PrintOptions {
            metadata: true,
            ..default_options()
        };
        let text = format_build_result(&result, &options);
        assert!(text.contains("Metadata"));
        assert!(text.contains("framework"));
        assert!(text.contains("express"));
    }

    #[test]
    fn test_format_build_result_hides_metadata_when_disabled() {
        let mut result = make_basic_result();
        result
            .metadata
            .insert("framework".to_string(), "express".to_string());
        let text = format_build_result(&result, &default_options());
        assert!(!text.contains("Metadata"));
    }
}
