import fs from "node:fs";
import path from "node:path";

export interface FixtureFile {
  file: string;
  fixture: any;
}

// "assets", "preferences", "state" and "typeMemory", and the operations of a "log" step, may
// name a JSON file relative to the fixture instead. Under a spec every such file must be listed
// in "inputs", so the vector's generator hash covers it.
export function readFixture(entry: string, argument: string | undefined): FixtureFile {
  const file = argument ?? process.env.ASRS_ORACLE_SPEC;
  if (file === undefined) {
    process.stderr.write(`usage: ${entry} <spec or fixture file>\n`);
    process.exit(2);
  }
  const parsed = JSON.parse(fs.readFileSync(file, "utf8"));
  const isSpec = "fixture" in parsed && "entry" in parsed;
  const fixture = isSpec ? parsed.fixture : parsed;
  const declared: string[] = isSpec ? (parsed.inputs ?? []).map((p: string) => path.resolve(path.dirname(file), p)) : [];
  const load = (holder: any, key: string) => {
    if (typeof holder?.[key] !== "string") return;
    const target = path.resolve(path.dirname(file), holder[key]);
    if (isSpec && !declared.includes(target)) {
      process.stderr.write(`${entry}: ${file}: ${holder[key]} is not listed in "inputs"\n`);
      process.exit(2);
    }
    holder[key] = JSON.parse(fs.readFileSync(target, "utf8"));
  };
  for (const key of ["assets", "preferences", "state", "typeMemory"]) load(fixture, key);
  for (const step of fixture.steps ?? []) load(step, "log");
  return { file, fixture };
}
