//! Deterministic controller-input transport for local replay and delayed lockstep.
//!
//! This module owns no socket, executor, RDRAM, clock, or renderer. A transport
//! producer can submit immutable future input bundles through the bounded
//! ingress, while the simulation thread exclusively owns the consumer and
//! asks for one exact controller-poll ordinal at the authoritative SI poll.
//! Missing input is an error and never becomes an inferred neutral input.
//!
//! This is not rollback netcode. The whole-function execution lane cannot
//! serialize an arbitrary native coroutine continuation; see `docs/DESIGN.md`
//! "Instruction-exact savestate transplant is NOT REPRESENTABLE here".

use crate::si::ContInput;
use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};

pub const CONTROLLER_PORT_COUNT: usize = 4;

/// One physical N64 controller port, indexed exactly as the PIF model indexes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ControllerPort(u8);

impl ControllerPort {
    pub const PORT_1: Self = Self(0);
    pub const PORT_2: Self = Self(1);
    pub const PORT_3: Self = Self(2);
    pub const PORT_4: Self = Self(3);
    pub const ALL: [Self; CONTROLLER_PORT_COUNT] =
        [Self::PORT_1, Self::PORT_2, Self::PORT_3, Self::PORT_4];

    pub const fn index(self) -> usize {
        self.0 as usize
    }

    pub const fn wire_index(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControllerPortError {
    pub index: usize,
}

impl fmt::Display for ControllerPortError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            output,
            "controller port {} is outside physical ports 0..=3",
            self.index
        )
    }
}

impl std::error::Error for ControllerPortError {}

impl TryFrom<usize> for ControllerPort {
    type Error = ControllerPortError;

    fn try_from(index: usize) -> Result<Self, Self::Error> {
        if index < CONTROLLER_PORT_COUNT {
            Ok(Self(index as u8))
        } else {
            Err(ControllerPortError { index })
        }
    }
}

impl TryFrom<u8> for ControllerPort {
    type Error = ControllerPortError;

    fn try_from(index: u8) -> Result<Self, Self::Error> {
        Self::try_from(usize::from(index))
    }
}

impl From<ControllerPort> for usize {
    fn from(port: ControllerPort) -> Self {
        port.index()
    }
}

impl From<ControllerPort> for u8 {
    fn from(port: ControllerPort) -> Self {
        port.wire_index()
    }
}

/// A compact set of physical controller ports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ControllerPortSet(u8);

impl ControllerPortSet {
    const VALID_BITS: u8 = (1 << CONTROLLER_PORT_COUNT) - 1;

    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn from_ports(ports: impl IntoIterator<Item = ControllerPort>) -> Self {
        let mut set = Self::empty();
        for port in ports {
            set.insert(port);
        }
        set
    }

    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !Self::VALID_BITS == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    pub const fn contains(self, port: ControllerPort) -> bool {
        self.0 & (1 << port.0) != 0
    }

    pub fn insert(&mut self, port: ControllerPort) {
        self.0 |= 1 << port.0;
    }

    pub fn iter(self) -> impl Iterator<Item = ControllerPort> {
        ControllerPort::ALL
            .into_iter()
            .filter(move |port| self.contains(*port))
    }
}

/// Successful-read ordinal for one physical port.
///
/// Scripted evidence counts these independently per port. It is intentionally
/// distinct from [`ControllerPollOrdinal`]: channel prefixes and port
/// presence can make the per-port counters diverge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ControllerReadOrdinal(u64);

impl ControllerReadOrdinal {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Authoritative SI/PIF read-transaction ordinal for a fixed session port set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ControllerPollOrdinal(u64);

impl ControllerPollOrdinal {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn checked_after(self, delay: InputDelay) -> Option<Self> {
        match self.0.checked_add(delay.0) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

/// Delayed-lockstep lead measured in authoritative SI/PIF polls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InputDelay(u64);

impl InputDelay {
    pub const fn new(polls: u64) -> Self {
        Self(polls)
    }

