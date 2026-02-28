pub mod build_llb;
pub mod convert;
pub mod daemon;
pub mod grpc;
pub mod grpc_client;
pub mod image;
pub mod llb;
pub mod platform;
pub mod proto;

use std::time::Duration;

/// 构建输出
#[derive(Debug)]
pub struct BuildOutput {
    /// 镜像摘要
    pub image_digest: Option<String>,
    /// 构建耗时
    pub duration: Duration,
}
