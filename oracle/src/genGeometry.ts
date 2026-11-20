import "./shims";
import { toTree } from "./cve1";
import { Rng } from "./corpus/rng";
import { run } from "./geometryRun";

// Emits `oracle:geometryRun` ground truth for asrs_log::geometry's grab* readers:
//   genGeometry <count> <seed>
//     {"fn":"oracle:geometryRun","args":[<cache>,[<op>...],[<call>...]],"ok":[<result>...]}
// Fixtures use only ops asrs_log already supports (putUnknownStruct, putPose,
// putTranslationArray, putRaw, scalar puts and metadata setters); putStruct
// (L08) is not needed since the grab* readers only see plain children and Raw.
// Every call inside a record returns {ok|throw}, so thrown errors like the
// /length RangeError and the missing-selection TypeError are recorded too.

const [countArg, seedArg] = process.argv.slice(2);
const count = Number(countArg);
const seed = Number(seedArg);

const f64 = (x: number): Uint8Array => {
  const b = new Uint8Array(8);
  new DataView(b.buffer).setFloat64(0, x, true);
  return b;
};
const pack = (...xs: number[]): Uint8Array => {
  const out = new Uint8Array(8 * xs.length);
  xs.forEach((x, i) => new DataView(out.buffer).setFloat64(8 * i, x, true));
  return out;
};

const pose2 = (x: number, y: number, r: number) => ({ translation: { x, y }, rotation: { value: r } });
const pose3 = (x: number, y: number, z: number, w: number, qx: number, qy: number, qz: number) => ({
  translation: { x, y, z },
  rotation: { q: { w, x: qx, y: qy, z: qz } }
});

const cases: [boolean, unknown[][], unknown[][]][] = [];
const emit = (ops: unknown[][], calls: unknown[][], cache = true) => cases.push([cache, ops, calls]);

// Pose2d children written by putPose and putUnknownStruct; reads before the
// first sample fall back to defaults, later reads see the latest write.
{
  const ops: unknown[][] = [
    ["putPose", "pose", 10, [1.5, -2.25], 0.75],
    ["putPose", "pose", 20, [3, 4], Math.PI],
    ["putUnknownStruct", "built", 15, pose2(NaN, Infinity, -0)],
    ["putUnknownStruct", "t3", 15, { x: 1, y: 2, z: 3 }],
    ["putUnknownStruct", "t3bad", 15, { x: "oops", y: 2, z: 3 }],
    ["putUnknownStruct", "rot3", 15, { q: { w: 1, x: 0, y: 0, z: 0 } }],
    ["putUnknownStruct", "rot2", 15, { value: -3.5 }],
    ["putUnknownStruct", "corner", 15, { x: 1e300, y: -5e-324 }]
  ];
  emit(ops, [
    ["grabPose2d", "pose", 0],
    ["grabPose2d", "pose", 10],
    ["grabPose2d", "pose", 15],
    ["grabPose2d", "pose", 50],
    ["grabPose2d", "built", 15],
    ["grabPose2d", "missing", 15],
    ["grabTranslation3d", "t3", 15],
    ["grabTranslation3d", "t3bad", 15],
    ["grabRotation3d", "rot3", 15],
    ["grabRotation2d", "rot2", 15],
    ["grabTargetCorner", "corner", 15],
    ["grabPosesAuto", "pose", "Pose2d", 25],
    ["grabPosesAuto", "pose", "Transform2d", 25],
    ["grabPosesAuto", "pose", "Garbage", 25],
    ["grabPosesAuto", "pose", "Pose2d", 25, "u1"]
  ]);
}

