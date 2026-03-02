/// SPA 部署模块（Caddy 静态服务）
///
/// 对齐 railpack `core/providers/node/spa.go`
/// SPA 框架使用 Caddy 作为静态文件服务器，包括 Caddyfile 模板和健康检查端点。
use crate::generate::install_bin_builder::InstallBinBuilder;
use crate::generate::GenerateContext;
use crate::plan::{Command, Filter, Layer};
use crate::Result;

/// Caddy 默认版本
const DEFAULT_CADDY_VERSION: &str = "latest";
/// Caddy 安装步骤名
const CADDY_STEP_NAME: &str = "packages:caddy";

/// 默认 Caddyfile 模板
///
/// 对齐 railpack `core/providers/node/Caddyfile.template`
const CADDYFILE_TEMPLATE: &str = include_str!("templates/Caddyfile.template");

fn render_caddyfile(template: &str, output_dir: &str) -> String {
    let normalized_output = output_dir.trim_end_matches('/');
    template.replace("{{.DIST_DIR}}", &format!("/app/{normalized_output}"))
}

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
    ctx.steps.push(Box::new(caddy_builder));

    // 2. 读取 Caddyfile 模板（优先 Caddyfile.template，再 Caddyfile）
    let caddy_template = if ctx.app.has_file("Caddyfile.template") {
        ctx.app.read_file("Caddyfile.template")?
    } else if ctx.app.has_file("Caddyfile") {
        ctx.app.read_file("Caddyfile")?
    } else {
        CADDYFILE_TEMPLATE.to_string()
    };
    let caddyfile_content = render_caddyfile(&caddy_template, output_dir);

    // 创建 Caddy 配置步骤
    let caddy_step = ctx.new_command_step("caddy");
    caddy_step.add_input(Layer::new_step_layer(CADDY_STEP_NAME, None));
    caddy_step
        .assets
        .insert("Caddyfile".to_string(), caddyfile_content);
    caddy_step.add_command(Command::new_file("/Caddyfile", "Caddyfile"));
    caddy_step.add_command(Command::new_exec("caddy fmt --overwrite /Caddyfile"));

    // 3. 配置 Deploy
    ctx.deploy.start_cmd =
        Some("caddy run --config /Caddyfile --adapter caddyfile 2>&1".to_string());

    // deploy inputs: caddy 二进制 + Caddyfile + 构建输出目录
    let caddy_config_layer = Layer::new_step_layer(
        "caddy",
        Some(Filter::include_only(vec!["/Caddyfile".to_string()])),
    );
    let build_output_layer = Layer::new_step_layer(
        build_step_name,
        Some(Filter::include_only(vec![output_dir.to_string()])),
    );

    ctx.deploy
        .add_inputs(&[caddy_layer, caddy_config_layer, build_output_layer]);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_caddyfile_template_has_health_endpoint() {
        assert!(CADDYFILE_TEMPLATE.contains("/health"));
        assert!(CADDYFILE_TEMPLATE.contains("respond /health 200"));
    }

    #[test]
    fn test_caddyfile_template_has_spa_fallback() {
        assert!(CADDYFILE_TEMPLATE.contains("try_files"));
        assert!(CADDYFILE_TEMPLATE.contains("/index.html"));
    }

    #[test]
    fn test_caddyfile_template_has_compression() {
        assert!(CADDYFILE_TEMPLATE.contains("encode {"));
        assert!(CADDYFILE_TEMPLATE.contains("zstd"));
    }

    #[test]
    fn test_caddyfile_template_substitution() {
        let content = render_caddyfile(CADDYFILE_TEMPLATE, "dist");
        assert!(content.contains("root * /app/dist"));
    }
}
