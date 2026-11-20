import "./shims";
import * as path from "node:path";
import { childMatrix } from "./corpus/child";
import { Corpus } from "./corpus/corpus";
import { csvMatrix } from "./corpus/csv";
import { dslogMatrix } from "./corpus/dslog";
import { emitCorpus, option, seedOption } from "./corpus/emit";
import { mutate } from "./corpus/fuzz";
import { mergeMatrix } from "./corpus/merge";
import { rlogMatrix } from "./corpus/rlog";
import { roadRunnerMatrix } from "./corpus/roadrunner";
import { urclMatrix } from "./corpus/urcl";

const USAGE = "usage: corpusB --seed <u32> --out <dir>";

const FUZZ_FAMILIES: [string, RegExp][] = [
  ["rlog", /^rlog\/(r1|r2|random)\/.*\.rlog$/],
  ["dslog", /^dslog\/.*\.dslog$/],
  ["dsevents", /^dslog\/.*\.dsevents$/],
  ["roadrunner", /^roadrunner\/(rev0|rev1|random|opmode|concatenated).*\.log$/],
  ["csv", /^csv\/.*\.csv$/],
  ["urcl", /^urcl\/.*\.wpilog$/],
  ["child", /^child\/.*\.wpilog$/]
];

const FUZZ_PER_FAMILY = 6;

/** A few mutants per format, so the fuzzer's determinism is checked with the rest of the corpus. */
function fuzzSamples(corpus: Corpus) {
  const files = corpus.cases.flatMap((c) => c.files).filter((f) => f.bytes.length < 200000);
  for (const [family, pattern] of FUZZ_FAMILIES) {
    const pool = files.filter((f) => pattern.test(f.path)).sort((a, b) => (a.path < b.path ? -1 : 1));
    for (let i = 0; i < FUZZ_PER_FAMILY; i++) {
      const id = `fuzz/${family}/${i}`;
      const rng = corpus.rng(id);
      const source = rng.pick(pool);
      const donor = rng.pick(pool);
      const { bytes, steps } = mutate(source.bytes, rng, donor.bytes);
      corpus.add(id, `${source.path} mutated (donor ${donor.path}): ${steps.join(" ")}`, [
        { path: `${id}${path.extname(source.path)}`, bytes }
      ]);
    }
  }
}

async function main() {
  const seed = seedOption(USAGE);
  const out = path.resolve(option(USAGE, "--out"));
  const corpus = new Corpus(seed);
  rlogMatrix(corpus);
  dslogMatrix(corpus);
  roadRunnerMatrix(corpus);
  await csvMatrix(corpus);
  urclMatrix(corpus);
  await childMatrix(corpus);
  mergeMatrix(corpus);
  fuzzSamples(corpus);
  await emitCorpus(corpus, out);
}

main().then(
  () => process.exit(0),
  (error) => {
    console.error(error);
    process.exit(1);
  }
);