// Raw WPILib struct byte arrays, including a truncated tail record and bytes
// that decode to NaN, Infinity and -0.
{
  const ops: unknown[][] = [
    ["putRaw", "poses2", 5, pack(1, 2, 0.5, -3, -4, -0.5, 0, 0, 0)],
    ["putRaw", "poses2tail", 5, new Uint8Array([...pack(1, 2, 3, 4, 5, 6), ...new Uint8Array(7)])],
    ["putRaw", "poses2short", 5, new Uint8Array(20)],
    ["putRaw", "poses2empty", 5, new Uint8Array(0)],
    ["putRaw", "poses2nan", 5, pack(NaN, Infinity, -Infinity, -0, 5e-324, 1e300)],
    ["putRaw", "t2s", 5, pack(1, 2, -3, 4)],
    ["putRaw", "t2stail", 5, new Uint8Array([...pack(1, 2), ...new Uint8Array(15)])],
    ["putRaw", "t3s", 5, pack(1, 2, 3, -4, -5, -6)],
    ["putRaw", "poses3", 5, pack(1, 2, 3, 1, 0, 0, 0, -1, -2, -3, 0, 0, 0, -1)],
    ["putRaw", "poses3tail", 5, new Uint8Array([...pack(1, 2, 3, 1, 0, 0, 0), ...new Uint8Array(55)])],
    ["putNumber", "fallback/length", 5, 0]
  ];
  const keys = ["poses2", "poses2tail", "poses2short", "poses2empty", "poses2nan"];
  emit(ops, [
    ...keys.map((k) => ["grabPose2dArray", k, 5]),
    ...keys.map((k) => ["grabPosesAuto", k, "Pose2d[]", 5]),
    ["grabTranslation2dArray", "t2s", 5],
    ["grabTranslation2dArray", "t2stail", 5],
    ["grabTranslation3dArray", "t3s", 5],
    ["grabPose3dArray", "poses3", 5],
    ["grabPose3dArray", "poses3tail", 5],
    ["grabPosesAuto", "poses3", "Transform3d[]", 5],
    ["grabTranslation2dArray", "fallback", 5],
    ["grabPose2dArray", "fallback", 5],
    ["grabPose3dArray", "fallback", 5]
  ]);
}

// Child-decoded arrays: putUnknownStruct writes /length plus indexed children.
{
  const ops: unknown[][] = [
    ["putUnknownStruct", "ps", 8, [pose2(1, 2, 3), pose2(4, 5, 6), pose2(-1, 0, 1e21), pose2(NaN, -0, Infinity)]],
    ["putUnknownStruct", "ps3", 8, [pose3(1, 2, 3, 0.5, 0.5, 0.5, 0.5), pose3(0, 0, 0, 1, 0, 0, 0)]],
    ["putUnknownStruct", "ts", 8, [{ x: 1, y: 2 }, { x: NaN, y: -1e-300 }]],
    ["putUnknownStruct", "ts3", 8, [{ x: 1, y: 2, z: 3 }]],
    ["putUnknownStruct", "rs", 8, [{ value: 0.25 }, { value: -1e300 }, { value: NaN }]],
    ["putUnknownStruct", "rs3", 8, [{ q: { w: 0, x: 1, y: 0, z: 0 } }]],
    ["putUnknownStruct", "cs", 8, [{ x: 5, y: -7 }]],
    ["putUnknownStruct", "traj", 8, { states: [{ pose: pose3(1, 2, 3, 1, 0, 0, 0) }, { pose: pose3(4, 5, 6, 0, 1, 0, 0) }] }],
    ["putUnknownStruct", "tags", 8, [{ pose: pose3(1, 2, 3, 1, 0, 0, 0), ID: 7 }, { pose: pose3(0, 0, 0, 0, 0, 0, 0) }]],
    ["putUnknownStruct", "samples", 8, [{ pose: pose2(1, 2, 0.5) }, { pose: pose2(3, 4, -0.5) }]],
    ["putUnknownStruct", "empty", 8, []],
    ["putNumber", "huge/length", 8, 4294967296],
    ["putNumber", "frac/length", 8, 2.5],
    ["putNumber", "neg/length", 8, -1],
    ["putNumber", "nan/length", 8, NaN]
  ];
  emit(ops, [
    ["grabPose2dArray", "ps", 8],
    ["grabPosesAuto", "ps", "Transform2d[]", 8],
    ["grabPose3dArray", "ps3", 8],
    ["grabPosesAuto", "ps3", "Pose3d[]", 8],
    ["grabTranslation2dArray", "ts", 8],
    ["grabTranslation3dArray", "ts3", 8],
    ["grabRotation2dArray", "rs", 8],
    ["grabRotation3dArray", "rs3", 8],
    ["grabTargetCornerArray", "cs", 8],
    ["grabPosesAuto", "cs", "TargetCorner:16f6ac0dedc8eaccb951f4895d9e18b6[]", 8],
    ["grabTrajectory", "traj", 8],
    ["grabPosesAuto", "traj", "Trajectory", 8],
    ["grabAprilTagArray", "tags", 8],
    ["grabChoreoSampleArray", "samples", 8],
    ["grabPosesAuto", "samples", "DifferentialSample[]", 8],
    ["grabPosesAuto", "samples", "SwerveSample[]", 8],
    ["grabPose2dArray", "empty", 8],
    ["grabPose2dArray", "huge", 8],
    ["grabPose2dArray", "frac", 8],
    ["grabPose2dArray", "neg", 8],
    ["grabPose2dArray", "nan", 8]
  ]);
}