    pub const fn polls(self) -> u64 {
        self.0
    }
}

/// Opaque session identity supplied by the session layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NetplaySessionId([u8; 16]);

impl NetplaySessionId {
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// Complete input for the session's fixed port set at one SI/PIF poll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputBundle {
    session: NetplaySessionId,
    poll_ordinal: ControllerPollOrdinal,
    inputs: [Option<ContInput>; CONTROLLER_PORT_COUNT],
    ports: ControllerPortSet,
}

impl InputBundle {
    pub fn try_new(
        session: NetplaySessionId,
        poll_ordinal: ControllerPollOrdinal,
        inputs: impl IntoIterator<Item = (ControllerPort, ContInput)>,
    ) -> Result<Self, InputBundleError> {
        let mut slots = [None; CONTROLLER_PORT_COUNT];
        let mut ports = ControllerPortSet::empty();
        for (port, input) in inputs {
            if ports.contains(port) {
                return Err(InputBundleError::DuplicatePort { port });
            }
            ports.insert(port);
            slots[port.index()] = Some(input);
        }
        if ports.is_empty() {
            return Err(InputBundleError::Empty);
        }
        Ok(Self {
            session,
            poll_ordinal,
            inputs: slots,
            ports,
        })
    }

    pub const fn session(self) -> NetplaySessionId {
        self.session
    }

    pub const fn poll_ordinal(self) -> ControllerPollOrdinal {
        self.poll_ordinal
    }

    pub const fn ports(self) -> ControllerPortSet {
        self.ports
    }

    pub fn input(self, port: ControllerPort) -> Option<ContInput> {
        self.inputs[port.index()]
    }

    pub fn inputs(self) -> impl Iterator<Item = (ControllerPort, ContInput)> {
        self.ports.iter().map(move |port| {
            (
                port,
                self.inputs[port.index()].expect("port set and slots agree"),
            )
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputBundleError {
    Empty,
    DuplicatePort { port: ControllerPort },
}

impl fmt::Display for InputBundleError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(output, "input bundle names no controller ports"),
            Self::DuplicatePort { port } => write!(
                output,
                "input bundle names controller port {} more than once",
                port.wire_index()
            ),
        }
    }
}

impl std::error::Error for InputBundleError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputPollRequest {
    pub session: NetplaySessionId,
    pub poll_ordinal: ControllerPollOrdinal,
    pub ports: ControllerPortSet,
}

impl InputPollRequest {
    pub const fn new(
        session: NetplaySessionId,
        poll_ordinal: ControllerPollOrdinal,
        ports: ControllerPortSet,
    ) -> Self {
        Self {
            session,
            poll_ordinal,
            ports,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrokerConfigError {
    EmptyPortSet,
}

impl fmt::Display for BrokerConfigError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPortSet => write!(output, "input broker requires at least one active port"),
        }
    }
}

impl std::error::Error for BrokerConfigError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrokerSubmitError {
    WrongSession {
        expected: NetplaySessionId,
        found: NetplaySessionId,
    },
    WrongPorts {
        expected: ControllerPortSet,
        found: ControllerPortSet,
    },
    Stale {
        expected: ControllerPollOrdinal,
        found: ControllerPollOrdinal,
    },
    OutsideWindow {
        expected: ControllerPollOrdinal,
        found: ControllerPollOrdinal,
        capacity: usize,
    },
    Duplicate {
        ordinal: ControllerPollOrdinal,
    },
    ConflictingDuplicate {
        ordinal: ControllerPollOrdinal,
    },
}

impl fmt::Display for BrokerSubmitError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongSession { .. } => write!(output, "input bundle belongs to another session"),
            Self::WrongPorts { expected, found } => write!(
                output,
                "input bundle port set {:#06b} does not match session port set {:#06b}",
                found.bits(),
                expected.bits()
            ),
            Self::Stale { expected, found } => write!(
                output,
                "input ordinal {} is stale; next expected ordinal is {}",
                found.get(),
                expected.get()
            ),
            Self::OutsideWindow {
                expected,
                found,
                capacity,
            } => write!(
                output,
                "input ordinal {} is outside the {capacity}-poll window beginning at {}",
                found.get(),
                expected.get()
            ),
            Self::Duplicate { ordinal } => {
                write!(
                    output,
                    "input ordinal {} was submitted twice",
                    ordinal.get()
                )
            }
            Self::ConflictingDuplicate { ordinal } => write!(
                output,
                "input ordinal {} was resubmitted with different input",
                ordinal.get()
            ),
        }
    }
}

impl std::error::Error for BrokerSubmitError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitDisposition {
    Ready,
    QueuedAhead {
        first_missing: ControllerPollOrdinal,
    },
}

/// Simulation-owned bounded reorder buffer.
#[derive(Debug)]
pub struct DeterministicInputBroker {
    session: NetplaySessionId,
    ports: ControllerPortSet,
    capacity: NonZeroUsize,
    next: ControllerPollOrdinal,
    pending: BTreeMap<ControllerPollOrdinal, InputBundle>,
}

impl DeterministicInputBroker {
    pub fn new(
        session: NetplaySessionId,
        ports: ControllerPortSet,
        capacity: NonZeroUsize,
        first: ControllerPollOrdinal,
    ) -> Result<Self, BrokerConfigError> {
        if ports.is_empty() {
            return Err(BrokerConfigError::EmptyPortSet);
        }
        Ok(Self {
            session,
            ports,
            capacity,
            next: first,
            pending: BTreeMap::new(),
        })
    }

