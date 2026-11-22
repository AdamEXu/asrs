import "./shims";
import * as mathjs from "mathjs";
import { create, all, evaluate, parse, format, typeOf } from "mathjs";
import fs from "node:fs";
import path from "node:path";
import { toTree } from "./cve1";
import { Rng } from "./corpus/rng";

// Ground truth for asrs_model::mathjs, the mathjs 11.3.0 `evaluate` that upstream's
// src/unitConversion.ts runs on the "Extra Factor" field (`typeof result === "number"` is accepted,
// anything else or a throw is "Failed to parse extra factor."). Every layer is recorded apart so a
// wrong helper cannot hide behind an end-to-end match:
//
//   genMathjs parse <seed> <random> <soup>      parse(): AST dump or the thrown error, over the
//                                                hand-written cases plus grammar-random expressions
//                                                over the whole grammar plus random token soup
//   genMathjs evaluate <seed> <random>          evaluate(): result or the thrown error, over the
//                                                hand-written cases plus random expressions drawn
//                                                from the evaluation subset (SUBSET below)
//   genMathjs call                              each subset function called directly with
//                                                primitive arguments, every arity
//   genMathjs format <seed> <random>            the `format(value, {truncate: 78})` that
//                                                FunctionNode puts in "is not a function" errors
//   genMathjs signatures                        typed-function's signature list of each subset
//                                                function, the data its argument errors come from
//
// Hand-written cases come from the spec's sibling `<name>.cases.json`. The evaluation subset is
// what asrs ports: numbers, booleans, strings, null, undefined, functions and ResultSets. A case
// is outside it when it builds a matrix, range, object, accessor, index or unit, calls or reads a
// mathjs name outside SUBSET_NAMES, or when any proper subexpression without variables evaluates
// to anything else (a Complex is allowed only as the whole result). Hand-written evaluate cases
// outside the subset fail the run; random ones are redrawn.

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
const fail = (message: string): never => {
  process.stderr.write(`genMathjs: ${message}\n`);
  process.exit(1);
};

const math: any = create(all);
if (math.evaluate("1") !== evaluate("1")) fail("instance check");

export const OPERATOR_FUNCTIONS = [
  "add", "subtract", "multiply", "divide", "pow", "mod", "unaryMinus", "unaryPlus", "factorial",
  "ctranspose", "bitAnd", "bitOr", "bitXor", "bitNot", "leftShift", "rightArithShift",
  "rightLogShift", "and", "or", "xor", "not", "equal", "unequal", "smaller", "larger",
  "smallerEq", "largerEq", "dotMultiply", "dotDivide", "dotPow"
];
export const NAMED_FUNCTIONS = [
  "abs", "sqrt", "cbrt", "exp", "log", "log10", "log2", "round", "floor", "ceil", "fix", "sign",
  "min", "max", "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "number", "square", "cube",
  "gamma"
];
export const CONSTANTS = ["pi", "PI", "e", "E", "tau", "phi", "LN2", "LN10", "LOG2E", "LOG10E", "SQRT1_2", "SQRT2", "i", "version"];
const FUNCTIONS = [...OPERATOR_FUNCTIONS, ...NAMED_FUNCTIONS];
const HELPER_FUNCTIONS = ["addScalar", "multiplyScalar", "equalScalar", "conj"];
const SUBSET_NAMES = new Set([...FUNCTIONS, ...CONSTANTS]);
const OBJECT_PROTOTYPE_NAMES = new Set(Object.getOwnPropertyNames(Object.prototype));

const errorOf = (e: any) =>
  e instanceof Error ? { name: e.name, message: e.message } : fail(`non-Error throw ${String(e)}`);

function result(value: any): any {
  if (typeof value === "number") return { type: "number", value: toTree(value) };
  if (typeof value === "boolean") return { type: "boolean", value };
  if (typeof value === "string") return { type: "string", value };
  if (value === null) return { type: "null" };
  if (value === undefined) return { type: "undefined" };
  if (typeof value === "function") return { type: "function", name: value.name };
  if (math.isResultSet(value)) return { type: "ResultSet", entries: value.entries.map(result) };
  return { type: "other", typeOf: typeOf(value) };
}

function outcome(run: () => any): any {
  let value;
  try {
    value = run();
  } catch (e) {
    return { throw: errorOf(e) };
  }
  return { ok: result(value) };
}

