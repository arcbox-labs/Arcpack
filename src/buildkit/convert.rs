use super::build_llb::cache_store::BuildKitCacheStore;
use super::build_llb::BuildGraph;
use super::image::{build_image_config, ImageConfig};
use super::platform::Platform;
use crate::plan::BuildPlan;

/// 转换选项
///
/// 对齐 railpack `ConvertPlanOptions`
#[derive(Debug)]
pub struct ConvertPlanOptions {
    pub secrets_hash: Option<String>,
    pub platform: Platform,
    pub cache_key: String,
}

/// 转换结果
#[derive(Debug)]
pub struct ConvertResult {
    pub dockerfile: String,
    pub image_config: ImageConfig,
}

/// 将 BuildPlan 转换为 Dockerfile + ImageConfig
///
/// 对齐 railpack `ConvertPlanToLLB()`（Phase A 版本）
pub fn convert_plan_to_dockerfile(
    plan: &BuildPlan,
    opts: &ConvertPlanOptions,
) -> crate::Result<ConvertResult> {
    let cache_store = BuildKitCacheStore::new(&opts.cache_key);

    let mut build_graph = BuildGraph::new(
        plan.clone(),
        cache_store,
        opts.secrets_hash.clone(),
        opts.platform.clone(),
    )?;

    let output = build_graph.to_dockerfile()?;

    let image_config = build_image_config(&output.output_env, &plan.deploy, &opts.platform);

    Ok(ConvertResult {
        dockerfile: output.dockerfile,
        image_config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{
        BuildPlan, Command, Deploy, Filter, Layer, Step, ARCPACK_BUILDER_IMAGE,
        ARCPACK_RUNTIME_IMAGE,
    };
    use std::collections::HashMap;

    /// 辅助函数：创建简单 plan（packages -> install -> build -> deploy）
    fn simple_plan() -> BuildPlan {
        let mut plan = BuildPlan::new();

        // packages 步骤
        let mut packages = Step::new("packages");
        packages
            .inputs
            .push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
        plan.add_step(packages);

        // install 步骤
        let mut install = Step::new("install");
        install
            .inputs
            .push(Layer::new_step_layer("packages", None));
        install.commands.push(Command::new_exec("npm install"));
        plan.add_step(install);

        // build 步骤
        let mut build = Step::new("build");
        build.inputs.push(Layer::new_step_layer("install", None));
        build.inputs.push(Layer::new_local_layer());
        build.commands.push(Command::new_exec("npm run build"));
        plan.add_step(build);

        // deploy 配置
        plan.deploy = Deploy {
            base: Some(Layer::new_image_layer(ARCPACK_RUNTIME_IMAGE, None)),
            inputs: vec![Layer::new_step_layer(
                "build",
                Some(Filter::include_only(vec![".".to_string()])),
            )],
            start_cmd: Some("node server.js".to_string()),
            variables: HashMap::new(),
            paths: vec![],
        };

        plan
    }

    /// 辅助函数：创建默认转换选项
    fn default_opts() -> ConvertPlanOptions {
        ConvertPlanOptions {
            secrets_hash: None,
            platform: Platform {
                os: "linux".to_string(),
                arch: "amd64".to_string(),
                variant: None,
            },
            cache_key: "test".to_string(),
        }
    }

    #[test]
    fn test_convert_plan_to_dockerfile_basic() {
        let plan = simple_plan();
        let opts = default_opts();

        let result = convert_plan_to_dockerfile(&plan, &opts).unwrap();

        // Dockerfile 不为空
        assert!(
            !result.dockerfile.is_empty(),
            "生成的 Dockerfile 不应为空"
        );
        // 包含 FROM 指令
        assert!(
            result.dockerfile.contains("FROM"),
            "Dockerfile 应包含 FROM 指令"
        );
        // 包含构建命令
        assert!(
            result.dockerfile.contains("RUN npm install"),
            "Dockerfile 应包含 npm install"
        );
        assert!(
            result.dockerfile.contains("RUN npm run build"),
            "Dockerfile 应包含 npm run build"
        );
        // ImageConfig 有效
        assert_eq!(result.image_config.working_dir, "/app");
    }

    #[test]
    fn test_convert_result_has_syntax_header() {
        let plan = simple_plan();
        let opts = default_opts();

        let result = convert_plan_to_dockerfile(&plan, &opts).unwrap();
        assert!(
            result.dockerfile.starts_with("# syntax=docker/dockerfile:1"),
            "Dockerfile 应以 syntax header 开头"
        );
    }

    #[test]
    fn test_convert_result_image_config_has_cmd() {
        let plan = simple_plan();
        let opts = default_opts();

        let result = convert_plan_to_dockerfile(&plan, &opts).unwrap();
        assert_eq!(
            result.image_config.cmd,
            vec!["node server.js"],
            "ImageConfig.cmd 应包含启动命令"
        );
        assert_eq!(
            result.image_config.entrypoint,
            vec!["/bin/bash", "-c"],
            "ImageConfig.entrypoint 应为 [/bin/bash, -c]"
        );
    }
}
