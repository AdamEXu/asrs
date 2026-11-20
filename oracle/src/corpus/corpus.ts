import { Rng } from "./rng";
import { LogSpec } from "./wpilog";

export interface CaseFile {
  path: string;
  bytes: Uint8Array;
}

export interface Case {
  id: string;
  about: string;
  files: CaseFile[];
}

const ID_PATTERN = /^[a-z0-9]+(?:[-.][a-z0-9]+)*(?:\/[a-z0-9]+(?:[-.][a-z0-9]+)*)*$/;

export class Corpus {
  readonly cases: Case[] = [];
  private ids = new Set<string>();

  constructor(readonly seed: number) {}

  rng(id: string): Rng {
    return new Rng(this.seed, id);
  }

  add(id: string, about: string, files: CaseFile[]): void {
    if (!ID_PATTERN.test(id)) throw new Error(`bad case id ${id}`);
    if (this.ids.has(id)) throw new Error(`duplicate case id ${id}`);
    this.ids.add(id);
    this.cases.push({ id, about, files });
  }

  wpilog(id: string, about: string, bytes: Uint8Array): void {
    this.add(id, about, [{ path: `${id}.wpilog`, bytes }]);
  }

  /** A payload case: the WPILOG file and the equivalent Log write sequence. */
  spec(id: string, about: string, spec: LogSpec): void {
    this.add(id, about, [
      { path: `${id}.wpilog`, bytes: spec.bytes() },
      { path: `${id}.ops.jsonl`, bytes: new TextEncoder().encode(spec.ops.map((l) => l + "\n").join("")) }
    ]);
  }
}