function dump(node: any): any {
  const d: any = { type: node.type };
  if (Object.prototype.hasOwnProperty.call(node, "comment")) d.comment = node.comment;
  switch (node.type) {
    case "ConstantNode":
      d.value = toTree(node.value);
      break;
    case "SymbolNode":
      d.name = node.name;
      break;
    case "OperatorNode":
      d.op = node.op;
      d.fn = node.fn;
      d.implicit = node.implicit;
      d.isPercentage = node.isPercentage;
      d.args = node.args.map(dump);
      break;
    case "ParenthesisNode":
      d.content = dump(node.content);
      break;
    case "FunctionNode":
      d.fn = dump(node.fn);
      d.args = node.args.map(dump);
      break;
    case "AccessorNode":
      d.object = dump(node.object);
      d.index = dump(node.index);
      break;
    case "IndexNode":
      d.dimensions = node.dimensions.map(dump);
      d.dotNotation = node.dotNotation;
      break;
    case "ArrayNode":
      d.items = node.items.map(dump);
      break;
    case "ObjectNode":
      d.properties = Object.keys(node.properties).map((key) => [key, dump(node.properties[key])]);
      break;
    case "AssignmentNode":
      d.object = dump(node.object);
      d.index = node.index === null ? null : dump(node.index);
      d.value = dump(node.value);
      break;
    case "FunctionAssignmentNode":
      d.name = node.name;
      d.params = node.params;
      d.types = node.types;
      d.expr = dump(node.expr);
      break;
    case "ConditionalNode":
      d.condition = dump(node.condition);
      d.trueExpr = dump(node.trueExpr);
      d.falseExpr = dump(node.falseExpr);
      break;
    case "RangeNode":
      d.start = dump(node.start);
      d.end = dump(node.end);
      d.step = node.step ? dump(node.step) : null;
      break;
    case "RelationalNode":
      d.conditionals = node.conditionals;
      d.params = node.params.map(dump);
      break;
    case "BlockNode":
      d.blocks = node.blocks.map((b: any) => ({ node: dump(b.node), visible: b.visible }));
      break;
    default:
      fail(`unknown node type ${node.type}`);
  }
  return d;
}

function children(node: any): any[] {
  const list: any[] = [];
  node.forEach((child: any) => list.push(child));
  return list;
}

function walk(node: any, visit: (n: any, root: boolean) => void, root = true): void {
  visit(node, root);
  for (const child of children(node)) walk(child, visit, false);
}

function isSubsetValue(value: any): boolean {
  return (
    typeof value === "number" ||
    typeof value === "boolean" ||
    typeof value === "string" ||
    value === null ||
    value === undefined ||
    typeof value === "function"
  );
}

/** Why `expr` is outside the evaluation subset, or null when it is inside. */
function outsideSubset(expr: string): string | null {
  let tree: any;
  try {
    tree = parse(expr);
  } catch {
    return null;
  }
  const variables = new Set<string>();
  walk(tree, (n) => {
    if (n.type === "AssignmentNode" && n.object.type === "SymbolNode") variables.add(n.object.name);
    if (n.type === "FunctionAssignmentNode") {
      variables.add(n.name);
      n.params.forEach((p: string) => variables.add(p));
    }
  });
  let reason: string | null = null;
  walk(tree, (n) => {
    if (reason !== null) return;
    switch (n.type) {
      case "ArrayNode":
      case "RangeNode":
      case "ObjectNode":
      case "AccessorNode":
      case "IndexNode":
        reason = n.type;
        return;
      case "AssignmentNode":
        if (n.index !== null) reason = "indexed assignment";
        return;
      case "OperatorNode":
        if (!FUNCTIONS.includes(n.fn)) reason = `operator ${n.fn}`;
        return;
      case "SymbolNode": {
        const name = n.name;
        if (variables.has(name) || SUBSET_NAMES.has(name) || OBJECT_PROTOTYPE_NAMES.has(name)) return;
        if (name in math) reason = `mathjs name ${name}`;
        else if (math.Unit.isValuelessUnit(name)) reason = `unit ${name}`;
        return;
      }
      case "FunctionNode":
        if (n.fn.type !== "SymbolNode") reason = "FunctionNode callee";
        else if (OBJECT_PROTOTYPE_NAMES.has(n.fn.name) && !variables.has(n.fn.name)) reason = `call of ${n.fn.name}`;
        else if (n.fn.name === "cbrt" && n.args.length === 2 && !variables.has("cbrt")) reason = "cbrt(x, allRoots)";
        return;
    }
  });
  if (reason !== null) return reason;
  const free = (n: any) => {
    let found = false;
    walk(n, (m) => {
      if (m.type === "SymbolNode" && variables.has(m.name)) found = true;
    });
    return found;
  };
  walk(tree, (n, root) => {
    if (reason !== null || root || free(n)) return;
    let value;
    try {
      value = n.compile().evaluate();
    } catch {
      return;
    }
    if (!isSubsetValue(value)) reason = `subexpression ${n.toString()} is ${typeOf(value)}`;
  });
  if (reason !== null) return reason;
  let whole;
  try {
    whole = evaluate(expr);
  } catch {
    return null;
  }
  if (isSubsetValue(whole) || math.isComplex(whole)) return null;
  if (math.isResultSet(whole)) {
    const bad = whole.entries.find((v: any) => !isSubsetValue(v));
    return bad === undefined ? null : `result set entry ${typeOf(bad)}`;
  }
  return `result ${typeOf(whole)}`;
}

