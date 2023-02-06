{
  description = "A simple task runner";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-22.11";
  inputs.flake-utils.url = "github:numtide/flake-utils";

  outputs = { self, nixpkgs, flake-utils }: {
    nixosModule = { config, lib, pkgs, ... }: 
      let 
        cfg = config.services.taru;
        taru_config = builtins.toJSON ({
          tasks = cfg.tasks;
        } // lib.optionalAttrs (!isNull cfg.users) {
          users = cfg.users;
        } // lib.optionalAttrs (!isNull cfg.heartbeat) {
          heartbeat = cfg.heartbeat;
        });

        taru_config_file = pkgs.writeText "taru.yml" taru_config;
      in with lib; {
      options.services.taru = {
        enable = mkEnableOption "Taru";

        heartbeat = mkOption {
          description = "Heartbeat for /events";
          type = types.nullOr types.int;
          default = null;
        };

        tasks = mkOption {
          type = types.attrsOf (types.submodule {
            options = {
              command = mkOption {
                type = types.listOf types.str;
              };

              meta = mkOption {
                type = types.attrs;
                default = {};
              };

              arguments = mkOption {
                type = types.listOf (types.submodule {
                  options = {
                    name = mkOption {
                      type = types.str;
                    };

                    datatype = mkOption {
                      type = types.enum ["String" "Int" "Enum"];
                      default = "Enum";
                    };

                    enum_source = mkOption {
                      type = types.nullOr (types.enum (lib.attrNames cfg.tasks));
                    };
                  };
                });
                default = [];
              };

              buffered = mkOption {
                type = types.bool;
                default = true;
              };

              headers = mkOption {
                type = types.attrsOf types.str;
                default = {};
              };
            };
          });
          default = {};
        };

        users = mkOption {
          type = types.nullOr (types.attrsOf (types.submodule {
            options = {
              can_run = mkOption {
                type = types.listOf (types.enum (lib.attrNames cfg.tasks));
              };

              can_view_output = mkOption {
                type = types.listOf (types.enum (lib.attrNames cfg.tasks));
              };

              can_view_status = mkOption {
                type = types.listOf (types.enum (lib.attrNames cfg.tasks));
              };
            };
          }));
          default = null;
        };

        nginx = {
          virtualHost = mkOption {
            type = types.nullOr types.str;
            default = null;
            description = "Expose taru on an nginx virtual host";
          };
        };
      };

      config = mkIf cfg.enable {
        users.users = {
          taru = {
            isSystemUser = true;
            group = "taru";
          };
        };

        users.groups = {
          taru = {};
        };

        system.activationScripts.users = stringAfter ["users"] "${pkgs.systemd}/bin/loginctl enable-linger taru";

        systemd.services.taru = {
          serviceConfig = {
            ExecStart = "${pkgs.taru}/bin/taru ${taru_config_file}";
            User = "taru";
            Group = "taru";
            WorkingDirectory = "${pkgs.taru}";
            ProtectSystem = true;
          };
        };

        systemd.sockets.taru = {
          listenStreams = [ "127.0.0.1:3000" ];
          wantedBy = [ "sockets.target" ];
          enable = true;
        };

        services.nginx = mkIf (!isNull cfg.nginx.virtualHost) {
          enable = true;
          virtualHosts.${cfg.nginx.virtualHost} = {
            locations."/" = {
              proxyPass = "http://127.0.0.1:3000";
              extraConfig = "proxy_buffering off;";
            };
          };
        };
      };
    };

    overlay = final: prev:
      let
        cargo_nix = import ./Cargo.nix { pkgs = prev; };
        rust_drv = cargo_nix.rootCrate.build;
      in {
        taru = prev.stdenv.mkDerivation {
          name = "taru";
          src = ./.;
          phases = [ "buildPhase" ];
          buildPhase = ''
            mkdir $out
            ${final.coreutils}/bin/cp -r ${rust_drv}/bin $out/bin
            ${final.coreutils}/bin/cp -r $src/public/ $out/
          '';
        };
      };
  } // (
    flake-utils.lib.eachSystem ["x86_64-linux"] (system: rec {
      defaultPackage =
        let
          pkgs = import nixpkgs { 
            inherit system;
            overlays = [
              self.overlay
            ];
          };
        in pkgs.taru;


      apps.default = {
        type = "app";
        program = "${defaultPackage}/bin/taru";
      };

      apps.crate2nix = {
        type = "app";
        program = "${nixpkgs.legacyPackages.${system}.crate2nix}/bin/crate2nix";
      };
    })
  );
}
