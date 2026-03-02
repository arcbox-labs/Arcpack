use std::time::Duration;

use crate::buildkit::proto::control::{StatusResponse, Vertex, VertexLog};

/// 构建进度事件（从 StatusResponse 解析）
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// 构建步骤开始
    VertexStarted { id: String, name: String },
    /// 构建步骤完成
    VertexCompleted {
        id: String,
        name: String,
        duration: Duration,
        cached: bool,
    },
    /// 构建步骤失败
    VertexError {
        id: String,
        name: String,
        error: String,
    },
    /// 步骤日志输出
    Log { vertex_id: String, data: Vec<u8> },
}

/// 进度渲染模式（对齐 buildctl --progress）
#[derive(Debug, Clone, Default)]
pub enum ProgressMode {
    #[default]
    Auto,
    Plain,
    Tty,
    Quiet,
}

/// 从 StatusResponse 解析进度事件
pub fn parse_status_response(resp: &StatusResponse) -> Vec<ProgressEvent> {
    let mut events = Vec::new();

    for vertex in &resp.vertexes {
        events.extend(parse_vertex(vertex));
    }

    for log in &resp.logs {
        events.push(parse_log(log));
    }

    events
}

/// 解析单个 Vertex 为进度事件
fn parse_vertex(v: &Vertex) -> Vec<ProgressEvent> {
    let mut events = Vec::new();

    if !v.error.is_empty() {
        // 错误状态优先
        events.push(ProgressEvent::VertexError {
            id: v.digest.clone(),
            name: v.name.clone(),
            error: v.error.clone(),
        });
    } else if v.completed.is_some() {
        // 已完成（可能是 cached）
        let duration = compute_duration(v);
        events.push(ProgressEvent::VertexCompleted {
            id: v.digest.clone(),
            name: v.name.clone(),
            duration,
            cached: v.cached,
        });
    } else if v.started.is_some() {
        // 正在运行
        events.push(ProgressEvent::VertexStarted {
            id: v.digest.clone(),
            name: v.name.clone(),
        });
    }

    events
}

/// 计算 Vertex 执行时长
fn compute_duration(v: &Vertex) -> Duration {
    match (&v.started, &v.completed) {
        (Some(start), Some(end)) => {
            let start_nanos = start.seconds as u64 * 1_000_000_000 + start.nanos.max(0) as u64;
            let end_nanos = end.seconds as u64 * 1_000_000_000 + end.nanos.max(0) as u64;
            Duration::from_nanos(end_nanos.saturating_sub(start_nanos))
        }
        _ => Duration::ZERO,
    }
}

fn parse_log(log: &VertexLog) -> ProgressEvent {
    ProgressEvent::Log {
        vertex_id: log.vertex.clone(),
        data: log.msg.clone(),
    }
}

