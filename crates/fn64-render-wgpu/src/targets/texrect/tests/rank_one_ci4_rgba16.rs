use super::*;
use crate::sample_point;
use fn64_render::{
    NeutralImageFormat, NeutralPixelSize, NeutralTileAddressMode, NeutralTileDescriptor,
    NeutralTileSize,
};
use fn64_render_ir::PhysicalMemoryLayout;
use std::hint::black_box;
use std::time::{Duration, Instant};

use crate::targets::{ColorTargetExtent, ColorTargetRegistry};

#[derive(Clone, Copy)]
struct AdmissionInputs {
    combine: CombineParams,
    other_mode: OtherMode,
    target_format: ColorTargetFormat,
    lut_mode: TextureLutMode,
    descriptor: NeutralTileDescriptor,
    size: NeutralTileSize,
    draw: TexrectDraw,
}

impl AdmissionInputs {
    fn tile(self) -> TexrectTileBinding {
        TexrectTileBinding::try_from_neutral(self.descriptor, self.size).unwrap()
    }

    fn admit(self) -> Option<RankOneCi4Rgba16> {
        RankOneCi4Rgba16::admit(
            self.combine,
            self.other_mode,
            self.target_format,
            self.lut_mode,
            self.tile(),
            self.draw,
        )
    }
}

fn exact_inputs() -> AdmissionInputs {
    AdmissionInputs {
        combine: CombineParams::from_wire(
            RankOneCi4Rgba16::COMBINE_LOW,
            RankOneCi4Rgba16::COMBINE_HIGH,
        ),
        other_mode: OtherMode::from_wire(
            RankOneCi4Rgba16::OTHER_MODE_HIGH,
            RankOneCi4Rgba16::OTHER_MODE_LOW,
        ),
        target_format: ColorTargetFormat::Rgba16,
        lut_mode: TextureLutMode::Rgba16,
        descriptor: NeutralTileDescriptor {
            format: NeutralImageFormat::ColorIndex,
            size: NeutralPixelSize::Bits4,
            line_words: 1,
            tmem_word_address: 0,
            palette: 0,
            s_mode: NeutralTileAddressMode::default(),
            mask_s: 4,
            shift_s: 0,
            t_mode: NeutralTileAddressMode::default(),
            mask_t: 4,
            shift_t: 0,
        },
        size: NeutralTileSize {
            low_s: 0,
            low_t: 0,
            high_s: 60,
            high_t: 60,
        },
        draw: TexrectDraw {
            left: 0,
            top: 0,
            right: 64,
            bottom: 64,
            s_start: 0,
            t_start: 0,
            s_end: 2048,
            t_end: 2048,
            flipped_axes: false,
        },
    }
}

