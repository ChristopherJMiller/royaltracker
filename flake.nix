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

        # The source tree needs *.rs/Cargo.toml/Cargo.lock (crane defaults) PLUS
        # static frontend assets (embedded via rust-embed) PLUS SQL migrations
        # (embedded via sqlx::migrate!). Include all of them explicitly.
        rawSrc = pkgs.lib.fileset.toSource {
          root = ./.;
          fileset = pkgs.lib.fileset.unions [
            (craneLib.fileset.commonCargoSources ./.)
            ./crates/royaltracker-web/static
            ./migrations
          ];
        };

        # Pre-build minification: HTML via html-minifier-terser, JS via esbuild.
        # Both are zero-config and safe — esbuild's --minify on our app.js
        # collapses whitespace + mangles internal identifiers without bundling
        # (so ES module imports from esm.sh stay intact).
        minifiedSrc = pkgs.runCommand "royaltracker-src-minified" {
          nativeBuildInputs = with pkgs; [ minify ];
        } ''
          cp -r ${rawSrc} $out
          chmod -R +w $out
          STATIC=$out/crates/royaltracker-web/static

          # tdewolff/minify handles HTML/CSS/JS in one binary; --type guesses by extension.
          minify -o "$STATIC/index.html.min" "$STATIC/index.html"
          minify -o "$STATIC/app.js.min"     "$STATIC/app.js"
          mv "$STATIC/index.html.min" "$STATIC/index.html"
          mv "$STATIC/app.js.min"     "$STATIC/app.js"

          echo "minified static assets:"
          ls -la "$STATIC"
        '';

        commonArgs = {
          src = minifiedSrc;
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
            git              # boring-sys2 needs `git init` + `git apply` to patch BoringSSL
            llvmPackages.libclang
          ];

          # wreq pulls in BoringSSL via boring-sys
          OPENSSL_NO_VENDOR = "1";
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          BINDGEN_EXTRA_CLANG_ARGS = builtins.toString [
            "-isystem ${pkgs.glibc.dev}/include"
            "-isystem ${pkgs.llvmPackages.libclang.lib}/lib/clang/${pkgs.lib.versions.major (pkgs.lib.getVersion pkgs.llvmPackages.libclang)}/include"
          ];
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        buildBin = name: features: craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = name;
          cargoExtraArgs = "-p ${name} --no-default-features --features ${features}";
          doCheck = false;
        });

        royaltracker-bot     = buildBin "royaltracker-bot"     "postgres";
        royaltracker-scraper = buildBin "royaltracker-scraper" "postgres";

        # Shared build env so `nix run .#dev` works outside `nix develop`.
        devEnv = {
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

        devInputs = with pkgs; [
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
        # (ROYALTRACKER_WEB__PUBLIC_URL). No config.toml edits needed.
        devScript = pkgs.writeShellApplication {
          name = "royaltracker-dev";
          runtimeInputs = devInputs ++ (with pkgs; [ cloudflared coreutils gnugrep ]);
          runtimeEnv = devEnv;
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
            echo "    (the bot reads this from ROYALTRACKER_WEB__PUBLIC_URL, overriding config.toml)"
            echo ""

            export ROYALTRACKER_WEB__PUBLIC_URL="$URL"
            exec cargo run -q -p royaltracker-bot --no-default-features --features sqlite
          '';
        };

        # OCI image. Distroless-ish: just the binary + CA bundle + tzdata + a
        # writable /tmp. Runs unprivileged. Use `royaltracker-bot` for the long-poll +
        # Mini App server; `royaltracker-scraper` for the daily CronJob.
        mkImage = bin: pkgs.dockerTools.buildLayeredImage {
          # bin.pname is already "royaltracker-<role>", so use it directly.
          name = bin.pname;
          tag = "latest";
          contents = [
            pkgs.cacert
            pkgs.tzdata
            (pkgs.writeTextDir "etc/passwd"
              "nobody:x:65534:65534:nobody:/:/sbin/nologin\n")
            (pkgs.writeTextDir "etc/group"
              "nobody:x:65534:\n")
          ];
          extraCommands = ''
            mkdir -p tmp
            chmod 1777 tmp
          '';
          config = {
            Entrypoint = [ "${bin}/bin/${bin.pname}" ];
            Env = [
              "SSL_CERT_FILE=/etc/ssl/certs/ca-bundle.crt"
              "TZ=UTC"
              "RUST_LOG=info,royaltracker=info"
            ];
            User = "65534:65534";
            ExposedPorts = pkgs.lib.optionalAttrs (bin.pname == "royaltracker-bot") {
              "8080/tcp" = {};
            };
            Labels = {
              "org.opencontainers.image.source" =
                "https://github.com/ChristopherJMiller/royaltracker";
              "org.opencontainers.image.title" = bin.pname;
              "org.opencontainers.image.description" =
                "Cruise Planner price-drop bot (Royal Caribbean + Celebrity)";
              "org.opencontainers.image.licenses" = "MIT";
            };
          };
        };
      in
      {
        packages = {
          inherit royaltracker-bot royaltracker-scraper;
          royaltracker-bot-image     = mkImage royaltracker-bot;
          royaltracker-scraper-image = mkImage royaltracker-scraper;
          dev                  = devScript;
          default = royaltracker-bot;
        };

        apps.dev = {
          type = "app";
          program = "${devScript}/bin/royaltracker-dev";
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
            minify
            stdenv.cc
            llvmPackages.libclang
          ];
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          CC  = "${pkgs.stdenv.cc}/bin/cc";
          CXX = "${pkgs.stdenv.cc}/bin/c++";
          BINDGEN_EXTRA_CLANG_ARGS = builtins.toString [
            "-isystem ${pkgs.glibc.dev}/include"
            "-isystem ${pkgs.llvmPackages.libclang.lib}/lib/clang/${pkgs.lib.versions.major (pkgs.lib.getVersion pkgs.llvmPackages.libclang)}/include"
          ];
          OPENSSL_NO_VENDOR = "1";
          PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";

          shellHook = ''
            export DATABASE_URL="sqlite:dev.db?mode=rwc"
            export RUST_LOG="info,royaltracker=debug"
            echo "royaltracker dev shell"
            echo "  rustc:    $(rustc --version)"
            echo "  sqlx:     $(sqlx --version 2>/dev/null || echo unavailable)"
            echo ""
            echo "Quick start:"
            echo "  nix run .#dev                                          # bot + cloudflared in one shot"
            echo "  cargo run -p royaltracker-bot --no-default-features --features sqlite   # bot only"
            echo "  nix build .#royaltracker-bot-image                           # OCI image (minified static)"
          '';
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
