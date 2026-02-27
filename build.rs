fn main() {
    // B-1/B-2：LLB 原语只需 ops.proto（不生成 gRPC client/server 代码）
    #[cfg(feature = "llb")]
    {
        tonic_build::configure()
            .build_server(false)
            .build_client(false)
            .compile_protos(
                &["proto/moby/buildkit/v1/ops.proto"],
                &["proto/"],
            )
            .expect("Failed to compile ops.proto");
    }

    // B-3/B-4：gRPC 需要额外的 service proto（control/filesync/secrets）
    // Session 需要 server 端代码（响应 buildkitd 回调）
    // extern_path: ops.proto 的 pb 包已在 llb 块编译，此处引用现有类型避免重复生成
    #[cfg(feature = "grpc")]
    {
        tonic_build::configure()
            .build_server(true)
            .build_client(true)
            .extern_path(".pb", "crate::buildkit::proto::pb")
            .compile_protos(
                &[
                    "proto/moby/buildkit/v1/control.proto",
                    "proto/moby/buildkit/v1/filesync.proto",
                    "proto/moby/buildkit/v1/secrets.proto",
                    "proto/moby/buildkit/v1/auth.proto",
                ],
                &["proto/"],
            )
            .expect("Failed to compile gRPC protos");
    }
}
