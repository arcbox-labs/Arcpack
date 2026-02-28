//! Layer 合并策略 —— 决定多个 Layer 如何转换为 Dockerfile 片段
//!
//! 对齐 railpack `buildkit/build_llb/layers.go`。
//! 核心逻辑：判断多个 Layer 是否可以 merge（高效但不能重叠），
//! 还是必须逐层 copy（安全但可能冗余）。

use std::collections::HashMap;
use std::path::Path;

use crate::buildkit::llb::{self, OperationOutput};
use super::step_node::StepNode;

use crate::plan::{Filter, Layer};

/// 清理阶段名，使其符合 Dockerfile AS 命名规则
///
/// Dockerfile 阶段名不允许包含冒号等特殊符号，仅支持 [a-zA-Z0-9_.-]。
/// 将 `:` 替换为 `-`，对齐 railpack 的行为。
pub fn sanitize_stage_name(name: &str) -> String {
    name.replace(':', "-")
}

/// Layer 转换为 Dockerfile 片段的结果
#[derive(Debug, Clone)]
pub struct LayerResult {
    /// FROM 指令（如 "FROM ubuntu:22.04 AS step_name"）
    pub from_instruction: Option<String>,
    /// COPY 指令列表
    pub copy_instructions: Vec<String>,
}

/// 从 Layer 列表生成 Dockerfile 片段
///
/// 对齐 railpack `GetFullStateFromLayers()`。
/// 根据 layer 间是否有路径重叠，选择 merge 或 copy 策略。
pub fn get_full_state_from_layers(layers: &[Layer], step_name: &str) -> LayerResult {
    if layers.is_empty() {
        return LayerResult {
            from_instruction: None,
            copy_instructions: Vec::new(),
        };
    }

    // 第一个 layer 决定 FROM 基础
    let first = &layers[0];
    let from_instruction = get_from_instruction(first, step_name);

    if layers.len() == 1 {
        // 单 layer：FROM + 可能的 COPY（如果有 include filter）
        let copies = if first.local == Some(true) {
            // local layer: COPY from build context
            copy_layer_paths(None, &first.filter, true)
        } else if first.step.is_some() && !first.filter.include.is_empty() {
            // step layer with filter: COPY --from=step_name（步骤名需要 sanitize）
            let safe_ref = first.step.as_deref().map(sanitize_stage_name);
            copy_layer_paths(safe_ref.as_deref(), &first.filter, false)
        } else {
            Vec::new()
        };

        return LayerResult {
            from_instruction,
            copy_instructions: copies,
        };
    }

    // 多 layer：选择合并或逐层 copy
    if should_merge(layers) {
        get_merge_state(layers, step_name)
    } else {
        get_copy_state(layers, step_name)
    }
}

/// 生成 FROM 指令
///
/// 阶段名（AS 后面的名称）和步骤引用（FROM 后面的引用）都需要 sanitize，
/// 因为 Dockerfile 的阶段名不允许包含冒号。
fn get_from_instruction(layer: &Layer, step_name: &str) -> Option<String> {
    let safe_name = sanitize_stage_name(step_name);
    if let Some(ref image) = layer.image {
        Some(format!("FROM {} AS {}", image, safe_name))
    } else if let Some(ref step) = layer.step {
        // step 引用指向前面的阶段名，也需要 sanitize
        let safe_step = sanitize_stage_name(step);
        Some(format!("FROM {} AS {}", safe_step, safe_name))
    } else if layer.local == Some(true) {
        Some(format!("FROM scratch AS {}", safe_name))
    } else {
        None
    }
}

/// 判断是否应该合并多个 layers
///
/// 对齐 railpack `shouldLLBMerge()`。
/// 返回 false 的条件（任一满足则不合并）：
/// 1. 非首层无 include 路径（完整基础替换）
/// 2. 任何层包含根路径 "/"
/// 3. 任何层是 local 引用
/// 4. 两层之间存在显著重叠
pub fn should_merge(layers: &[Layer]) -> bool {
    if layers.len() <= 1 {
        return true;
    }

    for (i, layer) in layers.iter().enumerate() {
        // 条件 1：非首层无 include 路径
        if i > 0 && layer.filter.include.is_empty() {
            return false;
        }

        // 条件 2：任何层包含根路径 "/"
        for path in &layer.filter.include {
            if path == "/" {
                return false;
            }
        }

        // 条件 3：任何层是 local 引用
        if layer.local == Some(true) {
            return false;
        }

        // 条件 4：检查与后续层的显著重叠
        for other in &layers[i + 1..] {
            if has_significant_overlap(layer, other) {
                return false;
            }
        }
    }

    true
}

