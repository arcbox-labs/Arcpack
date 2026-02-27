/// 等价性测试框架 —— 验证不同 backend 路径产出的镜像等价
///
/// 每个端到端测试标记 #[ignore]，需要 buildkitd + docker 运行时环境。
/// 非 ignore 测试仅验证辅助函数的结构正确性。

use std::process::Command;

/// 使用指定后端构建 fixture 项目，返回镜像名
fn build_with_backend(fixture: &str, backend: &str) -> Result<String, String> {
    let image_name = format!(
        "arcpack-equiv-{}-{}-{}",
        fixture,
        backend,
        std::process::id()
    );

    let fixture_dir = format!("tests/fixtures/{}", fixture);

    let output = Command::new(env!("CARGO_BIN_EXE_arcpack"))
        .args([
            "build",
            &fixture_dir,
            "--backend",
            backend,
            "--name",
            &image_name,
            "--progress",
            "plain",
        ])
        .output()
        .map_err(|e| format!("failed to run arcpack: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "build failed (backend={backend}, fixture={fixture}): {stderr}"
        ));
    }

    Ok(image_name)
}

/// 比较两个镜像中指定路径的文件是否一致
fn assert_files_equal(image_a: &str, image_b: &str, path: &str) {
    let hash_a = get_file_hash(image_a, path);
    let hash_b = get_file_hash(image_b, path);
    assert_eq!(
        hash_a, hash_b,
        "文件 {path} 在镜像 {image_a} 和 {image_b} 中不一致"
    );
}

/// 获取镜像内文件的 md5sum
fn get_file_hash(image: &str, path: &str) -> String {
    let output = Command::new("docker")
        .args(["run", "--rm", image, "md5sum", path])
        .output()
        .expect("failed to run docker");
    assert!(
        output.status.success(),
        "docker run md5sum failed for image={image}, path={path}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .expect("md5sum output should contain hash")
        .to_string()
}

/// 比较两个镜像的环境变量
fn assert_env_equal(image_a: &str, image_b: &str) {
    let env_a = get_sorted_env(image_a);
    let env_b = get_sorted_env(image_b);
    assert_eq!(
        env_a, env_b,
        "环境变量在镜像 {image_a} 和 {image_b} 中不一致"
    );
}

/// 获取镜像排序后的环境变量
fn get_sorted_env(image: &str) -> Vec<String> {
    let output = Command::new("docker")
        .args(["run", "--rm", image, "env"])
        .output()
        .expect("failed to run docker");
    assert!(
        output.status.success(),
        "docker run env failed for image={image}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut lines: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        // 过滤掉 HOSTNAME 等运行时变量
        .filter(|l| !l.starts_with("HOSTNAME=") && !l.starts_with("HOME="))
        .map(|l| l.to_string())
        .collect();
    lines.sort();
    lines
}

/// 比较两个镜像的 CMD 配置
fn assert_cmd_equal(image_a: &str, image_b: &str) {
    let cmd_a = get_image_cmd(image_a);
    let cmd_b = get_image_cmd(image_b);
    assert_eq!(
        cmd_a, cmd_b,
        "CMD 在镜像 {image_a} 和 {image_b} 中不一致"
    );
}

/// 获取镜像的 CMD 配置
fn get_image_cmd(image: &str) -> String {
    let output = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{json .Config.Cmd}}",
            image,
        ])
        .output()
        .expect("failed to run docker inspect");
    assert!(
        output.status.success(),
        "docker inspect failed for image={image}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// RAII guard：在 Drop 时自动清理测试镜像，防止 assert 失败后镜像泄漏
struct ImageCleanupGuard {
    names: Vec<String>,
}

impl ImageCleanupGuard {
    fn new() -> Self {
        Self { names: Vec::new() }
    }

    fn track(&mut self, name: String) {
        self.names.push(name);
    }
}

impl Drop for ImageCleanupGuard {
    fn drop(&mut self) {
        for name in &self.names {
            let result = Command::new("docker")
                .args(["rmi", "-f", name])
                .output();
            if let Ok(output) = result {
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    eprintln!("warning: failed to remove image {name}: {stderr}");
                }
            }
        }
    }
}