function cases(name: string): string[] {
  const spec = process.env.ASRS_ORACLE_SPEC;
  if (spec === undefined) fail("ASRS_ORACLE_SPEC is not set");
  const file = path.join(path.dirname(spec!), name + ".cases.json");
  const list = JSON.parse(fs.readFileSync(file, "utf8"));
  if (!Array.isArray(list) || list.some((c) => typeof c !== "string")) fail(`${file}: expected an array of strings`);
  return list;
}

const ws = (rng: Rng, newlines: boolean) => {
  const r = rng.below(20);
  if (r < 11) return "";
  if (r < 17) return " ";
  if (r === 17) return "  ";
  if (r === 18) return "\t";
  return newlines ? "\n" : " ";
};

const DECIMALS = ["0", "1", "2", "3", "7", "10", "12", "100", "0.5", "2.54", "0.0254", ".5", "5.", "1e3", "1e-3", "1.5e2", "2E+2", "3e-0", "0.1", "0.2", "1e21", "1e-7", "123456789", "9007199254740993", "4.35", "1.005", "2.5", "0.30000000000000004", "6.02e23", "1e308", "1e400", "5e-324", "0.000001"];
const RADIX = ["0x10", "0xff", "0xFF", "0b101", "0o17", "0x1.8", "0b1.1", "0o7.4", "0xffi8", "0x7fi8", "0x80i8", "0xffffi16", "0x1ffi8", "0b11i2", "0xffffffffi32", "0x", "0b", "0x.8", "0xg"];
const IDENTS = ["x", "y", "foo", "bar", "a1", "_q", "$k", "α", "Ω", "é", "ℵ", "𝐱", "zz9", "qq", "w_2", "end", "toString", "constructor", "__proto__", "hasOwnProperty", "valueOf"];
const NAMED_DELIMS = ["mod", "to", "in", "and", "xor", "or", "not"];

function numberLiteral(rng: Rng): string {
  const r = rng.below(10);
  if (r < 6) return rng.pick(DECIMALS);
  if (r < 8) return rng.pick(RADIX);
  const digits = String(rng.below(100000));
  const frac = rng.bool() ? "." + String(rng.below(1000)) : "";
  const exp = rng.below(4) === 0 ? "e" + rng.pick(["", "-", "+"]) + String(rng.below(30)) : "";
  return digits + frac + exp;
}

function stringLiteral(rng: Rng, full: boolean): string {
  const bodies = full
    ? ["", "abc", "2", " 3 ", "0x10", "a\\\"b", "\\n", "\\u00e9", "\\t", "it\\'s", "\\q", "\\u12", "é", "line\\\\", "1e3", "NaN", "Infinity", "-5"]
    : ["2", " 3 ", "0x10", "1e3", "-5", "abc", "", "NaN", "Infinity", ".5", "0b11", "4.5e-1"];
  const quote = rng.bool() ? '"' : "'";
  return quote + rng.pick(bodies) + quote;
}