#[test]
fn admission_is_closed_over_every_census_field_the_sampler_depends_on() {
    let exact = exact_inputs();
    assert_eq!(exact.admit(), Some(RankOneCi4Rgba16));
    let mut mutations = Vec::new();

    let mut input = exact;
    input.combine = CombineParams::from_wire(0, RankOneCi4Rgba16::COMBINE_HIGH);
    mutations.push(input);
    let mut input = exact;
    input.combine = CombineParams::from_wire(RankOneCi4Rgba16::COMBINE_LOW, 0);
    mutations.push(input);
    let mut input = exact;
    input.other_mode = OtherMode::from_wire(0, RankOneCi4Rgba16::OTHER_MODE_LOW);
    mutations.push(input);
    let mut input = exact;
    input.other_mode = OtherMode::from_wire(RankOneCi4Rgba16::OTHER_MODE_HIGH, 0);
    mutations.push(input);
    let mut input = exact;
    input.target_format = ColorTargetFormat::Rgba32;
    mutations.push(input);
    let mut input = exact;
    input.lut_mode = TextureLutMode::Ia16;
    mutations.push(input);

    let mut input = exact;
    input.descriptor.format = NeutralImageFormat::IntensityAlpha;
    mutations.push(input);
    let mut input = exact;
    input.descriptor.size = NeutralPixelSize::Bits8;
    mutations.push(input);
    let mut input = exact;
    input.descriptor.line_words = 2;
    mutations.push(input);
    let mut input = exact;
    input.descriptor.tmem_word_address = 1;
    mutations.push(input);
    let mut input = exact;
    input.descriptor.palette = 1;
    mutations.push(input);
    let mut input = exact;
    input.descriptor.s_mode.mirror = true;
    mutations.push(input);
    let mut input = exact;
    input.descriptor.s_mode.clamp = true;
    mutations.push(input);
    let mut input = exact;
    input.descriptor.mask_s = 3;
    mutations.push(input);
    let mut input = exact;
    input.descriptor.shift_s = 1;
    mutations.push(input);
    let mut input = exact;
    input.descriptor.t_mode.mirror = true;
    mutations.push(input);
    let mut input = exact;
    input.descriptor.t_mode.clamp = true;
    mutations.push(input);
    let mut input = exact;
    input.descriptor.mask_t = 3;
    mutations.push(input);
    let mut input = exact;
    input.descriptor.shift_t = 1;
    mutations.push(input);

    let mut input = exact;
    input.size.low_s = 4;
    mutations.push(input);
    let mut input = exact;
    input.size.low_t = 4;
    mutations.push(input);
    let mut input = exact;
    input.size.high_s = 56;
    mutations.push(input);
    let mut input = exact;
    input.size.high_t = 56;
    mutations.push(input);
    let mut input = exact;
    input.draw = input.draw.with_flipped_axes();
    mutations.push(input);

    assert_eq!(mutations.len(), 24);
    for (index, mutation) in mutations.into_iter().enumerate() {
        assert_eq!(mutation.admit(), None, "mutation {index} escaped admission");
    }
}

struct CorpusTmem {
    bytes: [u8; 4096],
    valid: [bool; 4096],
    snapshot: crate::TmemSnapshotIdentity,
}

impl CorpusTmem {
    fn complete() -> Self {
        let mut bytes = [0; 4096];
        for (address, byte) in bytes.iter_mut().enumerate() {
            *byte = (address as u8)
                .wrapping_mul(73)
                .wrapping_add((address >> 4) as u8)
                .wrapping_add(19);
        }
        let physical = crate::PhysicalTmemState::try_new().unwrap();
        Self {
            bytes,
            valid: [true; 4096],
            snapshot: crate::TmemByteSource::snapshot(&physical),
        }
    }
}

impl crate::TmemByteSource for CorpusTmem {
    fn snapshot(&self) -> crate::TmemSnapshotIdentity {
        self.snapshot
    }

    fn valid_byte(&self, address: u16) -> Option<u8> {
        let index = usize::from(address);
        self.valid[index].then_some(self.bytes[index])
    }
}

fn generic_sample(
    tmem: &CorpusTmem,
    tile: TexrectTileBinding,
    s: i16,
    t: i16,
) -> Result<[u8; 4], PointSampleError> {
    sample_point(
        tmem,
        tile.descriptor(),
        tile.size(),
        PointSampleRequest::new(
            PointSampleCoordinates::new(
                TextureCoordinateS10_5::from_raw(s),
                TextureCoordinateS10_5::from_raw(t),
            ),
            TmemFirstRowParity::Even,
        ),
        TextureLutMode::Rgba16,
    )
    .map(|sample| sample.texel().rgba8888())
}

