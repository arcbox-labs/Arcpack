// LLB Terminal：将 DAG 序列化为 pb::Definition
//
// marshal() 执行 BFS 遍历 DAG，收集所有可达操作，
// 按拓扑序生成 Definition。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use prost::Message;

use crate::buildkit::proto::pb;

use super::operation::{OperationOutput, SerializedOp};

/// 将 LLB DAG 序列化为 BuildKit 可接受的 Definition。
///
/// 从 output 开始 BFS 遍历所有依赖操作，按拓扑序序列化。
/// 对齐 Go `State.Marshal(ctx, constraints)` → `Definition`
pub fn marshal(output: &OperationOutput) -> crate::Result<pb::Definition> {
    // 1. BFS 收集所有唯一 Op
    let ops = collect_ops(output);

    // 2. 拓扑排序（叶节点在前，根节点在后）
    let sorted = topological_sort_ops(&ops);

    // 3. 构造 terminal Op
    let terminal_bytes = build_terminal_op(output);

    // 4. 组装 Definition
    let mut def = Vec::new();
    let mut metadata = HashMap::new();

    for op in &sorted {
        def.push(op.bytes.clone());

        // 构造 pb::OpMetadata
        let op_meta = pb::OpMetadata {
            description: op.metadata.description.clone(),
            caps: op.metadata.caps.clone(),
            ..Default::default()
        };
        metadata.insert(op.digest.clone(), op_meta);
    }

    // 添加 terminal Op
    def.push(terminal_bytes);

    Ok(pb::Definition {
        def,
        metadata,
        source: None,
    })
}

/// BFS 收集所有可达操作，按 digest 去重
fn collect_ops(output: &OperationOutput) -> Vec<Arc<SerializedOp>> {
    let mut visited = HashSet::new();
    let mut result = Vec::new();
    let mut queue = VecDeque::new();

    queue.push_back(output.serialized_op.clone());

    while let Some(op) = queue.pop_front() {
        if !visited.insert(op.digest.clone()) {
            continue;
        }
        // 先入队依赖项
        for input in &op.inputs {
            queue.push_back(input.serialized_op.clone());
        }
        result.push(op);
    }

    result
}

/// 拓扑排序：叶节点在前，根节点在后
///
/// 使用 Kahn 算法：计算入度，从入度为 0 的节点开始。
/// 这里的"入度"指被多少个其他操作依赖。
fn topological_sort_ops(ops: &[Arc<SerializedOp>]) -> Vec<Arc<SerializedOp>> {
    let op_map: HashMap<&str, &Arc<SerializedOp>> =
        ops.iter().map(|op| (op.digest.as_str(), op)).collect();

    // 计算每个 Op 被多少个其他 Op 引用（出度，从依赖角度看是"被需要的次数"）
    // 在这个 DAG 中，"叶节点"是没有 inputs 的操作（如 Image/Scratch）
    // 我们需要叶节点在前，所以用反向拓扑排序

    // 构建邻接表：op -> 它依赖的 op digest 列表
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

    for op in ops {
        in_degree.entry(op.digest.as_str()).or_insert(0);
        for input in &op.inputs {
            let dep_digest = input.serialized_op.digest.as_str();
            if op_map.contains_key(dep_digest) {
                *in_degree.entry(op.digest.as_str()).or_insert(0) += 1;
                dependents
                    .entry(dep_digest)
                    .or_default()
                    .push(op.digest.as_str());
            }
        }
    }

    // Kahn 算法：从入度为 0 的节点开始（叶节点）
    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&digest, _)| digest)
        .collect();

    let mut sorted = Vec::new();
    while let Some(digest) = queue.pop_front() {
        if let Some(op) = op_map.get(digest) {
            sorted.push((*op).clone());
        }
        if let Some(deps) = dependents.get(digest) {
            for &dep in deps {
                if let Some(deg) = in_degree.get_mut(dep) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(dep);
                    }
                }
            }
        }
    }

    sorted
}

