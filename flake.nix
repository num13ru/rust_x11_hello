{
  description = "PaperPad development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-26.05-darwin";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, rust-overlay, ... }: {
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
      in {
        default = pkgs.mkShell {
          packages = with pkgs; [ rust gnumake bash llvmPackages.llvm file jq python3 ];
        };
      });
  };
}
