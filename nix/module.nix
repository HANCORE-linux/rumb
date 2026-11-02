# Módulo NixOS: programs.rumb.enable = true;
{ rumbPackages }:
{ config, lib, pkgs, ... }:
let
  cfg = config.programs.rumb;
in
{
  options.programs.rumb = {
    enable = lib.mkEnableOption "rumb, a terminal dashboard for NixOS system health";
    package = lib.mkOption {
      type = lib.types.package;
      default = rumbPackages.${pkgs.stdenv.hostPlatform.system}.rumb;
      defaultText = lib.literalExpression "inputs.rumb.packages.\${pkgs.stdenv.hostPlatform.system}.rumb";
      description = "The rumb package to install.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
  };
}
