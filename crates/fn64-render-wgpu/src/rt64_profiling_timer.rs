//! Literal port of RT64's `ProfilingTimer` statistical core (the ring-buffer
//! history, `log(double)`, `accumulation`, `average()`) plus the pure-
//! arithmetic slice of the `Timestamp`/`Timer`/`ElapsedTimer` type shapes, a
//! literal port of the permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`):
//!
//! - `src/common/rt64_profiling_timer.h`, lines 1-35 (whole file, SHA-256
//!   `adebf6054ce4e9d367a8705f13afbd049df1db6df003d7af2d7f1606e99506a4`).
//! - `src/common/rt64_profiling_timer.cpp`, lines 1-88 (whole file, SHA-256
//!   `50d65d9048684628e86aee052b8913384c737e62f07d1018716cefa342b7790c`).
//! - `src/common/rt64_elapsed_timer.h`, lines 1-19 (whole file, SHA-256
//!   `001939b9be0f4e3544a9a52be07e0528ab63acab8232cb11ee147dcfaad9ee6f`).
//! - `src/common/rt64_elapsed_timer.cpp`, lines 1-29 (whole file, SHA-256
//!   `435a3ee9995bba8c79cad6a9fd312368e75673e5de4590b9d0d772fce4c8a158`).
//! - `src/common/rt64_timer.h`, lines 1-21 (whole file, SHA-256
//!   `344b60d2a1c921eb14cc26ff9c5bf93416e5165e414965eeb6aaacb817296bc8`).
//! - `src/common/rt64_timer.cpp`, lines 1-74 (whole file, SHA-256
//!   `26acd31fb9236219441f4d5044060a041147ff6aebd445a671c6f4743cadc7cd`).
//!
//! All six digests above were recomputed locally with `shasum -a 256` against
//! `/private/tmp/rt64-upstream-audit/rt64-5473732a822a4423b5696e7cb18fecc425a59875/`
//! and cross-checked against `docs/rt64-port-inventory.json`'s
//! `sources.port.sha256` for each of the six `path` entries -- all six match
//! exactly.
//!
//! ```text
//! // rt64_profiling_timer.h
//! struct ProfilingTimer {
//!     std::vector<double> history;
//!     uint32_t historyIndex;
//!     double accumulation;
//!     Timestamp startedTimestamp;
//!
//!     ProfilingTimer();
//!     ProfilingTimer(size_t historyCount);
//!     void setCount(size_t historyCount);
//!     void clear();
//!     void reset();
//!     void start();
//!     void end();
//!     void log();
//!     void log(double value);
//!
//!     // Convenience function for logging the time between each call to it.
//!     void logAndRestart();
//!     uint32_t index() const;
//!     size_t size() const;
//!     const double *data() const;
//!     double average() const;
//! };
//!
//! // rt64_profiling_timer.cpp
//! ProfilingTimer::ProfilingTimer() {
//!     historyIndex = 0;
//!     accumulation = 0.0;
//!     startedTimestamp = {};
//! }
//!
//! ProfilingTimer::ProfilingTimer(size_t historySize) : ProfilingTimer() {
//!     setCount(historySize);
//! }
//!
//! void ProfilingTimer::setCount(size_t historyCount) {
//!     history.clear();
//!     history.resize(historyCount, 0);
//! }
//!
//! void ProfilingTimer::clear() {
//!     setCount(history.size());
//!     historyIndex = 0;
//!     accumulation = 0.0;
//! }
//!
//! void ProfilingTimer::reset() {
//!     accumulation = 0.0;
//! }
//!
//! void ProfilingTimer::log() {
//!     assert(!history.empty());
//!     history[historyIndex] = accumulation;
//!     historyIndex = (historyIndex + 1) % history.size();
//! }
//!
//! void ProfilingTimer::log(double value) {
//!     assert(!history.empty());
//!     history[historyIndex] = value;
//!     historyIndex = (historyIndex + 1) % history.size();
//! }
//!
//! uint32_t ProfilingTimer::index() const {
//!     return historyIndex;
//! }
//!
//! size_t ProfilingTimer::size() const {
//!     return history.size();
//! }
//!
//! const double *ProfilingTimer::data() const {
//!     return history.data();
//! }
//!
//! double ProfilingTimer::average() const {
//!     assert(!history.empty());
//!     return std::accumulate(history.begin(), history.end(), 0.0) / history.size();
//! }
//!
//! // rt64_timer.h
//! typedef std::chrono::high_resolution_clock::time_point Timestamp;
//!
//! struct Timer {
//!     static int64_t deltaMicroseconds(const Timestamp t1, const Timestamp t2);
//! };
//!
//! // rt64_timer.cpp
//! int64_t Timer::deltaMicroseconds(const Timestamp t1, const Timestamp t2) {
//!     return std::chrono::duration_cast<std::chrono::microseconds>(t2 - t1).count();
//! }
//!
//! // rt64_elapsed_timer.h
//! struct ElapsedTimer {
//!     Timestamp startTime;
//! };
//!
//! // rt64_elapsed_timer.cpp
//! double ElapsedTimer::elapsedMilliseconds() const {
//!     return static_cast<double>(elapsedMicroseconds()) / 1000.0;
//! }
//!
//! double ElapsedTimer::elapsedSeconds() const {
//!     return static_cast<double>(elapsedMicroseconds()) / 1000000.0;
//! }
//! ```
//!
//! **Reuse, not new type.** No `fn64-render-ir`/`fn64-abi` type is reused
//! here -- RT64 has no existing fn64-owned equivalent of a ring-buffer
//! profiling accumulator or a monotonic-tick timestamp, so `ProfilingTimer`
//! and `Timestamp` are ported as new owned types local to this module,
//! matching `rt64_common.rs`'s precedent for `FixedRect`/`FixedMatrix` (also
//! newly introduced, since RT64 does not reuse an existing type for them
//! either).
//!
//! ## Admitted domain
//!
//! - **The clock boundary is drawn at "does this function read
//!   `std::chrono::high_resolution_clock::now()`."** Everything that does
//!   (`Timer::current()`, `Timer::initialize()`, `Timer::preciseSleepUntil()`,
//!   `ProfilingTimer::start()`, `ProfilingTimer::end()`,
//!   `ProfilingTimer::logAndRestart()`, `ElapsedTimer::ElapsedTimer()`,
//!   `ElapsedTimer::reset()`, `ElapsedTimer::elapsedMicroseconds()`) is
//!   **not** ported -- it is wall-clock-dependent and would make
//!   characterization tests flaky against this repository's 10x determinism
//!   bar (`AGENTS.md`). Everything on the other side of that line --
//!   `Timer::deltaMicroseconds(t1, t2)` (a pure `(t2 - t1)` subtraction plus
//!   a fixed unit-scale, taking both timestamps as parameters rather than
//!   reading either from a clock) and `elapsedMilliseconds`/
//!   `elapsedSeconds`'s divisions (pure `f64` scaling of an elapsed-
//!   microseconds count) -- is ported with the clock read replaced by an
//!   explicit caller-supplied parameter, matching this project's established
//!   precedent (prior cards modeling `extendRDRAM`, `avgLuma`, `loadTLUT` the
//!   same way: replace "reads live state" with "takes the value as an
//!   argument").
//! - **`Timestamp` is modeled as an opaque tick count, never constructed from
//!   a live clock in this module.** `std::chrono::high_resolution_clock::
//!   time_point` is an opaque monotonic tick with implementation-defined
//!   epoch and resolution; the only operation this port needs from it is
//!   subtraction with a known tick period. `Timestamp` is ported as
//!   `pub struct Timestamp(pub i64)` wrapping a caller-supplied nanosecond
//!   tick count (nanoseconds because `high_resolution_clock` is
//!   nanosecond-resolution on every platform RT64 ships to, and because
//!   `deltaMicroseconds`'s `duration_cast<microseconds>` truncates a
//!   finer-grained duration -- modeling the stored unit as nanoseconds is
//!   the most precision-preserving explicit-parameter representation, and
//!   strictly a superset of what any coarser unit could represent). No
//!   method on `Timestamp` reads a clock; every value a caller passes to
//!   [`delta_microseconds`] in this module's own tests is a hand-picked
//!   literal, not one obtained by observing wall-clock time.
//! - **Ring-buffer wrap: `%`, not saturation, not a reset -- and it is
//!   guarded, but not where a first read suggests.** `log`/`log(value)`
//!   compute `historyIndex = (historyIndex + 1) % history.size()`, a true
//!   modulo wrap that revisits index 0 once `historyIndex` reaches
//!   `history.size() - 1`. This project's hazard note warns a prior card
//!   found a divide-by-zero frontier "exactly here-shaped": `% history.size()`
//!   on an empty (`size() == 0`) buffer is division/modulo by zero in both
//!   C++ (undefined behavior) and Rust (`%` on integers panics on a zero
//!   divisor, in both debug and release). **The source does not guard this
//!   with a runtime check that degrades gracefully -- it guards it with
//!   `assert(!history.empty())`, a debug-only precondition**, immediately
//!   before the modulo in both `log()` and `log(double value)`. This is
//!   preserved literally as `debug_assert!(!self.history.is_empty())`
//!   (matching `rt64_common.rs`'s established `assert()` ->
//!   `debug_assert!()` precedent for debug-only C++ preconditions -- see
//!   that module's "Admitted domain" for the full reasoning, which applies
//!   identically here) -- **not** a silent early-return guard, since the
//!   source has no such guard and inventing one would be an unrequested
//!   behavior widening. In a release build (`NDEBUG`, or Rust
//!   `--release`), calling `log`/`log(value)` on a zero-size
//!   `ProfilingTimer` still panics in this Rust port, but **not at the
//!   modulo**: `history[historyIndex]` is evaluated *first*, and Rust's
//!   slice indexing is bounds-checked in every profile, so the panic is
//!   `index out of bounds: the len is 0 but the index is 0` and the
//!   `% 0` is never reached. (Verified by running the empty-buffer `log`
//!   test under `-C debug-assertions=off`.) The C++ has the same statement
//!   order, so in a release C++ build `history[historyIndex]` is an
//!   unchecked out-of-bounds `std::vector::operator[]` -- genuine UB there,
//!   reached before the modulo just as in Rust. Rust's bounds check turns
//!   that UB into a deterministic panic; this is the one place the port is
//!   *narrower* (louder, safer) than the source, and it is unavoidable
//!   without `get_unchecked`.
//! - **`average()` on an empty buffer is NOT undefined and does NOT panic
//!   in either language's release build -- it yields NaN.**
//!   `std::accumulate(...) / history.size()` with an empty buffer is
//!   `0.0 / 0.0`, and IEEE-754 defines that as NaN for `double`; there is
//!   no integer division and no out-of-bounds access anywhere in the
//!   expression, because `accumulate` over an empty range simply returns
//!   its `0.0` seed. Confirmed both ways: the C++ body compiled with
//!   `-DNDEBUG` returns NaN, and this port's `average()` built with
//!   `-C debug-assertions=off` returns NaN. The `debug_assert!` is
//!   preserved as a faithful port of the source's debug-only `assert`, but
//!   it is the *only* thing that makes an empty-buffer `average()` fail --
//!   remove it (as a release build does) and the call is well-defined.
//!   The characterization test therefore asserts the profile-independent
//!   truth (`average().is_nan()`), never `#[should_panic]`.
//! - **On `#[should_panic]` tests over `debug_assert!`: this module follows
//!   `rt64_common.rs`'s precedent, with one narrow, deliberate exception.**
//!   That module declines such tests outright, on the grounds that they
//!   assert "a build-profile-dependent property outside this port's
//!   characterization scope". That reasoning is adopted here: no test in
//!   this module asserts that a `debug_assert!` fires. The empty-buffer
//!   `average()` case previously did, and was wrong to -- it inverted under
//!   `-C debug-assertions=off`, where the call returns NaN instead of
//!   panicking, and it has been replaced by a NaN assertion that holds in
//!   both profiles. The one surviving `#[should_panic]` test covers
//!   empty-buffer `log`, and it is *not* an exception to the precedent:
//!   what it pins is the bounds check on `history[historyIndex]`, which
//!   Rust performs in **every** profile, so the panic is profile-
//!   independent and the property is in scope by `rt64_common.rs`'s own
//!   standard. Its `debug_assert!` is merely what fires first in a debug
//!   build; the test is named and commented for the bounds check, and is
//!   verified to pass with debug assertions both on and off.
//! - **The zero-argument `log()` overload IS ported, as
//!   [`ProfilingTimer::log_accumulation`].** An earlier revision of this
//!   port excluded it on the stated grounds that "only `start`/`end` mutate
//!   `accumulation`, and since neither is ported the overload has no
//!   meaningful body left". That reason does not survive checking the
//!   source: `accumulation` is written in four places -- the constructor
//!   (`.cpp:14`), `clear()` (`.cpp:30`), `reset()` (`.cpp:34`), and `end()`
//!   (`.cpp:44`) -- and three of those four are ported here. Only the
//!   `end()` write is excluded, and only because it reads a clock. The
//!   overload's own body (`history[historyIndex] = accumulation;` then the
//!   `%`-wrap) reads no clock and touches no excluded state: it is exactly
//!   as pure as `log(double)`, differing only in taking its value from the
//!   `accumulation` field instead of a parameter. Since `accumulation` is a
//!   public field that callers and tests already set directly, the overload
//!   is fully exercisable and is ported literally. It is named
//!   `log_accumulation` rather than `log` only because Rust has no function
//!   overloading -- the two C++ overloads need two distinct Rust names.
//! - **`average()` divides by `history.size()`, not by a logged-so-far
//!   count -- even on a partially-filled buffer.** `std::accumulate(history.
//!   begin(), history.end(), 0.0) / history.size()` sums (and divides by)
//!   the *entire allocated* history vector, including any slots at their
//!   `resize(historyCount, 0)`-initialized `0.0` default that have never
//!   been `log`-ed. There is no separate "how many times has `log` been
//!   called" counter anywhere in `ProfilingTimer` -- `historyIndex` is a
//!   write cursor, not a fill counter, and wraps back to counting from zero
//!   once the buffer is full. So a `ProfilingTimer` with `size() == 10` that
//!   has only been `log`-ed 3 times computes `average()` as `(v0 + v1 + v2 +
//!   0 + 0 + 0 + 0 + 0 + 0 + 0) / 10`, not `(v0 + v1 + v2) / 3` -- the
//!   unlogged slots silently participate in the average as zeros. This is
//!   preserved exactly, with no "only average what's been logged" correction
//!   invented.
//! - **`std::accumulate`'s seed is `0.0` (a `double` literal), not `0` (an
//!   `int` literal) -- so accumulation is `f64` the whole way through, never
//!   silently narrowed to integer arithmetic.** Ported as `history.iter()
//!   .fold(0.0_f64, |acc, &v| acc + v)`, an explicit strict left fold over
//!   `history`'s natural (index 0 -> `size() - 1`) iteration order, matching
//!   `std::accumulate`'s guaranteed left-to-right, non-reassociated
//!   summation exactly (`Iterator::sum()` on a `Vec<f64>` in Rust does not
//!   documented-guarantee summation order, so `fold` is used instead to keep
//!   the same evaluation order as the source, bit-for-bit).
//! - **`clear()` calls `setCount(history.size())`, not a direct zero-fill --
//!   preserved as this exact indirection, not normalized to
//!   `self.history.fill(0.0)`.** `set_count(self.history.len())` re-`clear()`s
//!   and `resize(n, 0)`s the `Vec` to its own current length: `Vec::clear()`
//!   drops the elements but **retains the allocation**, so the following
//!   `resize` back to that same length refills the buffer already in hand --
//!   no reallocation happens, and both the backing pointer and the capacity
//!   are unchanged across the pair (verified directly). The *observable*
//!   result (every slot `0.0`, same length as before) is therefore
//!   identical to a direct fill -- the indirection is preserved literally
//!   because the brief marks it as a hazard to preserve verbatim, not
//!   because this port found a behavior difference a direct fill would
//!   introduce. `clear()` additionally resets `historyIndex` to `0` and
//!   `accumulation` to `0.0`; `reset()` resets `accumulation` only, leaving
//!   `history` and `historyIndex` untouched -- these are different
//!   operations and this port keeps them as two separate methods with no
//!   shared implementation, exactly as the source does not share one either.
//! - **Zero-size construction (`ProfilingTimer(0)` / the default
//!   constructor) is legal and does not panic.** `ProfilingTimer()` and
//!   `ProfilingTimer(size_t historyCount)` (including `historyCount == 0`,
//!   which `resize(0, 0)` accepts as a no-op) never touch the `assert(!
//!   history.empty())`-guarded methods (`log`/`log(value)`/`average`) --
//!   only *calling* those methods on an empty buffer is the (debug-asserted,
//!   release-live) divide-by-zero frontier described above. Construction
//!   itself is unconditionally safe at every `historyCount`, and this port's
//!   `ProfilingTimer::new`/`with_history_count` reflect that (no
//!   `debug_assert!` on either constructor).
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet, matching `rt64_common.rs`'s precedent -- dead-code warnings
//! on the unused public surface are expected and correct), and no RT64
//! visual/pixel/silicon parity or performance claim -- nothing here has been
//! measured against real RT64 output or wall-clock behavior, only
//! characterized against hand-computed expectations from the C++ source
//! text. Deliberately not ported, because each reads a live clock:
//!
//! - `Timer::initialize()` (a no-op body in the pinned commit, but still a
//!   clock-subsystem lifecycle hook, out of scope).
//! - `Timer::current()` (`std::chrono::high_resolution_clock::now()` --
//!   reads the live clock directly).
//! - `Timer::preciseSleepUntil(endTime)` (spins/sleeps against the live
//!   clock; also stateful via `thread_local` sleep-duration statistics,
//!   which is a second, independent reason it is out of scope for a pure
//!   characterization port).
//! - `ElapsedTimer::ElapsedTimer()` and `ElapsedTimer::reset()` (both call
//!   `Timer::current()` to stamp `startTime`).
//! - `ElapsedTimer::elapsedMicroseconds()` (calls `Timer::current()` to get
//!   "now" before subtracting `startTime`).
//! - `ProfilingTimer::start()` (calls `Timer::current()`, and its debug-only
//!   `assert(startedTimestamp == Timestamp{})` precondition -- not ported,
//!   since the function itself is not ported).
//! - `ProfilingTimer::end()` (calls `Timer::current()` via
//!   `Timer::deltaMicroseconds(startedTimestamp, Timer::current())`, and its
//!   debug-only `assert(startedTimestamp > Timestamp{})` precondition -- not
//!   ported for the same reason).
//! - `ProfilingTimer::logAndRestart()` (calls `end()`/`start()`/`Timer::
//!   current()` transitively).
//! - `ProfilingTimer`'s `startedTimestamp` field is not represented in this
//!   port's struct: it exists in the source solely to support `start()`/
//!   `end()`/`logAndRestart()`, none of which are ported, so a same-named
//!   field with no method ever reading or writing it would be a dead,
//!   behaviorless vestige rather than a faithful port of anything -- omitted
//!   outright rather than carried as an inert field.
//! - `Timestamp`'s equality/ordering operators (`operator==`, `operator>`)
//!   are not exercised by anything in this port (they are only used inside
//!   the excluded `start`/`end`/`logAndRestart`); `Timestamp` here derives
//!   `PartialEq`/`Eq`/`PartialOrd`/`Ord` structurally (from its single `i64`
//!   field) but no *behavior* from those derives is characterized or relied
//!   upon by this module's own code.

