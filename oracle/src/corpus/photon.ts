import { Bytes, ByteSource, hex } from "./bytes";
import { Corpus } from "./corpus";
import { Rng } from "./rng";
import { WPILIB_STRUCTS, putStructSchema } from "./struct";
import { LogSpec } from "./wpilog";

const WIDTHS: { [type: string]: number } = {
  bool: 1,
  char: 1,
  int8: 1,
  uint8: 1,
  int16: 2,
  uint16: 2,
  int32: 4,
  uint32: 4,
  int64: 8,
  uint64: 8,
  float: 4,
  float32: 4,
  double: 8,
  float64: 8
};

interface Field {
  type: string;
  name: string;
  optional: boolean;
  vla: boolean;
}

function parseSchema(text: string): Field[] {
  return text
    .split(";")
    .filter((s) => s.length > 0)
    .map((s) => {
      const optional = s.startsWith("optional ");
      const parts = (optional ? s.slice(9) : s).split(" ").filter((p) => p.length > 0);
      const nameStr = parts.slice(1).join("");
      return { type: parts[0], name: nameStr.split("[")[0], optional, vla: nameStr.includes("[?]") };
    });
}

const HASH = {
  metadata: "PhotonPipelineMetadata:626e70461cbdb274fb43ead09c255f4e",
  corner: "TargetCorner:16f6ac0dedc8eaccb951f4895d9e18b6",
  pnp: "PnpResult:ae4d655c0a3104d88df4f5db144c1e86",
  multi: "MultiTargetPNPResult:541096947e9f3ca2d3f425ff7b04aa7b",
  target: "PhotonTrackedTarget:cc6dbb5c5c1e0fa808108019b20863f1",
  result: "PhotonPipelineResult:4b2ff16a964b5e2bf04be0c1454d91c4"
};

const PHOTON_SCHEMAS: [string, string][] = [
  [
    HASH.metadata,
    "int64 sequenceID;int64 captureTimestampMicros;int64 publishTimestampMicros;int64 timeSinceLastPong;"
  ],
  [HASH.corner, "float64 x;float64 y;"],
  [HASH.pnp, "Transform3d best;Transform3d alt;float64 bestReprojErr;float64 altReprojErr;float64 ambiguity;"],
  [HASH.multi, `${HASH.pnp} estimatedPose;int16 fiducialIDsUsed[?];`],
  [
    HASH.target,
    "float64 yaw;float64 pitch;float64 area;float64 skew;int32 fiducialId;int32 objDetectId;float32 objDetectConf;" +
      `Transform3d bestCameraToTarget;Transform3d altCameraToTarget;float64 poseAmbiguity;${HASH.corner} minAreaRectCorners[?];` +
      `${HASH.corner} detectedCorners[?];`
  ],
  [HASH.result, `${HASH.metadata} metadata;${HASH.target} targets[?];optional ${HASH.multi} multitagResult;`]
];

/** Writes a value for `name` with random content; VLA lengths and optional presence are drawn from the rng. */
class PhotonWriter {
  private schemas = new Map<string, Field[]>();

  constructor(
    readonly rng: Rng,
    schemas: [string, string][]
  ) {
    schemas.forEach(([name, text]) => this.schemas.set(name, parseSchema(text)));
  }

  write(name: string, out = new Bytes(), depth = 0): Bytes {
    for (const field of this.schemas.get(name)!) {
      if (field.optional) {
        const present = this.rng.below(3) !== 0;
        out.u8(present ? 1 : 0);
        if (!present) continue;
      }
      const count = field.vla ? this.rng.below(depth > 1 ? 2 : 4) : 1;
      if (field.vla) out.u8(count);
      for (let i = 0; i < count; i++) {
        if (field.type in WIDTHS) out.raw(this.rng.bytes(WIDTHS[field.type]));
        else this.write(field.type, out, depth + 1);
      }
    }
    return out;
  }
}

class PhotonCase {
  readonly spec = new LogSpec();
  private ids = new Map<string, number>();
  private t = 1000;

  schema(name: string, text: ByteSource) {
    this.spec.put(this.spec.entry(`/.schema/photonstruct:${name}`, "photonstructschema"), this.t, text);
    return this;
  }

  data(key: string, schema: string, bytes: ByteSource) {
    if (!this.ids.has(key)) this.ids.set(key, this.spec.entry(key, `photonstruct:${schema}`));
    this.t += 20000;
    this.spec.put(this.ids.get(key)!, this.t, bytes);
    return this;
  }
}

function wpilibGeometry(c: PhotonCase) {
  WPILIB_STRUCTS.filter(([name]) => /3d|Quaternion/.test(name)).forEach(([name, text]) =>
    putStructSchema(c.spec, name, text)
  );
}

