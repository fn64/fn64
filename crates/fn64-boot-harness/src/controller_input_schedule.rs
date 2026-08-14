//! Deterministic controller input indexed by guest-visible read ordinal.
//!
//! Instruction counts diverge when a runtime substitutes typed host calls,
//! while a controller read is a shared external boundary. Indexing each port
//! independently by its successful read ordinal gives fn64 and a black-box
//! emulator input plugin the same replay clock.

use fn64_runtime::ContInput;
use sha2::{Digest, Sha256};
use std::fmt;

pub const CONTROLLER_INPUT_SCHEDULE_SCHEMA: &str = "fn64.controller-input-schedule.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControllerInputPhase {
    pub port: u8,
    pub first_read: u64,
    pub end_read: u64,
    pub input: ContInput,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerInputSchedule {
    phases: Vec<ControllerInputPhase>,
    source_sha256: [u8; 32],
}

impl ControllerInputSchedule {
    pub fn input_for_read(&self, port: usize, read_ordinal: u64) -> ContInput {
        self.phases
            .iter()
            .find(|phase| {
                usize::from(phase.port) == port
                    && read_ordinal >= phase.first_read
                    && read_ordinal < phase.end_read
            })
            .map(|phase| phase.input)
            .unwrap_or_default()
    }

    pub const fn source_sha256(&self) -> [u8; 32] {
        self.source_sha256
    }

    pub fn source_sha256_hex(&self) -> String {
        use std::fmt::Write as _;
        let mut output = String::with_capacity(64);
        for byte in self.source_sha256 {
            write!(output, "{byte:02x}").expect("writing SHA-256 to String");
        }
        output
    }

    pub fn phases(&self) -> &[ControllerInputPhase] {
        &self.phases
    }

    /// Which physical ports this route actually drives, as a 4-slot mask.
    ///
    /// A route that only ever names port 0 describes a single-player session;
    /// one that names port 1 describes a two-player card and needs a
    /// controller physically present in that port, or the game's
    /// `osContStartQuery` sees `CONT_ABSENT` and never polls it. Deriving the
    /// hardware configuration from the route rather than a separate env var
    /// keeps the two from disagreeing: a schedule cannot declare presses for a
    /// port the harness left empty.
    ///
    /// A phase is only counted when it commands something -- an all-neutral
    /// phase is what a schedule writes to RELEASE a button, so a port whose
    /// every phase is idle was never really driven. This keeps a route that
    /// pads port 1 with neutral filler from silently changing the guest's
    /// probe answers.
    pub fn driven_ports(&self) -> [bool; 4] {
        let mut driven = [false; 4];
        for phase in &self.phases {
            if phase.input != ContInput::default() {
                driven[usize::from(phase.port)] = true;
            }
        }
        driven
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControllerInputScheduleError {
    InvalidUtf8,
    MissingSchema,
    WrongSchema { found: String },
    WrongFieldCount { line: usize, found: usize },
    InvalidNumber { line: usize, field: &'static str },
    InvalidPort { line: usize, port: u8 },
    EmptyRange { line: usize },
    Overlap { line: usize, port: u8 },
}

impl fmt::Display for ControllerInputScheduleError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUtf8 => write!(output, "controller schedule is not UTF-8"),
            Self::MissingSchema => write!(output, "controller schedule is empty"),
            Self::WrongSchema { found } => write!(
                output,
                "controller schedule schema {found:?} is not {CONTROLLER_INPUT_SCHEDULE_SCHEMA:?}"
            ),
            Self::WrongFieldCount { line, found } => write!(
                output,
                "controller schedule line {line} has {found} fields; expected 6"
            ),
            Self::InvalidNumber { line, field } => {
                write!(
                    output,
                    "controller schedule line {line} has invalid {field}"
                )
            }
            Self::InvalidPort { line, port } => {
                write!(
                    output,
                    "controller schedule line {line} names port {port}; expected 0..=3"
                )
            }
            Self::EmptyRange { line } => write!(
                output,
                "controller schedule line {line} has an empty or reversed read range"
            ),
            Self::Overlap { line, port } => write!(
                output,
                "controller schedule line {line} overlaps an earlier phase for port {port}"
            ),
        }
    }
}

impl std::error::Error for ControllerInputScheduleError {}

fn parse_number<T: std::str::FromStr>(
    value: &str,
    line: usize,
    field: &'static str,
) -> Result<T, ControllerInputScheduleError> {
    value
        .parse()
        .map_err(|_| ControllerInputScheduleError::InvalidNumber { line, field })
}