/// `Timestamp`: an opaque monotonic tick, modeled as a caller-supplied
/// nanosecond count rather than by reading a live clock (see module doc
/// "Admitted domain" for why nanoseconds, and "Nonclaims" for why no
/// constructor here calls into a clock).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(pub i64);

/// `Timer::deltaMicroseconds(t1, t2)`: pure `(t2 - t1)` in nanoseconds,
/// truncated to microseconds -- matches `std::chrono::duration_cast`'s
/// truncate-toward-zero narrowing.
pub fn delta_microseconds(t1: Timestamp, t2: Timestamp) -> i64 {
    (t2.0 - t1.0) / 1_000
}

/// `ElapsedTimer::elapsedMilliseconds()`'s pure division, taking the elapsed
/// microsecond count as an explicit parameter in place of a live
/// `elapsedMicroseconds()` clock read (see module doc "Admitted domain").
pub fn elapsed_milliseconds(elapsed_microseconds: i64) -> f64 {
    elapsed_microseconds as f64 / 1000.0
}

/// `ElapsedTimer::elapsedSeconds()`'s pure division, taking the elapsed
/// microsecond count as an explicit parameter in place of a live
/// `elapsedMicroseconds()` clock read (see module doc "Admitted domain").
pub fn elapsed_seconds(elapsed_microseconds: i64) -> f64 {
    elapsed_microseconds as f64 / 1_000_000.0
}

