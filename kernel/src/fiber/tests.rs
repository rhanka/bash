//! Native tests of the bootstrap interpreter. The test-only commands need `--features test-commands`.
use super::*;
use crate::fs::Vfs;
use crate::shell::{Config, G_CUSTOM};

const CEIL: u64 = 1 << 30;

fn shell(profile: usize, overrides: &[(&str, u64)], grants: u8, custom: &[&str]) -> KRc<Shell> {
    crate::mem::testing::reset(CEIL);
    let meta = Budget::root(LimitId::HEADROOM, u64::MAX).unwrap();
    let vfs = KRc::try_new_in(Vfs::create(1 << 20, LimitId(30)).unwrap(), &meta).ok().unwrap();
    let cfg = Config { profile, overrides, grants, custom_commands: custom, env: &[(b"A", b"1", true)], cwd: Some(b"/x") };
    let s = Shell::create(vfs, &cfg, CEIL).unwrap();
    KRc::try_new_in(s, &meta).ok().unwrap()
}

fn fib(sh: &KRc<Shell>, script: &str) -> Fiber {
    Fiber::spawn(sh.clone(), None, &Opts { script: script.as_bytes(), overflow: None, env: &[], replace_env: false, cwd: None, bypass_custom: None }).unwrap()
}

fn run_done(f: &mut Fiber) -> ExecResult {
    loop {
        match f.run(100_000) {
            Step::Done(r) => return r,
            Step::OutOfFuel => {}
            Step::Host { .. } => panic!("host call"),
        }
    }
}

fn drain(f: &mut Fiber, err: bool) -> std::string::String {
    let mut out = std::vec::Vec::new();
    loop {
        let c = if err { f.stderr_chunk(1 << 20) } else { f.stdout_chunk(1 << 20) };
        if c.is_empty() {
            break;
        }
        out.extend_from_slice(c);
        let n = c.len();
        if err { f.stderr_consume(n) } else { f.stdout_consume(n) }
    }
    std::string::String::from_utf8(out).unwrap()
}

#[test]
fn empty_and_unknown() {
    let sh = shell(1, &[], 0, &[]);
    let mut f = fib(&sh, "  \n\t\n");
    let r = run_done(&mut f);
    assert_eq!((r.exit, r.status), (0, Status::Completed));
    assert_eq!(drain(&mut f, false) + &drain(&mut f, true), "");
    let mut f = fib(&sh, "nosuch arg");
    let r = run_done(&mut f);
    assert_eq!(r.exit, 127);
    assert_eq!(drain(&mut f, true), "bash: nosuch: command not found\n");
    assert_eq!(r.env.get(b"?").unwrap().value.as_slice(), b"127");
    assert_eq!(r.env.get(b"A").unwrap().value.as_slice(), b"1");
}

#[test]
fn source_size_checks() {
    let sh = shell(1, &[("maxSourceBytes", 100)], 0, &[]);
    let mut f = fib(&sh, &"x".repeat(205));
    let r = run_done(&mut f);
    assert_eq!((r.exit, r.status), (126, Status::LimitExceeded));
    assert_eq!(drain(&mut f, true), "bash: script input size limit exceeded (100 bytes)\n");
    let sh = shell(1, &[], 0, &[]);
    let mut f = fib(&sh, &" ".repeat(1_000_006));
    let r = run_done(&mut f);
    assert_eq!(r.exit, 2);
    assert_eq!(drain(&mut f, true), "bash: syntax error: Parse error at 1:1: Input too large: 1000006 bytes exceeds limit of 1000000\n");
    let mut f = Fiber::spawn(sh.clone(), None, &Opts { script: b"", overflow: Some((5, 3_145_729)), env: &[], replace_env: false, cwd: None, bypass_custom: None }).unwrap();
    assert_eq!(run_done(&mut f).exit, 126);
}

