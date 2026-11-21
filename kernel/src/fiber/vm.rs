//! Non-recursive execution. A frame schedules at most one child before returning to the dispatch loop.
use super::expand::{Argument, Delimiters, Expander, Scalar, BYTE_CHUNK};
use super::syntax::{assignment, Command, Link, PartKind, Pipe, Span};
use super::*;
use crate::cmd::{coverage_id, CommandSpec, COVERAGE_FEATURES};
use crate::fs::{Code, Kind};
use crate::mem::KMap;

pub(super) struct Vm {
    frames: KVec<Frame>,
    simple: Option<Simple>,
    functions: KMap<KBytes, u32>,
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
        term: u32,
        top: bool,
    },
    AndOr {
        next: Option<u32>,
        top: bool,
        negate: bool,
    },
    Pipeline {
        stage: u32,
        input: Option<KChunks>,
    },
    StageEnd(KBox<StageEnd>),
    SimpleCmd(Span, u32),
    Restore(KVec<Undo>),
    SetLastArg(KBytes),
    FuncCall(KVec<KBytes>),
}

struct StageEnd {
    env: Env,
    input: Option<KChunks>,
    targets: [Option<usize>; 2],
    next: Option<u32>,
    previous_exit: i32,
    last_arg: KBytes,
}

struct Undo {
    name: KBytes,
    previous: Option<(KBytes, bool)>,
}

struct Simple {
    entered: bool,
    word: usize,
    scalar: Option<Scalar>,
    arg: Option<Argument>,
    name: Option<KBytes>,
    args: KVec<KBytes>,
    undo: KVec<Undo>,
    delims: Delimiters,
    last_arg: KBytes,
    dispatch_ready: bool,
    path: PathSearch,
}

#[derive(Default)]
struct PathSearch {
    started: bool,
    start: usize,
    end: usize,
    candidate: Option<(core::ops::Range<usize>, usize)>,
}

impl Fiber {
    pub(super) fn start_vm(&mut self, entry: Option<u32>) -> Result<(), Fail> {
        if let Some(term) = entry {
            self.vm
                .frames
                .try_push(Frame::StmtList { term, top: true })?;
        }
        Ok(())
    }

