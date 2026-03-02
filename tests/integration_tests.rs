/// 集成测试入口
///
/// 需要 buildkitd + docker 运行环境。
/// 运行：`cargo test --test integration_tests -- --ignored`
mod integration;

/// 扫描所有带 test.json 的 fixture 并逐个运行
#[test]
#[ignore]
fn test_all_fixtures() {
    let fixtures = integration::find_testable_fixtures();
    if fixtures.is_empty() {
        println!("no testable fixtures found (no test.json files)");
        return;
    }

    let mut failures = Vec::new();

    for fixture in &fixtures {
        println!("testing fixture: {}", fixture);
        if let Err(e) = integration::run_fixture_test(fixture) {
            eprintln!("  FAILED: {}", e);
            failures.push((fixture.clone(), e));
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} fixture(s) failed:\n{}",
            failures.len(),
            failures
                .iter()
                .map(|(name, err)| format!("  - {}: {}", name, err))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

/// 单独测试 plan 生成（不需要 docker）
#[test]
#[ignore]
fn test_fixture_plan_generation() {
    let fixtures = integration::find_testable_fixtures();

    for fixture in &fixtures {
        println!("generating plan for: {}", fixture);
        // 只验证 plan 生成不 panic
        let fixture_path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), fixture);

        let app = arcpack::app::App::new(&fixture_path).unwrap();
        let env = arcpack::app::environment::Environment::new(std::collections::HashMap::new());

        let mut matched = false;
        for p in arcpack::provider::get_all_providers() {
            if p.detect(&app, &env).unwrap() {
                matched = true;
                break;
            }
        }

        assert!(matched, "fixture '{}' should match a provider", fixture);
    }
}
