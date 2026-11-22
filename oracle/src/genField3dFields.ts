import "./shims";
import { toTree } from "./cve1";
import { run } from "./lazyHelpers/field3dFieldsRun";

// Ground truth for asrs-field3d's procedural fields (src/shared/renderers/field3d
// EvergreenField.ts, AxesField.ts, AxesTemplate.ts): each case builds one field with
// `oracle:field3dFieldsRun` and records every object under it:
//
//   {"case", "field": "evergreen" | "axes" | "template", "args", "out": [node...]}
//
// Values are CVE-1 trees. The materials are Field3dRendererImpl's per-mode MATERIAL_SPECULAR and
// MATERIAL_SHININESS (XRRenderer uses the standard pair).

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

const MATERIALS = [
  { mode: "standard", specular: 0x000000, shininess: 0 },
  { mode: "cinematic", specular: 0x666666, shininess: 100 }
];
const SYSTEMS = ["wall-alliance", "wall-blue", "center-rotated", "center-red"];

function emit(name: string, field: string, args: object) {
  out({ case: name, field, args: toTree(args), out: toTree(run(field, args)) });
}

for (const { mode, specular, shininess } of MATERIALS) {
  for (const isFTC of [false, true]) {
    const league = isFTC ? "ftc" : "frc";
    emit(`evergreen/${league}/${mode}`, "evergreen", { specular, shininess, isFTC });
    for (const coordinateSystem of SYSTEMS) {
      emit(`axes/${league}/${coordinateSystem}/${mode}`, "axes", { specular, shininess, isFTC, coordinateSystem });
    }
  }
  emit(`template/${mode}`, "template", { specular, shininess });
}