    pub fn submit(&mut self, bundle: InputBundle) -> Result<SubmitDisposition, BrokerSubmitError> {
        if bundle.session != self.session {
            return Err(BrokerSubmitError::WrongSession {
                expected: self.session,
                found: bundle.session,
            });
        }
        if bundle.ports != self.ports {
            return Err(BrokerSubmitError::WrongPorts {
                expected: self.ports,
                found: bundle.ports,
            });
        }
        let ordinal = bundle.poll_ordinal;
        if ordinal < self.next {
            return Err(BrokerSubmitError::Stale {
                expected: self.next,
                found: ordinal,
            });
        }
        let distance = ordinal.get() - self.next.get();
        if distance >= self.capacity.get() as u64 {
            return Err(BrokerSubmitError::OutsideWindow {
                expected: self.next,
                found: ordinal,
                capacity: self.capacity.get(),
            });
        }
        if let Some(existing) = self.pending.get(&ordinal) {
            return Err(if existing == &bundle {
                BrokerSubmitError::Duplicate { ordinal }
            } else {
                BrokerSubmitError::ConflictingDuplicate { ordinal }
            });
        }
        self.pending.insert(ordinal, bundle);
        Ok(if ordinal == self.next {
            SubmitDisposition::Ready
        } else {
            SubmitDisposition::QueuedAhead {
                first_missing: self.next,
            }
        })
    }

