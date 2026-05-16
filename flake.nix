{
  description = "RoyalTracker — Cruise Planner price-drop Telegram bot";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane = {
      url = "github:ipetkov/crane";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, crane }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        commonArgs = {
          src = craneLib.cleanCargoSource ./.;
          strictDeps = true;

          buildInputs = with pkgs; [
            openssl
          ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            pkgs.libiconv
          ];

          nativeBuildInputs = with pkgs; [
            pkg-config
            cmake
            perl
          ];

          # wreq pulls in BoringSSL via boring-sys
          OPENSSL_NO_VENDOR = "1";
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        buildBin = name: features: craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = name;
          cargoExtraArgs = "-p ${name} --no-default-features --features ${features}";
          doCheck = false;
        });

        cruise-bot     = buildBin "cruise-bot"     "postgres";
        cruise-scraper = buildBin "cruise-scraper" "postgres";

        # Shared build env so `nix run .#dev` works outside `nix develop`.
        buildEnv = {
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          CC  = "${pkgs.stdenv.cc}/bin/cc";
          CXX = "${pkgs.stdenv.cc}/bin/c++";
          BINDGEN_EXTRA_CLANG_ARGS = builtins.toString [
            "-isystem ${pkgs.glibc.dev}/include"
            "-isystem ${pkgs.llvmPackages.libclang.lib}/lib/clang/${pkgs.lib.versions.major (pkgs.lib.getVersion pkgs.llvmPackages.libclang)}/include"
          ];
          OPENSSL_NO_VENDOR = "1";
          PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
        };

        buildInputs = with pkgs; [
          stdenv.cc
          pkg-config
          cmake
          gnumake
          perl
          openssl
          llvmPackages.libclang
          rustToolchain
        ];

        # `nix run .#dev` — start a cloudflared quick-tunnel, capture its public URL,
        # and launch the bot+web with that URL injected via figment's env override
        # (CRUISE_WEB__PUBLIC_URL). No config.toml edits needed.
        devScript = pkgs.writeShellApplication {
          name = "cruise-dev";
          runtimeInputs = buildInputs ++ (with pkgs; [ cloudflared coreutils gnugrep ]);
          runtimeEnv = buildEnv;
          text = ''
            LOG=$(mktemp)
            cleanup() { jobs -p | xargs -r kill 2>/dev/null || true; rm -f "$LOG"; }
            trap cleanup EXIT INT TERM

            echo "==> starting cloudflared quick-tunnel..."
            cloudflared tunnel --no-autoupdate --url http://localhost:8080 \
              > "$LOG" 2>&1 &

            URL=""
            for _ in $(seq 1 60); do
              URL=$(grep -oE 'https://[a-z0-9-]+\.trycloudflare\.com' "$LOG" | head -1 || true)
              [ -n "$URL" ] && break
              sleep 1
            done
            if [ -z "$URL" ]; then
              echo "!! cloudflared didn't produce a public URL in 60s — log:" >&2
              cat "$LOG" >&2
              exit 1
            fi
            echo "==> Mini App URL: $URL"
            echo "    (the bot reads this from CRUISE_WEB__PUBLIC_URL, overriding config.toml)"
            echo ""

            export CRUISE_WEB__PUBLIC_URL="$URL"
            exec cargo run -q -p cruise-bot --no-default-features --features sqlite
          '';
        };

        mkImage = bin: pkgs.dockerTools.buildLayeredImage {
          name = bin.pname;
          tag = "latest";
          contents = [
            pkgs.cacert
            pkgs.tzdata
          ];
          config = {
            Entrypoint = [ "${bin}/bin/${bin.pname}" ];
            Env = [
              "SSL_CERT_FILE=/etc/ssl/certs/ca-bundle.crt"
              "TZ=UTC"
            ];
            User = "65534:65534";
          };
        };
      in
      {
        packages = {
          inherit cruise-bot cruise-scraper;
          cruise-bot-image     = mkImage cruise-bot;
          cruise-scraper-image = mkImage cruise-scraper;
          dev                  = devScript;
          default = cruise-bot;
        };

        apps.dev = {
          type = "app";
          program = "${devScript}/bin/cruise-dev";
        };

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rustToolchain
            pkg-config
            cmake
            perl
            openssl
            sqlx-cli
            sops
            age
            kubectl
            kubernetes-helm
            cargo-watch
            cargo-nextest
            cargo-edit
            postgresql_16
            sqlite
            jq
            cloudflared
            stdenv.cc           # gcc-wrapper with sane include paths
            llvmPackages.libclang
          ];
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          # Don't let cc-rs pick up bare clang and lose nix's include paths.
          CC  = "${pkgs.stdenv.cc}/bin/cc";
          CXX = "${pkgs.stdenv.cc}/bin/c++";
          # bindgen drives libclang directly, which doesn't inherit nix's gcc-wrapper
          # include paths — point it at glibc + the matching clang resource dir.
          BINDGEN_EXTRA_CLANG_ARGS = builtins.toString [
            "-isystem ${pkgs.glibc.dev}/include"
            "-isystem ${pkgs.llvmPackages.libclang.lib}/lib/clang/${pkgs.lib.versions.major (pkgs.lib.getVersion pkgs.llvmPackages.libclang)}/include"
          ];
          OPENSSL_NO_VENDOR = "1";
          PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";

          shellHook = ''
            export DATABASE_URL="sqlite:dev.db?mode=rwc"
            export RUST_LOG="info,cruise=debug"
            echo "royaltracker dev shell"
            echo "  rustc:    $(rustc --version)"
            echo "  sqlx:     $(sqlx --version 2>/dev/null || echo unavailable)"
            echo ""
            echo "Quick start:"
            echo "  nix run .#dev                                          # bot + cloudflared in one shot"
            echo "  cargo run -p cruise-bot --no-default-features --features sqlite   # bot only"
            echo "  nix build .#cruise-bot-image                           # OCI image"
          '';
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
