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

/// gRPC service proto 生成代码
///
/// 模块层级匹配 proto package 层级，确保 tonic 生成的 super:: 交叉引用正确解析。
/// ops.proto 的 pb 包通过 extern_path 引用 llb 块的已有类型。
#[cfg(feature = "grpc")]
pub mod grpc_proto {
    pub mod google {
        pub mod rpc {
            include!(concat!(env!("OUT_DIR"), "/google.rpc.rs"));
        }
    }

    pub mod fsutil {
        pub mod types {
            include!(concat!(env!("OUT_DIR"), "/fsutil.types.rs"));
        }
    }

    pub mod moby {
        pub mod buildkit {
            pub mod v1 {
                include!(concat!(env!("OUT_DIR"), "/moby.buildkit.v1.rs"));

                pub mod types {
                    include!(concat!(env!("OUT_DIR"), "/moby.buildkit.v1.types.rs"));
                }

                pub mod sourcepolicy {
                    include!(concat!(env!("OUT_DIR"), "/moby.buildkit.v1.sourcepolicy.rs"));
                }

                pub mod apicaps {
                    include!(concat!(env!("OUT_DIR"), "/moby.buildkit.v1.apicaps.rs"));
                }

                pub mod frontend {
                    include!(concat!(env!("OUT_DIR"), "/moby.buildkit.v1.frontend.rs"));
                }
            }

            pub mod secrets {
                pub mod v1 {
                    include!(concat!(env!("OUT_DIR"), "/moby.buildkit.secrets.v1.rs"));
                }
            }
        }

        pub mod filesync {
            pub mod v1 {
                include!(concat!(env!("OUT_DIR"), "/moby.filesync.v1.rs"));
            }
        }
    }
}

// 便捷别名：简化 gRPC 代码中的类型引用
#[cfg(feature = "grpc")]
pub use grpc_proto::moby::buildkit::v1 as control;
#[cfg(feature = "grpc")]
pub use grpc_proto::moby::buildkit::secrets::v1 as secrets;
#[cfg(feature = "grpc")]
pub use grpc_proto::moby::filesync::v1 as filesync;
#[cfg(feature = "grpc")]
pub use grpc_proto::moby::buildkit::v1::frontend as gateway;
