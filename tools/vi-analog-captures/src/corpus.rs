use super::{
    digest, validate_vector, Blob, ConsoleRegion, DigitalVector, FramebufferEncoding,
    ValidationError, VectorFramebuffer, ViField, ViFilters, ViPixelType, ViRegisters,
    DIGITAL_VECTOR_SCHEMA,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

pub const DIGITAL_CORPUS_SCHEMA: &str = "fn64.vi-digital-corpus.v1";
pub const NTSC_SYNTHETIC_CORPUS_ID: &str = "fn64-public-vi-ntsc-v1";

const FRAMEBUFFER_EXTENT: u32 = 36;
const LOGICAL_SOURCE_START: u32 = 3;
const LOGICAL_SOURCE_EXTENT: u32 = 32;
const DEFAULT_OUTPUT_EXTENT: u32 = 8;
const BOUNDARY_OUTPUT_EXTENT: u32 = 9;
const ORIGIN: u32 = 0x0010_0000;
const PIXEL_RGBA16: u32 = 2;
const GAMMA_DITHER: u32 = 1 << 2;
const GAMMA: u32 = 1 << 3;
const DIVOT: u32 = 1 << 4;
const SERRATE: u32 = 1 << 6;
const DITHER_FILTER: u32 = 1 << 16;
const IDENTITY_SCALE: u32 = 0x0400;
const BASE_OFFSET: u32 = LOGICAL_SOURCE_START << 10;
const BASE_IDENTITY_SCALE: u32 = (BASE_OFFSET << 16) | IDENTITY_SCALE;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorpusObjective {
    NtscProgressive,
    NtscInterlacedEven,
    NtscInterlacedOdd,
    PartialCoverageAa,
    FullCoverageRestoration,
    DitherFilterDisabled,
    DitherFilterEnabled,
    DivotDisabled,
    DivotEnabled,
    GammaDisabled,
    GammaEnabled,
    GammaDitherDisabled,
    GammaDitherEnabled,
    IdentityResampling,
    FractionalOffset,
    MinimumNonzeroScale,
    MaximumNonzeroScale,
    ExactLastSample,
    BeyondActiveWindow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigitalCorpusVector {
    pub vector_id: String,
    pub path: String,
    pub byte_len: u64,
    pub sha256: String,
    pub objectives: Vec<CorpusObjective>,
    pub fetch_footprint: FetchFootprint,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchFootprint {
    pub output_width: u32,
    pub output_height: u32,
    pub logical_source_x: SourceSpan,
    pub logical_source_y: SourceSpan,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpan {
    pub start: u32,
    pub extent: u32,
    pub leading_guard: u32,
    pub trailing_guard: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigitalCorpusIndex {
    pub schema: String,
    pub corpus_id: String,
    pub content_class: String,
    pub region: ConsoleRegion,
    pub vectors: Vec<DigitalCorpusVector>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedDigitalVector {
    pub vector: DigitalVector,
    pub artifact: DigitalCorpusVector,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedDigitalCorpus {
    pub index: DigitalCorpusIndex,
    pub index_bytes: Vec<u8>,
    pub vectors: Vec<GeneratedDigitalVector>,
}

#[derive(Copy, Clone)]
enum CoveragePattern {
    Full,
    Silhouette,
}

#[derive(Clone)]
struct VectorSpec {
    id: &'static str,
    status: u32,
    current: u32,
    field: ViField,
    x_scale: u32,
    y_scale: u32,
    output_width: u32,
    output_height: u32,
    coverage: CoveragePattern,
    objectives: &'static [CorpusObjective],
}

pub fn generate_digital_vector_corpus(
    region: ConsoleRegion,
) -> Result<GeneratedDigitalCorpus, ValidationError> {
    if region != ConsoleRegion::Ntsc {
        return Err(ValidationError::new(format!(
            "digital corpus generation for {region:?} is unsupported: no PAL/MPAL register preset is established by the allowed local evidence"
        )));
    }

    let mut vectors = Vec::new();
    for spec in ntsc_specs() {
        let vector = vector_from_spec(&spec);
        validate_vector(&vector)?;
        let mut bytes = serde_json::to_vec_pretty(&vector)
            .map_err(|error| ValidationError::new(format!("serialize digital vector: {error}")))?;
        bytes.push(b'\n');
        let path = format!("vectors/{}.json", spec.id);
        let artifact = DigitalCorpusVector {
            vector_id: spec.id.to_owned(),
            path,
            byte_len: bytes.len() as u64,
            sha256: digest(&bytes),
            objectives: spec.objectives.to_vec(),
            fetch_footprint: fetch_footprint(&spec),
        };
        validate_fetch_footprint(&vector, &artifact)?;
        vectors.push(GeneratedDigitalVector {
            vector,
            artifact,
            bytes,
        });
    }

    let index = DigitalCorpusIndex {
        schema: DIGITAL_CORPUS_SCHEMA.to_owned(),
        corpus_id: NTSC_SYNTHETIC_CORPUS_ID.to_owned(),
        content_class: "synthetic_vi_probe".to_owned(),
        region,
        vectors: vectors.iter().map(|item| item.artifact.clone()).collect(),
    };
    validate_index(&index)?;
    let mut index_bytes = serde_json::to_vec_pretty(&index)
        .map_err(|error| ValidationError::new(format!("serialize corpus index: {error}")))?;
    index_bytes.push(b'\n');
    Ok(GeneratedDigitalCorpus {
        index,
        index_bytes,
        vectors,
    })
}

impl GeneratedDigitalCorpus {
    /// Writes the already-validated corpus into a new directory. Refusing an
    /// existing path prevents stale vectors from surviving a regeneration.
    pub fn write_new(&self, output: &Path) -> Result<(), ValidationError> {
        if output.exists() {
            return Err(ValidationError::new(format!(
                "corpus output {} already exists; choose a new path",
                output.display()
            )));
        }
        fs::create_dir_all(output.join("vectors")).map_err(|error| {
            ValidationError::new(format!(
                "create corpus output {}: {error}",
                output.display()
            ))
        })?;
        for vector in &self.vectors {
            fs::write(output.join(&vector.artifact.path), &vector.bytes).map_err(|error| {
                ValidationError::new(format!(
                    "write corpus vector {}: {error}",
                    vector.artifact.path
                ))
            })?;
        }
        fs::write(output.join("corpus.json"), &self.index_bytes).map_err(|error| {
            ValidationError::new(format!("write corpus index {}: {error}", output.display()))
        })
    }
}

fn validate_index(index: &DigitalCorpusIndex) -> Result<(), ValidationError> {
    if index.vectors.is_empty() {
        return Err(ValidationError::new("digital corpus must contain vectors"));
    }
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut objectives = BTreeSet::new();
    for vector in &index.vectors {
        if !ids.insert(&vector.vector_id) {
            return Err(ValidationError::new(format!(
                "digital corpus duplicates vector_id {:?}",
                vector.vector_id
            )));
        }
        if !paths.insert(&vector.path) {
            return Err(ValidationError::new(format!(
                "digital corpus duplicates path {:?}",
                vector.path
            )));
        }
        if vector.byte_len == 0 || vector.sha256.len() != 64 {
            return Err(ValidationError::new(format!(
                "digital corpus vector {:?} has invalid artifact identity",
                vector.vector_id
            )));
        }
        objectives.extend(vector.objectives.iter().cloned());
    }
    let expected = all_objectives().into_iter().collect::<BTreeSet<_>>();
    if objectives != expected {
        return Err(ValidationError::new(
            "digital corpus does not cover its complete declared objective set",
        ));
    }
    Ok(())
}

fn fetch_footprint(spec: &VectorSpec) -> FetchFootprint {
    let span = SourceSpan {
        start: LOGICAL_SOURCE_START,
        extent: LOGICAL_SOURCE_EXTENT,
        leading_guard: LOGICAL_SOURCE_START,
        trailing_guard: FRAMEBUFFER_EXTENT - LOGICAL_SOURCE_START - LOGICAL_SOURCE_EXTENT,
    };
    FetchFootprint {
        output_width: spec.output_width,
        output_height: spec.output_height,
        logical_source_x: span.clone(),
        logical_source_y: span,
    }
}

fn validate_fetch_footprint(
    vector: &DigitalVector,
    artifact: &DigitalCorpusVector,
) -> Result<(), ValidationError> {
    let footprint = &artifact.fetch_footprint;
    if vector.framebuffer.width != FRAMEBUFFER_EXTENT
        || vector.framebuffer.height != FRAMEBUFFER_EXTENT
        || vector.framebuffer.row_stride_bytes != FRAMEBUFFER_EXTENT * 2
        || vector.registers.width != FRAMEBUFFER_EXTENT
    {
        return Err(ValidationError::new(format!(
            "corpus vector {:?} does not bind the complete guarded framebuffer",
            artifact.vector_id
        )));
    }
    validate_active_window(
        "H_START",
        vector.registers.h_start,
        footprint.output_width,
        1,
    )?;
    validate_active_window(
        "V_START",
        vector.registers.v_start,
        footprint.output_height,
        2,
    )?;
    validate_source_span("X", &footprint.logical_source_x, vector.framebuffer.width)?;
    validate_source_span("Y", &footprint.logical_source_y, vector.framebuffer.height)?;

    let exact = artifact
        .objectives
        .contains(&CorpusObjective::ExactLastSample);
    let beyond = artifact
        .objectives
        .contains(&CorpusObjective::BeyondActiveWindow);
    if exact && beyond {
        return Err(ValidationError::new(format!(
            "corpus vector {:?} conflates exact-last and beyond-active objectives",
            artifact.vector_id
        )));
    }
    let x_fetch = validate_axis_fetch(
        "X",
        vector.registers.x_scale,
        footprint.output_width,
        &footprint.logical_source_x,
        vector.framebuffer.width,
        exact,
        beyond,
    )?;
    let y_fetch = validate_axis_fetch(
        "Y",
        vector.registers.y_scale,
        footprint.output_height,
        &footprint.logical_source_y,
        vector.framebuffer.height,
        exact,
        beyond,
    )?;
    validate_filter_neighbors(vector, artifact, x_fetch, y_fetch, exact || beyond)
}

fn validate_active_window(
    label: &str,
    register: u32,
    output_extent: u32,
    units_per_pixel: u32,
) -> Result<(), ValidationError> {
    if output_extent == 0 {
        return Err(ValidationError::new(format!(
            "corpus {label} output extent is zero"
        )));
    }
    let start = register >> 16;
    let end = register & 0xffff;
    let expected = output_extent
        .checked_mul(units_per_pixel)
        .and_then(|extent| start.checked_add(extent))
        .ok_or_else(|| ValidationError::new(format!("corpus {label} extent overflow")))?;
    if end != expected {
        return Err(ValidationError::new(format!(
            "corpus {label} active window {start}..{end} does not bind {output_extent} output pixels"
        )));
    }
    Ok(())
}

fn validate_source_span(
    axis: &str,
    span: &SourceSpan,
    framebuffer_extent: u32,
) -> Result<(), ValidationError> {
    let total = span
        .leading_guard
        .checked_add(span.extent)
        .and_then(|value| value.checked_add(span.trailing_guard))
        .ok_or_else(|| ValidationError::new(format!("corpus {axis} source span overflow")))?;
    if span.extent == 0
        || span.start != span.leading_guard
        || total != framebuffer_extent
        || span.trailing_guard == 0
    {
        return Err(ValidationError::new(format!(
            "corpus {axis} source span does not bind its leading/source/trailing storage"
        )));
    }
    Ok(())
}

fn validate_axis_fetch(
    axis: &str,
    register: u32,
    output_extent: u32,
    source: &SourceSpan,
    framebuffer_extent: u32,
    exact: bool,
    beyond: bool,
) -> Result<(u32, u32), ValidationError> {
    let step = u64::from(register & 0x0fff);
    let offset = u64::from((register >> 16) & 0x0fff);
    if step == 0 {
        return Err(ValidationError::new(format!(
            "corpus {axis} scale must be nonzero"
        )));
    }
    let last_position = offset
        .checked_add(u64::from(output_extent - 1) * step)
        .ok_or_else(|| ValidationError::new(format!("corpus {axis} position overflow")))?;
    let source_first = u64::from(source.start) << 10;
    let source_last_coordinate = source.start + source.extent - 1;
    let source_last = u64::from(source_last_coordinate) << 10;
    if offset < source_first {
        return Err(ValidationError::new(format!(
            "corpus {axis} fetch begins before the declared logical source"
        )));
    }

    let fetch_first = offset >> 10;
    // Conservatively bind the adjacent sample even at a zero fraction. The
    // public linear topology names both inputs but does not promise a
    // fraction-zero fetch optimization in silicon.
    let fetch_last = (last_position >> 10) + 1;
    if fetch_last >= u64::from(framebuffer_extent) {
        return Err(ValidationError::new(format!(
            "corpus {axis} fetch reaches unbound framebuffer coordinate {fetch_last}"
        )));
    }
    if exact {
        if last_position != source_last {
            return Err(ValidationError::new(format!(
                "corpus {axis} exact-last probe ends at U2.10 {last_position:#x}, not logical last {source_last:#x}"
            )));
        }
    } else if beyond {
        if last_position <= source_last || fetch_last != u64::from(source.start + source.extent) {
            return Err(ValidationError::new(format!(
                "corpus {axis} beyond-active probe does not land solely in the bound trailing guard"
            )));
        }
    } else if fetch_last > u64::from(source.start + source.extent - 1) {
        return Err(ValidationError::new(format!(
            "corpus {axis} normal fetch escapes the declared logical source"
        )));
    }
    Ok((fetch_first as u32, fetch_last as u32))
}

fn validate_filter_neighbors(
    vector: &DigitalVector,
    artifact: &DigitalCorpusVector,
    x_fetch: (u32, u32),
    y_fetch: (u32, u32),
    boundary_probe: bool,
) -> Result<(), ValidationError> {
    let coverage = super::decode_blob(
        "digital corpus coverage",
        &vector.framebuffer.coverage_counts,
    )?;
    let has_partial = coverage.iter().any(|&count| count < 8);
    if boundary_probe && (has_partial || vector.filters.dither_filter || vector.filters.divot) {
        return Err(ValidationError::new(format!(
            "corpus boundary vector {:?} must isolate resampling with full coverage and neighbor filters disabled",
            artifact.vector_id
        )));
    }

    let framebuffer_width = vector.framebuffer.width;
    let framebuffer_height = vector.framebuffer.height;
    for (pixel, &count) in coverage.iter().enumerate() {
        if count == 8 {
            continue;
        }
        let x = pixel as u32 % framebuffer_width;
        let y = pixel as u32 / framebuffer_width;
        let y_radius = if matches!(vector.field, ViField::Progressive) {
            1
        } else {
            2
        };
        if x < 2 || x + 2 >= framebuffer_width || y < y_radius || y + y_radius >= framebuffer_height
        {
            return Err(ValidationError::new(format!(
                "corpus partial-coverage pixel {pixel} has an unbound AA neighbor footprint"
            )));
        }
    }

    if vector.filters.dither_filter {
        validate_neighbor_interval("dither X", x_fetch, 1, framebuffer_width)?;
        validate_neighbor_interval("dither Y", y_fetch, 1, framebuffer_height)?;
    }
    if vector.filters.divot {
        validate_neighbor_interval("divot X", x_fetch, 1, framebuffer_width)?;
    }
    Ok(())
}

fn validate_neighbor_interval(
    label: &str,
    interval: (u32, u32),
    radius: u32,
    framebuffer_extent: u32,
) -> Result<(), ValidationError> {
    if interval.0 < radius || interval.1.saturating_add(radius) >= framebuffer_extent {
        return Err(ValidationError::new(format!(
            "corpus {label} neighbor footprint escapes declared framebuffer storage"
        )));
    }
    Ok(())
}

fn active_window(start: u32, output_extent: u32, units_per_pixel: u32) -> u32 {
    let end = start
        .checked_add(
            output_extent
                .checked_mul(units_per_pixel)
                .expect("corpus active-window extent overflow"),
        )
        .expect("corpus active-window end overflow");
    (start << 16) | end
}

fn in_filter_safe_source(coordinate: u32) -> bool {
    let first = LOGICAL_SOURCE_START + 2;
    let end = LOGICAL_SOURCE_START + LOGICAL_SOURCE_EXTENT - 2;
    (first..end).contains(&coordinate)
}

fn vector_from_spec(spec: &VectorSpec) -> DigitalVector {
    let coverage = coverage_bytes(spec.coverage);
    let contents = rgba16_bytes(&coverage);
    DigitalVector {
        schema: DIGITAL_VECTOR_SCHEMA.to_owned(),
        vector_id: spec.id.to_owned(),
        content_class: "synthetic_vi_probe".to_owned(),
        framebuffer: VectorFramebuffer {
            encoding: FramebufferEncoding::Rgba16BigEndian,
            width: FRAMEBUFFER_EXTENT,
            height: FRAMEBUFFER_EXTENT,
            row_stride_bytes: FRAMEBUFFER_EXTENT * 2,
            contents: blob(contents),
            coverage_counts: blob(coverage),
        },
        registers: ViRegisters {
            status: spec.status,
            origin: ORIGIN,
            width: FRAMEBUFFER_EXTENT,
            intr: 2,
            current: spec.current,
            burst: 0x03e5_2239,
            v_sync: 525,
            h_sync: 0x0c15,
            leap: 0x0c15_0c15,
            h_start: active_window(108, spec.output_width, 1),
            v_start: active_window(37, spec.output_height, 2),
            v_burst: 0x000e_0204,
            x_scale: spec.x_scale,
            y_scale: spec.y_scale,
        },
        filters: ViFilters {
            pixel_type: ViPixelType::Rgba16,
            gamma: spec.status & GAMMA != 0,
            gamma_dither: spec.status & GAMMA_DITHER != 0,
            divot: spec.status & DIVOT != 0,
            dither_filter: spec.status & DITHER_FILTER != 0,
        },
        region: ConsoleRegion::Ntsc,
        field: spec.field.clone(),
    }
}

fn blob(bytes: Vec<u8>) -> Blob {
    Blob {
        byte_len: bytes.len() as u64,
        sha256: digest(&bytes),
        bytes_hex: bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
    }
}

fn coverage_bytes(pattern: CoveragePattern) -> Vec<u8> {
    (0..FRAMEBUFFER_EXTENT)
        .flat_map(|y| {
            (0..FRAMEBUFFER_EXTENT).map(move |x| match pattern {
                CoveragePattern::Full => 8,
                CoveragePattern::Silhouette
                    if in_filter_safe_source(x)
                        && in_filter_safe_source(y)
                        && (x == y || x + 1 == y) =>
                {
                    ((x + y * 3) % 7 + 1) as u8
                }
                CoveragePattern::Silhouette => 8,
            })
        })
        .collect()
}

fn rgba16_bytes(coverage: &[u8]) -> Vec<u8> {
    coverage
        .iter()
        .enumerate()
        .flat_map(|(pixel, &count)| {
            let x = pixel as u16 % FRAMEBUFFER_EXTENT as u16;
            let y = pixel as u16 / FRAMEBUFFER_EXTENT as u16;
            let red = (x * 31 / (FRAMEBUFFER_EXTENT as u16 - 1)) & 0x1f;
            let green = (y * 31 / (FRAMEBUFFER_EXTENT as u16 - 1)) & 0x1f;
            let blue = (x * 5 + y * 3) & 0x1f;
            let visible_coverage = u16::from((count - 1) >> 2);
            ((red << 11) | (green << 6) | (blue << 1) | visible_coverage).to_be_bytes()
        })
        .collect()
}

fn all_objectives() -> Vec<CorpusObjective> {
    use CorpusObjective::*;
    vec![
        NtscProgressive,
        NtscInterlacedEven,
        NtscInterlacedOdd,
        PartialCoverageAa,
        FullCoverageRestoration,
        DitherFilterDisabled,
        DitherFilterEnabled,
        DivotDisabled,
        DivotEnabled,
        GammaDisabled,
        GammaEnabled,
        GammaDitherDisabled,
        GammaDitherEnabled,
        IdentityResampling,
        FractionalOffset,
        MinimumNonzeroScale,
        MaximumNonzeroScale,
        ExactLastSample,
        BeyondActiveWindow,
    ]
}

fn ntsc_specs() -> Vec<VectorSpec> {
    use CorpusObjective::*;
    let progressive = PIXEL_RGBA16;
    vec![
        spec(
            "field-progressive",
            progressive,
            (ViField::Progressive, 0),
            (BASE_IDENTITY_SCALE, BASE_IDENTITY_SCALE),
            CoveragePattern::Silhouette,
            &[NtscProgressive],
        ),
        spec(
            "field-interlaced-even",
            progressive | SERRATE,
            (ViField::InterlacedEven, 0),
            (BASE_IDENTITY_SCALE, BASE_IDENTITY_SCALE),
            CoveragePattern::Silhouette,
            &[NtscInterlacedEven],
        ),
        spec(
            "field-interlaced-odd",
            progressive | SERRATE,
            (ViField::InterlacedOdd, 1),
            (BASE_IDENTITY_SCALE, BASE_IDENTITY_SCALE),
            CoveragePattern::Silhouette,
            &[NtscInterlacedOdd],
        ),
        spec(
            "partial-aa-dither-filter-off",
            progressive,
            (ViField::Progressive, 0),
            (BASE_IDENTITY_SCALE, BASE_IDENTITY_SCALE),
            CoveragePattern::Silhouette,
            &[PartialCoverageAa, DitherFilterDisabled],
        ),
        spec(
            "partial-aa-dither-filter-on",
            progressive | DITHER_FILTER,
            (ViField::Progressive, 0),
            (BASE_IDENTITY_SCALE, BASE_IDENTITY_SCALE),
            CoveragePattern::Silhouette,
            &[PartialCoverageAa, DitherFilterEnabled],
        ),
        spec(
            "full-coverage-restoration-off",
            progressive,
            (ViField::Progressive, 0),
            (BASE_IDENTITY_SCALE, BASE_IDENTITY_SCALE),
            CoveragePattern::Full,
            &[FullCoverageRestoration, DitherFilterDisabled],
        ),
        spec(
            "full-coverage-restoration-on",
            progressive | DITHER_FILTER,
            (ViField::Progressive, 0),
            (BASE_IDENTITY_SCALE, BASE_IDENTITY_SCALE),
            CoveragePattern::Full,
            &[FullCoverageRestoration, DitherFilterEnabled],
        ),
        spec(
            "divot-off",
            progressive,
            (ViField::Progressive, 0),
            (BASE_IDENTITY_SCALE, BASE_IDENTITY_SCALE),
            CoveragePattern::Silhouette,
            &[DivotDisabled],
        ),
        spec(
            "divot-on",
            progressive | DIVOT,
            (ViField::Progressive, 0),
            (BASE_IDENTITY_SCALE, BASE_IDENTITY_SCALE),
            CoveragePattern::Silhouette,
            &[DivotEnabled],
        ),
        spec(
            "gamma-off-gamma-dither-off",
            progressive,
            (ViField::Progressive, 0),
            (BASE_IDENTITY_SCALE, BASE_IDENTITY_SCALE),
            CoveragePattern::Full,
            &[GammaDisabled, GammaDitherDisabled],
        ),
        spec(
            "gamma-on-gamma-dither-off",
            progressive | GAMMA,
            (ViField::Progressive, 0),
            (BASE_IDENTITY_SCALE, BASE_IDENTITY_SCALE),
            CoveragePattern::Full,
            &[GammaEnabled, GammaDitherDisabled],
        ),
        spec(
            "gamma-off-gamma-dither-on",
            progressive | GAMMA_DITHER,
            (ViField::Progressive, 0),
            (BASE_IDENTITY_SCALE, BASE_IDENTITY_SCALE),
            CoveragePattern::Full,
            &[GammaDisabled, GammaDitherEnabled],
        ),
        spec(
            "gamma-on-gamma-dither-on",
            progressive | GAMMA | GAMMA_DITHER,
            (ViField::Progressive, 0),
            (BASE_IDENTITY_SCALE, BASE_IDENTITY_SCALE),
            CoveragePattern::Full,
            &[GammaEnabled, GammaDitherEnabled],
        ),
        spec(
            "resample-identity",
            progressive,
            (ViField::Progressive, 0),
            (BASE_IDENTITY_SCALE, BASE_IDENTITY_SCALE),
            CoveragePattern::Full,
            &[IdentityResampling],
        ),
        spec(
            "resample-fractional-offset",
            progressive,
            (ViField::Progressive, 0),
            (
                ((BASE_OFFSET + 0x0200) << 16) | IDENTITY_SCALE,
                ((BASE_OFFSET + 0x0200) << 16) | IDENTITY_SCALE,
            ),
            CoveragePattern::Full,
            &[FractionalOffset],
        ),
        spec(
            "resample-minimum-nonzero",
            progressive,
            (ViField::Progressive, 0),
            ((BASE_OFFSET << 16) | 1, (BASE_OFFSET << 16) | 1),
            CoveragePattern::Full,
            &[MinimumNonzeroScale],
        ),
        spec(
            "resample-maximum-nonzero",
            progressive,
            (ViField::Progressive, 0),
            ((BASE_OFFSET << 16) | 0x0fff, (BASE_OFFSET << 16) | 0x0fff),
            CoveragePattern::Full,
            &[MaximumNonzeroScale],
        ),
        spec(
            "resample-exact-last",
            progressive,
            (ViField::Progressive, 0),
            ((BASE_OFFSET << 16) | 0x0f80, (BASE_OFFSET << 16) | 0x0f80),
            CoveragePattern::Full,
            &[ExactLastSample],
        ),
        spec(
            "resample-beyond-active-window",
            progressive,
            (ViField::Progressive, 0),
            ((BASE_OFFSET << 16) | 0x0f81, (BASE_OFFSET << 16) | 0x0f81),
            CoveragePattern::Full,
            &[BeyondActiveWindow],
        ),
    ]
}

fn spec(
    id: &'static str,
    status: u32,
    field: (ViField, u32),
    scales: (u32, u32),
    coverage: CoveragePattern,
    objectives: &'static [CorpusObjective],
) -> VectorSpec {
    let boundary = objectives.iter().any(|objective| {
        matches!(
            objective,
            CorpusObjective::ExactLastSample | CorpusObjective::BeyondActiveWindow
        )
    });
    let output_extent = if boundary {
        BOUNDARY_OUTPUT_EXTENT
    } else {
        DEFAULT_OUTPUT_EXTENT
    };
    VectorSpec {
        id,
        status,
        current: field.1,
        field: field.0,
        x_scale: scales.0,
        y_scale: scales.1,
        output_width: output_extent,
        output_height: output_extent,
        coverage,
        objectives,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ntsc_corpus_is_stable_complete_and_individually_valid() {
        let first = generate_digital_vector_corpus(ConsoleRegion::Ntsc).unwrap();
        let second = generate_digital_vector_corpus(ConsoleRegion::Ntsc).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.vectors.len(), 19);
        assert_eq!(first.index.vectors.len(), 19);
        assert_eq!(
            digest(&first.index_bytes),
            "13a28331286a2bfb0b23e623357b96c21e55dd93001113cc787c6bbcbfca8c86"
        );
        for generated in &first.vectors {
            assert_eq!(generated.artifact.byte_len, generated.bytes.len() as u64);
            assert_eq!(generated.artifact.sha256, digest(&generated.bytes));
            assert_eq!(generated.vector.vector_id, generated.artifact.vector_id);
            validate_vector(&generated.vector).unwrap();
            validate_fetch_footprint(&generated.vector, &generated.artifact).unwrap();
        }
        validate_index(&first.index).unwrap();
    }

    #[test]
    fn every_coverage_count_is_complete_and_matches_rgba16_storage() {
        let corpus = generate_digital_vector_corpus(ConsoleRegion::Ntsc).unwrap();
        for generated in corpus.vectors {
            let coverage = super::super::decode_blob(
                "coverage",
                &generated.vector.framebuffer.coverage_counts,
            )
            .unwrap();
            let pixels =
                super::super::decode_blob("pixels", &generated.vector.framebuffer.contents)
                    .unwrap();
            assert_eq!(
                coverage.len(),
                (FRAMEBUFFER_EXTENT * FRAMEBUFFER_EXTENT) as usize
            );
            for (pixel, count) in coverage.into_iter().enumerate() {
                assert!((1..=8).contains(&count));
                assert_eq!(pixels[pixel * 2 + 1] & 1, (count - 1) >> 2);
            }
        }
    }

    #[test]
    fn exact_last_and_guarded_beyond_are_causally_distinct() {
        let corpus = generate_digital_vector_corpus(ConsoleRegion::Ntsc).unwrap();
        let exact = corpus
            .vectors
            .iter()
            .find(|item| {
                item.artifact
                    .objectives
                    .contains(&CorpusObjective::ExactLastSample)
            })
            .unwrap();
        let beyond = corpus
            .vectors
            .iter()
            .find(|item| {
                item.artifact
                    .objectives
                    .contains(&CorpusObjective::BeyondActiveWindow)
            })
            .unwrap();
        assert_eq!(exact.vector.registers.x_scale & 0x0fff, 0x0f80);
        assert_eq!(beyond.vector.registers.x_scale & 0x0fff, 0x0f81);
        let exact_last = BASE_OFFSET + 8 * 0x0f80;
        let beyond_last = BASE_OFFSET + 8 * 0x0f81;
        let logical_source_last = LOGICAL_SOURCE_START + LOGICAL_SOURCE_EXTENT - 1;
        assert_eq!(exact_last, logical_source_last << 10);
        assert_eq!(beyond_last, (logical_source_last << 10) + 8);
        assert!(!exact.vector.filters.dither_filter);
        assert!(!exact.vector.filters.divot);
        assert!(exact
            .vector
            .framebuffer
            .coverage_counts
            .bytes_hex
            .bytes()
            .all(|byte| byte == b'0' || byte == b'8'));

        let mut mislabeled = exact.artifact.clone();
        mislabeled.objectives = vec![CorpusObjective::BeyondActiveWindow];
        assert!(validate_fetch_footprint(&exact.vector, &mislabeled)
            .unwrap_err()
            .to_string()
            .contains("does not land solely in the bound trailing guard"));

        let mut filtered_boundary = exact.vector.clone();
        filtered_boundary.filters.dither_filter = true;
        assert!(
            validate_fetch_footprint(&filtered_boundary, &exact.artifact)
                .unwrap_err()
                .to_string()
                .contains("must isolate resampling")
        );

        let ordinary = corpus
            .vectors
            .iter()
            .find(|item| item.artifact.vector_id == "field-progressive")
            .unwrap();
        let mut unsafe_partial = ordinary.vector.clone();
        let mut coverage = vec![8; (FRAMEBUFFER_EXTENT * FRAMEBUFFER_EXTENT) as usize];
        coverage[0] = 1;
        unsafe_partial.framebuffer.coverage_counts = blob(coverage);
        assert!(
            validate_fetch_footprint(&unsafe_partial, &ordinary.artifact)
                .unwrap_err()
                .to_string()
                .contains("unbound AA neighbor footprint")
        );
    }

    #[test]
    fn unsupported_regions_fail_loudly() {
        for region in [ConsoleRegion::Pal, ConsoleRegion::Mpal] {
            assert!(generate_digital_vector_corpus(region)
                .unwrap_err()
                .to_string()
                .contains("no PAL/MPAL register preset"));
        }
    }
}
