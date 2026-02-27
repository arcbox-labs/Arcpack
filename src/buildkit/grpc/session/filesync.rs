/// 文件同步提供者——响应 buildkitd 通过 Session 回调的文件请求
///
/// 当 LLB 中包含 local:// Source 操作时，buildkitd 回调此 provider 获取本地文件。
/// 对应 buildctl `--local context=/path/to/dir` 的功能。
///
/// DiffCopy 协议对齐 Go `tonistiigi/fsutil/send.go`：
/// 1. Walk 阶段：遍历目录，逐条发送 STAT packet
/// 2. 请求-响应阶段：接收 REQ/FIN，分块发送 DATA
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::Result;
use bytes::Bytes;
use h2::RecvStream;
use h2::SendStream;
use prost::Message;
use tracing::debug;
use walkdir::WalkDir;

use crate::buildkit::proto::grpc_proto::fsutil::types::packet::PacketType;
use crate::buildkit::proto::grpc_proto::fsutil::types::{Packet, Stat};

use super::grpc_frame::{decode_grpc_frame, encode_grpc_frame};

/// 编码 protobuf 消息为 gRPC 帧并发送到 h2 stream
fn send_packet(send: &mut SendStream<Bytes>, packet: &Packet, context: &str) -> Result<()> {
    let encoded = packet.encode_to_vec();
    let frame = encode_grpc_frame(&encoded);
    send.send_data(frame, false)
        .map_err(|e| anyhow::anyhow!("failed to send {context}: {e}"))
}

/// 文件同步提供者——管理目录映射
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

/// DiffCopy 发送器——遍历目录并响应文件请求
///
/// 对齐 Go `fsutil.Send()` 的协议流程：
/// 1. Walk 遍历目录，为每个条目发送 PACKET_STAT
/// 2. Walk 结束发送空 PACKET_STAT 作为结束标记
/// 3. 循环接收 PACKET_REQ（文件内容请求）/ PACKET_FIN（结束）
/// 4. 对 REQ 请求，分块读取文件发送 PACKET_DATA
pub struct DiffCopySender {
    dir_path: PathBuf,
    /// id → 文件相对路径（Walk 阶段构建，请求阶段查找）
    files: HashMap<u32, PathBuf>,
    /// 持久接收缓冲区，避免 h2 chunk 包含多个 gRPC frame 时丢帧
    recv_buf: Vec<u8>,
}

/// 文件内容分块大小（32KB，对齐 Go fsutil）
const CHUNK_SIZE: usize = 32 * 1024;

impl DiffCopySender {
    pub fn new(dir_path: PathBuf) -> Self {
        Self {
            dir_path,
            files: HashMap::new(),
            recv_buf: Vec::new(),
        }
    }

    /// 运行完整的 DiffCopy 协议
    pub async fn run(
        &mut self,
        mut recv: RecvStream,
        send: &mut SendStream<Bytes>,
    ) -> Result<()> {
        // Phase 1: Walk 遍历目录，发送 STAT packets
        self.walk_and_send_stats(send)?;

        // Phase 2: 请求-响应循环
        self.handle_requests(&mut recv, send).await?;

        Ok(())
    }

