{
  description = "LDK Node Development Environment";

  inputs = {
    # Nixpkgs channel. New channels are released every 6 months.
    # See: https://github.com/NixOS/nixpkgs/tags
    nixpkgs.url = "github:nixos/nixpkgs/25.11";

    # This makes it easy for the flake to be multi-platform.
    # See: https://github.com/numtide/flake-utils
    flake-utils.url = "github:numtide/flake-utils";

    # Provides Rust toolchains.
    # See: https://github.com/oxalica/rust-overlay
    rust-overlay.url = "github:oxalica/rust-overlay";

    # Pinned nixpkgs that ships bitcoind 27.1. The integration tests' bundled
    # `corepc-node` deserializes the 27.x `getblockchaininfo` schema, so newer
    # bitcoind (25.11 ships 30.0) fails RPC decoding. Used only for BITCOIND_EXE.
    nixpkgs-bitcoind.url = "github:nixos/nixpkgs/ab7b6889ae9d484eed2876868209e33eb262511d";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
      nixpkgs-bitcoind,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        # Overlays provide additional packages not available in the channels.
        overlays = [
          # Provides the rust-bin package; a set of pre-built Rust toolchains.
          (import rust-overlay)
        ];

        # The final set of packages.
        pkgs = import nixpkgs {
          # Inheriting from system is what makes this multi-platform.
          # We also inherit the overlays that we want to use.
          inherit system overlays;
        };

        # bitcoind 27.1 from the pinned input (see inputs above).
        bitcoind = nixpkgs-bitcoind.legacyPackages.${system}.bitcoind;

        # The specific Rust toolchain that we use in development shells.
        # Matches the `rust-version` declared in Cargo.toml. We use the `minimal`
        # profile (rustc, cargo, rust-std) plus clippy and rust-src, but
        # deliberately exclude `rustfmt`: this repo's `rustfmt.toml` relies on
        # nightly-only options, so formatting is supplied by `rustfmt-nightly`
        # below to keep `just fmt`/`just check` consistent with CI's nightly job.
        rust-toolchain = pkgs.rust-bin.stable."1.85.1".minimal.override {
          extensions = [
            "rust-src" # Needed for the rust-analyzer extension to work.
            "clippy" # Linter used by `just check`.
          ];
        };

        # Nightly rustfmt only. `cargo fmt` shells out to whichever `rustfmt` is
        # on PATH, so a nightly rustfmt lets the nightly-only options in
        # `rustfmt.toml` apply even though cargo/rustc are pinned to stable.
        rustfmt-nightly = pkgs.rust-bin.nightly.latest.rustfmt;

        # Esplora/HTTP electrs for the integration tests' Esplora chain source.
        #
        # We deliberately do NOT use nixpkgs' `blockstream-electrs`: it is a far
        # newer build whose initial regtest indexing takes ~80s, blowing past the
        # tests' sync timeout. Instead we pin the *exact* prebuilt binary CI uses
        # (see scripts/download_bitcoind_electrs.sh) and patch it to run on Nix.
        # This keeps local test behaviour identical to CI.
        electrs-esplora =
          let
            rev = "a33e97e1a1fc63fa9c20a116bb92579bbf43b254";
            sources = {
              x86_64-linux = {
                file = "electrs_linux_esplora_${rev}.zip";
                sha256 = "865e26a96e8df77df01d96f2f569dcf9622fc87a8d99a9b8fe30861a4db9ddf1";
              };
              x86_64-darwin = {
                file = "electrs_macos_esplora_${rev}.zip";
                sha256 = "2d5ff149e8a2482d3658e9b386830dfc40c8fbd7c175ca7cbac58240a9505bcd";
              };
            };
            src =
              sources.${system}
                or (throw "no prebuilt esplora electrs for ${system}");
          in
          pkgs.stdenv.mkDerivation {
            pname = "electrs-esplora";
            version = "esplora-${builtins.substring 0 9 rev}";
            src = pkgs.fetchurl {
              url = "https://github.com/RCasatta/electrsd/releases/download/electrs_releases/${src.file}";
              inherit (src) sha256;
            };
            nativeBuildInputs =
              [ pkgs.unzip ] ++ pkgs.lib.optional pkgs.stdenv.isLinux pkgs.autoPatchelfHook;
            buildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [ pkgs.stdenv.cc.cc.lib ];
            unpackPhase = ''
              runHook preUnpack
              unzip "$src"
              runHook postUnpack
            '';
            installPhase = ''
              runHook preInstall
              install -Dm755 electrs "$out/bin/electrs"
              runHook postInstall
            '';
          };

      in
      {
        # The development shell. Use `nix develop` or direnv to enter it.
        devShells.default = pkgs.mkShell {
          name = "ldk-node-dev-shell";

          packages = with pkgs; [
            rustfmt-nightly # Nightly rustfmt; must precede the stable toolchain on PATH.
            rust-toolchain # Rust toolchain (no rustfmt; see above).
            nodejs # JavaScript runtime, required for MCP tools
            mold # Fast linker for Rust/C/C++
            pnpm # Package manager for JavaScript, required for MCP tools
            pkg-config # Required by build scripts that link against system libraries
            just # Command runner used for `just check`, `just fmt`, etc.
            stdenv.cc.cc.lib # C++ standard library for runtime
          ];

          env = {
            # OpenSSL configuration for Nix
            PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
            # C++ standard library path for runtime
            LD_LIBRARY_PATH = "${pkgs.stdenv.cc.cc.lib}/lib";
            # Integration tests (run with `--cfg no_download`) locate these via env
            # rather than downloading generic-linux binaries that can't run on NixOS.
            BITCOIND_EXE = "${bitcoind}/bin/bitcoind";
            ELECTRS_EXE = "${electrs-esplora}/bin/electrs";
          };

          shellHook = ''
            echo "LDK Node dev shell"
            rustc --version
            cargo --version
          '';
        };
      }
    );
}