#[test]
fn specialization_matches_the_generic_oracle_at_boundaries_and_mutations() {
    let mut tmem = CorpusTmem::complete();
    let tile = exact_inputs().tile();
    let specialized = RankOneCi4Rgba16;
    let boundaries = [
        i16::MIN,
        -1025,
        -513,
        -512,
        -511,
        -33,
        -32,
        -31,
        -1,
        0,
        1,
        31,
        32,
        33,
        479,
        480,
        481,
        511,
        512,
        513,
        i16::MAX,
    ];
    for &s in &boundaries {
        for &t in &boundaries {
            assert_eq!(
                specialized.sample(&tmem, s, t),
                generic_sample(&tmem, tile, s, t)
            );
        }
    }

    let mut state = 0x9e37_79b9u32;
    for _ in 0..50_000 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let s = state as i16;
        state = state.rotate_left(11).wrapping_add(0x7f4a_7c15);
        let t = state as i16;
        assert_eq!(
            specialized.sample(&tmem, s, t),
            generic_sample(&tmem, tile, s, t)
        );
    }

    for address in [0u16, 4, 127, 0x0800, 0x0878, 0x0879] {
        tmem.valid[usize::from(address)] = false;
        for &s in &boundaries {
            for &t in &boundaries {
                assert_eq!(
                    specialized.sample(&tmem, s, t),
                    generic_sample(&tmem, tile, s, t)
                );
            }
        }
        tmem.valid[usize::from(address)] = true;
    }
}

fn time_samples(
    generic: bool,
    tmem: &CorpusTmem,
    tile: TexrectTileBinding,
    coordinates: &[(i16, i16)],
) -> Duration {
    let started = Instant::now();
    let mut checksum = 0u64;
    for &(s, t) in coordinates {
        let rgba = if generic {
            generic_sample(tmem, tile, black_box(s), black_box(t)).unwrap()
        } else {
            RankOneCi4Rgba16
                .sample(tmem, black_box(s), black_box(t))
                .unwrap()
        };
        checksum = checksum.wrapping_add(u64::from(rgba[0]) + u64::from(rgba[3]));
    }
    black_box(checksum);
    started.elapsed()
}

fn full_draw(
    generic: bool,
    candidate: &CandidateColorTarget,
    tmem: &CorpusTmem,
    resident: &[u8],
) -> CompletedColorTargetWrite {
    let inputs = exact_inputs();
    let execute = || {
        execute_texture_rectangle(
            candidate,
            inputs.other_mode,
            inputs.draw,
            inputs.tile(),
            tmem,
            inputs.lut_mode,
            TexrectShading::new(
                inputs.combine,
                Color4::from_wire(0x2040_80ff),
                PrimColor::from_wire(0, 0x80ff_40ff),
            ),
            TexrectBlendRegisters::new(
                Color4::from_wire(0x1020_30ff),
                Color4::from_wire(0x4050_60ff),
            ),
            RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 256, 256),
            Cow::Borrowed(resident),
            None,
        )
        .unwrap()
    };
    if generic {
        with_generic_rank_one_for_test(execute)
    } else {
        execute()
    }
}

fn full_draw_fixture() -> (CandidateColorTarget, CorpusTmem, Vec<u8>) {
    let layout = PhysicalMemoryLayout::try_new(8 * 1024 * 1024).unwrap();
    let key = ColorTargetKey::try_new(
        layout.address(0x400).unwrap(),
        ColorTargetExtent::try_new(64, 64).unwrap(),
        ColorTargetFormat::Rgba16,
    )
    .unwrap();
    let registry = ColorTargetRegistry::try_new(layout, 1).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let resident = vec![0x5a; key.extent().pixels() as usize * 2];
    (candidate, CorpusTmem::complete(), resident)
}

