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
        defaultPackage = pkgs.callPackage ./pict-rs.nix {
          inherit (pkgs.darwin.apple_sdk.frameworks) Security;
        };
      });
}