#[test]
fn custom_command_epochs() {
    let sh = shell(1, &[], G_CUSTOM, &["foo"]);
    let mut f = fib(&sh, "foo a b\nnosuch");
    let Step::Host { epoch, req } = f.run(10) else { panic!() };
    assert_eq!(epoch, 1);
    assert!(matches!(req, HostReq::Custom { .. }));
    assert_eq!(f.resume(9, Outcome::CommandExit(0)).err(), Some(CallError::StaleEpoch));
    assert_eq!(f.read_call_input(2, 0, 10).err(), Some(CallError::StaleEpoch));
    f.push_call_data(1, Chan::Stdout, b"hi").unwrap();
    let Step::Done(r) = f.resume(1, Outcome::CommandExit(3)).unwrap() else { panic!() };
    assert_eq!(r.exit, 127);
    assert_eq!(drain(&mut f, false), "hi");
    // After the end every stateful call is bad-state.
    assert_eq!(f.resume(1, Outcome::CommandExit(0)).err(), Some(CallError::BadState));
}

#[test]
fn cancel_ends_and_keeps_buffers() {
    let sh = shell(1, &[], G_CUSTOM, &["foo"]);
    let mut f = fib(&sh, "foo");
    let Step::Host { epoch, .. } = f.run(10) else { panic!() };
    f.push_call_data(epoch, Chan::Stdout, b"kept").unwrap();
    let r = f.cancel(CancelReason::Aborted);
    assert_eq!((r.exit, r.status), (124, Status::Aborted));
    assert_eq!(f.resume(epoch, Outcome::CommandExit(0)).err(), Some(CallError::BadState));
    assert_eq!(drain(&mut f, false), "kept");
    assert_eq!(drain(&mut f, true), "bash: execution aborted\n");
    assert!(matches!(f.run(1), Step::Done(_)));
}

#[test]
fn stdin_beyond_the_input_limit() {
    let sh = shell(1, &[("maxInputBytes", 10)], 0, &[]);
    let mut f = fib(&sh, "nosuch");
    f.write_stdin(&[0u8; 8], false).unwrap();
    f.write_stdin(&[0u8; 8], true).unwrap();
    let r = run_done(&mut f);
    assert_eq!((r.exit, r.status), (126, Status::LimitExceeded));
    assert_eq!(r.limit_name.as_ref().unwrap().as_str(), "maxInputBytes");
}

#[cfg(feature = "test-commands")]
mod commands {
    use super::*;

    #[test]
    fn output_cap_and_fill() {
        let sh = shell(1, &[("maxOutputSize", 10)], 0, &[]);
        let mut f = fib(&sh, "__out 8\n__err 2");
        let r = run_done(&mut f);
        assert_eq!(r.exit, 0);
        assert_eq!((drain(&mut f, false), drain(&mut f, true)), ("aaaaaaaa".into(), "aa".into()));
        let mut f = fib(&sh, "__out 16");
        let r = run_done(&mut f);
        assert_eq!((r.exit, r.status), (126, Status::LimitExceeded));
        assert_eq!(r.limit_name.as_ref().unwrap().as_str(), "maxOutputSize");
        assert_eq!(drain(&mut f, true), "bash: pipeline: total output size exceeded (>10 bytes), increase executionLimits.maxOutputSize\n");
    }

    #[test]
    fn alloc_beyond_live_budget_then_usable() {
        let sh = shell(1, &[("maxLiveBytes", 10000)], 0, &[]);
        let mut f = fib(&sh, "__alloc 20000");
        let r = run_done(&mut f);
        assert_eq!(r.exit, 126);
        assert_eq!(r.limit_name.as_ref().unwrap().as_str(), "maxLiveBytes");
        assert_eq!(drain(&mut f, true), "bash: maxLiveBytes exceeded\n");
        let mut g = fib(&sh, "__alloc 4000\n__out 3");
        assert_eq!(run_done(&mut g).exit, 0);
        
    }

    #[test]
    fn nested_budget_charges_the_parent() {
        let sh = shell(1, &[("maxLiveBytes", 10000)], 0, &[]);
        let mut p = fib(&sh, "__alloc 6000\n__spin");
        assert!(matches!(p.run(10), Step::OutOfFuel));
        let mut c = Fiber::spawn(sh.clone(), Some(p.live_budget()), &Opts { script: b"__alloc 6000", overflow: None, env: &[], replace_env: false, cwd: None, bypass_custom: None }).unwrap();
        let r = run_done(&mut c);
        assert_eq!(r.exit, 126);
        assert_eq!(r.limit_name.as_ref().unwrap().as_str(), "maxLiveBytes");
    }

