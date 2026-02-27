fn main() {
    #[cfg(feature = "llb")]
    {
        tonic_build::configure()
            .build_server(false) // B-1 不需要 server 代码
            .build_client(false) // B-1 不需要 client 代码
            .compile_protos(
                &["proto/moby/buildkit/v1/ops.proto"],
                &["proto/"],
            )
            .expect("Failed to compile protos");
    }
}
