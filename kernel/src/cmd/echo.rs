use super::{
    CmdCtx, CmdFrame, CmdPoll, CommandKind, CommandSpec, Fail, KBox, KBytes, KVec, Outcome,
};
use crate::fiber::BYTE_CHUNK;

pub const SPEC: &[super::CommandSpec] = &[CommandSpec {
    name: "echo",
    kind: CommandKind::Standard,
    flags: &["-n", "-e", "-E"],
    start,
}];

fn start(ctx: &mut CmdCtx<'_>, args: KVec<KBytes>) -> Result<CmdPoll, Fail> {
    let mut echo = Echo {
        args,
        index: 0,
        offset: 0,
        end: 0,
        first: 0,
        started_arg: false,
        flags: true,
        newline: true,
        escapes: ctx.xpg_echo,
    };
    let poll = echo.resume(ctx, None)?;
    if matches!(poll, CmdPoll::Yield) {
        let frame = KBox::try_new_in(echo, &ctx.fiber.live).map_err(|(_, e)| e)?;
        ctx.suspend(crate::kbox_unsize!(frame, dyn CmdFrame));
    }
    Ok(poll)
}

struct Echo {
    args: KVec<KBytes>,
    index: usize,
    offset: usize,
    end: usize,
    first: usize,
    started_arg: bool,
    flags: bool,
    newline: bool,
    escapes: bool,
}

impl CmdFrame for Echo {
    fn resume(
        &mut self,
        ctx: &mut CmdCtx<'_>,
        _outcome: Option<Outcome<'_>>,
    ) -> Result<CmdPoll, Fail> {
        while let Some(arg) = self.args.get(self.index) {
            if !ctx.fiber.tick() {
                return Ok(CmdPoll::Yield);
            }
            if self.flags {
                match arg.as_slice() {
                    b"-n" => self.newline = false,
                    b"-e" => self.escapes = true,
                    b"-E" => self.escapes = false,
                    b"-ne" | b"-en" => {
                        self.newline = false;
                        self.escapes = true;
                    }
                    _ => self.flags = false,
                }
                if self.flags {
                    self.index += 1;
                    continue;
                }
                self.first = self.index;
            }

            if !self.started_arg {
                if self.index != self.first {
                    ctx.stdout(b" ")?;
                }
                self.started_arg = true;
                if !self.escapes {
                    self.end = arg.len();
                    if !ctx.fiber.out_len_ok(self.end as u64) {
                        return Err(Fail::Output);
                    }
                }
            }

            if self.offset == self.end && self.escapes && arg.get(self.offset) == Some(&b'\\') {
                let Some(n) = escape(ctx, &arg[self.offset..])? else {
                    return Ok(CmdPoll::Done(0));
                };
                self.offset += n;
                self.end = self.offset;
            } else if self.offset == arg.len() {
                self.index += 1;
                self.offset = 0;
                self.end = 0;
                self.started_arg = false;
            } else if self.escapes && self.end < arg.len() && arg[self.end] != b'\\' {
                // Scan ahead for the end of this literal span before writing it.
                let chunk = &arg[self.end..(self.end + BYTE_CHUNK).min(arg.len())];
                self.end += chunk
                    .iter()
                    .position(|&b| b == b'\\')
                    .unwrap_or(chunk.len());
                if (self.end == arg.len() || arg[self.end] == b'\\')
                    && !ctx.fiber.out_len_ok((self.end - self.offset) as u64)
                {
                    return Err(Fail::Output);
                }
            } else {
                let end = (self.offset + BYTE_CHUNK).min(self.end);
                ctx.stdout(&arg[self.offset..end])?;
                self.offset = end;
            }
        }
        if self.newline {
            ctx.stdout(b"\n")?;
        }
        Ok(CmdPoll::Done(0))
    }
}

fn escape(ctx: &mut CmdCtx<'_>, text: &[u8]) -> Result<Option<usize>, Fail> {
    let Some(&code) = text.get(1) else {
        ctx.stdout(text)?;
        return Ok(Some(1));
    };
    let mut consumed = 2;
    let byte = match code {
        b'\\' => b'\\',
        b'n' => b'\n',
        b't' => b'\t',
        b'r' => b'\r',
        b'a' => 7,
        b'b' => 8,
        b'f' => 12,
        b'v' => 11,
        b'e' | b'E' => 27,
        b'c' => return Ok(None),
        b'0' | b'x' | b'u' | b'U' => {
            let (radix, max) = match code {
                b'0' => (8, 3),
                b'x' => (16, 2),
                b'u' => (16, 4),
                _ => (16, 8),
            };
            let mut value = 0;
            for &digit in text[2..].iter().take(max) {
                let Some(digit) = (digit as char).to_digit(radix) else {
                    break;
                };
                value = value * radix + digit;
                consumed += 1;
            }
            if (consumed == 2 && code != b'0') || value > 0x10ffff {
                ctx.stdout(&text[..consumed])?;
                return Ok(Some(consumed));
            }
            if matches!(code, b'u' | b'U') {
                let c = char::from_u32(value).unwrap_or(char::REPLACEMENT_CHARACTER);
                ctx.stdout(c.encode_utf8(&mut [0; 4]).as_bytes())?;
                return Ok(Some(consumed));
            }
            value as u8
        }
        _ => {
            ctx.stdout(&text[..2])?;
            return Ok(Some(2));
        }
    };
    ctx.stdout(&[byte])?;
    Ok(Some(consumed))
}
