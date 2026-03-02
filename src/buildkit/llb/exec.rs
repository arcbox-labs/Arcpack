// LLB Exec 操作：容器内执行命令
//
// 使用 builder pattern 构建 ExecOp，
// 支持缓存挂载、Secret 环境变量、只读层挂载等。

use crate::buildkit::proto::pb;

use super::operation::{make_output, serialize_op, OpMetadata, OperationOutput};

/// Exec 操作构建器
///
/// 对齐 Go llb.State.Run() + RunOption
pub struct ExecBuilder {
    /// 执行环境（rootfs）
    input: OperationOutput,
    /// 命令参数
    args: Vec<String>,
    /// 环境变量
    env: Vec<(String, String)>,
    /// 工作目录
    cwd: String,
    /// 额外挂载
    mounts: Vec<MountSpec>,
    /// 描述信息
    metadata: OpMetadata,
}

/// 挂载规格
///
/// 对齐 Go pb.Mount 的各种模式
pub enum MountSpec {
    /// 缓存挂载：持久化目录，跨构建复用
    Cache {
        target: String,
        cache_id: String,
        sharing: CacheSharingMode,
    },
    /// Secret 环境变量挂载
    SecretEnv { name: String, env_name: String },
    /// 只读层挂载
    ReadOnlyLayer {
        input: OperationOutput,
        target: String,
    },
}

/// 缓存共享模式
///
/// 对齐 Go pb.CacheSharingOpt
#[derive(Clone, Copy, Debug)]
pub enum CacheSharingMode {
    /// 多个构建步骤可同时读写
    Shared,
    /// 互斥锁定
    Locked,
}

impl ExecBuilder {
    /// 创建新的 ExecBuilder
    pub fn new(input: OperationOutput, args: Vec<String>) -> Self {
        Self {
            input,
            args,
            env: Vec::new(),
            cwd: "/".to_string(),
            mounts: Vec::new(),
            metadata: OpMetadata::default(),
        }
    }

    /// 添加环境变量
    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    /// 设置工作目录
    pub fn cwd(mut self, dir: &str) -> Self {
        self.cwd = dir.to_string();
        self
    }

    /// 添加通用挂载
    pub fn add_mount(mut self, mount: MountSpec) -> Self {
        self.mounts.push(mount);
        self
    }

    /// 添加缓存挂载
    pub fn add_cache_mount(self, target: &str, cache_id: &str, sharing: CacheSharingMode) -> Self {
        self.add_mount(MountSpec::Cache {
            target: target.to_string(),
            cache_id: cache_id.to_string(),
            sharing,
        })
    }

    /// 添加 Secret 环境变量
    pub fn add_secret_env(self, name: &str, env_name: &str) -> Self {
        self.add_mount(MountSpec::SecretEnv {
            name: name.to_string(),
            env_name: env_name.to_string(),
        })
    }

    /// 设置描述信息
    pub fn description(mut self, desc: &str) -> Self {
        self.metadata
            .description
            .insert("llb.customname".to_string(), desc.to_string());
        self
    }