/// Plain 模式渲染（逐行输出）
pub fn render_plain(event: &ProgressEvent) -> String {
    match event {
        ProgressEvent::VertexStarted { name, .. } => {
            format!("[{name}] RUNNING")
        }
        ProgressEvent::VertexCompleted {
            name,
            duration,
            cached,
            ..
        } => {
            if *cached {
                format!("[{name}] CACHED")
            } else {
                format!("[{name}] DONE {:.1}s", duration.as_secs_f64())
            }
        }
        ProgressEvent::VertexError { name, error, .. } => {
            format!("[{name}] ERROR: {error}")
        }
        ProgressEvent::Log { data, .. } => {
            let text = String::from_utf8_lossy(data);
            // 去掉末尾换行，避免重复空行
            let text = text.trim_end_matches('\n');
            format!("  | {text}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_types::Timestamp;

    fn make_vertex(
        digest: &str,
        name: &str,
        started: Option<Timestamp>,
        completed: Option<Timestamp>,
        cached: bool,
        error: &str,
    ) -> Vertex {
        Vertex {
            digest: digest.to_string(),
            name: name.to_string(),
            inputs: vec![],
            cached,
            started,
            completed,
            error: error.to_string(),
            progress_group: None,
        }
    }

    fn ts(seconds: i64, nanos: i32) -> Timestamp {
        Timestamp { seconds, nanos }
    }

    #[test]
    fn test_parse_vertex_started() {
        let resp = StatusResponse {
            vertexes: vec![make_vertex(
                "sha256:abc",
                "RUN apt-get install",
                Some(ts(1000, 0)),
                None,
                false,
                "",
            )],
            statuses: vec![],
            logs: vec![],
            warnings: vec![],
        };

        let events = parse_status_response(&resp);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProgressEvent::VertexStarted { id, name } => {
                assert_eq!(id, "sha256:abc");
                assert_eq!(name, "RUN apt-get install");
            }
            other => panic!("expected VertexStarted, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_vertex_completed() {
        let resp = StatusResponse {
            vertexes: vec![make_vertex(
                "sha256:def",
                "COPY . /app",
                Some(ts(1000, 0)),
                Some(ts(1002, 500_000_000)),
                false,
                "",
            )],
            statuses: vec![],
            logs: vec![],
            warnings: vec![],
        };

        let events = parse_status_response(&resp);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProgressEvent::VertexCompleted {
                id,
                name,
                duration,
                cached,
            } => {
                assert_eq!(id, "sha256:def");
                assert_eq!(name, "COPY . /app");
                assert!(!cached);
                assert_eq!(duration.as_millis(), 2500);
            }
            other => panic!("expected VertexCompleted, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_vertex_cached() {
        let resp = StatusResponse {
            vertexes: vec![make_vertex(
                "sha256:ghi",
                "RUN npm install",
                Some(ts(1000, 0)),
                Some(ts(1000, 100_000)),
                true,
                "",
            )],
            statuses: vec![],
            logs: vec![],
            warnings: vec![],
        };

        let events = parse_status_response(&resp);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProgressEvent::VertexCompleted { cached, .. } => {
                assert!(cached);
            }
            other => panic!("expected VertexCompleted, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_vertex_error() {
        let resp = StatusResponse {
            vertexes: vec![make_vertex(
                "sha256:err",
                "RUN make build",
                Some(ts(1000, 0)),
                None,
                false,
                "exit code: 1",
            )],
            statuses: vec![],
            logs: vec![],
            warnings: vec![],
        };

        let events = parse_status_response(&resp);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProgressEvent::VertexError { id, name, error } => {
                assert_eq!(id, "sha256:err");
                assert_eq!(name, "RUN make build");
                assert_eq!(error, "exit code: 1");
            }
            other => panic!("expected VertexError, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_log() {
        let resp = StatusResponse {
            vertexes: vec![],
            statuses: vec![],
            logs: vec![VertexLog {
                vertex: "sha256:abc".to_string(),
                timestamp: Some(ts(1000, 0)),
                stream: 1,
                msg: b"Installing packages...\n".to_vec(),
            }],
            warnings: vec![],
        };

        let events = parse_status_response(&resp);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProgressEvent::Log { vertex_id, data } => {
                assert_eq!(vertex_id, "sha256:abc");
                assert_eq!(data, b"Installing packages...\n");
            }
            other => panic!("expected Log, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_empty_response() {
        let resp = StatusResponse {
            vertexes: vec![],
            statuses: vec![],
            logs: vec![],
            warnings: vec![],
        };

        let events = parse_status_response(&resp);
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_vertex_no_started() {
        // Vertex 无 started 也无 completed → 不产生事件
        let resp = StatusResponse {
            vertexes: vec![make_vertex("sha256:x", "step", None, None, false, "")],
            statuses: vec![],
            logs: vec![],
            warnings: vec![],
        };

        let events = parse_status_response(&resp);
        assert!(events.is_empty());
    }

    #[test]
    fn test_render_plain_running() {
        let event = ProgressEvent::VertexStarted {
            id: "sha256:abc".to_string(),
            name: "RUN apt-get install".to_string(),
        };
        assert_eq!(render_plain(&event), "[RUN apt-get install] RUNNING");
    }

    #[test]
    fn test_render_plain_done() {
        let event = ProgressEvent::VertexCompleted {
            id: "sha256:def".to_string(),
            name: "COPY . /app".to_string(),
            duration: Duration::from_millis(2500),
            cached: false,
        };
        assert_eq!(render_plain(&event), "[COPY . /app] DONE 2.5s");
    }

    #[test]
    fn test_render_plain_cached() {
        let event = ProgressEvent::VertexCompleted {
            id: "sha256:ghi".to_string(),
            name: "RUN npm install".to_string(),
            duration: Duration::from_millis(0),
            cached: true,
        };
        assert_eq!(render_plain(&event), "[RUN npm install] CACHED");
    }

    #[test]
    fn test_render_plain_error() {
        let event = ProgressEvent::VertexError {
            id: "sha256:err".to_string(),
            name: "RUN make build".to_string(),
            error: "exit code: 1".to_string(),
        };
        assert_eq!(render_plain(&event), "[RUN make build] ERROR: exit code: 1");
    }

    #[test]
    fn test_render_plain_log() {
        let event = ProgressEvent::Log {
            vertex_id: "sha256:abc".to_string(),
            data: b"Hello world\n".to_vec(),
        };
        assert_eq!(render_plain(&event), "  | Hello world");
    }

    #[test]
    fn test_render_plain_log_no_trailing_newline() {
        let event = ProgressEvent::Log {
            vertex_id: "sha256:abc".to_string(),
            data: b"no newline".to_vec(),
        };
        assert_eq!(render_plain(&event), "  | no newline");
    }
}