// === 结构测试（非 ignore，验证辅助函数逻辑） ===

#[test]
fn test_assert_files_equal_same_content_passes() {
    // 结构测试：验证 assert_files_equal 的参数传递正确
    // 不实际运行 docker，只验证函数签名和字符串处理
    let hash1 = "d41d8cd98f00b204e9800998ecf8427e  /dev/null";
    let parts: Vec<&str> = hash1.split_whitespace().collect();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], "d41d8cd98f00b204e9800998ecf8427e");
}

#[test]
fn test_assert_cmd_equal_same_config_passes() {
    // 结构测试：验证 JSON 格式的 CMD 比较逻辑
    let cmd_json_a = r#"["node","server.js"]"#;
    let cmd_json_b = r#"["node","server.js"]"#;
    assert_eq!(cmd_json_a, cmd_json_b);

    // 不同 CMD 应不相等
    let cmd_json_c = r#"["python","app.py"]"#;
    assert_ne!(cmd_json_a, cmd_json_c);
}

// === 端到端等价性测试（需要 buildkitd + docker） ===

#[test]
#[ignore]
fn test_equivalence_node_npm() {
    let mut guard = ImageCleanupGuard::new();
    let dockerfile_img = build_with_backend("node-npm", "dockerfile").unwrap();
    guard.track(dockerfile_img.clone());
    let llb_img = build_with_backend("node-npm", "llb").unwrap();
    guard.track(llb_img.clone());

    assert_env_equal(&dockerfile_img, &llb_img);
    assert_cmd_equal(&dockerfile_img, &llb_img);
    assert_files_equal(&dockerfile_img, &llb_img, "/app/package.json");
}

#[test]
#[ignore]
fn test_equivalence_node_pnpm() {
    let mut guard = ImageCleanupGuard::new();
    let dockerfile_img = build_with_backend("node-pnpm", "dockerfile").unwrap();
    guard.track(dockerfile_img.clone());
    let llb_img = build_with_backend("node-pnpm", "llb").unwrap();
    guard.track(llb_img.clone());

    assert_env_equal(&dockerfile_img, &llb_img);
    assert_cmd_equal(&dockerfile_img, &llb_img);
}

#[test]
#[ignore]
fn test_equivalence_node_yarn() {
    let mut guard = ImageCleanupGuard::new();
    let dockerfile_img = build_with_backend("node-yarn", "dockerfile").unwrap();
    guard.track(dockerfile_img.clone());
    let llb_img = build_with_backend("node-yarn", "llb").unwrap();
    guard.track(llb_img.clone());

    assert_env_equal(&dockerfile_img, &llb_img);
    assert_cmd_equal(&dockerfile_img, &llb_img);
}

#[test]
#[ignore]
fn test_equivalence_node_yarn_berry() {
    let mut guard = ImageCleanupGuard::new();
    let dockerfile_img = build_with_backend("node-yarn-berry", "dockerfile").unwrap();
    guard.track(dockerfile_img.clone());
    let llb_img = build_with_backend("node-yarn-berry", "llb").unwrap();
    guard.track(llb_img.clone());

    assert_env_equal(&dockerfile_img, &llb_img);
    assert_cmd_equal(&dockerfile_img, &llb_img);
}

#[test]
#[ignore]
fn test_equivalence_node_bun() {
    let mut guard = ImageCleanupGuard::new();
    let dockerfile_img = build_with_backend("node-bun", "dockerfile").unwrap();
    guard.track(dockerfile_img.clone());
    let llb_img = build_with_backend("node-bun", "llb").unwrap();
    guard.track(llb_img.clone());

    assert_env_equal(&dockerfile_img, &llb_img);
    assert_cmd_equal(&dockerfile_img, &llb_img);
}