    fn take(&mut self, request: InputPollRequest) -> Result<InputBundle, InputSourceError> {
        validate_request(self.session, self.ports, self.next, request)?;
        let next = self
            .next
            .checked_next()
            .ok_or(InputSourceError::OrdinalExhausted)?;
        let bundle = self
            .pending
            .remove(&self.next)
            .ok_or(InputSourceError::Missing { ordinal: self.next })?;
        self.next = next;
        Ok(bundle)
    }
}

impl ControllerInputSource for DeterministicInputBroker {
    fn take_for_poll(
        &mut self,
        request: InputPollRequest,
    ) -> Result<InputBundle, InputSourceError> {
        self.take(request)
    }
}

fn validate_request(
    session: NetplaySessionId,
    ports: ControllerPortSet,
    next: ControllerPollOrdinal,
    request: InputPollRequest,
) -> Result<(), InputSourceError> {
    if request.session != session {
        return Err(InputSourceError::WrongSession {
            expected: session,
            found: request.session,
        });
    }
    if request.ports != ports {
        return Err(InputSourceError::WrongPorts {
            expected: ports,
            found: request.ports,
        });
    }
    if request.poll_ordinal != next {
        return Err(InputSourceError::UnexpectedOrdinal {
            expected: next,
            found: request.poll_ordinal,
        });
    }
    Ok(())
}

/// Common authoritative-SI input seam. Implementations return one exact
/// bundle or an explicit error; there is no default/prediction method.
pub trait ControllerInputSource {
    fn take_for_poll(&mut self, request: InputPollRequest)
        -> Result<InputBundle, InputSourceError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputSourceError {
    WrongSession {
        expected: NetplaySessionId,
        found: NetplaySessionId,
    },
    WrongPorts {
        expected: ControllerPortSet,
        found: ControllerPortSet,
    },
    UnexpectedOrdinal {
        expected: ControllerPollOrdinal,
        found: ControllerPollOrdinal,
    },
    Missing {
        ordinal: ControllerPollOrdinal,
    },
    IngressRejected(BrokerSubmitError),
    Disconnected {
        ordinal: ControllerPollOrdinal,
    },
    ReplayExhausted {
        ordinal: ControllerPollOrdinal,
    },
    OrdinalExhausted,
    RecordingInvariant(InputRecordingError),
}

impl fmt::Display for InputSourceError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongSession { .. } => {
                write!(output, "controller poll belongs to another session")
            }
            Self::WrongPorts { expected, found } => write!(
                output,
                "controller poll port set {:#06b} does not match source port set {:#06b}",
                found.bits(),
                expected.bits()
            ),
            Self::UnexpectedOrdinal { expected, found } => write!(
                output,
                "controller poll requested ordinal {}; source requires {}",
                found.get(),
                expected.get()
            ),
            Self::Missing { ordinal } => write!(
                output,
                "controller input for ordinal {} has not arrived",
                ordinal.get()
            ),
            Self::IngressRejected(error) => write!(output, "input ingress rejected: {error}"),
            Self::Disconnected { ordinal } => write!(
                output,
                "input ingress disconnected before ordinal {} arrived",
                ordinal.get()
            ),
            Self::ReplayExhausted { ordinal } => write!(
                output,
                "input replay ended before ordinal {}",
                ordinal.get()
            ),
            Self::OrdinalExhausted => write!(output, "controller-poll ordinal exhausted u64"),
            Self::RecordingInvariant(error) => write!(output, "input recording invariant: {error}"),
        }
    }
}

impl std::error::Error for InputSourceError {}

/// Producer half of a bounded SPSC mailbox. A network/input coordinator may
/// own this value; it can submit immutable bundles but has no executor access.
pub struct InputIngress {
    sender: SyncSender<InputBundle>,
    _single_producer: PhantomData<std::cell::Cell<()>>,
}

