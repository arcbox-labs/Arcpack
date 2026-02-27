/// 结构化日志收集器
///
/// 对齐 railpack `core/logger/logger.go`。
/// 在构建过程中收集 Info/Warn/Error 消息，最终序列化到 BuildResult.logs。
use crate::{LogLevel, LogMsg};

#[derive(Debug, Clone, Default)]
pub struct LogCollector {
    entries: Vec<LogMsg>,
}

impl LogCollector {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// 记录 Info 级别日志
    pub fn info(&mut self, message: impl Into<String>) {
        let msg = message.into();
        tracing::info!("{}", msg);
        self.entries.push(LogMsg {
            level: LogLevel::Info,
            message: msg,
        });
    }

    /// 记录 Warn 级别日志
    pub fn warn(&mut self, message: impl Into<String>) {
        let msg = message.into();
        tracing::warn!("{}", msg);
        self.entries.push(LogMsg {
            level: LogLevel::Warn,
            message: msg,
        });
    }

    /// 记录 Error 级别日志
    pub fn error(&mut self, message: impl Into<String>) {
        let msg = message.into();
        tracing::error!("{}", msg);
        self.entries.push(LogMsg {
            level: LogLevel::Error,
            message: msg,
        });
    }

    /// 获取所有收集的日志
    pub fn into_logs(self) -> Vec<LogMsg> {
        self.entries
    }

    /// 获取日志引用
    pub fn logs(&self) -> &[LogMsg] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_collector_info() {
        let mut lc = LogCollector::new();
        lc.info("detected Node.js project");
        assert_eq!(lc.logs().len(), 1);
        assert!(matches!(lc.logs()[0].level, LogLevel::Info));
        assert_eq!(lc.logs()[0].message, "detected Node.js project");
    }

    #[test]
    fn test_log_collector_warn() {
        let mut lc = LogCollector::new();
        lc.warn("no start command found");
        assert_eq!(lc.logs().len(), 1);
        assert!(matches!(lc.logs()[0].level, LogLevel::Warn));
    }

    #[test]
    fn test_log_collector_multiple() {
        let mut lc = LogCollector::new();
        lc.info("step 1");
        lc.warn("step 2");
        lc.error("step 3");
        let logs = lc.into_logs();
        assert_eq!(logs.len(), 3);
        assert!(matches!(logs[0].level, LogLevel::Info));
        assert!(matches!(logs[1].level, LogLevel::Warn));
        assert!(matches!(logs[2].level, LogLevel::Error));
    }
}
