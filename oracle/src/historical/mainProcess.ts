import * as fs from "node:fs";

export type MainReply = { uuid: string; files: (Uint8Array | null)[]; error: string | null };

/** Formats the main process converts before decoding. The oracle has no converter for them. */
const CONVERTED = [".hoot", ".revlog", ".wpilogxz"];

export class UnsupportedInput extends Error {}

/**
 * Emulates the main-process side of `historical-start` and `historical-start-raw`
 * (src/main/electron/main.ts) for files on disk and in-memory child logs. Replies are queued as
 * page timeouts, the way an IPC reply arrives as a later task.
 */
export class MainProcess {
  readonly unhandled: string[] = [];

  constructor(private reply: (message: MainReply) => void) {}

  handle(name: string, data: any): void {
    switch (name) {
      case "historical-start":
        this.start(data.uuid, data.path);
        break;
      case "historical-start-raw":
        this.startRaw(data.uuid, data.data, data.extension);
        break;
      default:
        this.unhandled.push(name);
    }
  }

  private send(message: MainReply): void {
    setTimeout(() => this.reply(message), 0);
  }

  private start(uuid: string, logPath: string): void {
    if (CONVERTED.some((ext) => logPath.endsWith(ext))) {
      throw new UnsupportedInput(`${logPath}: the oracle cannot convert this format`);
    }
    if (logPath.endsWith(".dslog")) {
      const events = logPath.slice(0, logPath.length - 5) + "dsevents";
      this.send({ uuid, files: [readOrNull(logPath), readOrNull(events)], error: null });
    } else {
      this.send({ uuid, files: [readOrNull(logPath)], error: null });
    }
  }

  /**
   * Main writes the bytes to `child_<uuid>.<extension>` and opens that path. Nothing sits beside
   * the temporary file, so a `.dslog` child gets no events file. A conversion the oracle cannot
   * run counts as a failed conversion: no data and an error message, which is what upstream
   * produces for a payload that is not a valid file of that format.
   */
  private startRaw(uuid: string, data: Uint8Array, extension: string): void {
    if (extension.includes("/") || extension.includes("\\") || extension.includes("..")) {
      this.send({ uuid, files: [null], error: "Invalid log extension" });
      return;
    }
    const tempPath = `child_${uuid}.${extension}`;
    if (CONVERTED.some((ext) => tempPath.endsWith(ext))) {
      this.send({ uuid, files: [null], error: "oracle: conversion unavailable" });
    } else if (tempPath.endsWith(".dslog")) {
      this.send({ uuid, files: [fresh(data), null], error: null });
    } else {
      this.send({ uuid, files: [fresh(data)], error: null });
    }
  }
}

/** A copy with its own ArrayBuffer, as an IPC message delivers it (RLOG reads `.buffer`). */
function fresh(bytes: Uint8Array): Uint8Array {
  return new Uint8Array(bytes);
}

function readOrNull(path: string): Uint8Array | null {
  try {
    return fresh(fs.readFileSync(path));
  } catch {
    return null;
  }
}