impl InputIngress {
    pub fn try_submit(&self, bundle: InputBundle) -> Result<(), InputMailboxError> {
        self.sender.try_send(bundle).map_err(|error| match error {
            TrySendError::Full(bundle) => InputMailboxError::Full(bundle),
            TrySendError::Disconnected(bundle) => InputMailboxError::Disconnected(bundle),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum InputMailboxError {
    Full(InputBundle),
    Disconnected(InputBundle),
}

impl fmt::Display for InputMailboxError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full(bundle) => write!(
                output,
                "input mailbox is full while submitting ordinal {}",
                bundle.poll_ordinal.get()
            ),
            Self::Disconnected(bundle) => write!(
                output,
                "input mailbox disconnected while submitting ordinal {}",
                bundle.poll_ordinal.get()
            ),
        }
    }
}

impl std::error::Error for InputMailboxError {}

/// Consumer half of the bounded mailbox plus its deterministic reorder buffer.
/// This value is intended to remain exclusively simulation-thread owned.
pub struct BrokeredInputSource {
    receiver: Receiver<InputBundle>,
    broker: DeterministicInputBroker,
    ingress_disconnected: bool,
    protocol_failure: Option<BrokerSubmitError>,
}

impl BrokeredInputSource {
    fn drain_ingress(&mut self) -> Result<(), InputSourceError> {
        if let Some(error) = self.protocol_failure {
            return Err(InputSourceError::IngressRejected(error));
        }
        loop {
            match self.receiver.try_recv() {
                Ok(bundle) => {
                    if let Err(error) = self.broker.submit(bundle) {
                        self.protocol_failure = Some(error);
                        return Err(InputSourceError::IngressRejected(error));
                    }
                }
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => {
                    self.ingress_disconnected = true;
                    return Ok(());
                }
            }
        }
    }
}

impl ControllerInputSource for BrokeredInputSource {
    fn take_for_poll(
        &mut self,
        request: InputPollRequest,
    ) -> Result<InputBundle, InputSourceError> {
        self.drain_ingress()?;
        match self.broker.take(request) {
            Err(InputSourceError::Missing { ordinal }) if self.ingress_disconnected => {
                Err(InputSourceError::Disconnected { ordinal })
            }
            result => result,
        }
    }
}

/// Construct the in-memory loopback used by tests and future transport
/// adapters. Both the mailbox and simulation reorder window are bounded.
pub fn bounded_input_loopback(
    session: NetplaySessionId,
    ports: ControllerPortSet,
    capacity: NonZeroUsize,
    first: ControllerPollOrdinal,
) -> Result<(InputIngress, BrokeredInputSource), BrokerConfigError> {
    let broker = DeterministicInputBroker::new(session, ports, capacity, first)?;
    let (sender, receiver) = mpsc::sync_channel(capacity.get());
    Ok((
        InputIngress {
            sender,
            _single_producer: PhantomData,
        },
        BrokeredInputSource {
            receiver,
            broker,
            ingress_disconnected: false,
            protocol_failure: None,
        },
    ))
}

/// Contiguous authoritative input history suitable for deterministic replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputRecording {
    session: NetplaySessionId,
    ports: ControllerPortSet,
    first: ControllerPollOrdinal,
    next: ControllerPollOrdinal,
    bundles: Vec<InputBundle>,
}

impl InputRecording {
    pub fn new(
        session: NetplaySessionId,
        ports: ControllerPortSet,
        first: ControllerPollOrdinal,
    ) -> Result<Self, BrokerConfigError> {
        if ports.is_empty() {
            return Err(BrokerConfigError::EmptyPortSet);
        }
        Ok(Self {
            session,
            ports,
            first,
            next: first,
            bundles: Vec::new(),
        })
    }

    pub const fn session(&self) -> NetplaySessionId {
        self.session
    }

    pub const fn ports(&self) -> ControllerPortSet {
        self.ports
    }

    pub const fn first_ordinal(&self) -> ControllerPollOrdinal {
        self.first
    }

    pub fn bundles(&self) -> &[InputBundle] {
        &self.bundles
    }

    pub fn append(&mut self, bundle: InputBundle) -> Result<(), InputRecordingError> {
        if bundle.session != self.session {
            return Err(InputRecordingError::WrongSession);
        }
        if bundle.ports != self.ports {
            return Err(InputRecordingError::WrongPorts);
        }
        if bundle.poll_ordinal != self.next {
            return Err(InputRecordingError::NonContiguous {
                expected: self.next,
                found: bundle.poll_ordinal,
            });
        }
        let next = self
            .next
            .checked_next()
            .ok_or(InputRecordingError::OrdinalExhausted)?;
        self.bundles.push(bundle);
        self.next = next;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputRecordingError {
    WrongSession,
    WrongPorts,
    NonContiguous {
        expected: ControllerPollOrdinal,
        found: ControllerPollOrdinal,
    },
    OrdinalExhausted,
}

impl fmt::Display for InputRecordingError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongSession => write!(output, "recorded bundle belongs to another session"),
            Self::WrongPorts => write!(output, "recorded bundle has a different port set"),
            Self::NonContiguous { expected, found } => write!(
                output,
                "recorded input ordinal {} is not next ordinal {}",
                found.get(),
                expected.get()
            ),
            Self::OrdinalExhausted => write!(output, "controller-poll ordinal exhausted u64"),
        }
    }
}

