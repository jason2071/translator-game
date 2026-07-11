# Vendored unrpyc

[unrpyc](https://github.com/CensoredUsername/unrpyc) is a Ren'Py script
decompiler by Yuri K. Schlesner, CensoredUsername, Jackmcbarn and contributors.
It is **MIT-licensed** (see `LICENSE`). We bundle it so the Ren'Py engine can
auto-decompile a game that ships only compiled `.rpyc` (no source `.rpy`) at
import time — see `engine/unrpyc.rs` and `engine::renpy::ensure_decompiled`.

## What's here

Two branches are vendored because the decompiler split when Ren'Py moved to
Python 3 in Ren'Py 8. The engine picks one by probing the game's *own* bundled
interpreter (`<game>/lib/py3-*` → v2, `<game>/lib/py2-*` → v1):

| Dir  | unrpyc branch | Python | Ren'Py support        |
|------|---------------|--------|-----------------------|
| `v2` | `master`      | 3.9+   | 8.x down to 6.18.0    |
| `v1` | `legacy`      | 2.7    | 7.x / 6.x             |

Only the CLI-decompile subset is vendored: `unrpyc.py` + `deobfuscate.py` (for
`--try-harder`) + the `decompiler/` package. The `testcases/`, `un.rpyc/`
injection payloads, README, and packaging files are intentionally dropped.

## Pinned versions

- `v2` — `CensoredUsername/unrpyc` `master` @ `3ae8334ed71a05535927dcc559663d3aca51215b`
- `v1` — `CensoredUsername/unrpyc` `legacy` @ `13f7e0ec56a7134a5afc89d70f3e48823e715f3e`

## Bumping

When a newer Ren'Py version ships `.rpyc` the pinned copy can't decompile,
refresh the vendor:

```sh
git clone --depth 1 https://github.com/CensoredUsername/unrpyc.git tmp-v2
git clone --depth 1 --branch legacy https://github.com/CensoredUsername/unrpyc.git tmp-v1
# for each: copy unrpyc.py, deobfuscate.py, decompiler/ into v2/ or v1/ (drop __pycache__)
# then update the pinned SHAs above and re-run `cargo test`.
```
