{
  description = "Fully featured Rust driver for the Brother HL-2270DW network laser printer";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "brother-2270dw";
            version = "0.1.0";

            # Keep the store source reproducible: only ship what cargo needs.
            src = pkgs.lib.fileset.toSource {
              root = ./.;
              fileset = pkgs.lib.fileset.unions [
                ./Cargo.toml
                ./Cargo.lock
                ./src
                ./examples
                ./ppd
                ./README.md
                ./LICENSE
              ];
            };

            # Fetch crate dependencies directly from the committed lockfile.
            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            # PDF printing shells out to `pdftops`, so put it on PATH. Also
            # install the PPD for CUPS.
            nativeBuildInputs = [ pkgs.makeWrapper pkgs.pkg-config ];
            buildInputs = [ pkgs.libusb1 ];
            postInstall = ''
              wrapProgram $out/bin/brother-2270dw \
                --prefix PATH : ${pkgs.poppler-utils}/bin
              mkdir -p $out/share/ppd
              cp ${./ppd/brother-hl-2270dw.ppd} $out/share/ppd/
            '';

            meta = with pkgs.lib; {
              description = "Driver for the Brother HL-2270DW network laser printer";
              homepage = "https://example.invalid/open-brother-2270dw";
              license = licenses.mit;
              mainProgram = "brother-2270dw";
              platforms = platforms.unix;
            };
          };
        });

      devShells = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              rustc
              cargo
              rustfmt
              clippy
              cargo-audit
              poppler-utils
              libusb1
              pkg-config
            ];
            RUST_BACKTRACE = "1";
            shellHook = ''
              echo "open-brother-2270dw dev shell — rustc $(${pkgs.rustc}/bin/rustc --version)"
            '';
          };
        });
    };
}