#[test]
fn full_draw_device_bytes_match_the_forced_generic_oracle() {
    let (candidate, tmem, resident) = full_draw_fixture();
    let generic = full_draw(true, &candidate, &tmem, &resident);
    let specialized = full_draw(false, &candidate, &tmem, &resident);
    assert_eq!(
        specialized.device_bytes().device_bytes(),
        generic.device_bytes().device_bytes()
    );
    assert_eq!(specialized.rectangle(), generic.rectangle());

    // ------------------------------------------------------------------
    // Independent oracle (follow-up 5.1b).
    //
    // The two assertions above are lane-vs-lane: the specialization and its
    // forced-generic twin share the TMEM fetch, the TLUT lookup, the
    // combiner and the RGBA16 pack, so a drift in any shared subexpression
    // cancels on both sides. What follows is derived from the latched
    // register words alone, with no second run of this crate's raster.
    //
    // Combiner. `RankOneCi4Rgba16::COMBINE_LOW/HIGH` are
    // 0xfcff_ffff / 0xfffd_f6fb, and one-cycle mode reads the *second*-cycle
    // bitfield slice (`CombineParams::parse_*(second_cycle = true)`):
    //   color A = (low  >> 5)  & 0xF  = 15 -> `color_input_a`  8..=15 -> Zero
    //   color B = (high >> 24) & 0xF  = 15 -> `color_input_b`  8..=15 -> Zero
    //   color C =  low         & 0x1F = 31 -> `color_input_c` 16..=31 -> Zero
    //   color D = (high >> 6)  & 0x7  =  3 -> `color_input_d`      3  -> Primitive
    //   alpha A = (high >> 21) & 0x7  =  7 -> `alpha_input_abd`    7  -> Zero
    //   alpha B = (high >> 3)  & 0x7  =  7 -> Zero
    //   alpha C = (high >> 18) & 0x7  =  7 -> `alpha_input_c`      7  -> Zero
    //   alpha D =  high        & 0x7  =  3 -> Primitive
    // so `(A - B) * C + D` is `(0 - 0) * 0 + Primitive` in *both* the colour
    // and the alpha lane: the combined fragment is exactly the primitive
    // register, for every pixel, independent of the sampled texel. The
    // fixture latches `PrimColor::from_wire(0, 0x80ff_40ff)`, i.e.
    // RGBA = (0x80, 0xff, 0x40, 0xff).
    //
    // Terminal. `write_pixel` packs RGBA16 as
    //   (R >> 3) << 11 | (G >> 3) << 6 | (B >> 3) << 1 | (coverage.stored() >> 2) & 1
    //   R: 0x80 >> 3 = 16 = 0b10000
    //   G: 0xff >> 3 = 31 = 0b11111
    //   B: 0x40 >> 3 =  8 = 0b01000
    // giving RGB bits (16 << 11) | (31 << 6) | (8 << 1) = 0x87D0.
    //
    // Coverage bit. Other-mode low is 0x0050_41c8:
    //   IM_RD   = (low >> 6) & 1 = 1 (image read enabled)
    //   CVG_DST = (low >> 8) & 3 = 1 = Wrap
    //   CVG_X_ALPHA = (low >> 12) & 1 = 1, ALPHA_CVG_SEL = (low >> 13) & 1 = 0
    // A texrect fragment is `Coverage::FULL` = 8; `times_alpha(0xff)` is
    // `(8 * 255 + 127) / 255` = 8, so pixel coverage stays 8. The resident
    // is 0x5a5a, whose stored bit `dest[1] & 1` is 0, so
    // `blend_and_write_pixel` supplies `Coverage::new(1)` as the memory
    // count. Wrap with IM_RD set stores `sum - 8` when `sum > 8`:
    // sum = 8 + 1 = 9 > 8, so destination = 1, `stored()` = 0, and bit 2 of
    // 0 is 0. The packed word is therefore 0x87D0 with the coverage bit
    // clear -- big-endian bytes [0x87, 0xD0].
    //
    // The blender contributes nothing here: this is the whole-frame value,
    // so if any blend term were selected the result would vary with the
    // (constant) resident rather than equal the primitive exactly.
    const EXPECTED_PIXEL: [u8; 2] = [0x87, 0xD0];
    let bytes = specialized.device_bytes().device_bytes();
    assert_eq!(bytes.len(), 64 * 64 * 2, "the draw covers the whole target");
    for y in 0..64usize {
        for x in 0..64usize {
            let offset = (y * 64 + x) * 2;
            assert_eq!(
                [bytes[offset], bytes[offset + 1]],
                EXPECTED_PIXEL,
                "pixel ({x},{y}) must hold the hand-computed primitive-only value"
            );
        }
    }

    // Non-vacuity: the oracle above must not be satisfiable by "nothing
    // drawn". The resident is 0x5a everywhere and the expected word is not,
    // so a draw that wrote no pixel would fail the loop -- these assertions
    // state that directly, and pin the changed-pixel floor at the full
    // 64x64 rectangle the draw declares.
    assert_ne!(bytes, &resident[..], "the draw must write pixels");
    let changed = bytes
        .iter()
        .zip(resident.iter())
        .filter(|(drawn, seeded)| drawn != seeded)
        .count();
    assert_eq!(
        changed,
        64 * 64 * 2,
        "every byte of the declared rectangle must be written"
    );
}