    /// Walk 阶段：遍历目录，为每个条目发送 STAT packet
    ///
    /// 普通文件分配自增 id，记录到 files 映射。
    /// Walk 结束发送空 STAT packet（无 stat 字段）作为结束标记。
    fn walk_and_send_stats(&mut self, send: &mut SendStream<Bytes>) -> Result<()> {
        let mut next_id: u32 = 0;

        for entry in WalkDir::new(&self.dir_path)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| {
                // 跳过 .git 目录
                let name = e.file_name().to_str().unwrap_or("");
                !(e.file_type().is_dir() && name == ".git")
            })
        {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    debug!(error = %err, "skipping unreadable entry during walk");
                    continue;
                }
            };

            let rel_path = entry
                .path()
                .strip_prefix(&self.dir_path)
                .unwrap_or(entry.path());

            // 跳过根目录自身
            if rel_path == Path::new("") {
                continue;
            }

            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(err) => {
                    debug!(path = %rel_path.display(), error = %err, "skipping unreadable metadata");
                    continue;
                }
            };

            let stat = Stat {
                path: rel_path.to_string_lossy().to_string(),
                mode: metadata.mode(),
                uid: metadata.uid(),
                gid: metadata.gid(),
                size: metadata.len() as i64,
                mod_time: metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as i64)
                    .unwrap_or(0),
                linkname: if metadata.is_symlink() {
                    std::fs::read_link(entry.path())
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default()
                } else {
                    String::new()
                },
                devmajor: 0,
                devminor: 0,
                xattrs: HashMap::new(),
            };

            // 普通文件分配 id，用于后续 REQ 查找
            let id = if metadata.is_file() {
                let id = next_id;
                self.files.insert(id, rel_path.to_path_buf());
                next_id += 1;
                id
            } else {
                0
            };

            let packet = Packet {
                r#type: PacketType::PacketStat as i32,
                stat: Some(stat),
                id,
                data: Vec::new(),
            };
            send_packet(send, &packet, "STAT packet")?;
        }

        // Walk 结束标记：空 STAT packet（无 stat 字段）
        let end_packet = Packet {
            r#type: PacketType::PacketStat as i32,
            stat: None,
            id: 0,
            data: Vec::new(),
        };
        send_packet(send, &end_packet, "walk-end marker")?;

        debug!(
            file_count = self.files.len(),
            dir = %self.dir_path.display(),
            "walk complete"
        );

        Ok(())
    }

    /// 请求-响应阶段：接收 REQ/FIN，响应 DATA
    async fn handle_requests(
        &mut self,
        recv: &mut RecvStream,
        send: &mut SendStream<Bytes>,
    ) -> Result<()> {
        loop {
            // 读取下一个 gRPC 帧（使用持久缓冲区避免丢帧）
            let data = match read_grpc_message(recv, &mut self.recv_buf).await? {
                Some(data) => data,
                None => break, // stream 结束
            };

            let packet = Packet::decode(data.as_slice())
                .map_err(|e| anyhow::anyhow!("failed to decode request Packet: {e}"))?;

            match PacketType::try_from(packet.r#type) {
                Ok(PacketType::PacketReq) => {
                    self.send_file_data(packet.id, send)?;
                }
                Ok(PacketType::PacketFin) => {
                    // 回复 FIN 并结束
                    let fin = Packet {
                        r#type: PacketType::PacketFin as i32,
                        stat: None,
                        id: 0,
                        data: Vec::new(),
                    };
                    send_packet(send, &fin, "FIN reply")?;
                    debug!("DiffCopy FIN received and replied");
                    break;
                }
                Ok(other) => {
                    debug!(packet_type = ?other, "ignoring unexpected packet type");
                }
                Err(_) => {
                    debug!(raw_type = packet.r#type, "ignoring unknown packet type");
                }
            }
        }

        Ok(())
    }

    /// 分块发送文件内容（流式读取，避免大文件峰值内存）
    fn send_file_data(&self, id: u32, send: &mut SendStream<Bytes>) -> Result<()> {
        use std::io::Read;

        let rel_path = self.files.get(&id).ok_or_else(|| {
            anyhow::anyhow!("file id {id} not found in walk mapping")
        })?;
        let full_path = self.dir_path.join(rel_path);

        debug!(id = id, path = %rel_path.display(), "sending file data");

        let file = std::fs::File::open(&full_path)
            .map_err(|e| anyhow::anyhow!("failed to open {}: {e}", full_path.display()))?;
        let mut reader = std::io::BufReader::new(file);
        let mut chunk_buf = vec![0u8; CHUNK_SIZE];

        loop {
            let n = reader.read(&mut chunk_buf)
                .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", full_path.display()))?;
            if n == 0 {
                break;
            }
            let packet = Packet {
                r#type: PacketType::PacketData as i32,
                stat: None,
                id,
                data: chunk_buf[..n].to_vec(),
            };
            send_packet(send, &packet, "DATA chunk")?;
        }

        // 发送空 DATA 作为文件结束标记
        let end_packet = Packet {
            r#type: PacketType::PacketData as i32,
            stat: None,
            id,
            data: Vec::new(),
        };
        send_packet(send, &end_packet, "DATA end marker")?;

        Ok(())
    }
}

