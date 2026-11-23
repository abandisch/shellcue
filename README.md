# ShellCue

Terminal autocomplete: a sprig cut free.

ShellCue adds IDE-style completions to your terminal for hundreds of command-line tools such as `git`, `npm` and `docker`. It runs locally: no account, no telemetry, no AI.

It supports zsh on macOS and Linux.

## ⚡️ Installation

ShellCue is built from source; build instructions will follow.


## 🚀 Start Contributing

### Prerequisites

- MacOS
  - Xcode 13 or later
  - Brew

### 1. Clone repo

```shell
git clone https://github.com/abandisch/shellcue.git
```

### 2. Setup
Hassle-free setup:
```shell
npm run setup
```

Or if you'd like to DIY:

<details>
<summary>Manual Setup</summary>
<div>

### 1. Install platform dependencies

For Debian/Ubuntu:

```shell
sudo apt update
sudo apt install build-essential pkg-config jq dpkg curl wget cmake clang libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev valac libibus-1.0-dev libglib2.0-dev sqlite3 libxdo-dev protobuf-compiler
```
### 2. Install Rust toolchain using [Rustup](https://rustup.rs):

```shell
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup default stable
```

For pre-commit hooks, the following commands are required:

```shell
rustup toolchain install nightly
cargo install typos-cli
```

For MacOS development make sure the right targets are installed:

```shell
rustup target add x86_64-apple-darwin
rustup target add aarch64-apple-darwin
```

### 3. Setup the pinned toolchain using [`mise`](https://mise.jdx.dev)

Add mise integrations to your shell:

For zsh:
```shell
echo 'eval "$(mise activate zsh)"' >> "${ZDOTDIR-$HOME}/.zshrc"
```

For bash:
```shell
echo 'eval "$(mise activate bash)"' >> ~/.bashrc
```

For fish:
```shell
echo 'mise activate fish | source' >> ~/.config/fish/config.fish
```

Do not install pnpm separately: the repo refuses to run with any pnpm version other than the one pinned in `package.json`.

Install the pinned Node, pnpm, protoc and Python versions (Rust is pinned by `rust-toolchain.toml` and installed by rustup) using:

```shell
mise trust
mise install
```

### 4. Setup precommit hooks

Run `pnpm` in root directory to add pre-commit hooks:

```shell
pnpm install --ignore-scripts
```

</div>
</details>


### 3. Start Local Development
To compile and run the `shellcue` CLI:
```shell
cargo run --bin q_cli
```

> To run a specific subcommand, append `-- <command name>`. For example, to run `shellcue doctor`, you can run `cargo run --bin q_cli -- doctor`

To run tests for the CLI crate:
```shell
cargo test -p q_cli
```

To format Rust files:
```shell
cargo +nightly fmt
```

To run clippy:
```shell
cargo clippy --locked --workspace --color always -- -D warnings
```



## 🏗️ Project Layout

Several projects live here:

- [`autocomplete`](packages/autocomplete-app/) - The autocomplete react app
- [`figterm`](crates/figterm/) - figterm, our headless terminal/pseudoterminal that
  intercepts the user’s terminal edit buffer.
- [`q_cli`](crates/q_cli/) - the `shellcue` CLI
- [`fig_desktop`](crates/fig_desktop/) - the Rust desktop app, uses
  [`tao`](https://docs.rs/tao/latest/tao/)/[`wry`](https://docs.rs/wry/latest/wry/)
  for windowing/webviews
- [`fig_input_method`](crates/fig_input_method/) - The input method used to get cursor
  position on macOS

Other folder to be aware of

- [`build-scripts/`](build-scripts/) - Contains all python scripts to build
  and test the project on macOS and Linux
- [`crates/`](crates/) - Contains all internal rust crates
- [`packages/`](packages/) - Contains all internal npm packages
- [`proto/`](proto/) -
  [protocol buffer](https://developers.google.com/protocol-buffers/) message
  specification for inter-process communication
- [`tests/`](tests/) - Contain integration tests for the projects




## 🛡️ Security

See [SECURITY.md](SECURITY.md) for more information.

## 📜 Licensing

This repo is dual licensed under MIT and Apache 2.0 licenses.

ShellCue is a fork of [Amazon Q Developer CLI autocomplete](https://github.com/aws/amazon-q-developer-cli-autocomplete), which grew out of [Fig](https://github.com/withfig/autocomplete). It is an independent project and is not affiliated with, endorsed by or supported by Amazon or AWS.
