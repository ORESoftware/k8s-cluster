# Nix development shell

From the repository root:

```sh
nix develop ./.nix
flags2env audit .cli-flags.toml
```

The flake pins the toolchain inputs through `.nix/flake.lock` and builds the
`flags2env` CLI/native library directly from
[oresoftware/flags-2-env](https://github.com/oresoftware/flags-2-env).

