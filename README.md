# humanify-lua

> Un-minify Lua code using LLMs ("AI")

This tool uses large language models (like ChatGPT, Claude, Gemini, and
locally-hosted Ollama models) to unminify and rename minified or obfuscated Lua
code. The LLM only suggests new identifier names; the heavy lifting is done by
[full_moon](https://github.com/Kampfkarren/full-moon), a lossless Lua parser,
so the rewritten code is byte-identical to the input except for the renamed
identifiers — humanify does not reformat or reindent your code.

## Version 3 is out! 🎉

v3 highlights compared to v2:

* **Single static binary:** (Rust) — grab a binary from Releases. No Node, no
  npm, no Python.
* **Unix-style I/O**: read from stdin or a file, write to stdout or `-o <file>`.
* **New providers:** Ollama (local), Anthropic and OpenRouter.

Check out the [v3 PR](https://github.com/jehna/humanify/pull/744) for more info.

## Example

Given the following minified code in `splitstring.min.lua`:

```lua
local function a(e,t)local n={}local r=#e local i=1 while i<=r do local j=i+t-1 if j>r then j=r end table.insert(n,e:sub(i,j)) i=i+t end return n end
```

Run:

```shell
humanify-lua openai splitstring.min.lua -o splitstring.lua
```

Result (`splitstring.lua`):

```lua
local function split_string(input_string,chunk_size)local chunks={}local length=#input_string local start_index=1 while start_index<=length do local end_index=start_index+chunk_size-1 if end_index>length then end_index=length end table.insert(chunks,input_string:sub(start_index,end_index)) start_index=start_index+chunk_size end return chunks end
```

Only the identifiers changed — humanify never reflows or reindents your code,
so if you want it pretty-printed too, pipe the result through a Lua formatter
such as [StyLua](https://github.com/JohnnyMorganz/StyLua):

```shell
humanify-lua openai splitstring.min.lua | stylua - -o splitstring.lua
```

You can also pipe via stdin:

```shell
cat splitstring.min.lua | humanify-lua openai - > splitstring.lua
```

## Note on token usage

🚨 **NOTE:** 🚨

humanify-lua makes one LLM call per identifier in your code. For ChatGPT-class APIs
the cost roughly scales with the number of identifiers and the surrounding
context window (default 500 chars per call). A medium minified file (~500
identifiers) typically costs in the range of $0.10–$1.00 with OpenAI's small
models, free with the Gemini free tier, and free with Ollama or OpenRouter free
models.

For a rough character-count estimate of OpenAI mode:

```shell
echo "$((2 * $(wc -c < yourscript.min.lua)))"
```

Using `humanify-lua ollama` is free but slower; quality depends on your local model.
Free OpenRouter models (e.g. `qwen/qwen3-coder:free`) can help with your budget,
but expect them to be heaviy rate limited.

## Getting started

### Installation

The preferred way to install humanify-lua is to download a pre-built binary from the
[latest release](https://github.com/jehna/humanify/releases/latest).

```shell
# macOS (Apple Silicon)
curl -L https://github.com/jehna/humanify/releases/latest/download/humanify-lua-aarch64-apple-darwin.tar.gz | tar xz
sudo mv humanify-lua /usr/local/bin/

# macOS (Intel)
curl -L https://github.com/jehna/humanify/releases/latest/download/humanify-lua-x86_64-apple-darwin.tar.gz | tar xz
sudo mv humanify-lua /usr/local/bin/

# Linux (x86_64)
curl -L https://github.com/jehna/humanify/releases/latest/download/humanify-lua-x86_64-unknown-linux-gnu.tar.gz | tar xz
sudo mv humanify-lua /usr/local/bin/

# Linux (aarch64)
curl -L https://github.com/jehna/humanify/releases/latest/download/humanify-lua-aarch64-unknown-linux-gnu.tar.gz | tar xz
sudo mv humanify-lua /usr/local/bin/

# Windows: download humanify-lua-x86_64-pc-windows-msvc.zip from the releases page
```

Or build from source:

```shell
cargo install --git https://github.com/jehna/humanify
```

### Usage

```shell
humanify-lua <openai|gemini|anthropic|ollama|openrouter|requesty> [FLAGS] <INPUT>
```

* `<INPUT>` is a file path or `-` for stdin.
* `-o <FILE>` writes to a file (default: stdout).
* `-m <MODEL>` overrides the preset's default model.
* `-k <KEY>` overrides the env-var-based API key.
* `--base-url <URL>` overrides the preset's base URL.
* `--context-size <N>` sets surrounding-code chars per identifier (default 500).
* `--json-mode <MODE>` pins a JSON-mode strategy. Options:
  `ladder` (default), `openai-json-schema`, `anthropic-native`,
  `forced-tool-call`, `tool-call-and-prompt`, `prompt`.
* `-v` prints resolved configuration and identifier-level rename steps to stderr.
* `--progress` shows an identifier progress bar on stderr.

Run `humanify-lua --help` for the full reference.

Note: humanify-lua does one job — rename identifiers in one Lua file in, one out.
It supports Lua 5.1 through 5.4, LuaJIT, and CitizenFX/FiveM syntax
extensions (compound assignment, safe navigation, etc). It does not reverse
control-flow flattening, string encryption, or other bytecode/VM-level
obfuscation (e.g. from Luraph or Prometheus) — only identifier renaming.

### OpenAI mode

You'll need an OpenAI API key. Sign up at https://openai.com/ and create a
key in the dashboard.

```shell
humanify-lua openai obfuscated.lua -o readable.lua -k your-token
```

Or via environment variable:

```shell
export OPENAI_API_KEY=your-token
humanify-lua openai obfuscated.lua -o readable.lua
```

Default model: `gpt-5-mini`. Override with `-m`.

### Gemini mode

You'll need a Google AI Studio key. Sign up at https://aistudio.google.com/.
Gemini's free tier is generous and is enough for most files.

```shell
export GEMINI_API_KEY=your-token
humanify-lua gemini obfuscated.lua -o readable.lua
```

Default model: `gemini-3.1-flash-lite`. Override with `-m`.

### Anthropic mode

You'll need an Anthropic API key. Sign up at https://console.anthropic.com/.

```shell
export ANTHROPIC_API_KEY=your-token
humanify-lua anthropic obfuscated.lua -o readable.lua
```

Default model: `claude-sonnet-4-6`. Override with `-m`.

The Anthropic preset uses Anthropic's native structured-outputs API
(`output_format: json_schema`) when available, falling back to forced
tool-calls if your account doesn't have the structured-outputs beta enabled.

### Local mode (Ollama)

Local mode runs against [Ollama](https://ollama.com/), which manages local LLM
weights and exposes an OpenAI-compatible API on `localhost:11434`. (pre-v3
migration note: There's no `humanify download` anymore — use a local inference
provider like Ollama to run your own models)

Prerequisites:
1. Install Ollama: <https://ollama.com/download>
2. Pull the recommended model: `ollama pull qwen3.5:4b`

Then run:

```shell
humanify-lua ollama obfuscated.lua -o readable.lua
```

Default model: `qwen3.5:4b`. Override with `-m` to use any model you've
pulled. Local mode is free and private, but slower and less accurate than
the hosted providers; quality depends on the model you pick.

If you want to point humanify-lua at a remote Ollama instance, override the
base URL:

```shell
humanify-lua ollama obfuscated.lua --base-url http://my-server:11434/v1
```

### OpenRouter mode

[OpenRouter](https://openrouter.ai/) routes requests across many backend
models. Useful for trying free-tier coding models without setting up
multiple accounts.

You'll need an OpenRouter API key. Sign up at https://openrouter.ai/.

```shell
export OPENROUTER_API_KEY=your-token
humanify-lua openrouter obfuscated.lua -o readable.lua
```

Default model: `openai/gpt-oss-120b`. For free-tier usage:

```shell
humanify-lua openrouter obfuscated.lua -m qwen/qwen3-coder:free
```

### Requesty mode

[Requesty](https://requesty.ai/) provides an OpenAI-compatible router across
many backend models via a single API key.

You'll need a Requesty API key. Sign up at https://requesty.ai/.

```shell
export REQUESTY_API_KEY=your-token
humanify-lua requesty obfuscated.lua -o readable.lua
```

Default model: `nvidia/nemotron-3-super-120b-a12b`. Override with `-m`:

```shell
humanify-lua requesty obfuscated.lua -m nvidia/nemotron-3-super-120b-a12b
```

## Features

* Uses LLMs to get smart suggestions to rename variable and function names, and
  make the rename deterministically with [full_moon][full_moon], a lossless
  Lua parser — no reformatting, no reflowing, just renamed identifiers
* Supports Lua 5.1, 5.2, 5.3, 5.4, LuaJIT, and CitizenFX/FiveM (`cfxlua`)
  syntax extensions — compound assignment (`+=`), safe navigation (`?.`),
  `local a, b in t` field-unpacking, `{ .a }` set constructors, C-style
  `/* */` comments, and `` `hash` `` backtick string literals
* Renames preserve all references and respect lexical scoping, including
  upvalues captured by closures and the implicit `self` in `function t:m()`
  methods
* Reserved-word and collision-aware safe naming. The LLM's suggestion is
  normalised to a valid Lua identifier and suffixed with a number if it
  collides with an existing binding

[full_moon]:https://github.com/Kampfkarren/full-moon

## Contributing

If you'd like to contribute, please fork the repository and use a feature
branch. Pull requests are warmly welcome.

```shell
git clone https://github.com/jehna/humanify
cd humanify
cargo build
cargo test
```

CI runs `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` on
every PR. Local `ollama` and judge e2e suites also run on every PR.
The `gemini` e2e suite runs by default only for branches in this repository.
Other providers' e2e suites require both a branch in this repository and
their corresponding label (`test-openai`, `test-anthropic`,
`test-openrouter`, `test-requesty`) to avoid exposing secrets to forks or
burning API credits on every PR.

## Star History

<a href="https://www.star-history.com/?repos=jehna%2Fhumanify&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=jehna/humanify&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=jehna/humanify&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=jehna/humanify&type=date&legend=top-left" />
 </picture>
</a>

## Licensing

The code in this project is licensed under MIT license.
