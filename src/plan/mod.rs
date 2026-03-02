pub mod cache;
/// plan 模块 —— 构建计划纯数据结构
///
/// 对齐 railpack `core/plan/` 包。
pub mod command;
pub mod dockerignore;
pub mod filter;
pub mod layer;
pub mod packages;
pub mod spread;
pub mod step;

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

// re-export 核心类型
pub use cache::{Cache, CacheType};
pub use command::Command;
pub use dockerignore::DockerignoreContext;
pub use filter::Filter;
pub use layer::Layer;
pub use packages::PlanPackages;
pub use spread::{spread, spread_strings, Spreadable};
pub use step::Step;

/// arcpack 默认构建镜像
pub const ARCPACK_BUILDER_IMAGE: &str = "ghcr.io/railwayapp/railpack-builder:latest";
/// arcpack 默认运行时镜像
pub const ARCPACK_RUNTIME_IMAGE: &str = "ghcr.io/railwayapp/railpack-runtime:latest";

/// Deploy —— 部署配置
///
/// 对齐 railpack `core/plan/plan.go` 的 Deploy 结构体。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deploy {
    /// 运行时基础层
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<Layer>,

    /// 从构建步骤复制到最终镜像的输入层
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Layer>,

    /// 启动命令
    #[serde(rename = "startCommand", skip_serializing_if = "Option::is_none")]
    pub start_cmd: Option<String>,

    /// 运行时环境变量
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub variables: HashMap<String, String>,

    /// 运行时 PATH 条目
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

/// BuildPlan —— 构建计划
///
/// 对齐 railpack `core/plan/plan.go` 的 BuildPlan 结构体。
/// 所有 Provider 输出的聚合产物。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildPlan {
    /// 构建步骤列表
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,

    /// 缓存定义（缓存键 → 缓存配置）
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub caches: HashMap<String, Cache>,

    /// Secret 引用列表
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,

    /// 部署配置
    #[serde(default)]
    pub deploy: Deploy,
}

impl BuildPlan {
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加构建步骤
    pub fn add_step(&mut self, step: Step) {
        self.steps.push(step);
    }

