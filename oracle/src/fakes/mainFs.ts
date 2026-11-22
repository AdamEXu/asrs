// The `fs`/`graceful-fs` replacement main-process modules see in the `files` harness: an
// in-memory Map plus a log of write destinations, matching what the asrs-services `Fs` trait
// exposes to the port.

const files = new Map<string, Buffer>();
const failures = new Map<string, "read" | "write" | "readOpened">();
const missingDirs = new Set<string>();
const openFds = new Map<number, string>();
let nextFd = 100;
export const writes: string[] = [];

export function setFailure(name: string, kind: "read" | "write" | "readOpened" | null): void {
  if (kind === null) failures.delete(name);
  else failures.set(name, kind);
}

// Writes to a file directly inside `dir` fail with ENOENT, as if the directory did not exist.
export function setMissingDir(dir: string, missing: boolean): void {
  if (missing) missingDirs.add(dir);
  else missingDirs.delete(dir);
}

export function reset(): void {
  files.clear();
  failures.clear();
  missingDirs.clear();
  openFds.clear();
  writes.splice(0, writes.length);
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

function eisdir(syscall: string): NodeJS.ErrnoException {
  const error = new Error(`EISDIR: illegal operation on a directory, ${syscall}`) as NodeJS.ErrnoException;
  error.code = "EISDIR";
  error.errno = -21;
  error.syscall = syscall;
  return error;
}

// Node validates paths before any callback API runs and throws synchronously on a NUL byte.
function checkPath(name: unknown): void {
  if (String(name).includes("\0")) {
    const error = new TypeError(
      `The argument 'path' must be a string, Uint8Array, or URL without null bytes. Received ${JSON.stringify(String(name))}`
    ) as NodeJS.ErrnoException;
    error.code = "ERR_INVALID_ARG_VALUE";
    throw error;
  }
}

const parentDir = (name: string): string => name.slice(0, Math.max(name.lastIndexOf("/"), 0));

export function writeFileSync(name: unknown, data: unknown): void {
  if (failures.get(String(name)) === "write") throw eacces("open", name);
  if (missingDirs.has(parentDir(String(name)))) throw enoent("open", name);
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

export function open(name: unknown, ...rest: unknown[]): void {
  checkPath(name);
  const cb = last(rest);
  if (cb === undefined) return;
  let fd = -1;
  let failure: unknown = null;
  if (failures.get(String(name)) === "read") failure = eacces("open", name);
  else if (!files.has(String(name))) failure = enoent("open", name);
  else {
    fd = nextFd++;
    openFds.set(fd, String(name));
  }
  setImmediate(() => (failure === null ? cb(null, fd) : cb(failure)));
}

function readOpened(fd: number, cb: (...a: any[]) => void): void {
  const name = openFds.get(fd);
  setImmediate(() => {
    if (name === undefined || failures.get(name) === "readOpened" || !files.has(name)) cb(eisdir("read"), undefined);
    else cb(null, Buffer.from(files.get(name)!));
  });
}

export function readFile(name: unknown, ...rest: unknown[]): void {
  const cb = last(rest);
  if (cb === undefined) return;
  if (typeof name === "number") {
    readOpened(name, cb);
    return;
  }
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
  checkPath(name);
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
  unlink,
  open
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
