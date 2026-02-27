/// gRPC 帧编解码
///
/// gRPC over HTTP/2 的消息格式：5 字节前缀 + protobuf payload
///
/// ```text
/// [compress_flag: u8] [length: u32 big-endian] [payload: bytes]
/// ```
///
/// 参考：https://github.com/grpc/grpc/blob/master/doc/PROTOCOL-HTTP2.md
use anyhow::{bail, Result};
use bytes::{BufMut, Bytes, BytesMut};

/// 编码 gRPC 帧：0x00 + 4字节长度 + payload
pub fn encode_grpc_frame(payload: &[u8]) -> Bytes {
    let len = payload.len() as u32;
    let mut buf = BytesMut::with_capacity(5 + payload.len());
    buf.put_u8(0); // 无压缩
    buf.put_u32(len);
    buf.put_slice(payload);
    buf.freeze()
}

/// 解码 gRPC 帧：验证前缀，返回 payload 切片
pub fn decode_grpc_frame(data: &[u8]) -> Result<&[u8]> {
    if data.len() < 5 {
        bail!(
            "gRPC frame too short: expected at least 5 bytes, got {}",
            data.len()
        );
    }

    let compress_flag = data[0];
    if compress_flag != 0 {
        bail!("gRPC compressed frames not supported (flag={compress_flag})");
    }

    let length = u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as usize;
    let payload = &data[5..];

    if payload.len() < length {
        bail!(
            "gRPC frame payload incomplete: declared {length} bytes, got {}",
            payload.len()
        );
    }

    Ok(&payload[..length])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_roundtrip() {
        let payload = b"hello gRPC";
        let frame = encode_grpc_frame(payload);
        let decoded = decode_grpc_frame(&frame).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn test_encode_empty_payload() {
        let frame = encode_grpc_frame(b"");
        assert_eq!(frame.len(), 5); // 只有前缀
        assert_eq!(frame[0], 0); // 无压缩
        assert_eq!(&frame[1..5], &[0, 0, 0, 0]); // 长度 0

        let decoded = decode_grpc_frame(&frame).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_encode_large_payload() {
        let payload = vec![0xABu8; 65536];
        let frame = encode_grpc_frame(&payload);
        assert_eq!(frame.len(), 5 + 65536);

        // 验证长度字段
        let length = u32::from_be_bytes([frame[1], frame[2], frame[3], frame[4]]);
        assert_eq!(length, 65536);

        let decoded = decode_grpc_frame(&frame).unwrap();
        assert_eq!(decoded.len(), 65536);
        assert!(decoded.iter().all(|&b| b == 0xAB));
    }

    #[test]
    fn test_decode_too_short() {
        let result = decode_grpc_frame(&[0, 0, 0]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too short"));
    }

    #[test]
    fn test_decode_compressed_frame_rejected() {
        // compress_flag = 1
        let data = [1, 0, 0, 0, 3, b'a', b'b', b'c'];
        let result = decode_grpc_frame(&data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("compressed"));
    }

    #[test]
    fn test_decode_incomplete_payload() {
        // 声明 10 字节，但只有 3 字节 payload
        let data = [0, 0, 0, 0, 10, b'a', b'b', b'c'];
        let result = decode_grpc_frame(&data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("incomplete"));
    }

    #[test]
    fn test_decode_extra_trailing_bytes_ignored() {
        // payload 声明 3 字节，后面多余 2 字节应被忽略
        let data = [0, 0, 0, 0, 3, b'a', b'b', b'c', b'x', b'y'];
        let decoded = decode_grpc_frame(&data).unwrap();
        assert_eq!(decoded, b"abc");
    }
}