    /// 构建 ExecOp，返回 rootfs 输出
    ///
    /// 对齐 Go .Root()
    pub fn root(self) -> OperationOutput {
        // 收集所有输入（rootfs + 只读层挂载）
        let mut inputs = vec![self.input.clone()];
        let mut pb_mounts = Vec::new();
        let mut secret_envs = Vec::new();

        // rootfs mount：input 0，dest "/"，output 0
        pb_mounts.push(pb::Mount {
            input: 0,
            dest: "/".to_string(),
            output: 0,
            mount_type: pb::MountType::Bind as i32,
            ..Default::default()
        });

        for mount in &self.mounts {
            match mount {
                MountSpec::Cache {
                    target,
                    cache_id,
                    sharing,
                } => {
                    let sharing_val = match sharing {
                        CacheSharingMode::Shared => pb::CacheSharingOpt::Shared as i32,
                        CacheSharingMode::Locked => pb::CacheSharingOpt::Locked as i32,
                    };
                    pb_mounts.push(pb::Mount {
                        dest: target.clone(),
                        mount_type: pb::MountType::Cache as i32,
                        cache_opt: Some(pb::CacheOpt {
                            id: cache_id.clone(),
                            sharing: sharing_val,
                        }),
                        // cache mount 不关联 input/output
                        input: -1,
                        output: -1,
                        ..Default::default()
                    });
                }
                MountSpec::SecretEnv { name, env_name } => {
                    secret_envs.push(pb::SecretEnv {
                        id: name.clone(),
                        name: env_name.clone(),
                        optional: false,
                    });
                }
                MountSpec::ReadOnlyLayer { input, target } => {
                    let input_index = inputs.len() as i64;
                    inputs.push(input.clone());
                    pb_mounts.push(pb::Mount {
                        input: input_index,
                        dest: target.clone(),
                        output: -1, // 只读，无输出
                        readonly: true,
                        mount_type: pb::MountType::Bind as i32,
                        ..Default::default()
                    });
                }
            }
        }

        // 构造环境变量列表
        let env: Vec<String> = self
            .env
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();

        let exec_op = pb::ExecOp {
            meta: Some(pb::Meta {
                args: self.args,
                env,
                cwd: self.cwd,
                ..Default::default()
            }),
            mounts: pb_mounts,
            secretenv: secret_envs,
            ..Default::default()
        };

        // 构造 pb::Op
        let pb_inputs: Vec<pb::Input> = inputs
            .iter()
            .map(|i| pb::Input {
                digest: i.serialized_op.digest.clone(),
                index: i.output_index,
            })
            .collect();

        let op = pb::Op {
            inputs: pb_inputs,
            op: Some(pb::op::Op::Exec(exec_op)),
            ..Default::default()
        };

        let serialized = serialize_op(&op, self.metadata, inputs);
        make_output(serialized, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buildkit::llb::source::image;
    use prost::Message;

    fn decode_exec_op(output: &OperationOutput) -> (pb::Op, pb::ExecOp) {
        let op = pb::Op::decode(output.serialized_op.bytes.as_slice()).unwrap();
        let exec = match op.op.clone().unwrap() {
            pb::op::Op::Exec(e) => e,
            _ => panic!("应为 ExecOp"),
        };
        (op, exec)
    }

    #[test]
    fn test_exec_basic_args_and_cwd() {
        let base = image("node:20");
        let output = ExecBuilder::new(
            base,
            vec!["/bin/sh".into(), "-c".into(), "npm install".into()],
        )
        .cwd("/app")
        .root();

        let (_op, exec) = decode_exec_op(&output);
        let meta = exec.meta.unwrap();
        assert_eq!(meta.args, vec!["/bin/sh", "-c", "npm install"]);
        assert_eq!(meta.cwd, "/app");
    }

    #[test]
    fn test_exec_env_accumulates() {
        let base = image("node:20");
        let output = ExecBuilder::new(base, vec!["echo".into()])
            .env("NODE_ENV", "production")
            .env("CI", "true")
            .root();

        let (_op, exec) = decode_exec_op(&output);
        let meta = exec.meta.unwrap();
        assert_eq!(meta.env, vec!["NODE_ENV=production", "CI=true"]);
    }

    #[test]
    fn test_exec_cache_mount() {
        let base = image("node:20");
        let output = ExecBuilder::new(base, vec!["npm".into(), "install".into()])
            .add_cache_mount("/root/.npm", "npm-cache", CacheSharingMode::Shared)
            .root();

        let (_op, exec) = decode_exec_op(&output);
        // rootfs mount + cache mount = 2
        assert_eq!(exec.mounts.len(), 2);
        let cache_mount = &exec.mounts[1];
        assert_eq!(cache_mount.mount_type, pb::MountType::Cache as i32);
        assert_eq!(cache_mount.dest, "/root/.npm");
        let cache_opt = cache_mount.cache_opt.as_ref().unwrap();
        assert_eq!(cache_opt.id, "npm-cache");
        assert_eq!(cache_opt.sharing, pb::CacheSharingOpt::Shared as i32);
    }

    #[test]
    fn test_exec_secret_env() {
        let base = image("node:20");
        let output = ExecBuilder::new(base, vec!["deploy".into()])
            .add_secret_env("my-token", "API_TOKEN")
            .root();

        let (_op, exec) = decode_exec_op(&output);
        assert_eq!(exec.secretenv.len(), 1);
        assert_eq!(exec.secretenv[0].id, "my-token");
        assert_eq!(exec.secretenv[0].name, "API_TOKEN");
    }

    #[test]
    fn test_exec_readonly_layer_mount() {
        let base = image("node:20");
        let other = image("alpine:3");
        let output = ExecBuilder::new(base, vec!["ls".into()])
            .add_mount(MountSpec::ReadOnlyLayer {
                input: other.clone(),
                target: "/mnt/data".to_string(),
            })
            .root();

        let (op, exec) = decode_exec_op(&output);
        // rootfs + readonly = 2 mounts
        assert_eq!(exec.mounts.len(), 2);
        let ro_mount = &exec.mounts[1];
        assert_eq!(ro_mount.dest, "/mnt/data");
        assert!(ro_mount.readonly);
        assert_eq!(ro_mount.input, 1); // 第二个输入
                                       // Op.inputs 应有 2 个
        assert_eq!(op.inputs.len(), 2);
        assert_eq!(op.inputs[1].digest, other.serialized_op.digest);
    }

    #[test]
    fn test_exec_root_output_index_zero() {
        let base = image("node:20");
        let output = ExecBuilder::new(base, vec!["echo".into()]).root();
        assert_eq!(output.output_index, 0);
    }

    #[test]
    fn test_exec_multiple_mount_types() {
        let base = image("node:20");
        let output = ExecBuilder::new(base, vec!["build".into()])
            .add_cache_mount("/cache", "build-cache", CacheSharingMode::Locked)
            .add_secret_env("token", "TOKEN")
            .root();

        let (_op, exec) = decode_exec_op(&output);
        // rootfs + cache = 2 mounts
        assert_eq!(exec.mounts.len(), 2);
        // 1 secret env
        assert_eq!(exec.secretenv.len(), 1);
    }

    #[test]
    fn test_exec_empty_env() {
        let base = image("node:20");
        let output = ExecBuilder::new(base, vec!["echo".into()]).root();

        let (_op, exec) = decode_exec_op(&output);
        let meta = exec.meta.unwrap();
        assert!(meta.env.is_empty());
    }
}
