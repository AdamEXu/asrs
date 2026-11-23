import { createHash } from "node:crypto";
import fs from "node:fs";
import { upstreamRequire } from "./bundle";

// ssh2's SFTP client session (lib/protocol/SFTP.js) and its fastGet, with scripted server packets
// and a scripted fs. Each case is one session. Records are {case, op, ..., effects, threw?}; `threw`
// is what SFTP.push let escape (an "error" event nobody listens to once the session is ready).
// Ops: init; push (parts: hex, or {fill, len} for len bytes of one value); end; readdir; fastGet;
// fs (the oldest pending fs call completes, with `error` or without).
// Effects: {k: "send", hex}, {k: "close"}, {k: "ready"}, {k: "error", message},
// {k: "readdir", id, error | list}, {k: "step", id, total, nb, size}, {k: "get", id, error},
// {k: "fsOpen", path, flags}, {k: "fsWrite", fd, len, sha256, pos}, {k: "fsClose", fd}.

type Part = string | { fill: number; len: number };
const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

const u32 = (n: number) => (n >>> 0).toString(16).padStart(8, "0");
const u64 = (n: number) => BigInt(n).toString(16).padStart(16, "0");
const str = (text: string | Buffer) => {
  const data = Buffer.from(text as any);
  return u32(data.length) + data.toString("hex");
};
const packet = (body: string) => u32(body.length / 2) + body;

