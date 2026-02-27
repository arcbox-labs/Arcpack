// LLB Source 操作：Image / Local / Scratch
//
// LLB DAG 的叶节点，所有构建图从这里开始。

use std::collections::HashMap;

use crate::buildkit::platform::Platform;
use crate::buildkit::proto::pb;

use super::operation::{make_output, serialize_op, OpMetadata, OperationOutput};

/// 拉取基础镜像
///
/// 对齐 Go `llb.Image(ref)`
pub fn image(reference: &str) -> OperationOutput {
    let identifier = normalize_image_ref(reference);
    build_source_op(&identifier, HashMap::new(), None)
}

/// 指定平台的镜像拉取
///
/// 对齐 Go `llb.Image(ref, llb.Platform(...))`
pub fn image_with_platform(reference: &str, platform: &Platform) -> OperationOutput {
    let identifier = normalize_image_ref(reference);
    build_source_op(&identifier, HashMap::new(), Some(platform))
}

/// 挂载本地构建上下文
///
/// 对齐 Go `llb.Local(name)`
pub fn local(name: &str) -> OperationOutput {
    let identifier = format!("local://{}", name);
    build_source_op(&identifier, HashMap::new(), None)
}

/// 带过滤选项的本地挂载
///
/// 对齐 Go `llb.Local(name, opts...)`
pub fn local_with_opts(name: &str, opts: LocalOpts) -> OperationOutput {
    let identifier = format!("local://{}", name);
    let mut attrs = HashMap::new();
    if !opts.include_patterns.is_empty() {
        attrs.insert(
            "local.includepattern".to_string(),
            serde_json::to_string(&opts.include_patterns).unwrap_or_default(),
        );
    }
    if !opts.exclude_patterns.is_empty() {
        attrs.insert(
            "local.excludepatterns".to_string(),
            serde_json::to_string(&opts.exclude_patterns).unwrap_or_default(),
        );
    }
    if !opts.shared_key_hint.is_empty() {
        attrs.insert("local.sharedkeyhint".to_string(), opts.shared_key_hint);
    }
    build_source_op(&identifier, attrs, None)
}

/// 空文件系统
///
/// 对齐 Go `llb.Scratch()`
pub fn scratch() -> OperationOutput {
    build_source_op("", HashMap::new(), None)
}

/// 本地挂载选项
#[derive(Clone, Debug, Default)]
pub struct LocalOpts {
    pub include_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
    pub shared_key_hint: String,
}

/// 规范化镜像引用
///
/// - 无 `/` → 补 `docker.io/library/`
/// - 有 `/` 无域名（无 `.` 和 `:`） → 补 `docker.io/`
/// - 已有域名 → 原样
fn normalize_image_ref(reference: &str) -> String {
    let normalized = if !reference.contains('/') {
        // 无 /，如 "node:20" → "docker.io/library/node:20"
        format!("docker.io/library/{}", reference)
    } else {
        let first_part = reference.split('/').next().unwrap_or("");
        if first_part.contains('.') || first_part.contains(':') {
            // 有域名，如 "ghcr.io/user/repo:tag" → 原样
            reference.to_string()
        } else {
            // 无域名但有 /，如 "user/repo:tag" → "docker.io/user/repo:tag"
            format!("docker.io/{}", reference)
        }
    };
    format!("docker-image://{}", normalized)
}

/// 将 buildkit::platform::Platform 转换为 pb::Platform
fn to_pb_platform(p: &Platform) -> pb::Platform {
    pb::Platform {
        os: p.os.clone(),
        architecture: p.arch.clone(),
        variant: p.variant.clone().unwrap_or_default(),
        ..Default::default()
    }
}

/// 构造 SourceOp 并序列化
fn build_source_op(
    identifier: &str,
    attrs: HashMap<String, String>,
    platform: Option<&Platform>,
) -> OperationOutput {
    let source_op = pb::SourceOp {
        identifier: identifier.to_string(),
        attrs,
    };
    let op = pb::Op {
        op: Some(pb::op::Op::Source(source_op)),
        platform: platform.map(to_pb_platform),
        ..Default::default()
    };
    let serialized = serialize_op(&op, OpMetadata::default(), vec![]);
    make_output(serialized, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[test]
    fn test_image_identifier_normalized() {
        let output = image("node:20");
        let op = pb::Op::decode(output.serialized_op.bytes.as_slice()).unwrap();
        if let Some(pb::op::Op::Source(src)) = &op.op {
            assert_eq!(src.identifier, "docker-image://docker.io/library/node:20");
        } else {
            panic!("应为 SourceOp");
        }
    }

    #[test]
    fn test_image_digest_stable() {
        let d1 = image("node:20").serialized_op.digest.clone();
        let d2 = image("node:20").serialized_op.digest.clone();
        assert_eq!(d1, d2, "相同输入应产生相同 digest");
    }

    #[test]
    fn test_image_with_platform_sets_platform() {
        let p = Platform {
            os: "linux".to_string(),
            arch: "arm64".to_string(),
            variant: Some("v8".to_string()),
        };
        let output = image_with_platform("node:20", &p);
        let op = pb::Op::decode(output.serialized_op.bytes.as_slice()).unwrap();
        let platform = op.platform.expect("应设置 platform");
        assert_eq!(platform.os, "linux");
        assert_eq!(platform.architecture, "arm64");
        assert_eq!(platform.variant, "v8");
    }

    #[test]
    fn test_local_identifier() {
        let output = local("context");
        let op = pb::Op::decode(output.serialized_op.bytes.as_slice()).unwrap();
        if let Some(pb::op::Op::Source(src)) = &op.op {
            assert_eq!(src.identifier, "local://context");
        } else {
            panic!("应为 SourceOp");
        }
    }

    #[test]
    fn test_local_with_opts_sets_attrs() {
        let opts = LocalOpts {
            include_patterns: vec!["*.js".to_string(), "*.ts".to_string()],
            exclude_patterns: vec!["node_modules".to_string()],
            shared_key_hint: "mykey".to_string(),
        };
        let output = local_with_opts("context", opts);
        let op = pb::Op::decode(output.serialized_op.bytes.as_slice()).unwrap();
        if let Some(pb::op::Op::Source(src)) = &op.op {
            assert!(src.attrs.contains_key("local.includepattern"));
            assert!(src.attrs.contains_key("local.excludepatterns"));
            assert_eq!(src.attrs.get("local.sharedkeyhint").unwrap(), "mykey");
        } else {
            panic!("应为 SourceOp");
        }
    }

    #[test]
    fn test_scratch_empty_identifier() {
        let output = scratch();
        let op = pb::Op::decode(output.serialized_op.bytes.as_slice()).unwrap();
        if let Some(pb::op::Op::Source(src)) = &op.op {
            assert_eq!(src.identifier, "", "scratch 的 identifier 应为空");
        } else {
            panic!("应为 SourceOp");
        }
    }

    #[test]
    fn test_all_source_output_index_zero() {
        assert_eq!(image("node:20").output_index, 0);
        assert_eq!(local("ctx").output_index, 0);
        assert_eq!(scratch().output_index, 0);
    }

    #[test]
    fn test_different_images_different_digest() {
        let d1 = image("node:20").serialized_op.digest.clone();
        let d2 = image("python:3.12").serialized_op.digest.clone();
        assert_ne!(d1, d2, "不同镜像应产生不同 digest");
    }
}
