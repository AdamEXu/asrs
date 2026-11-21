// The `fs`/`graceful-fs` replacement main-process modules see in the `files` harness: an
// in-memory Map plus a log of write destinations, matching what the asrs-services `Fs` trait
// exposes to the port.

const files = new Map<string, Buffer>();
const failures = new Map<string, "read" | "write">();
export const writes: string[] = [];

export function setFailure(name: string, kind: "read" | "write" | null): void {
  if (kind === null) failures.delete(name);
  else failures.set(name, kind);
}

export function putFile(name: string, data: Uint8Array): void {
  files.set(name, Buffer.from(data));
}

export function removeFile(name: string): void {
  files.delete(name);
}

export function dumpFiles(): Record<string, Uint8Array> {
  const out: Record<string, Uint8Array> = {};
  [...files.keys()].sort().forEach((name) => {
    out[name] = new Uint8Array(files.get(name)!);
  });
  return out;
}

export function takeWrites(): string[] {
  return writes.splice(0, writes.length);
}

function enoent(syscall: string, name: unknown): NodeJS.ErrnoException {
  const error = new Error(`ENOENT: no such file or directory, ${syscall} '${name}'`) as NodeJS.ErrnoException;
  error.code = "ENOENT";
  error.errno = -2;
  error.syscall = syscall;
  error.path = String(name);
  return error;
}

function eacces(syscall: string, name: unknown): NodeJS.ErrnoException {
  const error = new Error(`EACCES: permission denied, ${syscall} '${name}'`) as NodeJS.ErrnoException;
  error.code = "EACCES";
  error.errno = -13;
  error.syscall = syscall;
  error.path = String(name);
  return error;
}

const asBuffer = (data: unknown): Buffer => (Buffer.isBuffer(data) ? data : Buffer.from(data as any));

export function existsSync(name: unknown): boolean {
  return files.has(String(name));
}

export function readFileSync(name: unknown): Buffer {
  if (failures.get(String(name)) === "read") throw eacces("open", name);
  const data = files.get(String(name));
  if (data === undefined) throw enoent("open", name);
  return Buffer.from(data);
}

export function writeFileSync(name: unknown, data: unknown): void {
  if (failures.get(String(name)) === "write") throw eacces("open", name);
  files.set(String(name), Buffer.from(asBuffer(data)));
  writes.push(String(name));
}

export function copyFileSync(from: unknown, to: unknown): void {
  if (failures.get(String(from)) === "read" || failures.get(String(to)) === "write") throw eacces("copyfile", from);
  const data = files.get(String(from));
  if (data === undefined) throw enoent("copyfile", from);
  files.set(String(to), Buffer.from(data));
  writes.push(String(to));
}

export function rmSync(name: unknown): void {
  files.delete(String(name));
}

export function unlinkSync(name: unknown): void {
  if (!files.delete(String(name))) throw enoent("unlink", name);
}

export function mkdirSync(): void {}

export function readdirSync(): string[] {
  return [];
}

const last = (args: unknown[]): ((...a: any[]) => void) | undefined => {
  const end = args[args.length - 1];
  return typeof end === "function" ? (end as (...a: any[]) => void) : undefined;
};

export function readFile(name: unknown, ...rest: unknown[]): void {
  const cb = last(rest);
  if (cb === undefined) return;
  let data: Buffer;
  try {
    data = readFileSync(name);
  } catch (error) {
    cb(error, undefined);
    return;
  }
  cb(null, data);
}

function callback(rest: unknown[], run: () => void): void {
  let failure: unknown = null;
  try {
    run();
  } catch (error) {
    failure = error;
  }
  last(rest)?.(failure);
}

export function writeFile(name: unknown, data: unknown, ...rest: unknown[]): void {
  callback(rest, () => writeFileSync(name, data));
}

export function unlink(name: unknown, ...rest: unknown[]): void {
  callback(rest, () => unlinkSync(name));
}

const api = {
  existsSync,
  readFileSync,
  writeFileSync,
  copyFileSync,
  rmSync,
  unlinkSync,
  mkdirSync,
  readdirSync,
  readFile,
  writeFile,
  unlink
};

export const promises = {
  readFile: async (name: unknown): Promise<Buffer> => readFileSync(name),
  writeFile: async (name: unknown, data: unknown): Promise<void> => writeFileSync(name, data),
  unlink: async (name: unknown): Promise<void> => unlinkSync(name),
  access: async (name: unknown): Promise<void> => {
    readFileSync(name);
  },
  mkdir: async (): Promise<void> => {}
};

export default api;
