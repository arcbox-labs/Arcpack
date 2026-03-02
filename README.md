# Arcpack

Zero-configuration application builder — automatically detects source code language/framework → generates a build plan → transpiles to LLB → outputs OCI images via BuildKit, no Dockerfile required.

Core build component of the ArcBox PaaS platform.

## Core Architecture

```
Source Code → Provider Detection → BuildPlan → LLB Transpilation → BuildKit Build → OCI Image
```

- **Source Analyzer** — Reads source directories, provides filesystem abstraction (glob cache + JSONC parsing)
- **Provider** — Language/framework detectors, multiple can match simultaneously (detect → initialize → plan → cleanse_plan)
- **BuildPlan** — Aggregates all Provider outputs into a build blueprint (Step DAG + Layer + Filter + Cache + Command)
- **LLB Generator** — Transpiles BuildPlan into a BuildKit DAG, implementing OCI layering strategy
- **BuildKit Client** — gRPC communication (Session + FileSend + Secrets), supports subprocess/external daemon

## Quick Start

```bash
# Generate build plan (preview)
arcpack plan /path/to/your/app

# Full build → OCI image
arcpack build /path/to/your/app

# View build metadata
arcpack info /path/to/your/app

# Output arcpack.json JSON Schema
arcpack schema
```

## Roadmap

### Core Pipeline

| Status | Feature | Description |
|--------|---------|-------------|
| ✅ | Source Analyzer | App filesystem abstraction + glob cache + JSONC parsing |
| ✅ | Provider Framework | detect → initialize → plan → cleanse_plan lifecycle |
| ✅ | BuildPlan Data Structure | Step DAG, Layer, Filter, Cache, Command |
| ✅ | Plan Validation | commands / step-inputs / deploy-base / start-command |
| ✅ | DAG Topological Sort | Transitive dependency elimination |
| ✅ | LLB Primitives | exec / file / merge / source / terminal |
| ✅ | BuildPlan → LLB Conversion | Direct conversion path |
| ✅ | BuildPlan → Dockerfile | Compatibility path |
| ✅ | BuildKit gRPC Client | Session + FileSend + Secrets |
| ✅ | buildkitd Subprocess Management | SubprocessDaemonManager |
| ✅ | External buildkitd Connection | ExternalDaemonManager via BUILDKIT_HOST |

### CLI Commands

| Status | Command | Description |
|--------|---------|-------------|
| ✅ | `arcpack plan` | Generate BuildPlan JSON |
| ✅ | `arcpack build` | Full build → OCI image |
| ✅ | `arcpack info` | Build metadata output |
| ✅ | `arcpack schema` | arcpack.json JSON Schema |
| ✅ | `arcpack prepare` | Write plan + info JSON files |
| ✅ | `arcpack frontend` | BuildKit frontend mode |

### Providers (15)

| Status | Provider | Description |
|--------|----------|-------------|
| ✅ | Node.js | npm/pnpm/yarn/bun + 9 frameworks + SPA + workspace + Corepack |
| ✅ | Python | pip/uv/poetry/pdm/pipenv + Django/FastAPI/Flask/FastHTML |
| ✅ | Go | go modules + workspace + CGO detection + Gin metadata |
| ✅ | Rust | Cargo + workspace + WASM detection + 7-level version resolution |
| ✅ | Java | Maven/Gradle + Spring Boot + wrapper support |
| ✅ | PHP | Composer + Laravel + FrankenPHP + PHP extension detection + Node.js dual-language build |
| ✅ | Ruby | Bundler + Rails + YJIT + Node.js/ExecJS integration |
| ✅ | Elixir | Mix/Hex + Phoenix + Erlang version compatibility mapping + Node.js assets |
| ✅ | Gleam | gleam.toml + Erlang shipment |
| ✅ | Deno | deno.json/deno.jsonc + entrypoint detection |
| ✅ | .NET | NuGet + dotnet publish + multi-target framework |
| ✅ | C++ | CMake/Meson + Ninja |
| ✅ | Static Sites | Staticfile/public/index.html + Caddy |
| ✅ | Shell | shebang parsing + multi-shell support |
| ✅ | Procfile | Post-processor, web > worker > first |

### Configuration System

| Status | Feature | Description |
|--------|---------|-------------|
| ✅ | arcpack.json | Project configuration file |
| ✅ | Environment Variable Overrides | ARCPACK_* prefix |
| ✅ | JSON Schema Generation | schemars |
| ✅ | .dockerignore Support | Build context filtering |
| ✅ | Secrets Management | SHA256 hash + GITHUB_TOKEN auto-injection |

### Tool Integration

| Status | Feature | Description |
|--------|---------|-------------|
| ✅ | mise | Version manager integration |
| ✅ | Caddy | Web server (SPA + static sites) |
| ✅ | BuildKit Cache | cache-import / cache-export / cache-key |

### Testing

| Status | Feature | Description |
|--------|---------|-------------|
| ✅ | Unit Tests | 80+ `#[cfg(test)]` modules |
| ✅ | insta Snapshot Tests | 32 fixture snapshots |
| ✅ | Integration Test Framework | tests/ directory |
| 🚧 | Integration Test Coverage | Only 8 Node.js fixtures have test.json (33 fixtures total) |

### Pending Features (P0 Alignment Items)

| Status | ID | Description |
|--------|------|-------------|
| ✅ | P0-01 | Frontend plan-file read path (read plan file when filename present, fallback to detection otherwise) |
| ⬜ | P0-02 | docker-container:// BuildKit connection protocol |
| ✅ | P0-03a | Default image naming (derived from source directory name) |
| ⬜ | P0-03b | docker load export |
| 🚧 | P0-05 | CLI semantic alignment: --env bare KEY, --error-missing-start flag |
| 🚧 | P0-06 | ARCPACK_CONFIG_FILE integrated into config priority, railpack.json compatibility pending |
| ⬜ | P0-08 | Regression test reinforcement: expand fixtures to railpack-level 104 |
| ⬜ | P0-09 | CI/CD (GitHub Actions workflow) |

### Potential Improvements

| Status | Description |
|--------|-------------|
| ⬜ | C++ binary name parsing from CMakeLists.txt (currently uses directory name heuristic) |
| ⬜ | Gleam version pinning (currently latest) |
| ⬜ | More Node.js fixtures (turborepo/prisma/puppeteer, etc.) |
| ⬜ | Python fixture additions (fastapi/flask/pdm, etc.) |
| ⬜ | TTY progress display (currently plain text only) |

## Tech Stack

- **Language**: Rust
- **Build Tool**: BuildKit (buildkitd + buildctl)
- **Communication**: gRPC (Unix Socket / TCP)
- **Key Crates**: tokio (async), tonic (gRPC), clap (CLI), serde (serialization)

## License

Private — ArcBox internal project