/// 检测两个 Layer 的 include 路径是否有显著重叠
///
/// 对齐 railpack `hasSignificantOverlap()`。
/// 先将路径规范化到绝对路径（相对路径加 /app 前缀），
/// 再检查精确匹配或前缀包含，最后排除被 exclude 覆盖的情况。
pub fn has_significant_overlap(l1: &Layer, l2: &Layer) -> bool {
    for p1 in &l1.filter.include {
        let p1_clean = normalize_path(p1);

        for p2 in &l2.filter.include {
            let p2_clean = normalize_path(p2);

            // 精确匹配——始终重叠
            if p1_clean == p2_clean {
                return true;
            }

            let p1_with_slash = format!("{}/", p1_clean);
            let p2_with_slash = format!("{}/", p2_clean);

            let (overlap, rel_path, outer_excludes) =
                if p1_with_slash.starts_with(&p2_with_slash) {
                    // p1 在 p2 内部（如 /app/.nvmrc 在 /app 内）
                    let rel = p1_clean
                        .strip_prefix(&p2_clean)
                        .unwrap_or("")
                        .trim_start_matches('/');
                    (true, rel.to_string(), &l2.filter.exclude)
                } else if p2_with_slash.starts_with(&p1_with_slash) {
                    // p2 在 p1 内部
                    let rel = p2_clean
                        .strip_prefix(&p1_clean)
                        .unwrap_or("")
                        .trim_start_matches('/');
                    (true, rel.to_string(), &l1.filter.exclude)
                } else {
                    (false, String::new(), &l1.filter.exclude)
                };

            if overlap {
                // 检查相对路径是否被 exclude 覆盖
                if !is_path_excluded(&rel_path, outer_excludes) {
                    return true;
                }
            }
        }
    }
    false
}

/// 将路径规范化为绝对形式
///
/// - "." → "/app"
/// - 相对路径 → "/app/{path}"
/// - 绝对路径 → 保持不变
fn normalize_path(p: &str) -> String {
    let cleaned = if p == "." { "/app" } else { p };
    if cleaned.starts_with('/') {
        cleaned.to_string()
    } else {
        format!("/app/{}", cleaned)
    }
}

/// 检查路径是否被 exclude 模式覆盖
///
/// 对齐 railpack `isPathExcluded()`。
/// 匹配方式：路径分量匹配 + 前缀匹配。
pub fn is_path_excluded(rel_path: &str, excludes: &[String]) -> bool {
    if excludes.is_empty() {
        return false;
    }

    // 拆分路径为分量
    let parts: Vec<&str> = rel_path.split('/').collect();

    for exclude in excludes {
        // 路径分量匹配
        if parts.contains(&exclude.as_str()) {
            return true;
        }
        // 完整相对路径前缀匹配或精确匹配
        if rel_path == exclude || rel_path.starts_with(&format!("{}/", exclude)) {
            return true;
        }
    }
    false
}

/// 解析路径：local 取 basename 映射到 /app，绝对保留，相对加 /app
///
/// 对齐 railpack `resolvePaths()`。
pub fn resolve_paths(include: &str, is_local: bool) -> (String, String) {
    if is_local {
        // local 路径：取 basename 拼入 /app
        let basename = Path::new(include)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| include.to_string());
        let dest = format!("/app/{}", basename);
        return (include.to_string(), dest);
    }

    match include {
        "." | "/app" | "/app/" => ("/app".to_string(), "/app".to_string()),
        _ if Path::new(include).is_absolute() => (include.to_string(), include.to_string()),
        _ => {
            let joined = format!("/app/{}", include);
            (joined.clone(), joined)
        }
    }
}

/// 生成 COPY 指令列表
///
/// 对齐 railpack `copyLayerPaths()`。
/// 根据 filter.include 生成对应的 COPY 指令。
/// `from_ref` 如果是步骤名引用，调用方应确保已 sanitize；
/// 如果是镜像名引用，则不需要 sanitize。
pub fn copy_layer_paths(from_ref: Option<&str>, filter: &Filter, is_local: bool) -> Vec<String> {
    let mut copies = Vec::new();

    if filter.include.is_empty() {
        // 无 include 列表，整体 COPY
        let from_part = from_ref
            .map(|r| format!(" --from={}", r))
            .unwrap_or_default();
        copies.push(format!("COPY{} . /app", from_part));
        return copies;
    }

    for path in &filter.include {
        let (src, dest) = resolve_paths(path, is_local);
        let from_part = from_ref
            .map(|r| format!(" --from={}", r))
            .unwrap_or_default();
        copies.push(format!("COPY{} {} {}", from_part, src, dest));
    }

    copies
}

