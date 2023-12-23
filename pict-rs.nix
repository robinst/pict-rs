{ exiftool
, ffmpeg_6-full
, imagemagick
, lib
, makeWrapper
, nixosTests
, rustPlatform
, Security
, stdenv
}:

rustPlatform.buildRustPackage {
  pname = "pict-rs";
  version = "0.5.0-rc.6";
  src = ./.;

  cargoLock = {
    lockFile = ./Cargo.lock;
  };

  nativeBuildInputs = [ stdenv makeWrapper ];
  buildInputs = lib.optionals stdenv.isDarwin [ Security ];

  RUSTFLAGS = "--cfg tokio_unstable";
  TARGET_CC = "${stdenv.cc}/bin/${stdenv.cc.targetPrefix}cc";
  TARGET_AR = "${stdenv.cc}/bin/${stdenv.cc.targetPrefix}ar";

  postInstall = ''
    wrapProgram $out/bin/pict-rs \
        --prefix PATH : "${lib.makeBinPath [ imagemagick ffmpeg_6-full exiftool ]}"
  '';

  passthru.tests = { inherit (nixosTests) pict-rs; };

  meta = with lib; {
    description = "A simple image hosting service";
    homepage = "https://git.asonix.dog/asonix/pict-rs";
    license = with licenses; [ agpl3Plus ];
  };
}