impl std::error::Error for InputRecordingError {}

/// Decorator that records only bundles actually committed by an input source.
pub struct RecordingInputSource<S> {
    source: S,
    recording: InputRecording,
}

impl<S> RecordingInputSource<S> {
    pub fn new(
        source: S,
        session: NetplaySessionId,
        ports: ControllerPortSet,
        first: ControllerPollOrdinal,
    ) -> Result<Self, BrokerConfigError> {
        Ok(Self {
            source,
            recording: InputRecording::new(session, ports, first)?,
        })
    }

    pub fn recording(&self) -> &InputRecording {
        &self.recording
    }

    pub fn into_parts(self) -> (S, InputRecording) {
        (self.source, self.recording)
    }
}

impl<S: ControllerInputSource> ControllerInputSource for RecordingInputSource<S> {
    fn take_for_poll(
        &mut self,
        request: InputPollRequest,
    ) -> Result<InputBundle, InputSourceError> {
        let bundle = self.source.take_for_poll(request)?;
        self.recording
            .append(bundle)
            .map_err(InputSourceError::RecordingInvariant)?;
        Ok(bundle)
    }
}

/// Deterministic source backed by a completed contiguous recording.
pub struct ReplayInputSource {
    recording: InputRecording,
    cursor: usize,
}

impl ReplayInputSource {
    pub fn new(recording: InputRecording) -> Self {
        Self {
            recording,
            cursor: 0,
        }
    }
}

