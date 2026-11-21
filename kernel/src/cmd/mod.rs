//! Registered kernel commands and their resumable execution contract.
use crate::fiber::{Chan, Fail, Fiber, HostReq, Outcome};
use crate::fs::Vfs;
use crate::limits::Resolved;
use crate::mem::{KBox, KBytes, KChunks, KVec};
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
