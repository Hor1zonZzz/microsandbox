<div align="center">

# @hor1zonz/microsandbox — Cloud Adapter Fork

<a href="https://www.npmjs.com/package/@hor1zonz/microsandbox"><img src="https://img.shields.io/badge/%40hor1zonz%2Fmicrosandbox-v0.0.1-CB3837?style=for-the-badge&logo=npm" alt="npm @hor1zonz/microsandbox v0.0.1"></a>
<a href="https://github.com/superradcompany/microsandbox"><img src="https://img.shields.io/badge/fork%20of-superradcompany%2Fmicrosandbox-A770EF?style=for-the-badge&logo=github" alt="fork of superradcompany/microsandbox"></a>
<a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache%202.0-blue.svg?style=for-the-badge" alt="Apache 2.0 License"></a>

**A fork of [`superradcompany/microsandbox`](https://github.com/superradcompany/microsandbox) that adds a self-hostable cloud API gateway (`msb-cloud-adapter`) and a fully-wired cloud backend for the SDKs — so the same SDK code can talk to a remote (or your own) microsandbox host over HTTP, not just spawn local microVMs.**

</div>

> [!NOTE]
> This is the **fork-specific section**. The complete, unmodified **upstream README is preserved below** → [jump to the original README](#-upstream-readme).

## <img height="18" src="https://octicons-col.vercel.app/git-branch/A770EF" alt="fork">&nbsp;&nbsp;What this fork adds

Upstream microsandbox boots microVMs as **local** child processes. This fork keeps all of that and adds a network-addressable "cloud" path on top of it:

| Addition | Where | What it does |
| --- | --- | --- |
| **`msb-cloud-adapter` service** | [`crates/cloud-adapter`](./crates/cloud-adapter) | An Axum HTTP + WebSocket server that exposes the `msb-cloud` REST/WS contract on top of a **local** microsandbox runtime. Run it on any machine that can boot microVMs and you have your own self-hosted "cloud". |
| **Cloud backend in the Rust SDK** | [`sdk/rust/lib/backend/cloud.rs`](./sdk/rust/lib/backend/cloud.rs) | `CloudBackend` (`url` + `api_key`, `from_env`, named profiles) wired through the full sandbox / volume / fs / exec / logs / metrics surface. |
| **Cloud backend in the Node/TS SDK** | [`sdk/node-ts`](./sdk/node-ts) | `setDefaultBackend` / `withDefaultBackend` / `defaultBackendKind`, a `CloudHttpError` type, and CBOR exec-over-WebSocket support. |
| **Published npm package** | `@hor1zonz/microsandbox` | The Node SDK is published under this scope (currently **darwin-arm64 / Apple Silicon** prebuilt). |
| **Cloud ephemeral-stop cleanup** | SDK + adapter | Correctly tears down ephemeral sandboxes on `stop()` when running against the cloud backend. |

The cloud backend is **API-compatible** with the local one: `Sandbox.builder(...).create()`, `exec`, `fs.*`, `logs`, `metrics`, and `Volume` all behave the same — only the transport changes.

## <img height="18" src="https://octicons-col.vercel.app/server/A770EF" alt="architecture">&nbsp;&nbsp;Architecture

```
  your app (Rust / TypeScript SDK)
            │  HTTP + WebSocket (Bearer API key)
            ▼
  ┌───────────────────────────┐
  │   msb-cloud-adapter        │   serves the /v1 msb-cloud contract
  │   (crates/cloud-adapter)   │
  └────────────┬──────────────┘
               │  in-process LocalBackend
               ▼
        microsandbox runtime  ──►  microVMs (libkrun)
```

The adapter boots real microVMs through the local runtime, so it must run on a host that can do so: **macOS (Apple Silicon)** or **Linux with KVM enabled**.

## <img height="18" src="https://octicons-col.vercel.app/rocket/A770EF" alt="quickstart">&nbsp;&nbsp;Quick start: run your own cloud

#### 1.&nbsp;&nbsp;Build & run the adapter

```sh
# Requires the msb runtime + libkrunfw under ~/.microsandbox (MSB_HOME).
# Install the upstream CLI once if you don't have them:
#   curl -fsSL https://install.microsandbox.dev | sh

export MSB_CLOUD_ADAPTER_API_KEY="choose-a-strong-key"
cargo run -p msb-cloud-adapter --release -- --api-key "$MSB_CLOUD_ADAPTER_API_KEY"
# Listening on http://127.0.0.1:8088   (override with --bind / MSB_CLOUD_ADAPTER_BIND)
# Health check:  curl http://127.0.0.1:8088/healthz
```

**Adapter configuration:**

| Flag | Env var | Default | Description |
| --- | --- | --- | --- |
| `--bind` | `MSB_CLOUD_ADAPTER_BIND` | `127.0.0.1:8088` | Socket address to listen on. |
| `--api-key` | `MSB_CLOUD_ADAPTER_API_KEY` | _(required)_ | Bearer key every SDK request must present. |
| — | `MSB_HOME` | `~/.microsandbox` | Where the `msb` binary + `libkrunfw` live. |

#### 2.&nbsp;&nbsp;Point an SDK at it

Both SDKs read `MSB_API_URL` + `MSB_API_KEY` (or use a named `MSB_PROFILE`):

```sh
export MSB_API_URL="http://127.0.0.1:8088"
export MSB_API_KEY="choose-a-strong-key"   # must match the adapter's key
```

<details open>
<summary><b>&nbsp;TypeScript (npm) →</b></summary>

```sh
npm i @hor1zonz/microsandbox
```

```typescript
import { Sandbox, setDefaultBackend } from "@hor1zonz/microsandbox";

// Route all SDK calls to the cloud adapter instead of spawning local microVMs.
setDefaultBackend({
  kind: "cloud",
  url: process.env.MSB_API_URL!,
  apiKey: process.env.MSB_API_KEY!,
});

await using sandbox = await Sandbox.builder("hello-cloud")
  .image("alpine:3.19")
  .cpus(1)
  .memory(512)
  .create();

const output = await sandbox.shell("uname -m && echo 'hello from the cloud adapter'");
console.log(output.stdout());
```

</details>

<details>
<summary><b>&nbsp;Rust →</b></summary>

```rust
use microsandbox::{CloudBackend, Sandbox, set_default_backend};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Reads MSB_API_URL + MSB_API_KEY from the environment.
    set_default_backend(CloudBackend::from_env()?);

    let sandbox = Sandbox::builder("hello-cloud")
        .image("alpine:3.19")
        .cpus(1)
        .memory(512)
        .create()
        .await?;

    let output = sandbox.shell("uname -m && echo 'hello from the cloud adapter'").await?;
    print!("{}", output.stdout()?);

    sandbox.stop().await?;
    Ok(())
}
```

</details>

> Runnable end-to-end examples live in [`examples/typescript/cloud-backend`](./examples/typescript/cloud-backend) and [`examples/rust/cloud-backend`](./examples/rust/cloud-backend).

## <img height="18" src="https://octicons-col.vercel.app/plug/A770EF" alt="api">&nbsp;&nbsp;Cloud HTTP API surface

All routes are under `/v1` and require an `Authorization: Bearer <api-key>` header.

- **Sandboxes** — create / list / get / start / stop / kill / drain / destroy
- **Filesystem** — `fs/read`, `fs/write`, `fs/list`, `fs/stat`, `fs/mkdir`, `fs/copy`, `fs/rename`, `fs/exists`, delete
- **Exec** — `exec.cbor` over WebSocket (`msb.cbor` subprotocol)
- **Logs** — Server-Sent Events stream (`/logs`)
- **Metrics** — live CPU / memory / network (`/metrics`)
- **Volumes** — create / list / get / remove, plus the same `fs/*` operations

See [`crates/cloud-adapter/bin/main.rs`](./crates/cloud-adapter/bin/main.rs) for the full route table.

## <img height="18" src="https://octicons-col.vercel.app/alert/A770EF" alt="notes">&nbsp;&nbsp;Notes & compatibility

- **Prebuilt platform:** the published `@hor1zonz/microsandbox` ships a prebuilt native binary for **macOS Apple Silicon (darwin-arm64)** only. Other platforms must build the SDK from source.
- **Relationship to upstream:** this fork tracks [`superradcompany/microsandbox`](https://github.com/superradcompany/microsandbox) and only *adds* the cloud adapter + cloud backend wiring. The local-microVM workflow documented below is unchanged.
- **License:** unchanged — [Apache 2.0](./LICENSE).

<br />

---

<div align="center"><a id="-upstream-readme"></a><sub><b>↓ &nbsp; Everything below is the original upstream README, preserved unchanged. &nbsp; ↓</b></sub></div>

---

<br />

<div align="center">
    <a href="./#gh-dark-mode-only" target="_blank" align="center">
        <img width="35%" src="./assets/microsandbox-gh-banner-dark.png" alt="microsandbox-banner-xl-dark">
    </a>
</div>

<div align="center">
    <a href="./#gh-light-mode-only" target="_blank">
        <img width="35%" src="./assets/microsandbox-gh-banner-light.png" alt="microsandbox-banner-xl">
    </a>
</div>

<br />

<div align="center"><b>——&nbsp;&nbsp;&nbsp;easy, fast, local microVMs for untrusted workloads&nbsp;&nbsp;&nbsp;——</b></div>

<br />
<br />

<div align='center'>
  <a href="https://github.com/superradcompany/microsandbox/releases"><img src="https://img.shields.io/github/v/release/superradcompany/microsandbox?include_prereleases&style=for-the-badge" alt="GitHub release"></a>
  <a href="https://discord.gg/T95Y3XnEAK"><img src="https://img.shields.io/discord/1315784565562019870?label=Discord&logo=discord&logoColor=white&color=5865F2&style=for-the-badge" alt="Discord"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache 2.0-blue.svg?style=for-the-badge" alt="Apache 2.0 License"></a>
</div>

<br />

**Microsandbox** runs **untrusted workloads** inside fast, local microVMs: AI agents, user code, plugins, CI jobs, dev environments, scrapers, and automation.

##

- <img height="14" src="https://octicons-col.vercel.app/shield-lock/A770EF"> **Hardware Isolation**: Hardware-level isolation with microVM technology.
- <img height="14" src="https://octicons-col.vercel.app/globe/A770EF"> **Cross Platform**: Runs on Linux, macOS, and Windows.
- <img height="14" src="https://octicons-col.vercel.app/package/A770EF"> **OCI Compatible**: Runs standard container images from Docker Hub, GHCR, or any OCI registry.
- <img height="14" src="https://octicons-col.vercel.app/container/A770EF"> **Docker-Like Workflows**: Familiar image, command, shell, and volume workflows.
- <img height="14" src="https://octicons-col.vercel.app/zap/A770EF"> **Instant Startup**: Average boot times[^boot-time] under 100 milliseconds.
- <img height="14" src="https://octicons-col.vercel.app/plug/A770EF"> **Embeddable**: Spawn VMs right within your code. No setup server. No long-running daemon.
- <img height="14" src="https://octicons-col.vercel.app/lock/A770EF"> **Secrets That Can't Leak**: Unexploitable secret keys that never enter the VM.
- <img height="14" src="https://octicons-col.vercel.app/database/A770EF"> **Long-Running**: Sandboxes can run in detached mode. Great for long-lived sessions.
- <img height="14" src="https://octicons-col.vercel.app/terminal/A770EF"> **Agent-Ready**: Your agents can create their own sandboxes with our [Agent Skills](https://github.com/superradcompany/skills) and [MCP server](https://github.com/superradcompany/microsandbox-mcp).

<br />

## <a href="./#gh-dark-mode-only" target="_blank"><img height="13" src="https://octicons-col.vercel.app/rocket/ffffff" alt="rocket-dark"></a><a href="./#gh-light-mode-only" target="_blank"><img height="13" src="https://octicons-col.vercel.app/rocket/000000" alt="rocket"></a>&nbsp;&nbsp;Getting Started

#### <img height="14" src="https://octicons-col.vercel.app/move-to-bottom/A770EF">&nbsp;&nbsp;Install the SDK

> ```sh
> cargo add microsandbox                                   # 🦀 Rust
> ```
>
> ```sh
> uv add microsandbox                                      # 🐍 Python
> ```
>
> ```sh
> npm i microsandbox                                       # 🟦 TypeScript
> ```
>
> ```sh
> go get github.com/superradcompany/microsandbox/sdk/go    # 🐹 Go
> ```

#### <img height="14" src="https://octicons-col.vercel.app/download/A770EF">&nbsp;&nbsp;Install the CLI

> Boot a microVM in a single command:
>
> ```sh
> npx microsandbox run debian
> ```
>
> ##
>
> Or install the `msb` command globally:
>
> ```sh
> curl -fsSL https://install.microsandbox.dev | sh        # 🍎 macOS / 🐧 Linux
> ```
>
> ```powershell
> irm https://install.microsandbox.dev/windows | iex      # 🪟 Windows
> ```
>
> <details>
> <summary><em>&nbsp;We also support other package managers  →</em></summary>
>
> ##
>
> ```sh
> brew install superradcompany/tap/microsandbox
> ```
>
> ```sh
> npm i -g microsandbox
> ```
>
> ```sh
> uv tool install microsandbox
> ```
>
> ```sh
> cargo install microsandbox
> ```
>
> </details>
>
> ##
>
> Then you can run `msb` directly:
>
> ```sh
> msb run debian
> ```

##

> **Requirements**:
>
> - <img height="14" src="https://api.iconify.design/simple-icons:apple.svg?color=%23A770EF" alt="macOS"> **macOS**: Apple Silicon.
> - <img height="14" src="https://api.iconify.design/simple-icons:linux.svg?color=%23A770EF" alt="Linux"> **Linux**: KVM enabled.
> - <img height="14" src="https://api.iconify.design/simple-icons:windows.svg?color=%23A770EF" alt="Windows"> **Windows**: WHP enabled.
>
> **Warning**: Microsandbox is still **beta software**. Expect breaking changes, missing features, and rough edges.

<br />

## <a href="./#gh-dark-mode-only" target="_blank"><img height="18" src="https://octicons-col.vercel.app/package-dependencies/ffffff" alt="sdk-dark"></a><a href="./#gh-light-mode-only" target="_blank"><img height="18" src="https://octicons-col.vercel.app/package-dependencies/000000" alt="sdk"></a>&nbsp;&nbsp;SDK

The SDK lets you create and control sandboxes directly from your application. `Sandbox::builder("...").create()` boots a microVM as a child process. No infrastructure required.

#### <img height="14" src="https://octicons-col.vercel.app/play/A770EF">&nbsp;&nbsp;Run Code in a Sandbox

> ```rs
> use microsandbox::Sandbox;
>
> #[tokio::main]
> async fn main() -> Result<(), Box<dyn std::error::Error>> {
>     let sandbox = Sandbox::builder("my-sandbox")
>         .image("python")
>         .cpus(1)
>         .memory(512)
>         .create()
>         .await?;
>
>     let output = sandbox
>         .exec("python", ["-c", "print('Hello from a microVM!')"])
>         .await?;
>
>     println!("{}", output.stdout()?);
>
>     sandbox.stop().await?;
>
>     Ok(())
> }
> ```
>
> <details>
> <summary><b>&nbsp;Python Example →</b></summary>
>
> ```python
> import asyncio
> from microsandbox import Sandbox
>
> async def main():
>     sandbox = await Sandbox.create(
>         "my-sandbox",
>         image="python",
>         cpus=1,
>         memory=512,
>     )
>
>     output = await sandbox.exec("python", ["-c", "print('Hello from a microVM!')"])
>
>     print(output.stdout_text)
>
>     await sandbox.stop()
>
> asyncio.run(main())
> ```
>
> </details>
>
> <details>
> <summary><b>&nbsp;TypeScript Example →</b></summary>
>
> ```typescript
> import { Sandbox } from "microsandbox";
>
> await using sandbox = await Sandbox.builder("my-sandbox")
>   .image("python")
>   .cpus(1)
>   .memory(512)
>   .create();
>
> const output = await sandbox.exec("python", [
>   "-c",
>   "print('Hello from a microVM!')",
> ]);
>
> console.log(output.stdout());
> ```
>
> </details>
>
> <details>
> <summary><b>&nbsp;Go Example →</b></summary>
>
> ```go
> package main
>
> import (
>     "context"
>     "fmt"
>     "log"
>
>     microsandbox "github.com/superradcompany/microsandbox/sdk/go"
> )
>
> func main() {
>     ctx := context.Background()
>
>     // Downloads the microsandbox runtime to ~/.microsandbox/ on first run.
>     if err := microsandbox.EnsureInstalled(ctx); err != nil {
>         log.Fatal(err)
>     }
>
>     sandbox, err := microsandbox.CreateSandbox(ctx, "my-sandbox",
>         microsandbox.WithImage("python"),
>         microsandbox.WithCPUs(1),
>         microsandbox.WithMemory(512),
>     )
>     if err != nil {
>         log.Fatal(err)
>     }
>     defer sandbox.Stop(ctx)
>
>     output, err := sandbox.Exec(ctx, "python", []string{"-c", "print('Hello from a microVM!')"})
>     if err != nil {
>         log.Fatal(err)
>     }
>
>     fmt.Println(output.Stdout())
> }
> ```
>
> </details>

> The first call to `create()` pulls the image if it isn't cached locally, so it may take longer depending on your connection. Subsequent runs reuse the cache.

<br />

<a href="https://docs.microsandbox.dev/sdk/overview"><img src="https://img.shields.io/badge/SDK_Docs-%E2%86%92-A770EF?style=flat-square&labelColor=2b2b2b" alt="SDK Docs"></a>

<br />

## <a href="./#gh-dark-mode-only" target="_blank"><img height="18" src="https://octicons-col.vercel.app/terminal/ffffff" alt="cli-dark"></a><a href="./#gh-light-mode-only" target="_blank"><img height="18" src="https://octicons-col.vercel.app/terminal/000000" alt="cli"></a>&nbsp;&nbsp;CLI

The `msb` CLI provides a complete interface for managing sandboxes, images, and volumes.

#### <img height="14" src="https://octicons-col.vercel.app/play/A770EF">&nbsp;&nbsp;Run a Command

> ```sh
> msb run python -- python3 -c "print('Hello from a microVM!')"
> ```

#### <img height="14" src="https://octicons-col.vercel.app/stopwatch/A770EF">&nbsp;&nbsp;Named Sandboxes

> ```sh
> # Create and start a named sandbox
> msb create --name app python
> ```
>
> ```sh
> # Execute commands
> msb exec app -- python -c "import this"
> msb exec app -- curl https://example.com
> ```
>
> ```sh
> # Lifecycle
> msb stop app
> msb start app
> msb rm app
> ```

#### <img height="14" src="https://octicons-col.vercel.app/cache/A770EF">&nbsp;&nbsp;Image Management

> ```sh
> msb pull python           # Pull an image
> msb image ls              # List cached images
> msb image rm python       # Remove an image
> ```

#### <img height="14" src="https://octicons-col.vercel.app/download/A770EF">&nbsp;&nbsp;Install & Uninstall Sandboxes

> ```sh
> msb install ubuntu               # Install ubuntu sandbox as 'ubuntu' command
> ubuntu                           # Opens Ubuntu in a microVM
> msb uninstall ubuntu             # Uninstall the ubuntu sandbox
> ```

#### <img height="14" src="https://octicons-col.vercel.app/list-unordered/A770EF">&nbsp;&nbsp;Status & Inspection

> ```sh
> msb ls                         # List all sandboxes
> msb ps app                     # Show sandbox status
> msb inspect app                # Detailed sandbox info
> msb metrics app                # Live CPU/memory/network stats
> ```

> [!TIP]
>
> Run:<br />
> · `msb --help` for quick help menu. <br />
> · `msb --tree` for complete command hierarchy and descriptions. <br />
> · `msb <command> --tree` for a specific command tree.

<br />

<a href="https://docs.microsandbox.dev/cli/overview"><img src="https://img.shields.io/badge/CLI_Docs-%E2%86%92-A770EF?style=flat-square&labelColor=2b2b2b" alt="CLI Docs"></a>

<br />

## <a href="./#gh-dark-mode-only" target="_blank"><img height="18" src="https://octicons-col.vercel.app/dependabot/ffffff" alt="agents-dark"></a><a href="./#gh-light-mode-only" target="_blank"><img height="18" src="https://octicons-col.vercel.app/dependabot/000000" alt="agents"></a>&nbsp;&nbsp;AI Agents

#### <img height="14" src="https://octicons-col.vercel.app/book/A770EF">&nbsp;&nbsp;Agent Skills

> Teach any AI coding agent how to use microsandbox by installing the [Agent Skills](https://github.com/superradcompany/skills). Works with Claude Code, Cursor, Codex, Gemini CLI, GitHub Copilot, and more.
>
> ```sh
> npx skills add superradcompany/skills
> ```

#### <img height="14" src="https://octicons-col.vercel.app/plug/A770EF">&nbsp;&nbsp;MCP Server

> Connect any MCP-compatible agent to microsandbox with the [MCP server](https://github.com/superradcompany/microsandbox-mcp). Provides structured tool calls for sandbox lifecycle, command execution, filesystem access, volumes, and monitoring.
>
> ```sh
> # Claude Code
> claude mcp add --transport stdio microsandbox -- npx -y microsandbox-mcp
> ```

<br />

## <a href="./#gh-dark-mode-only" target="_blank"><img height="18" src="https://octicons-col.vercel.app/book/ffffff" alt="docs-dark"></a><a href="./#gh-light-mode-only" target="_blank"><img height="18" src="https://octicons-col.vercel.app/book/000000" alt="docs"></a>&nbsp;&nbsp;Documentation

For guides, API references, and examples, visit the [microsandbox documentation](https://docs.microsandbox.dev).

<br />

## <a href="./#gh-dark-mode-only" target="_blank"><img height="18" src="https://octicons-col.vercel.app/gear/ffffff" alt="contributing-dark"></a><a href="./#gh-light-mode-only" target="_blank"><img height="18" src="https://octicons-col.vercel.app/gear/000000" alt="contributing"></a>&nbsp;&nbsp;Contributing

Interested in contributing to `microsandbox`? Check out our [CONTRIBUTING.md](./CONTRIBUTING.md) for guidelines and [DEVELOPMENT.md](./DEVELOPMENT.md) for build, test, and release instructions.

<br />

## <a href="./#gh-dark-mode-only" target="_blank"><img height="18" src="https://octicons-col.vercel.app/law/ffffff" alt="license-dark"></a><a href="./#gh-light-mode-only" target="_blank"><img height="18" src="https://octicons-col.vercel.app/law/000000" alt="license"></a>&nbsp;&nbsp;License

This project is licensed under the [Apache License 2.0](./LICENSE).

<br />

## <a href="./#gh-dark-mode-only" target="_blank"><img height="18" src="https://octicons-col.vercel.app/heart/ffffff" alt="acknowledgements-dark"></a><a href="./#gh-light-mode-only" target="_blank"><img height="18" src="https://octicons-col.vercel.app/heart/000000" alt="acknowledgements"></a>&nbsp;&nbsp;Acknowledgements

Special thanks to all our contributors, testers, and community members who help make microsandbox better every day! We'd like to thank the following projects and communities that made `microsandbox` possible: [libkrun](https://github.com/containers/libkrun) and [smoltcp](https://github.com/smoltcp-rs/smoltcp)

<br />

<div align='center'>
  <a href="https://www.ycombinator.com/"><img src="https://img.shields.io/badge/BACKED%20BY-Y%20COMBINATOR-F26522?style=for-the-badge&logo=ycombinator&logoColor=white" alt="Backed by Y Combinator"></a>
</div>

<br />
<br />

[^boot-time]: Boot time refers to guest boot on an M1 machine.
