//! Railpack 对拍测试：对比 `railpack plan` 与 `arcpack plan` 的语义一致性。
//!
//! 运行方式：
//! `cargo test --test plan_parity_tests -- --ignored --nocapture`
//!
//! 可选环境变量：
//! - `ARCPACK_BIN`: arcpack 可执行文件路径（默认 `CARGO_BIN_EXE_arcpack`）
//! - `RAILPACK_BIN`: railpack 可执行文件路径（默认 `railpack`）
//! - `PARITY_FIXTURES_DIR`: fixture 根目录（默认 `tests/fixtures`）
//! - `PARITY_FIXTURES`: 逗号分隔的 fixture 名单（默认扫描 fixture 根目录下含 `test.json` 的目录）
//! - `PARITY_FAKE_MISE`: 是否注入离线 mise stub（`1/true/yes/on` 启用）

use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const DEFAULT_FIXTURES_DIR: &str = "tests/fixtures";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FixtureCase {
    envs: Vec<(String, String)>,
    should_fail: bool,
    config_file: Option<String>,
}

#[test]
#[ignore]
fn test_fixtures_plan_parity_with_railpack() {
    let fixtures_root = fixtures_root();
    maybe_prepare_fake_mise(&fixtures_root);

    let fixtures = discover_fixtures(&fixtures_root);
    assert!(
        !fixtures.is_empty(),
        "no fixtures found for parity test (set PARITY_FIXTURES / PARITY_FIXTURES_DIR)"
    );

    let arcpack_bin =
        std::env::var("ARCPACK_BIN").unwrap_or_else(|_| env!("CARGO_BIN_EXE_arcpack").to_string());
    let railpack_bin = std::env::var("RAILPACK_BIN").unwrap_or_else(|_| "railpack".to_string());

    assert_command_resolvable(&arcpack_bin, "arcpack", "ARCPACK_BIN");
    assert_command_resolvable(&railpack_bin, "railpack", "RAILPACK_BIN");

    let mut failures = Vec::new();

    for fixture in fixtures {
        let fixture_dir = fixture_path(&fixtures_root, &fixture);
        let cases = load_fixture_cases(&fixture_dir);

        for (idx, case) in cases.iter().enumerate() {
            if case.should_fail {
                continue;
            }

            let case_label = format!("{fixture}#{}", idx + 1);
            let arcpack_envs = adapt_envs_for_arcpack(&case.envs);

            let arcpack_config = case
                .config_file
                .clone()
                .or_else(|| arcpack_default_config_override(&fixture_dir));
            let railpack_config = case.config_file.clone();

            let arcpack_raw = match run_plan(
                &arcpack_bin,
                &fixture_dir,
                &arcpack_envs,
                arcpack_config.as_deref(),
            ) {
                Ok(v) => v,
                Err(e) => {
                    failures.push(format!("[{case_label}] arcpack plan failed:\n{e}"));
                    continue;
                }
            };

            let railpack_raw = match run_plan(
                &railpack_bin,
                &fixture_dir,
                &case.envs,
                railpack_config.as_deref(),
            ) {
                Ok(v) => v,
                Err(e) => {
                    failures.push(format!("[{case_label}] railpack plan failed:\n{e}"));
                    continue;
                }
            };

            let arcpack_norm = normalize_plan(arcpack_raw);
            let railpack_norm = normalize_plan(railpack_raw);
            if arcpack_norm != railpack_norm {
                let diff = format_plan_diff(&arcpack_norm, &railpack_norm);
                failures.push(format!("[{case_label}] normalized plans differ:\n{diff}\n"));
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} parity issue(s):\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

fn env_truthy(key: &str) -> bool {
    match std::env::var(key) {
        Ok(v) => {
            let s = v.trim().to_ascii_lowercase();
            matches!(s.as_str(), "1" | "true" | "yes" | "on")
        }
        Err(_) => false,
    }
}

fn maybe_prepare_fake_mise(fixtures_root: &Path) {
    if !env_truthy("PARITY_FAKE_MISE") {
        return;
    }

    let Some(railpack_root) = fixtures_root.parent() else {
        panic!(
            "PARITY_FAKE_MISE requires PARITY_FIXTURES_DIR under a railpack repo, got: {}",
            fixtures_root.display()
        );
    };

    let version_file = railpack_root.join("core/mise/version.txt");
    let version = std::fs::read_to_string(&version_file)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", version_file.display()))
        .trim()
        .to_string();
    assert!(
        !version.is_empty(),
        "empty mise version in {}",
        version_file.display()
    );

    let mise_dir = Path::new("/tmp/railpack/mise");
    std::fs::create_dir_all(mise_dir)
        .unwrap_or_else(|e| panic!("failed to create {}: {e}", mise_dir.display()));

    let stub_path = mise_dir.join(format!("mise-{version}"));
    if stub_path.exists() {
        return;
    }

    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

if [[ "${{1:-}}" == "--version" ]]; then
  echo "{version}"
  exit 0
fi

if [[ "${{1:-}}" == "latest" ]]; then
  q="${{2:-}}"
  if [[ "$q" == *"@"* ]]; then
    v="${{q##*@}}"
    if [[ -z "$v" || "$v" == "latest" || "$v" == "*" ]]; then
      v="1.0.0"
    fi
    echo "$v"
  else
    echo "1.0.0"
  fi
  exit 0
fi

if [[ "${{1:-}}" == "ls-remote" ]]; then
  q="${{2:-}}"
  if [[ "$q" == *"@"* ]]; then
    v="${{q##*@}}"
    if [[ -z "$v" || "$v" == "latest" || "$v" == "*" ]]; then
      v="1.0.0"
    fi
    echo "$v"
  else
    echo "1.0.0"
  fi
  exit 0
fi

if [[ "${{1:-}}" == "--cd" ]]; then
  shift 2 || true
  if [[ "${{1:-}}" == "list" ]]; then
    echo "[]"
    exit 0
  fi
fi

exit 0
"#
    );

    std::fs::write(&stub_path, script)
        .unwrap_or_else(|e| panic!("failed to write fake mise {}: {e}", stub_path.display()));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&stub_path, perms)
            .unwrap_or_else(|e| panic!("failed to chmod fake mise {}: {e}", stub_path.display()));
    }
}

