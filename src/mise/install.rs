use crate::plan::command::Command;

/// 容器内 mise 安装目录
pub const INSTALL_DIR: &str = "/tmp/arcpack/mise";

/// mise 安装脚本 URL
const MISE_INSTALL_SCRIPT: &str = "https://mise.jdx.dev/install.sh";

/// 生成容器内 mise 安装命令序列
pub fn get_install_commands(mise_toml_path: &str) -> Vec<Command> {
    vec![
        // 安装 mise
        Command::new_exec_shell(&format!(
            "curl -fsSL {} | sh -s -- -y --install-dir {}",
            MISE_INSTALL_SCRIPT, INSTALL_DIR
        )),
        // 使用 mise 安装工具
        Command::new_exec_shell(&format!(
            "PATH={}:$PATH MISE_DATA_DIR=/mise/data MISE_CACHE_DIR=/mise/cache MISE_STATE_DIR=/mise/state mise install -f {}",
            INSTALL_DIR, mise_toml_path
        )),
    ]
}

/// 生成 mise install-into 命令（将特定包安装到指定目录）
pub fn get_install_into_command(package: &str, version: &str, dest_dir: &str) -> Command {
    Command::new_exec_shell(&format!(
        "PATH={}:$PATH MISE_DATA_DIR=/mise/data MISE_CACHE_DIR=/mise/cache MISE_STATE_DIR=/mise/state mise install-into {} {}@{} {}",
        INSTALL_DIR, dest_dir, package, version, dest_dir
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_install_commands_produces_two_commands() {
        let cmds = get_install_commands("/etc/mise.toml");
        assert_eq!(cmds.len(), 2);
    }

    #[test]
    fn test_get_install_commands_first_is_curl() {
        let cmds = get_install_commands("/etc/mise.toml");
        if let Command::Exec(ref exec) = cmds[0] {
            assert!(exec.cmd.contains("curl"));
            assert!(exec.cmd.contains(MISE_INSTALL_SCRIPT));
        } else {
            panic!("expected Exec command");
        }
    }

    #[test]
    fn test_get_install_commands_second_is_mise_install() {
        let cmds = get_install_commands("/etc/mise.toml");
        if let Command::Exec(ref exec) = cmds[1] {
            assert!(exec.cmd.contains("mise install"));
            assert!(exec.cmd.contains("/etc/mise.toml"));
        } else {
            panic!("expected Exec command");
        }
    }

    #[test]
    fn test_get_install_into_command() {
        let cmd = get_install_into_command("caddy", "2.0.0", "/railpack/caddy");
        if let Command::Exec(ref exec) = cmd {
            assert!(exec.cmd.contains("mise install-into"));
            assert!(exec.cmd.contains("caddy@2.0.0"));
        } else {
            panic!("expected Exec command");
        }
    }
}
