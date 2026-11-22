//! Registered kernel commands and their resumable execution contract.
use crate::fiber::{Chan, Fail, Fiber, HostReq, Outcome};
pub use crate::fiber::{FindTraceStats, TraceDetail, TraceEvent, TraceValue};
use crate::fs::Vfs;
use crate::limits::Resolved;
use crate::mem::{Budget, KBox, KBytes, KChunks, KVec};
use crate::util::Env;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    Standard,
    Network,
}

pub enum CmdPoll {
    Done(i32),
    Yield,
    Host(HostReq),
}

pub trait CmdFrame {
    fn resume(
        &mut self,
        ctx: &mut CmdCtx<'_>,
        outcome: Option<Outcome<'_>>,
    ) -> Result<CmdPoll, Fail>;
}

pub struct CommandSpec {
    pub name: &'static str,
    pub kind: CommandKind,
    pub flags: &'static [&'static str],
    pub start: fn(&mut CmdCtx<'_>, KVec<KBytes>) -> Result<CmdPoll, Fail>,
}

pub struct CmdCtx<'a> {
    pub(crate) fiber: &'a mut Fiber,
    pub xpg_echo: bool,
}

impl CmdCtx<'_> {
    pub fn suspend(&mut self, frame: KBox<dyn CmdFrame>) {
        self.fiber.active = Some(frame);
    }
    pub fn stdin(&mut self) -> &mut KChunks {
        &mut self.fiber.stdin
    }
    pub fn stdout(&mut self, data: &[u8]) -> Result<(), Fail> {
        self.fiber.write_out(Chan::Stdout, data)
    }
    pub fn stderr(&mut self, data: &[u8]) -> Result<(), Fail> {
        self.fiber.write_out(Chan::Stderr, data)
    }
    pub fn cwd(&self) -> &[u8] {
        &self.fiber.cwd
    }
    pub fn env(&mut self) -> &mut Env {
        &mut self.fiber.env
    }
    pub fn vfs(&self) -> &Vfs {
        &self.fiber.shell.root
    }
    pub fn limits(&self) -> &Resolved {
        &self.fiber.shell.lim
    }
    pub fn grants(&self) -> u8 {
        self.fiber.shell.grants
    }
    pub fn charge_work(&mut self, units: u64) -> Result<(), Fail> {
        self.fiber.charge_work(units)
    }
    pub fn live_budget(&self) -> &Budget {
        &self.fiber.live
    }
    pub fn should_yield(&self) -> bool {
        self.fiber.should_yield_observability() || !self.fiber.tick()
    }
    pub fn coverage_enabled(&self) -> bool {
        self.fiber.coverage_enabled()
    }
    pub fn hit_coverage(&mut self, feature: &str) -> Result<(), Fail> {
        self.fiber.hit_coverage(feature)
    }
    pub fn trace_enabled(&self) -> bool {
        self.fiber.trace_enabled()
    }
    pub fn trace_clock_req(&self) -> Option<HostReq> {
        self.fiber.trace_enabled().then_some(HostReq::TraceClock)
    }
    pub fn emit_trace(&mut self, event: TraceEvent) -> Result<(), Fail> {
        self.fiber.push_trace(event)
    }
    pub fn emit_find_trace(
        &mut self,
        raw_search_path: &[u8],
        start_ns: u64,
        end_ns: u64,
        stats: &FindTraceStats,
    ) -> Result<(), Fail> {
        self.fiber.emit_find_trace(raw_search_path, start_ns, end_ns, stats)
    }
}

include!(concat!(env!("OUT_DIR"), "/registry.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_sorted_and_unique() {
        assert!(ALL_COMMANDS
            .windows(2)
            .all(|pair| pair[0].name < pair[1].name));
        assert_eq!(COVERAGE_FEATURES.len(), 184);
        for (id, feature) in COVERAGE_FEATURES.iter().enumerate() {
            assert_eq!(coverage_id(feature), Some(id as u16));
        }
    }
}
