# fn64 vs. jessetbh's AKI wrestling recomps

Measured 2026-08-06. External claims link to primary sources (repo READMEs,
per-title configs, issue trackers). Every fn64 claim carries `file:line` from
this worktree.

Licensing: this document reads and summarizes public *documentation, configs,
and issue text*. No source code from those projects was copied, transcribed,
or adapted. The AKI recomps are GPL-3.0 and fn64 is not; nothing here is a
design to lift, and the two trees must stay separate.

---

## Provenance: whose work is what

This matters and was initially unclear, so state it plainly.

- **jessetbh** is the author of the shipped AKI ports — six public repos, one
  per title, all created July 2026 ([WCW vs. nWo World
  Tour](https://github.com/jessetbh/WCWvsNWOWorldTourRecomp),
  [Revenge](https://github.com/jessetbh/WCWnWoRevengeRecomp),
  [WrestleMania 2000](https://github.com/jessetbh/WWFWrestleMania2000Recomp),
  [VPW64](https://github.com/jessetbh/VPW64Recomp),
  [No Mercy](https://github.com/jessetbh/WWFNoMercyRecomp),
  [VPW2](https://github.com/jessetbh/VPW2Recomp)), plus
  [AkiLauncher](https://github.com/jessetbh/AkiLauncher) as a hub and
  [WCWSyms](https://github.com/jessetbh/WCWSyms) for symbol metadata. These
  build on the Wiseguy N64Recomp stack (N64Recomp → C, N64ModernRuntime, RT64,
  RecompFrontend).
- **`~/Code/aki-recomp` is Jer's own repository, not jessetbh's.** All 70
  commits are authored by `Jer <jeremy.weiskotten@gmail.com>`, and it has **no
  git remote**. Its README describes it as a "profile-driven
  static-recompilation toolchain for the AKI Corporation N64 wrestling games …
  **derived from jessetbh's WCW recomps (GPL-3.0)**," and `PINS.md` pins the
  exact upstream commits it was derived from (`WCWnWoRevengeRecomp e74f0c1`,
  `WCWvsNWOWorldTourRecomp 2048b15`, `WCWSyms 2211e04`). Its `AGENTS.md` names
  fn64 as "the runtime it will stand on."

So the local checkout is **a third thing**: Jer's GPL-3.0 downstream fork
exploring mechanical profile generation, sitting between jessetbh's hand-ground
ports and fn64. When this document says "they," it means **jessetbh's shipped
ports**, the actual comparison target.

Background framing (relevant but not the body): fn64 also consumes
N64Recomp-generated C as an oracle lane in `recomps/wm2000/packages/oot-boot`,
`recomps/wm2000/packages/sm64-boot`, and `recomps/wm2000/packages/wm2000-boot`. The general N64Recomp
lineage's constraint — it requires an ELF from a decompilation project
([README](https://github.com/N64Recomp/N64Recomp#how-to-use)) — is the backdrop
against which the AKI work is impressive, because **no public decomp of these
games exists**.

---

## The honest bottom line

**They have six shipped, playable AKI ports. We have one title that boots and
renders and is not yet playable.** These are the same five games (plus VPW64),
on the same engine, toward the same goal — and they finished.

Every jessetbh AKI port is at **beta with full matches, sound, input, menus,
and persistent saves**. World Tour is described as "fully playable end to end —
boots, renders, full matches with sound, keyboard + gamepad input, menus, local
multiplayer, rumble, and persistent saves"
([README](https://github.com/jessetbh/WCWvsNWOWorldTourRecomp)). Their issue
trackers are nearly empty — across all six repos there are **four** issues
total, and the substantive ones are cosmetic (story-mode text offset and
scaling in No Mercy, [#4](https://github.com/jessetbh/WWFNoMercyRecomp/issues/4),
[#3](https://github.com/jessetbh/WWFNoMercyRecomp/issues/3)). That is a
finished-product bug profile.

fn64's five AKI titles pass *CPU recompilation only*, which
`docs/plans/aki-recompile-certification.md:12-16` is explicit does **not**
prove a booting game: "RSP audio and RDP graphics are separate subsystems and
the gate never consults host bindings." WM2000 boots and renders but is not
playable — `recomps/wm2000/docs/wm2000-playable-blocker-ledger.md:1-5` frames playability
as the *goal*, with a live journal blocker still open.

**The uncomfortable part:** the per-title manual effort we are trying to
eliminate turns out to be *small* on this engine. jessetbh's WM2000 and No
Mercy configs are ~2.7 KB each with ~18 stubs and 1–2 instruction patches, and
their `patches/` directories are **2,198 bytes**. Our zero-config thesis is
aimed at a cost that, for AKI titles, a skilled human already drove near zero
by hand. The thesis has to justify itself on *breadth beyond this family*, not
on saving effort here.

---

## 1. Per-title status across the five AKI titles

From each README's own status line and Known Issues.

| Title | fn64 | jessetbh |
|---|---|---|
| WWF WrestleMania 2000 | CPU recomp `unsupported=0`; boots + renders; **not playable** | **Beta, playable.** Boots, renders, music + SFX, keyboard/gamepad, rumble, cart save **and Controller Pak** persist ([README](https://github.com/jessetbh/WWFWrestleMania2000Recomp)) |
| WWF No Mercy | CPU recomp `unsupported=0`; no boot | **Beta, playable.** Full matches; flash cart save persists ([README](https://github.com/jessetbh/WWFNoMercyRecomp)) |
| WCW/nWo Revenge | CPU recomp `unsupported=0`; no boot | **Beta, playable.** Saves persist ([README](https://github.com/jessetbh/WCWnWoRevengeRecomp)) |
| WCW vs. nWo World Tour | CPU recomp `unsupported=0`; no boot | **Beta, "fully playable end to end,"** incl. 4-player local multiplayer ([README](https://github.com/jessetbh/WCWvsNWOWorldTourRecomp)) |
| Virtual Pro Wrestling 2 | CPU recomp `unsupported=0` (cold, first attempt); no boot | **Beta, playable.** Full matches, clean post-match transitions, SRAM saves ([README](https://github.com/jessetbh/VPW2Recomp)) |
| *(VPW64 — not in fn64's set)* | — | **Beta, playable** ([README](https://github.com/jessetbh/VPW64Recomp)) |

fn64 column verified at `docs/plans/aki-recompile-certification.md:18-24` (all
five PASS, WM2000 43,032 blocks / VPW2 49,329 / No Mercy 57,284 / Revenge
25,057 / World Tour 25,375) and `recomps/wm2000/docs/wm2000-playable-blocker-ledger.md`.

### Their known-broken subsystems

The most valuable content, because these are the *same subsystems we must
cross*:

- **Frame interpolation is disabled on all four early titles** — the single
  most-repeated limitation. The AKI engine "builds each visual frame from
  several RSP tasks and submits fully composed matrices, which defeats RT64's
  frame-interpolation heuristics — interpolated frames warp geometry."
  High-framerate support "needs game-side matrix-group patches" and is
  *planned, not done*
  ([World Tour](https://github.com/jessetbh/WCWvsNWOWorldTourRecomp#known-issues),
  repeated verbatim in Revenge, WM2000, VPW64).
- **Multi-controller instability** — WM2000: "Multi-controller (2+ player)
  sessions are still being stabilized and can crash in certain modes"; No
  Mercy: same, "single controller (or keyboard) play is solid."
- **Rumble is player-1-only** in World Tour, "matching the game's own pak
  handling."
- **VPW2**: rumble and Controller Pak "have not been through a full
  verification pass yet"; CI not yet wired up.
- **VPW64**: the title screen misreports pak status (a virtual-pak probe
  artifact, saves still work).
- **Platform**: Windows only across the board; Linux is *planned* on every
  repo. macOS is not offered anywhere.
- **Mod support**: planned, not shipped, on all titles.

---

## 2. Per-title manual effort — quantified

Measured via the GitHub contents/trees API, 2026-08-06.

| Artifact | WrestleMania 2000 | No Mercy |
|---|---|---|
| Recompiler config | `wm2k.toml` **2,729 B** | `nomercy.toml` **2,707 B** |
| `patches/` | **2,198 B**, 3 files | **2,198 B**, 3 files |
| `syms/` (generated metadata) | 178,996 B, 4 files | 239,180 B, 4 files |
| `tools/` (generation pipeline) | 85,369 B, 21 files | 117,098 B, 28 files |
| `disasm/` (splat config) | 11,075 B, 5 files | 12,354 B, 5 files |
| `src/` | 91,134 B, 4 files (mostly `main.cpp` 51 KB + a vendored 36 KB header) | 1,346,455 B — but ~1.25 MB is **vendored rcheevos** (RetroAchievements), not port code |

The contrast with Zelda64Recomp is the headline: **~800 KB of hand-written C
patches for Majora's Mask versus 2.2 KB for WrestleMania 2000.** Two reasons,
both instructive. First, MM's bulk is *enhancement* (widescreen, autosave, HFR,
mod hooks). Second, and more importantly: **the AKI ports substitute mechanical
metadata generation for hand-written patches.** The `tools/` + `syms/` +
`disasm/` triad (≈275 KB for WM2000) is a *generation pipeline*, not
hand-authored per-function work.

This is the finding that most complicates fn64's positioning, so state it
carefully. From the [VPW2 FAQ](https://github.com/jessetbh/VPW2Recomp):

> no public decompilation of Virtual Pro Wrestling 2 exists. Unlike most
> recompilation ports, which borrow symbol names from a decomp, this project
> generates its own symbol metadata from scratch via a
> [splat](https://github.com/ethteck/splat) disassembly (see `disasm/` and
> `syms/`), transferring libultra identifications from its sister projects WWF
> No Mercy and WWF WrestleMania 2000 by machine-code fingerprinting.

**They solved the no-decomp problem too** — by splat disassembly plus
cross-title machine-code fingerprinting. That is a *different* mechanism from
fn64's proof-carrying discovery, and it is not zero-config (it needs a splat
yaml, a stub-candidate scan, and per-title verification), but it is far cheaper
than a decomp and it demonstrably shipped six games.

### What the manual work actually consists of

From [`wm2k.toml`](https://github.com/jessetbh/WWFWrestleMania2000Recomp/blob/main/wm2k.toml):

- **19 stubs** in WM2000, 17 in No Mercy — generated by `tools/gen_stubs.py`
  under the rule "functions containing cop0/cache/eret/tlb opcodes are
  stubbed," described in the config as a "first-recompile bootstrap." The
  config notes these "move to RENAMEs … as identification proceeds."
- **1 ignored symbol** (`osDriveRomInit`, routed to a runtime shim).
- **2 hand-written single-instruction patches** in WM2000, **1** in No Mercy.
- A separate RSP audio microcode config (`rsp/wm2k_audio.toml`, 936 B).

So the honest bill per AKI title is roughly: a splat disassembly config, a
generated symbol dump, ~18 machine-generated stubs, 1–2 hand-authored
instruction patches, an RSP audio ucode config, and a `main.cpp`. Call it
**days-to-weeks of expert work per title**, dropping sharply for each
additional sibling as fingerprinting transfers identifications forward.

**Verdict on effort: NOT the decisive advantage we assumed.** fn64's
`gate_rom_recompile` needs one env var and no per-game constants
(`crates/fn64-discover/src/bin/gate_rom_recompile.rs:22`, and `:1-8` contrasts
itself with the hand-configured `gate_wm2000_recompile`). That is genuinely
less input than a splat yaml plus symbol dump. But their cost is already low
enough that eliminating it does not by itself justify the approach — **and
their low cost buys a playable game, while ours buys a passing gate.**

---

## 3. AKI-engine-specific problems they documented

The highest-value section: these are problems fn64 must also cross, with their
diagnoses already public.

### 3a. The idle-thread busy-spin that starves the cooperative scheduler

Documented identically in WM2000's and No Mercy's configs. After the main
thread launches all threads and drops its own priority to 0, it parks in a
`jal …; j …` busy loop that "never yields under ultramodern's cooperative
scheduler, starving message delivery." The measured signature quoted in
`nomercy.toml` is precise: "ext backlog +60/s, del/s=0, vis/s=0."

Their fix: rewrite the backward `j` into a self-branch so N64Recomp emits
`pause_self()`. **This is an AKI-family-wide defect** — the config says it
mirrors "every sister's documented patch."

**Does fn64 handle it? YES, and better — without a patch.**
`crates/fn64-recomp-rs-codegen/src/emit/mod.rs:1394-1400` detects a `j`/`b` to
its own address and emits `pause_self()` directly. But note the crucial
difference: **their fix requires an instruction patch because the loop is not
already self-referential** — it is a two-instruction `jal`/`j` pair. Our
detection at `:1395` matches `target == Some(vram)`, i.e. a *literal* self-loop.
**Inference, labeled:** fn64 would likely hit the same starvation on this exact
AKI pattern, because the guest loop calls a function and then jumps backward
rather than branching to itself. This is a concrete, checkable risk, and it is
recommendation #3 below.

### 3b. The music-sequence assert that parks a thread forever

From `wm2k.toml`: `func_80003DD4(out, id)` resolves a song id; its invalid-id
branch is a `j .` self-hang. Recompiled, that "recompiles to pause_self and
parks the calling thread FOREVER (observed boot36/38: gfx frozen, audio
alive)." Root cause was a music-slot init/use race that only opened under the
scheduler starvation above; their fix rewrites the assert to resolve as song 1,
reasoning that "a silent permanent park is the worst possible failure mode."

This is a **direct consequence of 3a's mechanism**: turning self-loops into
`pause_self` converts guest asserts into silent hangs. fn64 has the same
emission rule at `emit/mod.rs:1394-1400` and therefore inherits the same hazard.

### 3c. The stub-list false-positive problem — independently found by both

Their `gen_stubs.py` rule ("cop0/cache/eret/tlb opcodes → stub") is a
heuristic, and `nomercy.toml` records the scar tissue: "**NEVER stub
osGetCount** — it freezes the game clock and timed waits never complete."

fn64 measured the same class independently. `docs/ROADMAP.md` V1a documents
that the OoT lane's 127 stubs come from a "blunt bootstrap heuristic" (the
script's own words) where branch-guarded `break` opcodes — compiler
divide-by-zero asserts — are false positives, silently no-oping
`Letterbox_Update`, `Interface_Draw`, `Camera_Normal1`, `KaleidoScope_Update`.

**fn64 recovers automatically:** `compiler_div_guards_only()` at
`crates/fn64-recomp-rs-codegen/src/bin/recompile_rom.rs:538` disassembles each
stub candidate and auto-un-stubs when its only traps are guarded div/overflow
asserts (`auto_div_guards`, `:284-303`; tests `:974-997`). Jer's own
`~/Code/aki-recomp` git log shows the same fight by hand — commits `9f06f81`
"un-stub the four div-guard siblings of the music interpreter" and `a4068d1`
"un-stub 16 audio driver funcs (div-guard break false positive)".

**This is a genuine fn64 advantage on a problem all three projects hit.**

### 3d. The composed-matrix / multi-RSP-task frame structure

The engine "builds each visual frame from several RSP tasks and submits fully
composed matrices." This defeats RT64's interpolation heuristics and blocks
high-framerate support family-wide, needing "game-side matrix-group patches."

fn64 has not reached this problem — WM2000 renders (1,707 graphics submits on
a 420k-step route, `recomps/wm2000/docs/wm2000-playable-blocker-ledger.md:14-18`) but
has no interpolation work. **Their documented failure is a preview of ours.**

### 3e. Save media differs per title

Documented per README: WM2000 uses cart save **plus Controller Pak**; No Mercy
uses **flash**; VPW2 uses **SRAM** ("256 Kbit, confirmed during bring-up — the
ROM contains no flash driver"); VPW64 and World Tour use Controller Pak. So
"the AKI engine" is *not* uniform in save media — a per-title discovery
obligation fn64 must also meet.

### 3f. Overlay structure — confirmed, and it matches fn64's model

`wm2k.toml` states WM2000 has "FOUR overlays across two swap slots," and
`nomercy.toml` describes "multiple overlays across shared vram swap slots
(**9-word descriptor table** in fixed-segment data)."

**This independently corroborates fn64's overlay recovery.**
`crates/fn64-discover/src/overlay_recipe.rs:1-10` describes promoting exactly a
"complete nine-word layout … ROM bounds, loaded image start, text/data cache
extents, and the zeroed BSS extent," and only "when every word is present and
all independent range equations agree." Two projects independently identifying
the same nine-word descriptor is strong mutual validation — **and fn64 recovers
it from ROM bytes where they hand-declared it in a splat yaml.**

---

## 4. Ahead / behind / parity — AKI titles specifically

### BEHIND — decisively, on the thing that matters

- **Shipped playable titles: 0 vs 6.** Not a nuance. They finished the exact
  games we are working on.
- **Renderer/HFR**: they ship RT64 at high resolution with widescreen; we
  consume RT64 and track gaps (`docs/RT64-GAP-REGISTER.md`). They are blocked
  only on *interpolation*; we are blocked on playability.
- **Input**: they ship keyboard + gamepad, rebinding, 4-player local
  multiplayer, a 20 KB controller DB, and AKI-tailored default mappings. Our
  controller input is a scheduled replay file — without
  `FN64_CONTROLLER_SCHEDULE` "every controller read returned
  `ContInput::default()`" (`recomps/wm2000/docs/wm2000-playable-blocker-ledger.md:11-14`).
  That is a test harness, not player input.
- **Saves**: they persist cart/flash/SRAM/Controller Pak per title. fn64 has
  no shipped save persistence for these titles.
- **Audio**: they ship music and SFX in full matches. fn64's
  `recomps/wm2000/docs/WM2000-AUDIO-STATUS.md:1-4` is a live status doc, with the RSP audio
  stack working end-to-end in `fn64-shell` but not certified in a playable game.
- **Distribution**: SHA256-verified releases, a launcher, ROM intake
  validation, RetroAchievements integration in No Mercy. We have gates.
- **Per-title effort**: theirs is already small (§2). We are not saving as much
  as assumed.

### AHEAD — narrower than I would have claimed before this research

- **Input independence.** One env var
  (`gate_rom_recompile.rs:22`) versus a splat yaml + generated symbol dump +
  stub candidate list. Real, but a difference of *degree*, not kind — they also
  solved no-decomp titles ([VPW2 FAQ](https://github.com/jessetbh/VPW2Recomp)).
- **Automatic stub false-positive recovery** (§3c) —
  `recompile_rom.rs:538`. They hand-maintain the exception list
  ("NEVER stub osGetCount"); we detect and recover mechanically.
- **The mutation journal.** Eight closed writer channels at
  `crates/fn64-cpu-runtime/src/runtime/host.rs:141-152`, where undeclared writes
  to watched executable ranges fail the next dispatch
  (`crates/fn64-runtime/src/executor/mod.rs:193-195`). It caught a real bug:
  `mirror_queue_to_rdram` writing an `OSMesgQueue` field undeclared at step
  ~1,183,304 (`recomps/wm2000/docs/wm2000-playable-blocker-ledger.md:41-45`). **No
  equivalent exists in their stack** — I searched READMEs, configs, and issues
  and found none. *Inference, labeled:* an unjournaled executable write in
  their model would silently run stale translated code.
- **Byte-exact ROM rebuild.** `crates/fn64-discover/src/bin/gate_rom_rebuild.rs:1-24`
  — "The oracle is the byte. No answer key, dump, or symbol file is read"
  (`:24`). Structurally impossible in a pipeline whose ground truth is a
  hand-built symbol dump.
- **Overlay recovery from bytes** rather than declaration (§3f).
- **Breadth beyond the family.** 17 of 26 sampled corpus ROMs certify cold
  including GoldenEye (`docs/plans/corpus-certification-frontier.md:12-19`).
  Their pipeline is AKI-specific by construction. **This is where the
  zero-config thesis actually pays, and it is the argument to lead with.**

### PARITY / N/A

- **Overlay handling**: both handle the AKI nine-word descriptor correctly;
  different inputs, same result. **PARITY.**
- **Self-loop → cooperative yield**: same mechanism, same inherited hazard
  (§3a/§3b). **PARITY**, with a caveat we should verify.
- **RSP audio microcode**: both recompile it (their `rsp/*_audio.toml`; our
  `crates/fn64-audio/src/rsp/`). Ours is clean-room MIT by necessity
  (`recomps/wm2000/docs/WM2000-AUDIO-STATUS.md:7-11`) since we cannot depend on GPL-3.0
  `librecomp/rsp.hpp`. **PARITY on capability, N/A on approach.**
- **Frame interpolation / HFR**: neither has it. **PARITY, both absent.**
- **Mod support**: planned by them, not attempted by us. **N/A.**
- **Output language**: C vs Rust. **N/A.**

---

## 5. Design lessons (prose only)

### Lesson 1 — Turning self-loops into `pause_self` converts guest asserts into silent hangs

Their music-assert case (§3b) is the cleanest statement of a trap fn64 shares
by construction. A guest `j .` used as an assert is *intended* to hang the
console visibly; recompiled to a cooperative yield it becomes a thread parked
forever while the rest of the game keeps running — "gfx frozen, audio alive,"
the hardest failure to diagnose because it looks like a rendering bug.

**Do we handle it? NO — we have the same emission rule and no mitigation.**
`crates/fn64-recomp-rs-codegen/src/emit/mod.rs:1394-1400` emits `pause_self()`
for any self-targeting `j`/`b`. We should distinguish *idle loops* (reached
with other threads runnable) from *assert loops* (reached once, never exited),
and at minimum make a thread that parks in `pause_self` and is never rescheduled
a **loud** event rather than a silent one. This fits our existing
fail-loud discipline and is cheap.

### Lesson 2 — Opcode-scan stub heuristics are wrong often enough to need mechanical recovery

Three independent projects hit this: jessetbh's "NEVER stub osGetCount," fn64's
ROADMAP V1a letterbox finding, and Jer's aki-recomp un-stub commits. The
general rule: **presence of a privileged opcode is not evidence a function is
unrecompilable**, because compilers emit guarded traps on ordinary paths.

**Do we handle it? YES, partially.** `recompile_rom.rs:538` auto-un-stubs
div/overflow-guard false positives. But that covers the `break` class
specifically; their rule also stubs on `cop0`/`cache`/`eret`/`tlb`. Worth
checking whether a *branch-guarded* cop0 or cache op — plausible in
cache-writeback helpers — would be over-stubbed by us too. Related residual:
`docs/WAVE-STATUS.md:33` still reports a `semantic.rs:241` unsupported-recorder
bypass, a known hole in the loudness guarantee.

### Lesson 3 — The AKI engine's composed-matrix frame structure will block our HFR work too

Documented family-wide (§3d): several RSP tasks per visual frame with
fully-composed matrices submitted, defeating interpolation heuristics and
requiring "game-side matrix-group patches."

**Do we handle it? NOT YET, and we have not reached it.** We render (1,707
submits, `wm2000-playable-blocker-ledger.md:14-18`) but have done no
interpolation work. The lesson is *predictive*: when we get to HFR, this is
already known to be a game-side structural problem, not a renderer setting. It
also suggests a fn64-shaped opportunity — matrix *groups* are a static
structure in the display-list builder, so our whole-program analysis may be
able to identify them where a runtime heuristic cannot.

### Further lessons (recorded, not top-3)

- **Cross-title fingerprinting transfers identifications cheaply.** Their VPW2
  port bootstrapped libultra identifications from No Mercy and WM2000 by
  machine-code fingerprinting ([VPW2
  FAQ](https://github.com/jessetbh/VPW2Recomp)). fn64's discovery is per-ROM;
  a corroboration signal across sibling ROMs could raise confidence on
  ambiguous boundaries. Our memory already notes corpus homology work
  (`gate_corpus_homology`) — worth connecting.
- **Save media varies per AKI title** (§3e) — cart, flash, SRAM, Controller
  Pak. Do not assume family uniformity when we get to save persistence.
- **Multi-controller is where their polish ends** — crashes in 2+ player modes
  on WM2000 and No Mercy. If we ever reach parity, this is an area where
  careful device-fabric modeling (SI/PI) could plausibly beat them.

---

## 6. What this suggests we do next — prioritized

1. **Make WM2000 playable, or stop claiming the AKI family as the proving
   ground.** Six shipped ports on these exact games means "we target AKI
   titles" is no longer a differentiator by itself. The blocker is concrete and
   diagnosed (`wm2000-playable-blocker-ledger.md:41-45`, fixed at
   `executor/mod.rs:694-703`) — finish it.

2. **Re-position the thesis around breadth, not effort.** §2 shows per-title
   AKI cost is already small for an expert. The defensible claim is the corpus
   result: 17/26 sampled ROMs cold, including GoldenEye
   (`corpus-certification-frontier.md:12-19`), where their pipeline is
   AKI-specific by construction. Attack the 35% tail — three named failure
   classes, and `OutsideAllMappings` spans four unrelated engines (`:32-35`),
   so it is one missing capability, not five special cases.

3. **Audit the `pause_self` self-loop rule against the AKI idle pattern**
   (Lesson 1 + §3a). Two concrete checks: (a) does our `target == Some(vram)`
   test at `emit/mod.rs:1395` even fire on WM2000's `jal`/`j`-backward idle
   loop, or would we starve exactly as they did? (b) make a permanently-parked
   thread loud. Cheap, and it targets a defect they proved is family-wide.

4. **Verify our stub recovery covers cop0/cache/eret/tlb guards**, not just
   div-guard `break` (Lesson 2), and close the `semantic.rs:241` bypass
   (`docs/WAVE-STATUS.md:33`).

5. **Keep the mutation journal and byte-exact rebuild prominent.** These are
   our only genuinely unmatched capabilities (§4) and the strongest argument
   that fn64 is a different kind of artifact rather than a slower one.

6. **Do not build launcher/mods/HFR/input polish.** They are years ahead and we
   would be racing on their strengths using their renderer.

---

## Summary table — AKI titles specifically

| Axis | Verdict |
|---|---|
| Shipped playable AKI titles (0 vs 6) | **BADLY BEHIND** |
| Audio / input / saves / multiplayer in-game | **BEHIND** |
| Platform coverage (Windows shipped; Linux planned) | **BEHIND** |
| Distribution, launcher, RetroAchievements | **BEHIND** |
| Per-title configuration cost | **MARGINALLY AHEAD** (their cost already low) |
| Input independence (ROM vs splat+syms) | **AHEAD (degree, not kind)** |
| Stub false-positive auto-recovery | **AHEAD** |
| Mutation journal / executable-write safety | **AHEAD** (no counterpart found) |
| Byte-exact rebuild proof | **AHEAD** (structurally unavailable to them) |
| Overlay recovery from bytes vs declaration | **AHEAD** |
| Breadth beyond the AKI family | **AHEAD** — the real differentiator |
| AKI nine-word overlay descriptor handling | **PARITY** (mutual validation) |
| Self-loop → cooperative yield | **PARITY** (shared hazard) |
| RSP audio microcode recompilation | **PARITY** |
| Frame interpolation / HFR | **PARITY** (both absent) |
| Mod support | **N/A** |

---

## Sources

- jessetbh's AKI ports: [WCW vs. nWo World Tour](https://github.com/jessetbh/WCWvsNWOWorldTourRecomp), [WCW/nWo Revenge](https://github.com/jessetbh/WCWnWoRevengeRecomp), [WWF WrestleMania 2000](https://github.com/jessetbh/WWFWrestleMania2000Recomp), [Virtual Pro Wrestling 64](https://github.com/jessetbh/VPW64Recomp), [WWF No Mercy](https://github.com/jessetbh/WWFNoMercyRecomp), [Virtual Pro Wrestling 2](https://github.com/jessetbh/VPW2Recomp)
- [AkiLauncher](https://github.com/jessetbh/AkiLauncher), [WCWSyms](https://github.com/jessetbh/WCWSyms)
- Per-title configs: [`wm2k.toml`](https://github.com/jessetbh/WWFWrestleMania2000Recomp/blob/main/wm2k.toml), [`nomercy.toml`](https://github.com/jessetbh/WWFNoMercyRecomp/blob/main/nomercy.toml)
- Issue trackers: [No Mercy #3](https://github.com/jessetbh/WWFNoMercyRecomp/issues/3), [#4](https://github.com/jessetbh/WWFNoMercyRecomp/issues/4), [World Tour #1](https://github.com/jessetbh/WCWvsNWOWorldTourRecomp/issues/1)
- Upstream stack: [N64Recomp](https://github.com/N64Recomp/N64Recomp), [N64ModernRuntime](https://github.com/N64Recomp/N64ModernRuntime), [RT64](https://github.com/rt64/rt64), [splat](https://github.com/ethteck/splat)
- Local (Jer's own, GPL-3.0, no remote): `~/Code/aki-recomp` — `README.md`, `AGENTS.md`, `PINS.md`, `docs/BOOT-LADDER-PLAYBOOK.md`
- Repo/tree byte counts measured via the GitHub contents and trees APIs, 2026-08-06.
