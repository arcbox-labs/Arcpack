# Phase 8: Ruby + Elixir Provider

> [← 返回目录](./README.md) | 上一阶段：[← Phase 7](./phase-7-lightweight-providers.md) | 下一阶段：[Phase 9 →](./phase-9-php-dotnet.md)

**目标：** 新增 2 个中复杂度 Provider（Ruby / Elixir），包含 Rails / Phoenix 框架检测 + 集成测试。

**前置条件：** Phase 6 T6.1（集成测试框架已就绪）+ Phase 5（Provider 框架已稳定）

## 任务依赖图

```
（以下任务可并行开发）

T8.1 (Ruby Provider)     ──┐
T8.2 (Elixir Provider)   ──┤──► T8.3 (注册表更新 + 测试)
                           ─┘
```

## 任务列表

### T8.1 Ruby Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **railpack 参考** | `rp:core/providers/ruby/ruby.go`（1 文件，约 550 行，MEDIUM 复杂度） |
| **依赖** | Phase 2 |

**描述：** 支持 Ruby 项目的完整构建流程，包括 Rails 检测、Bundler 版本管理、jemalloc、YJIT、原生 gem APT 依赖。

**交付文件：** `src/provider/ruby/mod.rs`

**检测逻辑（detect）：**
- `Gemfile` 存在

**版本解析优先级（initialize）：**

| 优先级 | 来源 | 解析方式 |
|--------|------|---------|
| 1 | `.ruby-version` 文件 | 读取文件内容，trim 后作为版本号 |
| 2 | `Gemfile` 中 `ruby 'X.X.X'` | 正则匹配 `ruby\s+['"](\d+\.\d+\.\d+)['"]` |
| 3 | `Gemfile.lock` RUBY VERSION 段 | 正则匹配 `RUBY VERSION\s+ruby (\d+\.\d+\.\d+)` |
| 4 | `RUBY_VERSION` 环境变量 | 直接使用 |
| 5 | mise `.tool-versions` | 通过 Resolver |
| 6 | 默认值 | `3.4.6` |

**Bundler 版本解析：**
- 从 `Gemfile.lock` 末尾 `BUNDLED WITH` 段提取版本号
- 正则：`BUNDLED WITH\s+(\d+\.\d+\.\d+)`
- 若无法解析，默认使用系统 bundler

**Rails 检测：**
- 检查 `config/application.rb` 文件是否包含 `Rails::Application`
- 或检查 `Gemfile` 中是否有 `gem 'rails'` / `gem "rails"` 依赖

**构建计划（plan）：**

| Step | 名称 | 输入 | 行为 |
|------|------|------|------|
| mise | `packages:mise` | — | 安装 `ruby`（指定版本）+ `bundler`（指定版本）；APT 构建包 |
| install | `install` | mise | `bundle config set deployment true` → `bundle config set without 'development test'` → `bundle install` |
| build | `build` | install + local | Rails: asset pipeline + bootsnap 预编译；非 Rails: 无额外步骤 |
| apt:runtime | `packages:apt:runtime` | — | 运行时 APT 包 |

**关键特性详述：**

**1. 本地 gem 路径依赖：**
- 解析 `Gemfile.lock` 中 `PATH` 段的 `remote: ./some/gem`
- 这些本地 gem 路径需要在 install step 中额外复制到构建上下文
- 正则：`PATH\s+remote: (\.\/[^\s]+)`

**2. Asset pipeline 检测：**
- 检测 `Gemfile` 中是否有 `sprockets` 或 `propshaft` gem 依赖
- 若有，build step 追加 `bundle exec rake assets:precompile`
- 注意：需先检查 `bin/rails` 是否存在

**3. Bootsnap 预编译：**
- 检测 `Gemfile` 中是否有 `bootsnap` gem
- 若有，build step 追加 `bundle exec bootsnap precompile --gemfile app/ lib/`

**4. 原生 gem APT 依赖（构建时 + 运行时）：**

| Gem | 构建时 APT 包 | 运行时 APT 包 |
|-----|-------------|-------------|
| `pg` | `libpq-dev` | `libpq5` |
| `mysql2` | `default-libmysqlclient-dev` | `default-mysql-client` |
| `rmagick` / `mini_magick` | `libmagickwand-dev` | `libmagickwand-6.q16-6` |
| `ruby-vips` / `image_processing` | `libvips-dev` | `libvips42` |
| `sqlite3` | `libsqlite3-dev` | `libsqlite3-0` |
| `nokogiri` | `pkg-config`, `libxml2-dev`, `libxslt-dev` | `libxml2`, `libxslt1.1` |
| `ffi` | `libffi-dev` | `libffi8` |

检测方式：解析 `Gemfile.lock` 的 `GEM` 段中的 gem 名称列表。

**5. jemalloc（始终安装）：**
- 构建时 APT 包：`libjemalloc-dev`
- 运行时 APT 包：`libjemalloc2`
- 环境变量：`LD_PRELOAD=/usr/lib/x86_64-linux-gnu/libjemalloc.so.2`
- 环境变量：`MALLOC_ARENA_MAX=2`

