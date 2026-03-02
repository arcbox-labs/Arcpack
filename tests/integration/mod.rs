pub mod docker_compose;
pub mod http_check;
/// 集成测试框架
///
/// 扫描 `tests/fixtures/*/test.json`，为每个 fixture 运行端到端构建和验证。
/// 需要 buildkitd + docker 运行环境。
pub mod test_config;

use std::path::Path;
use std::process::Command;

use arcpack::app::environment::Environment;
use arcpack::app::App;
use arcpack::config::Config;
use arcpack::generate::GenerateContext;
use arcpack::provider;
use arcpack::resolver::VersionResolver;

/// 集成测试版本解析器：使用 mise 真实解析
struct IntegrationVersionResolver;

impl VersionResolver for IntegrationVersionResolver {
    fn get_latest_version(&self, pkg: &str, version: &str) -> arcpack::Result<String> {
        // 集成测试使用固定版本避免网络依赖
        match pkg {
            "node" => Ok(format!("{}.0.0", version)),
            "bun" => {
                if version == "latest" {
                    Ok("1.2.0".to_string())
                } else {
                    Ok(format!("{}.0.0", version))
                }
            }
            "caddy" => Ok(format!("{}.0.0", version)),
            _ => Ok(format!("{}.0.0", version)),
        }
    }

    fn get_all_versions(&self, _pkg: &str, _version: &str) -> arcpack::Result<Vec<String>> {
        Ok(vec!["1.0.0".to_string()])
    }
}

/// 扫描 fixtures 目录，返回有 test.json 的 fixture 名
pub fn find_testable_fixtures() -> Vec<String> {
    let fixtures_dir = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    let mut fixtures = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&fixtures_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("test.json").exists() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    fixtures.push(name.to_string());
                }
            }
        }
    }

    fixtures.sort();
    fixtures
}

/// 为单个 fixture 运行集成测试
pub fn run_fixture_test(fixture_name: &str) -> Result<(), String> {
    let fixture_path = format!(
        "{}/tests/fixtures/{}",
        env!("CARGO_MANIFEST_DIR"),
        fixture_name
    );

    // 1. 加载 test.json
    let test_json_path = format!("{}/test.json", fixture_path);
    let config = test_config::TestConfig::load(&test_json_path)
        .map_err(|e| format!("failed to load test.json: {}", e))?;

    // 2. 生成 BuildPlan
    let app = App::new(&fixture_path).map_err(|e| format!("failed to create app: {}", e))?;

    let env_vars = config.envs.clone().unwrap_or_default();
    let env = Environment::new(env_vars);
    let arcpack_config = Config::load(&app, &env, Config::empty(), &None)
        .map_err(|e| format!("failed to load config: {}", e))?;

    let mut provider_to_use: Option<Box<dyn provider::Provider>> = None;
    for p in provider::get_all_providers() {
        if p.detect(&app, &env)
            .map_err(|e| format!("detect error: {}", e))?
        {
            provider_to_use = Some(p);
            break;
        }
    }

    let mut provider_to_use = provider_to_use.ok_or_else(|| "no provider matched".to_string())?;

    let version_resolver = Box::new(IntegrationVersionResolver);
    let mut ctx = GenerateContext::new(app, env, arcpack_config, version_resolver)
        .map_err(|e| format!("failed to create context: {}", e))?;

    provider_to_use
        .initialize(&mut ctx)
        .map_err(|e| format!("initialize error: {}", e))?;
    provider_to_use
        .plan(&mut ctx)
        .map_err(|e| format!("plan error: {}", e))?;

    let (mut plan, _resolved) = ctx
        .generate()
        .map_err(|e| format!("generate error: {}", e))?;
    provider_to_use.cleanse_plan(&mut plan);

    // 3. 验证 should_fail 场景
    if config.should_fail.unwrap_or(false) {
        return Err("should_fail fixtures not yet implemented".to_string());
    }

    // 4. just_build 模式只验证 plan 生成成功
    if config.just_build.unwrap_or(false) {
        println!(
            "  [{}] plan generated successfully (justBuild)",
            fixture_name
        );
        return Ok(());
    }

    // 5. 需要真实构建：调用 buildctl
    let image_tag = format!("arcpack-test-{}:latest", fixture_name);
    build_image(&fixture_path, &image_tag)?;

    // 6. 运行容器并验证
    if let Some(ref expected) = config.expected_output {
        verify_expected_output(&image_tag, expected)?;
    }

    if let Some(ref http_check) = config.http_check {
        http_check::verify_http(&image_tag, http_check)?;
    }

    // 7. 清理
    cleanup_image(&image_tag);

    println!("  [{}] passed", fixture_name);
    Ok(())
}

/// 构建 Docker 镜像（通过 arcpack CLI）
fn build_image(fixture_path: &str, tag: &str) -> Result<(), String> {
    let binary_path = Path::new(env!("CARGO_BIN_EXE_arcpack"));

    let output = Command::new(binary_path)
        .args(["build", fixture_path, "--name", tag])
        .output()
        .map_err(|e| format!("failed to run arcpack build: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("arcpack build failed: {}", stderr));
    }

    Ok(())
}

/// 运行容器并验证预期输出
fn verify_expected_output(image_tag: &str, expected: &str) -> Result<(), String> {
    let output = Command::new("docker")
        .args(["run", "--rm", image_tag])
        .output()
        .map_err(|e| format!("failed to run container: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.contains(expected) {
        return Err(format!(
            "expected output containing '{}', got: {}",
            expected, stdout
        ));
    }

    Ok(())
}

/// 清理镜像
fn cleanup_image(tag: &str) {
    let _ = Command::new("docker").args(["rmi", "-f", tag]).output();
}