function randomFull(rng: Rng, depth: number): string {
  const nl = rng.below(3) === 0;
  const w = () => ws(rng, nl);
  if (depth <= 0 || rng.below(5) === 0) {
    const r = rng.below(9);
    if (r < 3) return numberLiteral(rng);
    if (r < 5) return rng.pick(IDENTS);
    if (r === 5) return stringLiteral(rng, true);
    if (r === 6) return rng.pick(["true", "false", "null", "undefined", "NaN", "Infinity", "pi", "e", "i"]);
    if (r === 7) return rng.pick(NAMED_DELIMS);
    return rng.pick(["end", ":", "x", "2"]);
  }
  const sub = () => randomFull(rng, depth - 1);
  switch (rng.below(24)) {
    case 0:
      return rng.pick(["-", "+", "~", "not ", "--", "- "]) + w() + sub();
    case 1:
    case 2:
    case 3:
      return sub() + w() + rng.pick(["+", "-", "*", "/", "^", ".*", "./", ".^", "%", " mod ", "&", "|", "^|", "<<", ">>", ">>>", " and ", " or ", " xor ", "==", "!=", "<", ">", "<=", ">=", " to ", " in "]) + w() + sub();
    case 4:
      return sub() + w() + rng.pick(["!", "'", "%", "!!", "'!"]);
    case 5:
      return "(" + w() + sub() + w() + ")";
    case 6: {
      const n = rng.below(4);
      const args = Array.from({ length: n }, sub).join("," + w());
      return rng.pick(["f", "sin", "max", "foo", "x", "a.b"]) + "(" + w() + args + w() + ")";
    }
    case 7: {
      const rows = 1 + rng.below(3);
      const cols = rng.below(3);
      const row = () => Array.from({ length: cols }, sub).join("," + w());
      return "[" + w() + Array.from({ length: rows }, row).join(";" + w()) + w() + "]";
    }
    case 8: {
      const n = rng.below(3);
      const key = () => rng.pick(["a", "b", "'c d'", "\"e\"", "1", "2", "mod", "a", "toString"]);
      return "{" + w() + Array.from({ length: n }, () => key() + w() + ":" + w() + sub()).join("," + w()) + w() + "}";
    }
    case 9:
      return sub() + "." + rng.pick(["a", "b", "x2", "toString", "2", "length"]);
    case 10:
      return sub() + "[" + w() + Array.from({ length: 1 + rng.below(2) }, () => rng.pick([sub(), "end", ":", "1:end", "2:3"])).join(",") + w() + "]";
    case 11:
      return sub() + w() + "?" + w() + sub() + w() + ":" + w() + sub();
    case 12:
      return sub() + ":" + sub() + (rng.bool() ? ":" + sub() : "");
    case 13:
      return rng.pick(IDENTS) + w() + "=" + w() + sub();
    case 14:
      return rng.pick(["f", "g", "end", "h"]) + "(" + rng.pick(["x", "x, y", "", "2", "x,"]) + ")" + w() + "=" + w() + sub();
    case 15:
      return sub() + rng.pick([";", "\n", ";\n", "; ", "\n\n", ";;"]) + sub();
    case 16:
      return sub() + w() + rng.pick(["# note", "#", "#c\n2"]);
    case 17:
    case 18:
      return sub() + rng.pick([" ", "", "  "]) + sub();
    case 19:
      return numberLiteral(rng) + rng.pick(IDENTS);
    case 20:
      return sub() + "/" + numberLiteral(rng) + rng.pick([" ", ""]) + rng.pick(["x", "(2)", "pi", "3", ""]);
    case 21:
      return sub() + "=" + sub();
    case 22:
      return rng.pick(["a.b", "a[1]", "a[1].c", "a.b[2]"]) + w() + "=" + w() + sub();
    default:
      return stringLiteral(rng, true) + rng.pick(["", "[1]", ".a", "(2)"]);
  }
}

const SOUP = ["1", "2.5", ".", "e", "E", "x", "+", "-", "*", "/", "^", "(", ")", "[", "]", "{", "}", ",", ";", ":", "?", "!", "'", "\"", "%", "=", "==", "<", ">", "&", "|", "~", "#", " ", "\n", "\t", "@", "`", "\\", "$", "é", "α", "𝐱", "😀", "\ud835", "\udc00", " ", "0x", "i8", "mod", "to", "in", "and", "not", "end", "..", "...", ".*", "./", ".^", "^|", "<<", ">>>", "\r"];

function randomSoup(rng: Rng): string {
  const n = 1 + rng.below(8);
  let s = "";
  for (let i = 0; i < n; i++) s += rng.pick(SOUP);
  return s;
}

