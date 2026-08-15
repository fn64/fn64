# Deterministic multiplayer-input foundation

Status: foundation implemented; authoritative SI integration and networking
not implemented.

## Scope

`fn64-runtime::multiplayer` provides the game-neutral mechanism needed before
online multiplayer can be trustworthy:

- typed physical controller ports, per-port successful-read ordinals,
  session-wide SI/PIF poll ordinals, input delay, and opaque session identity;
- a complete `InputBundle` for the session's fixed controller-port set at one
  SI/PIF poll;
- a bounded single-producer mailbox and simulation-owned deterministic reorder
  window;
- strict errors for missing, duplicate, conflicting, stale, wrong-session,
  wrong-port-set, outside-window, exhausted-replay, and disconnected input;
- contiguous authoritative recording and exact replay; and
- an abstract `ControllerInputSource` poll seam with no socket, UI, game, or
  renderer dependency.

This is delayed-lockstep infrastructure. It intentionally contains no peer
discovery, lobby, relay, NAT traversal, authentication, socket, or transport
wire format. A later transport receives and validates packets, constructs
typed bundles, and owns only `InputIngress`. It cannot reach the executor,
RDRAM, devices, or renderer through this API.

## Ownership and timing

```text
local/remote transport owner                 simulation owner

validated future InputBundle
          |
          v
 bounded InputIngress  ----immutable---->  BrokeredInputSource
                                              |
                                  exact InputPollRequest
                                              |
                                              v
                                   authoritative SI poll
```

The mailbox is bounded and non-blocking at ingress. `InputIngress` is `Send`
but deliberately neither `Clone` nor `Sync`, so exactly one coordinator can
move it to and own a producer thread. Its receiver and reorder window are not
shared with the producer. Arrival order may differ from poll order: an
explicitly reported `QueuedAhead` bundle waits in the bounded window. The
consumer commits only its next exact ordinal. A repeated ordinal
is an error even when its bytes match; different bytes at the same ordinal are
a distinct conflicting-duplicate error. A protocol error poisons the brokered
source so a caller cannot ignore one rejected packet and continue from an
ambiguous stream.

`InputDelay` maps an observed local sample to a declared future poll ordinal.
The session layer must explicitly provide the first delayed polls. The
runtime never fills that prefix, predicts a remote input, or substitutes an
idle controller. Waiting is visible as `InputSourceError::Missing` and does
not advance the source.

The existing `ControllerInputSchedule` uses `ControllerPort` and
`ControllerReadOrdinal`, whose counters are independent per port. Netplay
bundles instead use `ControllerPollOrdinal`, one SI/PIF read transaction for
the session's fixed port set. These clocks are separate types because channel
prefixes and port presence can make the per-port successful-operation counts
diverge. A schedule's neutral value outside a declared phase is part of that
complete local script, not a netplay fallback. Its retained raw-index
compatibility method traps when the caller names a port outside `0..=3`;
neutral input never hides an invalid physical port.

## Remaining SI integration seam

The current shell updates live `PifModel` controller state between guest
steps. That cadence is host scheduling policy and is not an authoritative
network clock. The current SI transaction samples controller state while the
timed PIF transaction completes, while scripted routes infer successful read
ordinals afterward from `ControllerOperationEvent::Read` history.

The integration change must therefore add an executor/ABI-owned global
controller-poll ordinal and require one exact `ControllerInputSource` bundle
before the read transaction is admitted, or gate its timed completion before
the PIF samples input. All active ports in that transaction must be latched
from the same bundle. On `Missing`, the runtime must expose a host wait reason
and leave the ordinal and SI transaction uncommitted. It must not keep running
with the previous live input, install neutral input, or let a networking
thread call `set_controller_state`.

That change crosses scheduler/device timing and is deliberately not hidden in
this foundation commit. Its tests need to prove the exact interleaving among
input arrival, SI completion, guest wakeup, and repeated polls before it can be
claimed complete.

## Record/replay role

`RecordingInputSource` appends only bundles successfully consumed by the
wrapped source. A missing poll records nothing. `ReplayInputSource` requires
the same session, active ports, and contiguous next ordinal and fails when the
recording ends. This gives the SI integration a deterministic local oracle
before a real network transport exists.

A persistent wire format is deferred until the session handshake defines the
identities it must bind. At minimum that handshake must bind the exact game
program/ROM identity, runtime build, TV standard, controller-port ownership,
input delay, and gameplay-affecting runtime policy. Renderer settings should
not affect simulation identity unless measurements prove they feed guest-
visible timing or state.

## Why this is not rollback

Delayed lockstep waits for authoritative future inputs. It does not rewind.
The current whole-function execution lane cannot serialize or restore an
arbitrary native coroutine continuation. `docs/DESIGN.md` records this as
"Instruction-exact savestate transplant is NOT REPRESENTABLE here": a saved
PC normally lies inside a recompiled native function, while the lane exposes
only whole-function entry points.

Consequently this module makes no savestate, resynchronization, prediction, or
GGPO-style rollback claim. Generic rollback requires the arbitrary-PC block
lane plus portable state for every runnable/suspended thread and every future-
affecting device. That is a separate architecture program, not an extension of
this input queue.

## Staged path

1. Integrate `ControllerInputSource` at authoritative SI read admission or
   completion and prove local record/replay identity.
2. Add a deterministic state digest for desync detection at a committed
   boundary; a diagnostic evidence snapshot is not automatically sufficient.
3. Implement a loopback session runner with declared input delay and explicit
   startup bundles.
4. Define a versioned transport wire and session handshake, then test two
   processes over loopback/LAN with loss, duplication, delay, and reordering.
5. Add internet session/relay/UI policy separately.

None of those stages should couple renderer presentation or audio-device
delivery to simulation advancement. Render and host audio are consumers of
committed simulation state; controller input is an authority boundary.
