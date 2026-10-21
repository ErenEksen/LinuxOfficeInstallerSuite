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

      # Libraries Wine needs to start, draw windows and run Office, on both widths.
      # This is the same soname set the app checks for in src/deps/maps.rs, plus the
      # usual Wine extras that Bottles' FHS environment carries.
      winePkgs =
        pkgs: with pkgs; [
          # src/deps/maps.rs REQUIRED
          glibc # ld-linux.so.2 (32-bit)
          libunwind
          freetype
          fontconfig
          gnutls
          libx11
          libxext
          libxrandr
          libxcursor
          libxrender
          libxxf86vm
          libxkbcommon
          wayland
          libglvnd # libGL.so.1 and libEGL.so.1
          vulkan-loader
          libpulseaudio
          alsa-lib
          udev # libudev.so.1
          dbus
          krb5
          gmp
          libgcrypt
          cups # libcups.so.2
          # what a Wine prefix usually wants on top of that
          attr
          giflib
          libgpg-error
          libjpeg_turbo
          libpcap
          libpng
          libtiff
          libusb1
          libv4l
          libva
          libxml2
          libxslt
          mpg123
          ncurses
          ocl-icd
          openal
          p11-kit
          sane-backends
          SDL2
          zlib
          # X11 extensions Wine registers
          libsm
          libxcb
          libxcomposite
          libxdamage
          libxfixes
          libxi
          libxinerama
          libxmu
          libxpm
          libxtst
          libxv
        ];
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          lib = pkgs.lib;
        in
        rec {
          # The app itself: one Rust/Slint binary.
          linux-office-installer-suite-unwrapped = pkgs.rustPlatform.buildRustPackage {
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
              # Rust's unwinder leaves libgcc_s.so.1 in DT_NEEDED; autoPatchelfHook only
              # searches the lib dirs of the inputs, so it must be listed here or the fixup
              # fails with "auto-patchelf could not satisfy dependency libgcc_s.so.1".
              libgcc
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

          # The app also *runs* Wine: it checks the libraries Wine needs with `ldconfig -p`
          # and looks for cabextract/icoutils in PATH, and the Soda runner it downloads is a
          # prebuilt binary that expects a normal FHS layout. NixOS has no ldconfig,
          # /etc/ld.so.cache or 32-bit libraries, so the runnable package lives in an FHS
          # environment (the same way nixpkgs packages Bottles). `multiArch` gives 32-bit
          # Wine what it needs: every lib in `multiPkgs` is also provided for i686.
          linux-office-installer-suite = pkgs.buildFHSEnv {
            name = "linux-office-installer-suite";
            multiArch = true;
            targetPkgs =
              p:
              [
                linux-office-installer-suite-unwrapped
                p.cabextract
                p.icoutils # wrestool, icotool
              ];
            multiPkgs = winePkgs;
            runScript = "linux-office-installer-suite";
            inherit (linux-office-installer-suite-unwrapped) meta;
          };

          default = linux-office-installer-suite;
        }
      );
    };
}