// AprilTag with and without ID, number rotations, and timestamp edges.
{
  const ops: unknown[][] = [
    ["putUnknownStruct", "tag", 3, { pose: pose3(1, 2, 3, 0, 0, 0, 1), ID: 42 }],
    ["putUnknownStruct", "tagNoId", 3, { pose: pose3(4, 5, 6, 1, 0, 0, 0), ID: "oops" }],
    ["putUnknownStruct", "tagNullId", 3, { pose: pose3(0, 0, 0, 1, 0, 0, 0) }],
    ["putNumber", "yaw", 3, 1.234],
    ["putNumber", "yaw2", 3, NaN],
    ["putString", "notnum", 3, "hello"]
  ];
  emit(ops, [
    ["grabAprilTag", "tag", 3],
    ["grabAprilTag", "tagNoId", 3],
    ["grabAprilTag", "tagNullId", 3],
    ["grabAprilTag", "missing", 3],
    ["grabNumberRotation", "yaw", 3],
    ["grabNumberRotation", "yaw", 3, "degrees"],
    ["grabNumberRotation", "yaw", 3, "radians"],
    ["grabNumberRotation", "yaw", 3, "gradians"],
    ["grabNumberRotation", "yaw2", 3, "degrees"],
    ["grabNumberRotation", "notnum", 3],
    ["grabNumberRotation", "missing", 3],
    ["grabPosesAuto", "yaw", "Number", 3],
    ["grabPosesAuto", "yaw", "Number", 3, undefined, "degrees"],
    ["grabPosesAuto", "yaw", "Number", 3, "u1", "degrees"],
    ["grabPosesAuto", "yaw", "Number", 3, "u1", "radians"]
  ]);
}

// Module velocities: arrangements, the legacy "speed" key, non-finite
// components through jsonCopy, and out-of-range sources.
{
  const mod = (velocity: number, angle: number) => ({ velocity, angle: { value: angle } });
  const ops: unknown[][] = [
    ["putUnknownStruct", "m4", 2, [mod(1, 0.1), mod(2, 0.2), mod(3, 0.3), mod(4, 0.4)]],
    ["putUnknownStruct", "m4nan", 2, [mod(NaN, 0), mod(Infinity, 1), mod(2, -0), mod(-0, 3)]],
    ["putUnknownStruct", "m3", 2, [mod(1, 0), mod(2, 0), mod(3, 0)]],
    ["putUnknownStruct", "m5", 2, [mod(1, 0), mod(2, 0), mod(3, 0), mod(4, 0), mod(5, 0)]],
    ["putUnknownStruct", "legacy", 2, [
      { speed: 7, angle: { value: 0.7 } },
      { speed: 8, angle: { value: 0.8 } },
      { speed: 9, angle: { value: 0.9 } },
      { speed: 10, angle: { value: 1 } }
    ]],
    ["setStructuredType", "legacy", "SwerveModuleState[]"],
    ["putUnknownStruct", "mixed", 2, [{ speed: 1, velocity: 2, angle: { value: 3 } }, { speed: 4, angle: { value: 5 } }]]
  ];
  emit(ops, [
    ["grabModuleVelocities", "m4", 2],
    ["grabModuleVelocities", "m4", 2, "3,2,1,0"],
    ["grabModuleVelocities", "m4", 2, "0,0,0,0"],
    ["grabModuleVelocities", "m4", 2, "0,1,2,3,0"],
    ["grabModuleVelocities", "m4", 2, "0,1,2,3,0,1"],
    ["grabModuleVelocities", "m4", 2, "4,3,2,1"],
    ["grabModuleVelocities", "m4", 2, "-1,1.5,abc,,3"],
    ["grabModuleVelocities", "m4", 2, ""],
    ["grabModuleVelocities", "m4", 2, " 0 , 2 ,x"],
    ["grabModuleVelocities", "m4", 2, "0.5,1,2,3"],
    ["grabModuleVelocities", "m4", 2, "1e9,1,2,3"],
    ["grabModuleVelocities", "m4", 2, "1e20,1,2,3"],
    ["grabModuleVelocities", "m4", 2, "-0,1,2,3"],
    ["grabModuleVelocities", "m4nan", 2, "3,2,1,0"],
    ["grabModuleVelocities", "m3", 2, "2,1,0"],
    ["grabModuleVelocities", "m5", 2, "4,3,2,1,0"],
    ["grabModuleVelocities", "legacy", 2],
    ["grabModuleVelocities", "legacy", 2, "3,2,1,0"],
    ["grabModuleVelocities", "mixed", 2],
    ["grabModuleVelocities", "missing", 2, "0,1,2,3"],
    ["grabModuleVelocities", "m4", 2, "3,2,1,0", "u1"]
  ]);
}

