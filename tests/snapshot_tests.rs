//! Snapshot 测试：端到端 BuildPlan 生成
//!
//! 使用 insta 对 fixture 生成的 BuildPlan 进行快照比对。
//! 使用 MockVersionResolver 避免依赖 mise 二进制。

use std::collections::HashMap;

use arcpack::app::App;
use arcpack::app::environment::Environment;
use arcpack::config::Config;
use arcpack::error::ArcpackError;
use arcpack::generate::GenerateContext;
use arcpack::plan::BuildPlan;
use arcpack::provider;
use arcpack::resolver::{ResolvedPackage, VersionResolver};

/// Mock 版本解析器：返回可预测的版本号
struct MockVersionResolver;

impl VersionResolver for MockVersionResolver {
    fn get_latest_version(&self, pkg: &str, version: &str) -> arcpack::Result<String> {
        // 返回可预测的版本，用于快照稳定性
        match pkg {
            "node" => Ok(format!("{}.0.0", version)),
            "bun" => {
                if version == "latest" {
                    Ok("1.2.0".to_string())
                } else {
                    Ok(format!("{}.0.0", version))
                }
            }
            "pnpm" => Ok(format!("{}.0.0", version)),
            "yarn" => Ok(format!("{}.0.0", version)),
            _ => Ok(format!("{}.0.0", version)),
        }
    }

    fn get_all_versions(&self, _pkg: &str, _version: &str) -> arcpack::Result<Vec<String>> {
        Ok(vec!["1.0.0".to_string()])
    }
}

/// 配置 insta：排序 map 键确保快照稳定性
fn insta_settings() -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    settings.set_sort_maps(true);
    settings
}

/// 从 fixture 目录生成 BuildPlan
fn generate_plan_from_fixture(
    fixture_name: &str,
) -> arcpack::Result<(BuildPlan, HashMap<String, ResolvedPackage>, Vec<String>)> {
    let fixture_path = format!(
        "{}/tests/fixtures/{}",
        env!("CARGO_MANIFEST_DIR"),
        fixture_name
    );

    let app = App::new(&fixture_path)?;
    let env = Environment::new(HashMap::new());
    let config = Config::load(&app, &env, Config::empty(), &None)?;

    // 检测 Provider
    let mut provider_to_use: Option<Box<dyn provider::Provider>> = None;
    for p in provider::get_all_providers() {
        if p.detect(&app, &env)? {
            provider_to_use = Some(p);
            break;
        }
    }

    let mut provider_to_use =
        provider_to_use.ok_or_else(|| ArcpackError::NoProviderMatched)?;

    let version_resolver = Box::new(MockVersionResolver);
    let mut ctx = GenerateContext::new(app, env, config, version_resolver)?;

    provider_to_use.initialize(&mut ctx)?;
    provider_to_use.plan(&mut ctx)?;

    let (mut plan, resolved_packages) = ctx.generate()?;
    provider_to_use.cleanse_plan(&mut plan);

    Ok((
        plan,
        resolved_packages,
        vec![provider_to_use.name().to_string()],
    ))
}

#[test]
fn test_snapshot_node_npm() {
    let (plan, _resolved, providers) = generate_plan_from_fixture("node-npm").unwrap();
    assert_eq!(providers, vec!["node"]);
    insta_settings().bind(|| {
        insta::assert_json_snapshot!("node-npm-plan", plan);
    });
}

#[test]
fn test_snapshot_node_yarn() {
    let (plan, _resolved, providers) = generate_plan_from_fixture("node-yarn").unwrap();
    assert_eq!(providers, vec!["node"]);
    insta_settings().bind(|| {
        insta::assert_json_snapshot!("node-yarn-plan", plan);
    });
}

#[test]
fn test_snapshot_node_yarn_berry() {
    let (plan, _resolved, providers) = generate_plan_from_fixture("node-yarn-berry").unwrap();
    assert_eq!(providers, vec!["node"]);
    insta_settings().bind(|| {
        insta::assert_json_snapshot!("node-yarn-berry-plan", plan);
    });
}

#[test]
fn test_snapshot_node_pnpm() {
    let (plan, _resolved, providers) = generate_plan_from_fixture("node-pnpm").unwrap();
    assert_eq!(providers, vec!["node"]);
    insta_settings().bind(|| {
        insta::assert_json_snapshot!("node-pnpm-plan", plan);
    });
}

#[test]
fn test_snapshot_node_bun() {
    let (plan, _resolved, providers) = generate_plan_from_fixture("node-bun").unwrap();
    assert_eq!(providers, vec!["node"]);
    insta_settings().bind(|| {
        insta::assert_json_snapshot!("node-bun-plan", plan);
    });
}

#[test]
fn test_empty_directory_returns_no_provider() {
    let dir = tempfile::TempDir::new().unwrap();
    let app = App::new(dir.path().to_str().unwrap()).unwrap();
    let env = Environment::new(HashMap::new());

    let mut matched = false;
    for p in provider::get_all_providers() {
        if p.detect(&app, &env).unwrap() {
            matched = true;
            break;
        }
    }
    assert!(!matched, "empty directory should not match any provider");
}

#[test]
fn test_detected_providers_contains_node() {
    let (_, _, providers) = generate_plan_from_fixture("node-npm").unwrap();
    assert!(providers.contains(&"node".to_string()));
}

#[test]
fn test_npm_plan_has_start_command() {
    let (plan, _, _) = generate_plan_from_fixture("node-npm").unwrap();
    assert_eq!(plan.deploy.start_cmd, Some("npm run start".to_string()));
}

#[test]
fn test_pnpm_plan_has_install_step() {
    let (plan, _, _) = generate_plan_from_fixture("node-pnpm").unwrap();
    let step_names: Vec<&str> = plan.steps.iter().filter_map(|s| s.name.as_deref()).collect();
    assert!(step_names.contains(&"install"), "missing install step");
}

#[test]
fn test_bun_plan_has_bun_start() {
    let (plan, _, _) = generate_plan_from_fixture("node-bun").unwrap();
    assert_eq!(plan.deploy.start_cmd, Some("bun run start".to_string()));
}

#[test]
fn test_snapshot_node_next() {
    let (plan, _resolved, providers) = generate_plan_from_fixture("node-next").unwrap();
    assert_eq!(providers, vec!["node"]);
    // Next.js SSR 模式应使用 npm start
    assert!(plan.deploy.start_cmd.as_deref().unwrap().contains("start"));
    insta_settings().bind(|| {
        insta::assert_json_snapshot!("node-next-plan", plan);
    });
}

#[test]
fn test_snapshot_node_vite_spa() {
    let (plan, _resolved, providers) = generate_plan_from_fixture("node-vite-spa").unwrap();
    assert_eq!(providers, vec!["node"]);
    // SPA 模式应使用 caddy
    assert!(plan.deploy.start_cmd.as_deref().unwrap().contains("caddy"));
    insta_settings().bind(|| {
        insta::assert_json_snapshot!("node-vite-spa-plan", plan);
    });
}

#[test]
fn test_snapshot_node_monorepo() {
    let (plan, _resolved, providers) = generate_plan_from_fixture("node-monorepo").unwrap();
    assert_eq!(providers, vec!["node"]);
    insta_settings().bind(|| {
        insta::assert_json_snapshot!("node-monorepo-plan", plan);
    });
}
