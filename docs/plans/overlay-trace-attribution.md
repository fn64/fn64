# Overlay bank attribution for dynamic traces

## The blocker

`gate_decomp_functions` promotes an observed executed PC to an owner root when
execution proves the word runs and the structural entry test proves a function
begins there. That lane works for the boot bank (NWXE 725 -> 728, wrong=0) and
is wired for overlays, but overlays promote **zero**.

`tools/mupen-trace/mupen_trace.c` attributes a PC by a single hardcoded VA
window: `[0x80000400, 0x80056670)` becomes bank `boot`, everything else becomes
bank-unknown and is dropped at fold time. On a 40,000,000-record WM2000
capture that is 37,353,421 PCs labelled `boot` and **2,646,579 dropped**, many
of which executed in overlay VA space.

## Why the obvious fix does not work

Adding overlay VA windows (`FN64_OVERLAY_WINDOWS`, implemented and committed)
is fail-closed against slot aliasing: a window whose VA range another window
also claims is rejected, because VA alone cannot say which image is resident.

Measured, every AKI overlay shares a slot:

| game | slot | images |
|---|---|---|
| NWXE | `0x800e1b90` | `_0`, `_3` |
| NWXE | `0x8011c900` | `_1`, `_2` |
| NW4E | `0x800d9960` | `_0`, `_4` |
| NW4E | `0x80106760` | `_1`, `_2`, `_3` |

So **0 of 4 NWXE windows are admitted**, and NW4E would fare no better. This
is not a tuning problem: the engine reuses two slots for every image, so
address-based attribution is structurally impossible for this family.

## The design that does work: generation from observed loads

The active image is decided by the most recent overlay LOAD into the slot, and
that event is observable. `tools/mupen-trace/mupen_devtrace.c` already emits
PI DMA start/complete records; `fn64-discover` already proves each overlay's
`(rom_start, rom_end, va_start)` mapping (M1).

A PI DMA whose cart address matches a proven overlay's ROM range and whose
DRAM address matches that overlay's slot **identifies the resident image from
that moment until the next load into the same slot**. Attribution becomes:

1. maintain a per-slot "currently resident bank" that starts empty;
2. on a PI DMA matching exactly one proven overlay mapping, set that slot's
   resident bank and bump an activation counter;
3. attribute a PC in a slot to the currently resident bank, with that
   activation;
4. attribute nothing while a slot's resident image is unknown or ambiguous --
   before the first load, or if a DMA matches more than one mapping.

Step 4 is what keeps it sound: `BankContext::Known` already carries an
`activation`, so the trace can distinguish "bank X, activation 2" from an
earlier residency and the fold stays honest about which image ran.

## Work required

- Merge the PC producer and the device producer, or teach `mupen_trace.c` to
  watch PI DMA registers (it currently reads none).
- Pass proven overlay mappings to the producer the same way
  `FN64_OVERLAY_WINDOWS` passes windows -- data, never a built-in table.
- Fold-side: nothing. `fold_executed_pcs_into_fact_db` and both entry-root
  promotions already key on bank name and are already tested.

## Expected value

79% of the remaining AKI gap is in overlays: NWXE 443 open of 1,595, NW4E 596
of 2,455. The boot lane's own numbers suggest the conversion rate is modest
(17 roots from 5,279 observed words), so this is worth doing for coverage of a
much larger surface, not because overlays will convert better per word.
