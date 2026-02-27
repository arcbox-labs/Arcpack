// LLB File 操作：Copy / MkFile / MkDir
//
// 在容器文件系统中复制文件、创建文件、创建目录。

use crate::buildkit::proto::pb;

use super::operation::{make_output, serialize_op, OpMetadata, OperationOutput};

/// 复制文件
///
/// 对齐 Go `llb.Copy(src, srcPath, dest, destPath)`
pub fn copy(
    src: OperationOutput,
    src_path: &str,
    dest: OperationOutput,
    dest_path: &str,
) -> OperationOutput {
    copy_with_opts(src, src_path, dest, dest_path, CopyOpts::default())
}

/// 带选项的复制
///
/// 对齐 Go `llb.Copy(src, srcPath, dest, destPath, opts...)`
pub fn copy_with_opts(
    src: OperationOutput,
    src_path: &str,
    dest: OperationOutput,
    dest_path: &str,
    opts: CopyOpts,
) -> OperationOutput {
    // inputs: [dest, src]（dest 作为 input 0，src 作为 secondaryInput = input 1）
    let inputs = vec![dest.clone(), src.clone()];

    let file_action = pb::FileAction {
        input: 0,           // dest
        secondary_input: 1, // src
        output: 0,
        action: Some(pb::file_action::Action::Copy(pb::FileActionCopy {
            src: src_path.to_string(),
            dest: dest_path.to_string(),
            follow_symlink: opts.follow_symlinks,
            create_dest_path: opts.create_dest_path,
            allow_wildcard: opts.allow_wildcard,
            allow_empty_wildcard: opts.allow_empty_wildcard,
            ..Default::default()
        })),
    };

    build_file_op(vec![file_action], inputs)
}

/// 创建文件
///
/// 对齐 Go `llb.Mkfile(path, mode, content)`
pub fn make_file(
    dest: OperationOutput,
    path: &str,
    content: &[u8],
    mode: i32,
) -> OperationOutput {
    let inputs = vec![dest.clone()];

    let file_action = pb::FileAction {
        input: 0,
        secondary_input: -1,
        output: 0,
        action: Some(pb::file_action::Action::Mkfile(pb::FileActionMkFile {
            path: path.to_string(),
            mode,
            data: content.to_vec(),
            ..Default::default()
        })),
    };

    build_file_op(vec![file_action], inputs)
}

/// 创建目录
///
/// 对齐 Go `llb.Mkdir(path, mode)`
pub fn make_dir(dest: OperationOutput, path: &str, mode: i32) -> OperationOutput {
    let inputs = vec![dest.clone()];

    let file_action = pb::FileAction {
        input: 0,
        secondary_input: -1,
        output: 0,
        action: Some(pb::file_action::Action::Mkdir(pb::FileActionMkDir {
            path: path.to_string(),
            mode,
            make_parents: true,
            ..Default::default()
        })),
    };

    build_file_op(vec![file_action], inputs)
}

/// 复制选项
#[derive(Clone, Debug, Default)]
pub struct CopyOpts {
    /// 自动创建目标目录
    pub create_dest_path: bool,
    /// 允许通配符匹配
    pub allow_wildcard: bool,
    /// 通配符无匹配时不报错
    pub allow_empty_wildcard: bool,
    /// 跟随符号链接
    pub follow_symlinks: bool,
}