/// 构造 terminal Op 的序列化字节
///
/// Terminal Op 是空操作，仅引用最终输出。
fn build_terminal_op(output: &OperationOutput) -> Vec<u8> {
    let terminal = pb::Op {
        inputs: vec![pb::Input {
            digest: output.serialized_op.digest.clone(),
            index: output.output_index,
        }],
        op: None,
        ..Default::default()
    };
    terminal.encode_to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buildkit::llb::exec::ExecBuilder;
    use crate::buildkit::llb::file::copy;
    use crate::buildkit::llb::source::{image, scratch};

    #[test]
    fn test_marshal_scratch() {
        let s = scratch();
        let def = marshal(&s).unwrap();
        // 1 个 SourceOp + 1 个 terminal
        assert_eq!(def.def.len(), 2);
    }

    #[test]
    fn test_marshal_linear_chain_topological_order() {
        // image → exec → copy
        let img = image("node:20");
        let exec_out = ExecBuilder::new(img.clone(), vec!["npm".into(), "install".into()]).root();
        let dest = scratch();
        let copied = copy(exec_out.clone(), "/app", dest.clone(), "/app");

        let def = marshal(&copied).unwrap();
        // 3 个 Op（image, scratch, exec, copy）+ 1 个 terminal = 5
        // image, scratch 是叶节点
        assert_eq!(def.def.len(), 5, "应有 4 个 Op + 1 个 terminal");

        // 验证拓扑序：最后一个（除 terminal 外）应是 copy（根节点）
        // 前面应是叶节点
        let last_op_bytes = &def.def[def.def.len() - 2]; // terminal 前一个
        let last_op = pb::Op::decode(last_op_bytes.as_slice()).unwrap();
        assert!(
            matches!(last_op.op, Some(pb::op::Op::File(_))),
            "拓扑序最后一个 Op 应为 FileOp（copy）"
        );
    }

    #[test]
    fn test_marshal_dag_dedup_shared_image() {
        // 两个 Exec 共享同一 Image
        let img = image("node:20");
        let exec1 = ExecBuilder::new(img.clone(), vec!["step1".into()]).root();
        let exec2 = ExecBuilder::new(img.clone(), vec!["step2".into()]).root();

        // 合并到一个 copy
        let copied = copy(exec1, "/a", exec2, "/b");

        let def = marshal(&copied).unwrap();

        // 验证 Image 只出现一次
        let mut source_count = 0;
        for bytes in &def.def {
            if let Ok(op) = pb::Op::decode(bytes.as_slice()) {
                if matches!(op.op, Some(pb::op::Op::Source(_))) {
                    source_count += 1;
                }
            }
        }
        assert_eq!(source_count, 1, "共享的 Image 应只出现一次");
    }

    #[test]
    fn test_marshal_diamond_dependency() {
        // 菱形依赖：A → B, A → C, B → D(copy), C → D 的输入
        let a = image("node:20");
        let b = ExecBuilder::new(a.clone(), vec!["step-b".into()]).root();
        let c = ExecBuilder::new(a.clone(), vec!["step-c".into()]).root();
        let d = copy(b, "/from-b", c, "/to-c");

        let def = marshal(&d).unwrap();

        // 4 个唯一 Op + 1 个 terminal = 5
        // A (image), B (exec), C (exec), D (copy)
        assert_eq!(def.def.len(), 5, "菱形依赖应有 4 个唯一 Op + 1 个 terminal");
    }

    #[test]
    fn test_marshal_metadata_contains_descriptions() {
        let img = image("node:20");
        let exec_out = ExecBuilder::new(img, vec!["test".into()])
            .description("Run tests")
            .root();

        let def = marshal(&exec_out).unwrap();

        // metadata 应包含带描述的 Op
        let has_description = def.metadata.values().any(|m| !m.description.is_empty());
        assert!(has_description, "metadata 应包含描述信息");
    }

    #[test]
    fn test_marshal_terminal_inputs_correct() {
        let img = image("node:20");
        let def = marshal(&img).unwrap();

        // terminal 是最后一个
        let terminal_bytes = def.def.last().unwrap();
        let terminal = pb::Op::decode(terminal_bytes.as_slice()).unwrap();
        assert!(terminal.op.is_none(), "terminal Op 应无操作");
        assert_eq!(terminal.inputs.len(), 1);
        assert_eq!(terminal.inputs[0].digest, img.serialized_op.digest);
        assert_eq!(terminal.inputs[0].index, img.output_index);
    }

    #[test]
    fn test_marshal_definition_roundtrip() {
        let img = image("node:20");
        let exec_out = ExecBuilder::new(img, vec!["echo".into(), "hello".into()]).root();

        let def = marshal(&exec_out).unwrap();

        // 验证 Definition 可序列化和反序列化
        let encoded = def.encode_to_vec();
        assert!(!encoded.is_empty());
        let decoded = pb::Definition::decode(encoded.as_slice()).unwrap();
        assert_eq!(decoded.def.len(), def.def.len());
        assert_eq!(decoded.metadata.len(), def.metadata.len());
    }
}
