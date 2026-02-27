/// BytesMessage ↔ AsyncRead/AsyncWrite 适配器
///
/// 将 tonic 的 `Streaming<BytesMessage>` + `mpsc::Sender<BytesMessage>` 包装为
/// `tokio::io::AsyncRead + AsyncWrite`，供 h2 server 使用。
///
/// 对齐 Go `grpchijack/dial.go` 的 `streamToConn()` 实现。
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use tokio_stream::Stream;
use tonic::Streaming;

use crate::buildkit::proto::control::BytesMessage;

/// 异步 reserve permit 的 Future 类型
type ReserveFut = Pin<Box<dyn Future<Output = Result<mpsc::OwnedPermit<BytesMessage>, mpsc::error::SendError<()>>> + Send>>;

/// gRPC bidi stream 到 AsyncRead/AsyncWrite 的适配器
///
/// - Read 端：从 `Streaming<BytesMessage>` 接收数据，余量暂存于 `read_buf`
/// - Write 端：将数据封装为 `BytesMessage`，通过 `mpsc::Sender` 发送
/// - Shutdown：drop sender 关闭写端
pub struct GrpcStreamIo {
    /// buildkitd → client 的入站消息流
    reader: Streaming<BytesMessage>,
    /// client → buildkitd 的出站发送端
    writer: Option<mpsc::Sender<BytesMessage>>,
    /// 上次 Read 未消费完的余量
    read_buf: BytesMut,
    /// 当 channel 满时，暂存 reserve future
    pending_reserve: Option<ReserveFut>,
}

impl GrpcStreamIo {
    pub fn new(reader: Streaming<BytesMessage>, writer: mpsc::Sender<BytesMessage>) -> Self {
        Self {
            reader,
            writer: Some(writer),
            read_buf: BytesMut::new(),
            pending_reserve: None,
        }
    }
}

impl AsyncRead for GrpcStreamIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();

        // 先消费上次余量
        if !this.read_buf.is_empty() {
            let n = std::cmp::min(buf.remaining(), this.read_buf.len());
            buf.put_slice(&this.read_buf.split_to(n));
            return Poll::Ready(Ok(()));
        }

        // 从 stream 读取下一条消息（跳过空消息避免误判 EOF）
        loop {
            let stream = Pin::new(&mut this.reader);
            match stream.poll_next(cx) {
                Poll::Ready(Some(Ok(msg))) => {
                    let data = msg.data;
                    if data.is_empty() {
                        continue; // 空消息不等于 EOF，继续 poll
                    }
                    let n = std::cmp::min(buf.remaining(), data.len());
                    buf.put_slice(&data[..n]);
                    if n < data.len() {
                        this.read_buf.extend_from_slice(&data[n..]);
                    }
                    return Poll::Ready(Ok(()));
                }
                Poll::Ready(Some(Err(status))) => {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        format!("gRPC stream error: {status}"),
                    )));
                }
                Poll::Ready(None) => return Poll::Ready(Ok(())), // 真实 EOF
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl AsyncWrite for GrpcStreamIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();

        // 如果有 pending reserve，先 poll 它
        if let Some(fut) = &mut this.pending_reserve {
            match fut.as_mut().poll(cx) {
                Poll::Ready(Ok(permit)) => {
                    this.pending_reserve = None;
                    let msg = BytesMessage {
                        data: buf.to_vec(),
                    };
                    permit.send(msg);
                    return Poll::Ready(Ok(buf.len()));
                }
                Poll::Ready(Err(_)) => {
                    this.pending_reserve = None;
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "gRPC stream sender closed",
                    )));
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        let sender = match &this.writer {
            Some(s) => s.clone(),
            None => {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "writer already closed",
                )));
            }
        };

        // 尝试非阻塞发送
        match sender.try_send(BytesMessage {
            data: buf.to_vec(),
        }) {
            Ok(()) => Poll::Ready(Ok(buf.len())),
            Err(mpsc::error::TrySendError::Full(_)) => {
                // channel 满，创建 reserve future 等待容量
                this.pending_reserve = Some(Box::pin(sender.reserve_owned()));
                // 立即 poll 一次注册 waker
                if let Some(fut) = &mut this.pending_reserve {
                    match fut.as_mut().poll(cx) {
                        Poll::Ready(Ok(permit)) => {
                            this.pending_reserve = None;
                            let msg = BytesMessage {
                                data: buf.to_vec(),
                            };
                            permit.send(msg);
                            Poll::Ready(Ok(buf.len()))
                        }
                        Poll::Ready(Err(_)) => {
                            this.pending_reserve = None;
                            Poll::Ready(Err(std::io::Error::new(
                                std::io::ErrorKind::BrokenPipe,
                                "gRPC stream sender closed",
                            )))
                        }
                        Poll::Pending => Poll::Pending,
                    }
                } else {
                    Poll::Pending
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "gRPC stream sender closed",
                )))
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        this.writer.take();
        this.pending_reserve.take();
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 辅助函数：构建测试用 Streaming<BytesMessage>
    ///
    /// 由于 tonic::Streaming 没有公开构造函数，
    /// stream_adapter 的集成测试需要真实的 gRPC 连接。
    /// 单元测试验证辅助逻辑（BytesMut 缓存行为等）。

    #[test]
    fn test_read_buf_caching() {
        // 验证 BytesMut split_to 的缓存行为
        let mut buf = BytesMut::from(&b"0123456789"[..]);
        let first = buf.split_to(4);
        assert_eq!(&first[..], b"0123");
        assert_eq!(&buf[..], b"456789");
    }

    #[test]
    fn test_read_buf_empty_after_drain() {
        let mut buf = BytesMut::from(&b"abc"[..]);
        let _ = buf.split_to(3);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_bytes_message_construction() {
        let msg = BytesMessage {
            data: b"test data".to_vec(),
        };
        assert_eq!(msg.data, b"test data");
    }
}