fn assert_command_resolvable(bin: &str, tool_name: &str, hint_env: &str) {
    match Command::new(bin).arg("--version").output() {
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {
            panic!(
                "cannot execute {tool_name} binary `{bin}` (not found).\nset {hint_env} to the correct executable path"
            );
        }
        Err(e) => {
            panic!("cannot execute {tool_name} binary `{bin}`: {e}");
        }
    }
}

fn fixtures_root() -> PathBuf {
    match std::env::var("PARITY_FIXTURES_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => Path::new(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_FIXTURES_DIR),
    }
}

fn fixture_path(root: &Path, name: &str) -> PathBuf {
    root.join(name)
}

fn discover_fixtures(fixtures_root: &Path) -> Vec<String> {
    if let Ok(raw) = std::env::var("PARITY_FIXTURES") {
        let mut from_env: Vec<String> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        from_env.sort();
        from_env.dedup();
        return from_env;
    }

    let mut fixtures = Vec::new();
    let entries = std::fs::read_dir(fixtures_root).unwrap_or_else(|e| {
        panic!(
            "failed to read fixtures dir `{}`: {e}",
            fixtures_root.display()
        )
    });

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if !path.join("test.json").exists() {
            continue;
        }
        fixtures.push(name.to_string());
    }

    fixtures.sort();
    fixtures
}

