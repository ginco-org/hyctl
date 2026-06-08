{
  description = "hytctl — Hytale game launcher CLI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      fenix,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ fenix.overlays.default ];
        };

        rustToolchain = pkgs.fenix.stable.withComponents [
          "rustc"
          "cargo"
          "clippy"
          "rustfmt"
          "rust-analyzer"
          "rust-src"
        ];

        rustPlatform = pkgs.makeRustPlatform {
          cargo = pkgs.fenix.stable.cargo;
          rustc = pkgs.fenix.stable.rustc;
        };

        hytctl-unwrapped = rustPlatform.buildRustPackage {
          pname = "hytctl";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = with pkgs; [ pkg-config ];
          buildInputs = with pkgs; [ openssl ];
          # Strip the target/ dir from the source to avoid cache pollution.
          # cargoLock handles reproducibility.
          doCheck = false;
        };

        # Libraries the bundled SDL3 dlopen()s at runtime.
        # On non-NixOS these come from standard system paths; on NixOS we
        # provide them via buildFHSEnv so the game can find them.
        gameLibs = pkgs: with pkgs; [
          # Wayland
          wayland
          libdecor
          libxkbcommon
          # OpenGL / EGL / GLES
          libGL
          mesa
          # X11 (SDL3 fallback backend)
          libx11
          libxcursor
          libxext
          libxfixes
          libxi
          libxrandr
          libxscrnsaver
          # System
          dbus
          systemd   # provides libudev.so
          liburing
        ];

        # Wrapped binary: prepends Nix store lib paths to LD_LIBRARY_PATH so
        # the bundled SDL3 can dlopen its backends (wayland, X11, GL, etc.).
        # Simpler than a full FHS env — no bubblewrap needed.
        hytctl = pkgs.symlinkJoin {
          name = "hytctl";
          paths = [ hytctl-unwrapped ];
          nativeBuildInputs = [ pkgs.makeWrapper ];
          postBuild = ''
            wrapProgram $out/bin/hytctl \
              --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath (gameLibs pkgs)}
          '';
        };
      in
      {
        packages = pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          default = hytctl;
          unwrapped = hytctl-unwrapped;
        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [ pkg-config gcc ];
          buildInputs = with pkgs; [ openssl ] ++ gameLibs pkgs;

          packages = [
            rustToolchain
            pkgs.cargo-edit
            pkgs.cargo-audit
          ];

          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          RUST_LOG = "hytctl=info";

          # Let `cargo run -- run` find the bundled SDL3 backends at runtime.
          shellHook = ''
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath (gameLibs pkgs)}:$LD_LIBRARY_PATH"
          '';
        };
      }
    );
}
