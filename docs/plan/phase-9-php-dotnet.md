# Phase 9: PHP + .NET Provider

> [← 返回目录](./README.md) | 上一阶段：[← Phase 8](./phase-8-ruby-elixir.md) | 下一阶段：[Phase 10 →](./phase-10-build-infra.md)

**目标：** 新增 2 个高复杂度 Provider（PHP / .NET），包含 Laravel / FrankenPHP 特殊处理、.NET SDK/Runtime 分离 + 集成测试。

**前置条件：** Phase 6 T6.1（集成测试框架已就绪）+ Phase 5（Provider 框架已稳定）

## 任务依赖图

```
（以下任务可并行开发）

T9.1 (PHP Provider)    ──┐
T9.2 (.NET Provider)   ──┤──► T9.3 (注册表更新 + 测试)
                         ─┘
```

## 任务列表

### T9.1 PHP Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **railpack 参考** | `rp:core/providers/php/`（5 文件，约 800 行总计，HIGH 复杂度 — railpack 中最复杂的 Provider） |
| **依赖** | Phase 2 |

**描述：** PHP Provider 使用 FrankenPHP 基础镜像（而非 mise + debian），支持 Composer 包管理、PHP 扩展自动检测、Laravel 框架深度集成。这是唯一不使用 mise 安装运行时的 Provider。

**交付文件：**
- `src/provider/php/mod.rs` — PhpProvider 核心
- `src/provider/php/extensions.rs` — PHP 扩展自动检测
- `src/provider/php/laravel.rs` — Laravel 特殊逻辑
- `src/provider/php/templates/Caddyfile` — 嵌入资源（`include_str!`）
- `src/provider/php/templates/php.ini` — 嵌入资源
- `src/provider/php/templates/start-container.sh` — 嵌入资源

**检测逻辑（detect）：**
- `index.php` 存在 **或** `composer.json` 存在

**版本解析（initialize）：**
- 优先级：`ARCPACK_PHP_VERSION` 环境变量 → `composer.json` 的 `require.php` 约束解析 → 默认 `8.3`
- PHP 约束解析：`"php": "^8.1"` → 取最低版本 `8.1`
- FrankenPHP 镜像 tag 验证：HTTP HEAD 请求 Docker Hub API 检查 `dunglas/frankenphp:php{version}-bookworm` 是否存在

**核心设计差异（与其他 Provider 不同）：**

PHP 不使用 mise 安装运行时，而是使用 **FrankenPHP 基础镜像**：

```
基础镜像: dunglas/frankenphp:php{version}-bookworm
```

因此 plan 中不含 `packages:mise` 步骤，而是用 `ImageStepBuilder` 从 FrankenPHP 镜像获取 PHP + Caddy 运行时。

**构建计划（plan）：**

| Step | 名称 | 输入 | 行为 |
|------|------|------|------|
| php-base | `php:base` | — | `ImageStepBuilder` 使用 `dunglas/frankenphp:php{version}-bookworm` |
| composer | `composer:install` | php-base | 从 `composer:latest` 镜像 COPY `/usr/bin/composer` 二进制 |
| install | `install` | composer + local | `composer install --no-dev --optimize-autoloader --no-interaction` |
| build | `build` | install + local | Laravel: artisan 缓存命令；Node: npm build（若有） |
| apt:runtime | `packages:apt:runtime` | — | PHP 扩展依赖的运行时 APT 包 |

**PHP 扩展自动检测（extensions.rs）：**

从 `composer.json` 的 `require` 字段提取 `ext-` 前缀的扩展：

```json
{
  "require": {
    "ext-gd": "*",
    "ext-redis": "*",
    "ext-pdo_pgsql": "*"
  }
}
```

**扩展 → APT 包映射表（部分）：**

| 扩展 | 构建时 APT 包 | 安装命令 |
|------|-------------|---------|
| `gd` | `libpng-dev`, `libjpeg-dev`, `libfreetype-dev` | `docker-php-ext-configure gd && docker-php-ext-install gd` |
| `pdo_pgsql` | `libpq-dev` | `docker-php-ext-install pdo_pgsql` |
| `pdo_mysql` | — | `docker-php-ext-install pdo_mysql` |
| `redis` | — | `pecl install redis && docker-php-ext-enable redis` |
| `imagick` | `libmagickwand-dev` | `pecl install imagick && docker-php-ext-enable imagick` |
| `zip` | `libzip-dev` | `docker-php-ext-install zip` |
| `intl` | `libicu-dev` | `docker-php-ext-install intl` |
| `bcmath` | — | `docker-php-ext-install bcmath` |
| `soap` | `libxml2-dev` | `docker-php-ext-install soap` |
| `gmp` | `libgmp-dev` | `docker-php-ext-install gmp` |