impl ControllerInputSource for ReplayInputSource {
    fn take_for_poll(
        &mut self,
        request: InputPollRequest,
    ) -> Result<InputBundle, InputSourceError> {
        let offset = u64::try_from(self.cursor).expect("input replay cursor exceeds u64");
        let next = self
            .recording
            .first
            .checked_after(InputDelay::new(offset))
            .ok_or(InputSourceError::OrdinalExhausted)?;
        validate_request(self.recording.session, self.recording.ports, next, request)?;
        let bundle = self
            .recording
            .bundles
            .get(self.cursor)
            .copied()
            .ok_or(InputSourceError::ReplayExhausted { ordinal: next })?;
        self.cursor += 1;
        Ok(bundle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use std::sync::{Arc, Barrier};
    use std::thread;

    const SESSION: NetplaySessionId = NetplaySessionId::from_bytes([0x5a; 16]);

    assert_impl_all!(InputIngress: Send);
    assert_not_impl_any!(InputIngress: Sync, Clone);

    fn ports() -> ControllerPortSet {
        ControllerPortSet::from_ports([ControllerPort::PORT_1, ControllerPort::PORT_2])
    }

    fn bundle(ordinal: u64, button: u16) -> InputBundle {
        InputBundle::try_new(
            SESSION,
            ControllerPollOrdinal::new(ordinal),
            [
                (
                    ControllerPort::PORT_1,
                    ContInput {
                        button,
                        stick_x: -7,
                        stick_y: 9,
                    },
                ),
                (ControllerPort::PORT_2, ContInput::default()),
            ],
        )
        .unwrap()
    }

    fn request(ordinal: u64) -> InputPollRequest {
        InputPollRequest::new(SESSION, ControllerPollOrdinal::new(ordinal), ports())
    }

    #[test]
    fn strong_port_and_bundle_types_reject_invalid_shapes() {
        assert!(ControllerPort::try_from(4usize).is_err());
        assert!(ControllerPortSet::from_bits(0x10).is_none());
        assert_eq!(
            InputBundle::try_new(SESSION, ControllerPollOrdinal::ZERO, []),
            Err(InputBundleError::Empty)
        );
        assert!(matches!(
            InputBundle::try_new(
                SESSION,
                ControllerPollOrdinal::ZERO,
                [
                    (ControllerPort::PORT_1, ContInput::default()),
                    (ControllerPort::PORT_1, ContInput::default())
                ]
            ),
            Err(InputBundleError::DuplicatePort {
                port: ControllerPort::PORT_1
            })
        ));
    }

    #[test]
    fn missing_input_never_predicts_or_advances() {
        let (_ingress, mut source) = bounded_input_loopback(
            SESSION,
            ports(),
            NonZeroUsize::new(4).unwrap(),
            ControllerPollOrdinal::ZERO,
        )
        .unwrap();
        assert_eq!(
            source.take_for_poll(request(0)),
            Err(InputSourceError::Missing {
                ordinal: ControllerPollOrdinal::ZERO
            })
        );
        assert_eq!(
            source.take_for_poll(request(1)),
            Err(InputSourceError::UnexpectedOrdinal {
                expected: ControllerPollOrdinal::ZERO,
                found: ControllerPollOrdinal::new(1)
            })
        );
    }

    #[test]
    fn reordered_future_bundles_commit_in_ordinal_order() {
        let (ingress, mut source) = bounded_input_loopback(
            SESSION,
            ports(),
            NonZeroUsize::new(4).unwrap(),
            ControllerPollOrdinal::ZERO,
        )
        .unwrap();
        ingress.try_submit(bundle(1, 0x4000)).unwrap();
        ingress.try_submit(bundle(0, 0x8000)).unwrap();
        assert_eq!(source.take_for_poll(request(0)).unwrap(), bundle(0, 0x8000));
        assert_eq!(source.take_for_poll(request(1)).unwrap(), bundle(1, 0x4000));
    }

    #[test]
    fn duplicate_conflict_stale_and_window_fail_loudly() {
        let mut broker = DeterministicInputBroker::new(
            SESSION,
            ports(),
            NonZeroUsize::new(2).unwrap(),
            ControllerPollOrdinal::ZERO,
        )
        .unwrap();
        assert_eq!(broker.submit(bundle(0, 1)), Ok(SubmitDisposition::Ready));
        assert_eq!(
            broker.submit(bundle(0, 1)),
            Err(BrokerSubmitError::Duplicate {
                ordinal: ControllerPollOrdinal::ZERO
            })
        );
        assert_eq!(
            broker.submit(bundle(0, 2)),
            Err(BrokerSubmitError::ConflictingDuplicate {
                ordinal: ControllerPollOrdinal::ZERO
            })
        );
        assert!(matches!(
            broker.submit(bundle(2, 2)),
            Err(BrokerSubmitError::OutsideWindow { .. })
        ));
        broker.take(request(0)).unwrap();
        assert!(matches!(
            broker.submit(bundle(0, 1)),
            Err(BrokerSubmitError::Stale { .. })
        ));
    }

    #[test]
    fn session_and_fixed_port_set_are_part_of_broker_identity() {
        let mut broker = DeterministicInputBroker::new(
            SESSION,
            ports(),
            NonZeroUsize::new(2).unwrap(),
            ControllerPollOrdinal::ZERO,
        )
        .unwrap();
        let other_session = InputBundle::try_new(
            NetplaySessionId::from_bytes([0x6b; 16]),
            ControllerPollOrdinal::ZERO,
            [
                (ControllerPort::PORT_1, ContInput::default()),
                (ControllerPort::PORT_2, ContInput::default()),
            ],
        )
        .unwrap();
        assert!(matches!(
            broker.submit(other_session),
            Err(BrokerSubmitError::WrongSession { .. })
        ));
        let one_port = InputBundle::try_new(
            SESSION,
            ControllerPollOrdinal::ZERO,
            [(ControllerPort::PORT_1, ContInput::default())],
        )
        .unwrap();
        assert!(matches!(
            broker.submit(one_port),
            Err(BrokerSubmitError::WrongPorts { .. })
        ));
    }

    #[test]
    fn ingress_protocol_failure_poison_is_terminal() {
        let (ingress, mut source) = bounded_input_loopback(
            SESSION,
            ports(),
            NonZeroUsize::new(2).unwrap(),
            ControllerPollOrdinal::ZERO,
        )
        .unwrap();
        ingress.try_submit(bundle(0, 1)).unwrap();
        ingress.try_submit(bundle(0, 1)).unwrap();
        let expected = Err(InputSourceError::IngressRejected(
            BrokerSubmitError::Duplicate {
                ordinal: ControllerPollOrdinal::ZERO,
            },
        ));
        assert_eq!(source.take_for_poll(request(0)), expected);
        assert_eq!(source.take_for_poll(request(0)), expected);
    }

    #[test]
    fn mailbox_capacity_is_a_hard_bound() {
        let (ingress, _source) = bounded_input_loopback(
            SESSION,
            ports(),
            NonZeroUsize::new(1).unwrap(),
            ControllerPollOrdinal::ZERO,
        )
        .unwrap();
        ingress.try_submit(bundle(0, 1)).unwrap();
        assert_eq!(
            ingress.try_submit(bundle(1, 2)),
            Err(InputMailboxError::Full(bundle(1, 2)))
        );
    }

    #[test]
    fn committed_input_records_and_replays_exactly() {
        let (ingress, source) = bounded_input_loopback(
            SESSION,
            ports(),
            NonZeroUsize::new(3).unwrap(),
            ControllerPollOrdinal::ZERO,
        )
        .unwrap();
        ingress.try_submit(bundle(0, 0x8000)).unwrap();
        ingress.try_submit(bundle(1, 0x4000)).unwrap();
        let mut recording_source =
            RecordingInputSource::new(source, SESSION, ports(), ControllerPollOrdinal::ZERO)
                .unwrap();
        recording_source.take_for_poll(request(0)).unwrap();
        recording_source.take_for_poll(request(1)).unwrap();
        let (_, recording) = recording_source.into_parts();
        assert_eq!(recording.bundles(), &[bundle(0, 0x8000), bundle(1, 0x4000)]);

        let mut replay = ReplayInputSource::new(recording);
        assert_eq!(replay.take_for_poll(request(0)).unwrap(), bundle(0, 0x8000));
        assert_eq!(replay.take_for_poll(request(1)).unwrap(), bundle(1, 0x4000));
        assert_eq!(
            replay.take_for_poll(request(2)),
            Err(InputSourceError::ReplayExhausted {
                ordinal: ControllerPollOrdinal::new(2)
            })
        );
    }

    #[test]
    fn producer_consumer_interleavings_preserve_bundle_identity() {
        for pass in 0..64u16 {
            let (ingress, mut source) = bounded_input_loopback(
                SESSION,
                ports(),
                NonZeroUsize::new(2).unwrap(),
                ControllerPollOrdinal::ZERO,
            )
            .unwrap();
            let barrier = Arc::new(Barrier::new(2));
            let producer_barrier = Arc::clone(&barrier);
            let producer = thread::spawn(move || {
                producer_barrier.wait();
                ingress.try_submit(bundle(0, pass)).unwrap();
            });
            barrier.wait();
            let observed = loop {
                match source.take_for_poll(request(0)) {
                    Ok(bundle) => break bundle,
                    Err(InputSourceError::Missing { .. }) => thread::yield_now(),
                    Err(error) => panic!("unexpected input source error: {error}"),
                }
            };
            producer.join().unwrap();
            assert_eq!(observed.input(ControllerPort::PORT_1).unwrap().button, pass);
        }
    }
}
