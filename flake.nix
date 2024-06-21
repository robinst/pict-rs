{
  description = "pict-rs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
        };
      in
      {
        packages = rec {
          imagemagick7_pict-rs = pkgs.callPackage ./nix/pkgs/imagemagick_pict-rs {};
          ffmpeg6_pict-rs = pkgs.callPackage ./nix/pkgs/ffmpeg_pict-rs {};

          pict-rs = pkgs.callPackage ./pict-rs.nix {
            inherit (pkgs.darwin.apple_sdk.frameworks) Security;
            inherit imagemagick7_pict-rs ffmpeg6_pict-rs;
          };

          default = pict-rs;
        };

        docker = pkgs.dockerTools.buildLayeredImage {
          name = "pict-rs";
          tag = "latest";

          contents = [ pkgs.tini self.packages.${system}.pict-rs pkgs.bash ];

          config = {
            Entrypoint = [ "/bin/tini" "--" "/bin/pict-rs" ];
            Cmd = [ "run" ];
          };
        };

        apps = rec {
          dev = flake-utils.lib.mkApp { drv = self.packages.${system}.pict-rs; };
          default = dev;
        };

        devShell = with pkgs; mkShell {
          nativeBuildInputs = [
            cargo
            cargo-deny
            cargo-outdated
            certstrap
            clippy
            curl
            diesel-cli
            exiftool
            garage
            self.packages.${system}.imagemagick7_pict-rs
            self.packages.${system}.ffmpeg6_pict-rs
            jq
            minio-client
            rust-analyzer
            rustc
            rustfmt
            stdenv.cc
            taplo
            tokio-console
          ];

          RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
        };
      });
}
