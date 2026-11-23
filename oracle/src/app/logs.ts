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

// Console output over 10 s: plain, long, tabbed and multi-line lines, ANSI renditions, error and
// warning lines and some text outside ASCII, more than one window of rows.
function robotConsole(): Uint8Array {
  const encoder = new WPILOGEncoder("AdvantageScope");
  encoder.add(WPILOGEncoderRecord.makeControlStart(0, { entry: 1, name: "/RobotConsole", type: "string", metadata: "" }));
  encoder.add(WPILOGEncoderRecord.makeControlStart(0, { entry: 2, name: "/Battery", type: "double", metadata: "" }));
  const lines = [
    "Robot program starting on 10.63.28.2 (roboRIO 2, image 2026_v1.2)",
    "\u001b[32mConnected\u001b[0m to \u001b[1mPower Distribution\u001b[0m on \u001b[4mCAN 1\u001b[0m, \u001b[4mqueue y/g\u001b[0m, \u001b]8;;https://docs.advantagescope.org/\u0007docs\u001b]8;;\u0007",
    "Warning: Loop time of 0.02s overrun by 0.0131s in Drive.periodic(), SuperstructureStateMachine.periodic(), Vision.processFrames(): 0.018402s, Telemetry.flush(): 0.004211s, total 0.0331s",
    "\u001b[31;1mERROR\u001b[0m 1 DriveMotor[FL] (id=3): StatusCode::kStale -- no frame for 250ms",
    "{\"pose\":{\"x\":3.25,\"y\":-1.5,\"theta\":0.78539},\"targets\":[1,2,3],\"latency_ms\":12.5,\"path\":\"/home/lvuser/deploy/pathplanner/autos/Four-Piece_Amp+Center(v2).auto\"}",
    "col1\tcol2\tcolumn three\tlast",
    "first line\nsecond line\n    indented third line",
    "\\x1b[33mescaped yellow\\x1b[0m and \\ntext after an escaped newline",
    "\u001b[2mfaint\u001b[0m \u001b[2;44mfaint on blue\u001b[0m \u001b[3mitalic\u001b[0m \u001b[41;97mwhite on red\u001b[0m \u001b[38;5;208m208\u001b[0m \u001b[48;2;0;64;128;38;2;255;255;0mtrue colour\u001b[0m",
    "r\u00e9sum\u00e9 \u00b5s \u00b0C \u2192 \u2713 na\u00efve",
    "    at frc.robot.subsystems.drive.Drive.runVelocity(Drive.java:214)",
    "https://docs.advantagescope.org/tab-reference/console?field=/RobotConsole&filter=!warn#usage"
  ];
  for (let i = 0; i < 48; i++) {
    encoder.add(WPILOGEncoderRecord.makeString(1, i * 208000 + 125000, i < lines.length ? lines[i] : `[${i}] ${lines[(i * 5) % lines.length]}`));
  }
  for (let i = 0; i <= 10; i++) encoder.add(WPILOGEncoderRecord.makeDouble(2, i * 1000000, 12.5 - i / 8));
  return encoder.getEncoded();
}

export const GENERATED_LOGS: Record<string, () => Uint8Array> = { drive, console: robotConsole };