/// 构造 FileOp 并序列化
fn build_file_op(actions: Vec<pb::FileAction>, inputs: Vec<OperationOutput>) -> OperationOutput {
    let file_op = pb::FileOp { actions };

    let pb_inputs: Vec<pb::Input> = inputs
        .iter()
        .map(|i| pb::Input {
            digest: i.serialized_op.digest.clone(),
            index: i.output_index,
        })
        .collect();

    let op = pb::Op {
        inputs: pb_inputs,
        op: Some(pb::op::Op::File(file_op)),
        ..Default::default()
    };

    let serialized = serialize_op(&op, OpMetadata::default(), inputs);
    make_output(serialized, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buildkit::llb::source::{image, scratch};
    use prost::Message;

    #[test]
    fn test_copy_src_dest_refs_and_paths() {
        let src = image("node:20");
        let dest = scratch();
        let output = copy(src.clone(), "/app/dist", dest.clone(), "/output");

        let op = pb::Op::decode(output.serialized_op.bytes.as_slice()).unwrap();
        // inputs: [dest, src]
        assert_eq!(op.inputs.len(), 2);
        assert_eq!(op.inputs[0].digest, dest.serialized_op.digest);
        assert_eq!(op.inputs[1].digest, src.serialized_op.digest);

        if let Some(pb::op::Op::File(file_op)) = &op.op {
            let action = &file_op.actions[0];
            if let Some(pb::file_action::Action::Copy(copy_action)) = &action.action {
                assert_eq!(copy_action.src, "/app/dist");
                assert_eq!(copy_action.dest, "/output");
            } else {
                panic!("应为 FileActionCopy");
            }
        } else {
            panic!("应为 FileOp");
        }
    }

    #[test]
    fn test_copy_with_opts_passes_options() {
        let src = image("node:20");
        let dest = scratch();
        let opts = CopyOpts {
            create_dest_path: true,
            allow_wildcard: true,
            allow_empty_wildcard: true,
            follow_symlinks: true,
        };
        let output = copy_with_opts(src, "/src", dest, "/dest", opts);

        let op = pb::Op::decode(output.serialized_op.bytes.as_slice()).unwrap();
        if let Some(pb::op::Op::File(file_op)) = &op.op {
            if let Some(pb::file_action::Action::Copy(copy_action)) = &file_op.actions[0].action {
                assert!(copy_action.create_dest_path);
                assert!(copy_action.allow_wildcard);
                assert!(copy_action.allow_empty_wildcard);
                assert!(copy_action.follow_symlink);
            } else {
                panic!("应为 FileActionCopy");
            }
        } else {
            panic!("应为 FileOp");
        }
    }

    #[test]
    fn test_make_file_content_and_mode() {
        let dest = scratch();
        let output = make_file(dest, "/etc/config.txt", b"hello", 0o644);

        let op = pb::Op::decode(output.serialized_op.bytes.as_slice()).unwrap();
        if let Some(pb::op::Op::File(file_op)) = &op.op {
            if let Some(pb::file_action::Action::Mkfile(mkfile)) = &file_op.actions[0].action {
                assert_eq!(mkfile.path, "/etc/config.txt");
                assert_eq!(mkfile.data, b"hello");
                assert_eq!(mkfile.mode, 0o644);
            } else {
                panic!("应为 FileActionMkFile");
            }
        } else {
            panic!("应为 FileOp");
        }
    }

    #[test]
    fn test_make_dir_path_and_mode() {
        let dest = scratch();
        let output = make_dir(dest, "/app/data", 0o755);

        let op = pb::Op::decode(output.serialized_op.bytes.as_slice()).unwrap();
        if let Some(pb::op::Op::File(file_op)) = &op.op {
            if let Some(pb::file_action::Action::Mkdir(mkdir)) = &file_op.actions[0].action {
                assert_eq!(mkdir.path, "/app/data");
                assert_eq!(mkdir.mode, 0o755);
                assert!(mkdir.make_parents);
            } else {
                panic!("应为 FileActionMkDir");
            }
        } else {
            panic!("应为 FileOp");
        }
    }

    #[test]
    fn test_file_op_chain_dependencies() {
        // make_dir → copy → make_file 依赖链
        let base = scratch();
        let dir = make_dir(base, "/app", 0o755);
        let src = image("node:20");
        let copied = copy(src, "/dist", dir.clone(), "/app/dist");
        let final_output = make_file(copied.clone(), "/app/version.txt", b"1.0", 0o644);

        // 验证依赖链
        assert_eq!(final_output.serialized_op.inputs.len(), 1);
        assert_eq!(
            final_output.serialized_op.inputs[0].serialized_op.digest,
            copied.serialized_op.digest
        );
    }
}
