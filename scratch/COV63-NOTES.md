# cov63 lane: TMEM byte 0x04c invalid at texrect pixel (63,0)

Baseline (worktree @ 4371d57a, CARGO_TARGET_DIR=/private/tmp/fn64-cov63-target):
  `cargo nextest run -p fn64-render-wgpu` -> 4703 tests run: 4703 passed, 3 skipped

## Code read so far
- `tmem/read.rs:read_valid_byte` raises `InvalidTexelByte { address }` when
  `PhysicalTmemState::valid_byte` is None.
- LoadTile destination footprint comes from
  `tmem/types.rs:project_tmem_transfer_word`, `TmemLoadKind::Tile` arm:
      destination_word = descriptor.tmem() + row*line_words + within
      row = word / words_per_row ; within = word % words_per_row
  and `tmem/wire.rs:transfer_shape`'s Tile arm:
      words_per_row = ceil(texel_bytes(size, width) / 8)
      transfer_words = words_per_row * rows
- => the loaded SET is per-row runs of `words_per_row` words placed at a
  stride of `line_words`. When line_words > words_per_row the rows leave
  GAPS, so a contiguous "TMEM 0..1576" range description is not the
  loaded set.

## GROUND TRUTH (real ROM, FN64_COV63_DIAG=1)

[cov63] FAIL px=(63,0) s=4064 t=1536 fmt=IntensityAlpha siz=Bits4 line_words=5
        tmem_word=0 palette=0 mask_s=0 shift_s=0 mask_t=0 shift_t=0
        s_mode=clamp t_mode=clamp tile_size=(sl=252 tl=188 sh=512 th=384)
        lut=Rgba16 draw=(l=144 t=48 w=64 h=48)
        s_at(0)=2048 s_at(63)=4064 t_at(0)=1536 t_at(47)=3040

Valid-byte runs: strictly periodic, period 0x50 (80 B = 2 rows x line_words 5):
  +0x00..+0x20 (32B), +0x24..+0x26 (2B), +0x28..+0x4a (34B)

## ANSWER: NEITHER under-computed footprint NOR legitimate over-sample.
## It is a READER/WRITER ODD-ROW-PARITY INVERSION.

Writer (tmem/types.rs project_tmem_transfer_word, Tile arm):
    odd_row_exchange = (bounds.low_t().integer() + row) & 1
  applied in tmem/execute/load_tile.rs map_physical_lanes (Linear64 arm):
    physical[source_lane ^ (exchange*4)] = byte
  Here low_t.integer() = 188>>2 = 47 (ODD) => row 0 exchanges, row 1 does not.

Reader (tmem/read.rs odd_row_exchange):
    first_is_odd ^ (row & 1),  first_is_odd from request.first_row_parity()
  targets/texrect.rs:1048 hardcodes TmemFirstRowParity::Even
  => row 0 does NOT exchange, row 1 does. EXACTLY INVERTED.

Reproduced the observed valid set byte-for-byte with words_per_row=5,
rows=50, line_words=5, low_t_int=47, 34 defined bytes/row: model == observed,
zero symmetric difference, 1700 valid bytes.

Sweep of the whole 64x48 texrect against that valid set:
  first_row_parity = Even (current): 48/3072 invalid, FIRST = (63,0) -> 0x04c
  first_row_parity = Odd  (low_t=47): 0/3072 invalid
The Even model reproduces the production abort byte-for-byte.

Hand-derived for pixel (63,0):
  s=4064 (S10.5); rel = 4064 - 252*8 = 2048 -> texel column 64 (frac 0)
  t=1536;         rel = 1536 - 188*8 =   32 -> texel row 1
  4-bit: column_offset = 64/2 = 32
  linear = 0*8 + 1*5*8 + 32 = 72 = 0x048   <-- VALID (word 9 lane 0)
  reader XORs ^4 because row 1 is odd under first_row_parity=Even
        -> 0x04c = word 9 lane 4            <-- INVALID
  Under the writer's own parity (low_t 47 odd), row 1 does NOT exchange,
  so 0x048 is the correct address and it is loaded.

=> The validity guard is CORRECT and must not be weakened. The bug is the
   hardcoded TmemFirstRowParity::Even at targets/texrect.rs.
