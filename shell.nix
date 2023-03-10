{ pkgs
, mkShell
, cargo
, clippy
, gcc
, protobuf
, rust-analyzer
, rustc
, rustfmt
}:

mkShell {
  nativeBuildInputs = [ cargo clippy gcc protobuf rust-analyzer rustc rustfmt ];

  RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
}