// Robot velocities.
{
  const ops: unknown[][] = [
    ["putUnknownStruct", "v", 4, { vx: 1.5, vy: -2.5, omega: 3.5 }],
    ["putUnknownStruct", "vp", 4, { vx: 1 }]
  ];
  emit(ops, [
    ["grabRobotVelocities", "v", 4],
    ["grabRobotVelocities", "vp", 4],
    ["grabRobotVelocities", "missing", 4]
  ]);
}

// Heatmap over a DriverStation boolean series, every time range, and the
// missing-selection TypeError for "visible".
{
  const ops: unknown[][] = [
    ["putBoolean", "/DriverStation/Enabled", 0, false],
    ["putBoolean", "/DriverStation/Enabled", 1, true],
    ["putBoolean", "/DriverStation/Enabled", 3, false],
    ["putBoolean", "/DriverStation/Enabled", 5, true],
    ["putBoolean", "/DriverStation/Autonomous", 1, true],
    ["putBoolean", "/DriverStation/Autonomous", 2, false],
    ["putBoolean", "/DriverStation/Autonomous", 5, true],
    ["putBoolean", "/DriverStation/Autonomous", 6, false],
    ["putUnknownStruct", "pose", 0, pose2(1, 2, 0.5)],
    ["putUnknownStruct", "pose", 4, pose2(9, 9, 0)]
  ];
  for (const range of ["enabled", "auto", "teleop", "teleop-no-endgame", "full"]) {
    emit(ops, [["grabHeatmapData", "pose", "Pose2d", range]]);
  }
  emit(ops, [
    ["grabHeatmapData", "pose", "Pose2d", "visible", [0.5, 2.5]],
    ["grabHeatmapData", "pose", "Pose2d", "visible", null],
    ["grabHeatmapData", "pose", "Pose2d", "enabled", null, "u1"],
    ["grabHeatmapData", "pose", "Pose3d", "full"],
    ["grabHeatmapData", "missing", "Pose2d", "enabled"]
  ]);
}

// Heatmap with no enabled key at all: findLast comes back undefined, which is
// !== "disabled", so "enabled" still samples while "auto"/"teleop" do not.
{
  const ops: unknown[][] = [["putUnknownStruct", "pose", 1, pose2(1, 1, 0)], ["putUnknownStruct", "pose", 4, pose2(2, 2, 0)]];
  emit(ops, [
    ["grabHeatmapData", "pose", "Pose2d", "enabled"],
    ["grabHeatmapData", "pose", "Pose2d", "auto"],
    ["grabHeatmapData", "pose", "Pose2d", "full"]
  ]);
}

