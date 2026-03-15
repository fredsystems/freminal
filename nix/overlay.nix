# Overlay that adds the `freminal` package to nixpkgs.
#
# Usage in a flake:
#   nixpkgs.overlays = [ freminal.overlays.default ];
#
# Then `pkgs.freminal` is available.
{ freminal-flake }:
final: _prev: {
  inherit (freminal-flake.packages.${final.stdenv.hostPlatform.system}) freminal;
}