**6. Ruby 3.1+ YJIT 支持：**
- 检测 Ruby 版本 ≥ 3.1
- mise step 额外安装 `rustc` + `cargo`（YJIT 需要 Rust 构建依赖）
- 设置环境变量 `RUBY_YJIT_ENABLE=true`

**7. 可选 Node.js 集成：**
- 若 `package.json` 存在 **或** `Gemfile` 含 `execjs` gem
- 追加 Node.js mise 安装（`node` 默认版本）
- 追加 `yarn install` 或 `npm install`（若 `yarn.lock` 存在则 yarn，否则 npm）

**Deploy 配置：**
- `start_cmd` 优先级：
  1. Rails 项目：`bin/rails server -b 0.0.0.0 -p ${PORT:-3000}`
  2. `config.ru` 存在：`bundle exec rackup config.ru -p ${PORT:-3000}`
  3. 其他：`bundle exec ruby {main_file}`（或为空，触发 start_command_help）
- deploy inputs: mise 层 + vendor/bundle + 应用代码
- 环境变量：`RAILS_ENV=production`、`RACK_ENV=production`、`BUNDLE_WITHOUT=development:test`

**缓存：**
- `ruby-bundle` → `/app/vendor/bundle`（shared）

**Secrets 前缀：** `RUBY`、`GEM`、`BUNDLE`（install）；`RAILS`、`BUNDLE`、`BOOTSNAP`（build）

**Metadata：** `rubyVersion`、`bundlerVersion`、`isRails`、`hasAssetPipeline`、`hasBootsnap`、`hasNodeIntegration`

**start_command_help()：**
```
No start command found. For Rails apps, ensure bin/rails exists. For Rack apps, add a config.ru. Otherwise, set the start command in arcpack.json.
```

**测试 fixture：**
- `tests/fixtures/ruby-basic/`
  - `Gemfile`: 基础 Ruby 项目
  - `Gemfile.lock`
  - `app.rb`: `puts "Hello from Ruby"`
  - `test.json`: `[{"expectedOutput": "Hello from Ruby"}]`
- `tests/fixtures/ruby-rails/`
  - 最简 Rails 项目骨架
  - `test.json`: `[{"httpCheck": {"path": "/", "expected": 200}}]`
  - `docker-compose.yml`（PostgreSQL）

**测试要求：**
- detect 对含/无 Gemfile 的测试
- Ruby 版本解析 6 级优先级覆盖测试
- Bundler 版本从 Gemfile.lock 提取测试
- Rails 检测（config/application.rb 方式 + Gemfile 方式）
- 本地 gem 路径解析测试
- Asset pipeline 检测 → rake assets:precompile 添加测试
- Bootsnap 检测 → 预编译命令添加测试
- 原生 gem APT 依赖检测测试（至少覆盖 pg、mysql2、nokogiri）
- jemalloc 相关环境变量设置测试
- YJIT 版本条件检测测试（3.0 不启用、3.1+ 启用）
- Node.js 集成条件测试
- start_cmd 三级优先级测试
- 快照测试

---

### T8.2 Elixir Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **railpack 参考** | `rp:core/providers/elixir/elixir.go`（1 文件，约 400 行，MEDIUM 复杂度） |
| **依赖** | Phase 2 |

**描述：** 支持 Elixir 项目的完整构建流程，包括 Phoenix 检测、Erlang OTP 版本联动、mix release 部署。

**交付文件：** `src/provider/elixir.rs`

**检测逻辑（detect）：**
- `mix.exs` 存在

**版本解析（initialize）：**

| 工具 | 优先级 |
|------|--------|
| Elixir | `.elixir-version` 文件 → `mix.exs` 正则 `elixir: "~> X.Y"` → `ELIXIR_VERSION` 环境变量 → 默认 `1.18` |
| Erlang | `.erlang-version` 文件 → `ERLANG_VERSION` 环境变量 → OTP 兼容矩阵推导 → 默认 `27` |

**Elixir 版本 OTP 后缀解析：**
- `.elixir-version` 可能包含 OTP 后缀，如 `1.18.0-otp-27`
- 解析规则：`(\d+\.\d+\.\d+)(?:-otp-(\d+))?`
- 若有 OTP 后缀，同时设置 Erlang 版本

**内置 Elixir → Erlang OTP 兼容矩阵（部分）：**

| Elixir | 最低 OTP | 推荐 OTP |
|--------|---------|---------|
| 1.18.x | 27 | 27 |
| 1.17.x | 25 | 27 |
| 1.16.x | 24 | 26 |
| 1.15.x | 24 | 26 |
| 1.14.x | 23 | 25 |

**构建计划（plan）：**

| Step | 名称 | 输入 | 行为 |
|------|------|------|------|
| mise | `packages:mise` | — | 安装 `elixir` + `erlang`（版本联动） |
| install | `install` | mise | `mix local.hex --force` → `mix local.rebar --force` → `mix deps.get --only prod` → `mix deps.compile` |
| build | `build` | install + local | 条件命令 + `mix release` |

