# mods

Finished Rust ports of Minecraft mods for [moddedMC-rust](https://github.com/devmogged/moddedMC-rust), running on 26.3 through [our Pumpkin fork](https://github.com/devmogged/Pumpkin).

A port lands here only when it is done: its harness scenarios match the original mod, its performance gates pass, and everything in it may be published. Work in progress lives on branches in moddedMC-rust.

## Layout

```
<mod-id>/
  Cargo.toml      the mod crate (depends on moddedmc-api)
  mod.toml        manifest: id, version, original mod and version, code license, needs-companion
  src/
  scenarios/      harness scenarios that compare against the original mod
  LICENSE         the original mod's code license, which the port keeps
index.toml        generated list of every mod here, read by the app
```

Assets under a restrictive license (textures, models and sounds of most mods) are not in this repo. They live in the private master list, `devmogged/mods-private`.
