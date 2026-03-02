/// SPA 部署模块（Caddy 静态服务）
///
/// 对齐 railpack `core/providers/node/spa.go`
/// SPA 框架使用 Caddy 作为静态文件服务器，包括 Caddyfile 模板和健康检查端点。
use crate::generate::install_bin_builder::InstallBinBuilder;
use crate::generate::GenerateContext;
use crate::plan::{Command, Filter, Layer};
use crate::Result;

/// Caddy 默认版本
const DEFAULT_CADDY_VERSION: &str = "2";
/// Caddy 安装步骤名
const CADDY_STEP_NAME: &str = "packages:caddy";

/// 默认 Caddyfile 模板
///
/// 对齐 railpack `core/providers/node/Caddyfile.template`
const CADDYFILE_TEMPLATE: &str = r#":{$PORT:3000} {
    root * /app/{DIST_DIR}
    encode gzip zstd

    handle /health {
        respond "OK" 200
    }

    handle {
        try_files {path} {path}.html {path}/index.html /index.html
        file_server
    }

    header {
        X-Content-Type-Options nosniff
        X-Frame-Options DENY
        X-XSS-Protection "1; mode=block"
    }
}
"#;

/// 配置 SPA 部署（Caddy 静态服务）
///
/// 1. 通过 InstallBinBuilder 安装 caddy 二进制
/// 2. 创建 caddy 命令步骤：写入 Caddyfile
/// 3. 配置 deploy inputs: caddy 二进制层 + Caddyfile + 构建输出目录
pub fn deploy_as_spa(
    ctx: &mut GenerateContext,
    output_dir: &str,
    build_step_name: &str,
) -> Result<()> {
    // 1. 安装 Caddy 二进制
    let mut caddy_builder = InstallBinBuilder::new(CADDY_STEP_NAME);
    let caddy_ref =
        caddy_builder.default_package(&mut ctx.resolver, "caddy", DEFAULT_CADDY_VERSION);
    let _ = caddy_ref;

    let caddy_layer = caddy_builder.get_layer();
    let caddy_paths = caddy_builder.get_output_paths();
    ctx.steps.push(Box::new(caddy_builder));

    // 2. 检查用户自定义 Caddyfile（在借用 ctx.steps 之前）
    let has_custom_caddyfile =
        ctx.app.has_file("Caddyfile") || ctx.app.has_file("Caddyfile.template");
    let local_layer = if has_custom_caddyfile {
        Some(ctx.new_local_layer())
    } else {
        None
    };

    // 创建 Caddy 配置步骤
    let caddy_step = ctx.new_command_step("caddy");

    if let Some(layer) = local_layer {
        caddy_step.add_input(layer);
    } else {
        let caddyfile_content = CADDYFILE_TEMPLATE.replace("{DIST_DIR}", output_dir);
        caddy_step.add_command(Command::new_file("/app/Caddyfile", &caddyfile_content));
    }

    // 3. 配置 Deploy
    ctx.deploy.start_cmd =
        Some("caddy run --config /app/Caddyfile --adapter caddyfile".to_string());

    // deploy inputs: caddy 二进制 + 构建输出 + Caddyfile
    let build_output_layer = Layer::new_step_layer(
        build_step_name,
        Some(Filter::include_only(vec![output_dir.to_string()])),
    );

    ctx.deploy.add_inputs(&[caddy_layer, build_output_layer]);

    // PATH 中加入 caddy
    for path in &caddy_paths {
        ctx.deploy.paths.push(path.clone());
        ctx.deploy.paths.push(format!("{}/bin", path));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_caddyfile_template_has_health_endpoint() {
        assert!(CADDYFILE_TEMPLATE.contains("/health"));
        assert!(CADDYFILE_TEMPLATE.contains("respond \"OK\" 200"));
    }

    #[test]
    fn test_caddyfile_template_has_spa_fallback() {
        assert!(CADDYFILE_TEMPLATE.contains("try_files"));
        assert!(CADDYFILE_TEMPLATE.contains("/index.html"));
    }

    #[test]
    fn test_caddyfile_template_has_compression() {
        assert!(CADDYFILE_TEMPLATE.contains("encode gzip zstd"));
    }

    #[test]
    fn test_caddyfile_template_substitution() {
        let content = CADDYFILE_TEMPLATE.replace("{DIST_DIR}", "dist");
        assert!(content.contains("root * /app/dist"));
    }
}