fn run_plan(
    bin: &str,
    fixture_dir: &Path,
    envs: &[(String, String)],
    config_file: Option<&str>,
) -> Result<Value, String> {
    let mut cmd = Command::new(bin);

    let mut args = Vec::new();
    args.push("plan".to_string());

    if let Some(config_file) = config_file {
        args.push("--config-file".to_string());
        args.push(config_file.to_string());
    }

    for (k, v) in envs {
        args.push("--env".to_string());
        args.push(format!("{k}={v}"));
    }

    args.push(fixture_dir.display().to_string());
    cmd.args(&args);
    cmd.env("NO_COLOR", "1");
    // 对拍阶段优先保证可运行：mise 解析失败时回退到 fuzzy version。
    cmd.env("ARCPACK_RESOLVER_FALLBACK_ON_ERROR", "1");

    let output = cmd
        .output()
        .map_err(|e| format!("failed to execute `{bin}`: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "command failed: {bin} {}\nstatus: {}\nstderr:\n{}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    parse_json_output(&output.stdout, &output.stderr)
}

fn parse_json_output(stdout: &[u8], stderr: &[u8]) -> Result<Value, String> {
    match serde_json::from_slice::<Value>(stdout) {
        Ok(v) => Ok(v),
        Err(primary_err) => {
            let text = String::from_utf8_lossy(stdout);
            if let Some((start, end)) = find_json_bounds(&text) {
                let maybe_json = &text[start..=end];
                return serde_json::from_str(maybe_json).map_err(|secondary_err| {
                    format!(
                        "failed to parse json output.\nprimary: {primary_err}\nsecondary: {secondary_err}\nstdout:\n{text}\nstderr:\n{}",
                        String::from_utf8_lossy(stderr)
                    )
                });
            }

            Err(format!(
                "failed to parse json output: {primary_err}\nstdout:\n{text}\nstderr:\n{}",
                String::from_utf8_lossy(stderr)
            ))
        }
    }
}

fn find_json_bounds(text: &str) -> Option<(usize, usize)> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end >= start {
        Some((start, end))
    } else {
        None
    }
}

fn arcpack_default_config_override(fixture_dir: &Path) -> Option<String> {
    let arcpack_json = fixture_dir.join("arcpack.json");
    if arcpack_json.exists() {
        return None;
    }

    let railpack_json = fixture_dir.join("railpack.json");
    if railpack_json.exists() {
        return Some("railpack.json".to_string());
    }

    None
}

fn load_fixture_cases(fixture_dir: &Path) -> Vec<FixtureCase> {
    let test_json = fixture_dir.join("test.json");
    if !test_json.exists() {
        return vec![FixtureCase::default()];
    }

    let raw = match std::fs::read_to_string(&test_json) {
        Ok(v) => v,
        Err(_) => return vec![FixtureCase::default()],
    };

    let parsed: Value = match json5::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return vec![FixtureCase::default()],
    };

    let mut cases = match parsed {
        Value::Array(items) => items.into_iter().filter_map(parse_fixture_case).collect(),
        Value::Object(_) => parse_fixture_case(parsed).into_iter().collect(),
        _ => vec![],
    };

    if cases.is_empty() {
        cases.push(FixtureCase::default());
    }

    cases
}

fn parse_fixture_case(value: Value) -> Option<FixtureCase> {
    let obj = value.as_object()?;

    let should_fail = obj
        .get("shouldFail")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let config_file = obj
        .get("configFile")
        .and_then(Value::as_str)
        .map(|s| s.to_string());

    let mut envs = Vec::new();
    if let Some(env_obj) = obj
        .get("envs")
        .or_else(|| obj.get("env"))
        .and_then(Value::as_object)
    {
        for (k, v) in env_obj {
            let value = match v {
                Value::String(s) => s.clone(),
                Value::Bool(_) | Value::Number(_) => v.to_string(),
                Value::Null => String::new(),
                _ => continue,
            };
            envs.push((k.clone(), value));
        }
    }
    envs.sort_by(|a, b| a.0.cmp(&b.0));

    Some(FixtureCase {
        envs,
        should_fail,
        config_file,
    })
}

fn adapt_envs_for_arcpack(envs: &[(String, String)]) -> Vec<(String, String)> {
    let mut mapped = envs.to_vec();

    for (k, v) in envs {
        if let Some(suffix) = k.strip_prefix("RAILPACK_") {
            let arcpack_key = format!("ARCPACK_{suffix}");
            if mapped.iter().all(|(key, _)| key != &arcpack_key) {
                mapped.push((arcpack_key, v.clone()));
            }
        }
    }

    mapped.sort_by(|a, b| a.0.cmp(&b.0));
    mapped
}

fn normalize_plan(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = BTreeMap::new();
            for (k, v) in map {
                if k == "$schema" {
                    continue;
                }

                if k == "secrets" {
                    if let Value::Array(items) = v {
                        let mut strings: Vec<String> = items
                            .into_iter()
                            .filter_map(|x| x.as_str().map(normalize_secret_name))
                            .collect();
                        strings.sort();
                        strings.dedup();
                        out.insert(
                            k,
                            Value::Array(strings.into_iter().map(Value::String).collect()),
                        );
                        continue;
                    }
                }

                // mise.toml 内容受离线解析策略影响较大，按键名过滤避免无效噪音
                if k == "assets" {
                    if let Value::Object(assets) = v {
                        let mut normalized_assets = BTreeMap::new();
                        for (asset_name, asset_value) in assets {
                            if asset_name.ends_with("mise.toml") {
                                continue;
                            }
                            normalized_assets.insert(asset_name, normalize_plan(asset_value));
                        }
                        if !normalized_assets.is_empty() {
                            out.insert(k, Value::Object(normalized_assets.into_iter().collect()));
                        }
                        continue;
                    }
                }

                if k == "cmd" {
                    if let Value::String(cmd) = v {
                        out.insert(k, Value::String(normalize_command_string(&cmd)));
                        continue;
                    }
                }

                if k == "startCommand" {
                    if let Value::String(cmd) = v {
                        out.insert(k, Value::String(normalize_command_string(&cmd)));
                        continue;
                    }
                }

                out.insert(k, normalize_plan(v));
            }
            Value::Object(out.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_plan).collect()),
        other => other,
    }
}

