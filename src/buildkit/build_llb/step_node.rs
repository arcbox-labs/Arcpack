use crate::graph::Node;
use crate::plan::Step;
use super::build_env::BuildEnvironment;

#[cfg(feature = "llb")]
use crate::buildkit::llb::OperationOutput;

/// 构建图节点 —— Step + 处理状态 + 双环境
///
/// 对齐 railpack `step_node.go`
#[derive(Debug, Clone)]
pub struct StepNode {
    /// 原始构建步骤
    pub step: Step,
    /// Dockerfile 阶段片段（Phase A）
    pub dockerfile_stage: String,
    /// 是否已处理完成
    pub processed: bool,
    /// 处理中标记（递归环检测）
    pub in_progress: bool,
    /// 从父节点继承的环境
    pub input_env: BuildEnvironment,
    /// 本步骤处理后的累积环境
    pub output_env: BuildEnvironment,
    /// LLB 状态（Phase B）
    #[cfg(feature = "llb")]
    pub llb_state: Option<OperationOutput>,
}

impl StepNode {
    /// 从 Step 构造（初始化空环境和状态）
    pub fn new(step: Step) -> Self {
        Self {
            step,
            dockerfile_stage: String::new(),
            processed: false,
            in_progress: false,
            input_env: BuildEnvironment::new(),
            output_env: BuildEnvironment::new(),
            #[cfg(feature = "llb")]
            llb_state: None,
        }
    }

    /// 设置 LLB 状态
    #[cfg(feature = "llb")]
    pub fn set_llb_state(&mut self, state: OperationOutput) {
        self.llb_state = Some(state);
    }

    /// 获取 LLB 状态引用
    #[cfg(feature = "llb")]
    pub fn get_llb_state(&self) -> Option<&OperationOutput> {
        self.llb_state.as_ref()
    }
}

impl Node for StepNode {
    fn name(&self) -> &str {
        self.step.name.as_deref().unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_initializes_empty_state() {
        let step = Step::new("install");
        let node = StepNode::new(step);
        assert!(!node.processed, "processed should be false");
        assert!(!node.in_progress, "in_progress should be false");
        assert!(node.dockerfile_stage.is_empty(), "dockerfile_stage should be empty");
    }

    #[test]
    fn test_node_trait_returns_step_name() {
        let step = Step::new("build");
        let node = StepNode::new(step);
        assert_eq!(node.name(), "build");
    }

    #[test]
    fn test_node_trait_empty_name() {
        // Step with no name (using Default)
        let step = Step::default();
        let node = StepNode::new(step);
        assert_eq!(node.name(), "", "step with no name should return empty string");
    }

    #[cfg(feature = "llb")]
    #[test]
    fn test_llb_state_initial_none() {
        let step = Step::new("install");
        let node = StepNode::new(step);
        assert!(node.llb_state.is_none(), "新建 StepNode 的 llb_state 应为 None");
    }

    #[cfg(feature = "llb")]
    #[test]
    fn test_set_get_llb_state_roundtrip() {
        use crate::buildkit::llb::source::image;
        let step = Step::new("install");
        let mut node = StepNode::new(step);
        let img = image("node:20");
        let digest = img.serialized_op.digest.clone();
        node.set_llb_state(img);
        let state = node.get_llb_state().expect("set 后应为 Some");
        assert_eq!(state.serialized_op.digest, digest);
    }

    #[test]
    fn test_new_initializes_empty_environments() {
        let step = Step::new("setup");
        let node = StepNode::new(step);
        assert!(node.input_env.path_list.is_empty(), "input_env path_list should be empty");
        assert!(node.input_env.env_vars.is_empty(), "input_env env_vars should be empty");
        assert!(node.output_env.path_list.is_empty(), "output_env path_list should be empty");
        assert!(node.output_env.env_vars.is_empty(), "output_env env_vars should be empty");
    }
}
