import { Corpus } from "./corpus";
import { jsonMatrix } from "./json";
import { msgpackMatrix } from "./msgpack";
import { photonMatrix } from "./photon";
import { protoMatrix } from "./proto";
import { structMatrix } from "./struct";
import { wpilogMatrix } from "./wpilogMatrix";

export function buildCorpusA(seed: number): Corpus {
  const corpus = new Corpus(seed);
  wpilogMatrix(corpus);
  structMatrix(corpus);
  photonMatrix(corpus);
  protoMatrix(corpus);
  msgpackMatrix(corpus);
  jsonMatrix(corpus);
  return corpus;
}