**Laravel 必需扩展（无论 composer.json 是否声明）：**
- `pdo_mysql` 或 `pdo_pgsql`（根据 `.env` 的 `DB_CONNECTION` 检测）
- `bcmath`
- `ctype`
- `fileinfo`
- `mbstring`
- `openssl`
- `tokenizer`
- `xml`

**Laravel 特殊逻辑（laravel.rs）：**

检测条件：`artisan` 文件存在

Laravel 模式下的 build step 追加命令：
```bash
php artisan config:cache
php artisan event:cache
php artisan route:cache
php artisan view:cache
```

启动脚本（start-container.sh）追加：
```bash
# 可选 migration（可通过 ARCPACK_SKIP_MIGRATIONS 跳过）
if [ "$ARCPACK_SKIP_MIGRATIONS" != "true" ]; then
    php artisan migrate --force
fi
```

**双语言构建（PHP + Node.js）：**
- 若 `package.json` 存在
- 追加 Node.js 安装（mise 或从 node:alpine COPY）
- install step 追加 `npm install`（或 `pnpm install` / `yarn install`）
- build step 追加 `npm run build`（若有 build script）
- deploy inputs 包含 Node.js 构建产物

**嵌入模板文件：**

**Caddyfile：**
```caddyfile
{
    frankenphp
    order php_server before file_server
}

:3000 {
    root * {{PHP_ROOT_DIR}}
    encode zstd gzip

    php_server

    # 健康检查
    handle /health {
        respond "OK" 200
    }
}
```

**php.ini：**
```ini
[PHP]
memory_limit = 256M
upload_max_filesize = 50M
post_max_size = 50M
max_execution_time = 60
expose_php = Off
```

**start-container.sh：**
```bash
#!/bin/bash
set -e

# 用户自定义初始化脚本
if [ -f /app/docker-entrypoint.sh ]; then
    source /app/docker-entrypoint.sh
fi

# Laravel migrations
if [ -f /app/artisan ] && [ "$ARCPACK_SKIP_MIGRATIONS" != "true" ]; then
    php artisan migrate --force 2>/dev/null || true
fi

# 启动 FrankenPHP
exec frankenphp run --config /app/Caddyfile --adapter caddyfile
```

用户可通过项目根目录放置同名文件覆盖上述任何模板。

**Deploy 配置：**
- `start_cmd`: `/app/start-container.sh`（或 `frankenphp run --config /app/Caddyfile --adapter caddyfile`）
- deploy inputs: php-base 层 + composer 层 + 应用代码 + 模板文件
- 环境变量：`APP_ENV=production`、`COMPOSER_ALLOW_SUPERUSER=1`

**环境变量：**
- `ARCPACK_PHP_ROOT_DIR`: 覆盖 webroot（默认 `/app/public`）
- `ARCPACK_SKIP_MIGRATIONS`: 跳过 Laravel migrations
- `ARCPACK_PHP_VERSION`: 覆盖 PHP 版本

**缓存：**
- `composer-cache` → `/root/.composer/cache`（shared）

**Secrets 前缀：** `COMPOSER`、`PHP`、`APP_KEY`、`APP_ENV`、`DB_`、`REDIS_`、`MAIL_`、`AWS_`

**Metadata：** `phpVersion`、`isLaravel`、`hasNodeIntegration`、`phpExtensions`（数组）

**start_command_help()：**
```
PHP project detected. Ensure index.php or public/index.php exists as the entry point.
```

**测试 fixture：**
- `tests/fixtures/php-basic/`
  - `index.php`: `<?php echo "Hello from PHP";`
  - `composer.json`: `{"require": {"php": "^8.1"}}`
  - `test.json`: `[{"httpCheck": {"path": "/", "expected": 200, "expectedOutput": "Hello from PHP"}}]`
