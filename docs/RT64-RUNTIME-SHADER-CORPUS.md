# RT64 runtime shader corpus

Status: **shared component mechanism only; 0/56 runtime-ready rows**.

M2.5.3 must produce a separately checked runtime implementation for every one
of the 56 rows in `rt64-shader-source-denominator.json`. Conditional reference
validity from M2.5.1, wgpu/Naga ingestion classification from M2.5.2, and
runtime execution are distinct claims. No one substitutes for another.

## DirectTexelDecodeV1

The first owned component is a compute-only WGSL transcription of the seven
direct texel converters already implemented and cited by
`crates/fn64-render-wgpu/src/tmem/texel.rs`: RGBA16, RGBA32, IA4, IA8, IA16,
I4, and I8. It consumes isolated typed values and performs no physical TMEM
read, CI/TLUT resolution, YUV conversion, addressing, sampling, filtering,
LOD, rasterization, framebuffer, or presentation work.

The retained identities are:

| field | value |
|---|---|
| component | `DirectTexelDecodeV1` |
| entry | compute `decode_direct_texels` |
| source SHA-256 | `2f59380f62db77f1c11b81e149894947d01ad8c812ee11e3771125317fff3880` |
| fixture schema | `fn64.render-wgpu.direct-texel-decode-fixture.v1` |
| fixture SHA-256 | `f24aca795ae8954ae362280cd91c75017623bf7db1601688e9bc2775dcfb7d37` |
| input SHA-256 | `6199dd74587f3a8ca86e24fbab5949bb0dcb04db8c4c47f3e9a65504edd9c274` |
| expected-output SHA-256 | `89bce88b397fa2a5e08eb1549498eba49465368968ff844ed0e42e407c17114f` |
| cases | `131710` |
| input/output bytes | `2107360` / `1053680` |
| workgroups | `2058` at `64` invocations |
| promotion | `NotQualified` |
| native state | `NativeUnverified` |

The fixture covers all 20 format/size pairs at zero, all values for the direct
4-, 8-, and 16-bit pairs, and 74 fixed RGBA32 values: zero, all ones, every
one-hot value, every one-hot complement, and eight channel-order vectors.
Expected records are generated only through `RawTexel::try_new` and
`decode_direct_texel`; the large byte vectors are not checked into git.

The closed device profile requests `wgpu::Features::empty()`, two compute
storage buffers, workgroup width/invocations 64, binding sizes sufficient for
the exact fixture, and 2058 workgroups in one dimension. Callers cannot add
features or limits. Repository tests parse the owned source with Naga 30's WGSL
frontend and validate it with all validation flags and no extra capabilities.
Before submission, the host path rechecks frozen source, entry point, fixture,
input, and expected-output identities and requires the requested features and
every requested limit (including `max_buffer_size`) to exactly equal the typed
validated profile. A successful native receipt would additionally bind
adapter/device/driver identity, entry point, requested profile, source,
fixture, and input identities, checked pipeline creation, exact submission
wait, callback observation, readback size and digest, and zero unexpected
device errors.

No native receipt exists: the available host returned no native adapter before
device creation. That is an honest unsupported-host observation, not a skip or
a passing native result. The current evidence is one final-source Naga/CPU
diagnostic only. This component remains `NotQualified` and
`NativeUnverified`. Promotion requires 10 consecutive clean native
differential processes against the same frozen source and identities; neither
one native run nor any number of Naga/CPU-only runs satisfies that gate.

## Candidate composition, not row promotion

The frozen denominator records `src/shaders/Formats.hlsli` in 21 rows. Those
row IDs are retained in `DIRECT_TEXEL_DECODE_CANDIDATE_CONSUMERS` so later
complete row ports can reuse one checked component identity. Eight of those
rows also contain `TextureDecoder.hlsli`; `src-shaders-texturedecodecs` is the
natural first complete-row target.

Dependency overlap proves neither that a row calls every converter nor that
the component implements the row. Every row still needs its complete stage
interface, resources, control flow, other shared functions, pipeline state,
native execution, and semantic differential bound to the accepted M2.5.2
outcome. Therefore this component changes the runtime-ready numerator by zero.

## Provenance and exclusions

Format/size legality comes from the public SGI *Nintendo 64 RDP Command
Summary*, Tables 3, 4, and 6. Numeric conversion behavior follows the already
recorded permitted MIT RT64 port source at `5473732a822a4423b5696e7cb18fecc425a59875`:
`src/shaders/Formats.hlsli` SHA-256
`9b5765371d19de1e410dbe919433922db975994e2a6077bf9e499a8a94f33b7b`
and `src/shaders/TextureDecoder.hlsli` SHA-256
`63b2c1ce683e7e7880c9508d3232d90e90236157ac86ae91947c62ae1d359f07`.
The manifest binds those exact values plus denominator path
`docs/rt64-shader-source-denominator.json`, denominator SHA-256
`cae8956fff3258bf5c21bb5cea7ffb550ab726118840a16db69764d3507d3ebe`,
and dependency paths `src/shaders/Formats.hlsli` and
`src/shaders/TextureDecoder.hlsli`; candidate composition is therefore tied to
the accepted M2.5.1 denominator and dependencies rather than a path-only
assertion.
The WGSL is project-owned; no HLSL is copied.

This slice accesses no private shader corpus or external checkout and invokes
no DXC, `spirv-val`, external validator, GPL runtime, m2c, game content, or ROM.
It makes no HLSL equivalence, complete-row, portability, RT64 parity, runtime
integration, or performance claim.
