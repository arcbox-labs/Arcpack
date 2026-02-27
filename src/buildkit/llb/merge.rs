// LLB Merge 操作：合并多个文件系统层
//
// 对齐 Go llb.Merge()

use crate::buildkit::proto::pb;

use super::operation::{make_output, serialize_op, OpMetadata, OperationOutput};

/// 合并多个文件系统层
///
/// - 空输入 → 返回错误
/// - 单输入 → 直接传递（不创建 MergeOp）
/// - 多输入 → 构造 MergeOp
///
/// 对齐 Go `llb.Merge(inputs)`
pub fn merge(inputs: Vec<OperationOutput>) -> crate::Result<OperationOutput> {
    if inputs.is_empty() {
        return Err(anyhow::anyhow!("merge: 至少需要一个输入").into());
    }

    // 单输入退化，直接传递
    if inputs.len() == 1 {
        return Ok(inputs.into_iter().next().unwrap());
    }

    // 构造 MergeOp
    let merge_inputs: Vec<pb::MergeInput> = (0..inputs.len() as i64)
        .map(|i| pb::MergeInput { input: i })
        .collect();

    let merge_op = pb::MergeOp {
        inputs: merge_inputs,
    };

    let pb_inputs: Vec<pb::Input> = inputs
        .iter()
        .map(|i| pb::Input {
            digest: i.serialized_op.digest.clone(),
            index: i.output_index,
        })
        .collect();

    let op = pb::Op {
        inputs: pb_inputs,
        op: Some(pb::op::Op::Merge(merge_op)),
        ..Default::default()
    };

    let serialized = serialize_op(&op, OpMetadata::default(), inputs);
    Ok(make_output(serialized, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buildkit::llb::source::{image, scratch};
    use prost::Message;

    #[test]
    fn test_merge_single_input_passthrough() {
        let layer = image("node:20");
        let digest = layer.serialized_op.digest.clone();
        let result = merge(vec![layer]).unwrap();
        // 单输入应直接传递，digest 不变
        assert_eq!(result.serialized_op.digest, digest);
    }

    #[test]
    fn test_merge_multiple_inputs() {
        let layer1 = image("node:20");
        let layer2 = image("alpine:3");
        let layer3 = scratch();

        let result = merge(vec![layer1.clone(), layer2.clone(), layer3.clone()]).unwrap();
        let op = pb::Op::decode(result.serialized_op.bytes.as_slice()).unwrap();

        assert_eq!(op.inputs.len(), 3);
        if let Some(pb::op::Op::Merge(merge_op)) = &op.op {
            assert_eq!(merge_op.inputs.len(), 3);
            assert_eq!(merge_op.inputs[0].input, 0);
            assert_eq!(merge_op.inputs[1].input, 1);
            assert_eq!(merge_op.inputs[2].input, 2);
        } else {
            panic!("应为 MergeOp");
        }
    }

    #[test]
    fn test_merge_empty_returns_error() {
        let result = merge(vec![]);
        assert!(result.is_err(), "空输入应返回错误");
    }
}
