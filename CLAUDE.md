# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`humanify-lua` is a single-binary Rust CLI that un-minifies Lua by asking an LLM for a better name for each identifier, then applying the rename deterministically with [full_moon](https://github.com/Kampfkarren/full-moon), a lossless Lua parser. The LLM never rewrites code — it only returns `{"name": "..."}` per identifier. One LLM call per identifier.

## Commands

```shell
cargo build
cargo test                                   # unit + cli_smoke (no network needed; smoke tests hit an unreachable base-url)
cargo test <name>                            # single test by substring, e.g. cargo test ladder::tests::first_strategy
cargo test --test cli_smoke                  # one integration test file
cargo fmt -- --check                         # CI fails on formatting
cargo clippy -- -D warnings                  # CI also sets RUSTFLAGS=-D warnings
cargo run -- ollama fixtures/example.min.lua -v --progress
```

CI on every PR: `fmt --check`, `clippy -D warnings`, `cargo test`, plus the `e2e_ollama` and `e2e_judge` suites (local Ollama). Both `fmt` and `clippy` must be clean before pushing.

### E2E tests (all `#[ignore]`, need live providers)

```shell
cargo test --test e2e_judge  -- --ignored --nocapture --test-threads=1   # needs local Ollama with qwen3.5:4b
cargo test --test e2e_ollama -- --ignored --nocapture                    # needs Ollama with gemma3:4b + qwen3.5:4b
cargo test --test e2e_openai -- --ignored --nocapture                    # needs OPENAI_API_KEY (same pattern for gemini/anthropic/openrouter/requesty)
```

`HUMANIFY_E2E_JUDGE_URL` overrides the Ollama judge URL. In CI, hosted-provider e2e suites only run for in-repo branches with a `test-<provider>` PR label (`test-openai`, `test-anthropic`, `test-openrouter`, `test-requesty`); Gemini runs for any in-repo branch.

## Architecture

Data flow: `main.rs` (clap) → `cli/<provider>.rs` (`DEFAULTS` + `run`) → `cli/preset.rs::run_preset` → `rename::walker::rename_all_identifiers_with_observer` → `llm::LlmRenamer` → `llm::Ladder` → a `JsonStrategy` → `llm::http::HttpClient`.

### `rename/` — deterministic AST side (sync)

`full_moon` gives a lossless AST (every token, including whitespace/comments, is recoverable) but **no semantic analysis** — no scopes, no binding resolution. That's built here:

- `scope.rs`: walks the `full_moon` AST once, in source order, building a `ScopeModel` — every lexical scope (chunk, function body, `do`, loop bodies, each `if`/`elseif`/`else` branch, `repeat...until`) with its byte span and parent, every **local** binding (`local x`, `local function f`, parameters, `for` loop variables, the implicit `self` of `function t:m()`) with every identifier token that resolves to it, and every identifier that resolves to nothing (a global — `print`, a host global, or a name from another chunk). Resolution happens during the same walk: declarations are pushed onto their scope at the exact point Lua makes them visible (e.g. a `local x = x` initializer resolves against the *outer* `x`; a `local function f` is visible inside its own body, but `local f = function() end` is not), so a lookup at each reference is already correct. Only locals are ever renamed — globals may be provided by the host or another chunk, so touching them would change behavior.
- `walker.rs`: builds the `ScopeModel`, orders renamable bindings largest-scope-first (ties broken by source position), asks the `Renamer` for a new name with `context_size` chars of surrounding code, and re-emits the source by splicing renamed spans directly into the original text — this is why comments and formatting on unrenamed code survive untouched. Every binding is renamed independently — shadowed names in different scopes get separate LLM calls.
- `safe_name.rs`: normalizes the LLM suggestion into a valid Lua identifier (ASCII-only `[A-Za-z_][A-Za-z0-9_]*`, keyword-avoiding).
- `collision.rs`: prevents a new name from capturing/shadowing a binding visible in the same scope chain (ancestor, same scope, descendant, or a referenced global), using `ScopeModel`'s scope tree; dedupes by incrementing a numeric suffix (`index` → `index2`, `index3` → `index4`).
- `Renamer` trait (`rename(original, surrounding_code) -> String`; returning the same string means "leave alone") and `RenameObserver` (progress hooks) live in `rename/mod.rs`. The walker knows nothing about LLMs.

