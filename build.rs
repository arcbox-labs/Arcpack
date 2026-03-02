fn main() {
    // LLB proto 类型（pb 包）由 buildkit-client 提供，arcpack 不再需要本地编译。
    // gRPC service proto（control/filesync/secrets）同样由 buildkit-client 提供。
    //
    // 唯一需要本地编译的是 gateway.proto（LLBBridge service），
    // buildkit-client 目前不编译该 proto。使用 extern_path 将 gateway 引用的
    // 其他包类型指向 buildkit-client 已有的 Rust 类型，避免重复生成。
    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        // ⚠ 以下 extern_path 映射依赖 buildkit-client 的模块结构
        // （proto::pb, proto::fsutil::types, proto::moby::buildkit::v1::*）。
        // 升级 buildkit-client 版本时需验证这些路径仍然有效。
        // 当前锁定版本: rev = "3624b041"
        .extern_path(".pb", "::buildkit_client::proto::pb")
        .extern_path(".fsutil.types", "::buildkit_client::proto::fsutil::types")
        .extern_path(
            ".moby.buildkit.v1.types",
            "::buildkit_client::proto::moby::buildkit::v1::types",
        )
        .extern_path(
            ".moby.buildkit.v1.sourcepolicy",
            "::buildkit_client::proto::moby::buildkit::v1::sourcepolicy",
        )
        .extern_path(".google.rpc", "::buildkit_client::proto::google::rpc")
        .compile_protos(
            &[
                "proto/moby/buildkit/v1/gateway.proto",
                // caps.proto 和 worker.proto 不做 extern_path，本地生成
                // （buildkit-client 不导出这些模块）
            ],
            &["proto/"],
        )
        .expect("Failed to compile gateway.proto");
}
