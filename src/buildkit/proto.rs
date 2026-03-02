/// BuildKit protobuf 类型
///
/// LLB 原语 (pb) 和 gRPC service 类型 (control/secrets/filesync) 来自 buildkit-client crate。
/// Gateway 类型 (frontend/LLBBridge) 仍由 arcpack 本地编译（buildkit-client 尚未提供）。
// LLB 原语：Definition, Op, ExecOp, SourceOp, FileOp, MergeOp 等
pub use buildkit_client::proto::pb;

// gRPC control service：ControlClient, SolveRequest, StatusResponse, BytesMessage 等
pub use buildkit_client::proto::moby::buildkit::v1 as control;

// Gateway / LLBBridge service（本地编译，buildkit-client 不包含）
pub mod gateway {
    include!(concat!(env!("OUT_DIR"), "/moby.buildkit.v1.frontend.rs"));
}

// gateway.proto 依赖的 apicaps 类型（buildkit-client 不导出，本地生成）
pub mod apicaps {
    include!(concat!(env!("OUT_DIR"), "/moby.buildkit.v1.apicaps.rs"));
}
