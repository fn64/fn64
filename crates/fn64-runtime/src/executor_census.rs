//! Bounded, host-only census of the executor's outer resume boundary.
//!
//! `Executor::run_one_step` is the sole owner of coroutine selection,
//! `Resume` delivery, and `Yield` observation. When
//! `FN64_EXECUTOR_YIELD_CENSUS=1` is present at executor construction, this
//! module counts those typed values per guest thread and times the one outer
//! `GameThread::resume` call. It never adds a clock read inside generated code
//! or a translated block.
//!
//! This is diagnostic evidence, not emulated time. An unarmed snapshot is a
//! distinct enum variant, and exhausting the fixed thread-row budget is
//! explicit in the armed report rather than silently dropping observations.

use std::time::Duration;

use corosensei::CoroutineResult;

use crate::{Resume, ThreadId, Yield};

pub const EXECUTOR_YIELD_CENSUS_ENV: &str = "FN64_EXECUTOR_YIELD_CENSUS";
pub const EXECUTOR_YIELD_CENSUS_THREAD_LIMIT: usize = 64;

const RESUME_KINDS: usize = 5;
const YIELD_KINDS: usize = 9;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutorYieldCensusSnapshot {
    Unarmed,
    Armed(ExecutorYieldCensusReport),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutorYieldCensusReport {
    pub threads: Vec<ExecutorThreadYieldCensus>,
    pub overflow: ExecutorYieldCensusOverflow,
    pub total_resumes: u64,
    pub total_resume_wall_ns: u64,
    pub max_resume_wall_ns: u64,
}

impl ExecutorYieldCensusReport {
    pub fn complete_per_thread(&self) -> bool {
        !self.overflow.row_limit_exceeded
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecutorYieldCensusOverflow {
    /// At least one observation named a thread outside the fixed row budget.
    /// Its identity is deliberately not retained: doing so would make the
    /// diagnostic's memory use unbounded.
    pub row_limit_exceeded: bool,
    pub resumes: [u64; RESUME_KINDS],
    pub yields: [u64; YIELD_KINDS],
    pub returns: u64,
    pub resume_wall_ns: u64,
    pub max_resume_wall_ns: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutorThreadYieldCensus {
    pub thread: ThreadId,
    pub resumes: [u64; RESUME_KINDS],
    pub yields: [u64; YIELD_KINDS],
    pub returns: u64,
    pub resume_wall_ns: u64,
    pub max_resume_wall_ns: u64,
}

impl ExecutorThreadYieldCensus {
    fn new(thread: ThreadId) -> Self {
        Self {
            thread,
            resumes: [0; RESUME_KINDS],
            yields: [0; YIELD_KINDS],
            returns: 0,
            resume_wall_ns: 0,
            max_resume_wall_ns: 0,
        }
    }
}

pub const RESUME_KIND_NAMES: [&str; RESUME_KINDS] = [
    "start",
    "continue",
    "delivered",
    "send_unblocked",
    "would_block",
];

pub const YIELD_KIND_NAMES: [&str; YIELD_KINDS] = [
    "pause_self",
    "stop_self",
    "instruction_checkpoint",
    "recv_block",
    "recv_noblock",
    "send_block_tail",
    "send_block_jam",
    "send_noblock_tail",
    "send_noblock_jam",
];

pub(crate) struct ExecutorYieldCensus {
    armed: bool,
    threads: Vec<ExecutorThreadYieldCensus>,
    overflow: ExecutorYieldCensusOverflow,
    total_resumes: u64,
    total_resume_wall_ns: u64,
    max_resume_wall_ns: u64,
}

impl Default for ExecutorYieldCensus {
    fn default() -> Self {
        Self::new(env_gate())
    }
}

impl ExecutorYieldCensus {
    fn new(armed: bool) -> Self {
        Self {
            armed,
            threads: if armed {
                Vec::with_capacity(EXECUTOR_YIELD_CENSUS_THREAD_LIMIT)
            } else {
                Vec::new()
            },
            overflow: ExecutorYieldCensusOverflow::default(),
            total_resumes: 0,
            total_resume_wall_ns: 0,
            max_resume_wall_ns: 0,
        }
    }

    pub(crate) fn armed(&self) -> bool {
        self.armed
    }

    #[cfg(test)]
    pub(crate) fn armed_for_test() -> Self {
        Self::new(true)
    }

    pub(crate) fn record(
        &mut self,
        thread: ThreadId,
        resume: Resume,
        result: &CoroutineResult<Yield, ()>,
        elapsed: Duration,
    ) {
        if !self.armed {
            return;
        }
        let elapsed_ns = u64::try_from(elapsed.as_nanos())
            .expect("executor yield census outer resume duration exceeds u64 nanoseconds");
        self.total_resumes = checked_inc(self.total_resumes, "total resumes");
        self.total_resume_wall_ns = checked_add(
            self.total_resume_wall_ns,
            elapsed_ns,
            "total outer resume wall time",
        );
        self.max_resume_wall_ns = self.max_resume_wall_ns.max(elapsed_ns);

        if let Some(row) = self.threads.iter_mut().find(|row| row.thread == thread) {
            record_row(row, resume, result, elapsed_ns);
            return;
        }
        if self.threads.len() < EXECUTOR_YIELD_CENSUS_THREAD_LIMIT {
            let mut row = ExecutorThreadYieldCensus::new(thread);
            record_row(&mut row, resume, result, elapsed_ns);
            self.threads.push(row);
            return;
        }

        self.overflow.row_limit_exceeded = true;
        record_overflow(&mut self.overflow, resume, result, elapsed_ns);
    }

    pub(crate) fn snapshot(&self) -> ExecutorYieldCensusSnapshot {
        if !self.armed {
            return ExecutorYieldCensusSnapshot::Unarmed;
        }
        let mut threads = self.threads.clone();
        threads.sort_by_key(|row| row.thread);
        ExecutorYieldCensusSnapshot::Armed(ExecutorYieldCensusReport {
            threads,
            overflow: self.overflow.clone(),
            total_resumes: self.total_resumes,
            total_resume_wall_ns: self.total_resume_wall_ns,
            max_resume_wall_ns: self.max_resume_wall_ns,
        })
    }
}

fn env_gate() -> bool {
    let Some(value) = std::env::var_os(EXECUTOR_YIELD_CENSUS_ENV) else {
        return false;
    };
    match value.to_str() {
        Some("") | Some("0") => false,
        Some("1") => true,
        _ => panic!("{EXECUTOR_YIELD_CENSUS_ENV} must be absent, empty, 0, or 1; got {value:?}"),
    }
}

fn resume_index(resume: Resume) -> usize {
    match resume {
        Resume::Start => 0,
        Resume::Continue => 1,
        Resume::Delivered(_) => 2,
        Resume::SendUnblocked => 3,
        Resume::WouldBlock => 4,
    }
}

fn yield_index(yielded: Yield) -> usize {
    match yielded {
        Yield::PauseSelf => 0,
        Yield::StopSelf => 1,
        Yield::InstructionCheckpoint { .. } => 2,
        Yield::BlockOnRecv {
            may_block: true, ..
        } => 3,
        Yield::BlockOnRecv {
            may_block: false, ..
        } => 4,
        Yield::BlockOnSend {
            may_block: true,
            jam: false,
            ..
        } => 5,
        Yield::BlockOnSend {
            may_block: true,
            jam: true,
            ..
        } => 6,
        Yield::BlockOnSend {
            may_block: false,
            jam: false,
            ..
        } => 7,
        Yield::BlockOnSend {
            may_block: false,
            jam: true,
            ..
        } => 8,
    }
}

fn record_row(
    row: &mut ExecutorThreadYieldCensus,
    resume: Resume,
    result: &CoroutineResult<Yield, ()>,
    elapsed_ns: u64,
) {
    let resume_count = &mut row.resumes[resume_index(resume)];
    *resume_count = checked_inc(*resume_count, "per-thread resume count");
    match result {
        CoroutineResult::Yield(yielded) => {
            let count = &mut row.yields[yield_index(*yielded)];
            *count = checked_inc(*count, "per-thread yield count");
        }
        CoroutineResult::Return(()) => {
            row.returns = checked_inc(row.returns, "per-thread return count");
        }
    }
    row.resume_wall_ns = checked_add(
        row.resume_wall_ns,
        elapsed_ns,
        "per-thread outer resume wall time",
    );
    row.max_resume_wall_ns = row.max_resume_wall_ns.max(elapsed_ns);
}

fn record_overflow(
    overflow: &mut ExecutorYieldCensusOverflow,
    resume: Resume,
    result: &CoroutineResult<Yield, ()>,
    elapsed_ns: u64,
) {
    let resume_count = &mut overflow.resumes[resume_index(resume)];
    *resume_count = checked_inc(*resume_count, "overflow resume count");
    match result {
        CoroutineResult::Yield(yielded) => {
            let count = &mut overflow.yields[yield_index(*yielded)];
            *count = checked_inc(*count, "overflow yield count");
        }
        CoroutineResult::Return(()) => {
            overflow.returns = checked_inc(overflow.returns, "overflow return count");
        }
    }
    overflow.resume_wall_ns = checked_add(
        overflow.resume_wall_ns,
        elapsed_ns,
        "overflow outer resume wall time",
    );
    overflow.max_resume_wall_ns = overflow.max_resume_wall_ns.max(elapsed_ns);
}

fn checked_inc(value: u64, label: &str) -> u64 {
    checked_add(value, 1, label)
}

fn checked_add(value: u64, add: u64, label: &str) -> u64 {
    value
        .checked_add(add)
        .unwrap_or_else(|| panic!("executor yield census {label} overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RdramAddr;

    #[test]
    fn unarmed_snapshot_is_not_a_zero_filled_report() {
        assert_eq!(
            ExecutorYieldCensus::new(false).snapshot(),
            ExecutorYieldCensusSnapshot::Unarmed
        );
    }

    #[test]
    fn armed_census_separates_every_typed_resume_and_yield_shape() {
        let mut census = ExecutorYieldCensus::new(true);
        let queue = RdramAddr::from_offset(0x1000);
        let cases = [
            (Resume::Start, Yield::PauseSelf),
            (Resume::Continue, Yield::StopSelf),
            (
                Resume::Delivered(7),
                Yield::InstructionCheckpoint { instructions: 1 },
            ),
            (
                Resume::SendUnblocked,
                Yield::BlockOnRecv {
                    mq_addr: queue,
                    may_block: true,
                },
            ),
            (
                Resume::WouldBlock,
                Yield::BlockOnRecv {
                    mq_addr: queue,
                    may_block: false,
                },
            ),
            (
                Resume::Continue,
                Yield::BlockOnSend {
                    mq_addr: queue,
                    msg: 1,
                    may_block: true,
                    jam: false,
                },
            ),
            (
                Resume::Continue,
                Yield::BlockOnSend {
                    mq_addr: queue,
                    msg: 1,
                    may_block: true,
                    jam: true,
                },
            ),
            (
                Resume::Continue,
                Yield::BlockOnSend {
                    mq_addr: queue,
                    msg: 1,
                    may_block: false,
                    jam: false,
                },
            ),
            (
                Resume::Continue,
                Yield::BlockOnSend {
                    mq_addr: queue,
                    msg: 1,
                    may_block: false,
                    jam: true,
                },
            ),
        ];
        for (resume, yielded) in cases {
            census.record(
                4,
                resume,
                &CoroutineResult::Yield(yielded),
                Duration::from_nanos(3),
            );
        }
        census.record(
            4,
            Resume::Continue,
            &CoroutineResult::Return(()),
            Duration::from_nanos(5),
        );

        let ExecutorYieldCensusSnapshot::Armed(report) = census.snapshot() else {
            panic!("armed census reported unarmed");
        };
        assert!(report.complete_per_thread());
        assert_eq!(report.total_resumes, 10);
        assert_eq!(report.total_resume_wall_ns, 32);
        assert_eq!(report.max_resume_wall_ns, 5);
        assert_eq!(report.threads.len(), 1);
        assert_eq!(report.threads[0].resumes, [1, 6, 1, 1, 1]);
        assert_eq!(report.threads[0].yields, [1; YIELD_KINDS]);
        assert_eq!(report.threads[0].returns, 1);
    }

    #[test]
    fn thread_rows_are_bounded_and_overflow_is_loud_and_counted() {
        let mut census = ExecutorYieldCensus::new(true);
        for thread in 0..=EXECUTOR_YIELD_CENSUS_THREAD_LIMIT as u32 {
            census.record(
                thread,
                Resume::Start,
                &CoroutineResult::Yield(Yield::PauseSelf),
                Duration::from_nanos(1),
            );
        }
        let ExecutorYieldCensusSnapshot::Armed(report) = census.snapshot() else {
            panic!("armed census reported unarmed");
        };
        assert_eq!(report.threads.len(), EXECUTOR_YIELD_CENSUS_THREAD_LIMIT);
        assert!(!report.complete_per_thread());
        assert!(report.overflow.row_limit_exceeded);
        assert_eq!(report.overflow.resumes[0], 1);
        assert_eq!(report.overflow.yields[0], 1);
        assert_eq!(report.total_resumes, 65);
    }
}
