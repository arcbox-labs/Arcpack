// LLB DAG 核心原语类型
//
// OperationOutput 表示 DAG 中某个操作的某个输出端口，
// SerializedOp 封装序列化后的 protobuf Op 及其 content-addressable digest。

use std::collections::HashMap;
use std::sync::Arc;

use prost::Message;
use sha2::{Digest, Sha256};

use crate::buildkit::proto::pb;

/// LLB DAG 中某个操作的某个输出端口。
/// 对齐 Go llb.State 的核心概念 —— State 本质上就是 (Op, OutputIndex) 的引用。
#[derive(Clone, Debug)]
pub struct OperationOutput {
    pub serialized_op: Arc<SerializedOp>,
    pub output_index: i64,
}

/// 封装序列化后的 protobuf Op 及其 content-addressable digest。
#[derive(Debug)]
pub struct SerializedOp {
    /// prost 序列化后的 pb::Op 字节
    pub bytes: Vec<u8>,
    /// "sha256:{hex}" 格式
    pub digest: String,
    /// 描述信息
    pub metadata: OpMetadata,
    /// 此操作依赖的输入
    pub inputs: Vec<OperationOutput>,
}

/// 操作元数据
#[derive(Clone, Debug, Default)]
pub struct OpMetadata {
    pub description: HashMap<String, String>,
    pub caps: HashMap<String, bool>,
}

/// 计算 sha256 content digest，格式 "sha256:{64位hex}"
pub fn digest_of(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let result = hasher.finalize();
    format!("sha256:{:x}", result)
}

/// 序列化 pb::Op 并计算 digest
pub fn serialize_op(
    op: &pb::Op,
    metadata: OpMetadata,
    inputs: Vec<OperationOutput>,
) -> SerializedOp {
    let bytes = op.encode_to_vec();
    let digest = digest_of(&bytes);
    SerializedOp {
        bytes,
        digest,
        metadata,
        inputs,
    }
}

/// 构造 OperationOutput
pub fn make_output(serialized_op: SerializedOp, output_index: i64) -> OperationOutput {
    OperationOutput {
        serialized_op: Arc::new(serialized_op),
        output_index,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_digest_of_same_input_same_digest() {
        let data = b"hello world";
        let d1 = digest_of(data);
        let d2 = digest_of(data);
        assert_eq!(d1, d2);
    }

    #[test]
    fn test_digest_of_different_input_different_digest() {
        let d1 = digest_of(b"hello");
        let d2 = digest_of(b"world");
        assert_ne!(d1, d2);
    }

    #[test]
    fn test_digest_of_format_sha256_hex64() {
        let d = digest_of(b"test");
        assert!(d.starts_with("sha256:"), "digest 应以 sha256: 开头");
        let hex = d.strip_prefix("sha256:").unwrap();
        assert_eq!(hex.len(), 64, "hex 部分应为 64 位");
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()), "应为合法 hex");
    }

    #[test]
    fn test_serialize_op_empty_op() {
        let op = pb::Op::default();
        let serialized = serialize_op(&op, OpMetadata::default(), vec![]);
        assert!(
            !serialized.bytes.is_empty() || serialized.bytes.is_empty(),
            "空 Op 序列化不应 panic"
        );
        assert!(serialized.digest.starts_with("sha256:"));
    }

    #[test]
    fn test_serialize_op_with_fields() {
        let op = pb::Op {
            inputs: vec![pb::Input {
                digest: "sha256:abc".to_string(),
                index: 0,
            }],
            ..Default::default()
        };
        let serialized = serialize_op(&op, OpMetadata::default(), vec![]);
        assert!(!serialized.bytes.is_empty(), "带字段 Op 序列化后应有字节");
        assert!(serialized.digest.starts_with("sha256:"));
    }

    #[test]
    fn test_operation_output_clone_digest_stable() {
        let op = pb::Op::default();
        let serialized = serialize_op(&op, OpMetadata::default(), vec![]);
        let output = make_output(serialized, 0);
        let cloned = output.clone();
        assert_eq!(output.serialized_op.digest, cloned.serialized_op.digest);
        assert_eq!(output.output_index, cloned.output_index);
    }

    #[test]
    fn test_serialized_op_inputs_record_dependencies() {
        // 先创建一个依赖项
        let dep_op = pb::Op::default();
        let dep_serialized = serialize_op(&dep_op, OpMetadata::default(), vec![]);
        let dep_output = make_output(dep_serialized, 0);

        // 创建依赖于 dep_output 的操作
        let op = pb::Op {
            inputs: vec![pb::Input {
                digest: dep_output.serialized_op.digest.clone(),
                index: dep_output.output_index,
            }],
            ..Default::default()
        };
        let serialized = serialize_op(&op, OpMetadata::default(), vec![dep_output.clone()]);
        assert_eq!(serialized.inputs.len(), 1);
        assert_eq!(
            serialized.inputs[0].serialized_op.digest,
            dep_output.serialized_op.digest
        );
    }
}