const SUBSET_NUMBERS = ["0", "1", "2", "3", "4", "5", "10", "12", "0.5", "2.54", "0.0254", ".5", "1e3", "1e-3", "0.1", "0.2", "3.5", "-0", "180", "360", "1e21", "1e-7", "0x10", "0b101", "0o17", "0xffi8", "2.5", "1.005", "4.35", "1e308", "0.3", "7", "100", "1000", "25.4", "12.7", "6.02e23", "9007199254740993"];
const EVAL_CONSTANTS = ["pi", "e", "tau", "phi", "PI", "E", "LN2", "LN10", "LOG2E", "LOG10E", "SQRT1_2", "SQRT2", "Infinity", "NaN", "true", "false", "null", "undefined", "version", "i"];
const ARITY: Record<string, number[]> = {
  abs: [1], sqrt: [1], cbrt: [1], exp: [1], log: [1, 2], log10: [1], log2: [1], round: [1, 2], floor: [1, 2],
  ceil: [1, 2], fix: [1, 2], sign: [1], min: [1, 2, 3], max: [1, 2, 3], sin: [1], cos: [1], tan: [1], asin: [1],
  acos: [1], atan: [1], atan2: [2], number: [0, 1], square: [1], cube: [1], gamma: [1], add: [2, 3], subtract: [2],
  multiply: [2, 3], divide: [2], pow: [2], mod: [2], unaryMinus: [1], unaryPlus: [1], factorial: [1], ctranspose: [1],
  bitAnd: [2], bitOr: [2], bitXor: [2], bitNot: [1], leftShift: [2], rightArithShift: [2], rightLogShift: [2],
  and: [2], or: [2], xor: [2], not: [1], equal: [2], unequal: [2], smaller: [2], larger: [2], smallerEq: [2],
  largerEq: [2], dotMultiply: [2], dotDivide: [2], dotPow: [2]
};

function freshName(rng: Rng): string {
  for (;;) {
    const name = rng.pick(["foo", "bar", "qq", "zz", "xyz", "abc", "k9", "u_v", "α", "θ", "wq", "kk"]);
    if (!(name in math) && !math.Unit.isValuelessUnit(name)) return name;
  }
}

function randomSubset(rng: Rng, depth: number): string {
  const w = () => ws(rng, false);
  if (depth <= 0 || rng.below(4) === 0) {
    const r = rng.below(12);
    if (r < 7) return rng.pick(SUBSET_NUMBERS);
    if (r < 10) return rng.pick(EVAL_CONSTANTS);
    if (r === 10) return stringLiteral(rng, false);
    return rng.below(4) === 0 ? freshName(rng) : rng.pick(SUBSET_NUMBERS);
  }
  const sub = () => randomSubset(rng, depth - 1);
  switch (rng.below(20)) {
    case 0:
    case 1:
      return rng.pick(["-", "+", "-", "~", "not ", "- "]) + w() + sub();
    case 2:
    case 3:
    case 4:
    case 5:
      return sub() + w() + rng.pick(["+", "-", "*", "/", "^", "+", "-", "*", "/", ".*", "./", ".^", "%", " mod "]) + w() + sub();
    case 6:
      return sub() + w() + rng.pick(["&", "|", "^|", "<<", ">>", ">>>", " and ", " or ", " xor "]) + w() + sub();
    case 7:
      return sub() + w() + rng.pick(["==", "!=", "<", ">", "<=", ">="]) + w() + sub() + (rng.below(4) === 0 ? w() + rng.pick(["<", "<=", "=="]) + w() + sub() : "");
    case 8:
      return sub() + rng.pick(["!", "'", "%", "!"]);
    case 9:
    case 10:
      return "(" + w() + sub() + w() + ")";
    case 11:
    case 12:
    case 13: {
      const name = rng.pick(Object.keys(ARITY));
      let n = rng.pick(ARITY[name]);
      if (rng.below(12) === 0) n = rng.below(4);
      return name + "(" + w() + Array.from({ length: n }, sub).join("," + w()) + w() + ")";
    }
    case 14:
      return sub() + w() + "?" + w() + sub() + w() + ":" + w() + sub();
    case 15:
      return rng.pick(SUBSET_NUMBERS) + rng.pick([" ", ""]) + rng.pick(["pi", "e", "(" + sub() + ")", "tau", "sqrt(" + sub() + ")"]);
    case 16:
      return "(" + sub() + ")" + rng.pick(["", " "]) + "(" + sub() + ")";
    case 17:
      return sub() + " " + rng.pick(["pi", "e", "phi"]);
    case 18:
      return sub() + "/" + rng.pick(SUBSET_NUMBERS) + rng.pick(["pi", " e", "(2)", ""]);
    default:
      return sub() + w() + rng.pick(["%", "mod", "^"]) + w() + "(" + sub() + ")";
  }
}

