/// BuildKit LLB protobuf 生成代码
///
/// 由 tonic-build 从 `proto/moby/buildkit/v1/ops.proto` 生成，
/// 包含 Op、ExecOp、SourceOp、FileOp、MergeOp、Definition 等核心消息类型。
#[cfg(feature = "llb")]
pub mod pb {
    // 用 include! 而非 tonic::include_proto!，
    // 避免 llb feature 依赖 tonic 运行时。
    include!(concat!(env!("OUT_DIR"), "/pb.rs"));
}