/// `ProfilingTimer`'s statistical core: the ring-buffer `history`, its write
/// cursor `historyIndex`, and the `accumulation` scratch value. `start()`/
/// `end()`/`logAndRestart()` and the `startedTimestamp` field are
/// deliberately not represented (see module doc "Nonclaims"); both `log`
/// overloads are ported, as [`ProfilingTimer::log`] and
/// [`ProfilingTimer::log_accumulation`].
#[derive(Clone, Debug, PartialEq)]
pub struct ProfilingTimer {
    pub history: Vec<f64>,
    pub history_index: u32,
    pub accumulation: f64,
}

impl ProfilingTimer {
    /// `ProfilingTimer()`: zero-length history, cursor at `0`, no
    /// accumulation.
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
            history_index: 0,
            accumulation: 0.0,
        }
    }

    /// `ProfilingTimer(size_t historyCount)`: default-constructs, then
    /// `setCount(historyCount)`.
    pub fn with_history_count(history_count: usize) -> Self {
        let mut timer = Self::new();
        timer.set_count(history_count);
        timer
    }

    /// `setCount(historyCount)`: clears `history`, then resizes it to
    /// `historyCount` slots of `0.0`. Does not touch `historyIndex` or
    /// `accumulation`.
    pub fn set_count(&mut self, history_count: usize) {
        self.history.clear();
        self.history.resize(history_count, 0.0);
    }

    /// `clear()`: re-`setCount`s `history` to its own current length (see
    /// module doc "Admitted domain" for why this indirection is preserved
    /// rather than normalized to a direct fill), then resets `historyIndex`
    /// to `0` and `accumulation` to `0.0`.
    pub fn clear(&mut self) {
        self.set_count(self.history.len());
        self.history_index = 0;
        self.accumulation = 0.0;
    }

    /// `reset()`: resets `accumulation` to `0.0` only -- `history` and
    /// `historyIndex` are untouched. Differs from `clear()`.
    pub fn reset(&mut self) {
        self.accumulation = 0.0;
    }

    /// `log(double value)`: writes `value` at the current cursor, then
    /// advances the cursor with a true `%`-wrap. C++ `assert(!history.
    /// empty())` is a debug-only precondition -- ported as `debug_assert!`,
    /// not a silent early return, since the source has no such guard.
    ///
    /// On an empty buffer this panics in *every* profile, but at the
    /// indexing statement, not the modulo: `self.history[..]` is
    /// bounds-checked unconditionally in Rust and fires
    /// `index out of bounds: the len is 0 but the index is 0` before the
    /// `% 0` is ever evaluated (see module doc "Admitted domain").
    pub fn log(&mut self, value: f64) {
        debug_assert!(!self.history.is_empty());
        self.history[self.history_index as usize] = value;
        self.history_index = (self.history_index + 1) % self.history.len() as u32;
    }

    /// `log()` (the zero-argument overload): writes the current
    /// `accumulation` field at the cursor, then advances the cursor with the
    /// same `%`-wrap as [`ProfilingTimer::log`]. Renamed only because Rust
    /// lacks function overloading; the body is a literal port (see module
    /// doc "Admitted domain" for why this overload is in scope). Panics on
    /// an empty buffer by the same bounds check as [`ProfilingTimer::log`].
    pub fn log_accumulation(&mut self) {
        debug_assert!(!self.history.is_empty());
        self.history[self.history_index as usize] = self.accumulation;
        self.history_index = (self.history_index + 1) % self.history.len() as u32;
    }

    /// `index()`: current write cursor.
    pub fn index(&self) -> u32 {
        self.history_index
    }

    /// `size()`: `history`'s length.
    pub fn size(&self) -> usize {
        self.history.len()
    }

    /// `data()`: the backing slice. `const double *` becomes `&[f64]`
    /// (a length-carrying slice is the literal Rust equivalent of a C++
    /// `std::vector`'s `data()` pointer plus its `size()`, both of which
    /// this type already exposes as separate methods matching the source's
    /// own separate `data()`/`size()` surface).
    pub fn data(&self) -> &[f64] {
        &self.history
    }

    /// `average()`: sums the *entire* `history` buffer (including any
    /// never-`log`-ed, still-`0.0` slots) via a strict left fold seeded at
    /// `0.0`, then divides by `history.size()` -- not by how many times
    /// `log` has actually been called (see module doc "Admitted domain").
    /// C++ `assert(!history.empty())` is a debug-only precondition, ported
    /// as `debug_assert!`.
    ///
    /// Unlike [`ProfilingTimer::log`], an empty buffer here is **not** a
    /// panic and **not** UB once that assertion is compiled out: the
    /// expression is the floating-point `0.0 / 0.0`, which IEEE-754 defines
    /// as NaN. Both the C++ built with `-DNDEBUG` and this port built with
    /// `-C debug-assertions=off` return NaN (see module doc "Admitted
    /// domain").
    pub fn average(&self) -> f64 {
        debug_assert!(!self.history.is_empty());
        let sum = self.history.iter().fold(0.0_f64, |acc, &v| acc + v);
        sum / self.history.len() as f64
    }
}

