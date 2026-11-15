// Byte helpers of EVOL Annex D.5. A byte string is a string in which every character is one byte (0-255).

const MAX_CONVERSION = 512 * 1024 * 1024;
const encoder = new TextEncoder();
const strictDecoder = new TextDecoder("utf-8", { fatal: true });
const CHUNK = 8192;

/** An empty byte string. */
export const EMPTY_BYTES = "";

function checkSize(n) {
  if (n > MAX_CONVERSION) throw new RangeError("conversion above 512 MiB");
}

/** Text to a byte string holding its UTF-8 form. */
export function encodeUtf8ToBytes(text) {
  checkSize(text.length);
  return latin1FromBytes(encoder.encode(text));
}

/** Byte string to text: valid UTF-8 sequences are decoded, other bytes are kept one character each. */
export function decodeBytesToUtf8(bytes) {
  checkSize(bytes.length);
  return decodeUtf8(unsafeBytesFromLatin1(bytes));
}

/** `Uint8Array` to a byte string, without interpretation. */
export function latin1FromBytes(bytes) {
  checkSize(bytes.length);
  if (bytes.length <= CHUNK) return String.fromCharCode.apply(null, bytes);
  const parts = [];
  for (let i = 0; i < bytes.length; i += CHUNK) parts.push(String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK)));
  return parts.join("");
}

/** Byte string to `Uint8Array`, without interpretation (characters above 255 keep their low byte). */
export function unsafeBytesFromLatin1(text) {
  checkSize(text.length);
  const out = new Uint8Array(text.length);
  for (let i = 0; i < text.length; i++) out[i] = text.charCodeAt(i);
  return out;
}

/** A command result holding bytes. */
export function bytesOutput(bytes, stderr = "", exitCode = 0) {
  return { stdout: typeof bytes === "string" ? bytes : latin1FromBytes(bytes), stderr, exitCode, stdoutKind: "bytes" };
}

/** A command result holding text. */
export function textOutput(text, stderr = "", exitCode = 0) {
  return { stdout: text, stderr, exitCode, stdoutKind: "text" };
}

/** `"bytes"` or `"text"` for a command result. */
export function stdoutKind(result) {
  return result.stdoutKind === "bytes" ? "bytes" : "text";
}

/** The stdout of a result as a byte string. */
export function stdoutAsBytes(result) {
  return stdoutKind(result) === "bytes" ? result.stdout : encodeUtf8ToBytes(result.stdout);
}

/** Decodes bytes by the rule of Annex D.2: valid UTF-8 sequences, else one character per byte. */
export function decodeUtf8(bytes) {
  if (bytes.length === 0) return "";
  try {
    return strictDecoder.decode(bytes);
  } catch {
    return decodeLenient(bytes);
  }
}

function decodeLenient(b) {
  const out = [];
  let i = 0;
  let run = "";
  const flush = () => {
    if (run) out.push(run);
    run = "";
  };
  while (i < b.length) {
    const c = b[i];
    let len = 0;
    if (c < 0x80) len = 1;
    else if (c >= 0xc2 && c <= 0xdf) len = 2;
    else if (c >= 0xe0 && c <= 0xef) len = 3;
    else if (c >= 0xf0 && c <= 0xf4) len = 4;
    let ok = len > 0 && i + len <= b.length;
    if (ok) {
      for (let j = 1; j < len; j++) if ((b[i + j] & 0xc0) !== 0x80) ok = false;
    }
    if (ok && len === 3) {
      if (c === 0xe0 && b[i + 1] < 0xa0) ok = false;
      if (c === 0xed && b[i + 1] >= 0xa0) ok = false;
    }
    if (ok && len === 4) {
      if (c === 0xf0 && b[i + 1] < 0x90) ok = false;
      if (c === 0xf4 && b[i + 1] >= 0x90) ok = false;
    }
    if (ok) {
      run += strictDecoder.decode(b.subarray(i, i + len));
      i += len;
    } else {
      run += String.fromCharCode(c);
      i += 1;
    }
    if (run.length > 4096) flush();
  }
  flush();
  return out.join("");
}

/** UTF-8 form of a string. */
export function utf8(text) {
  return encoder.encode(text);
}

/** Length of the UTF-8 form of a string, without encoding it. */
export function utf8Length(text) {
  let n = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    if (c < 0x80) n += 1;
    else if (c < 0x800) n += 2;
    else if (c >= 0xd800 && c <= 0xdbff && i + 1 < text.length && (text.charCodeAt(i + 1) & 0xfc00) === 0xdc00) {
      n += 4;
      i++;
    } else n += 3;
  }
  return n;
}

/** Concatenates chunks. */
export function concat(chunks) {
  let n = 0;
  for (const c of chunks) n += c.length;
  const out = new Uint8Array(n);
  let o = 0;
  for (const c of chunks) {
    out.set(c, o);
    o += c.length;
  }
  return out;
}
