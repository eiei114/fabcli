# fabcli

`fab` is a small Rust CLI for [Fab](https://www.fab.com/) (Epic Games' asset marketplace). It reuses
an already logged-in Chrome over the DevTools protocol, then calls Fab's own web API from inside the
page. No API key, no headless login flow, no credential storage: it borrows your browser session.

```console
$ fab status
ok: Eichanman37564 (signed in)

$ fab search "crate" --free --limit 3
5e5b85e0-b14f-457f-a3d9-91775ca93870  FREE  [blender,fbx,additional-files,glb]  Wooden crate
...

$ fab download 5e5b85e0-b14f-457f-a3d9-91775ca93870 --format glb --out ./crate
claimed: Wooden crate (CC-BY, free)
saved ./crate/source.zip (595709560 bytes)
record ./crate/FAB_SOURCE.json
ATTRIBUTION REQUIRED: "Wooden crate" by Krag Studio (https://www.fab.com/listings/5e5b85e0-...)
```

## Requirements

- Rust 1.85+ to build (edition 2024); the binary is self-contained
- A Chrome instance started with `--remote-debugging-port` and logged in to Fab

## Install

```console
cargo install --path .
# or
cargo build --release   # target/release/fab
```

The default CDP endpoint is `http://127.0.0.1:9223`; override it with `FAB_CDP`.

## Commands

| command | what it does |
| --- | --- |
| `fab login` | open the Fab login page in the CDP browser |
| `fab import-session <port> [--to 9223]` | copy Fab/Epic cookies from another CDP browser into this one |
| `fab status` | confirm the session is logged in |
| `fab library [--limit N] [--json]` | list owned assets (listing UID vs. library UID are distinct) |
| `fab search <words> [--free] [--format fbx] [--limit N] [--json]` | search listings |
| `fab show <uid\|url>` | formats, licenses, prices, ownership state |
| `fab claim <uid\|url> [--license personal]` | add a **free** license to your library |
| `fab formats <uid\|url>` | files available per format |
| `fab download <uid\|url> --format <fmt> [--out DIR] [--unzip] [--only glb]` | claim if needed, then download |

Formats are `fbx`, `glb`, `gltf`, `obj`, `blender`, `additional-files` and similar — whatever the
listing offers.

`fab download` refuses paid listings. `fab claim` only ever selects a license whose price is 0, and
there is no purchase code path at all.

Each download writes `FAB_SOURCE.json` next to the files with the source URL, license slug, CC0 and
attribution flags, and the AI-generated flag from the listing.

## Notes and caveats

- **`is_free` / FREE badges are unreliable.** Always confirm with `fab show <uid>` and read the
  `licenses[].price` values. Prices arrive as JSON numbers that may be floats.
- `isCc0: true` alone does not make a listing CC0 — the `slug` decides. `fab show` reports CC0 from
  the slug only.
- `FAB_SOURCE.json` records `licenseBasis: listing-free-license-not-entitlement`. That is candidate
  information from the listing, not a verified entitlement. Re-check against your library after
  claiming.
- `unreal-engine` formats cannot be downloaded over the web; use the Fab plugin inside the UE
  editor. Prefer FBX / GLB / Blender.
- `converted-files` GLBs are usually far smaller than the full Blender/OBJ bundles. Check whether
  the GLB embeds textures before treating it as a complete PBR set.
- Zip handling: extraction sanitises entry names, so an entry cannot escape the destination, and the
  expanded size is not trusted blindly — but treat third-party archives as untrusted input anyway.
- Fab's web API is undocumented and may change without notice.

## Development

```console
cargo test      # unit tests, no network
cargo clippy --all-targets
```

The CDP session is only exercised by hand, since it needs a logged-in browser.

## See also

- [sfabcli](https://github.com/eiei114/sfabcli) — the same approach for Sketchfab

## License

MIT
