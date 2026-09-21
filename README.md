# hyctl

CLI for installing and launching the Hytale game client, with multi-account, profile, and version management.

## Commands

| Command | Description |
|---------|-------------|
| `launch` | Launch the game client |
| `serve`  | Run a game server |
| `auth`   | Manage accounts and profiles |
| `asset`  | Manage installed game versions |

### auth subcommands

| Command      | Description |
|--------------|-------------|
| `list`       | List saved accounts and profiles |
| `add`        | Add an account (opens browser for OAuth login) |
| `remove`     | Remove a saved account |
| `default`    | Set the default account |

### asset subcommands

| Command   | Description |
|-----------|-------------|
| `install` | Download and install a game version |
| `list`    | List installed versions |
| `remove`  | Remove an installed version |

## Install

### Download a binary

Grab a prebuilt binary from the [releases page](https://github.com/ginco-org/hyctl/releases) — no Nix or Rust required:

```sh
# Linux x86_64 (glibc)
curl -LO https://github.com/ginco-org/hyctl/releases/latest/download/hyctl-x86_64-unknown-linux-gnu.tar.gz
tar xzf hyctl-x86_64-unknown-linux-gnu.tar.gz
sudo mv hyctl-x86_64-unknown-linux-gnu/hyctl /usr/local/bin/
```

Also published per release: Linux x86_64 (musl, static), macOS Apple silicon
and Intel, Windows x86_64 (`.zip`), and a SHA-256 checksum for every archive.

### With Nix

```sh
nix run github:<owner>/hyctl
```

Or add to a flake input:

```nix
hyctl.url = "github:<owner>/hyctl";
```

### With Cargo

```sh
cargo install --git https://github.com/<owner>/hyctl
```

## Development

```sh
nix develop
cargo build
cargo run -- --help
```