    /// 规范化构建计划
    ///
    /// 对齐 railpack 的 Normalize()：
    /// 1. 移除各步骤和 deploy 中的空 Layer 输入
    /// 2. 从 deploy 开始反向遍历 DAG，移除未被引用的孤立步骤
    pub fn normalize(&mut self) {
        // 1. 移除各步骤中的空输入
        for step in &mut self.steps {
            step.inputs.retain(|input| !input.is_empty());
        }

        // 移除 deploy 中的空输入
        self.deploy.inputs.retain(|input| !input.is_empty());

        // 2. BFS 收集从 deploy 可达的步骤（O(V+E)）
        let referenced: HashSet<String> = {
            // 建索引：步骤名 → 该步骤引用的所有输入步骤名
            let step_deps: HashMap<&str, Vec<&str>> = self
                .steps
                .iter()
                .filter_map(|s| {
                    let name = s.name.as_deref()?;
                    let deps: Vec<&str> = s
                        .inputs
                        .iter()
                        .filter_map(|input| input.step.as_deref())
                        .collect();
                    Some((name, deps))
                })
                .collect();

            // 种子：deploy 直接引用的步骤
            let mut visited: HashSet<&str> = HashSet::new();
            let mut queue: VecDeque<&str> = VecDeque::new();

            if let Some(ref base) = self.deploy.base {
                if let Some(ref name) = base.step {
                    if visited.insert(name) {
                        queue.push_back(name);
                    }
                }
            }
            for input in &self.deploy.inputs {
                if let Some(ref name) = input.step {
                    if visited.insert(name) {
                        queue.push_back(name);
                    }
                }
            }

            // BFS 传递闭包
            while let Some(name) = queue.pop_front() {
                if let Some(deps) = step_deps.get(name) {
                    for dep in deps {
                        if visited.insert(dep) {
                            queue.push_back(dep);
                        }
                    }
                }
            }

            // 转为 owned，释放对 self.steps 的借用
            visited.into_iter().map(String::from).collect()
        };

        // 仅在有引用关系时才移除孤立步骤
        if !referenced.is_empty() {
            self.steps.retain(|step| {
                step.name
                    .as_ref()
                    .map(|name| referenced.contains(name.as_str()))
                    .unwrap_or(false)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_plan_new_is_empty() {
        let plan = BuildPlan::new();
        assert!(plan.steps.is_empty());
        assert!(plan.caches.is_empty());
        assert!(plan.secrets.is_empty());
    }

    #[test]
    fn test_build_plan_add_step() {
        let mut plan = BuildPlan::new();
        plan.add_step(Step::new("install"));
        plan.add_step(Step::new("build"));
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].name, Some("install".to_string()));
        assert_eq!(plan.steps[1].name, Some("build".to_string()));
    }

    #[test]
    fn test_build_plan_json_roundtrip() {
        let mut plan = BuildPlan::new();

        // 添加缓存
        plan.caches
            .insert("apt".to_string(), Cache::new_locked("/var/cache/apt"));
        plan.caches
            .insert("go-build".to_string(), Cache::new("/root/.cache/go-build"));

        // 添加步骤
        let mut packages = Step::new("packages");
        packages
            .inputs
            .push(Layer::new_image_layer(ARCPACK_BUILDER_IMAGE, None));
        plan.add_step(packages);

        let mut install = Step::new("install");
        install.inputs.push(Layer::new_step_layer("packages", None));
        install.commands.push(Command::new_exec("go mod download"));
        plan.add_step(install);

        let mut build = Step::new("build");
        build.inputs.push(Layer::new_step_layer("install", None));
        build.inputs.push(Layer::new_local_layer());
        build.commands.push(Command::new_exec("go build -o app ."));
        build.caches.push("go-build".to_string());
        plan.add_step(build);

        // 配置 deploy
        plan.deploy = Deploy {
            base: Some(Layer::new_step_layer("packages", None)),
            inputs: vec![Layer::new_step_layer(
                "build",
                Some(Filter::include_only(vec![".".to_string()])),
            )],
            start_cmd: Some("./app".to_string()),
            variables: HashMap::new(),
            paths: Vec::new(),
        };

        let json = serde_json::to_string_pretty(&plan).unwrap();
        let parsed: BuildPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, parsed);
    }

    #[test]
    fn test_deploy_start_cmd_serializes_as_start_command() {
        let deploy = Deploy {
            start_cmd: Some("./app".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&deploy).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("startCommand").is_some());
        assert!(value.get("start_cmd").is_none());
        assert!(value.get("startCmd").is_none());
    }

    #[test]
    fn test_deploy_empty_start_cmd_skipped_in_json() {
        let deploy = Deploy::default();
        let json = serde_json::to_string(&deploy).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("startCommand").is_none());
    }

    #[test]
    fn test_normalize_removes_empty_inputs() {
        let mut plan = BuildPlan::new();

        let mut step = Step::new("build");
        step.inputs.push(Layer::new_step_layer("install", None));
        step.inputs.push(Layer::default()); // 空输入
        plan.add_step(step);

        // deploy 引用 build 步骤
        plan.deploy
            .inputs
            .push(Layer::new_step_layer("build", None));

        plan.normalize();

        assert_eq!(plan.steps[0].inputs.len(), 1);
        assert_eq!(plan.steps[0].inputs[0].step, Some("install".to_string()));
    }

    #[test]
    fn test_normalize_removes_orphan_steps() {
        let mut plan = BuildPlan::new();

        plan.add_step(Step::new("packages"));
        plan.add_step(Step::new("install"));
        plan.add_step(Step::new("orphan")); // 未被引用

        let install = &mut plan.steps[1];
        install.inputs.push(Layer::new_step_layer("packages", None));

        plan.deploy
            .inputs
            .push(Layer::new_step_layer("install", None));

        plan.normalize();

        let step_names: Vec<_> = plan.steps.iter().filter_map(|s| s.name.as_ref()).collect();
        assert!(step_names.contains(&&"packages".to_string()));
        assert!(step_names.contains(&&"install".to_string()));
        assert!(!step_names.contains(&&"orphan".to_string()));
    }

    #[test]
    fn test_build_plan_empty_fields_skipped_in_json() {
        let plan = BuildPlan::new();
        let json = serde_json::to_string(&plan).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("steps").is_none());
        assert!(value.get("caches").is_none());
        assert!(value.get("secrets").is_none());
        // deploy 始终序列化（因为没有 skip_serializing_if）
        assert!(value.get("deploy").is_some());
    }
}
