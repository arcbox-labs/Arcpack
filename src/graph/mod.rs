/// 通用有向无环图（DAG）数据结构
///
/// 对齐 railpack `buildkit/graph/graph.go` 实现。
/// 边关系存储在 Graph 侧（children / parents 两个 HashMap），
/// Node trait 只需提供 name() 方法。
use std::collections::{HashMap, HashSet};

use crate::ArcpackError;
use crate::Result;

/// 图节点 trait，只需提供名称
pub trait Node {
    fn name(&self) -> &str;
}

/// 有向无环图
///
/// - `nodes`: 按名称索引的节点集合
/// - `children`: parent -> [children]，记录每个节点的子节点列表
/// - `parents`: child -> [parents]，记录每个节点的父节点列表
pub struct Graph<T: Node> {
    nodes: HashMap<String, T>,
    children: HashMap<String, Vec<String>>,
    parents: HashMap<String, Vec<String>>,
}

impl<T: Node> Graph<T> {
    /// 创建空图
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            children: HashMap::new(),
            parents: HashMap::new(),
        }
    }

    /// 注册节点（按 name 去重）
    pub fn add_node(&mut self, node: T) {
        let name = node.name().to_string();
        self.nodes.insert(name, node);
    }

    /// 按名称查找节点（不可变引用）
    pub fn get_node(&self, name: &str) -> Option<&T> {
        self.nodes.get(name)
    }

    /// 按名称查找节点（可变引用）
    pub fn get_node_mut(&mut self, name: &str) -> Option<&mut T> {
        self.nodes.get_mut(name)
    }

    /// 返回所有节点的引用
    pub fn get_nodes(&self) -> &HashMap<String, T> {
        &self.nodes
    }

    /// 添加有向边：parent -> child
    ///
    /// 同时记录到 children 和 parents 映射中，自动跳过重复边。
    pub fn add_edge(&mut self, parent: &str, child: &str) {
        // 记录 parent 的子节点
        let children_list = self.children.entry(parent.to_string()).or_default();
        if !children_list.contains(&child.to_string()) {
            children_list.push(child.to_string());
        }

        // 记录 child 的父节点
        let parents_list = self.parents.entry(child.to_string()).or_default();
        if !parents_list.contains(&parent.to_string()) {
            parents_list.push(parent.to_string());
        }
    }

    /// 获取指定节点的父节点列表，不存在则返回空切片
    pub fn get_parents(&self, name: &str) -> &[String] {
        self.parents.get(name).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// 获取指定节点的子节点列表，不存在则返回空切片
    pub fn get_children(&self, name: &str) -> &[String] {
        self.children.get(name).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// 拓扑排序，返回处理顺序（父节点在前，叶节点在后）
    ///
    /// 对齐 railpack `graph.go:46-92`：
    /// 1. 从叶节点（无子节点的节点）开始 DFS
    /// 2. 对每个节点先递归访问其所有父节点
    /// 3. 使用 temp 集合（灰色标记）检测环
    /// 4. 最后扫描剩余未访问节点
    pub fn compute_processing_order(&self) -> Result<Vec<String>> {
        let mut visited = HashSet::new();
        let mut temp = HashSet::new();
        let mut order = Vec::with_capacity(self.nodes.len());

        // 从叶节点开始（没有子节点的节点），排序保证确定性
        let mut leaf_names: Vec<&String> = self
            .nodes
            .keys()
            .filter(|name| self.get_children(name).is_empty())
            .collect();
        leaf_names.sort();
        for name in leaf_names {
            self.visit(name, &mut visited, &mut temp, &mut order)?;
        }

        // 处理剩余未访问的节点，排序保证确定性
        let mut remaining: Vec<String> = self
            .nodes
            .keys()
            .filter(|name| !visited.contains(name.as_str()))
            .cloned()
            .collect();
        remaining.sort();
        for name in &remaining {
            self.visit(name, &mut visited, &mut temp, &mut order)?;
        }

        Ok(order)
    }

    /// DFS 递归访问节点
    ///
    /// - temp 集合用于检测回边（环）
    /// - 先递归访问所有父节点，保证父节点排在前面
    fn visit(
        &self,
        name: &str,
        visited: &mut HashSet<String>,
        temp: &mut HashSet<String>,
        order: &mut Vec<String>,
    ) -> Result<()> {
        // 灰色标记存在说明有环
        if temp.contains(name) {
            return Err(ArcpackError::CycleDetected {
                node: name.to_string(),
            });
        }
        // 已访问过则跳过
        if visited.contains(name) {
            return Ok(());
        }

        temp.insert(name.to_string());

        // 先访问所有父节点
        for parent in self.get_parents(name) {
            self.visit(parent, visited, temp, order)?;
        }

        temp.remove(name);
        visited.insert(name.to_string());
        order.push(name.to_string());

        Ok(())
    }

    /// 传递归约：移除冗余的直接边
    ///
    /// 对齐 railpack `graph.go:95-136`：
    /// 对每个节点 N，检查其每个父节点 P——
    /// 如果 P 可以通过 N 的其他父节点间接到达，则说明 P->N 这条直接边是冗余的，
    /// 将其从 parents 和 children 映射中移除。
    pub fn compute_transitive_dependencies(&mut self) {
        // 收集所有节点名称，避免借用冲突
        let node_names: Vec<String> = self.nodes.keys().cloned().collect();

        for node_name in &node_names {
            let current_parents = self.parents.get(node_name).cloned().unwrap_or_default();

            let mut redundant_parents: Vec<String> = Vec::new();

            for parent in &current_parents {
                let mut is_redundant = false;

                for other_parent in &current_parents {
                    if other_parent == parent {
                        continue;
                    }

                    // 从 other_parent 出发沿父链 DFS，看能否到达 parent
                    let mut dfs_visited = HashSet::new();
                    if self.is_reachable_via_parents(other_parent, parent, &mut dfs_visited) {
                        is_redundant = true;
                        break;
                    }
                }

                if is_redundant {
                    redundant_parents.push(parent.clone());
                }
            }

            // 移除冗余边
            for redundant in &redundant_parents {
                // 从 node_name 的 parents 中移除 redundant
                if let Some(parents_list) = self.parents.get_mut(node_name) {
                    parents_list.retain(|p| p != redundant);
                }
                // 从 redundant 的 children 中移除 node_name
                if let Some(children_list) = self.children.get_mut(redundant) {
                    children_list.retain(|c| c != node_name);
                }
            }
        }
    }

    /// 从 current 出发沿父链 DFS，判断是否能到达 target
    fn is_reachable_via_parents(
        &self,
        current: &str,
        target: &str,
        visited: &mut HashSet<String>,
    ) -> bool {
        if current == target {
            return true;
        }
        for parent in self.get_parents(current) {
            if !visited.contains(parent.as_str()) {
                visited.insert(parent.clone());
                if self.is_reachable_via_parents(parent, target, visited) {
                    return true;
                }
            }
        }
        false
    }
}

impl<T: Node> Default for Graph<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用简单节点
    struct SimpleNode {
        name: String,
    }

    impl Node for SimpleNode {
        fn name(&self) -> &str {
            &self.name
        }
    }

    impl SimpleNode {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
            }
        }
    }

    /// 辅助函数：在有序列表中检查 a 是否排在 b 前面
    fn is_before(order: &[String], a: &str, b: &str) -> bool {
        let pos_a = order.iter().position(|x| x == a);
        let pos_b = order.iter().position(|x| x == b);
        match (pos_a, pos_b) {
            (Some(pa), Some(pb)) => pa < pb,
            _ => false,
        }
    }

    #[test]
    fn test_linear_chain_ordering() {
        // A -> B -> C（A 是 B 的父节点，B 是 C 的父节点）
        // 期望顺序：A 在 B 前，B 在 C 前
        let mut g = Graph::new();
        g.add_node(SimpleNode::new("A"));
        g.add_node(SimpleNode::new("B"));
        g.add_node(SimpleNode::new("C"));
        g.add_edge("A", "B");
        g.add_edge("B", "C");

        let order = g.compute_processing_order().unwrap();
        assert_eq!(order.len(), 3);
        assert!(is_before(&order, "A", "B"), "A should come before B");
        assert!(is_before(&order, "B", "C"), "B should come before C");
    }

    #[test]
    fn test_diamond_ordering() {
        // 菱形依赖：A -> B, A -> C, B -> D, C -> D
        // 期望：A 在 B、C 前；B、C 在 D 前
        let mut g = Graph::new();
        g.add_node(SimpleNode::new("A"));
        g.add_node(SimpleNode::new("B"));
        g.add_node(SimpleNode::new("C"));
        g.add_node(SimpleNode::new("D"));
        g.add_edge("A", "B");
        g.add_edge("A", "C");
        g.add_edge("B", "D");
        g.add_edge("C", "D");

        let order = g.compute_processing_order().unwrap();
        assert_eq!(order.len(), 4);
        assert!(is_before(&order, "A", "B"), "A should come before B");
        assert!(is_before(&order, "A", "C"), "A should come before C");
        assert!(is_before(&order, "B", "D"), "B should come before D");
        assert!(is_before(&order, "C", "D"), "C should come before D");
    }

    #[test]
    fn test_cycle_detection_returns_cycle_detected_error() {
        // A -> B -> C -> A 形成环
        let mut g = Graph::new();
        g.add_node(SimpleNode::new("A"));
        g.add_node(SimpleNode::new("B"));
        g.add_node(SimpleNode::new("C"));
        g.add_edge("A", "B");
        g.add_edge("B", "C");
        g.add_edge("C", "A");

        let result = g.compute_processing_order();
        assert!(result.is_err());
        match result.unwrap_err() {
            ArcpackError::CycleDetected { node } => {
                // 环中的某个节点会被检测到
                assert!(
                    ["A", "B", "C"].contains(&node.as_str()),
                    "cycle should be detected on one of the cycle nodes, got: {}",
                    node
                );
            }
            other => panic!("expected CycleDetected error, got: {:?}", other),
        }
    }

    #[test]
    fn test_empty_graph_returns_empty_order() {
        let g: Graph<SimpleNode> = Graph::new();
        let order = g.compute_processing_order().unwrap();
        assert!(order.is_empty());
    }

    #[test]
    fn test_single_node_returns_single_element() {
        let mut g = Graph::new();
        g.add_node(SimpleNode::new("A"));

        let order = g.compute_processing_order().unwrap();
        assert_eq!(order, vec!["A"]);
    }

    #[test]
    fn test_transitive_reduction_removes_redundant_edge() {
        // A -> B -> C，同时 A -> C（冗余边）
        // 归约后 A -> C 应被移除，因为 A 可通过 B 间接到达 C
        let mut g = Graph::new();
        g.add_node(SimpleNode::new("A"));
        g.add_node(SimpleNode::new("B"));
        g.add_node(SimpleNode::new("C"));
        g.add_edge("A", "B");
        g.add_edge("B", "C");
        g.add_edge("A", "C"); // 冗余边

        // 归约前 C 有两个父节点
        assert_eq!(g.get_parents("C").len(), 2);

        g.compute_transitive_dependencies();

        // 归约后 C 只有 B 一个父节点
        assert_eq!(g.get_parents("C"), &["B".to_string()]);
        // A 的子节点中不再包含 C
        assert!(!g.get_children("A").contains(&"C".to_string()));
        // A -> B 仍然存在
        assert!(g.get_children("A").contains(&"B".to_string()));
    }

    #[test]
    fn test_get_node_existing_returns_some() {
        let mut g = Graph::new();
        g.add_node(SimpleNode::new("hello"));

        let node = g.get_node("hello");
        assert!(node.is_some());
        assert_eq!(node.unwrap().name(), "hello");
    }

    #[test]
    fn test_get_node_missing_returns_none() {
        let g: Graph<SimpleNode> = Graph::new();
        assert!(g.get_node("nonexistent").is_none());
    }

    #[test]
    fn test_add_edge_records_both_directions() {
        let mut g = Graph::new();
        g.add_node(SimpleNode::new("X"));
        g.add_node(SimpleNode::new("Y"));
        g.add_edge("X", "Y");

        // X 的子节点包含 Y
        assert_eq!(g.get_children("X"), &["Y".to_string()]);
        // Y 的父节点包含 X
        assert_eq!(g.get_parents("Y"), &["X".to_string()]);
        // 反方向为空
        assert!(g.get_parents("X").is_empty());
        assert!(g.get_children("Y").is_empty());

        // 重复添加同一条边不应产生重复
        g.add_edge("X", "Y");
        assert_eq!(g.get_children("X").len(), 1);
        assert_eq!(g.get_parents("Y").len(), 1);
    }
}
