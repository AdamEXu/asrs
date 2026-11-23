import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

// analyze.mjs <request.json>: maps the raw V8 coverage preload.cjs wrote back to upstream's
// TypeScript and prints the Markdown report. Exits 1 when a run fails the evidence rule.
//
// request: {
//   upstream, dist, bundles   the upstream copy, the harness with maps, upstream's own bundles
//                             with maps
//   runs: [{ id, dir, baseline }]   baseline: the id of the empty run its lines are subtracted from
//   baselines: [{ id, dir }]
//   files: [string]           upstream files or directories under src/; empty reports every file
//   require: [string]         <file>:<function> or <file>:<line>[-<line>] each run must execute
// }

const request = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const real = (p) => (fs.existsSync(p) ? fs.realpathSync(p) : path.resolve(p));
const srcDir = path.join(real(request.upstream), "src") + path.sep;
const distDir = real(request.dist) + path.sep;
const bundleDir = path.join(real(request.upstream), "bundles") + path.sep;

const BASE64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const DIGITS = new Map([...BASE64].map((c, i) => [c, i]));

/** Segments `{ offset, source, line, column }` in generated order, offsets into `text`. */
function segments(text, map, dir) {
  const sources = map.sources.map((s) => {
    const file = real(path.resolve(dir, map.sourceRoot ?? "", s));
    return file.startsWith(srcDir) ? file.slice(srcDir.length).split(path.sep).join("/") : null;
  });
  const starts = [0];
  for (let i = text.indexOf("\n"); i >= 0; i = text.indexOf("\n", i + 1)) starts.push(i + 1);
  const out = [];
  let source = 0;
  let line = 0;
  let column = 0;
  map.mappings.split(";").forEach((group, row) => {
    let generated = 0;
    if (group === "") return;
    for (const segment of group.split(",")) {
      const fields = [];
      let value = 0;
      let shift = 0;
      for (const c of segment) {
        const digit = DIGITS.get(c);
        value |= (digit & 31) << shift;
        shift += 5;
        if ((digit & 32) === 0) {
          fields.push(value & 1 ? -(value >>> 1) : value >>> 1);
          value = 0;
          shift = 0;
        }
      }
      generated += fields[0];
      const offset = starts[row] + generated;
      if (fields.length < 4) {
        out.push({ offset, source: null });
        continue;
      }
      source += fields[1];
      line += fields[2];
      column += fields[3];
      out.push({ offset, source: sources[source], line: line + 1, column });
    }
  });
  return out.sort((a, b) => a.offset - b.offset);
}

const ts = createRequire(path.join(real(request.upstream), "package.json"))("typescript");

/** The index of the first of `segs` at or after `offset`. */
function firstAt(segs, offset) {
  let low = 0;
  let high = segs.length;
  while (low < high) {
    const mid = (low + high) >> 1;
    if (segs[mid].offset < offset) low = mid + 1;
    else high = mid;
  }
  return low;
}

/** Every statement list of a script, outer ones first, as ranges of segment indices. */
function statementLists(text, segs) {
  const root = ts.createSourceFile("script.js", text, ts.ScriptTarget.Latest, false, ts.ScriptKind.JS);
  const lists = [];
  const visit = (node) => {
    if (ts.isBlock(node) || ts.isCaseOrDefaultClause(node)) {
      const list = [];
      for (const s of node.statements) {
        // A function declaration is hoisted, so a jump before it does not make it unreachable.
        if (ts.isFunctionDeclaration(s)) continue;
        // esbuild maps a statement from its indentation, so it starts at the one before's end.
        const [from, start, to] = [s.pos, s.getStart(root), s.end].map((offset) => firstAt(segs, offset));
        if (start < to) list.push({ from, start, to });
      }
      if (list.length > 1) lists.push(list);
    }
    ts.forEachChild(node, visit);
  };
  visit(root);
  return lists;
}

const loaded = new Map();
function loadScript(file, mapFile, dir) {
  if (!loaded.has(file)) {
    const text = fs.readFileSync(file, "utf8");
    const segs = segments(text, JSON.parse(fs.readFileSync(mapFile, "utf8")), dir);
    loaded.set(file, { length: text.length, segments: segs, lists: statementLists(text, segs) });
  }
  return loaded.get(file);
}

