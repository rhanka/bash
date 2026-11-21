use super::{CmdCtx, CmdPoll, CommandKind, CommandSpec, Fail, KBytes};

pub const SPEC: &[super::CommandSpec] = &[CommandSpec {
    name: "echo",
    kind: CommandKind::Standard,
    flags: &["-n", "-e", "-E"],
    start,
}];

fn start(ctx: &mut CmdCtx<'_>, mut args: &[KBytes]) -> Result<CmdPoll, Fail> {
    let mut newline = true;
    let mut escapes = ctx.xpg_echo;
    while let Some((arg, rest)) = args.split_first() {
        match arg.as_slice() {
            b"-n" => newline = false,
            b"-e" => escapes = true,
            b"-E" => escapes = false,
            b"-ne" | b"-en" => {
                newline = false;
                escapes = true;
            }
            _ => break,
        }
        args = rest;
    }
    for (index, arg) in args.iter().enumerate() {
        if index != 0 {
            ctx.stdout(b" ")?;
        }
        if escapes {
            if !escaped(ctx, arg)? {
                return Ok(CmdPoll::Done(0));
            }
        } else {
            ctx.stdout(arg)?;
        }
    }
    if newline {
        ctx.stdout(b"\n")?;
    }
    Ok(CmdPoll::Done(0))
}

fn escaped(ctx: &mut CmdCtx<'_>, mut text: &[u8]) -> Result<bool, Fail> {
    while let Some(slash) = text.iter().position(|&b| b == b'\\') {
        ctx.stdout(&text[..slash])?;
        text = &text[slash..];
        let Some(&code) = text.get(1) else { break };
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
            b'c' => return Ok(false),
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
                    text = &text[consumed..];
                    continue;
                }
                if matches!(code, b'u' | b'U') {
                    let c = char::from_u32(value).unwrap_or(char::REPLACEMENT_CHARACTER);
                    ctx.stdout(c.encode_utf8(&mut [0; 4]).as_bytes())?;
                    text = &text[consumed..];
                    continue;
                }
                value as u8
            }
            _ => {
                ctx.stdout(&text[..2])?;
                text = &text[2..];
                continue;
            }
        };
        ctx.stdout(&[byte])?;
        text = &text[consumed..];
    }
    ctx.stdout(text)?;
    Ok(true)
}
