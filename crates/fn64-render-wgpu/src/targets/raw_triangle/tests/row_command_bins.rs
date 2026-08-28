use super::*;
use crate::targets::{ColorCoverageState, ExactColorRowBandMut};

const MEMBERS: usize = 149;
const DRAWS: usize = 1_137;
const WIDTH: u32 = 480;
const HEIGHT: u32 = 240;

fn spanning_triangle(x: f64, y: i16) -> RawTriangle {
    use crate::rdp_harness::Tri;

    let words = Tri::flat()
        .left_major()
        .edges(x, x + 32.0)
        .rows(y..y + 9)
        .shade(
            [0x20_0000, 0x40_0000, 0x60_0000, 0xff_0000],
            [0; 4],
            [0; 4],
            [0; 4],
        )
        .texture_planes([1 << 9, 1 << 9, 1 << 20, 0], [0; 4], [0; 4], [0; 4])
        .words();
    let bytes = words
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect::<Vec<_>>();
    RawTriangle::decode(0x0e, &bytes).unwrap()
}

#[derive(Clone, Copy)]
struct FixtureTmem<'a> {
    source: &'a BenchTmem,
    deny_all: bool,
}

impl crate::TmemByteSource for FixtureTmem<'_> {
    fn snapshot(&self) -> crate::TmemSnapshotIdentity {
        self.source.snapshot()
    }

    fn valid_byte(&self, address: u16) -> Option<u8> {
        (!self.deny_all)
            .then(|| self.source.valid_byte(address))
            .flatten()
    }
}

struct FixtureDraw<'a> {
    triangle: RawTriangle,
    declared: Vec<fn64_render_ir::ResourceAccess>,
    source: FixtureTmem<'a>,
}

fn draws(sources: &'static [&'static BenchTmem]) -> Vec<FixtureDraw<'static>> {
    let key = key_at(WIDTH, HEIGHT);
    (0..DRAWS)
        .map(|index| {
            let y = ((index * 17 + index / 11 * 29) % (HEIGHT as usize - 10)) as i16;
            let x = ((index * 37 + index / 7 * 13) % (WIDTH as usize - 34)) as f64;
            let triangle = spanning_triangle(x, y);
            FixtureDraw {
                declared: declared_accesses(key, &triangle, None),
                triangle,
                source: FixtureTmem {
                    source: sources[index * sources.len() / DRAWS],
                    deny_all: false,
                },
            }
        })
        .collect()
}

fn prepare<'a>(
    candidate: &CandidateColorTarget,
    draws: &'a [FixtureDraw<'a>],
    resident_len: usize,
) -> Vec<PreparedRawTriangleRaster<'a, FixtureTmem<'a>>> {
    draws
        .iter()
        .map(|draw| {
            PreparedRawTriangleRaster::try_new_exact(
                candidate,
                OtherMode::from_wire(0x0018_acff, 0x0f0a_7008),
                &draw.triangle,
                TexrectShading::new(
                    CombineParams::from_wire(0xfc15_fea3, 0xf00f_f23f),
                    Color4::from_wire(0xffff_ffff),
                    PrimColor::from_wire(0, 0x4060_80fe),
                ),
                TexrectBlendRegisters::default(),
                &draw.declared,
                Some(RawTriangleTexture {
                    tile: coverage_fog_tile_binding(),
                    tmem: &draw.source,
                    lut_mode: crate::TextureLutMode::Rgba16,
                }),
                resident_len,
            )
            .unwrap()
        })
        .collect()
}

fn limits() -> Vec<usize> {
    (1..=MEMBERS)
        .map(|member| member * DRAWS / MEMBERS)
        .collect()
}

