/// gRPC 集成测试
///
/// 需要 buildkitd 运行环境，标记 #[ignore]。
/// 通过 `cargo test -- --ignored` 运行。

use std::collections::HashMap;
use std::path::PathBuf;

use arcpack::buildkit::grpc_client::{GrpcBuildKitClient, GrpcBuildRequest, build_export_config};
use arcpack::buildkit::grpc::progress::ProgressMode;
use arcpack::buildkit::convert::{convert_plan_to_llb, ConvertPlanOptions};
use arcpack::buildkit::platform::Platform;
use arcpack::plan::{
    BuildPlan, Command, Deploy, Filter, Layer, Step,
    ARCPACK_BUILDER_IMAGE, ARCPACK_RUNTIME_IMAGE,
};

/// 创建简单的 Node.js plan
fn simple_node_plan() -> BuildPlan {
    let mut plan = BuildPlan::new();

    let mut packages = Step::new("packages");
    packages
        .inputs
        .push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
    plan.add_step(packages);

    let mut install = Step::new("install");
    install
        .inputs
        .push(Layer::new_step_layer("packages", None));
    install.commands.push(Command::new_exec("echo 'npm install'"));
    plan.add_step(install);

    plan.deploy = Deploy {
        base: Some(Layer::new_image_layer(ARCPACK_RUNTIME_IMAGE, None)),
        inputs: vec![Layer::new_step_layer(
            "install",
            Some(Filter::include_only(vec![".".to_string()])),
        )],
        start_cmd: Some("echo hello".to_string()),
        variables: HashMap::new(),
        paths: vec![],
    };

    plan
}

/// Smoke test：convert plan → LLB Definition → gRPC Solve
///
/// 需要 buildkitd 运行。
#[tokio::test]
#[ignore]
async fn test_grpc_build_via_solve() {
    let plan = simple_node_plan();
    let opts = ConvertPlanOptions {
        secrets_hash: None,
        platform: Platform {
            os: "linux".to_string(),
            arch: "amd64".to_string(),
            variant: None,
        },
        cache_key: "integration-test".to_string(),
    };

    // 转换为 LLB
    let llb_result = convert_plan_to_llb(&plan, &opts)
        .expect("convert_plan_to_llb 不应失败");

    assert!(
        !llb_result.definition.def.is_empty(),
        "LLB Definition 不应为空"
    );

    // 构建 export 配置
    let output_dir = tempfile::tempdir().expect("创建临时目录失败");
    let export = build_export_config(
        None,
        Some(&output_dir.path().to_path_buf()),
        false,
    )
    .expect("build_export_config 不应失败");

    // 连接 buildkitd
    let addr = std::env::var("BUILDKIT_HOST")
        .unwrap_or_else(|_| "unix:///run/buildkit/buildkitd.sock".to_string());
    let client = GrpcBuildKitClient::new(&addr)
        .await
        .expect("连接 buildkitd 失败");

    let mut local_dirs = HashMap::new();
    local_dirs.insert("context".to_string(), PathBuf::from("."));

    let request = GrpcBuildRequest {
        definition: llb_result.definition,
        image_config: llb_result.image_config,
        export,
        secrets: HashMap::new(),
        local_dirs,
        progress_mode: ProgressMode::Plain,
        cache_imports: vec![],
        cache_exports: vec![],
    };

    let output = client.build(request).await
        .expect("gRPC build 不应失败");

    assert!(
        output.duration.as_millis() > 0,
        "构建耗时应大于 0"
    );
}
