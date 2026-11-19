# NixOS module for Freminal's terminfo entry.
#
# Usage (in your NixOS config):
#
#   imports = [ freminal.nixosModules.default ];
#
#   programs.freminal.terminfo.enable = true;
#
# Freminal defaults to TERM=xterm-freminal. A copy of the entry is embedded in
# the binary and extracted per pane, but sudo, detached tmux servers and other
# processes that do not inherit TERMINFO need it installed system-wide.
{ freminal-flake }:
{
  config,
  lib,
  pkgs,
  ...
}:

let
  inherit (lib) mkEnableOption mkIf mkOption;
  cfg = config.programs.freminal.terminfo;
in
{
  options.programs.freminal.terminfo = {
    enable = mkEnableOption "the Freminal `xterm-freminal` terminfo entry system-wide";

    package = mkOption {
      type = lib.types.package;
      default = freminal-flake.packages.${pkgs.stdenv.hostPlatform.system}.terminfo;
      defaultText = lib.literalExpression "freminal.packages.\${system}.terminfo";
      description = "The package providing `share/terminfo` for Freminal.";
    };
  };

  config = mkIf cfg.enable {
    # hiPrio: the main freminal package ships the same files. Identical files
    # do not collide today, but a mixed-revision profile would; the dedicated
    # package winning keeps that deterministic.
    environment.systemPackages = [ (lib.hiPrio cfg.package) ];
  };
}
