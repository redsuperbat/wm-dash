{
  description = "Workmux agent dashboard";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { self, nixpkgs }:
    let
      forAllSystems = f: nixpkgs.lib.genAttrs [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ]
        (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "wm-dash";
          version = "0.1.0";
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          meta.mainProgram = "wm-dash";
        };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc clippy rustfmt rust-analyzer ];
        };
      });
    };
}