impl Default for ProfilingTimer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Timestamp / delta_microseconds ---

    #[test]
    fn timestamp_default_is_zero() {
        assert_eq!(Timestamp::default(), Timestamp(0));
    }

    #[test]
    fn delta_microseconds_zero_delta_is_zero() {
        assert_eq!(delta_microseconds(Timestamp(0), Timestamp(0)), 0);
    }

    #[test]
    fn delta_microseconds_one_microsecond_of_nanoseconds() {
        // 1000 ns = 1 us exactly.
        assert_eq!(delta_microseconds(Timestamp(0), Timestamp(1_000)), 1);
    }

    #[test]
    fn delta_microseconds_truncates_toward_zero_on_partial_microsecond() {
        // 1999 ns / 1000 = 1 (truncated), matching duration_cast's
        // truncating narrowing conversion, not rounding.
        assert_eq!(delta_microseconds(Timestamp(0), Timestamp(1_999)), 1);
    }

    #[test]
    fn delta_microseconds_negative_delta_truncates_toward_zero_not_floor() {
        // t2 - t1 = -1999 ns; integer division truncates toward zero in
        // Rust (and in C++ since C++11), giving -1, not -2 (floor).
        assert_eq!(delta_microseconds(Timestamp(1_999), Timestamp(0)), -1);
    }

    #[test]
    fn delta_microseconds_is_antisymmetric() {
        let a = Timestamp(500);
        let b = Timestamp(8_500);
        assert_eq!(delta_microseconds(a, b), -delta_microseconds(b, a));
    }

    #[test]
    fn delta_microseconds_large_span() {
        // 5,000,000,000 ns = 5,000,000 us = 5 seconds.
        assert_eq!(
            delta_microseconds(Timestamp(0), Timestamp(5_000_000_000)),
            5_000_000
        );
    }

    // --- elapsed_milliseconds / elapsed_seconds ---

    #[test]
    fn elapsed_milliseconds_zero_is_zero() {
        assert_eq!(elapsed_milliseconds(0), 0.0);
    }

    #[test]
    fn elapsed_milliseconds_one_thousand_microseconds_is_one_millisecond() {
        assert_eq!(elapsed_milliseconds(1_000), 1.0);
    }

    #[test]
    fn elapsed_milliseconds_fractional_result() {
        // 1500 us / 1000.0 = 1.5 ms.
        assert_eq!(elapsed_milliseconds(1_500), 1.5);
    }

    #[test]
    fn elapsed_milliseconds_negative_input_stays_negative() {
        assert_eq!(elapsed_milliseconds(-2_000), -2.0);
    }

    #[test]
    fn elapsed_seconds_zero_is_zero() {
        assert_eq!(elapsed_seconds(0), 0.0);
    }

    #[test]
    fn elapsed_seconds_one_million_microseconds_is_one_second() {
        assert_eq!(elapsed_seconds(1_000_000), 1.0);
    }

    #[test]
    fn elapsed_seconds_fractional_result() {
        // 250,000 us / 1,000,000.0 = 0.25 s.
        assert_eq!(elapsed_seconds(250_000), 0.25);
    }

    #[test]
    fn elapsed_seconds_negative_input_stays_negative() {
        assert_eq!(elapsed_seconds(-3_000_000), -3.0);
    }

    // --- ProfilingTimer: construction ---

    #[test]
    fn new_has_empty_history_zero_index_zero_accumulation() {
        let t = ProfilingTimer::new();
        assert_eq!(t.history, Vec::<f64>::new());
        assert_eq!(t.history_index, 0);
        assert_eq!(t.accumulation, 0.0);
    }

    #[test]
    fn default_matches_new() {
        assert_eq!(ProfilingTimer::default(), ProfilingTimer::new());
    }

    #[test]
    fn with_history_count_zero_is_legal_and_empty() {
        let t = ProfilingTimer::with_history_count(0);
        assert_eq!(t.size(), 0);
        assert_eq!(t.history_index, 0);
        assert_eq!(t.accumulation, 0.0);
    }

    #[test]
    fn with_history_count_nonzero_allocates_zero_filled_history() {
        let t = ProfilingTimer::with_history_count(4);
        assert_eq!(t.history, vec![0.0, 0.0, 0.0, 0.0]);
        assert_eq!(t.size(), 4);
        assert_eq!(t.history_index, 0);
    }

    // --- set_count ---

    #[test]
    fn set_count_grows_from_zero() {
        let mut t = ProfilingTimer::new();
        t.set_count(3);
        assert_eq!(t.history, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn set_count_shrinks_and_drops_prior_contents() {
        let mut t = ProfilingTimer::with_history_count(5);
        t.log(1.0);
        t.log(2.0);
        t.set_count(2);
        assert_eq!(t.history, vec![0.0, 0.0]);
    }

    #[test]
    fn set_count_does_not_touch_history_index_or_accumulation() {
        let mut t = ProfilingTimer::with_history_count(3);
        t.log(9.0);
        t.accumulation = 42.0;
        let index_before = t.history_index;
        t.set_count(6);
        assert_eq!(t.history_index, index_before);
        assert_eq!(t.accumulation, 42.0);
    }

    #[test]
    fn set_count_to_same_size_zero_fills_every_slot() {
        let mut t = ProfilingTimer::with_history_count(3);
        t.log(1.0);
        t.log(2.0);
        t.log(3.0);
        t.set_count(3);
        assert_eq!(t.history, vec![0.0, 0.0, 0.0]);
    }

    // --- clear vs reset ---

    #[test]
    fn clear_zero_fills_history_resets_index_and_accumulation() {
        let mut t = ProfilingTimer::with_history_count(3);
        t.log(1.0);
        t.log(2.0);
        t.accumulation = 7.0;
        t.clear();
        assert_eq!(t.history, vec![0.0, 0.0, 0.0]);
        assert_eq!(t.history_index, 0);
        assert_eq!(t.accumulation, 0.0);
    }

    #[test]
    fn clear_preserves_history_length() {
        let mut t = ProfilingTimer::with_history_count(5);
        t.clear();
        assert_eq!(t.size(), 5);
    }

    #[test]
    fn clear_on_zero_size_stays_zero_size() {
        let mut t = ProfilingTimer::new();
        t.clear();
        assert_eq!(t.size(), 0);
    }

    #[test]
    fn reset_only_touches_accumulation() {
        let mut t = ProfilingTimer::with_history_count(3);
        t.log(1.0);
        t.log(2.0);
        t.accumulation = 7.0;
        let history_before = t.history.clone();
        let index_before = t.history_index;
        t.reset();
        assert_eq!(t.accumulation, 0.0);
        assert_eq!(t.history, history_before);
        assert_eq!(t.history_index, index_before);
    }

    #[test]
    fn reset_and_clear_differ_on_history_contents() {
        let mut a = ProfilingTimer::with_history_count(3);
        a.log(1.0);
        a.log(2.0);
        let mut b = a.clone();

        a.reset();
        b.clear();

        // reset() leaves logged values in place; clear() zero-fills them.
        assert_eq!(a.history, vec![1.0, 2.0, 0.0]);
        assert_eq!(b.history, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn reset_and_clear_differ_on_history_index() {
        let mut a = ProfilingTimer::with_history_count(3);
        a.log(1.0);
        let mut b = a.clone();

        a.reset();
        b.clear();

        assert_eq!(a.history_index, 1);
        assert_eq!(b.history_index, 0);
    }

    // --- log / index / size / data: empty buffer ---

    #[test]
    #[should_panic]
    fn log_on_empty_history_panics_via_debug_assert_or_bounds_check() {
        // This call panics in every build profile, but NOT via the modulo:
        // `history[historyIndex]` is evaluated before the wrap arithmetic,
        // and Rust bounds-checks slice indexing unconditionally, so with
        // debug assertions off the panic is `index out of bounds: the len
        // is 0 but the index is 0` and the `% 0` is never reached. (Run
        // under `-C debug-assertions=off` to observe exactly that message.)
        // In a debug build the ported `debug_assert!` simply fires first.
        // Because the bounds check is profile-independent, this
        // `#[should_panic]` is in scope by `rt64_common.rs`'s own standard
        // -- see the module doc's note on that precedent.
        let mut t = ProfilingTimer::new();
        t.log(1.0);
    }

    #[test]
    #[should_panic]
    fn log_accumulation_on_empty_history_panics_via_debug_assert_or_bounds_check() {
        // Same profile-independent bounds check as `log`, via the
        // zero-argument overload's `history[historyIndex] = accumulation`.
        let mut t = ProfilingTimer::new();
        t.accumulation = 1.0;
        t.log_accumulation();
    }

    #[test]
    fn index_on_new_timer_is_zero() {
        assert_eq!(ProfilingTimer::new().index(), 0);
    }

    #[test]
    fn size_on_new_timer_is_zero() {
        assert_eq!(ProfilingTimer::new().size(), 0);
    }

    #[test]
    fn data_on_empty_history_is_empty_slice() {
        let t = ProfilingTimer::new();
        assert_eq!(t.data(), &[] as &[f64]);
    }

    // --- log: partially-filled buffer ---

    #[test]
    fn log_writes_value_at_current_index_then_advances() {
        let mut t = ProfilingTimer::with_history_count(4);
        t.log(10.0);
        assert_eq!(t.history, vec![10.0, 0.0, 0.0, 0.0]);
        assert_eq!(t.index(), 1);
    }

    #[test]
    fn log_second_call_writes_at_advanced_index() {
        let mut t = ProfilingTimer::with_history_count(4);
        t.log(10.0);
        t.log(20.0);
        assert_eq!(t.history, vec![10.0, 20.0, 0.0, 0.0]);
        assert_eq!(t.index(), 2);
    }

    #[test]
    fn log_partial_fill_leaves_remaining_slots_at_zero() {
        let mut t = ProfilingTimer::with_history_count(5);
        t.log(1.0);
        t.log(2.0);
        assert_eq!(t.history, vec![1.0, 2.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn log_data_matches_history_slice() {
        let mut t = ProfilingTimer::with_history_count(3);
        t.log(5.0);
        assert_eq!(t.data(), t.history.as_slice());
    }

    // --- log: exactly-full buffer ---

    #[test]
    fn log_exactly_fills_buffer_index_wraps_to_zero() {
        let mut t = ProfilingTimer::with_history_count(3);
        t.log(1.0);
        t.log(2.0);
        t.log(3.0);
        assert_eq!(t.history, vec![1.0, 2.0, 3.0]);
        // (2 + 1) % 3 = 0: index wraps back to zero exactly at full.
        assert_eq!(t.index(), 0);
    }

    #[test]
    fn log_single_slot_buffer_wraps_every_call() {
        let mut t = ProfilingTimer::with_history_count(1);
        t.log(1.0);
        assert_eq!(t.index(), 0);
        t.log(2.0);
        assert_eq!(t.index(), 0);
        assert_eq!(t.history, vec![2.0]);
    }

    // --- log: wrap-around past full ---

    #[test]
    fn log_wrap_around_overwrites_oldest_slot() {
        let mut t = ProfilingTimer::with_history_count(3);
        t.log(1.0);
        t.log(2.0);
        t.log(3.0);
        // Index is back at 0; this call overwrites the first slot.
        t.log(4.0);
        assert_eq!(t.history, vec![4.0, 2.0, 3.0]);
        assert_eq!(t.index(), 1);
    }

    #[test]
    fn log_wrap_around_multiple_cycles() {
        let mut t = ProfilingTimer::with_history_count(2);
        t.log(1.0);
        t.log(2.0);
        t.log(3.0); // wraps, overwrites slot 0
        t.log(4.0); // overwrites slot 1
        t.log(5.0); // wraps again, overwrites slot 0
        assert_eq!(t.history, vec![5.0, 4.0]);
        assert_eq!(t.index(), 1);
    }

    #[test]
    fn log_index_sequence_across_a_full_wrap_cycle() {
        let mut t = ProfilingTimer::with_history_count(3);
        let mut indices = Vec::new();
        for v in 0..7 {
            t.log(v as f64);
            indices.push(t.index());
        }
        // Cursor cycles 1,2,0,1,2,0,1 for a 3-slot buffer logged 7 times.
        assert_eq!(indices, vec![1, 2, 0, 1, 2, 0, 1]);
    }

    // --- log_accumulation(): the zero-argument log() overload ---

    #[test]
    fn log_accumulation_writes_accumulation_field_then_advances() {
        let mut t = ProfilingTimer::with_history_count(3);
        t.accumulation = 7.5;
        t.log_accumulation();
        assert_eq!(t.history, vec![7.5, 0.0, 0.0]);
        assert_eq!(t.index(), 1);
    }

    #[test]
    fn log_accumulation_does_not_reset_accumulation() {
        // The source's log() reads `accumulation` but never clears it --
        // only reset()/clear() do. Repeated calls therefore log the same
        // value into successive slots.
        let mut t = ProfilingTimer::with_history_count(3);
        t.accumulation = 2.0;
        t.log_accumulation();
        t.log_accumulation();
        assert_eq!(t.accumulation, 2.0);
        assert_eq!(t.history, vec![2.0, 2.0, 0.0]);
    }

    #[test]
    fn log_accumulation_wraps_like_the_value_overload() {
        let mut t = ProfilingTimer::with_history_count(2);
        t.accumulation = 1.0;
        t.log_accumulation();
        t.accumulation = 2.0;
        t.log_accumulation();
        t.accumulation = 3.0;
        t.log_accumulation(); // wraps, overwrites slot 0
        assert_eq!(t.history, vec![3.0, 2.0]);
        assert_eq!(t.index(), 1);
    }

    #[test]
    fn log_accumulation_after_reset_logs_zero() {
        let mut t = ProfilingTimer::with_history_count(2);
        t.accumulation = 9.0;
        t.reset();
        t.log_accumulation();
        assert_eq!(t.history, vec![0.0, 0.0]);
    }

    // --- average(): each buffer state ---

    // `average()` on an empty buffer is deliberately NOT a `#[should_panic]`
    // test. The ported `debug_assert!` fires only in debug builds; with
    // debug assertions off the body reduces to `0.0 / 0.0`, which IEEE-754
    // defines as NaN -- so a `#[should_panic]` form passes in debug and
    // FAILS under `--release`. The C++ body compiled with `-DNDEBUG`
    // returns NaN as well, so this is not UB in the source either.
    //
    // NaN is the property that holds in both profiles and both languages,
    // and it is asserted here over `average()`'s exact body (the strict
    // left fold seeded at `0.0`, divided by `len()`) evaluated on an empty
    // history -- i.e. precisely what a release build executes once the
    // `debug_assert!` is compiled out. Writing it this way keeps one test
    // that compiles and passes identically in every profile, rather than
    // a `#[cfg(not(debug_assertions))]` test that would make the suite's
    // test count depend on the build profile.
    #[test]
    fn average_body_on_empty_history_is_nan_not_a_panic() {
        let t = ProfilingTimer::new();
        assert_eq!(t.size(), 0);

        let sum = t.history.iter().fold(0.0_f64, |acc, &v| acc + v);
        let avg = sum / t.history.len() as f64;

        // The fold over an empty range returns its seed unchanged, so this
        // is 0.0 / 0.0 -- NaN, not a panic and not a divide-by-zero trap.
        assert_eq!(sum, 0.0);
        assert!(avg.is_nan(), "expected NaN from 0.0 / 0.0, got {avg}");
    }

    #[test]
    fn average_on_freshly_sized_zero_filled_buffer_is_zero() {
        let t = ProfilingTimer::with_history_count(4);
        assert_eq!(t.average(), 0.0);
    }

    #[test]
    fn average_on_partially_filled_buffer_divides_by_full_size_not_logged_count() {
        let mut t = ProfilingTimer::with_history_count(4);
        t.log(4.0);
        t.log(8.0);
        // Sum = 4 + 8 + 0 + 0 = 12; divided by size() = 4, not by the 2
        // values actually logged: 12 / 4 = 3.0, not 12 / 2 = 6.0.
        assert_eq!(t.average(), 3.0);
    }

    #[test]
    fn average_on_exactly_full_buffer_divides_by_size() {
        let mut t = ProfilingTimer::with_history_count(4);
        t.log(1.0);
        t.log(2.0);
        t.log(3.0);
        t.log(4.0);
        // Sum = 10, size = 4 -> 2.5.
        assert_eq!(t.average(), 2.5);
    }

    #[test]
    fn average_after_wrap_around_uses_only_current_history_contents() {
        let mut t = ProfilingTimer::with_history_count(3);
        t.log(1.0);
        t.log(2.0);
        t.log(3.0);
        t.log(4.0); // overwrites slot 0: history is now [4, 2, 3]
                    // Sum = 9, size = 3 -> 3.0.
        assert_eq!(t.average(), 3.0);
    }

    #[test]
    fn average_single_slot_buffer() {
        let mut t = ProfilingTimer::with_history_count(1);
        t.log(42.0);
        assert_eq!(t.average(), 42.0);
    }

    #[test]
    fn average_after_clear_is_zero() {
        let mut t = ProfilingTimer::with_history_count(3);
        t.log(1.0);
        t.log(2.0);
        t.log(3.0);
        t.clear();
        assert_eq!(t.average(), 0.0);
    }

    #[test]
    fn average_after_reset_is_unchanged_since_reset_does_not_touch_history() {
        let mut t = ProfilingTimer::with_history_count(3);
        t.log(1.0);
        t.log(2.0);
        t.log(3.0);
        let before = t.average();
        t.reset();
        assert_eq!(t.average(), before);
        assert_eq!(t.average(), 2.0);
    }

    #[test]
    fn average_accumulates_left_to_right_matching_std_accumulate_order() {
        // Use values whose floating-point sum is order-sensitive at f64
        // precision to pin down that this port folds left-to-right, not via
        // an order-independent reduction.
        let mut t = ProfilingTimer::with_history_count(3);
        t.log(1.0);
        t.log(1e16);
        t.log(-1e16);
        // Left fold: ((0.0 + 1.0) + 1e16) + (-1e16).
        // (0.0 + 1.0) = 1.0; (1.0 + 1e16) rounds to 1e16 at f64 precision
        // (1.0 is below 1e16's ULP); (1e16 + -1e16) = 0.0.
        // So the strict left fold yields 0.0 here, not 1.0 (which a
        // reassociated (1e16 + -1e16) + 1.0 = 0.0 + 1.0 = 1.0 would give).
        let sum = 0.0_f64;
        let sum = sum + 1.0;
        let sum = sum + 1e16;
        let sum = sum + (-1e16);
        assert_eq!(sum, 0.0);
        assert_eq!(t.average(), sum / 3.0);
    }

    #[test]
    fn average_negative_values_average_correctly() {
        let mut t = ProfilingTimer::with_history_count(2);
        t.log(-4.0);
        t.log(-6.0);
        assert_eq!(t.average(), -5.0);
    }

    // --- data() reflects live history contents ---

    #[test]
    fn data_after_several_logs_matches_history_vec() {
        let mut t = ProfilingTimer::with_history_count(3);
        t.log(1.0);
        t.log(2.0);
        t.log(3.0);
        t.log(4.0);
        assert_eq!(t.data(), &[4.0, 2.0, 3.0]);
    }

    // --- accumulation field: plain storage, only reset()/clear() touch it ---

    #[test]
    fn accumulation_is_directly_settable_and_survives_log() {
        let mut t = ProfilingTimer::with_history_count(2);
        t.accumulation = 99.5;
        t.log(1.0);
        assert_eq!(t.accumulation, 99.5);
    }

    #[test]
    fn accumulation_default_is_zero() {
        assert_eq!(ProfilingTimer::new().accumulation, 0.0);
        assert_eq!(ProfilingTimer::with_history_count(5).accumulation, 0.0);
    }
}