/// merge 策略：合并到单阶段
///
/// 第一个 layer 作为 FROM 基础，后续 layer 的文件 COPY 进来。
fn get_merge_state(layers: &[Layer], step_name: &str) -> LayerResult {
    let from_instruction = get_from_instruction(&layers[0], step_name);
    let mut copy_instructions = Vec::new();

    // 第一个 layer 可能有 filter
    if !layers[0].filter.include.is_empty() {
        let from_ref = layers[0]
            .step
            .as_deref()
            .map(sanitize_stage_name);
        let copies = copy_layer_paths(
            from_ref.as_deref(),
            &layers[0].filter,
            layers[0].local == Some(true),
        );
        copy_instructions.extend(copies);
    }

    // 后续 layers 的文件 COPY 进来
    for layer in &layers[1..] {
        let from_ref = sanitize_layer_ref(layer);
        let copies = copy_layer_paths(from_ref.as_deref(), &layer.filter, layer.local == Some(true));
        copy_instructions.extend(copies);
    }

    LayerResult {
        from_instruction,
        copy_instructions,
    }
}

/// copy 策略：逐层 COPY
///
/// 所有 layer（除首层作为 FROM 基础外）逐个 COPY 到目标阶段。
fn get_copy_state(layers: &[Layer], step_name: &str) -> LayerResult {
    let from_instruction = get_from_instruction(&layers[0], step_name);
    let mut copy_instructions = Vec::new();

    for layer in &layers[1..] {
        let from_ref = sanitize_layer_ref(layer);
        let is_local = layer.local == Some(true);
        let copies = copy_layer_paths(from_ref.as_deref(), &layer.filter, is_local);
        copy_instructions.extend(copies);
    }

    LayerResult {
        from_instruction,
        copy_instructions,
    }
}

/// 获取 layer 的 from 引用（步骤名 sanitize，镜像名保持原样）
fn sanitize_layer_ref(layer: &Layer) -> Option<String> {
    if let Some(ref step) = layer.step {
        Some(sanitize_stage_name(step))
    } else {
        layer.image.clone()
    }
}

// ============================================================
// LLB 策略：Layer → OperationOutput
// ============================================================

/// 从 Layer 列表生成 LLB OperationOutput
///
/// 对齐 Dockerfile 版本 `get_full_state_from_layers()`，但输出为 LLB 状态。
/// 根据 should_merge() 决定使用 merge 或 copy 策略。
pub fn get_full_state_from_layers_llb(
    layers: &[Layer],
    step_nodes: &HashMap<String, &StepNode>,
    base_image: &OperationOutput,
) -> crate::Result<OperationOutput> {
    if layers.is_empty() {
        return Ok(base_image.clone());
    }

    if layers.len() == 1 {
        return convert_single_layer_llb(&layers[0], step_nodes, base_image);
    }

    if should_merge(layers) {
        merge_layers_llb(layers, step_nodes, base_image)
    } else {
        copy_layers_llb(layers, step_nodes, base_image)
    }
}

/// 单个 Layer 转换为 LLB 状态
fn layer_to_llb_state(
    layer: &Layer,
    step_nodes: &HashMap<String, &StepNode>,
) -> crate::Result<OperationOutput> {
    if let Some(ref step_name) = layer.step {
        let node = step_nodes.get(step_name.as_str()).ok_or_else(|| {
            anyhow::anyhow!("layer 引用不存在的 step: {}", step_name)
        })?;
        node.get_llb_state().cloned().ok_or_else(|| {
            anyhow::anyhow!("step {} 尚未生成 llb_state", step_name)
        }.into())
    } else if let Some(ref image_name) = layer.image {
        Ok(llb::image(image_name))
    } else if layer.local == Some(true) {
        Ok(llb::local("context"))
    } else {
        Err(anyhow::anyhow!("layer 缺少 step/image/local 引用").into())
    }
}