// Heatmap over the other state key encodings: FMSControlData bitfields,
// RobotMode strings, DSDisabled/DSTeleop inversion and the utility "test" flag.
{
  const ops: unknown[][] = [
    ["putNumber", "NT:/FMSInfo/FMSControlData", 0, 0],
    ["putNumber", "NT:/FMSInfo/FMSControlData", 1, 1],
    ["putNumber", "NT:/FMSInfo/FMSControlData", 2, 3],
    ["putNumber", "NT:/FMSInfo/FMSControlData", 3, 5],
    ["putNumber", "NT:/FMSInfo/FMSControlData", 4, 7],
    ["putUnknownStruct", "pose", 0, pose2(1, 1, 0)]
  ];
  emit(ops, [
    ["grabHeatmapData", "pose", "Pose2d", "enabled"],
    ["grabHeatmapData", "pose", "Pose2d", "auto"],
    ["grabHeatmapData", "pose", "Pose2d", "teleop"]
  ]);
}
{
  const ops: unknown[][] = [
    ["putBoolean", "/DriverStation/Enabled", 0, true],
    ["putString", "/DriverStation/RobotMode", 0, "autonomous"],
    ["putString", "/DriverStation/RobotMode", 2, "TELEOP"],
    ["putString", "/DriverStation/RobotMode", 4, "test"],
    ["putString", "/DriverStation/RobotMode", 6, "utility"],
    ["putString", "/DriverStation/RobotMode", 8, "disabled"],
    ["putUnknownStruct", "pose", 0, pose2(1, 1, 0)]
  ];
  emit(ops, [
    ["grabHeatmapData", "pose", "Pose2d", "auto"],
    ["grabHeatmapData", "pose", "Pose2d", "teleop"],
    ["grabHeatmapData", "pose", "Pose2d", "enabled"]
  ]);
}
{
  const ops: unknown[][] = [
    ["putBoolean", "/DSLog/Status/DSDisabled", 0, true],
    ["putBoolean", "/DSLog/Status/DSDisabled", 1, false],
    ["putBoolean", "/DSLog/Status/DSTeleop", 1, true],
    ["putBoolean", "/DSLog/Status/DSTeleop", 3, false],
    ["putUnknownStruct", "pose", 0, pose2(1, 1, 0)]
  ];
  emit(ops, [
    ["grabHeatmapData", "pose", "Pose2d", "enabled"],
    ["grabHeatmapData", "pose", "Pose2d", "auto"],
    ["grabHeatmapData", "pose", "Pose2d", "teleop"]
  ]);
}
{
  const ops: unknown[][] = [
    ["putBoolean", "/DriverStation/Enabled", 0, true],
    ["putBoolean", "DS:test", 0, false],
    ["putBoolean", "DS:test", 1, true],
    ["putBoolean", "DS:test", 3, false],
    ["putUnknownStruct", "pose", 0, pose2(1, 1, 0)]
  ];
  emit(ops, [
    ["grabHeatmapData", "pose", "Pose2d", "enabled"],
    ["grabHeatmapData", "pose", "Pose2d", "teleop"],
    ["grabHeatmapData", "pose", "Pose2d", "teleop-no-endgame"]
  ]);
}
// teleop-no-endgame: an open-ended last range never satisfies end - t > 30.
{
  const ops: unknown[][] = [
    ["putBoolean", "/DriverStation/Enabled", 0, true],
    ["putBoolean", "/DriverStation/Enabled", 40, false],
    ["putUnknownStruct", "pose", 0, pose2(1, 1, 0)]
  ];
  emit(ops, [
    ["grabHeatmapData", "pose", "Pose2d", "teleop-no-endgame"],
    ["grabHeatmapData", "pose", "Pose2d", "teleop"]
  ]);
}

// Samples before the read timestamp are ignored by getOrDefault; reads honor
// uuid on the parent key but the array fallback drops it for children.
{
  const ops: unknown[][] = [
    ["putUnknownStruct", "p", 10, pose2(1, 2, 3)],
    ["putUnknownStruct", "p", 20, pose2(4, 5, 6)],
    ["putNumber", "arr/length", 10, 2],
    ["putNumber", "arr/0/x", 5, 1],
    ["putNumber", "arr/0/x", 15, 9],
    ["putNumber", "arr/0/y", 5, 2],
    ["putNumber", "arr/1/x", 5, 3],
    ["putNumber", "arr/1/y", 5, 4],
    ["putBoolean", "/DriverStation/Enabled", 0, true]
  ];
  emit(ops, [
    ["grabPose2d", "p", 15],
    ["grabPose2d", "p", 5],
    ["grabTranslation2dArray", "arr", 12],
    ["grabTranslation2dArray", "arr", 20],
    ["grabTranslation2dArray", "arr", 12, "u1"],
    ["grabHeatmapData", "p", "Pose2d", "enabled"]
  ]);
}