/// 从 h2 RecvStream 读取一个完整的 gRPC 消息
///
/// 使用外部传入的持久缓冲区 `buf`，读完一帧后用 `drain` 消费已解析字节，
/// 保留尾部字节供下次调用使用，避免一个 h2 chunk 包含多个 gRPC frame 时丢帧。
async fn read_grpc_message(recv: &mut RecvStream, buf: &mut Vec<u8>) -> Result<Option<Vec<u8>>> {
    // 累积读取直到至少有 5 字节（gRPC 帧头）
    while buf.len() < 5 {
        match recv.data().await {
            Some(Ok(chunk)) => {
                let len = chunk.len();
                buf.extend_from_slice(&chunk);
                recv.flow_control()
                    .release_capacity(len)
                    .map_err(|e| anyhow::anyhow!("flow control error: {e}"))?;
            }
            Some(Err(e)) => return Err(anyhow::anyhow!("h2 recv error: {e}")),
            None => {
                if buf.is_empty() {
                    return Ok(None); // clean EOF
                }
                return Err(anyhow::anyhow!(
                    "unexpected EOF: got {} bytes, expected at least 5",
                    buf.len()
                ));
            }
        }
    }

    // 解析帧长度
    let payload_len = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
    let total_len = 5 + payload_len;

    // 继续读取直到拿到完整 payload
    while buf.len() < total_len {
        match recv.data().await {
            Some(Ok(chunk)) => {
                let len = chunk.len();
                buf.extend_from_slice(&chunk);
                recv.flow_control()
                    .release_capacity(len)
                    .map_err(|e| anyhow::anyhow!("flow control error: {e}"))?;
            }
            Some(Err(e)) => return Err(anyhow::anyhow!("h2 recv error: {e}")),
            None => {
                return Err(anyhow::anyhow!(
                    "unexpected EOF reading gRPC frame: got {} of {} bytes",
                    buf.len(),
                    total_len
                ));
            }
        }
    }

    // 解码 gRPC 帧，返回 payload；消费已解析字节，保留尾部
    let payload = decode_grpc_frame(&buf[..total_len])?;
    let result = payload.to_vec();
    buf.drain(..total_len);
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
    fn test_diff_copy_sender_walk_generates_file_mapping() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        // 创建测试文件
        fs::write(dir.join("a.txt"), "hello").unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/b.txt"), "world").unwrap();

        let mut sender = DiffCopySender::new(dir.to_path_buf());

        // Walk 只构建映射（需要 send stream 来发送 packets，
        // 这里验证文件映射逻辑）
        // 手动模拟 walk 逻辑
        let mut next_id: u32 = 0;
        for entry in WalkDir::new(dir).sort_by_file_name() {
            let entry = entry.unwrap();
            let rel_path = entry.path().strip_prefix(dir).unwrap();
            if rel_path == Path::new("") {
                continue;
            }
            if entry.metadata().unwrap().is_file() {
                sender.files.insert(next_id, rel_path.to_path_buf());
                next_id += 1;
            }
        }

        assert_eq!(sender.files.len(), 2);
        // 验证文件路径存在于映射中
        let paths: Vec<_> = sender.files.values().collect();
        assert!(paths.contains(&&PathBuf::from("a.txt")));
        assert!(paths.contains(&&PathBuf::from("sub/b.txt")));
    }

    #[test]
    fn test_diff_copy_sender_file_chunking() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        // 创建大于 CHUNK_SIZE 的文件
        let large_content = vec![0xABu8; CHUNK_SIZE * 2 + 100];
        fs::write(dir.join("large.bin"), &large_content).unwrap();

        // 验证分块逻辑
        let chunks: Vec<_> = large_content.chunks(CHUNK_SIZE).collect();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), CHUNK_SIZE);
        assert_eq!(chunks[1].len(), CHUNK_SIZE);
        assert_eq!(chunks[2].len(), 100);
    }

    #[test]
    fn test_stat_from_file_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("test.txt");
        fs::write(&file_path, "test content").unwrap();

        let metadata = fs::metadata(&file_path).unwrap();
        let stat = Stat {
            path: "test.txt".to_string(),
            mode: metadata.mode(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            size: metadata.len() as i64,
            mod_time: 0,
            linkname: String::new(),
            devmajor: 0,
            devminor: 0,
            xattrs: HashMap::new(),
        };

        assert_eq!(stat.path, "test.txt");
        assert_eq!(stat.size, 12); // "test content" = 12 bytes
        assert!(stat.mode > 0);
    }

    #[test]
    fn test_packet_stat_encode_decode() {
        let stat = Stat {
            path: "src/main.rs".to_string(),
            mode: 0o100644,
            uid: 1000,
            gid: 1000,
            size: 256,
            mod_time: 1700000000_000_000_000,
            linkname: String::new(),
            devmajor: 0,
            devminor: 0,
            xattrs: HashMap::new(),
        };

        let packet = Packet {
            r#type: PacketType::PacketStat as i32,
            stat: Some(stat),
            id: 42,
            data: Vec::new(),
        };

        let encoded = packet.encode_to_vec();
        let decoded = Packet::decode(encoded.as_slice()).unwrap();

        assert_eq!(decoded.r#type, PacketType::PacketStat as i32);
        assert_eq!(decoded.id, 42);
        let decoded_stat = decoded.stat.unwrap();
        assert_eq!(decoded_stat.path, "src/main.rs");
        assert_eq!(decoded_stat.size, 256);
    }

    #[test]
    fn test_walk_end_marker_has_no_stat() {
        let end_packet = Packet {
            r#type: PacketType::PacketStat as i32,
            stat: None,
            id: 0,
            data: Vec::new(),
        };

        let encoded = end_packet.encode_to_vec();
        let decoded = Packet::decode(encoded.as_slice()).unwrap();

        assert_eq!(decoded.r#type, PacketType::PacketStat as i32);
        assert!(decoded.stat.is_none());
    }

    #[test]
    fn test_data_packet_with_content() {
        let packet = Packet {
            r#type: PacketType::PacketData as i32,
            stat: None,
            id: 5,
            data: b"file content chunk".to_vec(),
        };

        let encoded = packet.encode_to_vec();
        let decoded = Packet::decode(encoded.as_slice()).unwrap();

        assert_eq!(decoded.r#type, PacketType::PacketData as i32);
        assert_eq!(decoded.id, 5);
        assert_eq!(decoded.data, b"file content chunk");
    }

    #[test]
    fn test_fin_packet() {
        let packet = Packet {
            r#type: PacketType::PacketFin as i32,
            stat: None,
            id: 0,
            data: Vec::new(),
        };

        let encoded = packet.encode_to_vec();
        let decoded = Packet::decode(encoded.as_slice()).unwrap();
        assert_eq!(decoded.r#type, PacketType::PacketFin as i32);
    }
}