export async function runSftp(count: number, seed: number): Promise<void> {
  const { SFTP } = upstreamRequire()("ssh2/lib/protocol/SFTP.js");
  let state = seed >>> 0 || 1;
  const next = () => {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    return state >>> 0;
  };
  const below = (n: number) => next() % n;
  const pick = <T>(list: readonly T[]): T => list[below(list.length)];

  let effects: any[] = [];
  let fsQueue: { callback: (error: Error | null, value?: number) => void; value?: number }[] = [];
  let nextFd = 10;
  Object.assign(fs, {
    open: (name: string, flags: string, callback: any) => {
      effects.push({ k: "fsOpen", path: name, flags });
      fsQueue.push({ callback, value: nextFd++ });
    },
    write: (fd: number, buffer: Buffer, offset: number, len: number, pos: number, callback: any) => {
      const sha256 = createHash("sha256").update(buffer.subarray(offset, offset + len)).digest("hex").slice(0, 16);
      effects.push({ k: "fsWrite", fd, len, sha256, pos });
      fsQueue.push({ callback, value: len });
    },
    close: (fd: number, callback: any) => {
      effects.push({ k: "fsClose", fd });
      fsQueue.push({ callback });
    }
  });

  let caseIndex = -1;
  let sftp: any;
  let sent: { type: number; id: number; length: number }[] = [];
  function begin(ident: string) {
    caseIndex++;
    effects = [];
    fsQueue = [];
    nextFd = 10;
    sent = [];
    sftp = new SFTP(
      {
        _protocol: {
          _remoteIdentRaw: Buffer.from(ident),
          channelData: (_id: number, data: Buffer) => {
            effects.push({ k: "send", hex: data.toString("hex") });
            const type = data[4];
            const length = type === 5 ? data.readUInt32BE(data.length - 4) : 0;
            if (type !== 1) sent.push({ type, id: data.readUInt32BE(5), length });
          },
          channelClose: () => effects.push({ k: "close" })
        }
      },
      {
        type: "sftp",
        incoming: { id: 0, window: 2 ** 31, packetSize: 2 ** 20, state: "open" },
        outgoing: { id: 0, window: 2 ** 31, packetSize: 2 ** 20, state: "open" }
      },
      {}
    );
    const onError = (error: Error) => {
      stop();
      effects.push({ k: "error", message: error.message });
    };
    const onReady = () => {
      stop();
      effects.push({ k: "ready" });
    };
    const stop = () => sftp.removeListener("error", onError).removeListener("ready", onReady);
    sftp.on("error", onError).on("ready", onReady);
    out({ case: caseIndex, op: "begin", ident });
  }
  function record(op: string, args: object, run: () => void) {
    const r: any = { case: caseIndex, op, ...args };
    try {
      run();
    } catch (error) {
      r.threw = (error as Error).message;
    }
    r.effects = effects.splice(0);
    out(r);
    return r.threw === undefined;
  }
  const bytes = (parts: Part[]) => Buffer.concat(parts.map((p) => (typeof p === "string" ? Buffer.from(p, "hex") : Buffer.alloc(p.len, p.fill))));
  const init = () => record("init", {}, () => sftp._init());
  const push = (...parts: Part[]) => record("push", { parts }, () => sftp.push(bytes(parts)));
  const end = () => record("end", {}, () => sftp.push(null));
  let nextId = 0;
  const readdir = (path: string) => {
    const id = nextId++;
    record("readdir", { id, path }, () =>
      sftp.readdir(path, (error: any, list: any[]) => {
        if (error) effects.push({ k: "readdir", id, error: error.message });
        else
          effects.push({
            k: "readdir",
            id,
            list: list.map((e) => ({ filename: e.filename, mode: e.attrs.mode ?? null, size: e.attrs.size ?? null, dir: e.attrs.isDirectory() }))
          });
      })
    );
  };
  const fastGet = (remote: string, local: string) => {
    const id = nextId++;
    record("fastGet", { id, remote, local }, () =>
      sftp.fastGet(
        remote,
        local,
        { step: (total: number, nb: number, size: number) => effects.push({ k: "step", id, total, nb, size }) },
        (error: any) => effects.push({ k: "get", id, error: error ? error.message : null })
      )
    );
  };
  const fsDone = (error: string | null) =>
    record("fs", { error }, () => {
      const call = fsQueue.shift()!;
      if (error === null) call.callback(null, call.value);
      else call.callback(new Error(error));
    });
  const take = (type?: number) => {
    const i = type === undefined ? 0 : sent.findIndex((s) => s.type === type);
    return sent.splice(i, 1)[0];
  };

  const VERSION = packet("02" + u32(3));
  const LIMITS = packet("02" + u32(3) + str("posix-rename@openssh.com") + str("1") + str("limits@openssh.com") + str("1"));
  const limitsReply = (id: number, read: number, handles = 100) => packet("c9" + u32(id) + u64(262144) + u64(read) + u64(261120) + u64(handles));
  const status = (id: number, code: number, text = "") => packet("65" + u32(id) + u32(code) + str(text) + str(""));
  const handle = (id: number, text = "hnd") => packet("66" + u32(id) + str(text));
  const attrs = (size: number | null, mode: number | null, extra = false) =>
    u32((size === null ? 0 : 1) | (mode === null ? 0 : 4) | (extra ? 0x8000000a : 0)) +
    (size === null ? "" : u64(size)) +
    (extra ? u32(501) + u32(20) : "") +
    (mode === null ? "" : u32(mode)) +
    (extra ? u32(1) + u32(2) + u32(1) + str("x@y") + str("z") : "");
  const fileAttrs = (id: number, size: number | null) => packet("69" + u32(id) + attrs(size, 0o100644));
  const name = (file: string | Buffer, size: number | null, mode: number | null, extra = false) => str(file) + str("-rw-r--r-- long") + attrs(size, mode, extra);
  const names = (id: number, ...entries: string[]) => packet("68" + u32(id) + u32(entries.length) + entries.join(""));
  const data = (id: number, len: number): Part[] => [u32(len + 9) + "67" + u32(id) + u32(len), { fill: id & 0xff, len }];

  function ready(read?: number, ident = "SSH-2.0-OpenSSH_9.9") {
    begin(ident);
    init();
    if (read === undefined) push(VERSION);
    else {
      push(LIMITS);
      push(limitsReply(take().id, read));
    }
  }

  // Session start.
  for (const body of [VERSION, packet("02" + u32(6)), packet("02" + u32(3) + str("a") + str("b")), packet("02" + u32(3) + str("a")), packet("02"), packet("65" + u32(0) + u32(0)), packet("01" + u32(3)), packet("ff"), packet("67"), u32(0), u32(262145) + "02", u32(0xffffffff), u32(0x80000000) + "02"]) {
    begin("SSH-2.0-OpenSSH_9.9");
    init();
    push(body);
    readdir("/after");
    end();
  }
  begin("SSH-2.0-dropbear_2020.81");
  init();
  for (let i = 0; i < VERSION.length; i += 2) push(VERSION.slice(i, i + 2));
  push(VERSION);
  for (const reply of [status(0, 8, "nope"), status(0, 0), limitsReply(0, 0, 0), limitsReply(0, 1000), packet("c9" + u32(0) + u64(1)), packet("c9"), packet("c9" + u32(7))]) {
    begin("SSH-2.0-OpenSSH_9.9");
    init();
    push(LIMITS);
    push(reply);
    readdir("/x");
  }

  // readdir.
  ready();
  readdir("/U/logs/");
  push(handle(take().id, "dir\u0001"));
  push(names(take().id, name(".", 1, 0o040755), name("..", 1, 0o040755), name("a.wpilog", 12, 0o100644, true), name("ü–.rlog", 4294967296.0 * 3 + 7, 0o100600)));
  push(names(take().id, name("sub", 4096, 0o040700), name("none", null, null), name(Buffer.from([0x61, 0xff, 0xc3, 0x28, 0xe2, 0x82]), 1, 0o120777)));
  push(status(take().id, 1, "End of file"));
  push(status(take().id, 0, "Success"));
  for (const [code, text] of [[2, "No such file"], [3, "Permission denied"], [4, "Failure"], [2, ""], [3, ""], [4, ""], [5, ""], [6, ""], [7, ""], [8, ""], [1, ""], [99, ""], [99, "odd"]] as const) {
    ready();
    readdir("/x");
    push(status(take().id, code, text));
  }
  ready();
  readdir("/x");
  push(packet("65" + u32(take().id) + u32(2)));
  readdir("/x");
  push(packet("65" + u32(take().id)));
  readdir("/x");
  push(packet("65"));
  readdir("/x");
  push(handle(take().id));
  push(status(take().id, 3, "Permission denied"));
  push(status(take().id, 0));
  readdir("/x");
  push(handle(take().id));
  push(status(take().id, 1));
  push(status(take().id, 4, "close failed"));
  readdir("/x");
  readdir("/y");
  push(handle(take().id) + handle(take().id));
  push(names(take().id, name("a.wpilog", 1, 0o100644)) + names(take().id));
  end();
  readdir("/z");
  end();
  for (const bad of [packet("68" + u32(1)), packet("68" + u32(1) + u32(1)), packet("68" + u32(1) + u32(2) + name("a", 1, 2)), packet("68"), packet("66" + u32(1)), packet("66"), packet("69" + u32(1)), packet("69"), packet("69" + u32(1) + u32(1) + u32(0)), packet("67" + u32(1)), packet("67" + u32(99)), packet("67" + u32(99) + str("x")), packet("02" + u32(3)), packet("00"), u32(0)]) {
    ready();
    readdir("/x");
    push(handle(take().id));
    push(bad);
    end();
  }

  // fastGet.
  const transfer = (size: number, read?: number, ident?: string) => {
    ready(read, ident);
    fastGet("/r/file.wpilog", "/l/file.wpilog");
    push(handle(take().id, "file"));
    push(fileAttrs(take().id, size));
    fsDone(null);
  };
  const finishTransfer = (order: "fifo" | "lifo" | "random", short = 0) => {
    for (let guard = 0; guard < 400; guard++) {
      if (fsQueue.length > 0 && (sent.length === 0 || order === "fifo" || below(2) === 0)) {
        fsDone(null);
        continue;
      }
      if (sent.length === 0) return;
      const i = order === "fifo" ? 0 : order === "lifo" ? sent.length - 1 : below(sent.length);
      const request = sent.splice(i, 1)[0];
      if (request.type === 5) push(...data(request.id, short > 0 && below(short) === 0 ? Math.max(1, request.length >> 1) : request.length));
      else push(status(request.id, 0));
    }
  };
  for (const size of [1, 10, 32767, 32768, 32769, 65536, 100000]) {
    transfer(size);
    finishTransfer("fifo");
  }
  for (const order of ["lifo", "random"] as const) {
    transfer(230000);
    finishTransfer(order);
  }
  transfer(32768 * 70 + 5);
  finishTransfer("fifo");
  for (const ident of ["SSH-2.0-mod_sftp", "SSH-2.0-dropbear_2020.81", "SSH-1.99-OpenSSH_3.0"]) {
    transfer(70000, undefined, ident);
    finishTransfer("fifo");
  }
  transfer(40000, 1000);
  finishTransfer("fifo");
  transfer(70000, 20000);
  finishTransfer("random", 3);
  transfer(100000);
  finishTransfer("random", 2);
  for (const size of [0, null]) {
    ready();
    fastGet("/r", "/l");
    push(handle(take().id));
    push(fileAttrs(take().id, size));
    fsDone(null);
    while (fsQueue.length > 0) fsDone(null);
    while (sent.length > 0) push(status(take().id, 0));
  }
  // open, fstat, stat, local open, read, write and close failures.
  ready();
  fastGet("/r", "/l");
  push(status(take().id, 3, "Permission denied"));
  ready();
  fastGet("/r", "/l");
  push(handle(take().id));
  push(status(take().id, 8, "unsupported"));
  push(fileAttrs(take().id, 5));
  fsDone(null);
  finishTransfer("fifo");
  ready();
  fastGet("/r", "/l");
  push(handle(take().id));
  push(status(take().id, 8, "unsupported"));
  push(status(take().id, 2, "No such file"));
  push(status(take().id, 0));
  ready();
  fastGet("/r", "/l");
  push(handle(take().id));
  push(fileAttrs(take().id, 5));
  fsDone("ENOENT: no such file or directory, open '/l'");
  push(status(take().id, 0));
  transfer(100000);
  push(...data(take().id, 32768));
  push(status(take().id, 4, "Failure"));
  push(...data(take().id, 32768));
  while (fsQueue.length > 0) fsDone(null);
  while (sent.length > 0) push(status(take().id, 0));
  while (fsQueue.length > 0) fsDone(null);
  transfer(100000);
  push(...data(take().id, 32768));
  fsDone("ENOSPC: no space left on device, write");
  while (sent.length > 0) push(status(take().id, 0));
  while (fsQueue.length > 0) fsDone(null);
  transfer(10);
  push(...data(take().id, 10));
  fsDone(null);
  fsDone("EIO: i/o error, close");
  while (sent.length > 0) push(status(take().id, 0));
  while (fsQueue.length > 0) fsDone(null);
  transfer(10);
  push(...data(take().id, 10));
  fsDone(null);
  fsDone(null);
  push(status(take().id, 4, "Failure"));
  // A remote file that shrinks: EOF answers, then nothing.
  transfer(40000);
  push(status(take(5).id, 1, "End of file"));
  fsDone(null);
  push(status(take(5).id, 1));
  fsDone(null);
  // More data than requested, and data longer than its packet.
  transfer(10);
  const [head, body] = data(take().id, 10);
  push(head, body);
  transfer(10);
  push(...data(take().id, 20));
  fsDone(null);
  transfer(10);
  push(packet("67" + u32(take().id) + u32(50) + "00"));
  // The channel ends at each point.
  for (let stage = 0; stage < 7; stage++) {
    ready();
    fastGet("/r", "/l");
    fastGet("/r2", "/l2");
    if (stage > 0) push(handle(take().id));
    if (stage > 1) push(handle(take().id));
    if (stage > 2) push(fileAttrs(take().id, 80000));
    if (stage > 3) push(fileAttrs(take().id, 10));
    if (stage > 4) fsDone(null);
    if (stage > 5) push(...data(take(5).id, 32768));
    end();
    for (let i = 0; fsQueue.length > 0 && i < 20; i++) fsDone(null);
    end();
  }

  // Random sessions: replies in any order, split anywhere, right and wrong.
  for (let i = 0; i < count; i++) {
    ready(below(3) === 0 ? pick([100, 1000, 5000, 32768]) : undefined);
    for (let k = 1 + below(3); k > 0; k--) pick([() => readdir("/d" + below(9)), () => fastGet("/r" + below(9), "/l" + below(9))])();
    for (let step = 5 + below(40); step > 0; step--) {
      const roll = below(100);
      if (fsQueue.length > 0 && roll < 40) {
        fsDone(below(15) === 0 ? "EIO: i/o error" : null);
        continue;
      }
      if (roll > 97) {
        end();
        break;
      }
      if (roll > 94) {
        pick([() => readdir("/late"), () => fastGet("/late", "/late")])();
        continue;
      }
      if (sent.length === 0) continue;
      const request = sent.splice(below(sent.length), 1)[0];
      let reply = "";
      let parts: Part[] | null = null;
      if (below(12) === 0) reply = status(request.id, pick([1, 2, 3, 4]), pick(["", "Failure", "No such file"]));
      else
        switch (request.type) {
          case 3:
          case 11:
            reply = handle(request.id, "h" + below(100));
            break;
          case 8:
          case 17:
            reply = below(8) === 0 ? status(request.id, 4) : fileAttrs(request.id, pick([0, 1, 500, 32768, 40000, 70000]));
            break;
          case 5:
            parts = data(request.id, below(5) === 0 ? 1 + below(request.length) : request.length);
            break;
          case 12:
            reply =
              below(2) === 0
                ? status(request.id, 1)
                : names(request.id, ...Array.from({ length: below(4) }, (_, n) => name(pick([".", "..", "a", "b.wpilog", "é"]) + n, below(1 << 20), pick([0o100644, 0o040755, null]), below(3) === 0)));
            break;
          default:
            reply = status(request.id, below(10) === 0 ? 4 : 0);
        }
      let alive: boolean;
      if (parts !== null) alive = push(...parts);
      else if (below(3) === 0) {
        const cut = 2 * below(reply.length / 2);
        alive = push(reply.slice(0, cut)) && push(reply.slice(cut));
      } else alive = push(reply);
      if (!alive) break;
    }
  }
}
