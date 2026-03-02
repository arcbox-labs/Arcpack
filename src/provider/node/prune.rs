use super::package_manager::PackageManagerKind;
/// 依赖裁剪模块
///
/// 对齐 railpack `core/providers/node/prune.go`
/// 通过 ARCPACK_PRUNE_DEPS=true 触发，构建后删除 devDependencies 以减小镜像体积。
use std::collections::HashMap;

use crate::app::environment::Environment;
use crate::generate::GenerateContext;
use crate::plan::{BuildPlan, Command, Layer};

/// 获取裁剪命令
///
/// 各包管理器有不同的裁剪方式
pub fn get_prune_command(pm: &PackageManagerKind, env: &Environment) -> Option<String> {
    // 用户自定义裁剪命令优先
    if let (Some(custom_cmd), _) = env.get_config_variable("NODE_PRUNE_CMD") {
        if !custom_cmd.is_empty() {
            return Some(custom_cmd);
        }
    }

    // 是否启用裁剪
    if !env.is_config_variable_truthy("PRUNE_DEPS") {
        return None;
    }

    let cmd = match pm {
        PackageManagerKind::Npm => "npm prune --omit=dev --ignore-scripts".to_string(),
        PackageManagerKind::Pnpm => "pnpm prune --prod --ignore-scripts".to_string(),
        PackageManagerKind::Bun => {
            "rm -rf node_modules && bun install --production --ignore-scripts".to_string()
        }
        PackageManagerKind::Yarn1 => "yarn install --production=true".to_string(),
        PackageManagerKind::YarnBerry => "yarn workspaces focus --production --all".to_string(),
    };

    Some(cmd)
}

/// 创建裁剪步骤
///
/// 返回裁剪步骤名称（如果创建了的话）
pub fn create_prune_step(
    ctx: &mut GenerateContext,
    pm: &PackageManagerKind,
    input_step_name: &str,
) -> Option<String> {
    let prune_cmd = get_prune_command(pm, &ctx.env)?;
    let install_cache = pm.get_install_cache(&mut ctx.caches);

    let prune_step = ctx.new_command_step("prune");
    prune_step.add_input(Layer::new_step_layer(input_step_name, None));

    // 对齐 railpack：prune 复用包管理器安装缓存（如 npm-install）。
    prune_step.add_cache(&install_cache);
    // prune 不应默认继承 `*` secrets。
    prune_step.secrets.clear();
    // 对齐 railpack：npm prune 时显式开启 production 模式。
    if *pm == PackageManagerKind::Npm {
        prune_step.add_variables(&HashMap::from([(
            "NPM_CONFIG_PRODUCTION".to_string(),
            "true".to_string(),
        )]));
    }

    if prune_cmd.contains("&&") || prune_cmd.contains("||") || prune_cmd.contains(';') {
        prune_step.add_command(Command::new_exec_shell(&prune_cmd));
    } else {
        prune_step.add_command(Command::new_exec(prune_cmd));
    }

    Some("prune".to_string())
}

/// CleansePlan 后处理：裁剪步骤存在时，移除 install 步骤的 node_modules 缓存挂载
///
/// 对齐 railpack `core/providers/node/prune.go` cleansePlan 逻辑
pub fn cleanse_plan_for_prune(plan: &mut BuildPlan, has_prune: bool) {
    if !has_prune {
        return;
    }

    // 移除 install 和 build 步骤中的 node_modules 缓存挂载
    // 因为裁剪步骤会重新安装，缓存键计算会出错
    for step in &mut plan.steps {
        let step_name = step.name.as_deref().unwrap_or("");
        if step_name == "install" || step_name.starts_with("install:") {
            step.caches.retain(|c| !c.contains("node-modules"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_env(vars: &[(&str, &str)]) -> Environment {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Environment::new(map)
    }

    #[test]
    fn test_prune_disabled_by_default() {
        let env = make_env(&[]);
        assert!(get_prune_command(&PackageManagerKind::Npm, &env).is_none());
    }

    #[test]
    fn test_prune_enabled_npm() {
        let env = make_env(&[("ARCPACK_PRUNE_DEPS", "true")]);
        let cmd = get_prune_command(&PackageManagerKind::Npm, &env).unwrap();
        assert!(cmd.contains("npm prune --omit=dev"));
    }

    #[test]
    fn test_prune_enabled_pnpm() {
        let env = make_env(&[("ARCPACK_PRUNE_DEPS", "1")]);
        let cmd = get_prune_command(&PackageManagerKind::Pnpm, &env).unwrap();
        assert!(cmd.contains("pnpm prune --prod"));
    }

    #[test]
    fn test_prune_enabled_bun() {
        let env = make_env(&[("ARCPACK_PRUNE_DEPS", "true")]);
        let cmd = get_prune_command(&PackageManagerKind::Bun, &env).unwrap();
        assert!(cmd.contains("rm -rf node_modules"));
        assert!(cmd.contains("bun install --production"));
    }

    #[test]
    fn test_prune_enabled_yarn1() {
        let env = make_env(&[("ARCPACK_PRUNE_DEPS", "true")]);
        let cmd = get_prune_command(&PackageManagerKind::Yarn1, &env).unwrap();
        assert!(cmd.contains("yarn install --production=true"));
    }

    #[test]
    fn test_prune_enabled_yarn_berry() {
        let env = make_env(&[("ARCPACK_PRUNE_DEPS", "true")]);
        let cmd = get_prune_command(&PackageManagerKind::YarnBerry, &env).unwrap();
        assert!(cmd.contains("yarn workspaces focus --production"));
    }

    #[test]
    fn test_prune_custom_command() {
        let env = make_env(&[("ARCPACK_NODE_PRUNE_CMD", "custom prune")]);
        let cmd = get_prune_command(&PackageManagerKind::Npm, &env).unwrap();
        assert_eq!(cmd, "custom prune");
    }

    #[test]
    fn test_cleanse_plan_removes_node_modules_cache() {
        use crate::plan::Step;
        let mut plan = BuildPlan::new();
        let mut step = Step::new("install");
        step.caches = vec!["node-modules".to_string(), "apt-cache".to_string()];
        plan.add_step(step);

        cleanse_plan_for_prune(&mut plan, true);
        let install = plan
            .steps
            .iter()
            .find(|s| s.name.as_deref() == Some("install"))
            .unwrap();
        assert!(!install.caches.contains(&"node-modules".to_string()));
        assert!(install.caches.contains(&"apt-cache".to_string()));
    }

    #[test]
    fn test_cleanse_plan_noop_without_prune() {
        use crate::plan::Step;
        let mut plan = BuildPlan::new();
        let mut step = Step::new("install");
        step.caches = vec!["node-modules".to_string()];
        plan.add_step(step);

        cleanse_plan_for_prune(&mut plan, false);
        let install = plan
            .steps
            .iter()
            .find(|s| s.name.as_deref() == Some("install"))
            .unwrap();
        assert!(install.caches.contains(&"node-modules".to_string()));
    }
}
