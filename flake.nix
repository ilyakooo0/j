{
  description = "j — a functional-programming-centered CLI for Jujutsu repositories";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        packages = {
          j = pkgs.rustPlatform.buildRustPackage {
            pname = "j";
            version = "0.1.0";
            src = ./.;

            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            nativeBuildInputs = [ pkgs.makeWrapper ];

            # The test suite shells out to git and expects a writable temp dir
            # and network-free local remotes; it is not meant to run inside the
            # Nix build sandbox.
            doCheck = false;

            # The reference config.j ships with the binary; `j` looks for it at
            # <prefix>/share/j/config.j relative to its own executable.
            postInstall = ''
              install -Dm644 config.j $out/share/j/config.j

              # difft is a runtime dependency (§7.10): make sure `j` always
              # finds difftastic on PATH, without requiring it globally.
              wrapProgram $out/bin/j \
                --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.difftastic ]}
            '';

            meta = with pkgs.lib; {
              description = "A functional-programming-centered CLI for Jujutsu repositories";
              mainProgram = "j";
              license = licenses.mit;
              platforms = platforms.unix;
            };
          };

          default = self.packages.${system}.j;
        };

        apps = {
          j = {
            type = "app";
            program = "${self.packages.${system}.j}/bin/j";
          };
          default = self.apps.${system}.j;
        };
      });
}
