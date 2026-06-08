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