/// 单 layer 转换
///
/// 无 filter → 直接返回 layer state
/// 有 filter → copy 指定路径到 base_image
fn convert_single_layer_llb(
    layer: &Layer,
    step_nodes: &HashMap<String, &StepNode>,
    base_image: &OperationOutput,
) -> crate::Result<OperationOutput> {
    let layer_state = layer_to_llb_state(layer, step_nodes)?;

    if layer.filter.include.is_empty() {
        return Ok(layer_state);
    }

    // 有 filter：逐路径 copy 到 base_image
    let is_local = layer.local == Some(true);
    let mut state = base_image.clone();
    for path in &layer.filter.include {
        let (src_path, dest_path) = resolve_paths(path, is_local);
        state = llb::copy(layer_state.clone(), &src_path, state, &dest_path);
    }
    Ok(state)
}

/// Merge 策略：首层为基，后续层 copy 到 scratch 再 merge
///
/// BuildKit MergeOp 要求每个输入是 diff（增量层），不是 full state。
/// 因此后续层必须 copy 到 scratch 而非直接使用 full state。
fn merge_layers_llb(
    layers: &[Layer],
    step_nodes: &HashMap<String, &StepNode>,
    base_image: &OperationOutput,
) -> crate::Result<OperationOutput> {
    // 首层作为基础
    let first_state = layer_to_llb_state(&layers[0], step_nodes)?;

    // 如果首层有 filter，copy 到 base_image
    let base = if !layers[0].filter.include.is_empty() {
        let is_local = layers[0].local == Some(true);
        let mut state = base_image.clone();
        for path in &layers[0].filter.include {
            let (src_path, dest_path) = resolve_paths(path, is_local);
            state = llb::copy(first_state.clone(), &src_path, state, &dest_path);
        }
        state
    } else {
        first_state
    };

    // 后续层 copy 到 scratch（生成 diff），然后 merge
    let mut merge_inputs = vec![base.clone()];
    for layer in &layers[1..] {
        let layer_state = layer_to_llb_state(layer, step_nodes)?;
        let is_local = layer.local == Some(true);

        // Copy 到 scratch 形成 diff 层
        let scratch = llb::scratch();
        let mut diff = scratch;
        if layer.filter.include.is_empty() {
            diff = llb::copy(layer_state.clone(), "/", diff, "/");
        } else {
            for path in &layer.filter.include {
                let (src_path, dest_path) = resolve_paths(path, is_local);
                diff = llb::copy(layer_state.clone(), &src_path, diff, &dest_path);
            }
        }
        merge_inputs.push(diff);
    }

    if merge_inputs.len() == 1 {
        return Ok(base);
    }

    llb::merge(merge_inputs)
}