- `tests/fixtures/php-laravel/`
  - 最简 Laravel 项目骨架
  - `artisan` 文件
  - `test.json`: `[{"httpCheck": {"path": "/", "expected": 200}}]`
  - `docker-compose.yml`（MySQL）

**测试要求：**
- detect 对含/无 index.php、composer.json 的测试
- PHP 版本约束解析测试（`^8.1`→`8.1`、`>=8.0`→`8.0`、`8.2.*`→`8.2`）
- FrankenPHP 基础镜像选择测试（不使用 mise）
- Composer 二进制 COPY 步骤测试
- PHP 扩展自动检测测试（ext-gd、ext-redis 等）
- Laravel 检测（artisan 文件）
- Laravel artisan 缓存命令测试
- 双语言构建（PHP+Node）条件测试
- 模板文件嵌入正确性测试
- 用户自定义模板覆盖优先级测试
- `ARCPACK_PHP_ROOT_DIR` 覆盖测试
- `ARCPACK_SKIP_MIGRATIONS` 跳过 migration 测试
- 快照测试

---

### T9.2 .NET Provider

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **railpack 参考** | `rp:core/providers/dotnet/dotnet.go`（1 文件，约 300 行，MEDIUM 复杂度） |
| **依赖** | Phase 2 |

**描述：** 支持 .NET 项目的检测和构建，包括 SDK/Runtime 分离、csproj XML 解析、NuGet 包还原。

**交付文件：** `src/provider/dotnet.rs`

**检测逻辑（detect）：**
- `*.csproj` glob 匹配（任意 `.csproj` 文件存在）
- 或 `*.fsproj` glob 匹配（F# 项目）

**版本解析（initialize）：**

| 优先级 | 来源 | 解析方式 |
|--------|------|---------|
| 1 | `global.json` | JSON 解析 `sdk.version` 字段，取主版本号（`8.0.100` → `8.0`） |
| 2 | `.csproj` XML | 正则提取 `<TargetFramework>net(\d+\.\d+)</TargetFramework>` |
| 3 | `.csproj` XML | 正则提取 `<TargetFrameworks>` 中第一个 `net\d+\.\d+` |
| 4 | `ARCPACK_DOTNET_VERSION` 环境变量 | 直接使用 |
| 5 | 默认值 | `8.0` |

**csproj 项目名提取：**
- 默认使用 `.csproj` 文件名（去除 `.csproj` 后缀）作为项目名
- 如 `MyApp.csproj` → 项目名 `MyApp`
- 若有多个 `.csproj`，使用根目录的那个，或取第一个

**构建计划（plan）：**

| Step | 名称 | 输入 | 行为 |
|------|------|------|------|
| mise | `packages:mise` | — | 安装 `dotnet` SDK；APT 包 `libicu-dev` |
| install | `install` | mise | 复制 `nuget.config*`、`**/*.csproj`、`**/*.fsproj`、`global.json*`、`**/*.props`、`**/*.targets` → `dotnet restore` |
| build | `build` | install + local | `dotnet publish --no-restore -c Release -o out` |

**Install step 文件列表详述：**
- `nuget.config` / `NuGet.config` / `NuGet.Config`（多种大小写）
- `**/*.csproj` / `**/*.fsproj` — 项目文件
- `global.json` — SDK 版本锁定
- `Directory.Build.props` / `Directory.Build.targets` — MSBuild 共享属性
- `**/*.sln` — Solution 文件

**Deploy 配置：**
- `start_cmd`: `ASPNETCORE_URLS=http://0.0.0.0:${PORT:-3000} ./out/{project_name}`
- deploy inputs: mise 层（dotnet runtime）+ `out/` 目录
- APT 运行时包：`libicu-dev`（ICU 国际化库）
- 环境变量：
  - `ASPNETCORE_ENVIRONMENT=Production`
  - `DOTNET_CLI_TELEMETRY_OPTOUT=1`
  - `DOTNET_NOLOGO=1`
  - `DOTNET_ROOT=/mise/installs/dotnet/{version}`

**运行时 dotnet 说明：**
- 构建时需要完整 SDK（`dotnet` + MSBuild + NuGet）
- 运行时只需要 dotnet runtime
- mise 安装的 dotnet 包含完整 SDK，deploy 时通过 `DOTNET_ROOT` 指向 mise 安装路径
- 替代方案：若 `dotnet publish` 使用 `--self-contained`，可不需要运行时 dotnet