function randomEvaluate(rng: Rng): string {
  let expr = randomSubset(rng, 1 + rng.below(4));
  while (!/[^0-9.]/.test(expr)) expr = randomSubset(rng, 1 + rng.below(4));
  return rng.below(15) === 0 ? expr + " # c" : expr;
}

const [mode, ...rest] = process.argv.slice(2);

if (mode === "parse") {
  const [seedArg, randomArg, soupArg] = rest;
  const rng = new Rng(Number(seedArg), "mathjs-parse");
  const exprs = [...cases("parse")];
  for (let i = 0; i < Number(randomArg); i++) exprs.push(randomFull(rng, 1 + rng.below(4)));
  for (let i = 0; i < Number(soupArg); i++) exprs.push(randomSoup(rng));
  for (const expr of exprs) {
    let record: any;
    try {
      record = { ast: dump(parse(expr)) };
    } catch (e) {
      record = { throw: errorOf(e) };
    }
    out({ kind: "parse", expr: toTree(expr), ...record });
  }
} else if (mode === "evaluate") {
  const [seedArg, randomArg] = rest;
  const rng = new Rng(Number(seedArg), "mathjs-evaluate");
  const exprs = cases("evaluate");
  const outside = exprs.flatMap((expr) => {
    const why = outsideSubset(expr);
    return why === null ? [] : [`${JSON.stringify(expr)}: ${why}`];
  });
  if (outside.length > 0) fail(`evaluate cases outside the subset:\n  ${outside.join("\n  ")}`);
  let drawn = 0;
  while (drawn < Number(randomArg)) {
    const expr = randomEvaluate(rng);
    if (outsideSubset(expr) !== null) continue;
    exprs.push(expr);
    drawn++;
  }
  for (const expr of exprs) out({ kind: "evaluate", expr: toTree(expr), ...outcome(() => evaluate(expr)) });
} else if (mode === "call") {
  const values: any[] = [0, -0, 1, -1, 2, 3, -3, 0.5, -0.5, 1.5, 2.5, -2.5, 7, 10, 0.1, 1e-13, 4.35, 1.005, 31, 32, 2 ** 31, 2 ** 32, 2 ** 53, 1e21, 1e308, 5e-324, NaN, Infinity, -Infinity, true, false, "2", " 2 ", "0x10", "1e3", "", "abc", "NaN", "-Infinity", null, undefined];
  const pairValues: any[] = [0, -0, 1, -1, 2, 3, -3, 0.5, -2.5, 7, 1e-13, 1e-12, 4.35, 2 ** 31, 2 ** 53, 1e308, NaN, Infinity, -Infinity, true, "2", "abc", null, undefined];
  const emit = (fn: string, args: any[]) =>
    out({ kind: "call", fn, args: args.map((a) => toTree(a)), ...outcome(() => (mathjs as any)[fn](...args)) });
  const rng = new Rng(1, "mathjs-call");
  for (const fn of FUNCTIONS) {
    const arities = new Set<number>([0, ...ARITY[fn], Math.max(...ARITY[fn]) + 1]);
    for (const n of arities) {
      if (fn === "cbrt" && n === 2) continue;
      if (n === 0) emit(fn, []);
      else if (n === 1) for (const v of values) emit(fn, [v]);
      else if (n === 2) for (const a of pairValues) for (const b of pairValues) emit(fn, [a, b]);
      else {
        for (let k = 0; k < 60; k++) emit(fn, Array.from({ length: n }, () => rng.pick(pairValues)));
      }
    }
  }
  for (const fn of ["round", "floor", "ceil", "fix"]) {
    for (const x of [0.5, 1.5, 2.5, -2.5, 1.005, 4.35, 1.45, 2.675, 1234.5678, -1234.5678, 1e-7, 123456789.123, 0.1 + 0.2, 1e21, 5e-324])
      for (const n of [0, 1, 2, 3, 5, 10, 15, -1, 1.5, 16, NaN]) emit(fn, [x, n]);
  }
  for (const fn of ["min", "max"]) {
    for (let k = 0; k < 80; k++) emit(fn, Array.from({ length: 1 + rng.below(5) }, () => rng.pick(pairValues)));
  }
  for (const fn of ["gamma", "factorial"]) {
    for (let k = 0; k < 200; k++) emit(fn, [rng.below(4000) / 16 - 30]);
    for (const x of [170, 171, 171.5, 172, 141, 142, 143, -0.5, -1.5, 1e-300, 0.0001]) emit(fn, [x]);
  }
  for (const fn of ["sin", "cos", "tan", "asin", "acos", "atan", "exp", "log", "log10", "log2", "sqrt", "cbrt", "square", "cube"]) {
    for (let k = 0; k < 60; k++) emit(fn, [(rng.u32() / 2 ** 32 - 0.5) * 10 ** rng.range(-3, 6)]);
  }
  for (const fn of ["equal", "smaller", "larger", "smallerEq", "largerEq", "unequal"]) {
    for (const [a, b] of [[1, 1 + 1e-13], [1, 1 + 1e-11], [0, 1e-13], [0, 1e-300], [1e20, 1e20 + 1e8], [-0, 0], [0.1 + 0.2, 0.3], [1e-12, 2e-12], [5e-324, 0]]) {
      emit(fn, [a, b]);
      emit(fn, [b, a]);
    }
  }
} else if (mode === "format") {
  const [seedArg, randomArg] = rest;
  const rng = new Rng(Number(seedArg), "mathjs-format");
  const values: any[] = [0, -0, 1, -1, 0.1, 0.001, 0.0001, 1e-5, 12345, 99999, 100000, 123456, 1e5, 1e21, 1e-7, 1 / 3, 2 / 3, 0.1 + 0.2, Math.PI, Math.E, 1e308, 5e-324, NaN, Infinity, -Infinity, 2 ** 53, 9007199254740993, 123.456, -0.00012, 1e15, 1e16, 1e17, 0.30000000000000004, 1.5e-3, 1234567.89, true, false, null, undefined, "", "abc", "a\"b", "line\nbreak", "tab\t", "back\\slash", "é", "😀", "\ud800", "x".repeat(77), "y".repeat(78), "z".repeat(79), "w".repeat(120), "\u0001", "\u007f", " "];
  for (let i = 0; i < Number(randomArg); i++) {
    const r = rng.below(4);
    if (r === 0) values.push(rng.u32() * 10 ** rng.range(-12, 12));
    else if (r === 1) values.push((rng.u32() / 2 ** 32) * 10 ** rng.range(-20, 25) * (rng.bool() ? -1 : 1));
    else if (r === 2) values.push(rng.range(-100000, 100000) / 10 ** rng.range(0, 6));
    else {
      const buf = new Float64Array(1);
      new DataView(buf.buffer).setBigUint64(0, rng.u64(), true);
      values.push(buf[0]);
    }
  }
  for (const value of values) out({ kind: "format", value: toTree(value), text: format(value, { truncate: 78 }) });
} else if (mode === "signatures") {
  // typed-function keeps a conversion in its target type's conversionsTo list, not on the
  // conversion, and a signature it expands for a conversion keeps only the source type.
  const TYPE_NAMES = ["number", "Complex", "BigNumber", "Fraction", "Unit", "identifier", "string", "Chain", "Array", "Matrix", "DenseMatrix", "SparseMatrix", "Range", "Index", "boolean", "ResultSet", "Help", "function", "Date", "RegExp", "null", "undefined", "Object"];
  const targets = new Map<number, string>();
  for (const name of TYPE_NAMES) for (const c of math.typed._findType(name).conversionsTo) targets.set(c.index, name);
  const conversionTarget = (_param: any, type: any): string => {
    const target = targets.get(type.conversion.index);
    if (target === undefined) fail(`no target for the conversion from ${type.name}`);
    return target!;
  };
  for (const fn of [...FUNCTIONS, ...HELPER_FUNCTIONS]) {
    const data = math[fn]._typedFunctionData;
    if (data === undefined) fail(`${fn} is not a typed function`);
    const signatures = data.signatures.map((s: any) => ({
      params: s.params.map((p: any) => ({ types: p.types.map((t: any) => t.name), conversions: p.types.map((t: any) => (t.conversion ? [t.conversion.from, conversionTarget(p, t)] : null)), rest: p.restParam }))
    }));
    out({ kind: "signatures", fn, name: math[fn].name, signatures });
  }
} else {
  fail("usage: genMathjs parse|evaluate|call|format|signatures ...");
}