`full_moon` is built with the `lua54` + `luajit` + `cfxlua` + `serde` features (the first implies `lua52`/`lua53`; `serde` works around an upstream bug where `cfxlua`'s `compound.rs` unconditionally does `use serde::...` even with the `serde` feature off), and parsed via `LuaVersion::new().with_cfxlua()`, so the parser accepts the union of Lua 5.1–5.4, LuaJIT, and CitizenFX/FiveM syntax (`+=`-style compound assignment — handled explicitly in `scope.rs`'s `Stmt::CompoundAssignment` arm — plus `?.` safe navigation, `.a` table-constructor shorthand, `local a, b in t` field-unpacking, C-style `/* */` comments, and `` `hash` `` backtick string literals, all of which reuse existing AST shapes and need no special-casing). Luau (Roblox) is not enabled. Validated against the real 2.2MB/103k-line `static.cfx.re/natives/natives.lua` FiveM native reference: parses and round-trips byte-identical in ~0.6s.

One `full_moon` gotcha: `TokenReference` implements both an inherent (`Token`, via `Deref`) and a `Node`-trait version of `start_position`/`end_position` with the same name but different return types (`Position` vs `Option<Position>`); plain method-call syntax resolves to the `Node` version. `scope.rs`'s `tok_start`/`tok_end` helpers force the infallible `Token` version via UFCS — use them instead of calling `.start_position()`/`.end_position()` directly on a `&TokenReference`.

### `llm/` — async provider side

- `JsonStrategy` trait: `call(system, user, schema) -> Result<Value, StrategyError>`. Implementations in `openai_compat.rs` (`OpenAIJsonSchema`, `ForcedToolCall`, `ToolCallAndPrompt`, `PromptToJson`) and `anthropic.rs` (`AnthropicToolCallAndPrompt`, `AnthropicNativeJsonSchema`).
- `StrategyError` has exactly two variants and the distinction drives everything: `NotSupported` = the provider/model permanently rejects this strategy (fall through to the next); `Transient` = network/429/5xx/bad JSON (propagate; the renamer keeps the original name). `http.rs::classify_error` maps HTTP status + error body into one of these.
- `ladder.rs::Ladder`: probes strategies in order on the first call, **locks** onto the first that succeeds, marks `NotSupported` ones dead (bitmask, max 64 strategies). `Ladder::pinned` is the single-strategy form used by `--json-mode <specific>`; there `NotSupported` is converted to `Transient` since no fallback exists. Lock-free via atomics; safe under concurrent calls.
- `renamer.rs::LlmRenamer` bridges the sync `Renamer` trait to the async ladder with `Handle::block_on` — it **panics if called from inside the runtime**, which is why `run_preset` wraps the walker in `spawn_blocking`.
- On any LLM failure `LlmRenamer` returns the original name, so a fully-offline run is an identity transform (this is what `tests/cli_smoke.rs` relies on). This module is language-agnostic — nothing here knows the input is Lua except the prompt text in `renamer.rs`.

### `cli/` — presets

Each provider file is just a `PresetDefaults` const (name, base URL, default model, API-key env var, `ProviderKind`, timeout) plus an `Args` struct and `run` that forwards to `run_preset`. `ProviderKind::OpenAICompat` vs `Anthropic` selects the default ladder in `preset.rs::build_default_ladder` and which `--json-mode` values are legal (`validate_json_mode_for_provider`). Anthropic's default ladder deliberately contains only `AnthropicToolCallAndPrompt`; `anthropic-native` is opt-in because its response parsing is unvalidated against the live API.

Adding a provider = new `cli/<name>.rs` with `DEFAULTS`, a `Commands` variant + `into_*_args` in `main.rs`, `pub mod` in `cli/mod.rs`, and optionally `tests/e2e_<name>.rs` + a workflow.

Exit codes from `run_preset`: `0` ok, `1` I/O or internal error, `2` Lua parse error, `64` CLI usage / invalid `--json-mode`.

## Testing conventions

- Unit tests live inline (`#[cfg(test)] mod tests`) and use the per-module test DSLs: `rename/test_dsl.rs` (`FixedRenamer`, `QueueRenamer`, `SuffixRenamer`, `IdentityRenamer`, `MapRenamer` + scenario helpers, plus `model_of`/`renaming` for scope- and collision-level assertions) and `llm/test_dsl.rs` (`ScriptedStrategy`, `ok`, `ok_n`, `not_supported`, `script`, `ladder_with`, `pinned_with`). Prefer the DSL for new tests; where a module intentionally doesn't use it there's a `// DSL not applied: ...` comment explaining why — keep that pattern.
- `tests/common/mod.rs` is the e2e DSL: `given(fixture).judge_says_minified().await.when(humanify().ollama().model(..)).await.then_judge_says_one_of(&["EXCELLENT","GOOD"]).await`. An LLM "judge" (same provider as the renamer under test) classifies output as EXCELLENT / GOOD / GIBBERISH / MINIFIED. `e2e_judge.rs` guards the judge prompt's calibration itself.
- `fixtures/example.min.lua` is the shared e2e input.
