import "./environment";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { createRequire } from "node:module";

// The robot of a scenario with a "robot" key, for both drivers: ssh2's own Server on 127.0.0.1 with
// a host key made for this run, serving the scenario's files over SFTP.
//
//   robotServer <scenario.json>
//     {account: "systemcore" | "lvuser", files: {<path>: <size>}, stall?: {path, from}}
//
// Only `account` logs in, with the password main.ts gives it. A file's bytes are a fixed pattern.
// A READ of `stall.path` at the offset `from` or later is never answered. Prints {"port": <n>} once it
// listens and exits when its stdin ends.

declare const __UPSTREAM_DIR__: string;

type Robot = { account: "systemcore" | "lvuser"; files: Record<string, number>; stall?: { path: string; from: number } };

const PASSWORDS = { systemcore: "systemcore", lvuser: "" };
const { Server, utils } = createRequire(path.join(fs.realpathSync(__UPSTREAM_DIR__), "package.json"))("ssh2");
const { STATUS_CODE } = utils.sftp;

const robot: Robot = JSON.parse(fs.readFileSync(process.argv[2], "utf8")).robot;
const attrs = (file: string) => {
  const size = robot.files[file];
  if (size !== undefined) return { mode: 0o100644, size, uid: 0, gid: 0, atime: 0, mtime: 0 };
  return { mode: 0o040755, size: 4096, uid: 0, gid: 0, atime: 0, mtime: 0 };
};

function entries(dir: string): string[] | undefined {
  const prefix = dir.replace(/\/*$/, "/");
  const names = Object.keys(robot.files)
    .filter((file) => file.startsWith(prefix))
    .map((file) => file.slice(prefix.length).split("/")[0]);
  return names.length === 0 ? undefined : [...new Set(names)];
}

function bytes(offset: number, length: number): Buffer {
  const data = Buffer.alloc(length);
  for (let i = 0; i < length; i++) data[i] = ((offset + i) * 31 + ((offset + i) >>> 8) * 7) & 0xff;
  return data;
}

function serve(sftp: any) {
  const handles = new Map<number, { file: string; listed: boolean }>();
  const open = (id: number, file: string) => {
    const handle = Buffer.alloc(4);
    handle.writeUInt32BE(handles.size);
    handles.set(handles.size, { file, listed: false });
    sftp.handle(id, handle);
  };
  const at = (handle: Buffer) => handles.get(handle.readUInt32BE())!;
  sftp.on("OPENDIR", (id: number, dir: string) => {
    if (entries(dir) === undefined) sftp.status(id, STATUS_CODE.NO_SUCH_FILE, "No such file");
    else open(id, dir);
  });
  sftp.on("READDIR", (id: number, handle: Buffer) => {
    const dir = at(handle);
    if (dir.listed) return sftp.status(id, STATUS_CODE.EOF);
    dir.listed = true;
    const prefix = dir.file.replace(/\/*$/, "/");
    sftp.name(id, entries(dir.file)!.map((filename) => ({ filename, longname: filename, attrs: attrs(prefix + filename) })));
  });
  sftp.on("OPEN", (id: number, file: string) => {
    if (robot.files[file] === undefined) sftp.status(id, STATUS_CODE.NO_SUCH_FILE, "No such file");
    else open(id, file);
  });
  sftp.on("FSTAT", (id: number, handle: Buffer) => sftp.attrs(id, attrs(at(handle).file)));
  sftp.on("STAT", (id: number, file: string) => sftp.attrs(id, attrs(file)));
  sftp.on("READ", (id: number, handle: Buffer, offset: number, length: number) => {
    const open = at(handle);
    const left = robot.files[open.file] - offset;
    if (robot.stall?.path === open.file && offset >= robot.stall.from) return;
    if (left <= 0) sftp.status(id, STATUS_CODE.EOF);
    else sftp.data(id, bytes(offset, Math.min(left, length)));
  });
  sftp.on("CLOSE", (id: number) => sftp.status(id, STATUS_CODE.OK));
}

// ssh2 sometimes returns a private key its own parseKey rejects; retry until one parses.
let hostKey: string;
do {
  hostKey = utils.generateKeyPairSync("ed25519").private;
} while (utils.parseKey(hostKey) instanceof Error);
const server = new Server({ hostKeys: [hostKey] }, (client: any) => {
  client.on("error", () => undefined);
  client.on("authentication", (ctx: any) => {
    const accepted = ctx.method === "password" && ctx.username === robot.account && ctx.password === PASSWORDS[robot.account];
    if (accepted) ctx.accept();
    else ctx.reject(["password"]);
  });
  client.on("session", (accept: any) => accept().on("sftp", (accept: any) => serve(accept())));
});
server.listen(0, "127.0.0.1", () => process.stdout.write(JSON.stringify({ port: (server.address() as net.AddressInfo).port }) + "\n"));
process.stdin.resume();
process.stdin.on("end", () => process.exit(0));