    #[test]
    fn spin_runs_out_of_fuel_until_cancelled() {
        let sh = shell(1, &[], 0, &[]);
        let mut f = fib(&sh, "__out 2\n__spin\n__out 3");
        for _ in 0..5 {
            assert!(matches!(f.run(100), Step::OutOfFuel));
        }
        let r = f.cancel(CancelReason::Deadline);
        assert_eq!((r.exit, r.status), (124, Status::Deadline));
        assert_eq!(drain(&mut f, false), "aa");
    }

    #[test]
    fn fuel_slices_resume_where_they_stopped() {
        let sh = shell(1, &[], 0, &[]);
        let mut f = fib(&sh, "__out 1\n__out 1\n__out 1\n__exit 7");
        assert!(matches!(f.run(2), Step::OutOfFuel));
        assert_eq!(drain(&mut f, false), "aa");
        let Step::Done(r) = f.run(100) else { panic!() };
        assert_eq!(r.exit, 7);
        assert_eq!(drain(&mut f, false), "a");
    }

    #[test]
    fn default_deny_and_virtual_behaviour() {
        let sh = shell(1, &[], 0, &[]);
        for cmd in ["__hostfs 1 /x", "__lazy /x", "__fetch http://x"] {
            let mut f = fib(&sh, cmd);
            let r = run_done(&mut f);
            assert_eq!(r.exit, 126, "{cmd}");
            assert_eq!(drain(&mut f, true), "denied\n");
        }
        let mut f = fib(&sh, "__sleep 5000\n__clock");
        assert_eq!(run_done(&mut f).exit, 0);
        assert_eq!(drain(&mut f, false), "5000\n");
        let mut entropy = fib(&sh, "__entropy 16");
        let r = run_done(&mut entropy);
        assert_eq!(r.exit, 0);
        assert!(r.stdout_bytes);
        assert_eq!(entropy.stdout_chunk(16), &[0xad, 0x4d, 0xf3, 0x0b, 0xae, 0x77, 0x1b, 0xdc, 0x76, 0x60, 0x6e, 0x02, 0xb9, 0xee, 0xf0, 0x64]);
    }

    #[test]
    fn granted_host_calls_are_issued() {
        struct StatReply;
        impl<'a> BatchList<'a, BatchResult<'a>> for StatReply {
            fn len(&self) -> usize { 1 }
            fn at(&'a self, index: usize) -> Option<BatchResult<'a>> {
                (index == 0).then_some(Ok(crate::fs::BatchOk::None))
            }
        }
        let sh = shell(1, &[], crate::shell::G_HOST_FS | crate::shell::G_REAL_CLOCK, &[]);
        let mut f = fib(&sh, "__hostfs 4 /p\n__clock");
        let Step::Host { epoch, req } = f.run(100) else { panic!() };
        assert!(matches!(req, HostReq::FsBatch { mount_id: 4, ops } if matches!(&ops[..], [BatchOp::Stat(p)] if p.as_slice() == b"/p")));
        let Step::Host { epoch: e2, .. } = f.resume(epoch, Outcome::Fs(&StatReply)).unwrap() else { panic!() };
        assert_eq!(e2, 2);
        let Step::Done(_) = f.resume(e2, Outcome::Clock(42)).unwrap() else { panic!() };
        assert_eq!(drain(&mut f, false), "ok\n42\n");
    }

    #[test]
    fn stdin_env_and_files() {
        let sh = shell(1, &[], 0, &[]);
        let mut f = fib(&sh, "__stdin\n__setenv B 2\n__write /f hello\n__cat /f\n__cat /nope");
        f.write_stdin(b"in", true).unwrap();
        let r = run_done(&mut f);
        assert_eq!(r.exit, 1);
        assert_eq!(drain(&mut f, false), "inhello");
        assert_eq!(drain(&mut f, true), "cat: /nope: No such file or directory\n");
        assert_eq!(r.env.get(b"B").unwrap().value.as_slice(), b"2");
        assert!(sh.env.get(b"B").is_none(), "the shell's environment is untouched");
    }
}