fn time_full_draws(
    generic: bool,
    count: usize,
    candidate: &CandidateColorTarget,
    tmem: &CorpusTmem,
    resident: &[u8],
) -> Duration {
    let started = Instant::now();
    let mut checksum = 0u64;
    for iteration in 0..count {
        let completed = full_draw(generic, candidate, tmem, black_box(resident));
        let bytes = completed.device_bytes().device_bytes();
        checksum = checksum.wrapping_add(u64::from(bytes[iteration % bytes.len()]));
    }
    black_box(checksum);
    started.elapsed()
}

#[test]
#[ignore = "release-only alternating microbenchmark"]
fn release_microbenchmark_is_a_meaningful_win() {
    assert!(!cfg!(debug_assertions), "run this benchmark with --release");
    let tmem = CorpusTmem::complete();
    let tile = exact_inputs().tile();
    let coordinates = (0..250_000u32)
        .map(|index| {
            let s = index.wrapping_mul(73).wrapping_add(index >> 3) as i16;
            let t = index.wrapping_mul(151).wrapping_add(index >> 5) as i16;
            (s, t)
        })
        .collect::<Vec<_>>();
    let mut generic = Duration::ZERO;
    let mut specialized = Duration::ZERO;
    for round in 0..10 {
        if round & 1 == 0 {
            generic += time_samples(true, &tmem, tile, &coordinates);
            specialized += time_samples(false, &tmem, tile, &coordinates);
        } else {
            specialized += time_samples(false, &tmem, tile, &coordinates);
            generic += time_samples(true, &tmem, tile, &coordinates);
        }
    }
    eprintln!(
        "rank-one-ci4-rgba16 generic_ns={} specialized_ns={} speedup={:.2}x",
        generic.as_nanos(),
        specialized.as_nanos(),
        generic.as_secs_f64() / specialized.as_secs_f64()
    );
    assert!(
        specialized.as_nanos() * 10 < generic.as_nanos() * 9,
        "the specialization must save at least 10%: generic={generic:?}, specialized={specialized:?}"
    );
}

#[test]
#[ignore = "release-only alternating full-draw microbenchmark"]
fn release_full_draw_microbenchmark_is_a_meaningful_net_win() {
    assert!(!cfg!(debug_assertions), "run this benchmark with --release");
    let (candidate, tmem, resident) = full_draw_fixture();
    let mut generic = Duration::ZERO;
    let mut specialized = Duration::ZERO;
    for round in 0..10 {
        if round & 1 == 0 {
            generic += time_full_draws(true, 20, &candidate, &tmem, &resident);
            specialized += time_full_draws(false, 20, &candidate, &tmem, &resident);
        } else {
            specialized += time_full_draws(false, 20, &candidate, &tmem, &resident);
            generic += time_full_draws(true, 20, &candidate, &tmem, &resident);
        }
    }
    eprintln!(
        "rank-one-full-draw generic_ns={} specialized_ns={} speedup={:.2}x",
        generic.as_nanos(),
        specialized.as_nanos(),
        generic.as_secs_f64() / specialized.as_secs_f64()
    );
    assert!(
        specialized.as_nanos() * 100 < generic.as_nanos() * 95,
        "the full draw must save at least 5%: generic={generic:?}, specialized={specialized:?}"
    );
}
