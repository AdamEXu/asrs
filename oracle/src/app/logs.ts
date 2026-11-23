import { WPILOGEncoder, WPILOGEncoderRecord } from "@upstream/hub/dataSources/wpilog/WPILOGEncoder";

const SCHEMAS: [string, string][] = [
  ["Translation2d", "double x;double y"],
  ["Rotation2d", "double value"],
  ["Pose2d", "Translation2d translation;Rotation2d rotation"],
  ["Translation3d", "double x;double y;double z"],
  ["Quaternion", "double w;double x;double y;double z"],
  ["Rotation3d", "Quaternion q"],
  ["Pose3d", "Translation3d translation;Rotation3d rotation"]
];

function doubles(values: number[]): Uint8Array {
  const out = new Uint8Array(values.length * 8);
  const view = new DataView(out.buffer);
  values.forEach((v, i) => view.setFloat64(i * 8, v, true));
  return out;
}

// A 10 s drive around a square sampled at 50 Hz, written by upstream's own encoder. Only basic
// arithmetic, no Math.sin or Math.pow, so the bytes are the same on every host.
function drive(): Uint8Array {
  const encoder = new WPILOGEncoder("AdvantageScope");
  let nextEntry = 1;
  const start = (name: string, type: string, metadata = "") => {
    const entry = nextEntry++;
    encoder.add(WPILOGEncoderRecord.makeControlStart(0, { entry, name, type, metadata }));
    return entry;
  };
  const utf8 = new TextEncoder();
  for (const [name, schema] of SCHEMAS) {
    encoder.add(WPILOGEncoderRecord.makeRaw(start(`/.schema/struct:${name}`, "structschema"), 0, utf8.encode(schema)));
  }
  const left = start("/Drive/LeftVelocity", "double");
  const right = start("/Drive/RightVelocity", "double");
  const enabled = start("/DriverStation/Enabled", "boolean");
  const mode = start("/Drive/Mode", "string");
  const pose = start("/Odometry/Robot", "struct:Pose2d");
  const pose3d = start("/Odometry/Robot3d", "struct:Pose3d");
  const modules = start("/Drive/ModuleSpeeds", "double[]");

  const samples = 500;
  const step = 20000;
  for (let i = 0; i <= samples; i++) {
    const t = i * step;
    const phase = (i % 100) / 100;
    const tri = phase < 0.5 ? phase * 4 - 1 : 3 - phase * 4;
    encoder.add(WPILOGEncoderRecord.makeDouble(left, t, 2 + tri));
    encoder.add(WPILOGEncoderRecord.makeDouble(right, t, 2 - tri / 2));
    const quarter = Math.floor(i / 125);
    const s = [0, 1, 0, -1][quarter % 4];
    const c = [1, 0, -1, 0][quarter % 4];
    const frac = (i % 125) / 125;
    const x = 8 + 3 * (c - s * frac * 1.5);
    const y = 4 + 3 * (s + c * frac * 1.5);
    const heading = (quarter + frac) * 0.5;
    encoder.add(WPILOGEncoderRecord.makeRaw(pose, t, doubles([x, y, heading])));
    encoder.add(WPILOGEncoderRecord.makeRaw(pose3d, t, doubles([x, y, 0, 1, 0, 0, 0])));
    if (i % 25 === 0) encoder.add(WPILOGEncoderRecord.makeDoubleArray(modules, t, [tri, -tri, tri / 2, 1]));
    if (i % 50 === 0) encoder.add(WPILOGEncoderRecord.makeBoolean(enabled, t, i >= 50 && i < 450));
    if (i % 125 === 0) encoder.add(WPILOGEncoderRecord.makeString(mode, t, ["Idle", "Auto", "Teleop", "Test", "Idle"][quarter]));
  }
  return encoder.getEncoded();
}