**缓存：**
- `dotnet-nuget` → `/root/.nuget/packages`（shared）

**Secrets 前缀：** `DOTNET`、`ASPNETCORE`、`NUGET`

**Metadata：** `dotnetVersion`、`dotnetProjectName`、`dotnetTargetFramework`

**start_command_help()：**
```
No .NET project found. Ensure a .csproj or .fsproj file exists in the project root.
```

**测试 fixture：**
- `tests/fixtures/dotnet-basic/`
  - `MyApp.csproj`:
    ```xml
    <Project Sdk="Microsoft.NET.Sdk.Web">
      <PropertyGroup>
        <TargetFramework>net8.0</TargetFramework>
      </PropertyGroup>
    </Project>
    ```
  - `Program.cs`:
    ```csharp
    var builder = WebApplication.CreateBuilder(args);
    var app = builder.Build();
    app.MapGet("/", () => "Hello from .NET");
    app.Run();
    ```
  - `test.json`: `[{"httpCheck": {"path": "/", "expected": 200, "expectedOutput": "Hello from .NET"}}]`

**测试要求：**
- detect 对含/无 .csproj、.fsproj 的测试
- global.json SDK 版本解析测试
- csproj TargetFramework 解析测试（单框架 + 多框架）
- `ARCPACK_DOTNET_VERSION` 环境变量覆盖测试
- 项目名提取测试（单 csproj、多 csproj）
- install step 文件列表完整性测试
- dotnet publish 命令正确性测试
- deploy start_cmd 路径正确性测试
- ASPNETCORE_URLS 端口配置测试
- 环境变量设置测试
- 快照测试

---

### T9.3 注册表更新 + 测试

| 字段 | 值 |
|------|---|
| **状态** | `pending` |
| **优先级** | P1 |
| **依赖** | T9.1 ~ T9.2 |

**描述：** 将 PHP / .NET 加入 `get_all_providers()` 注册表。完成后 arcpack 覆盖全部 14 个语言 Provider + Procfile。

**最终注册顺序（对齐 railpack）：**

```rust
pub fn get_all_providers() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(PhpProvider::new()),        // NEW — 最高优先级（railpack 第 1 位）
        Box::new(GoProvider::new()),
        Box::new(JavaProvider::new()),
        Box::new(RustProvider::new()),
        Box::new(RubyProvider::new()),
        Box::new(ElixirProvider::new()),
        Box::new(PythonProvider::new()),
        Box::new(DenoProvider::new()),
        Box::new(DotnetProvider::new()),     // NEW
        Box::new(NodeProvider::new()),
        Box::new(GleamProvider::new()),
        Box::new(CppProvider::new()),
        Box::new(StaticFileProvider::new()),
        Box::new(ShellProvider::new()),
    ]
    // 14 个语言 Provider + ProcfileProvider（后处理，不在检测列表中）
}
```

> **注册顺序说明：**
> - PHP 在最高优先级（railpack 中 PHP 排第 1）：因为 PHP 项目可能同时有 `package.json`（双语言构建），需优先匹配 PHP 而非 Node.js
> - .NET 在 Node.js 之前：`.csproj` 不会与 `package.json` 冲突
> - 最终列表完全对齐 railpack 的 Provider 注册顺序

**快照测试更新：**
- 新增 `php-basic`、`dotnet-basic` 两份快照

**全量 Provider 验证：**
```bash
# 确认 14 个 Provider 全部注册
for fixture in tests/fixtures/*/; do
  arcpack info "$fixture" 2>/dev/null | jq -r '.detectedProvider // "none"'
done
```

---

## 验证清单

Phase 9 完成后：

```bash
cargo check                                            # 编译无错误
cargo test                                             # 全部单元测试通过
cargo test -- snapshot                                 # 快照测试通过
cargo insta review                                     # 审查新增快照
cargo test --test integration_tests -- --ignored php   # PHP 集成测试
cargo test --test integration_tests -- --ignored dotnet # .NET 集成测试
```

**终极验证（Phase 9 完成 = 全部 14 Provider 就绪）：**
```bash
# 对每种语言生成 BuildPlan
for fixture in tests/fixtures/*/; do
  echo "=== $(basename $fixture) ==="
  arcpack plan "$fixture" | jq '.steps | length'
done

# 全部集成测试
cargo test --test integration_tests -- --ignored
```