fn normalize_secret_name(secret: &str) -> String {
    if let Some(suffix) = secret.strip_prefix("ARCPACK_") {
        return format!("PACKCFG_{suffix}");
    }
    if let Some(suffix) = secret.strip_prefix("RAILPACK_") {
        return format!("PACKCFG_{suffix}");
    }
    secret.to_string()
}

fn normalize_command_string(cmd: &str) -> String {
    if let Some(rest) = cmd.strip_prefix("mise install-into ") {
        let Some((pkg_with_ver, path)) = rest.split_once(' ') else {
            return cmd.to_string();
        };
        let Some((pkg, _ver)) = pkg_with_ver.split_once('@') else {
            return cmd.to_string();
        };
        return format!("mise install-into {}@<resolved> {}", pkg, path);
    }

    if cmd.starts_with("java ") {
        let mut normalized = String::with_capacity(cmd.len());
        let mut prev_space = false;
        for ch in cmd.chars() {
            if ch == ' ' {
                if !prev_space {
                    normalized.push(ch);
                }
                prev_space = true;
            } else {
                normalized.push(ch);
                prev_space = false;
            }
        }
        return normalized.trim().to_string();
    }

    cmd.to_string()
}

fn format_plan_diff(arcpack: &Value, railpack: &Value) -> String {
    let mut output = String::new();

    if let Some(path) = first_diff_path(arcpack, railpack, "$".to_string()) {
        output.push_str(&format!("first differing path: {path}\n"));
    } else {
        output.push_str("plans differ but no path could be isolated\n");
    }

    let left = serde_json::to_string_pretty(arcpack).unwrap_or_else(|_| arcpack.to_string());
    let right = serde_json::to_string_pretty(railpack).unwrap_or_else(|_| railpack.to_string());
    let (line_no, left_line, right_line) = first_diff_line(&left, &right);

    output.push_str(&format!(
        "first differing line: {line_no}\narcpack: {left_line}\nrailpack: {right_line}\n"
    ));

    output
}

fn first_diff_line(left: &str, right: &str) -> (usize, String, String) {
    let left_lines: Vec<&str> = left.lines().collect();
    let right_lines: Vec<&str> = right.lines().collect();
    let max = left_lines.len().max(right_lines.len());

    for i in 0..max {
        let l = left_lines.get(i).copied().unwrap_or("<missing>");
        let r = right_lines.get(i).copied().unwrap_or("<missing>");
        if l != r {
            return (i + 1, l.to_string(), r.to_string());
        }
    }

    (0, "<same>".to_string(), "<same>".to_string())
}