const strip = (text) => text.replace(/\/\/# sourceMappingURL=.*\s*$/, "").trimEnd();
const verified = new Set();
function upstreamBundle(file) {
  const twin = path.join(request.bundles, path.relative(bundleDir, file));
  // rolldown's runtime helpers are a chunk of their own, with no source and so no map.
  if (!fs.existsSync(twin + ".map")) return null;
  if (!verified.has(file)) {
    // The build date is the only difference between two builds, and its length is fixed.
    const [a, b] = [file, twin].map((f) => strip(fs.readFileSync(f, "utf8")).length);
    if (a !== b) throw new Error(`${twin} is not the build the oracle ran (${b} characters, not ${a})`);
    verified.add(file);
  }
  return loadScript(twin, twin + ".map", path.dirname(twin));
}

/** The script a coverage URL names, or null for one that holds no upstream code. */
function scriptOf(url, runtime) {
  const named = fileURLToPath(url.replace(/[?#].*$/, ""));
  const file = runtime.has(named) ? named : real(named);
  if (runtime.has(file)) {
    const copy = runtime.get(file);
    return loadScript(copy + ".js", copy + ".map", path.dirname(copy));
  }
  if (file.startsWith(distDir) && fs.existsSync(file + ".map")) return loadScript(file, file + ".map", path.dirname(file));
  if (file.startsWith(bundleDir)) return upstreamBundle(file);
  return null;
}

const sourceText = new Map();
function sourceLine(file, line) {
  if (!sourceText.has(file)) sourceText.set(file, fs.readFileSync(srcDir + file, "utf8").split("\n"));
  return sourceText.get(file)[line - 1] ?? "";
}

/**
 * Whether a V8 function is module evaluation rather than a function upstream wrote: the script
 * itself, an IIFE around a whole bundle, esbuild's lazy module wrapper (named after its file) or
 * an enum's initializer.
 */
function moduleLevel(fn, size, at) {
  const [range] = fn.ranges;
  if (fn.functionName.includes("/") || fn.functionName === "<static_initializer>") return true;
  if (fn.functionName !== "") return false;
  if (range.endOffset - range.startOffset > size * 0.9) return true;
  return at !== null && /^\s*(export\s+)?(const\s+|declare\s+)?enum\b/.test(sourceLine(at.source, at.line));
}

/** Upstream file -> { lines: Map<line, {hit, fn}>, fns: Map<key, {name, line, count}> } */
function readRun(dir) {
  const files = new Map();
  const runtime = new Map();
  const copies = path.join(dir, "bundles");
  if (fs.existsSync(copies)) {
    for (const name of fs.readdirSync(copies).filter((n) => n.endsWith(".url"))) {
      const base = path.join(copies, name.slice(0, -4));
      for (const name of JSON.parse(fs.readFileSync(base + ".url", "utf8"))) runtime.set(name, base);
    }
  }
  const raw = fs.readdirSync(dir).filter((n) => n.endsWith(".jsonl"));
  const states = new Map();
  if (raw.length === 0) throw new Error(`${dir}: the run wrote no coverage`);
  for (const name of raw) {
    for (const line of fs.readFileSync(path.join(dir, name), "utf8").split("\n")) {
      if (line === "") continue;
      for (const script of JSON.parse(line)) {
        const source = scriptOf(script.url, runtime);
        if (source === null) continue;
        if (!states.has(source)) {
          states.set(source, { hits: new Uint8Array(source.segments.length), keys: [] });
        }
        addScript(files, script, source, states.get(source));
      }
    }
  }
  for (const [source, state] of states) addLines(files, source, state);
  return files;
}

const MISSED = 1;
const HIT = 2;

function addScript(files, script, source, state) {
  const size = Math.max(...script.functions.map((f) => f.ranges[0].endOffset));
  // A page bundle is the tail of the script the window runs, after the fixture.
  const shift = Math.max(0, size - source.length);
  const segs = source.segments;
  const ranges = [];
  for (const fn of script.functions) {
    const [range] = fn.ranges;
    const first = segs[firstAt(segs, range.startOffset - shift)];
    const at = first?.source && first.offset < range.endOffset - shift ? first : null;
    const entry = { module: moduleLevel(fn, size, at), key: undefined };
    if (!entry.module && at !== null) {
      const file = fileOf(files, at.source);
      const key = `${at.line}:${at.column}`;
      const known = file.fns.get(key);
      const name = fn.functionName || "(anonymous)";
      if (known === undefined) file.fns.set(key, { name, line: at.line, count: range.count });
      else known.count += range.count;
      entry.key = key;
    }
    for (const r of fn.ranges) ranges.push({ start: r.startOffset - shift, end: r.endOffset - shift, count: r.count, entry });
  }
  ranges.sort((a, b) => a.start - b.start || b.end - a.end);
  const stack = [];
  let next = 0;
  segs.forEach((seg, i) => {
    while (next < ranges.length && ranges[next].start <= seg.offset) {
      while (stack.length > 0 && stack[stack.length - 1].end <= ranges[next].start) stack.pop();
      stack.push(ranges[next++]);
    }
    while (stack.length > 0 && stack[stack.length - 1].end <= seg.offset) stack.pop();
    const top = stack[stack.length - 1];
    if (top === undefined || !seg.source) return;
    const seen = top.count > 0 ? HIT : MISSED;
    if (seen < state.hits[i]) return;
    state.hits[i] = seen;
    state.keys[i] = top.entry.module ? null : top.entry.key;
  });
}

/**
 * V8 ends the zero-count range after a jump (`return`, `break`, `throw`) where the next block of
 * the same statement list begins, and counts the code between later blocks as often as the
 * function itself. Nothing after an unexecuted statement of a list runs, so the first one found
 * makes the rest of its list unexecuted too.
 */
function addLines(files, source, { hits, keys }) {
  for (const list of source.lists) {
    let dead = false;
    for (const { from, start, to } of list) {
      dead ||= hits[start] === MISSED;
      if (!dead) continue;
      for (let i = from; i < to; i++) if (hits[i] === HIT) hits[i] = MISSED;
    }
  }
  source.segments.forEach((seg, i) => {
    if (hits[i] === 0 || keys[i] === null) return;
    const token = sourceLine(seg.source, seg.line).slice(seg.column);
    if (/^([\s})\];,]|else\b|$)/.test(token)) return;
    const { lines } = fileOf(files, seg.source);
    const known = lines.get(seg.line) ?? { hit: false, fn: keys[i] };
    if (hits[i] === HIT) {
      known.hit = true;
      known.fn = keys[i];
    }
    lines.set(seg.line, known);
  });
}

function fileOf(files, name) {
  if (!files.has(name)) files.set(name, { lines: new Map(), fns: new Map() });
  return files.get(name);
}

const span = (a, b) => (a === b ? `${a}` : `${a}-${b}`);
/** `1-4, 9`: consecutive entries of `known` (every executable line) join across blank lines. */
function spans(lines, known) {
  const order = [...known].sort((a, b) => a - b);
  const wanted = new Set(lines);
  const out = [];
  let start = null;
  let last = null;
  for (const line of order) {
    if (wanted.has(line)) {
      start ??= line;
      last = line;
    } else if (start !== null) {
      out.push(span(start, last));
      start = null;
    }
  }
  if (start !== null) out.push(span(start, last));
  return out.join(", ");
}

const runs = request.runs.map((run) => ({ ...run, files: readRun(run.dir) }));
const baselines = new Map(request.baselines.map((b) => [b.id, readRun(b.dir)]));
const selected = (file) => request.files.length === 0 || request.files.some((f) => file === f || file.startsWith(f.replace(/\/?$/, "/")));
const problems = [];
for (const f of request.files) {
  if (!fs.existsSync(srcDir + f)) problems.push(`\`${f}\` is not an upstream file or directory under src/`);
}

/** Lines of `file` a run executes inside functions that its empty baseline run does not. */
function beyond(run, file) {
  const own = run.files.get(file)?.lines ?? new Map();
  const base = baselines.get(run.baseline)?.get(file)?.lines ?? new Map();
  return [...own].filter(([line, l]) => l.hit && !base.get(line)?.hit).map(([line]) => line);
}

const union = new Map();
for (const run of runs) {
  for (const [name, file] of run.files) {
    if (!selected(name)) continue;
    const into = fileOf(union, name);
    for (const [line, l] of file.lines) {
      const known = into.lines.get(line) ?? { runs: [], fn: l.fn };
      if (l.hit) {
        known.runs.push(run.id);
        known.fn = l.fn;
      }
      into.lines.set(line, known);
    }
    for (const [key, f] of file.fns) {
      const known = into.fns.get(key) ?? { name: f.name, line: f.line, runs: [] };
      if (f.count > 0) known.runs.push(run.id);
      into.fns.set(key, known);
    }
  }
}
const names = [...union.keys()].sort();
const out = [];
const print = (text = "") => out.push(text);
const short = (() => {
  const ids = runs.map((r) => r.id.split("/"));
  let common = 0;
  while (ids.length > 1 && ids.every((p) => p.length > common + 1 && p[common] === ids[0][common])) common++;
  return (id) => id.split("/").slice(common).join("/");
})();
const listRuns = (ids) => ids.map(short).join(", ");

print(`# Oracle coverage: ${runs.length} runs`);
print();
print("Lines and functions of upstream's TypeScript, without module evaluation. Run lines leave out what the same harness executes on an empty input.");
print();

if (request.files.length === 0) {
  print("| File | Lines | Functions | Runs reaching it |");
  print("|---|---|---|---|");
  for (const name of names) {
    const reached = runs.filter((r) => beyond(r, name).length > 0);
    if (reached.length === 0) continue;
    const file = union.get(name);
    const fns = [...file.fns.values()];
    print(`| \`${name}\` | ${[...file.lines.values()].filter((l) => l.runs.length > 0).length} of ${file.lines.size} | ${fns.filter((f) => f.runs.length > 0).length} of ${fns.length} | ${reached.length} |`);
  }
  print();
} else {
  for (const f of request.files) {
    if (!names.some((n) => n === f || n.startsWith(f.replace(/\/?$/, "/")))) problems.push(`no run loads a function of \`${f}\``);
  }
  for (const name of names) fileReport(name);
}

/** Functions of `name` no run has compiled, as bundling dropped them, outermost ones only. */
function unbundled(name, known) {
  const text = fs.readFileSync(srcDir + name, "utf8");
  const root = ts.createSourceFile(name, text, ts.ScriptTarget.Latest, true);
  const lineOf = (offset) => root.getLineAndCharacterOfPosition(offset).line + 1;
  const out = [];
  const visit = (node) => {
    if (ts.isFunctionLike(node) && node.body !== undefined) {
      const [from, to] = [lineOf(node.getStart(root)), lineOf(node.body.getStart(root))];
      const seen = known.filter((f) => f.line >= from && f.line <= to);
      if (seen.length === 0) {
        out.push({ name: node.name?.getText(root) ?? (ts.isConstructorDeclaration(node) ? "constructor" : "(anonymous)"), line: from });
        return;
      }
      if (seen.every((f) => f.runs.length === 0)) return;
    }
    ts.forEachChild(node, visit);
  };
  visit(root);
  return out;
}

function fileReport(name) {
  const file = union.get(name);
  const fns = [...file.fns.entries()].sort((a, b) => a[1].line - b[1].line);
  const absent = unbundled(name, [...file.fns.values()]);
  const hit = [...file.lines].filter(([, l]) => l.runs.length > 0);
  print(`## \`${name}\`: ${hit.length} of ${file.lines.size} lines, ${fns.filter(([, f]) => f.runs.length > 0).length} of ${fns.length + absent.length} functions`);
  print();
  print("| Function | Line | Runs | Lines | Uncovered lines |");
  print("|---|---|---|---|---|");
  for (const [key, f] of fns) {
    const own = [...file.lines].filter(([, l]) => l.fn === key);
    const missed = own.filter(([, l]) => l.runs.length === 0).map(([line]) => line);
    const covered = own.length - missed.length;
    print(`| \`${f.name}\` | ${f.line} | ${f.runs.length} of ${runs.length} | ${covered} of ${own.length} | ${f.runs.length === 0 ? "**never called**" : spans(missed, file.lines.keys())} |`);
  }
  for (const f of absent) print(`| \`${f.name}\` | ${f.line} | 0 of ${runs.length} | | **not in what any run bundles** |`);
  print();
  if (runs.length < 2) return;
  const groups = new Map();
  for (const [line, l] of [...file.lines].sort((a, b) => a[0] - b[0])) {
    if (l.runs.length === 0 || l.runs.length === runs.length) continue;
    const key = l.runs.join("\n");
    if (!groups.has(key)) groups.set(key, { runs: l.runs, lines: [] });
    groups.get(key).lines.push(line);
  }
  const thin = [...groups.values()].sort((a, b) => a.runs.length - b.runs.length || b.lines.length - a.lines.length);
  if (thin.length === 0) return;
  print("Lines only some runs reach:");
  print();
  for (const g of thin) {
    const others = runs.map((r) => r.id).filter((id) => !g.runs.includes(id));
    const who =
      g.runs.length <= others.length
        ? `only ${listRuns(g.runs)}`
        : `all but ${listRuns(others)}`;
    print(`- ${g.runs.length} of ${runs.length} (${who}): ${spans(g.lines, file.lines.keys())}`);
  }
  print();
}

print("## Runs");
print();
print("| Run | Lines beyond the empty run | Lines only this run reaches |");
print("|---|---|---|");
const signatures = new Map();
for (const run of runs) {
  let lines = 0;
  let unique = 0;
  const parts = [];
  for (const name of names) {
    const own = beyond(run, name);
    lines += own.length;
    unique += own.filter((line) => union.get(name).lines.get(line).runs.length === 1).length;
    parts.push(`${name}:${[...(run.files.get(name)?.lines ?? [])].filter(([, l]) => l.hit).map(([line]) => line).sort((a, b) => a - b)}`);
  }
  print(`| ${run.id} | ${lines} | ${runs.length > 1 ? unique : ""} |`);
  if (lines === 0) problems.push(`${run.id} reaches none of ${request.files.length > 0 ? "the listed upstream files" : "upstream's functions"}`);
  const signature = parts.join("\n");
  signatures.set(signature, [...(signatures.get(signature) ?? []), run.id]);
}
print();
const same = [...signatures.values()].filter((ids) => ids.length > 1);
if (same.length > 0) {
  print("Runs that execute exactly the same lines (a fixture that adds values but no path; say why it stays):");
  print();
  for (const ids of same) print(`- ${listRuns(ids)}`);
  print();
}

for (const target of request.require) {
  const at = target.lastIndexOf(":");
  const name = target.slice(0, at);
  const what = target.slice(at + 1);
  const file = union.get(name) ?? (selected(name) ? undefined : null);
  if (at < 0 || file === null) {
    problems.push(`--require ${target}: not <file>:<function> or <file>:<lines> in a listed file`);
    continue;
  }
  const range = /^(\d+)(?:-(\d+))?$/.exec(what);
  let covers;
  if (range) {
    const [from, to] = [Number(range[1]), Number(range[2] ?? range[1])];
    const lines = [...(file?.lines.keys() ?? [])].filter((l) => l >= from && l <= to);
    if (lines.length === 0) {
      problems.push(`--require ${target}: no executable line of a function there`);
      continue;
    }
    covers = (run) => lines.some((l) => run.files.get(name)?.lines.get(l)?.hit);
  } else {
    if (![...(file?.fns.values() ?? [])].some((f) => f.name === what)) {
      problems.push(`--require ${target}: no function ${what} in ${name}`);
      continue;
    }
    covers = (run) => [...(run.files.get(name)?.fns.values() ?? [])].some((f) => f.name === what && f.count > 0);
  }
  const missing = runs.filter((run) => !covers(run));
  if (missing.length > 0) {
    problems.push(`${missing.length} of ${runs.length} runs never reach \`${target}\`: ${listRuns(missing.map((r) => r.id))}`);
  }
}

if (problems.length > 0) {
  print("## Problems");
  print();
  for (const p of problems) print(`- ${p}`);
  print();
}
process.stdout.write(out.join("\n"), () => process.exit(problems.length > 0 ? 1 : 0));
