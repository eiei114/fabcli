# fabcli

`fab` is a small CLI for [Fab](https://www.fab.com/) (Epic Games' asset marketplace) that reuses an
already logged-in Chrome over the DevTools protocol, then calls Fab's own web API from inside the
page. No API key, no headless login flow, no credential storage: it borrows your browser session.

```console
$ fab status
ok: Keisu（ログイン済み）

$ fab search "wooden crate" --free --format fbx --limit 5
8f1c...  FREE  [fbx,gltf]  Old Wooden Crate
...

$ fab download <uid> --format fbx --out ./crate --unzip
claimed: Old Wooden Crate（Personal・無料）
saved ./crate/old_wooden_crate.zip (18432112 bytes)
unzipped → ./crate/old_wooden_crate
```

## Requirements

- Node.js 22+ (uses the built-in `fetch` and `WebSocket` globals)
- A Chrome instance started with `--remote-debugging-port` and logged in to Fab
- `curl` (downloading) and `unzip` (only with `--unzip`)

## Install

```console
git clone https://github.com/eiei114/fabcli
cd fabcli
npm link        # or: export PATH="$PWD:$PATH"
```

The default CDP endpoint is `http://127.0.0.1:9223`; override it with `FAB_CDP`.

## Commands

| command | what it does |
| --- | --- |
| `fab login` | open the Fab login page in the CDP browser |
| `fab import-session <port>` | copy Fab/Epic cookies from another CDP browser into the resident one |
| `fab status` | confirm the session is logged in |
| `fab library [--limit N] [--json]` | list owned assets (listing UID vs. library UID are distinct) |
| `fab search <words> [--free] [--format fbx] [--limit N] [--json]` | search listings |
| `fab show <uid\|url>` | formats, licenses, prices, ownership state |
| `fab claim <uid\|url> [--license personal]` | add a **free** license to your library |
| `fab formats <uid\|url>` | files available per format |
| `fab download <uid\|url> --format fbx\|glb\|gltf\|obj\|blender\|additional-files [--out DIR] [--unzip] [--only glb]` | claim if needed, then download |

`fab download` refuses paid listings. `fab claim` only ever selects a license whose price is 0.

Each download writes `FAB_SOURCE.json` next to the files with the source URL, license slug, CC0 /
attribution flags and the AI-generated flag taken from the listing.

## Notes and caveats

- **`is_free` / FREE badges are unreliable.** Always confirm with `fab show <uid>` and read the
  `licenses` prices (`personal: 0`, `cc-by: 0`, `cc0: 0`). `isCc0: true` alone does not make a
  listing CC0 — check `slug`.
- `FAB_SOURCE.json` records `licenseBasis: listing-free-license-not-entitlement`. That is candidate
  information from the listing, not a verified entitlement. Re-check against your library after
  claiming.
- `unreal-engine` formats cannot be downloaded over the web; use the Fab plugin inside the UE
  editor. Prefer FBX / GLB / Blender.
- `converted-files` GLBs are usually far smaller than the full Blender/OBJ bundles. Check whether
  the GLB embeds textures before treating it as a complete PBR set.
- Zip handling: verify CRCs, path traversal and expanded size before extracting. `--unzip` uses
  plain `unzip`; treat third-party archives as untrusted input.
- Buying is out of scope. No purchase code path exists.
- Fab's web API is undocumented and may change without notice.

## See also

- [sfabcli](https://github.com/eiei114/sfabcli) — the same approach for Sketchfab

## License

MIT
