{
  description = "One-click installer for Microsoft 365 and Office 2024 on Linux";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      # .cargo/config.toml pins `-C target-cpu=x86-64-v3` and the project only ships
      # x86_64 builds, so that is the single system we build for.
      systems = [ "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          lib = pkgs.lib;
        in
        rec {
          linux-office-installer-suite = pkgs.rustPlatform.buildRustPackage {
            pname = "linux-office-installer-suite";
            version = (lib.importTOML ./Cargo.toml).package.version;
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = with pkgs; [
              pkg-config
              # aws-lc-sys (pulled in by rustls) is allowed to fall back to its CMake builder.
              cmake
              # Slint reaches X11, Wayland, xkbcommon and OpenGL through dlopen(3), so the
              # usual ELF fixup cannot see them. autoPatchelfHook plus `runtimeDependencies`
              # below adds them to the binary's RUNPATH and to the closure.
              autoPatchelfHook
            ];
            buildInputs = with pkgs; [
              fontconfig # yeslogic-fontconfig-sys (femtovg) requires it via pkg-config
              freetype # fontconfig's own pkg-config dependency
              libGL
              wayland # wayland-sys probes libwayland-client via pkg-config (winit)
              xz # lzma-sys, used by xz2
            ];
            runtimeDependencies = with pkgs; [
              libGL
              libx11
              libxcursor
              libxi
              libxkbcommon
              wayland
            ];
            # The unit tests shell out to pacman/desktop-file-validate/shellcheck and bind
            # loopback sockets, none of which belong to a plain build of the binary.
            doCheck = false;
            meta = {
              description = "Install Microsoft 365 or Office 2024 with Wine";
              homepage = "https://github.com/ErenEksen/LinuxOfficeInstallerSuite";
              mainProgram = "linux-office-installer-suite";
              platforms = lib.platforms.linux;
            };
          };
          default = linux-office-installer-suite;
        }
      );
    };
}