function randomPhotonSchemas(rng: Rng, prefix: string): [string, string][] {
  const out: [string, string][] = [];
  const primitives = Object.keys(WIDTHS).filter((t) => t !== "char");
  for (let s = 0; s < 4; s++) {
    const fields: string[] = [];
    const n = rng.range(1, 6);
    for (let f = 0; f < n; f++) {
      const type = out.length > 0 && rng.below(4) === 0 ? rng.pick(out)[0] : rng.pick(primitives);
      const roll = rng.below(4);
      fields.push(roll === 0 ? `optional ${type} f${f}` : roll === 1 ? `${type} f${f}[?]` : `${type} f${f}`);
    }
    out.push([`${prefix}${s}:${rng.u32().toString(16).padStart(8, "0")}`, fields.join(";") + ";"]);
  }
  return out;
}

export function photonMatrix(corpus: Corpus) {
  {
    const id = "payload/photon/pipeline-result";
    const c = new PhotonCase();
    wpilibGeometry(c);
    PHOTON_SCHEMAS.forEach(([name, text]) => c.schema(name, text));
    const w = new PhotonWriter(corpus.rng(id), [...WPILIB_STRUCTS, ...PHOTON_SCHEMAS]);
    for (let i = 0; i < 16; i++) c.data("/photonvision/front/rawBytes", HASH.result, w.write(HASH.result).finish());
    for (let i = 0; i < 4; i++) c.data("/photonvision/front/target", HASH.target, w.write(HASH.target).finish());
    corpus.spec(
      id,
      "PhotonVision pipeline results with nested WPILib Transform3d, VLAs and an optional multitag result",
      c.spec
    );
  }
  {
    const id = "payload/photon/optional-vla";
    const c = new PhotonCase();
    const inner = "Inner:0123456789abcdef";
    c.schema(inner, "float32 x;bool y;");
    c.schema("Mixed:ffff", `optional float64 a;int8 v[?];optional ${inner} b;${inner} list[?];int32 after;`);
    const payloads = [
      "00 00 00 00 00 00 00 00 01 00 00 00",
      "01 00 00 00 00 00 00 f0 3f 03 01 02 03 01 00 00 80 3f 01 02 00 00 00 40 00 00 00 40 40 01 2a 00 00 00",
      "00 ff 00 00 00 00 00 07 00 00 00",
      "01 00 00 00 00 00 00 f8 7f 7f 01 02",
      "00 02 01",
      "",
      "01"
    ];
    payloads.forEach((p) => c.data("/Photon/Mixed", "Mixed:ffff", hex(p)));
    corpus.spec(id, "optional presence flags, VLA lengths 0, 3, -1 and 127, and truncated buffers that throw", c.spec);
  }
  {
    const id = "payload/photon/compile";
    const c = new PhotonCase();
    c.data("/Photon/Late", "Late:1", hex("01 02 00 00 00"));
    c.schema("Fixed:1", "float64 a[3];");
    c.schema("OptVla:1", "optional int8 v[?];");
    c.schema("Unknown:1", "vec3 a;");
    c.schema("CharTail:1", "int8 a;char rest;");
    c.schema("Spaces:1", "  int8   a ;optional  float32 b;");
    c.schema("Late:1", "int8 a;int32 b;");
    c.schema("Bom:1", hex("ef bb bf 69 6e 74 38 20 61 3b"));
    c.schema("Late:1", "double replaced;");
    c.data("/Photon/Late", "Late:1", hex("03 04 00 00 00"));
    c.data("/Photon/Fixed", "Fixed:1", new Uint8Array(24)).data("/Photon/OptVla", "OptVla:1", hex("01 01 05"));
    c.data("/Photon/Unknown", "Unknown:1", hex("00")).data("/Photon/Missing", "Missing:1", hex("00"));
    c.data("/Photon/CharTail", "CharTail:1", hex("05 68 65 6c 6c 6f c3 a9 ff"));
    c.data("/Photon/CharTail", "CharTail:1", hex("05"));
    c.data("/Photon/Spaces", "Spaces:1", hex("09 01 00 00 c0 3f")).data("/Photon/Spaces", "Spaces:1", hex("09 00"));
    c.data("/Photon/Bom", "Bom:1", hex("80"));
    corpus.spec(
      id,
      "schemas that never compile (fixed arrays, optional VLAs, unknown types), late and duplicate schemas, char tails",
      c.spec
    );
  }
  for (let n = 0; n < 2; n++) {
    const id = `payload/photon/random-${n}`;
    const rng = corpus.rng(id);
    const schemas = randomPhotonSchemas(rng, `P${n}_`);
    const c = new PhotonCase();
    schemas.forEach(([name, text]) => c.schema(name, text));
    const w = new PhotonWriter(rng, schemas);
    schemas.forEach(([name]) => {
      for (let i = 0; i < 6; i++) c.data(`/Photon/Random/${name.split(":")[0]}`, name, w.write(name).finish());
    });
    corpus.spec(id, "seeded random photonstruct schemas with optionals, VLAs and nesting, random payloads", c.spec);
  }
}
