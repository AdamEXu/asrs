import fs from "node:fs";
import path from "node:path";
import { upstreamRequire } from "./bundle";

// ssh2's Protocol parser on what a server sends before encryption, cut into chunks in different
// ways: whole, byte by byte, and at random. The inputs are what the raw servers of
// download-live.scenarios.json send, plus key exchange packets. Records are {chunks, events}; the
// chunks are latin1, and parsing stops at the first error, as the client's listener does.
// Events: {k: "header", ident}, {k: "disconnect", reason, description}, {k: "thrown", message},
// {k: "fatal", message, level}.

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
const IDENT = "SSH-2.0-OpenSSH_9.9\r\n";
const u32 = (n: number) => Buffer.from([n >>> 24, n >>> 16, n >>> 8, n]).toString("latin1");
const str = (text: string) => u32(text.length) + text;
function packet(payload: string): string {
  let padding = 8 - ((5 + payload.length) % 8);
  if (padding < 4) padding += 8;
  return u32(1 + payload.length + padding) + String.fromCharCode(padding) + payload + "\0".repeat(padding);
}
const kexinit = (kex: string, hostKey = "ssh-ed25519", follows = "\0") =>
  packet(
    "\x14" + "\0".repeat(16) + [kex, hostKey, "aes128-ctr", "aes128-ctr", "hmac-sha2-256", "hmac-sha2-256", "none", "none", "", ""].map(str).join("") + follows + "\0\0\0\0"
  );
const reply = (key: string) => packet("\x1f" + str(str(key)) + str("d".repeat(32)) + str("s"));

const INPUTS = [
  "SSH-2.0-x\n" + kexinit("curve25519-sha256"),
  "a\r\n\r\nSSH-2.0-OpenSSH_9.9 comment here\r\n" + packet("\x02" + str("x")),
  "SSH-\r\nSSH\nSSH-2.0-dropbear_2020\r\n" + packet("\x05"),
  "SSH-2.0- software\r\n",
  "SSH-3.0-x\r\n",
  "\r\r\n",
  "SSH-2.0-OpenSSH_9.9\r",
  IDENT + u32(12) + "\xff" + "\0".repeat(11),
  IDENT + packet(""),
  IDENT + packet("") + packet("\x1e"),
  IDENT + packet("\x1e"),
  IDENT + kexinit("diffie-hellman-group-exchange-sha256") + packet("\x1e"),
  IDENT + kexinit("diffie-hellman-group-exchange-sha256") + packet("\x1f" + str("p")),
  IDENT + kexinit("diffie-hellman-group-exchange-sha256") + packet("\x21"),
  IDENT + kexinit("curve25519-sha256@libssh.org") + packet("\x20"),
  IDENT + kexinit("ecdh-sha2-nistp256", "rsa-sha2-512") + reply("ssh-ed25519"),
  IDENT + kexinit("ecdh-sha2-nistp256", "rsa-sha2-256") + reply("ssh-rsa") + packet("\x16"),
  IDENT + kexinit("ecdh-sha2-nistp521", "ssh-rsa") + reply("rsa-sha2-256"),
  IDENT + kexinit("curve25519-sha256") + reply("ssh-ed25519") + packet("\x16"),
  IDENT + kexinit("curve25519-sha256") + reply("ssh-ed25519") + kexinit("curve25519-sha256"),
  IDENT + kexinit("curve25519-sha256") + reply("ssh-ed25519") + packet("\x01" + u32(3) + str("late") + str("")) + packet("\x5a"),
  IDENT + kexinit("curve25519-sha256,ecdh-sha2-nistp256", "ssh-ed25519", "\x01") + packet("\x1f" + str("kept")),
  IDENT + kexinit("ecdh-sha2-nistp256,curve25519-sha256", "ssh-ed25519", "\x01") + packet("\x01" + u32(2) + str("skipped") + str("")) + packet("\x0f"),
  IDENT + packet("\x01" + u32(2) + str("caf\xc3\xa9 \xff") + str("en")) + packet("\x04\x00" + str("d") + str("")) + packet("\x06")
];

export async function runHandshake(count: number, seed: number): Promise<void> {
  const require = upstreamRequire();
  const Protocol = require("ssh2/lib/protocol/Protocol.js");
  await require("ssh2/lib/protocol/crypto.js").init;
  let state = seed >>> 0 || 1;
  const below = (n: number) => {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    return (state >>> 0) % n;
  };
  const scenarios = JSON.parse(
    fs.readFileSync(path.join(path.dirname(process.env.ASRS_ORACLE_SPEC!), "download-live.scenarios.json"), "utf8")
  ).scenarios;
  const inputs = [...new Set<string>([...INPUTS, ...scenarios.filter((s: any) => s.server.kind === "raw").map((s: any) => s.server.send)])];

  function run(chunks: string[]): void {
    const events: object[] = [];
    let stopped = false;
    const protocol = new Protocol({
      ident: undefined,
      offer: undefined,
      onWrite: () => undefined,
      onError: (error: any) => {
        events.push({ k: "fatal", message: error.message, level: error.level });
        stopped = true;
      },
      onHeader: (header: any) => events.push({ k: "header", ident: header.identRaw }),
      onHandshakeComplete: () => undefined,
      messageHandlers: {
        DISCONNECT: (_: unknown, reason: number, description: string) => events.push({ k: "disconnect", reason, description })
      }
    });
    protocol.start();
    for (const chunk of chunks) {
      if (stopped) break;
      const data = Buffer.from(chunk, "latin1");
      try {
        protocol.parse(data, 0, data.length);
      } catch (error) {
        events.push({ k: "thrown", message: (error as Error).message });
        stopped = true;
      }
    }
    out({ chunks, events });
  }

  for (const input of inputs) {
    if (input === "") continue;
    run([input]);
    if (input.length < 2000) run([...input]);
    for (let i = 0; i < count; i++) {
      const chunks: string[] = [];
      for (let p = 0; p < input.length; ) {
        const n = below(3) === 0 ? 1 + below(4) : 1 + below(input.length);
        chunks.push(input.slice(p, p + n));
        p += n;
      }
      run(chunks);
    }
  }
}