fn first_diff_path(left: &Value, right: &Value, path: String) -> Option<String> {
    match (left, right) {
        (Value::Object(a), Value::Object(b)) => {
            let mut keys: BTreeSet<&String> = BTreeSet::new();
            keys.extend(a.keys());
            keys.extend(b.keys());

            for key in keys {
                let next = if path == "$" {
                    format!("$.{key}")
                } else {
                    format!("{path}.{key}")
                };
                match (a.get(key), b.get(key)) {
                    (Some(av), Some(bv)) => {
                        if let Some(p) = first_diff_path(av, bv, next) {
                            return Some(p);
                        }
                    }
                    _ => return Some(next),
                }
            }
            None
        }
        (Value::Array(a), Value::Array(b)) => {
            let len = a.len().max(b.len());
            for i in 0..len {
                let next = format!("{path}[{i}]");
                match (a.get(i), b.get(i)) {
                    (Some(av), Some(bv)) => {
                        if let Some(p) = first_diff_path(av, bv, next) {
                            return Some(p);
                        }
                    }
                    _ => return Some(next),
                }
            }
            None
        }
        _ => {
            if left != right {
                Some(path)
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_json_bounds() {
        let text = "prefix\n{\n  \"a\": 1\n}\nsuffix";
        assert_eq!(find_json_bounds(text), Some((7, 18)));
    }

    #[test]
    fn test_normalize_plan_removes_schema_recursively() {
        let value = serde_json::json!({
            "$schema": "https://example.com/schema.json",
            "steps": [
                { "name": "build", "$schema": "ignored" }
            ]
        });

        let normalized = normalize_plan(value);
        assert!(normalized.get("$schema").is_none());
        assert!(
            normalized["steps"][0].get("$schema").is_none(),
            "nested $schema should be removed"
        );
    }

    #[test]
    fn test_first_diff_path_object() {
        let a = serde_json::json!({ "a": 1, "b": { "c": 2 } });
        let b = serde_json::json!({ "a": 1, "b": { "c": 3 } });
        assert_eq!(
            first_diff_path(&a, &b, "$".to_string()),
            Some("$.b.c".to_string())
        );
    }

    #[test]
    fn test_normalize_secret_name_equates_prefixes() {
        assert_eq!(
            normalize_secret_name("ARCPACK_BUILD_CMD"),
            "PACKCFG_BUILD_CMD"
        );
        assert_eq!(
            normalize_secret_name("RAILPACK_BUILD_CMD"),
            "PACKCFG_BUILD_CMD"
        );
        assert_eq!(normalize_secret_name("DATABASE_URL"), "DATABASE_URL");
    }

    #[test]
    fn test_normalize_command_string_mise_install_into() {
        let raw = "mise install-into caddy@latest /railpack/caddy";
        assert_eq!(
            normalize_command_string(raw),
            "mise install-into caddy@<resolved> /railpack/caddy"
        );
        assert_eq!(normalize_command_string("npm ci"), "npm ci");
    }

    #[test]
    fn test_normalize_command_string_java_whitespace() {
        let raw = "java $JAVA_OPTS -jar  $(ls -1 */build/libs/*jar | grep -v plain)";
        assert_eq!(
            normalize_command_string(raw),
            "java $JAVA_OPTS -jar $(ls -1 */build/libs/*jar | grep -v plain)"
        );
    }

    #[test]
    fn test_load_fixture_cases_parses_json5_array() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture_dir = tmp.path();
        let content = r#"[
          {
            // json5 comment
            "envs": { "B": "2", "A": "1" },
            "configFile": "railpack.other.json"
          },
          {
            "shouldFail": true,
            "env": { "PORT": 8080 }
          }
        ]"#;
        std::fs::write(fixture_dir.join("test.json"), content).unwrap();

        let cases = load_fixture_cases(fixture_dir);
        assert_eq!(cases.len(), 2);
        assert_eq!(
            cases[0],
            FixtureCase {
                envs: vec![
                    ("A".to_string(), "1".to_string()),
                    ("B".to_string(), "2".to_string())
                ],
                should_fail: false,
                config_file: Some("railpack.other.json".to_string()),
            }
        );
        assert_eq!(
            cases[1],
            FixtureCase {
                envs: vec![("PORT".to_string(), "8080".to_string())],
                should_fail: true,
                config_file: None,
            }
        );
    }

    #[test]
    fn test_adapt_envs_for_arcpack_adds_prefix_alias() {
        let envs = vec![
            ("RAILPACK_BUILD_CMD".to_string(), "echo build".to_string()),
            ("FOO".to_string(), "bar".to_string()),
        ];

        let mapped = adapt_envs_for_arcpack(&envs);
        assert!(mapped
            .iter()
            .any(|(k, v)| k == "RAILPACK_BUILD_CMD" && v == "echo build"));
        assert!(mapped
            .iter()
            .any(|(k, v)| k == "ARCPACK_BUILD_CMD" && v == "echo build"));
        assert!(mapped.iter().any(|(k, v)| k == "FOO" && v == "bar"));
    }

    #[test]
    fn test_adapt_envs_for_arcpack_does_not_duplicate_existing_arcpack_key() {
        let envs = vec![
            ("RAILPACK_START_CMD".to_string(), "echo rail".to_string()),
            ("ARCPACK_START_CMD".to_string(), "echo arc".to_string()),
        ];

        let mapped = adapt_envs_for_arcpack(&envs);
        let arcpack_values: Vec<&str> = mapped
            .iter()
            .filter(|(k, _)| k == "ARCPACK_START_CMD")
            .map(|(_, v)| v.as_str())
            .collect();

        assert_eq!(arcpack_values, vec!["echo arc"]);
    }
}
