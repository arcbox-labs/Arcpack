/// HTTP 健康检查模块
///
/// 启动容器，轮询 HTTP 端点直到满足预期条件。

use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use super::test_config::HttpCheck;

/// 验证 HTTP 健康检查
pub fn verify_http(image_tag: &str, check: &HttpCheck) -> Result<(), String> {
    // 启动容器（后台，暴露端口），直接从 stdout 获取容器 ID
    let container_id = start_container(image_tag)?;

    // 获取映射端口
    let port = get_mapped_port(&container_id)?;

    let url = format!("http://127.0.0.1:{}{}", port, check.path);
    let retry_interval = Duration::from_secs(2);
    let deadline = Instant::now() + Duration::from_secs(check.timeout_secs);

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("failed to build http client: {}", e))?;

    let mut last_error = String::new();

    for attempt in 1..=check.retries {
        if Instant::now() > deadline {
            break;
        }

        thread::sleep(retry_interval);

        match client.get(&url).send() {
            Ok(response) => {
                let status = response.status().as_u16();
                if status != check.expected_status {
                    last_error = format!(
                        "attempt {}: expected status {}, got {}",
                        attempt, check.expected_status, status
                    );
                    continue;
                }

                // 检查响应体
                if let Some(ref expected_body) = check.expected_body {
                    let body = response.text().unwrap_or_default();
                    if !body.contains(expected_body) {
                        last_error = format!(
                            "attempt {}: expected body containing '{}', got: {}",
                            attempt, expected_body, body
                        );
                        continue;
                    }
                }

                // 通过
                stop_container(&container_id);
                return Ok(());
            }
            Err(e) => {
                last_error = format!("attempt {}: {}", attempt, e);
            }
        }
    }

    // 超时或重试耗尽
    stop_container(&container_id);
    Err(format!(
        "HTTP check failed (retries={}, timeout={}s): {}",
        check.retries,
        check.timeout_secs,
        last_error
    ))
}

/// 启动后台容器，返回容器 ID
fn start_container(image_tag: &str) -> Result<String, String> {
    let output = Command::new("docker")
        .args(["run", "--rm", "-d", "-P", image_tag])
        .output()
        .map_err(|e| format!("failed to start container: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "container failed to start: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if id.is_empty() {
        return Err("docker run -d returned empty container ID".to_string());
    }

    Ok(id)
}

/// 获取容器映射端口
fn get_mapped_port(container_id: &str) -> Result<u16, String> {
    let output = Command::new("docker")
        .args(["port", container_id, "3000"])
        .output()
        .map_err(|e| format!("failed to get port: {}", e))?;

    let port_str = String::from_utf8_lossy(&output.stdout);
    // 输出格式: "0.0.0.0:PORT" 或 ":::PORT"
    let port = port_str.trim()
        .rsplit(':')
        .next()
        .and_then(|p| p.parse::<u16>().ok())
        .ok_or_else(|| format!("failed to parse port from: {}", port_str))?;

    Ok(port)
}

/// 停止容器
fn stop_container(container_id: &str) {
    let _ = Command::new("docker")
        .args(["stop", "-t", "2", container_id])
        .output();
}