**Build step 条件命令（按顺序，存在则执行）：**

| 条件 | 命令 | 说明 |
|------|------|------|
| `mix.exs` 含 `phx_dep` 或 `phoenix` 依赖 + `assets/` 目录存在 | `mix assets.setup` | Phoenix asset 初始化 |
| 同上 | `mix assets.deploy` | Phoenix asset 编译 |
| 始终 | `mix release` | 编译 Erlang release |

**Phoenix 检测：**
- 检查 `mix.exs` 中是否有 `{:phoenix,` 或 `{:phoenix_live_view,` 依赖
- 正则：`\{:phoenix[_a-z]*,`

**Phoenix/Node 集成：**
- 若 `assets/package.json` 存在（Phoenix 前端资产）
- 作为 Node sub-app：在 mise step 中额外安装 `node`
- 在 build step 中先 `cd assets && npm install && cd ..` 再执行 mix 命令

**App name 提取：**
- 从 `mix.exs` 中正则提取 `app: :(\w+)`
- 用于 release 路径推导

**Deploy 配置：**
- `start_cmd`: `/app/_build/prod/rel/{app_name}/bin/{app_name} start`
- deploy inputs: mise 层（erlang）+ `_build/prod/rel/{app_name}/` 目录
- 环境变量：
  - `MIX_ENV=prod`
  - `LANG=en_US.UTF-8`
  - `MIX_HOME=/root/.mix`
  - `HEX_HOME=/root/.hex`
  - `SECRET_KEY_BASE` — 需通过 secrets 注入
  - `PHX_SERVER=true`（Phoenix 项目）
  - `PHX_HOST=localhost`（Phoenix 项目）
  - `PORT=3000`

**缓存：**
- `elixir-deps` → `/app/deps`（shared）
- `elixir-build` → `/app/_build`（shared）

**Secrets 前缀：** `MIX`、`HEX`、`ELIXIR`、`ERLANG`、`SECRET_KEY_BASE`、`DATABASE_URL`

**Metadata：** `elixirVersion`、`erlangVersion`、`isPhoenix`、`appName`

**start_command_help()：**
```
No Elixir release found. Ensure mix.exs defines a valid project with app name.
```

**测试 fixture：**
- `tests/fixtures/elixir-basic/`
  - `mix.exs`: 基础 Elixir 项目
  - `mix.lock`
  - `lib/hello.ex`: `IO.puts "Hello from Elixir"`
  - `test.json`: `[{"expectedOutput": "Hello from Elixir"}]`
- `tests/fixtures/elixir-phoenix/`
  - 最简 Phoenix 项目骨架
  - `test.json`: `[{"httpCheck": {"path": "/", "expected": 200}}]`

**测试要求：**
- detect 对含/无 mix.exs 的测试
- Elixir 版本解析测试（.elixir-version、mix.exs 正则、环境变量）
- OTP 后缀解析测试（`1.18.0-otp-27`）
- Erlang 版本联动测试（OTP 兼容矩阵）
- Phoenix 检测测试
- app name 提取测试
- 条件命令执行顺序测试（assets.setup → assets.deploy → release）
- Phoenix/Node 集成条件测试
- deploy start_cmd 路径正确性测试
- 环境变量设置测试（MIX_ENV、LANG 等）
- 快照测试

---

### T8.3 注册表更新 + 测试

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **依赖** | T8.1 ~ T8.2 |

**描述：** 将 Ruby / Elixir 加入 `get_all_providers()` 注册表，更新全量快照测试。

**注册顺序更新（对齐 railpack）：**

```rust
pub fn get_all_providers() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(GoProvider::new()),
        Box::new(JavaProvider::new()),
        Box::new(RustProvider::new()),
        Box::new(RubyProvider::new()),       // NEW
        Box::new(ElixirProvider::new()),     // NEW
        Box::new(PythonProvider::new()),
        Box::new(DenoProvider::new()),
        Box::new(NodeProvider::new()),
        Box::new(GleamProvider::new()),
        Box::new(CppProvider::new()),
        Box::new(StaticFileProvider::new()),
        Box::new(ShellProvider::new()),
    ]
}
```

> **注册顺序说明：** Ruby 在 Python 之前（若项目同时有 Gemfile 和 requirements.txt，优先检测为 Ruby）。Elixir 在 Python 之后 Ruby 之后（mix.exs 不会与其他语言冲突）。

**快照测试更新：**
- 新增 `ruby-basic`、`elixir-basic` 两份快照

**验证：**
```bash
cargo check
cargo test
cargo test -- snapshot
cargo insta review
cargo test --test integration_tests -- --ignored ruby
cargo test --test integration_tests -- --ignored elixir
```

---

## 验证清单

Phase 8 完成后：

```bash
cargo check                                              # 编译无错误
cargo test                                               # 全部单元测试通过
cargo test -- snapshot                                   # 快照测试通过
cargo insta review                                       # 审查新增快照
cargo test --test integration_tests -- --ignored ruby    # Ruby 集成测试
cargo test --test integration_tests -- --ignored elixir  # Elixir 集成测试
```
