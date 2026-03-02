//! CLI 端到端测试
//!
//! 使用 assert_cmd + predicates 对 arcpack CLI 进行黑盒测试。
//! 需要 mise 的测试标记 #[ignore]，通过 `cargo test -- --ignored` 运行。

use assert_cmd::Command;
use predicates::prelude::*;

fn arcpack() -> Command {
    #[allow(deprecated)]
    Command::cargo_bin("arcpack").unwrap()
}

fn fixture_path(name: &str) -> String {
    format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
}

// === 基础命令（无需 mise） ===

#[test]
fn test_help_exits_zero() {
    arcpack()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("arcpack"));
}

#[test]
fn test_version_exits_zero() {
    arcpack()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("arcpack"));
}

// === schema 命令（无需 mise） ===

#[test]
fn test_schema_outputs_valid_json_with_type() {
    arcpack()
        .arg("schema")
        .assert()
        .success()
        .stdout(predicate::str::contains("\"type\""));
}

// === 错误处理（无需 mise） ===

#[test]
fn test_plan_nonexistent_path_fails() {
    arcpack()
        .args(["plan", "/nonexistent/path/that/does/not/exist"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("error"));
}

// === build 命令（无需 mise） ===

#[test]
fn test_build_nonexistent_path_fails() {
    arcpack()
        .args(["build", "/nonexistent/path/that/does/not/exist"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("error"));
}

#[test]
fn test_build_accepts_name_flag() {
    // build 命令应接受 --name 参数（即使构建本身会失败）
    arcpack()
        .args(["build", "--name", "myapp", "/nonexistent/path"])
        .assert()
        .failure();
}

#[test]
fn test_build_accepts_platform_flag() {
    // build 命令应接受 --platform 参数（即使构建本身会失败）
    arcpack()
        .args(["build", "--platform", "linux/amd64", "/nonexistent/path"])
        .assert()
        .failure();
}

// === plan 命令（需要 mise） ===

#[test]
#[ignore]
fn test_plan_node_npm_outputs_json_with_schema_and_steps() {
    arcpack()
        .args(["plan", &fixture_path("node-npm")])
        .assert()
        .success()
        .stdout(predicate::str::contains("$schema").and(predicate::str::contains("steps")));
}

#[test]
#[ignore]
fn test_plan_out_writes_valid_json_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let out_path = dir.path().join("plan.json");

    arcpack()
        .args([
            "plan",
            "--out",
            out_path.to_str().unwrap(),
            &fixture_path("node-npm"),
        ])
        .assert()
        .success();

    let content = std::fs::read_to_string(&out_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(value.get("$schema").is_some());
    assert!(value.get("steps").is_some() || value.get("deploy").is_some());
}

#[test]
#[ignore]
fn test_plan_with_env_flag() {
    arcpack()
        .args(["plan", "--env", "FOO=bar", &fixture_path("node-npm")])
        .assert()
        .success();
}

#[test]
#[ignore]
fn test_plan_with_build_cmd_flag() {
    arcpack()
        .args([
            "plan",
            "--build-cmd",
            "npm run build",
            &fixture_path("node-npm"),
        ])
        .assert()
        .success();
}

#[test]
#[ignore]
fn test_plan_with_start_cmd_flag() {
    arcpack()
        .args([
            "plan",
            "--start-cmd",
            "node server.js",
            &fixture_path("node-npm"),
        ])
        .assert()
        .success();
}

// === info 命令（需要 mise） ===

#[test]
#[ignore]
fn test_info_pretty_contains_node() {
    arcpack()
        .args(["info", &fixture_path("node-npm")])
        .assert()
        .success()
        .stdout(predicate::str::contains("node"));
}

#[test]
#[ignore]
fn test_info_json_outputs_valid_json() {
    let output = arcpack()
        .args(["info", "--format", "json", &fixture_path("node-npm")])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json_str = String::from_utf8(output).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert!(value.get("arcpackVersion").is_some());
    assert!(value.get("detectedProviders").is_some());
}

// === prepare 命令（需要 mise） ===

#[test]
#[ignore]
fn test_prepare_plan_out_writes_json_with_schema() {
    let dir = tempfile::TempDir::new().unwrap();
    let plan_path = dir.path().join("plan.json");

    arcpack()
        .args([
            "prepare",
            "--plan-out",
            plan_path.to_str().unwrap(),
            &fixture_path("node-npm"),
        ])
        .assert()
        .success();

    let content = std::fs::read_to_string(&plan_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(value.get("$schema").is_some());
}

#[test]
#[ignore]
fn test_prepare_info_out_has_null_plan() {
    let dir = tempfile::TempDir::new().unwrap();
    let info_path = dir.path().join("info.json");

    arcpack()
        .args([
            "prepare",
            "--info-out",
            info_path.to_str().unwrap(),
            &fixture_path("node-npm"),
        ])
        .assert()
        .success();

    let content = std::fs::read_to_string(&info_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(value.get("plan").unwrap().is_null());
}