// A 4 s log for the Swerve, Mechanism, Points and Joysticks tabs: module velocities, robot
// velocities and a heading, two 2D mechanisms, corner lists and split coordinates, and three
// joysticks, two in the DataLog layout and one AdvantageKit (SDL) with a touchpad.
function panels(): Uint8Array {
  const encoder = new WPILOGEncoder("AdvantageScope");
  let nextEntry = 1;
  const start = (name: string, type: string) => {
    const entry = nextEntry++;
    encoder.add(WPILOGEncoderRecord.makeControlStart(0, { entry, name, type, metadata: "" }));
    return entry;
  };
  const utf8 = new TextEncoder();
  const schemas: [string, string][] = [
    ["Translation2d", "double x;double y"],
    ["Rotation2d", "double value"],
    ["SwerveModuleState", "double speed;Rotation2d angle"],
    ["ChassisSpeeds", "double vx;double vy;double omega"]
  ];
  for (const [name, schema] of schemas) {
    encoder.add(WPILOGEncoderRecord.makeRaw(start(`/.schema/struct:${name}`, "structschema"), 0, utf8.encode(schema)));
  }
  const modules = start("/Swerve/Modules", "struct:SwerveModuleState[]");
  const speeds = start("/Swerve/Speeds", "struct:ChassisSpeeds");
  const heading = start("/Swerve/Heading", "struct:Rotation2d");
  const corners = start("/Vision/Corners", "struct:Translation2d[]");
  const listX = start("/Vision/ListX", "double[]");
  const listY = start("/Vision/ListY", "double[]");

  const constants: [string, string | number | number[]][] = [
    ["/Mech/Arm/.type", "Mechanism2d"],
    ["/Mech/Arm/backgroundColor", "#202060"],
    ["/Mech/Arm/dims", [3, 2]],
    ["/Mech/Arm/base/x", 1.5],
    ["/Mech/Arm/base/y", 0.25],
    ["/Mech/Arm/base/lift/.type", "line"],
    ["/Mech/Arm/base/lift/angle", 90],
    ["/Mech/Arm/base/lift/color", "#ff8040"],
    ["/Mech/Arm/base/lift/weight", 10],
    ["/Mech/Arm/base/lift/wrist/.type", "line"],
    ["/Mech/Arm/base/lift/wrist/length", 0.6],
    ["/Mech/Arm/base/lift/wrist/color", "#40c0ff"],
    ["/Mech/Arm/base/lift/wrist/weight", 6.5],
    ["/Mech/Claw/.type", "Mechanism2d"],
    ["/Mech/Claw/backgroundColor", "#f0e8d8"],
    ["/Mech/Claw/dims", [2, 2.5]],
    ["/Mech/Claw/pivot/x", 0.4],
    ["/Mech/Claw/pivot/y", 1.75],
    ["/Mech/Claw/pivot/finger/.type", "line"],
    ["/Mech/Claw/pivot/finger/length", 1.1],
    ["/Mech/Claw/pivot/finger/color", "#30a050"],
    ["/Mech/Claw/pivot/finger/weight", 3]
  ];
  for (const [name, value] of constants) {
    if (typeof value === "string") encoder.add(WPILOGEncoderRecord.makeString(start(name, "string"), 0, value));
    else if (typeof value === "number") encoder.add(WPILOGEncoderRecord.makeDouble(start(name, "double"), 0, value));
    else encoder.add(WPILOGEncoderRecord.makeDoubleArray(start(name, "double[]"), 0, value));
  }
  const liftLength = start("/Mech/Arm/base/lift/length", "double");
  const wristAngle = start("/Mech/Arm/base/lift/wrist/angle", "double");
  const fingerAngle = start("/Mech/Claw/pivot/finger/angle", "double");

  const pads = [0, 1].map((i) => ({
    buttons: start(`DS:joystick${i}/buttons`, "boolean[]"),
    axes: start(`DS:joystick${i}/axes`, "float[]"),
    povs: start(`DS:joystick${i}/povs`, "int64[]")
  }));
  const sdl: [string, string, number | number[] | boolean][] = [
    ["ButtonsAvailable", "int64", 4095],
    ["AxesAvailable", "int64", 63],
    ["POVsAvailable", "int64", 1],
    ["POVValues", "int64[]", [3]],
    ["TouchpadCount", "int64", 1],
    ["Touchpad/0/FingerCount", "int64", 2],
    ["Touchpad/0/Finger/0/Down", "boolean", true],
    ["Touchpad/0/Finger/0/X", "double", 0.25],
    ["Touchpad/0/Finger/0/Y", "double", 0.75],
    ["Touchpad/0/Finger/1/Down", "boolean", false],
    ["Touchpad/0/Finger/1/X", "double", 0.8],
    ["Touchpad/0/Finger/1/Y", "double", 0.3]
  ];
  for (const [name, type, value] of sdl) {
    const entry = start(`/DriverStation/Joystick2/${name}`, type);
    if (typeof value === "boolean") encoder.add(WPILOGEncoderRecord.makeBoolean(entry, 0, value));
    else if (typeof value !== "number") encoder.add(WPILOGEncoderRecord.makeIntegerArray(entry, 0, value));
    else if (type === "double") encoder.add(WPILOGEncoderRecord.makeDouble(entry, 0, value));
    else encoder.add(WPILOGEncoderRecord.makeInteger(entry, 0, value));
  }
  const sdlButtons = start("/DriverStation/Joystick2/ButtonValues", "int64");
  const sdlAxes = start("/DriverStation/Joystick2/AxisValues", "float[]");

  for (let i = 0; i <= 40; i++) {
    const t = i * 100000;
    const f = i / 40;
    const tri = (i % 20) / 10 - 1;
    encoder.add(WPILOGEncoderRecord.makeRaw(modules, t, doubles([4.5 * f, 0.5, 3, 1 - f, -2.5, 2 * f, 1.25, -1.5])));
    encoder.add(WPILOGEncoderRecord.makeRaw(speeds, t, doubles([2 + f, 1.5 - 2 * f, 0.5])));
    encoder.add(WPILOGEncoderRecord.makeRaw(heading, t, doubles([0.75 * f])));
    encoder.add(WPILOGEncoderRecord.makeRaw(corners, t, doubles([100 + 400 * f, 80, 480, 360, 0, 720, 860.5, 120 + 300 * f, 1000, 100])));
    encoder.add(WPILOGEncoderRecord.makeDoubleArray(listX, t, [0, 160, 320, 480, 640, 800, 960].map((x) => x + 40 * f)));
    encoder.add(WPILOGEncoderRecord.makeDoubleArray(listY, t, [700, 600 - 100 * f, 500, 400, 300, 200, 20]));
    encoder.add(WPILOGEncoderRecord.makeDouble(liftLength, t, 0.5 + f));
    encoder.add(WPILOGEncoderRecord.makeDouble(wristAngle, t, -90 + 120 * f));
    encoder.add(WPILOGEncoderRecord.makeDouble(fingerAngle, t, -30 - 45 * f));
    pads.forEach((pad, j) => {
      const buttons = Array.from({ length: 10 }, (_, b) => (b + i + j) % 3 === 0);
      encoder.add(WPILOGEncoderRecord.makeBooleanArray(pad.buttons, t, buttons));
      encoder.add(WPILOGEncoderRecord.makeFloatArray(pad.axes, t, [tri, -tri, f, 1 - f, tri / 2, -0.25]));
      encoder.add(WPILOGEncoderRecord.makeIntegerArray(pad.povs, t, [[-1, 0, 45, 270][(i + j) % 4]]));
    });
    encoder.add(WPILOGEncoderRecord.makeInteger(sdlButtons, t, (i * 37) % 4096));
    encoder.add(WPILOGEncoderRecord.makeFloatArray(sdlAxes, t, [-tri, tri / 2, f, -f, 0.5, 1]));
  }
  return encoder.getEncoded();
}

export const GENERATED_LOGS: Record<string, () => Uint8Array> = { drive, panels };
