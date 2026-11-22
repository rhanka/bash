//! Non-recursive explicit-stack VM for L3: pipelines, lists, control flow (`If`, `For`,
//! `While`, `Until`, `Subshell`, `Group`, `FunctionDef`), `time [-p]`, fd-0 heredocs/herestrings,
//! and deferred statement syntax errors.
use super::expand::{Argument, Delimiters, Expander, Fields, Scalar, BYTE_CHUNK};
use super::syntax::{
    identifier, CmdId, Command, ListOp, RedirOp, RedirTarget, SimpleCmd, Span, StmtId, WordId,
};
use super::*;
use crate::cmd::{CommandSpec, COVERAGE_FEATURES};
use crate::fs::{BatchOp, Code, HostKind, HostStat, Kind};
use crate::mem::KMap;
use crate::util::OrderedNames;

pub(super) struct Vm {
    frames: KVec<Frame>,
    pub(super) simple: Option<Simple>,
    functions: KMap<KBytes, CmdId>,
    command_count: u64,
    call_depth: u64,
    pub targets: [Option<usize>; 2],
    pub pipes: KVec<KChunks>,
}

impl Vm {
    pub fn new(live: &Budget) -> Self {
        Self {
            frames: KVec::new_in(live),
            simple: None,
            functions: KMap::new_in(live),
            command_count: 0,
            call_depth: 0,
            targets: [None; 2],
            pipes: KVec::new_in(live),
        }
    }
}

