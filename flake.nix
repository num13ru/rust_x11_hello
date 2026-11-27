{
  description = "PaperPad development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-26.05-darwin";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    mbx = {
      url = "github:jdx/mr-boxington/v1.31.0";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, rust-overlay, mbx, ... }: {
    devShells = nixpkgs.lib.genAttrs [
      "aarch64-darwin"
      "x86_64-darwin"
      "aarch64-linux"
      "x86_64-linux"
    ] (system:
      let
        pkgs = import nixpkgs { inherit system; overlays = [ rust-overlay.overlays.default ]; };
        rust = pkgs.rust-bin.stable."1.97.1".minimal.override {
          extensions = [ "rustfmt" "clippy" ];
          targets = [ "armv7-unknown-linux-musleabihf" ];
        };
        commonPackages = with pkgs; [ rust gnumake bash llvmPackages.llvm file jq python3 ];
        mbxPackage = mbx.packages.${system}.mbx;
      in {
        default = pkgs.mkShell {
          packages = commonPackages;
        };
        mbx = pkgs.mkShell {
          packages = commonPackages ++ [ mbxPackage ];
          shellHook = ''
            export PATH="${mbxPackage}/libexec/mbx:$PATH"
          '';
        };
      });
  };
}
