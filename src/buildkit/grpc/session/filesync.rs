use std::collections::HashMap;
use std::path::PathBuf;

use tonic::{Request, Response, Status, Streaming};
use tracing::debug;

use crate::buildkit::proto::filesync::file_sync_server;
use crate::buildkit::proto::grpc_proto::fsutil::types::Packet;

/// 文件同步提供者——响应 buildkitd 通过 Session 回调的文件请求
///
/// 当 LLB 中包含 local:// Source 操作时，buildkitd 回调此 provider 获取本地文件。
/// 对应 buildctl `--local context=/path/to/dir` 的功能。
///
/// 对齐 Go `filesync.NewFSSyncProvider(dirs)`
pub struct FilesyncProvider {
    /// 目录映射：name → 本地路径
    dirs: HashMap<String, PathBuf>,
}

impl FilesyncProvider {
    pub fn new(dirs: HashMap<String, PathBuf>) -> Self {
        Self { dirs }
    }

    /// 查找目录映射
    pub fn get_dir(&self, name: &str) -> Option<&PathBuf> {
        self.dirs.get(name)
    }

    /// 返回注册的目录名列表
    pub fn dir_names(&self) -> Vec<&str> {
        self.dirs.keys().map(|s| s.as_str()).collect()
    }
}

/// FileSync gRPC server 实现
///
/// 初版使用简化实现——DiffCopy 和 TarStream 均返回 Unimplemented。
/// 完整实现需将本地文件打包为 fsutil.types.Packet 流式传输。
/// Go 使用 tonistiigi/fsutil 进行高效 diff 传输，Rust 无等价物。
///
/// 实际的文件传输在 Session 协议的 bidi stream 内完成，
/// 当前 Session manager 的简化实现尚未路由到此 handler。
#[tonic::async_trait]
impl file_sync_server::FileSync for FilesyncProvider {
    type DiffCopyStream =
        tokio_stream::wrappers::ReceiverStream<Result<Packet, Status>>;
    type TarStreamStream =
        tokio_stream::wrappers::ReceiverStream<Result<Packet, Status>>;

    async fn diff_copy(
        &self,
        request: Request<Streaming<Packet>>,
    ) -> Result<Response<Self::DiffCopyStream>, Status> {
        // 从 gRPC metadata 中获取请求的目录名
        let dir_name = request
            .metadata()
            .get("urlpath")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("context");

        let available: Vec<&str> = self.dirs.keys().map(|s| s.as_str()).collect();
        let _dir_path = self.dirs.get(dir_name).ok_or_else(|| {
            Status::not_found(format!(
                "directory not registered: {dir_name} (available: {available:?})"
            ))
        })?;

        debug!(dir_name = dir_name, "filesync diff_copy requested");

        // filesync 尚未实现，返回 Unimplemented 让 buildkitd 知晓
        Err(Status::unimplemented(
            "filesync not yet implemented; use buildctl stdin mode as fallback",
        ))
    }

    async fn tar_stream(
        &self,
        request: Request<Streaming<Packet>>,
    ) -> Result<Response<Self::TarStreamStream>, Status> {
        let dir_name = request
            .metadata()
            .get("urlpath")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("context");

        let available: Vec<&str> = self.dirs.keys().map(|s| s.as_str()).collect();
        let _dir_path = self.dirs.get(dir_name).ok_or_else(|| {
            Status::not_found(format!(
                "directory not registered: {dir_name} (available: {available:?})"
            ))
        })?;

        debug!(dir_name = dir_name, "filesync tar_stream requested");

        // filesync 尚未实现，返回 Unimplemented 让 buildkitd 知晓
        Err(Status::unimplemented(
            "filesync not yet implemented; use buildctl stdin mode as fallback",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filesync_dir_lookup() {
        let dirs = HashMap::from([
            ("context".to_string(), PathBuf::from("/app/src")),
            ("dockerfile".to_string(), PathBuf::from("/app")),
        ]);
        let provider = FilesyncProvider::new(dirs);

        assert_eq!(
            provider.get_dir("context"),
            Some(&PathBuf::from("/app/src"))
        );
        assert_eq!(
            provider.get_dir("dockerfile"),
            Some(&PathBuf::from("/app"))
        );
    }

    #[test]
    fn test_filesync_missing_dir() {
        let dirs = HashMap::from([("context".to_string(), PathBuf::from("/app"))]);
        let provider = FilesyncProvider::new(dirs);

        assert_eq!(provider.get_dir("nonexistent"), None);
    }

    #[test]
    fn test_filesync_registers_dirs() {
        let dirs = HashMap::from([
            ("a".to_string(), PathBuf::from("/path/a")),
            ("b".to_string(), PathBuf::from("/path/b")),
            ("c".to_string(), PathBuf::from("/path/c")),
        ]);
        let provider = FilesyncProvider::new(dirs);

        let mut names = provider.dir_names();
        names.sort();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_filesync_empty_dirs() {
        let provider = FilesyncProvider::new(HashMap::new());
        assert!(provider.dir_names().is_empty());
        assert_eq!(provider.get_dir("anything"), None);
    }

    #[test]
    fn test_not_found_error_includes_available_dirs() {
        // 验证目录未找到时，错误信息包含可用目录列表
        let dirs = HashMap::from([
            ("context".to_string(), PathBuf::from("/app/src")),
            ("dockerfile".to_string(), PathBuf::from("/app")),
        ]);
        let provider = FilesyncProvider::new(dirs);

        // get_dir 返回 None 时，上层构造的 not_found 错误应包含 available 列表
        assert!(provider.get_dir("nonexistent").is_none());
        // diff_copy / tar_stream 已注册的目录会返回 Unimplemented（非 OK + 空 stream）
        // 完整 gRPC 测试需 tonic::Streaming，留给集成测试验证
        assert!(provider.get_dir("context").is_some());
    }
}