pub fn parse_controller_input_schedule(
    source: &[u8],
) -> Result<ControllerInputSchedule, ControllerInputScheduleError> {
    let text =
        std::str::from_utf8(source).map_err(|_| ControllerInputScheduleError::InvalidUtf8)?;
    let mut lines = text.lines().enumerate().filter_map(|(index, line)| {
        let line = line.split('#').next().unwrap_or_default().trim();
        (!line.is_empty()).then_some((index + 1, line))
    });
    let (_, schema) = lines
        .next()
        .ok_or(ControllerInputScheduleError::MissingSchema)?;
    if schema != CONTROLLER_INPUT_SCHEDULE_SCHEMA {
        return Err(ControllerInputScheduleError::WrongSchema {
            found: schema.to_string(),
        });
    }

    let mut phases = Vec::new();
    let mut last_end = [0u64; 4];
    for (line_number, line) in lines {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let [port, first_read, end_read, buttons, stick_x, stick_y] = fields.as_slice() else {
            return Err(ControllerInputScheduleError::WrongFieldCount {
                line: line_number,
                found: fields.len(),
            });
        };
        let port: u8 = parse_number(port, line_number, "port")?;
        if port >= 4 {
            return Err(ControllerInputScheduleError::InvalidPort {
                line: line_number,
                port,
            });
        }
        let first_read: u64 = parse_number(first_read, line_number, "first_read")?;
        let end_read: u64 = parse_number(end_read, line_number, "end_read")?;
        if first_read >= end_read {
            return Err(ControllerInputScheduleError::EmptyRange { line: line_number });
        }
        if first_read < last_end[usize::from(port)] {
            return Err(ControllerInputScheduleError::Overlap {
                line: line_number,
                port,
            });
        }
        let buttons = u16::from_str_radix(buttons.trim_start_matches("0x"), 16).map_err(|_| {
            ControllerInputScheduleError::InvalidNumber {
                line: line_number,
                field: "buttons_hex",
            }
        })?;
        let stick_x: i8 = parse_number(stick_x, line_number, "stick_x")?;
        let stick_y: i8 = parse_number(stick_y, line_number, "stick_y")?;
        phases.push(ControllerInputPhase {
            port,
            first_read,
            end_read,
            input: ContInput {
                button: buttons,
                stick_x,
                stick_y,
            },
        });
        last_end[usize::from(port)] = end_read;
    }
    Ok(ControllerInputSchedule {
        phases,
        source_sha256: Sha256::digest(source).into(),
    })
}

/// Physically populate every port the route drives, and leave the rest as the
/// `PifModel` default has them.
///
/// This is the missing half of the input seam. `set_controller_state` feeds a
/// port's button/stick bytes, but a port whose `PortState` is `Absent` answers
/// `osContStartQuery` with `CONT_ABSENT` -- so libultra's Controller Manager
/// records it as unconnected in `osContGetQuery`'s bitpattern and the game
/// never issues read-data for it. Feeding inputs to an absent port is
/// therefore inert: the bytes are stored and nothing reads them.
///
/// Port 0 stays populated unconditionally, matching the `PifModel` default, so
/// a route that drives nothing still boots as a single-player session and the
/// committed single-player routes are unaffected.
pub fn attach_controllers_for_driven_ports(driven: [bool; 4]) {
    for (port, driven) in driven.into_iter().enumerate() {
        if driven && port != 0 {
            fn64_abi::set_controller_port_state(
                port,
                fn64_runtime::PortState::StandardControllerNoPak,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_ranges_are_port_local_and_neutral_outside_phases() {
        let source = b"fn64.controller-input-schedule.v1\n\
            0 2 4 1000 0 0\n\
            1 0 1 8000 -3 4\n\
            0 7 8 0008 12 -12\n";
        let schedule = parse_controller_input_schedule(source).unwrap();
        assert_eq!(schedule.input_for_read(0, 1), ContInput::default());
        assert_eq!(schedule.input_for_read(0, 2).button, 0x1000);
        assert_eq!(schedule.input_for_read(0, 4), ContInput::default());
        assert_eq!(schedule.input_for_read(0, 7).button, 0x0008);
        assert_eq!(schedule.input_for_read(1, 0).stick_x, -3);
        assert_eq!(schedule.phases().len(), 3);
        let expected: [u8; 32] = Sha256::digest(source).into();
        assert_eq!(schedule.source_sha256(), expected);
    }

    #[test]
    fn driven_ports_follow_the_route_not_the_line_count() {
        // Port 0 presses A, port 1 deflects its stick -- both are driven, and
        // the harness must physically populate both or the guest's probe
        // answers CONT_ABSENT for port 1 and never reads it.
        let two_player = parse_controller_input_schedule(
            b"fn64.controller-input-schedule.v1\n\
              0 0 4 8000 0 0\n\
              1 0 4 0000 -80 0\n",
        )
        .unwrap();
        assert_eq!(two_player.driven_ports(), [true, true, false, false]);

        // A port whose every phase is neutral is filler, not a player: it
        // must not change the hardware the guest probes.
        let padded = parse_controller_input_schedule(
            b"fn64.controller-input-schedule.v1\n\
              0 0 4 8000 0 0\n\
              1 0 4 0000 0 0\n",
        )
        .unwrap();
        assert_eq!(padded.driven_ports(), [true, false, false, false]);
    }

    #[test]
    fn overlap_and_schema_drift_fail_loudly() {
        assert!(matches!(
            parse_controller_input_schedule(b"wrong\n"),
            Err(ControllerInputScheduleError::WrongSchema { .. })
        ));
        assert!(matches!(
            parse_controller_input_schedule(
                b"fn64.controller-input-schedule.v1\n0 2 5 1000 0 0\n0 4 6 0000 0 0\n"
            ),
            Err(ControllerInputScheduleError::Overlap { .. })
        ));
    }
}