enum Frame {
    StmtList {
        list: Span,
        index: usize,
        top: bool,
        in_cond: bool,
    },
    AndOr {
        stmt_id: StmtId,
        index: usize,
        top: bool,
        in_cond: bool,
    },
    PipelineEnd {
        stmt_id: StmtId,
        index: usize,
        top: bool,
        in_cond: bool,
        negated: bool,
        timed: bool,
        time_posix: bool,
    },
    PipelineStage {
        pipeline_idx: usize,
        stage_idx: usize,
        input: Option<KChunks>,
        snapshot: Option<(Env, Option<usize>)>,
        in_cond: bool,
    },
    StageEnd(KBox<StageEnd>),
    ExecCmd {
        cmd: CmdId,
        in_cond: bool,
    },
    PrepStdin(KBox<PrepStdin>),
    RestoreStdin(KChunks),
    Define {
        name: Span,
        body: CmdId,
        remaining: usize,
    },
    SimpleCmd(CmdId),
    If {
        clauses: Span,
        else_body: Option<Span>,
        index: usize,
        checking_cond: bool,
        in_cond: bool,
    },
    ForInit(KBox<ForInit>),
    ForLoop(KBox<ForLoop>),
    Loop {
        until: bool,
        cond: Span,
        body: Span,
        phase: LoopPhase,
        iter: u64,
        last_body_exit: i32,
        in_cond: bool,
    },
    SubshellEnd(KBox<SubshellEnd>),
    Restore {
        undo: OrderedNames<Option<EnvValue>>,
        remaining: Option<usize>,
    },
    SetLastArg(Option<KRc<KBytes>>),
    FuncCall(KVec<KBytes>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoopPhase {
    CondStart,
    AfterCond,
    AfterBody,
}

struct PrepStdin {
    cmd: CmdId,
    in_cond: bool,
    word: WordId,
    append_nl: bool,
    scalar: Option<Scalar>,
}

struct ForInit {
    var: Span,
    words: Option<Span>,
    body: Span,
    word_idx: usize,
    arg: Option<Argument>,
    pos_fields: Option<Fields>,
    items: KVec<KBytes>,
    delims: Delimiters,
    in_cond: bool,
}

struct ForLoop {
    var: Span,
    body: Span,
    items: KVec<KBytes>,
    index: usize,
    iter: u64,
    last_body_exit: i32,
    ran_body: bool,
    in_cond: bool,
}

struct SubshellEnd {
    env: Env,
    params: KVec<KBytes>,
    cwd: KBytes,
    functions: KMap<KBytes, CmdId>,
    last_arg: Option<KRc<KBytes>>,
}

struct StageEnd {
    env: Env,
    input: Option<KChunks>,
    targets: [Option<usize>; 2],
    pipeline_idx: usize,
    next_stage: Option<usize>,
    previous_exit: i32,
    last_arg: Option<KRc<KBytes>>,
    remaining: Option<usize>,
    in_cond: bool,
}

struct AssignmentState {
    name: Span,
    append: bool,
    value_word: Option<WordId>,
    remaining: usize,
    position: Option<usize>,
}

pub(super) struct Simple {
    entered: bool,
    assign_idx: usize,
    name_done: bool,
    arg_idx: usize,
    assignment: Option<AssignmentState>,
    scalar: Option<Scalar>,
    arg: Option<Argument>,
    name: Option<KBytes>,
    args: KVec<KBytes>,
    undo: OrderedNames<Option<EnvValue>>,
    delims: Delimiters,
    last_arg: KBytes,
    dispatch_ready: bool,
    pub(super) path: PathSearch,
}

#[derive(Default)]
pub(super) struct PathSearch {
    started: bool,
    explicit_left: Option<usize>,
    start: usize,
    end: usize,
    candidate: Option<(core::ops::Range<usize>, usize)>,
    pub(super) resolve_cache: Option<(KBytes, ResolveOutcome)>,
    local_only: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum ResolveOutcome {
    File(u32),
    Dir,
    Missing,
    Denied,
}

impl ResolveOutcome {
    pub(crate) fn of_stat(stat: HostStat) -> Self {
        match stat.kind {
            HostKind::File => Self::File(stat.mode),
            HostKind::Directory | HostKind::Symlink => Self::Dir,
        }
    }
}

enum Lookup {
    Ready(ResolveOutcome),
    Suspend(KBytes),
}

enum Find {
    Ready(Option<Dispatch>),
    Suspend(KBytes),
}

impl Fiber {
    pub(super) fn start_vm(&mut self, root: Span) -> Result<(), Fail> {
        if !root.is_empty() {
            self.vm.frames.try_push(Frame::StmtList {
                list: root,
                index: 0,
                top: true,
                in_cond: false,
            })?;
        }
        Ok(())
    }

    pub(super) fn run_vm(&mut self, program: &Program) -> Result<Option<Step>, Fail> {
        let ast = &program.ast;
        while let Some(frame) = self.vm.frames.pop() {
            match frame {
                Frame::StmtList {
                    list,
                    index,
                    top,
                    in_cond,
                } => {
                    let stmts = &ast.stmt_lists[list.range()];
                    if index >= stmts.len() {
                        continue;
                    }
                    let stmt_id = stmts[index];
                    let stmt = ast.stmts[stmt_id.0 as usize];
                    if let Some(msg) = stmt.deferred_error {
                        return Err(Fail::DeferredSyntax(bytes_in(
                            ast.bytes(msg),
                            &self.live,
                        )?));
                    }
                    if index + 1 < stmts.len() {
                        self.vm.frames.try_push(Frame::StmtList {
                            list,
                            index: index + 1,
                            top,
                            in_cond,
                        })?;
                    }
                    if !stmt.pipelines.is_empty() {
                        self.vm.frames.try_push(Frame::AndOr {
                            stmt_id,
                            index: 0,
                            top,
                            in_cond,
                        })?;
                    }
                }
                Frame::AndOr {
                    stmt_id,
                    index,
                    top,
                    in_cond,
                } => {
                    let stmt = ast.stmts[stmt_id.0 as usize];
                    let count = stmt.pipelines.len();
                    if index >= count {
                        if top {
                            self.update_status()?;
                        }
                        continue;
                    }
                    let run = if index == 0 {
                        true
                    } else {
                        match ast.list_ops[stmt.ops.start as usize + index - 1] {
                            ListOp::And => self.last_exit == 0,
                            ListOp::Or => self.last_exit != 0,
                        }
                    };
                    if run {
                        let pipeline_idx = stmt.pipelines.start as usize + index;
                        let pipe = ast.pipelines[pipeline_idx];
                        self.vm.frames.try_push(Frame::PipelineEnd {
                            stmt_id,
                            index,
                            top,
                            in_cond,
                            negated: pipe.negated,
                            timed: pipe.timed,
                            time_posix: pipe.time_posix,
                        })?;
                        if !pipe.commands.is_empty() {
                            self.vm.frames.try_push(Frame::PipelineStage {
                                pipeline_idx,
                                stage_idx: 0,
                                input: None,
                                snapshot: None,
                                in_cond: in_cond || pipe.negated,
                            })?;
                        } else {
                            self.last_exit = 0;
                        }
                    } else {
                        if !self.tick() {
                            self.vm.frames.try_push(Frame::AndOr {
                                stmt_id,
                                index,
                                top,
                                in_cond,
                            })?;
                            return Ok(Some(Step::OutOfFuel));
                        }
                        if index + 1 < count {
                            self.vm.frames.try_push(Frame::AndOr {
                                stmt_id,
                                index: index + 1,
                                top,
                                in_cond,
                            })?;
                        } else if top {
                            self.update_status()?;
                        }
                    }
                }
                Frame::PipelineEnd {
                    stmt_id,
                    index,
                    top,
                    in_cond,
                    negated,
                    timed,
                    time_posix,
                } => {
                    if timed {
                        let summary = if time_posix {
                            b"real 0.00\nuser 0.00\nsys 0.00\n".as_slice()
                        } else {
                            b"\nreal\t0m0.000s\nuser\t0m0.000s\nsys\t0m0.000s\n".as_slice()
                        };
                        self.write_out(Chan::Stderr, summary)?;
                    }
                    if negated {
                        self.last_exit = i32::from(self.last_exit == 0);
                    }
                    let stmt = ast.stmts[stmt_id.0 as usize];
                    if index + 1 < stmt.pipelines.len() {
                        self.vm.frames.try_push(Frame::AndOr {
                            stmt_id,
                            index: index + 1,
                            top,
                            in_cond,
                        })?;
                    } else if top {
                        self.update_status()?;
                    }
                }
                Frame::PipelineStage {
                    pipeline_idx,
                    stage_idx,
                    input,
                    mut snapshot,
                    in_cond,
                } => {
                    let pipe = ast.pipelines[pipeline_idx];
                    let stage_count = pipe.commands.len();
                    let has_next = stage_idx + 1 < stage_count;
                    let pipe_both = has_next
                        && ast.pipe_stderr[pipe.pipe_stderr.start as usize + stage_idx];

                    if snapshot.is_none() {
                        if !self.take_fuel() {
                            self.vm.frames.try_push(Frame::PipelineStage {
                                pipeline_idx,
                                stage_idx,
                                input,
                                snapshot,
                                in_cond,
                            })?;
                            return Ok(Some(Step::OutOfFuel));
                        }
                        if self.vm.command_count >= self.shell.lim.vals[I_COMMAND_COUNT] {
                            return Err(Fail::CommandCount);
                        }
                        self.vm.command_count += 1;
                        if input.is_some() || has_next {
                            let mut env = Env::new_in(&self.live);
                            env.reserve_entries(self.env.len())?;
                            snapshot = Some((env, None));
                        }
                    }
                    if let Some((mut env, mut remaining)) = snapshot {
                        while let Some((name, entry)) = self.env.entry(env.len()) {
                            let left = remaining.get_or_insert_with(|| {
                                Self::name_work(name.bytes().len(), env.len(), 2)
                            });
                            if !self.pay_bytes(left) {
                                self.vm.frames.try_push(Frame::PipelineStage {
                                    pipeline_idx,
                                    stage_idx,
                                    input,
                                    snapshot: Some((env, remaining)),
                                    in_cond,
                                })?;
                                return Ok(Some(Step::OutOfFuel));
                            }
                            env.insert_shared(name, entry.clone())?;
                            remaining = None;
                        }
                        self.begin_stage(
                            pipeline_idx,
                            has_next.then_some(stage_idx + 1),
                            pipe_both,
                            input,
                            env,
                            in_cond,
                        )?;
                    }
                    let cmd = ast.cmd_lists[pipe.commands.start as usize + stage_idx];
                    self.schedule_command_with_redirs(program, cmd, in_cond)?;
                }
                Frame::StageEnd(mut end) => {
                    while let Some(pos) = self.env.len().checked_sub(1) {
                        let (name, _) = self.env.entry(pos).expect("nonempty environment");
                        let left = end.remaining.get_or_insert_with(|| {
                            Self::name_work(name.bytes().len(), self.env.len(), 1)
                        });
                        if !self.pay_bytes(left) {
                            self.vm.frames.try_push(Frame::StageEnd(end))?;
                            return Ok(Some(Step::OutOfFuel));
                        }
                        drop(self.env.pop());
                        end.remaining = None;
                    }
                    let StageEnd {
                        env,
                        input,
                        targets,
                        pipeline_idx,
                        next_stage,
                        previous_exit,
                        last_arg,
                        remaining: _,
                        in_cond,
                    } = KBox::into_inner(end);
                    self.env = env;
                    self.last_arg = last_arg;
                    self.vm.targets = targets;
                    if let Some(input) = input {
                        self.stdin = input;
                    }
                    if let Some(stage_idx) = next_stage {
                        let input = self.vm.pipes.pop().expect("pipeline has an output buffer");
                        self.last_exit = previous_exit;
                        self.vm.frames.try_push(Frame::PipelineStage {
                            pipeline_idx,
                            stage_idx,
                            input: Some(input),
                            snapshot: None,
                            in_cond,
                        })?;
                    }
                }
                Frame::PrepStdin(mut ps) => {
                    let ex = self.expander(program);
                    let word = ps.word;
                    let sc = ps.scalar.get_or_insert_with(|| Scalar::new(&ex));
                    let Some(mut data) = sc.run(&ex, word, b"")? else {
                        self.vm.frames.try_push(Frame::PrepStdin(ps))?;
                        return Ok(Some(Step::OutOfFuel));
                    };
                    let PrepStdin {
                        cmd,
                        in_cond,
                        append_nl,
                        ..
                    } = KBox::into_inner(ps);
                    if append_nl {
                        data.try_push(b'\n')?;
                    }
                    let mut new_stdin = KChunks::new_in(&self.live);
                    new_stdin.try_append(&data)?;
                    let prior = core::mem::replace(&mut self.stdin, new_stdin);
                    self.vm.frames.try_push(Frame::RestoreStdin(prior))?;
                    self.vm.frames.try_push(Frame::ExecCmd { cmd, in_cond })?;
                }
                Frame::RestoreStdin(prior) => {
                    self.stdin = prior;
                }
                Frame::ExecCmd { cmd, in_cond } => {
                    self.exec_command(program, cmd, in_cond)?;
                }
                Frame::Define {
                    name,
                    body,
                    mut remaining,
                } => {
                    if !self.pay_bytes(&mut remaining) {
                        self.vm.frames.try_push(Frame::Define {
                            name,
                            body,
                            remaining,
                        })?;
                        return Ok(Some(Step::OutOfFuel));
                    }
                    self.hit("bash:cmd:FunctionDef")?;
                    self.vm
                        .functions
                        .try_insert(bytes_in(ast.bytes(name), &self.live)?, body)?;
                    self.last_exit = 0;
                }
                Frame::SimpleCmd(cmd_id) => {
                    let Command::Simple(scmd) = ast.commands[cmd_id.0 as usize] else {
                        unreachable!();
                    };
                    let mut simple = self.vm.simple.take().unwrap_or_else(|| Simple {
                        entered: false,
                        assign_idx: 0,
                        name_done: false,
                        arg_idx: 0,
                        assignment: None,
                        scalar: None,
                        arg: None,
                        name: None,
                        args: KVec::new_in(&self.live),
                        undo: OrderedNames::new_in(&self.live),
                        delims: Delimiters::new(),
                        last_arg: KBytes::new_in(&self.live),
                        dispatch_ready: false,
                        path: PathSearch::default(),
                    });
                    let flow = match self.simple(program, scmd, &mut simple) {
                        Ok(Some(flow)) => flow,
                        Ok(None) => {
                            self.vm.simple = Some(simple);
                            self.vm.frames.try_push(Frame::SimpleCmd(cmd_id))?;
                            return Ok(Some(Step::OutOfFuel));
                        }
                        Err(Fail::BadSubstitution(inner)) => {
                            self.restore_after_command(&mut simple)?;
                            self.command_error(&inner, b"bad substitution", 1)?;
                            Flow::Next
                        }
                        Err(fail) => return Err(fail),
                    };
                    match flow {
                        Flow::Next => {}
                        Flow::Yield => return Ok(Some(Step::OutOfFuel)),
                        Flow::Spin => {
                            self.state = State::Spin;
                            return Ok(Some(Step::OutOfFuel));
                        }
                        Flow::Exit => {
                            self.update_status()?;
                            return Ok(None);
                        }
                        Flow::Host(pending, req) => {
                            if matches!(pending, Pending::ResolveStat) {
                                self.vm.simple = Some(simple);
                                self.vm.frames.try_push(Frame::SimpleCmd(cmd_id))?;
                            }
                            return Ok(Some(self.host(pending, req)));
                        }
                        Flow::Fail(fail) => return Err(fail),
                    }
                }
                Frame::If {
                    clauses,
                    else_body,
                    index,
                    checking_cond,
                    in_cond,
                } => {
                    let slice = &ast.if_clauses[clauses.range()];
                    if checking_cond {
                        if index < slice.len() {
                            self.vm.frames.try_push(Frame::If {
                                clauses,
                                else_body,
                                index,
                                checking_cond: false,
                                in_cond,
                            })?;
                            self.vm.frames.try_push(Frame::StmtList {
                                list: slice[index].cond,
                                index: 0,
                                top: false,
                                in_cond: true,
                            })?;
                        } else if let Some(eb) = else_body {
                            self.vm.frames.try_push(Frame::StmtList {
                                list: eb,
                                index: 0,
                                top: false,
                                in_cond,
                            })?;
                        } else {
                            self.last_exit = 0;
                        }
                    } else if self.last_exit == 0 {
                        self.vm.frames.try_push(Frame::StmtList {
                            list: slice[index].body,
                            index: 0,
                            top: false,
                            in_cond,
                        })?;
                    } else {
                        self.vm.frames.try_push(Frame::If {
                            clauses,
                            else_body,
                            index: index + 1,
                            checking_cond: true,
                            in_cond,
                        })?;
                    }
                }
                Frame::ForInit(mut fi) => {
                    if let Some(words_span) = fi.words {
                        let list = &ast.word_lists[words_span.range()];
                        let mut yielded = false;
                        while fi.word_idx < list.len() {
                            let w = list[fi.word_idx];
                            if fi.arg.is_none() {
                                let (split, _) = self.expander(program).splitting(w);
                                fi.arg = Some(Argument::new(&self.expander(program), w, split));
                            }
                            let fi_ref = &mut *fi;
                            let arg = fi_ref.arg.as_mut().unwrap();
                            if !arg.run(
                                &self.expander(program),
                                w,
                                &mut fi_ref.delims,
                                &mut fi_ref.items,
                            )? {
                                yielded = true;
                                break;
                            }
                            fi.arg = None;
                            fi.word_idx += 1;
                        }
                        if yielded {
                            self.vm.frames.try_push(Frame::ForInit(fi))?;
                            return Ok(Some(Step::OutOfFuel));
                        }
                    } else {
                        // Bare `for var; do ...` expands positional params with IFS splitting.
                        let ex = self.expander(program);
                        let fi_ref = &mut *fi;
                        let pf = fi_ref
                            .pos_fields
                            .get_or_insert_with(|| Fields::new(&ex, true));
                        if !pf.expand_positional(&ex, &mut fi_ref.delims, &mut fi_ref.items)? {
                            self.vm.frames.try_push(Frame::ForInit(fi))?;
                            return Ok(Some(Step::OutOfFuel));
                        }
                    }
                    let ForInit {
                        var,
                        body,
                        items,
                        in_cond,
                        ..
                    } = KBox::into_inner(fi);
                    let fl = KBox::try_new_in(
                        ForLoop {
                            var,
                            body,
                            items,
                            index: 0,
                            iter: 0,
                            last_body_exit: 0,
                            ran_body: false,
                            in_cond,
                        },
                        &self.live,
                    )
                    .map_err(|(_, e)| e)?;
                    self.vm.frames.try_push(Frame::ForLoop(fl))?;
                }
                Frame::ForLoop(mut fl) => {
                    if fl.ran_body {
                        fl.last_body_exit = self.last_exit;
                        fl.ran_body = false;
                    }
                    if fl.index >= fl.items.len() {
                        self.last_exit = fl.last_body_exit;
                        continue;
                    }
                    if !self.tick() {
                        self.vm.frames.try_push(Frame::ForLoop(fl))?;
                        return Ok(Some(Step::OutOfFuel));
                    }
                    fl.iter += 1;
                    let max_iter = limits::index_of("maxLoopIterations")
                        .map_or(u64::MAX, |i| self.shell.lim.vals[i]);
                    if fl.iter > max_iter {
                        return Err(Fail::LoopLimit("for"));
                    }
                    let var_name = ast.bytes(fl.var);
                    let item = &fl.items[fl.index];
                    let exported = self.env.get(var_name).is_some_and(|e| e.exported);
                    self.env.set(var_name, item, exported)?;
                    fl.index += 1;
                    fl.ran_body = true;
                    let body = fl.body;
                    let in_cond = fl.in_cond;
                    self.vm.frames.try_push(Frame::ForLoop(fl))?;
                    self.vm.frames.try_push(Frame::StmtList {
                        list: body,
                        index: 0,
                        top: false,
                        in_cond,
                    })?;
                }
                Frame::Loop {
                    until,
                    cond,
                    body,
                    phase,
                    mut iter,
                    mut last_body_exit,
                    in_cond,
                } => match phase {
                    LoopPhase::CondStart => {
                        if !self.tick() {
                            self.vm.frames.try_push(Frame::Loop {
                                until,
                                cond,
                                body,
                                phase,
                                iter,
                                last_body_exit,
                                in_cond,
                            })?;
                            return Ok(Some(Step::OutOfFuel));
                        }
                        iter += 1;
                        let max_iter = limits::index_of("maxLoopIterations")
                            .map_or(u64::MAX, |i| self.shell.lim.vals[i]);
                        if iter > max_iter {
                            return Err(Fail::LoopLimit(if until { "until" } else { "while" }));
                        }
                        self.vm.frames.try_push(Frame::Loop {
                            until,
                            cond,
                            body,
                            phase: LoopPhase::AfterCond,
                            iter,
                            last_body_exit,
                            in_cond,
                        })?;
                        self.vm.frames.try_push(Frame::StmtList {
                            list: cond,
                            index: 0,
                            top: false,
                            in_cond: true,
                        })?;
                    }
                    LoopPhase::AfterCond => {
                        let done = if until {
                            self.last_exit == 0
                        } else {
                            self.last_exit != 0
                        };
                        if done {
                            self.last_exit = last_body_exit;
                        } else {
                            self.vm.frames.try_push(Frame::Loop {
                                until,
                                cond,
                                body,
                                phase: LoopPhase::AfterBody,
                                iter,
                                last_body_exit,
                                in_cond,
                            })?;
                            self.vm.frames.try_push(Frame::StmtList {
                                list: body,
                                index: 0,
                                top: false,
                                in_cond,
                            })?;
                        }
                    }
                    LoopPhase::AfterBody => {
                        last_body_exit = self.last_exit;
                        self.vm.frames.try_push(Frame::Loop {
                            until,
                            cond,
                            body,
                            phase: LoopPhase::CondStart,
                            iter,
                            last_body_exit,
                            in_cond,
                        })?;
                    }
                },
                Frame::SubshellEnd(saved) => {
                    let SubshellEnd {
                        env,
                        params,
                        cwd,
                        functions,
                        last_arg,
                    } = KBox::into_inner(saved);
                    self.env = env;
                    self.params = params;
                    self.cwd = cwd;
                    self.vm.functions = functions;
                    self.last_arg = last_arg;
                }
                Frame::Restore {
                    mut undo,
                    mut remaining,
                } => {
                    while let Some(pos) = undo.len().checked_sub(1) {
                        let (name, previous) = undo.entry(pos).expect("nonempty undo stack");
                        let len = name.bytes().len();
                        let left = remaining.get_or_insert_with(|| {
                            let shift = if previous.is_none() {
                                self.env.removal_bytes(name)
                            } else {
                                0
                            };
                            Self::name_work(len, undo.len(), 1)
                                + Self::name_work(len, self.env.len(), 3)
                                + shift
                        });
                        if !self.pay_bytes(left) {
                            self.vm
                                .frames
                                .try_push(Frame::Restore { undo, remaining })?;
                            return Ok(Some(Step::OutOfFuel));
                        }
                        let (name, previous) = undo.pop().expect("nonempty undo stack");
                        match (self.env.position(name.bytes()), previous) {
                            (Some(pos), Some(value)) => {
                                self.env.replace_at(pos, value);
                            }
                            (Some(pos), None) => {
                                self.env.remove_at(pos);
                            }
                            (None, Some(value)) => {
                                self.env.insert_shared(&name, value)?;
                            }
                            (None, None) => {}
                        }
                        remaining = None;
                    }
                }
                Frame::SetLastArg(arg) => self.last_arg = arg,
                Frame::FuncCall(previous) => {
                    self.params = previous;
                    self.vm.call_depth -= 1;
                }
            }
        }
        Ok(None)
    }

    fn command_redirs(ast: &Ast, cmd: CmdId) -> Span {
        match ast.commands[cmd.0 as usize] {
            Command::Simple(s) => s.redirs,
            Command::If { redirs, .. }
            | Command::For { redirs, .. }
            | Command::CStyleFor { redirs, .. }
            | Command::While { redirs, .. }
            | Command::Until { redirs, .. }
            | Command::Case { redirs, .. }
            | Command::Subshell { redirs, .. }
            | Command::Group { redirs, .. }
            | Command::FunctionDef { redirs, .. }
            | Command::Arithmetic { redirs, .. }
            | Command::Conditional { redirs, .. } => redirs,
        }
    }

    fn schedule_command_with_redirs(
        &mut self,
        program: &Program,
        cmd: CmdId,
        in_cond: bool,
    ) -> Result<(), Fail> {
        let ast = &program.ast;
        let redirs = &ast.redirs[Self::command_redirs(ast, cmd).range()];
        // Find the last fd-0 heredoc or herestring redirection on this command.
        let mut last_stdin_redir: Option<(WordId, bool, bool)> = None;
        for r in redirs {
            if r.fd.is_none_or(|f| f == 0) && r.fd_var.is_none() {
                match (r.op, r.target) {
                    (_, RedirTarget::HereDoc { content, quoted, .. }) => {
                        last_stdin_redir = Some((content, false, quoted));
                    }
                    (RedirOp::HereStr, RedirTarget::Word(word)) => {
                        last_stdin_redir = Some((word, true, false));
                    }
                    _ => {}
                }
            }
        }
        if let Some((word, append_nl, quoted_heredoc)) = last_stdin_redir {
            if quoted_heredoc {
                let mut new_stdin = KChunks::new_in(&self.live);
                for &part in &ast.parts[ast.words[word.0 as usize].parts.range()] {
                    if let super::syntax::WordPart::Literal(s) = part {
                        new_stdin.try_append(ast.bytes(s))?;
                    }
                }
                let prior = core::mem::replace(&mut self.stdin, new_stdin);
                self.vm.frames.try_push(Frame::RestoreStdin(prior))?;
                self.vm.frames.try_push(Frame::ExecCmd { cmd, in_cond })?;
            } else {
                let ps = KBox::try_new_in(
                    PrepStdin {
                        cmd,
                        in_cond,
                        word,
                        append_nl,
                        scalar: None,
                    },
                    &self.live,
                )
                .map_err(|(_, e)| e)?;
                self.vm.frames.try_push(Frame::PrepStdin(ps))?;
            }
        } else {
            self.vm.frames.try_push(Frame::ExecCmd { cmd, in_cond })?;
        }
        Ok(())
    }

    fn exec_command(
        &mut self,
        program: &Program,
        cmd: CmdId,
        in_cond: bool,
    ) -> Result<(), Fail> {
        let ast = &program.ast;
        match ast.commands[cmd.0 as usize] {
            Command::Simple(_) => {
                self.vm.frames.try_push(Frame::SimpleCmd(cmd))?;
            }
            Command::If {
                clauses, else_body, ..
            } => {
                self.hit("bash:cmd:If")?;
                self.vm.frames.try_push(Frame::If {
                    clauses,
                    else_body,
                    index: 0,
                    checking_cond: true,
                    in_cond,
                })?;
            }
            Command::For {
                var, words, body, ..
            } => {
                self.hit("bash:cmd:For")?;
                let var_bytes = ast.bytes(var);
                if !identifier(var_bytes) {
                    let var_str = core::str::from_utf8(var_bytes).unwrap_or("?");
                    let msg = fmt_in(&self.live, |s| {
                        write!(s, "bash: `{var_str}': not a valid identifier\n")
                    })?;
                    self.write_out(Chan::Stderr, msg.as_str().as_bytes())?;
                    self.last_exit = 1;
                    return Ok(());
                }
                let fi = KBox::try_new_in(
                    ForInit {
                        var,
                        words,
                        body,
                        word_idx: 0,
                        arg: None,
                        pos_fields: None,
                        items: KVec::new_in(&self.live),
                        delims: Delimiters::new(),
                        in_cond,
                    },
                    &self.live,
                )
                .map_err(|(_, e)| e)?;
                self.vm.frames.try_push(Frame::ForInit(fi))?;
            }
            Command::While { cond, body, .. } => {
                self.hit("bash:cmd:While")?;
                self.vm.frames.try_push(Frame::Loop {
                    until: false,
                    cond,
                    body,
                    phase: LoopPhase::CondStart,
                    iter: 0,
                    last_body_exit: 0,
                    in_cond,
                })?;
            }
            Command::Until { cond, body, .. } => {
                self.hit("bash:cmd:Until")?;
                self.vm.frames.try_push(Frame::Loop {
                    until: true,
                    cond,
                    body,
                    phase: LoopPhase::CondStart,
                    iter: 0,
                    last_body_exit: 0,
                    in_cond,
                })?;
            }
            Command::Subshell { body, .. } => {
                self.hit("bash:cmd:Subshell")?;
                if body.is_empty() {
                    self.last_exit = 0;
                    return Ok(());
                }
                let saved_env = self.env.try_clone()?;
                let mut saved_params = KVec::with_capacity_in(self.params.len(), &self.live)?;
                for p in self.params.iter() {
                    saved_params.try_push(bytes_in(p, &self.live)?)?;
                }
                let saved_cwd = bytes_in(&self.cwd, &self.live)?;
                let mut saved_fns = KMap::new_in(&self.live);
                for (k, &v) in self.vm.functions.iter() {
                    saved_fns.try_insert(bytes_in(k, &self.live)?, v)?;
                }
                let saved = KBox::try_new_in(
                    SubshellEnd {
                        env: saved_env,
                        params: saved_params,
                        cwd: saved_cwd,
                        functions: saved_fns,
                        last_arg: self.last_arg.clone(),
                    },
                    &self.live,
                )
                .map_err(|(_, e)| e)?;
                self.vm.frames.try_push(Frame::SubshellEnd(saved))?;
                self.vm.frames.try_push(Frame::StmtList {
                    list: body,
                    index: 0,
                    top: false,
                    in_cond,
                })?;
            }
            Command::Group { body, .. } => {
                self.hit("bash:cmd:Group")?;
                if body.is_empty() {
                    self.last_exit = 0;
                } else {
                    self.vm.frames.try_push(Frame::StmtList {
                        list: body,
                        index: 0,
                        top: false,
                        in_cond,
                    })?;
                }
            }
            Command::FunctionDef { name, body, .. } => {
                let remaining =
                    Self::name_work(ast.bytes(name).len(), self.vm.functions.len(), 2);
                self.vm.frames.try_push(Frame::Define {
                    name,
                    body,
                    remaining,
                })?;
            }
            Command::CStyleFor { .. } => {
                self.hit("bash:cmd:CStyleFor")?;
                self.last_exit = 0;
            }
            Command::Case { .. } => {
                self.hit("bash:cmd:Case")?;
                self.last_exit = 0;
            }
            Command::Arithmetic { .. } => {
                self.hit("bash:cmd:ArithmeticCommand")?;
                self.last_exit = 0;
            }
            Command::Conditional { .. } => {
                self.hit("bash:cmd:ConditionalCommand")?;
                self.last_exit = 0;
            }
        }
        Ok(())
    }

    fn update_status(&mut self) -> Result<(), Fail> {
        let mut status = Diagnostic::new();
        let _ = write!(status, "{}", self.last_exit);
        self.env.set(b"?", status.bytes(), false)?;
        Ok(())
    }

    fn begin_stage(
        &mut self,
        pipeline_idx: usize,
        next_stage: Option<usize>,
        pipe_both: bool,
        input: Option<KChunks>,
        env: Env,
        in_cond: bool,
    ) -> Result<(), Fail> {
        let prior_env = core::mem::replace(&mut self.env, env);
        let prior_input = input.map(|input| core::mem::replace(&mut self.stdin, input));
        let end = StageEnd {
            env: prior_env,
            input: prior_input,
            targets: self.vm.targets,
            pipeline_idx,
            next_stage,
            previous_exit: self.last_exit,
            last_arg: self.last_arg.clone(),
            remaining: None,
            in_cond,
        };
        if next_stage.is_some() {
            let i = self.vm.pipes.len();
            self.vm.pipes.try_push(KChunks::new_in(&self.out_budget))?;
            self.vm.targets[0] = Some(i);
            if pipe_both {
                self.vm.targets[1] = Some(i);
            }
        }
        let end = KBox::try_new_in(end, &self.live).map_err(|(_, e)| e)?;
        self.vm.frames.try_push(Frame::StageEnd(end))?;
        Ok(())
    }

    fn hit(&mut self, feature: &str) -> Result<(), Fail> {
        self.hit_coverage(feature)
    }

    fn expander<'a>(&'a self, program: &'a Program) -> Expander<'a> {
        Expander {
            fiber: self,
            ast: &program.ast,
        }
    }

    fn restore_after_command(&mut self, simple: &mut Simple) -> Result<(), Fail> {
        if !simple.undo.is_empty() {
            let undo = core::mem::replace(&mut simple.undo, OrderedNames::new_in(&self.live));
            self.vm.frames.try_push(Frame::Restore {
                undo,
                remaining: None,
            })?;
        }
        Ok(())
    }

    fn simple(
        &mut self,
        program: &Program,
        scmd: SimpleCmd,
        s: &mut Simple,
    ) -> Result<Option<Flow>, Fail> {
        let ast = &program.ast;
        let has_command = scmd.name.is_some();
        if !s.entered {
            self.hit("bash:cmd:SimpleCommand")?;
            s.entered = true;
        }
        let assigns = &ast.assigns[scmd.assigns.range()];
        while s.assign_idx < assigns.len() {
            let assign = assigns[s.assign_idx];
            let a = s.assignment.get_or_insert_with(|| {
                let len = ast.bytes(assign.name).len();
                let mut remaining = Self::name_work(len, self.env.len(), 3);
                if has_command {
                    remaining += Self::name_work(len, s.undo.len(), 3);
                }
                AssignmentState {
                    name: assign.name,
                    append: assign.append,
                    value_word: assign.value,
                    remaining,
                    position: None,
                }
            });
            let name = ast.bytes(a.name);
            if !self.pay_bytes(&mut a.remaining) {
                return Ok(None);
            }
            if readonly(name) {
                if !has_command {
                    return Err(Fail::Readonly(bytes_in(name, &self.live)?));
                }
                self.write_error(name, b"readonly variable")?;
                s.assignment = None;
                s.assign_idx += 1;
                continue;
            }
            let Some(val_word) = a.value_word else {
                // Array literal assignments (`a=(...)`) are executed in L5.
                s.assignment = None;
                s.assign_idx += 1;
                continue;
            };
            let ex = self.expander(program);
            let scalar = s.scalar.get_or_insert_with(|| {
                a.position = self.env.position(name);
                Scalar::new(&ex)
            });
            let previous = a
                .position
                .and_then(|pos| self.env.entry(pos))
                .map(|(_, value)| value);
            let value = scalar.run(
                &ex,
                val_word,
                if a.append {
                    previous.map_or(b"".as_slice(), |e| &e.value)
                } else {
                    b""
                },
            )?;
            let Some(value) = value else {
                return Ok(None);
            };
            s.scalar = None;
            let exported = has_command || previous.is_some_and(|e| e.exported);
            let value = self.env.own_value(value, exported)?;
            let (key, previous) = if let Some(pos) = a.position {
                let previous = self.env.replace_at(pos, value);
                (self.env.entry(pos).expect("just replaced").0, Some(previous))
            } else {
                (self.env.insert(name, value)?, None)
            };
            if has_command && s.undo.position(name).is_none() {
                s.undo.insert_shared(key, previous)?;
            }
            s.assignment = None;
            s.assign_idx += 1;
        }
        let Some(cmd_word) = scmd.name else {
            self.last_arg = None;
            self.last_exit = 0;
            return Ok(Some(Flow::Next));
        };

        if !s.name_done {
            let ex = self.expander(program);
            let scalar = s.scalar.get_or_insert_with(|| Scalar::new(&ex));
            let Some(name) = scalar.run(&ex, cmd_word, b"")? else {
                return Ok(None);
            };
            s.scalar = None;
            s.name = Some(name);
            s.name_done = true;
        }

        let arg_words = &ast.word_lists[scmd.args.range()];
        while s.arg_idx < arg_words.len() {
            let word = arg_words[s.arg_idx];
            if s.arg.is_none() {
                self.hit("bash:expansion:word_glob")?;
                let (split, positional) = self.expander(program).splitting(word);
                if split && !positional {
                    self.hit("bash:expansion:word_split")?;
                }
                s.arg = Some(Argument::new(&self.expander(program), word, split));
            }
            let arg = s.arg.as_mut().expect("argument expansion started");
            if !arg.run(&self.expander(program), word, &mut s.delims, &mut s.args)? {
                return Ok(None);
            }
            s.arg = None;
            s.arg_idx += 1;
        }

        if !s.dispatch_ready {
            let name = s.name.as_mut().expect("command word expanded");
            let elide = name.is_empty() && self.expander(program).is_all_unquoted_param(cmd_word);
            if s.args.len() as u64 + self.args.len() as u64
                > self.shell.lim.vals[I_ARRAY_ELEMENTS]
            {
                return Err(Fail::Arguments);
            }
            if !self.args.is_empty() {
                let mut appended = core::mem::replace(&mut self.args, KVec::new_in(&self.live));
                s.args.try_reserve_exact(appended.len())?;
                for value in appended.as_mut_slice() {
                    s.args
                        .try_push(core::mem::replace(value, KBytes::new_in(&self.live)))?;
                }
            }
            self.restore_after_command(s)?;
            if elide {
                if s.args.is_empty() {
                    self.last_arg = None;
                    self.last_exit = 0;
                    return Ok(Some(Flow::Next));
                }
                s.name = Some(s.args.remove(0));
            }
            s.dispatch_ready = true;
        }

        let name = s.name.as_ref().expect("command word expanded");
        let last_arg = s.args.last().unwrap_or(name);
        s.last_arg
            .try_reserve_exact(last_arg.len() - s.last_arg.len())?;
        while s.last_arg.len() < last_arg.len() {
            if !self.tick() {
                return Ok(None);
            }
            let start = s.last_arg.len();
            s.last_arg.try_extend_from_slice(
                &last_arg[start..(start + BYTE_CHUNK).min(last_arg.len())],
            )?;
        }
        self.dispatch(name, &mut s.args, &mut s.last_arg, &mut s.path)
    }

    fn dispatch(
        &mut self,
        name: &[u8],
        args: &mut KVec<KBytes>,
        last_arg: &mut KBytes,
        search: &mut PathSearch,
    ) -> Result<Option<Flow>, Fail> {
        search.local_only = name
            .rsplit(|&c| c == b'/')
            .next()
            .is_some_and(|base| base.starts_with(b"__"));
        if !search.started {
            if search.explicit_left.is_some() || name.contains(&b'/') {
                let left = search.explicit_left.get_or_insert_with(|| {
                    name.len() + if name.starts_with(b"/") { 0 } else { self.cwd.len() }
                });
                if !self.pay_bytes(left) {
                    return Ok(None);
                }
                let path = self.resolve_path(&self.cwd, name)?;
                return self.explicit_path(name, &path, search, args, last_arg);
            }
            if self.shell.coverage {
                const BUILTINS: core::ops::Range<usize> = 12..71;
                if let Some(id) = BUILTINS
                    .clone()
                    .find(|&id| &COVERAGE_FEATURES[id].as_bytes()[13..] == name)
                {
                    self.hit_coverage_id(id as u16)?;
                }
            }
            if let Some(body) = self.vm.functions.get(name).copied() {
                if self.vm.call_depth >= self.shell.lim.vals[I_CALL_DEPTH] {
                    let func = core::str::from_utf8(name).unwrap_or("?");
                    let func = &func[..func.floor_char_boundary(func.len().min(128))];
                    return Err(Fail::CallDepth(bytes_in(func.as_bytes(), &self.live)?));
                }
                let args = self.finish_dispatch(args, last_arg)?;
                self.hit("bash:cmd:Group")?;
                self.vm.call_depth += 1;
                let previous = core::mem::replace(&mut self.params, args);
                self.vm.frames.try_push(Frame::FuncCall(previous))?;
                self.vm.frames.try_push(Frame::ExecCmd {
                    cmd: body,
                    in_cond: false,
                })?;
                return Ok(Some(Flow::Next));
            }
            // Minimal shell builtins needed for L3 control-flow parity (`true`, `false`, `:`, `[`, `test`).
            if let Some(code) = self.try_shell_builtin(name, args) {
                let _ = self.finish_dispatch(args, last_arg)?;
                self.last_exit = code;
                return Ok(Some(Flow::Next));
            }
        }
        let Some(found) = self.find_command(name, search)? else {
            return Ok(None);
        };
        let command = match found {
            Find::Suspend(path) => return self.emit_resolve(path),
            Find::Ready(command) => command,
        };
        let args = self.finish_dispatch(args, last_arg)?;
        if let Some(command) = command {
            return self.invoke(command, name, args).map(Some);
        }
        #[cfg(feature = "test-commands")]
        if name.starts_with(b"__") {
            let mut borrowed = KVec::new_in(&self.live);
            for arg in args.iter() {
                borrowed.try_push(arg.as_slice())?;
            }
            if let Some(flow) = self.exec_test(name, &borrowed) {
                return Ok(Some(flow));
            }
        }
        self.command_error(name, b"command not found", 127)?;
        Ok(Some(Flow::Next))
    }

    fn try_shell_builtin(&mut self, name: &[u8], args: &[KBytes]) -> Option<i32> {
        match name {
            b"true" | b":" => Some(0),
            b"false" => Some(1),
            b"[" => {
                if args.last().map(KBytes::as_slice) != Some(b"]") {
                    let _ = self.write_out(Chan::Stderr, b"bash: [: missing `]'\n");
                    return Some(2);
                }
                Some(eval_test_args(&args[..args.len() - 1]))
            }
            b"test" => Some(eval_test_args(args)),
            _ => None,
        }
    }

    fn finish_dispatch(
        &mut self,
        args: &mut KVec<KBytes>,
        last_arg: &mut KBytes,
    ) -> Result<KVec<KBytes>, Fail> {
        let last_arg = if last_arg.is_empty() {
            None
        } else {
            let bytes = core::mem::replace(last_arg, KBytes::new_in(&self.live));
            Some(KRc::try_new_in(bytes, &self.live).map_err(|(_, e)| e)?)
        };
        self.vm.frames.try_push(Frame::SetLastArg(last_arg))?;
        Ok(core::mem::replace(args, KVec::new_in(&self.live)))
    }

    fn invoke(&mut self, command: Dispatch, name: &[u8], args: KVec<KBytes>) -> Result<Flow, Fail> {
        match command {
            Dispatch::Custom => self.custom_call(name, args),
            Dispatch::Kernel(spec) => {
                if self.shell.coverage {
                    for arg in args.iter() {
                        for flag in spec.flags {
                            if arg.as_slice() == flag.as_bytes() {
                                let feature = fmt_in(&self.live, |s| {
                                    write!(s, "cmd:flag:{}:{flag}", spec.name)
                                })?;
                                self.hit(feature.as_str())?;
                            }
                        }
                    }
                }
                let poll = (spec.start)(
                    &mut CmdCtx {
                        fiber: self,
                        xpg_echo: false,
                    },
                    args,
                )?;
                match poll {
                    CmdPoll::Done(code) => {
                        self.last_exit = code;
                        Ok(Flow::Next)
                    }
                    CmdPoll::Yield => Ok(Flow::Yield),
                    CmdPoll::Host(req) => Ok(Flow::Host(Pending::Kernel, req)),
                }
            }
            Dispatch::Script => {
                self.command_error(name, b"command not found", 127)?;
                Ok(Flow::Next)
            }
        }
    }

    fn active_command(&self, name: &[u8]) -> Option<Dispatch> {
        if self.shell.is_custom(name)
            && self
                .bypass
                .as_ref()
                .is_none_or(|s| s.as_str().as_bytes() != name)
        {
            Some(Dispatch::Custom)
        } else {
            self.shell.command(name).map(Dispatch::Kernel)
        }
    }

    fn file_dispatch(&self, path: &[u8], name: &[u8], mode: u32) -> Option<Dispatch> {
        let base = path
            .strip_prefix(b"/bin/")
            .or_else(|| path.strip_prefix(b"/usr/bin/"));
        if base == Some(name) {
            if let Some(command) = self.active_command(name) {
                return Some(command);
            }
        }
        (mode & 0o111 != 0).then_some(Dispatch::Script)
    }

    fn stat_lookup(&self, search: &mut PathSearch, path: &[u8]) -> Result<Lookup, Fail> {
        if let Some((cached, outcome)) = &search.resolve_cache {
            if cached.as_slice() == path {
                return Ok(Lookup::Ready(*outcome));
            }
        }
        if !search.local_only && self.shell.root.resolve_host_mount(path).is_some() {
            if !self.shell.granted(G_HOST_FS) {
                return Ok(Lookup::Ready(ResolveOutcome::Missing));
            }
            return Ok(Lookup::Suspend(bytes_in(path, &self.scratch)?));
        }
        match self.shell.root.stat_normalized(path) {
            Ok(s) if s.kind == Kind::File => Ok(Lookup::Ready(ResolveOutcome::File(s.mode))),
            Ok(_) => Ok(Lookup::Ready(ResolveOutcome::Dir)),
            Err(Code::NotFound | Code::NotDirectory) => Ok(Lookup::Ready(ResolveOutcome::Missing)),
            Err(Code::NoSpace) => Err(Fail::Mem(MemError::Ceiling)),
            Err(_) => Ok(Lookup::Ready(ResolveOutcome::Denied)),
        }
    }

    fn resolve_req(&self, path: &[u8]) -> Result<HostReq, Fail> {
        let (id, rest) = self
            .shell
            .root
            .resolve_host_mount(path)
            .ok_or(Fail::Mem(MemError::Ceiling))?;
        let mut ops = KVec::new_in(&self.scratch);
        ops.try_push(BatchOp::Stat(bytes_in(rest, &self.scratch)?))?;
        self.host_fs_batch(id, ops)
            .map_err(|_| Fail::Mem(MemError::Ceiling))
    }

    fn emit_resolve(&mut self, path: KBytes) -> Result<Option<Flow>, Fail> {
        if !self.tick() {
            return Ok(None);
        }
        let req = self.resolve_req(&path)?;
        self.resolve_want = Some(path);
        Ok(Some(Flow::Host(Pending::ResolveStat, req)))
    }

    fn lookup_outcome(&self, path: &[u8], name: &[u8], outcome: ResolveOutcome) -> Option<Dispatch> {
        match outcome {
            ResolveOutcome::File(mode) => self.file_dispatch(path, name, mode),
            _ => None,
        }
    }

    fn explicit_path(
        &mut self,
        original: &[u8],
        path: &[u8],
        search: &mut PathSearch,
        args: &mut KVec<KBytes>,
        last_arg: &mut KBytes,
    ) -> Result<Option<Flow>, Fail> {
        let outcome = match self.stat_lookup(search, path)? {
            Lookup::Ready(outcome) => outcome,
            Lookup::Suspend(path) => return self.emit_resolve(path),
        };
        let args = self.finish_dispatch(args, last_arg)?;
        match outcome {
            ResolveOutcome::File(mode) => {
                let base = path.rsplit(|&c| c == b'/').next().unwrap_or_default();
                if let Some(command) = self.file_dispatch(path, base, mode) {
                    return self
                        .invoke(
                            command,
                            if matches!(command, Dispatch::Script) {
                                original
                            } else {
                                base
                            },
                            args,
                        )
                        .map(Some);
                }
                self.command_error(original, b"Permission denied", 126)?;
            }
            ResolveOutcome::Missing => {
                self.command_error(original, b"No such file or directory", 127)?
            }
            ResolveOutcome::Dir | ResolveOutcome::Denied => {
                self.command_error(original, b"Permission denied", 126)?
            }
        }
        Ok(Some(Flow::Next))
    }

    fn resolve_path(&self, cwd: &[u8], name: &[u8]) -> Result<KBytes, Fail> {
        self.shell
            .root
            .resolve(cwd, name)
            .map_err(|_| Fail::Mem(MemError::Ceiling))
    }

    fn find_command(
        &mut self,
        name: &[u8],
        search: &mut PathSearch,
    ) -> Result<Option<Find>, Fail> {
        if name.is_empty() {
            return Ok(Some(Find::Ready(None)));
        }
        if !search.started {
            let recorded: Option<KBytes> = self
                .shell
                .paths
                .borrow()
                .get(name)
                .map(|p| bytes_in(p, &self.scratch))
                .transpose()
                .map_err(|_| Fail::Mem(MemError::Ceiling))?;
            match recorded {
                Some(path) => match self.stat_lookup(search, &path)? {
                    Lookup::Ready(outcome) => {
                        search.started = true;
                        if let Some(command) = self.lookup_outcome(&path, name, outcome) {
                            return Ok(Some(Find::Ready(Some(command))));
                        }
                        self.shell.paths.borrow_mut().remove(name);
                    }
                    Lookup::Suspend(path) => return Ok(Some(Find::Suspend(path))),
                },
                None => search.started = true,
            }
        }
        let path = self
            .env
            .get(b"PATH")
            .map_or(b"/usr/bin:/bin".as_slice(), |e| &e.value);
        loop {
            if let Some((dir, left)) = &mut search.candidate {
                if !self.pay_bytes(left) {
                    return Ok(None);
                }
                let dir = self.resolve_path(&self.cwd, &path[dir.clone()])?;
                let resolved = self.resolve_path(&dir, name)?;
                match self.stat_lookup(search, &resolved)? {
                    Lookup::Ready(outcome) => {
                        search.candidate = None;
                        if let Some(command) = self.lookup_outcome(&resolved, name, outcome) {
                            self.shell
                                .paths
                                .borrow_mut()
                                .try_insert(bytes_in(name, &self.shell.meta)?, resolved)?;
                            return Ok(Some(Find::Ready(Some(command))));
                        }
                    }
                    Lookup::Suspend(path) => return Ok(Some(Find::Suspend(path))),
                }
            }
            if search.end == path.len() {
                break;
            }
            if !self.tick() {
                return Ok(None);
            }
            let from = search.end;
            let chunk = &path[from..(from + BYTE_CHUNK).min(path.len())];
            let colon = chunk.iter().position(|&b| b == b':');
            search.end += colon.unwrap_or(chunk.len());
            if colon.is_none() && search.end != path.len() {
                continue;
            }
            let dir = search.start..search.end;
            if colon.is_some() {
                search.end += 1;
            }
            search.start = search.end;
            if !dir.is_empty() {
                let cwd_len = if path[dir.start] == b'/' {
                    0
                } else {
                    self.cwd.len()
                };
                let extra = name
                    .len()
                    .saturating_add(cwd_len)
                    .saturating_sub(BYTE_CHUNK);
                search.candidate = Some((dir, extra));
            }
        }
        match self.stat_lookup(search, b"/usr/bin")? {
            Lookup::Ready(ResolveOutcome::Missing) => {
                Ok(Some(Find::Ready(self.active_command(name))))
            }
            Lookup::Ready(_) => Ok(Some(Find::Ready(None))),
            Lookup::Suspend(path) => Ok(Some(Find::Suspend(path))),
        }
    }

    fn write_error(&mut self, name: &[u8], message: &[u8]) -> Result<(), Fail> {
        let len = name.len() + message.len() + b"bash: : \n".len();
        if !self.out_len_ok(len as u64) {
            return Err(Fail::Output);
        }
        for part in [b"bash: ".as_slice(), name, b": ", message, b"\n"] {
            self.write_out(Chan::Stderr, part)?;
        }
        Ok(())
    }

    fn command_error(&mut self, name: &[u8], message: &[u8], code: i32) -> Result<(), Fail> {
        self.last_exit = code;
        self.write_error(name, message)
    }
}

fn eval_test_args(args: &[KBytes]) -> i32 {
    if args.is_empty() {
        return 1;
    }
    if args[0].as_slice() == b"!" && args.len() > 1 {
        return i32::from(eval_test_args(&args[1..]) == 0);
    }
    match args.len() {
        1 => i32::from(args[0].is_empty()),
        2 => match args[0].as_slice() {
            b"-z" => i32::from(!args[1].is_empty()),
            b"-n" => i32::from(args[1].is_empty()),
            _ => 1,
        },
        3 => {
            let l = args[0].as_slice();
            let op = args[1].as_slice();
            let r = args[2].as_slice();
            let ok = match op {
                b"=" | b"==" => l == r,
                b"!=" => l != r,
                b"-eq" => parse_int(l) == parse_int(r),
                b"-ne" => parse_int(l) != parse_int(r),
                b"-lt" => parse_int(l) < parse_int(r),
                b"-le" => parse_int(l) <= parse_int(r),
                b"-gt" => parse_int(l) > parse_int(r),
                b"-ge" => parse_int(l) >= parse_int(r),
                _ => false,
            };
            i32::from(!ok)
        }
        _ => 2,
    }
}

fn parse_int(bytes: &[u8]) -> i64 {
    let (neg, digits) = if let Some(r) = bytes.strip_prefix(b"-") {
        (true, r)
    } else if let Some(r) = bytes.strip_prefix(b"+") {
        (false, r)
    } else {
        (false, bytes)
    };
    let mut acc: i64 = 0;
    for &b in digits {
        if !b.is_ascii_digit() {
            return 0;
        }
        acc = acc.wrapping_mul(10).wrapping_add((b - b'0') as i64);
    }
    if neg {
        acc.wrapping_neg()
    } else {
        acc
    }
}

#[derive(Clone, Copy)]
enum Dispatch {
    Custom,
    Kernel(&'static CommandSpec),
    Script,
}
