{ exiftool
, ffmpeg_6-full
, imagemagick
, lib
, makeWrapper
, nixosTests
, protobuf
, rustPlatform
, Security
, stdenv
}:

rustPlatform.buildRustPackage {
  pname = "pict-rs";
  version = "0.4.0-rc.12";
  src = ./.;

  cargoLock = {
    lockFile = ./Cargo.lock;
  };

  PROTOC = "${protobuf}/bin/protoc";
  PROTOC_INCLUDE = "${protobuf}/include";

  nativeBuildInputs = [ makeWrapper ];
  buildInputs = lib.optionals stdenv.isDarwin [ Security ];

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