    pub(super) fn run_vm(&mut self, program: &Program) -> Result<Option<Step>, Fail> {
        let ast = &program.ast;
        while let Some(frame) = self.vm.frames.pop() {
            match frame {
                Frame::StmtList { term: term_id, top } => {
                    let term = ast.terms[term_id as usize];
                    let run = match term.link {
                        Link::And => self.last_exit == 0,
                        Link::Or => self.last_exit != 0,
                        Link::Start | Link::Next => true,
                    };
                    if run {
                        self.vm.frames.try_push(Frame::AndOr {
                            next: term.next,
                            top,
                            negate: term.negate,
                        })?;
                        self.vm.frames.try_push(Frame::Pipeline {
                            stage: term.stage,
                            input: None,
                        })?;
                    } else {
                        if !self.tick() {
                            self.vm
                                .frames
                                .try_push(Frame::StmtList { term: term_id, top })?;
                            return Ok(Some(Step::OutOfFuel));
                        }
                        self.end_term(program, term.next, top)?;
                    }
                }
                Frame::AndOr { next, top, negate } => {
                    if negate {
                        self.last_exit = i32::from(self.last_exit == 0);
                    }
                    self.end_term(program, next, top)?;
                }
                Frame::Pipeline { stage, input } => {
                    if !self.take_fuel() {
                        self.vm.frames.try_push(Frame::Pipeline { stage, input })?;
                        return Ok(Some(Step::OutOfFuel));
                    }
                    if self.vm.command_count >= self.shell.lim.vals[I_COMMAND_COUNT] {
                        return Err(Fail::CommandCount);
                    }
                    self.vm.command_count += 1;
                    let stage = ast.stages[stage as usize];
                    self.begin_stage(stage.pipe, input)?;
                    match stage.command {
                        Command::Simple { words, first_arg } => self
                            .vm
                            .frames
                            .try_push(Frame::SimpleCmd(words, first_arg))?,
                        Command::Group(term) => {
                            self.hit("bash:cmd:Group")?;
                            self.vm
                                .frames
                                .try_push(Frame::StmtList { term, top: false })?;
                        }
                        Command::Define { name, body } => {
                            self.hit("bash:cmd:FunctionDef")?;
                            self.vm.functions.try_insert(
                                bytes_in(name.get(&program.source), &self.live)?,
                                body,
                            )?;
                            self.last_exit = 0;
                        }
                    }
                }
                Frame::StageEnd(end) => {
                    let StageEnd {
                        env,
                        input,
                        targets,
                        next,
                        previous_exit,
                        last_arg,
                    } = KBox::into_inner(end);
                    self.env = env;
                    self.last_arg = last_arg;

                    self.vm.targets = targets;
                    if let Some(input) = input {
                        self.stdin = input;
                    }
                    if let Some(stage) = next {
                        let input = self.vm.pipes.pop().expect("pipeline has an output buffer");
                        self.last_exit = previous_exit;
                        self.vm.frames.try_push(Frame::Pipeline {
                            stage,
                            input: Some(input),
                        })?;
                    }
                }
                Frame::SimpleCmd(words, first_arg) => {
                    let mut simple = self.vm.simple.take().unwrap_or_else(|| Simple {
                        entered: false,
                        word: words.range().start,
                        scalar: None,
                        arg: None,
                        name: None,
                        args: KVec::new_in(&self.live),
                        undo: KVec::new_in(&self.live),
                        delims: Delimiters::new(),
                        last_arg: KBytes::new_in(&self.live),
                        dispatch_ready: false,
                        path: PathSearch::default(),
                    });
                    let flow = match self.simple(program, words, first_arg as usize, &mut simple) {
                        Ok(Some(flow)) => flow,
                        Ok(None) => {
                            self.vm.simple = Some(simple);
                            self.vm
                                .frames
                                .try_push(Frame::SimpleCmd(words, first_arg))?;
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
                        Flow::Host(pending, req) => return Ok(Some(self.host(pending, req))),
                        Flow::Fail(fail) => return Err(fail),
                    }
                }
                Frame::Restore(mut undo) => {
                    while !undo.is_empty() {
                        if !self.tick() {
                            self.vm.frames.try_push(Frame::Restore(undo))?;
                            return Ok(Some(Step::OutOfFuel));
                        }
                        let Undo { name, previous } = undo.pop().expect("nonempty undo stack");
                        if let Some((value, exported)) = previous {
                            self.env.set_owned(&name, value, exported)?;
                        } else {
                            self.env.remove(&name);
                        }
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

    fn end_term(&mut self, program: &Program, next: Option<u32>, top: bool) -> Result<(), Fail> {
        if top && next.is_none_or(|id| program.ast.terms[id as usize].link == Link::Next) {
            self.update_status()?;
        }
        if let Some(term) = next {
            self.vm.frames.try_push(Frame::StmtList { term, top })?;
        }
        Ok(())
    }

    fn update_status(&mut self) -> Result<(), Fail> {
        let mut status = Diagnostic::new();
        let _ = write!(status, "{}", self.last_exit);
        self.env.set(b"?", status.bytes(), false)?;
        Ok(())
    }

    fn begin_stage(&mut self, pipe: Pipe, input: Option<KChunks>) -> Result<(), Fail> {
        if input.is_none() && matches!(pipe, Pipe::Last) {
            return Ok(());
        }
        let env = self.env.try_clone()?;
        let last_arg = self.last_arg.try_clone()?;
        let prior_env = core::mem::replace(&mut self.env, env);
        let prior_input = input.map(|input| core::mem::replace(&mut self.stdin, input));
        let end = StageEnd {
            env: prior_env,
            input: prior_input,
            targets: self.vm.targets,
            next: match pipe {
                Pipe::Last => None,
                Pipe::Stdout(next) | Pipe::Both(next) => Some(next),
            },
            previous_exit: self.last_exit,
            last_arg: core::mem::replace(&mut self.last_arg, last_arg),
        };
        if end.next.is_some() {
            let i = self.vm.pipes.len();
            self.vm.pipes.try_push(KChunks::new_in(&self.out_budget))?;
            self.vm.targets[0] = Some(i);
            if matches!(pipe, Pipe::Both(_)) {
                self.vm.targets[1] = Some(i);
            }
        }

        let end = KBox::try_new_in(end, &self.live).map_err(|(_, e)| e)?;
        self.vm.frames.try_push(Frame::StageEnd(end))?;
        Ok(())
    }

    fn hit(&mut self, feature: &str) -> Result<(), Fail> {
        if self.shell.coverage {
            if let Some(id) = coverage_id(feature) {
                self.coverage.try_push(id)?;
            }
        }
        Ok(())
    }

    fn expander<'a>(&'a self, program: &'a Program) -> Expander<'a> {
        Expander {
            fiber: self,
            ast: &program.ast,
            source: &program.source,
        }
    }

    fn restore_after_command(&mut self, simple: &mut Simple) -> Result<(), Fail> {
        if !simple.undo.is_empty() {
            let undo = core::mem::replace(&mut simple.undo, KVec::new_in(&self.live));
            self.vm.frames.try_push(Frame::Restore(undo))?;
        }
        Ok(())
    }

    fn simple(
        &mut self,
        program: &Program,
        words: Span,
        first_arg: usize,
        s: &mut Simple,
    ) -> Result<Option<Flow>, Fail> {
        let range = words.range();
        let has_command = first_arg < range.end;
        if !s.entered {
            self.hit("bash:cmd:SimpleCommand")?;
            s.entered = true;
        }
        while s.word < first_arg {
            let word = program.ast.word(s.word);
            let (name, append, from) =
                assignment(word, &program.source).expect("parsed assignment");
            let name = name.get(&program.source);
            if readonly(name) {
                if !self.tick() {
                    return Ok(None);
                }
                if !has_command {
                    return Err(Fail::Readonly(bytes_in(name, &self.live)?));
                }
                self.write_error(name, b"readonly variable")?;
                s.word += 1;
                continue;
            }
            let previous = self.env.get(name);
            let ex = self.expander(program);
            let scalar = s.scalar.get_or_insert_with(|| Scalar::new(&ex));
            let value = scalar.run(
                &ex,
                word,
                from,
                if append {
                    previous.map_or(b"".as_slice(), |e| &e.value)
                } else {
                    b""
                },
            )?;
            let Some(value) = value else { return Ok(None) };
            s.scalar = None;
            let exported = has_command || previous.is_some_and(|e| e.exported);
            let previous = self.env.replace_owned(name, value, exported)?;
            if has_command && !s.undo.iter().any(|e| e.name.as_slice() == name) {
                s.undo.try_push(Undo {
                    name: bytes_in(name, &self.live)?,
                    previous,
                })?;
            }
            s.word += 1;
        }
        if !has_command {
            self.last_arg = KBytes::new_in(&self.live);
            self.last_exit = 0;
            return Ok(Some(Flow::Next));
        }

        let word = program.ast.word(first_arg);
        if s.word == first_arg {
            let ex = self.expander(program);
            let scalar = s.scalar.get_or_insert_with(|| Scalar::new(&ex));
            let Some(name) = scalar.run(&ex, word, word.text.start, b"")? else {
                return Ok(None);
            };
            s.scalar = None;
            s.name = Some(name);
            s.word += 1;
        }
        while s.word < range.end {
            let word = program.ast.word(s.word);
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
            s.word += 1;
        }
        if !s.dispatch_ready {
            let name = s.name.as_mut().expect("command word expanded");
            let elide = name.is_empty()
                && !program.ast.parts[word.parts.range()].is_empty()
                && program.ast.parts[word.parts.range()]
                    .iter()
                    .all(|p| p.kind == PartKind::Param);
            if s.args.len() as u64 + self.args.len() as u64 > self.shell.lim.vals[I_ARRAY_ELEMENTS]
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
                    self.last_arg = KBytes::new_in(&self.live);
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
        if !search.started {
            if name.contains(&b'/') {
                let path = self.resolve_path(&self.cwd, name)?;
                let args = self.finish_dispatch(args, last_arg)?;
                return self.explicit_path(name, &path, args).map(Some);
            }
            if self.shell.coverage {
                const BUILTINS: core::ops::Range<usize> = 12..71;
                if let Some(id) = BUILTINS
                    .clone()
                    .find(|&id| &COVERAGE_FEATURES[id].as_bytes()[13..] == name)
                {
                    self.coverage.try_push(id as u16)?;
                }
            }
            if let Some(term) = self.vm.functions.get(name).copied() {
                if self.vm.call_depth >= self.shell.lim.vals[I_CALL_DEPTH] {
                    let func = core::str::from_utf8(name).unwrap_or("?");
                    let func = &func[..func.floor_char_boundary(func.len().min(128))];
                    return Err(Fail::CallDepth(bytes_in(func.as_bytes(), &self.live)?));
                }
                let args = self.finish_dispatch(args, last_arg)?;
                self.vm.call_depth += 1;
                let previous = core::mem::replace(&mut self.params, args);
                self.vm.frames.try_push(Frame::FuncCall(previous))?;
                self.vm
                    .frames
                    .try_push(Frame::StmtList { term, top: false })?;
                return Ok(Some(Flow::Next));
            }
        }
        let Some(command) = self.find_command(name, search)? else {
            return Ok(None);
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

    fn finish_dispatch(
        &mut self,
        args: &mut KVec<KBytes>,
        last_arg: &mut KBytes,
    ) -> Result<KVec<KBytes>, Fail> {
        self.vm
            .frames
            .try_push(Frame::SetLastArg(core::mem::replace(
                last_arg,
                KBytes::new_in(&self.live),
            )))?;
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

    fn explicit_path(
        &mut self,
        original: &[u8],
        path: &[u8],
        args: KVec<KBytes>,
    ) -> Result<Flow, Fail> {
        match self.shell.root.stat_normalized(path) {
            Ok(s) if s.kind == Kind::File => {
                let base = path.rsplit(|&c| c == b'/').next().unwrap_or_default();
                if let Some(command) = self.file_dispatch(path, base, s.mode) {
                    return self.invoke(
                        command,
                        if matches!(command, Dispatch::Script) {
                            original
                        } else {
                            base
                        },
                        args,
                    );
                }
                self.command_error(original, b"Permission denied", 126)?;
            }
            Err(Code::NotFound | Code::NotDirectory) => {
                self.command_error(original, b"No such file or directory", 127)?
            }
            Err(Code::NoSpace) => return Err(Fail::Mem(MemError::Ceiling)),
            _ => self.command_error(original, b"Permission denied", 126)?,
        }
        Ok(Flow::Next)
    }

    fn resolve_path(&self, cwd: &[u8], name: &[u8]) -> Result<KBytes, Fail> {
        self.shell
            .root
            .resolve(cwd, name)
            .map_err(|_| Fail::Mem(MemError::Ceiling))
    }

    // The outer option indicates whether the search finished in this slice.
    fn find_command(
        &self,
        name: &[u8],
        search: &mut PathSearch,
    ) -> Result<Option<Option<Dispatch>>, Fail> {
        if name.is_empty() {
            return Ok(Some(None));
        }
        let lookup = |path: &[u8]| match self.shell.root.stat_normalized(path) {
            Ok(s) if s.kind == Kind::File => Ok(self.file_dispatch(path, name, s.mode)),
            Err(Code::NoSpace) => Err(Fail::Mem(MemError::Ceiling)),
            _ => Ok(None),
        };
        let mut table = self.shell.paths.borrow_mut();
        if !search.started {
            search.started = true;
            if let Some(path) = table.get(name) {
                if let Some(command) = lookup(path)? {
                    return Ok(Some(Some(command)));
                }
                table.remove(name);
            }
        }
        let path = self
            .env
            .get(b"PATH")
            .map_or(b"/usr/bin:/bin".as_slice(), |e| &e.value);
        loop {
            if let Some((dir, left)) = &mut search.candidate {
                while *left > 0 {
                    if !self.tick() {
                        return Ok(None);
                    }
                    *left = left.saturating_sub(BYTE_CHUNK);
                }
                let dir = self.resolve_path(&self.cwd, &path[dir.clone()])?;
                let resolved = self.resolve_path(&dir, name)?;
                search.candidate = None;
                if let Some(command) = lookup(&resolved)? {
                    table.try_insert(bytes_in(name, &self.shell.meta)?, resolved)?;
                    return Ok(Some(Some(command)));
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
                // Directory scanning already paid for its own bytes. Charge the
                // other input copied to resolve this candidate before doing so.
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
        if matches!(
            self.shell.root.stat_normalized(b"/usr/bin"),
            Err(Code::NotFound)
        ) {
            return Ok(Some(self.active_command(name)));
        }
        Ok(Some(None))
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

#[derive(Clone, Copy)]
enum Dispatch {
    Custom,
    Kernel(&'static CommandSpec),
    Script,
}
