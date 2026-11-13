# kunianki

## Rust

- `Cargo.toml` `[lints.clippy]` denies `unwrap_used`, `expect_used`, and `dbg_macro`: propagate errors with `Result` and `?` instead of panicking.
- Use the `mcpls` MCP server for compiler-backed types, references, and diagnostics instead of guessing from text search.
- Follow the `rust-best-practices` skill when writing Rust and the `rust-review` skill when reviewing it.

## Slint

This is a [Slint](https://slint.dev/) desktop app written in Rust: the UI lives in `ui/app-window.slint` (compiled by `build.rs`) and the logic in `src/main.rs`.

- Check `Cargo.toml` and `Cargo.lock` before relying on skill advice. The bundled `slint` skill targets Slint 1.17 and newer; for exact APIs, prefer the docs for the version pinned in this project (the hosted `slint-docs` MCP searches and fetches them).
- Give components explicit dimensions when their preferred size is zero.
- Compile a `.slint` file and print diagnostics with `slint-viewer --check ui/app-window.slint`; render an isolated layout with `slint-viewer --screenshot out.png ui/app-window.slint` and inspect the image. Install the viewer matching the generated Slint version:

  ```sh
  SLINT_VERSION=$(awk '/^name = "slint"$/ { getline; gsub(/"/, "", $3); print $3; exit }' Cargo.lock)
  cargo install slint-viewer --version "$SLINT_VERSION" --locked
  ```

## Slint MCP (headless)

The app embeds the [Slint MCP server](https://github.com/slint-ui/slint/blob/master/internal/backends/testing/README.md), and `.mcp.json` registers it as `slint` at `http://127.0.0.1:9315/mcp`. Use it to verify UI changes in the running app with real data: walk the element tree, read properties, take screenshots, and click, drag, or type.

The server only answers while the app runs. Start it headless (no display needed) as a background process and wait for `Slint MCP server listening on http://127.0.0.1:9315/mcp`:

```sh
SLINT_EMIT_DEBUG_INFO=1 SLINT_MCP_PORT=9315 SLINT_BACKEND=headless cargo run --features slint/mcp
```

- `SLINT_EMIT_DEBUG_INFO=1` keeps element IDs and source locations; `--features slint/mcp` enables the server without adding `mcp` to `Cargo.toml`. The headless backend is meant for automation and is not a stable interface.
- Agents connect to MCP servers when they start, so the `slint` tools exist only if the app was already listening then (or after reconnecting the server).
- Otherwise call the same server with `curl` (JSON-RPC over Streamable HTTP):

  ```sh
  curl -s -X POST http://127.0.0.1:9315/mcp \
    -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' \
    -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_windows","arguments":{}}}'
  ```

  Use `"method":"tools/list"` to see every tool. Call `take_screenshot` with the `windowHandle` returned by `list_windows` (for example, `{"windowHandle":{"index":"1","generation":"1"}}`). Its JSON response contains a base64 PNG at `result.content[].data`; extract and base64-decode that field to create a PNG file.
- Stop the app when done.

## GitHub Actions

- Workflows live in `.github/workflows/`. Lint workflow changes with [`actionlint`](https://github.com/rhysd/actionlint) when it is installed. In private repositories, `.github/actionlint.yaml` declares the self-hosted ARC runner label (`arc-runner-*`) used by `runs-on`; keep the two in sync instead of switching `runs-on` to a hosted label.
- Follow the `github-actions-hardening` skill when writing or reviewing workflows: keep `permissions:` minimal and pass untrusted `${{ github.event.* }}` values through `env:` instead of interpolating them into `run:`.
- Inspect CI from the terminal with `gh pr checks`, or find a run with `gh run list` and read its failures with `gh run view <run-id> --log-failed` (the run ID is required outside an interactive terminal).
- GitHub docs index for agents: https://docs.github.com/llms.txt

## MCP servers

Configured in `.mcp.json` (read by Claude Code and OMP):

- `mcpls`: rust-analyzer over MCP (types, references, diagnostics, rename). Install: `rustup component add rust-analyzer && cargo install mcpls --version 0.9.1 --locked`
- `slint`: Running-app UI inspection and interaction MCP. Start the app with `SLINT_EMIT_DEBUG_INFO=1 SLINT_MCP_PORT=9315 SLINT_BACKEND=headless cargo run --features slint/mcp`.
- `slint-docs`: Hosted official Slint documentation search and fetch MCP; requires internet access.

## Agent skills

Pinned copies in `.claude/skills/` (read by Claude Code and OMP):

- `rust-best-practices` (`morganlinton/BeautifulRust@b9718436d77ee7bcf17cdcb6aa27628feb95d97a`)
- `rust-review` (`morganlinton/BeautifulRust@b9718436d77ee7bcf17cdcb6aa27628feb95d97a`)
- `slint` (`slint-ui/ai-plugins@d68574f61921630dd0a9eac33302f443cacea8eb`)
- `github-actions-hardening` (`github/awesome-copilot@227949a7542a55d2155c00e7cdb998d9a0fee4f3`)