// A reload mid-fixture and a cache-disabled replay.
{
  const ops: unknown[][] = [
    ["putUnknownStruct", "p", 10, pose2(1, 2, 3)],
    ["reload"],
    ["putUnknownStruct", "p", 20, pose2(4, 5, 6)],
    ["putBoolean", "/DriverStation/Enabled", 0, true]
  ];
  emit(
    ops,
    [
      ["grabPose2d", "p", 15],
      ["grabHeatmapData", "p", "Pose2d", "enabled"]
    ],
    false
  );
}

// Seeded random fixtures: putUnknownStruct poses with arbitrary doubles
// (including NaN, infinities, -0 and huge magnitudes), random timestamps and
// uuid arguments across the child-path grab* readers.
{
  const r = new Rng(seed, "geometry");
  const weird = () => r.pick([NaN, Infinity, -Infinity, 0, -0, 1, -1, 1e300, -1e300, 5e-324, 1e21, Math.PI]);
  const GRABS = [
    "grabPose2d",
    "grabPose3d",
    "grabTranslation2d",
    "grabTranslation3d",
    "grabRotation2d",
    "grabRotation3d",
    "grabTargetCorner",
    "grabAprilTag",
    "grabChoreoSample",
    "grabTrajectory"
  ];
  const LOG_TYPES = [
    "Pose2d",
    "Pose3d",
    "Transform2d",
    "Transform3d",
    "Translation2d",
    "Translation3d",
    "Rotation2d",
    "Rotation3d",
    "Number",
    "Trajectory",
    "Nonsense",
    "Pose2d[]",
    "Pose3d[]",
    "Translation2d[]"
  ];
  for (let i = 0; i < count; i++) {
    const key = `k${i % 7}`;
    const ops: unknown[][] = [
      ["putUnknownStruct", key, r.range(0, 10), pose2(weird(), weird(), weird())],
      ["putUnknownStruct", `${key}3`, r.range(0, 10), pose3(weird(), weird(), weird(), weird(), weird(), weird(), weird())],
      ["putUnknownStruct", `${key}arr`, r.range(0, 10), Array.from({ length: r.below(5) }, () => pose2(weird(), weird(), weird()))],
      ["putUnknownStruct", `${key}tag`, r.range(0, 10), { pose: pose3(weird(), weird(), weird(), weird(), weird(), weird(), weird()), ID: weird() }],
      ["putBoolean", "/DriverStation/Enabled", r.range(0, 3), r.bool()],
      ["putBoolean", "/DriverStation/Enabled", r.range(4, 8), r.bool()]
    ];
    const calls: unknown[][] = [];
    const ncalls = r.range(1, 6);
    for (let j = 0; j < ncalls; j++) {
      const t = r.pick([r.range(-5, 30), r.unit() * 20 - 5, NaN, Infinity]);
      const uuid = r.below(4) === 0 ? `u${r.below(3)}` : undefined;
      switch (r.below(4)) {
        case 0:
          calls.push([r.pick(GRABS), key, t, uuid]);
          break;
        case 1:
          calls.push(["grabPosesAuto", key, r.pick(LOG_TYPES), t, uuid, r.pick([undefined, "degrees", "radians"])]);
          break;
        case 2:
          calls.push(["grabPose2dArray", `${key}arr`, t, uuid]);
          calls.push(["grabPose3dArray", `${key}3`, t, uuid]);
          calls.push(["grabAprilTagArray", `${key}tag`, t, uuid]);
          break;
        default:
          calls.push([
            "grabHeatmapData",
            key,
            r.pick(["Pose2d", "Pose3d", "Translation2d"]),
            r.pick(["enabled", "auto", "teleop", "teleop-no-endgame", "full", "visible"]),
            r.below(2) === 0 ? [r.range(0, 10), r.range(0, 40)] : null,
            uuid
          ]);
          break;
      }
    }
    emit(ops, calls, r.below(4) !== 0);
  }
}

for (const [cache, ops, calls] of cases) {
  const args = [toTree(cache), toTree(ops), toTree(calls)];
  const result = run(cache, ops as any[][], calls as any[][]);
  process.stdout.write(JSON.stringify({ fn: "oracle:geometryRun", args, ok: toTree(result) }) + "\n");
}
