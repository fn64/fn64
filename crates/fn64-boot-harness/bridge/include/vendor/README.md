# Vendored upstream headers

Not fn64 code. Do not edit these files — re-vendor them.

| File | Upstream | Commit | License |
|---|---|---|---|
| `recomp.h` | https://github.com/N64Recomp/N64Recomp.git (`include/recomp.h`) | `a8e2200471f37da298eebe2e11d949d3702649d0` (2026-07-14) | MIT, (c) 2024 Wiseguy — see `LICENSE-N64Recomp` |

## Why vendored rather than submoduled

`recomp.h` is the ABI contract the recompiler's generated C expects, and fn64
exists to serve it. It is 475 lines, MIT, and self-contained (system includes
only). One header does not earn a submodule — and the `c` lane it serves is
scheduled for retirement to CI-oracle-only (ROADMAP P1), so the dependency has
a known end date.

The honest cost: a copied file can drift from upstream invisibly, where a
submodule pins a commit and shows drift in `git status`. The commit above is
the mitigation — it makes drift *detectable*:

```sh
# Is the vendored copy still what upstream says at that commit?
git -C <N64Recomp checkout> show a8e2200:include/recomp.h | diff - recomp.h
```

If upstream ever changes this header, the divergence surfaces loudly anyway:
`crates/fn64-abi/tests/c_smoke.rs` compiles a real C caller against the
staticlib, so an ABI that no longer matches fails the link, not a review.

## Re-vendoring

Copy the file, update the commit in the table above, and run
`cargo nextest run -p fn64-abi` (the `c_smoke` link test is the gate).
`RECOMP_H_DIR` overrides this directory if you need to build against a
different fork without re-vendoring.
