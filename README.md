# mods

Finished Rust ports of Minecraft mods for [moddedMC-rust](https://github.com/devmogged/moddedMC-rust), running on 26.3 through [our Pumpkin fork](https://github.com/devmogged/Pumpkin). Every mod here shows up in the Mods tab of the moddedMC app, where server owners install it with one click.

A port lands here only when it is done: its harness scenarios match the original mod, its performance gates pass, and everything in it may be published. Work in progress lives on branches in moddedMC-rust.

## Layout

```
mods/<id>/
  Cargo.toml      the mod crate: name = <id>, version = the mod's version, moddedmc-api.workspace = true
  mod.toml        the listing the app shows (below)
  icon.png        optional, 128x128, shown on the mod's card
  src/
  resources/data/    loads like a datapack
  resources/assets/  goes into the players' resource pack (publishable assets only)
  scenarios/      harness scenarios that compare against the original mod
  LICENSE         the original mod's code license, which the port keeps
Cargo.toml        the workspace; pins moddedmc-api
index.toml        generated; the app reads it. Never edit it by hand
scripts/publish.py
```

Assets under a restrictive license (textures, models and sounds of most mods) are not in this repo. They live in the private master list, `devmogged/mods-private`.

### mod.toml

```toml
name = "Saint's Dragons"
description = "Dragons that hatch, grow, fly and fight, with their own nests and loot."
license = "MIT"                 # the port's code license (the original's)
dependencies = []               # ids of other mods here; the app installs them too

[original]
name = "Saint's Dragons"
version = "0.9.88"              # the release the port follows (always the newest)
minecraft = "1.20.1"
url = "https://modrinth.com/mod/saints-dragons"
```

The id and version come from the crate's `Cargo.toml`, so they are written once.

## Publishing

Merging to `main` publishes. For every mod whose version has no release yet, [the Publish workflow](.github/workflows/publish.yml):

1. builds it for each server platform (`windows-x86_64`, `linux-x86_64`, `linux-aarch64`);
2. packs one `<id>-<version>.mmc`: `mod.json` (id, version, ABI, dependencies), a library per platform, and `resources/`;
3. releases it as `<id>-v<version>`;
4. regenerates `index.toml` with the download link, SHA-256 and size, and commits it.

To ship a new build, bump the version in the mod's `Cargo.toml`. Releases are never replaced, so a version that is out stays installable.

### index.toml

One `[[mods]]` table per published mod. The app verifies each download against `sha256` before installing it.

| Field | |
|---|---|
| `id`, `name`, `version`, `description`, `license` | from the crate and `mod.toml` |
| `kind` | `drop-in`: a `.mmc` file |
| `original`, `minecraft`, `upstream` | the ported release, its Minecraft version, its page |
| `dependencies` | mod ids installed along with it |
| `icon` | URL of `icon.png`, if there is one |
| `download`, `sha256`, `size` | the released `.mmc` |