fn checkpoint_accesses(
    draws: &[FixtureDraw<'_>],
    limits: &[usize],
) -> Vec<Vec<fn64_render_ir::ResourceAccess>> {
    let mut first = 0;
    limits
        .iter()
        .map(|&end| {
            let accesses = draws[first..end]
                .iter()
                .flat_map(|draw| draw.declared.iter().copied())
                .collect();
            first = end;
            accesses
        })
        .collect()
}

fn initial_bytes() -> Vec<u8> {
    (0..WIDTH * HEIGHT)
        .flat_map(|pixel| {
            ((((pixel * 13) & 0x1f) << 11)
                | (((pixel * 7) & 0x1f) << 6)
                | (((pixel * 3) & 0x1f) << 1)
                | (pixel & 1))
                .to_be_bytes()[2..]
                .iter()
                .copied()
                .collect::<Vec<_>>()
        })
        .collect()
}

fn initial_coverage(key: ColorTargetKey, bytes: &[u8]) -> ColorCoverageState {
    let mut coverage = ColorCoverageState::unknown(key.extent());
    coverage.reconcile_unknown_visible(key, bytes);
    coverage
}

#[derive(Debug, PartialEq, Eq)]
struct Patch {
    access: fn64_render_ir::ResourceAccess,
    bytes: Vec<u8>,
    coverage: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
struct Output {
    checkpoints: Vec<Vec<Patch>>,
    bytes: Vec<u8>,
    coverage: ColorCoverageState,
}

fn capture(
    key: ColorTargetKey,
    accesses: &[fn64_render_ir::ResourceAccess],
    bytes: &[u8],
    coverage: &ColorCoverageState,
) -> Vec<Patch> {
    let base = key.address().get();
    accesses
        .iter()
        .copied()
        .map(|access| {
            let fn64_render_ir::ResourceRegion::Rdram { range, .. } = access.region() else {
                panic!("checkpoint access must name RDRAM")
            };
            let start = (range.start().get() - base) as usize;
            let end = start + range.len() as usize;
            Patch {
                access,
                bytes: bytes[start..end].to_vec(),
                coverage: coverage.cells[start / 2..end / 2].to_vec(),
            }
        })
        .collect()
}

fn scalar(
    key: ColorTargetKey,
    prepared: &[PreparedRawTriangleRaster<'_, FixtureTmem<'_>>],
    limits: &[usize],
    accesses: &[Vec<fn64_render_ir::ResourceAccess>],
    mut bytes: Vec<u8>,
    mut coverage: ColorCoverageState,
) -> Result<Output, (usize, TexrectExecutionError)> {
    let mut checkpoints = Vec::with_capacity(limits.len());
    let mut member = 0;
    for (draw, raster) in prepared.iter().enumerate() {
        raster
            .raster_rows(&mut bytes, &mut coverage, None, None, false)
            .map_err(|error| (draw, error))?;
        if limits[member] == draw + 1 {
            checkpoints.push(capture(key, &accesses[member], &bytes, &coverage));
            member += 1;
        }
    }
    Ok(Output {
        checkpoints,
        bytes,
        coverage,
    })
}

fn error_key(error: &(usize, TexrectExecutionError)) -> (usize, u32, u32) {
    match &error.1 {
        TexrectExecutionError::Sample { column, row, .. } => (error.0, *row, *column),
        _ => (error.0, u32::MAX, u32::MAX),
    }
}

fn binned(
    key: ColorTargetKey,
    prepared: &[PreparedRawTriangleRaster<'_, FixtureTmem<'_>>],
    limits: &[usize],
    accesses: &[Vec<fn64_render_ir::ResourceAccess>],
    bytes: Vec<u8>,
    coverage: ColorCoverageState,
    workers: usize,
) -> Result<Output, (usize, TexrectExecutionError)> {
    let output = execute_prepared_raw_triangle_row_bins(
        key, prepared, limits, accesses, bytes, coverage, workers,
    )?;
    assert!(output.band_jobs > 0);
    Ok(Output {
        checkpoints: output
            .checkpoints
            .into_iter()
            .map(|patches| {
                patches
                    .into_iter()
                    .map(|patch| Patch {
                        access: patch.access,
                        bytes: patch.bytes,
                        coverage: patch.coverage,
                    })
                    .collect()
            })
            .collect(),
        bytes: output.bytes,
        coverage: output.coverage,
    })
}

fn fixture() -> (
    ColorTargetKey,
    Vec<u8>,
    ColorCoverageState,
    Vec<FixtureDraw<'static>>,
) {
    let sources = Box::leak(
        (0..8)
            .map(|_| &*Box::leak(Box::new(BenchTmem::new())))
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    let key = key_at(WIDTH, HEIGHT);
    let bytes = initial_bytes();
    let coverage = initial_coverage(key, &bytes);
    (key, bytes, coverage, draws(sources))
}

#[test]
fn prepared_row_command_bins_match_all_checkpoints_and_final_state_for_2_4_6_8_workers() {
    let (key, bytes, coverage, draws) = fixture();
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let prepared = prepare(&candidate, &draws, bytes.len());
    let limits = limits();
    let accesses = checkpoint_accesses(&draws, &limits);
    let expected = scalar(
        key,
        &prepared,
        &limits,
        &accesses,
        bytes.clone(),
        coverage.clone(),
    )
    .unwrap();
    for workers in [2, 4, 6, 8] {
        assert_eq!(
            binned(
                key,
                &prepared,
                &limits,
                &accesses,
                bytes.clone(),
                coverage.clone(),
                workers
            )
            .unwrap(),
            expected,
            "workers={workers}"
        );
    }
}

#[test]
fn prepared_row_command_bins_reduce_errors_lexicographically_and_publish_nothing_on_failure() {
    let (key, bytes, coverage, mut draws) = fixture();
    draws[15].source.deny_all = true;
    draws[27].source.deny_all = true;
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let prepared = prepare(&candidate, &draws, bytes.len());
    let limits = limits();
    let accesses = checkpoint_accesses(&draws, &limits);
    let expected = scalar(
        key,
        &prepared,
        &limits,
        &accesses,
        bytes.clone(),
        coverage.clone(),
    )
    .unwrap_err();
    for workers in [2, 4, 6, 8] {
        let actual = binned(
            key,
            &prepared,
            &limits,
            &accesses,
            bytes.clone(),
            coverage.clone(),
            workers,
        )
        .unwrap_err();
        assert_eq!(error_key(&actual), error_key(&expected));
        assert_eq!(actual.1, expected.1);
    }
    assert_eq!(bytes, initial_bytes());
    assert_eq!(coverage, initial_coverage(key, &bytes));
}

#[test]
fn prepared_row_command_bin_prefix_retains_only_checkpoints_before_later_raster_error() {
    let (key, bytes, coverage, mut draws) = fixture();
    let failing_draw = 27;
    draws[failing_draw].source.deny_all = true;
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let prepared = prepare(&candidate, &draws, bytes.len());
    let limits = limits();
    let accesses = checkpoint_accesses(&draws, &limits);
    let completed_members = limits.partition_point(|limit| *limit <= failing_draw);
    let completed_draws = limits[completed_members - 1];
    let expected = scalar(
        key,
        &prepared[..completed_draws],
        &limits[..completed_members],
        &accesses[..completed_members],
        bytes.clone(),
        coverage.clone(),
    )
    .unwrap();

    let attempt = execute_prepared_raw_triangle_row_bin_prefix(
        key, &prepared, &limits, &accesses, bytes, coverage, 4,
    );
    assert_eq!(
        attempt.error.as_ref().map(|error| error.0),
        Some(failing_draw)
    );
    let checkpoints = attempt
        .checkpoints
        .into_iter()
        .map(|patches| {
            patches
                .into_iter()
                .map(|patch| Patch {
                    access: patch.access,
                    bytes: patch.bytes,
                    coverage: patch.coverage,
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(checkpoints, expected.checkpoints);
}

#[test]
#[should_panic(expected = "cannot mutate a different color target")]
fn prepared_triangle_rejects_a_row_band_for_another_exact_target_key() {
    let (key, _, _, draws) = fixture();
    let registry = ColorTargetRegistry::try_new(layout(), 1).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let prepared = prepare(&candidate, &draws[..1], key.range().len() as usize);
    let other_key = ColorTargetKey::try_new(
        layout().address(FIXTURE_START + 0x40000).unwrap(),
        key.extent(),
        key.format(),
    )
    .unwrap();
    let mut bytes = initial_bytes();
    let mut coverage = initial_coverage(other_key, &bytes);
    let mut band = ExactColorRowBandMut::from_full(other_key, 0..HEIGHT, &mut bytes, &mut coverage);
    let _ = prepared[0].raster_band(&mut band, None, false);
}
