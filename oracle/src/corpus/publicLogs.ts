import { createHash } from "node:crypto";
import * as fs from "node:fs";
import * as path from "node:path";

// The logs of parity/corpus/public.toml, read from the public-logs cache that
// `cargo xtask corpus fetch` fills. A log with `redistributable = false` may only feed a
// local spec (`"local": true`): xtask sets ASRS_ORACLE_LOCAL=1 for those and writes their
// vectors into $ASRS_CACHE/vectors instead of parity/vectors.

export interface PublicLog {
  name: string;
  file: string;
  sha256: string;
  size: number;
  format: string;
  merge?: string;
  redistributable: boolean;
}

export function publicLogs(root: string): PublicLog[] {
  const text = fs.readFileSync(path.join(root, "parity/corpus/public.toml"), "utf8");
  const logs: PublicLog[] = [];
  let current: { [key: string]: string | number | boolean } | null = null;
  for (const line of text.split("\n")) {
    const trimmed = line.trim();
    if (trimmed === "[[log]]") {
      current = {};
      logs.push(current as unknown as PublicLog);
    } else if (current !== null) {
      const m = /^([a-z0-9_]+) = ("(?:[^"\\]|\\.)*"|-?[0-9]+|true|false)$/.exec(trimmed);
      if (m === null) continue;
      const value = m[2];
      current[m[1]] = value.startsWith('"')
        ? (JSON.parse(value) as string)
        : value === "true" || value === "false"
          ? value === "true"
          : Number(value);
    }
  }
  for (const log of logs) {
    if (typeof log.redistributable !== "boolean") {
      throw new Error(`public.toml: ${log.name} needs redistributable = true or false`);
    }
  }
  return logs;
}

function publicLogsDir(root: string): string {
  let cache = process.env.ASRS_CACHE;
  if (cache === undefined) {
    const target = path.join(root, "target");
    try {
      cache = path.join(path.dirname(fs.realpathSync(target)), "cache");
    } catch {
      cache = path.join(target, "cache");
    }
  }
  return path.join(cache, "public-logs");
}

/** The cached file of `log` after checking its size and sha256, and that this run may use it. */
export function publicLogPath(root: string, log: PublicLog): string {
  if (!log.redistributable && process.env.ASRS_ORACLE_LOCAL !== "1") {
    throw new Error(
      `${log.name} has no redistribution license, so vectors derived from it must come from a local spec ("local": true)`
    );
  }
  const file = path.join(publicLogsDir(root), log.file);
  const data = fs.readFileSync(file);
  const sha256 = createHash("sha256").update(data).digest("hex");
  if (sha256 !== log.sha256 || data.length !== log.size) {
    throw new Error(`${file}: sha256 or size differs from public.toml`);
  }
  return file;
}

export function readPublicLog(root: string, log: PublicLog): Uint8Array {
  return new Uint8Array(fs.readFileSync(publicLogPath(root, log)));
}
