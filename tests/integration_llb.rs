/// LLB 集成测试
///
/// 需要 buildkitd + buildctl 运行环境，标记 #[ignore]。
/// 通过 `cargo test --features llb -- --ignored` 运行。

#[cfg(feature = "llb")]
mod llb_integration {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use arcpack::buildkit::client::{BuildKitClient, LlbBuildRequest};
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

    /// Smoke test：convert plan → LLB Definition → build via buildctl stdin
    ///
    /// 需要 buildkitd 和 buildctl 运行。
    #[tokio::test]
    #[ignore]
    async fn test_llb_build_via_buildctl_stdin() {
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

        // 构建请求
        let build_request = LlbBuildRequest {
            definition: llb_result.definition,
            context_dir: PathBuf::from("."),
            image_name: None,
            output_dir: None,
            push: false,
            progress_mode: "plain".to_string(),
            secrets: HashMap::new(),
            no_cache: false,
        };

        // 执行构建（需要 buildkitd 运行）
        let addr = std::env::var("BUILDKIT_HOST")
            .unwrap_or_else(|_| "unix:///run/buildkit/buildkitd.sock".to_string());
        let client = BuildKitClient::new(addr);
        let output = client.build_from_llb(&build_request).await
            .expect("build_from_llb 不应失败");

        assert!(
            output.duration.as_millis() > 0,
            "构建耗时应大于 0"
        );
    }
}