/// Copy 策略：首层为基，后续层逐路径 copy 叠加
fn copy_layers_llb(
    layers: &[Layer],
    step_nodes: &HashMap<String, &StepNode>,
    _base_image: &OperationOutput,
) -> crate::Result<OperationOutput> {
    // 首层作为基础
    let mut state = layer_to_llb_state(&layers[0], step_nodes)?;

    // 后续层逐路径 copy 叠加
    for layer in &layers[1..] {
        let layer_state = layer_to_llb_state(layer, step_nodes)?;
        let is_local = layer.local == Some(true);

        if layer.filter.include.is_empty() {
            // 无 filter → copy 整体
            state = llb::copy(layer_state, "/", state, "/");
        } else {
            for path in &layer.filter.include {
                let (src_path, dest_path) = resolve_paths(path, is_local);
                state = llb::copy(layer_state.clone(), &src_path, state, &dest_path);
            }
        }
    }

    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Filter, Layer};

    // === should_merge 测试 ===

    #[test]
    fn test_should_merge_single_layer_true() {
        let layers = vec![Layer::new_step_layer("install", None)];
        assert!(should_merge(&layers));
    }

    #[test]
    fn test_should_merge_empty_layers_true() {
        let layers: Vec<Layer> = Vec::new();
        assert!(should_merge(&layers));
    }

    #[test]
    fn test_should_merge_non_first_no_include_false() {
        // 第二层无 include → 不合并
        let layers = vec![
            Layer::new_step_layer("packages", None),
            Layer::new_step_layer("build", None),
        ];
        assert!(!should_merge(&layers));
    }

    #[test]
    fn test_should_merge_root_path_false() {
        // 包含 "/" 根路径 → 不合并
        let layers = vec![
            Layer::new_step_layer(
                "packages",
                Some(Filter::include_only(vec!["/".to_string()])),
            ),
            Layer::new_step_layer(
                "build",
                Some(Filter::include_only(vec!["/app".to_string()])),
            ),
        ];
        assert!(!should_merge(&layers));
    }

    #[test]
    fn test_should_merge_local_layer_false() {
        let layers = vec![
            Layer::new_step_layer("packages", None),
            Layer::new_local_layer(),
        ];
        assert!(!should_merge(&layers));
    }

    #[test]
    fn test_should_merge_significant_overlap_false() {
        // 对齐 railpack "overlapping include" 用例
        let layers = vec![
            Layer::new_step_layer(
                "build",
                Some(Filter::include_only(vec![".".to_string()])),
            ),
            Layer::new_step_layer(
                "build",
                Some(Filter::include_only(vec![
                    ".".to_string(),
                    "/root/.cache".to_string(),
                ])),
            ),
        ];
        assert!(!should_merge(&layers));
    }

    #[test]
    fn test_should_merge_no_overlap_true() {
        // 不同路径，无重叠 → 可以合并
        let layers = vec![
            Layer::new_step_layer(
                "mise",
                Some(Filter::include_only(vec![
                    "/mise/shims".to_string(),
                    "/mise/installs".to_string(),
                ])),
            ),
            Layer::new_step_layer(
                "build",
                Some(Filter::include_only(vec!["/root/.cache".to_string()])),
            ),
        ];
        assert!(should_merge(&layers));
    }

    #[test]
    fn test_should_merge_no_overlap_with_excludes_true() {
        // 对齐 railpack "no overlap with excludes" 用例
        let layers = vec![
            Layer::new_step_layer(
                "install",
                Some(Filter::include_only(vec![
                    "/app/node_modules".to_string(),
                ])),
            ),
            Layer::new_step_layer(
                "build",
                Some(Filter::new(
                    vec![".".to_string()],
                    vec!["node_modules".to_string()],
                )),
            ),
            Layer::new_step_layer(
                "build",
                Some(Filter::include_only(vec!["/root/.cache".to_string()])),
            ),
        ];
        assert!(should_merge(&layers));
    }

    #[test]
    fn test_should_merge_overlap_excluded_by_containing_layer_true() {
        // 对齐 railpack "overlap excluded by containing layer" 用例
        let layers = vec![
            Layer::new_step_layer(
                "install",
                Some(Filter::include_only(vec![
                    "/app/node_modules".to_string(),
                ])),
            ),
            Layer::new_step_layer(
                "build",
                Some(Filter::new(
                    vec!["/app".to_string()],
                    vec!["node_modules".to_string()],
                )),
            ),
        ];
        assert!(should_merge(&layers));
    }

    #[test]
    fn test_should_merge_nested_path_excluded_true() {
        // 对齐 railpack "nested path excluded" 用例
        let layers = vec![
            Layer::new_step_layer(
                "install",
                Some(Filter::include_only(vec![
                    "/app/node_modules/.cache".to_string(),
                ])),
            ),
            Layer::new_step_layer(
                "build",
                Some(Filter::new(
                    vec!["/app".to_string()],
                    vec!["node_modules".to_string()],
                )),
            ),
        ];
        assert!(should_merge(&layers));
    }

    #[test]
    fn test_should_merge_relative_path_overlap_not_excluded_false() {
        // 对齐 railpack "relative path overlap not excluded" 用例
        let layers = vec![
            Layer::new_step_layer(
                "mise",
                Some(Filter::include_only(vec![".nvmrc".to_string()])),
            ),
            Layer::new_step_layer(
                "build",
                Some(Filter::new(
                    vec![".".to_string()],
                    vec!["node_modules".to_string(), ".yarn".to_string()],
                )),
            ),
        ];
        assert!(!should_merge(&layers));
    }

    #[test]
    fn test_should_merge_relative_path_overlap_is_excluded_true() {
        // 对齐 railpack "relative path overlap is excluded" 用例
        let layers = vec![
            Layer::new_step_layer(
                "mise",
                Some(Filter::include_only(vec![".nvmrc".to_string()])),
            ),
            Layer::new_step_layer(
                "build",
                Some(Filter::new(
                    vec![".".to_string()],
                    vec!["node_modules".to_string(), ".nvmrc".to_string()],
                )),
            ),
        ];
        assert!(should_merge(&layers));
    }

    #[test]
    fn test_should_merge_multiple_overlaps_some_excluded_false() {
        // 对齐 railpack "multiple overlaps some excluded" 用例
        // /app/dist 未被排除，因此仍有重叠
        let layers = vec![
            Layer::new_step_layer(
                "install",
                Some(Filter::include_only(vec![
                    "/app/node_modules".to_string(),
                    "/app/dist".to_string(),
                ])),
            ),
            Layer::new_step_layer(
                "build",
                Some(Filter::new(
                    vec!["/app".to_string()],
                    vec!["node_modules".to_string()],
                )),
            ),
        ];
        assert!(!should_merge(&layers));
    }

    #[test]
    fn test_should_merge_all_overlaps_excluded_true() {
        // 对齐 railpack "all overlaps excluded" 用例
        let layers = vec![
            Layer::new_step_layer(
                "install",
                Some(Filter::include_only(vec![
                    "/app/node_modules".to_string(),
                    "/app/.yarn".to_string(),
                ])),
            ),
            Layer::new_step_layer(
                "build",
                Some(Filter::new(
                    vec!["/app".to_string()],
                    vec!["node_modules".to_string(), ".yarn".to_string()],
                )),
            ),
        ];
        assert!(should_merge(&layers));
    }

    // === has_significant_overlap 测试 ===

    #[test]
    fn test_has_significant_overlap_exact_match() {
        let l1 = Layer::new_step_layer(
            "build",
            Some(Filter::include_only(vec!["/app/dist".to_string()])),
        );
        let l2 = Layer::new_step_layer(
            "install",
            Some(Filter::include_only(vec!["/app/dist".to_string()])),
        );
        assert!(has_significant_overlap(&l1, &l2));
    }

    #[test]
    fn test_has_significant_overlap_prefix_match() {
        // "/app/dist" 在 "/app" 内部 → 重叠
        let l1 = Layer::new_step_layer(
            "build",
            Some(Filter::include_only(vec!["/app/dist".to_string()])),
        );
        let l2 = Layer::new_step_layer(
            "install",
            Some(Filter::include_only(vec!["/app".to_string()])),
        );
        assert!(has_significant_overlap(&l1, &l2));
    }

    #[test]
    fn test_has_significant_overlap_no_overlap() {
        let l1 = Layer::new_step_layer(
            "build",
            Some(Filter::include_only(vec!["/app/dist".to_string()])),
        );
        let l2 = Layer::new_step_layer(
            "install",
            Some(Filter::include_only(vec!["/var/lib".to_string()])),
        );
        assert!(!has_significant_overlap(&l1, &l2));
    }

    #[test]
    fn test_has_significant_overlap_excluded() {
        // 重叠但被 exclude 覆盖 → 不算显著重叠
        let l1 = Layer::new_step_layer(
            "install",
            Some(Filter::include_only(vec![
                "/app/node_modules".to_string(),
            ])),
        );
        let l2 = Layer::new_step_layer(
            "build",
            Some(Filter::new(
                vec!["/app".to_string()],
                vec!["node_modules".to_string()],
            )),
        );
        assert!(!has_significant_overlap(&l1, &l2));
    }

    // === is_path_excluded 测试 ===

    #[test]
    fn test_is_path_excluded_exact() {
        assert!(is_path_excluded(
            "node_modules",
            &["node_modules".to_string()]
        ));
    }

    #[test]
    fn test_is_path_excluded_prefix() {
        // 嵌套路径匹配父 exclude
        assert!(is_path_excluded(
            "node_modules/.cache",
            &["node_modules".to_string()]
        ));
    }

    #[test]
    fn test_is_path_excluded_no_match() {
        assert!(!is_path_excluded(
            ".nvmrc",
            &["node_modules".to_string(), ".yarn".to_string()]
        ));
    }

    #[test]
    fn test_is_path_excluded_partial_name_no_match() {
        // "node_modules_backup" 不应匹配 "node_modules"
        assert!(!is_path_excluded(
            "node_modules_backup",
            &["node_modules".to_string()]
        ));
    }

    #[test]
    fn test_is_path_excluded_empty_excludes() {
        assert!(!is_path_excluded("anything", &[]));
    }

    // === resolve_paths 测试 ===

    #[test]
    fn test_resolve_paths_local_basename() {
        // local 路径取 basename
        let (src, dest) = resolve_paths("/path/to/file", true);
        assert_eq!(src, "/path/to/file");
        assert_eq!(dest, "/app/file");
    }

    #[test]
    fn test_resolve_paths_local_dot() {
        let (src, dest) = resolve_paths(".", true);
        assert_eq!(src, ".");
        assert_eq!(dest, "/app/.");
    }

    #[test]
    fn test_resolve_paths_absolute() {
        let (src, dest) = resolve_paths("/usr/lib", false);
        assert_eq!(src, "/usr/lib");
        assert_eq!(dest, "/usr/lib");
    }

    #[test]
    fn test_resolve_paths_relative() {
        let (src, dest) = resolve_paths("dist", false);
        assert_eq!(src, "/app/dist");
        assert_eq!(dest, "/app/dist");
    }

    #[test]
    fn test_resolve_paths_dot_non_local() {
        // "." 非 local → 映射为 /app
        let (src, dest) = resolve_paths(".", false);
        assert_eq!(src, "/app");
        assert_eq!(dest, "/app");
    }

    // === copy_layer_paths 测试 ===

    #[test]
    fn test_copy_layer_paths_with_from() {
        let filter = Filter::include_only(vec!["/app/dist".to_string()]);
        let copies = copy_layer_paths(Some("build"), &filter, false);
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0], "COPY --from=build /app/dist /app/dist");
    }

    #[test]
    fn test_copy_layer_paths_without_from() {
        let filter = Filter::include_only(vec!["/app/dist".to_string()]);
        let copies = copy_layer_paths(None, &filter, false);
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0], "COPY /app/dist /app/dist");
    }

    #[test]
    fn test_copy_layer_paths_empty_include() {
        // 无 include → 整体 COPY
        let filter = Filter::default();
        let copies = copy_layer_paths(Some("build"), &filter, false);
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0], "COPY --from=build . /app");
    }

    #[test]
    fn test_copy_layer_paths_multiple_includes() {
        let filter = Filter::include_only(vec![
            "/app/dist".to_string(),
            "/app/static".to_string(),
        ]);
        let copies = copy_layer_paths(Some("build"), &filter, false);
        assert_eq!(copies.len(), 2);
        assert_eq!(copies[0], "COPY --from=build /app/dist /app/dist");
        assert_eq!(copies[1], "COPY --from=build /app/static /app/static");
    }

    // === get_full_state_from_layers 测试 ===

    #[test]
    fn test_get_full_state_empty_layers() {
        let result = get_full_state_from_layers(&[], "deploy");
        assert!(result.from_instruction.is_none());
        assert!(result.copy_instructions.is_empty());
    }

    #[test]
    fn test_get_full_state_single_image_layer() {
        let layers = vec![Layer::new_image_layer("ubuntu:22.04", None)];
        let result = get_full_state_from_layers(&layers, "runtime");
        assert_eq!(
            result.from_instruction,
            Some("FROM ubuntu:22.04 AS runtime".to_string())
        );
        assert!(result.copy_instructions.is_empty());
    }

    #[test]
    fn test_get_full_state_single_step_layer_with_filter() {
        let filter = Filter::include_only(vec!["/app/dist".to_string()]);
        let layers = vec![Layer::new_step_layer("build", Some(filter))];
        let result = get_full_state_from_layers(&layers, "deploy");
        assert_eq!(
            result.from_instruction,
            Some("FROM build AS deploy".to_string())
        );
        assert_eq!(result.copy_instructions.len(), 1);
        assert_eq!(
            result.copy_instructions[0],
            "COPY --from=build /app/dist /app/dist"
        );
    }

    #[test]
    fn test_get_full_state_single_local_layer() {
        let layers = vec![Layer::new_local_layer()];
        let result = get_full_state_from_layers(&layers, "source");
        assert_eq!(
            result.from_instruction,
            Some("FROM scratch AS source".to_string())
        );
        // local layer 的 include 是 ["."]
        assert_eq!(result.copy_instructions.len(), 1);
        assert!(result.copy_instructions[0].contains("/app/."));
    }

    // === LLB 策略测试 ===

    mod llb_tests {
        use super::*;
        use crate::buildkit::llb::source::image;
        use crate::plan::Step;

        /// 辅助函数：创建带 llb_state 的 StepNode
        fn make_step_node_with_state(name: &str) -> StepNode {
            let step = Step::new(name);
            let mut node = StepNode::new(step);
            // 用 image 作为 llb_state（简单模拟已处理的节点）
            node.set_llb_state(image(&format!("test-{}", name)));
            node
        }

        #[test]
        fn test_layers_llb_empty_returns_base() {
            let base = image("ubuntu:22.04");
            let result = get_full_state_from_layers_llb(
                &[],
                &HashMap::new(),
                &base,
            ).unwrap();
            assert_eq!(result.serialized_op.digest, base.serialized_op.digest);
        }

        #[test]
        fn test_layers_llb_single_step() {
            let node = make_step_node_with_state("install");
            let expected_digest = node.get_llb_state().unwrap().serialized_op.digest.clone();
            let nodes: HashMap<String, &StepNode> =
                [("install".to_string(), &node)].into_iter().collect();

            let layers = vec![Layer::new_step_layer("install", None)];
            let base = image("ubuntu:22.04");
            let result = get_full_state_from_layers_llb(&layers, &nodes, &base).unwrap();
            // 无 filter → 直接返回 step 的 llb_state
            assert_eq!(result.serialized_op.digest, expected_digest);
        }

        #[test]
        fn test_layers_llb_single_image() {
            let layers = vec![Layer::new_image_layer("node:20", None)];
            let base = image("ubuntu:22.04");
            let result = get_full_state_from_layers_llb(
                &layers,
                &HashMap::new(),
                &base,
            ).unwrap();
            // 无 filter → 返回 image 状态
            let expected = image("node:20");
            assert_eq!(result.serialized_op.digest, expected.serialized_op.digest);
        }

        #[test]
        fn test_layers_llb_single_with_filter() {
            let node = make_step_node_with_state("build");
            let nodes: HashMap<String, &StepNode> =
                [("build".to_string(), &node)].into_iter().collect();

            let filter = Filter::include_only(vec!["/app/dist".to_string()]);
            let layers = vec![Layer::new_step_layer("build", Some(filter))];
            let base = image("ubuntu:22.04");
            let result = get_full_state_from_layers_llb(&layers, &nodes, &base).unwrap();
            // 有 filter → 生成 copy 链，digest 不同于 base 和 node
            assert_ne!(result.serialized_op.digest, base.serialized_op.digest);
        }

        #[test]
        fn test_layers_llb_merge_strategy() {
            // 两个不重叠层 → should_merge = true → merge 策略
            let node1 = make_step_node_with_state("install");
            let node2 = make_step_node_with_state("build");
            let nodes: HashMap<String, &StepNode> = [
                ("install".to_string(), &node1),
                ("build".to_string(), &node2),
            ].into_iter().collect();

            let layers = vec![
                Layer::new_step_layer("install", Some(Filter::include_only(
                    vec!["/app/node_modules".to_string()],
                ))),
                Layer::new_step_layer("build", Some(Filter::include_only(
                    vec!["/root/.cache".to_string()],
                ))),
            ];
            assert!(should_merge(&layers), "层不重叠应使用 merge");

            let base = image("ubuntu:22.04");
            let result = get_full_state_from_layers_llb(&layers, &nodes, &base).unwrap();
            assert_ne!(result.serialized_op.digest, base.serialized_op.digest);
        }

        #[test]
        fn test_layers_llb_copy_strategy() {
            // 两个重叠层 → should_merge = false → copy 策略
            let node1 = make_step_node_with_state("install");
            let node2 = make_step_node_with_state("build");
            let nodes: HashMap<String, &StepNode> = [
                ("install".to_string(), &node1),
                ("build".to_string(), &node2),
            ].into_iter().collect();

            let layers = vec![
                Layer::new_step_layer("install", None),
                Layer::new_step_layer("build", None),
            ];
            assert!(!should_merge(&layers), "非首层无 include 不应 merge");

            let base = image("ubuntu:22.04");
            let result = get_full_state_from_layers_llb(&layers, &nodes, &base).unwrap();
            assert_ne!(result.serialized_op.digest, base.serialized_op.digest);
        }

        #[test]
        fn test_layer_to_llb_state_missing_step_errors() {
            let layers = vec![Layer::new_step_layer("nonexistent", None)];
            let base = image("ubuntu:22.04");
            let result = get_full_state_from_layers_llb(
                &layers,
                &HashMap::new(),
                &base,
            );
            assert!(result.is_err(), "引用不存在的 step 应返回错误");
        }

        #[test]
        fn test_layer_to_llb_state_unprocessed_step_errors() {
            // StepNode 无 llb_state
            let step = Step::new("unprocessed");
            let node = StepNode::new(step);
            let nodes: HashMap<String, &StepNode> =
                [("unprocessed".to_string(), &node)].into_iter().collect();

            let layers = vec![Layer::new_step_layer("unprocessed", None)];
            let base = image("ubuntu:22.04");
            let result = get_full_state_from_layers_llb(&layers, &nodes, &base);
            assert!(result.is_err(), "未处理的 step 应返回错误");
        }
    }
}
