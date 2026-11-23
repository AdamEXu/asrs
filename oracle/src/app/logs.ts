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

// Metadata as AdvantageKit publishes it: enough rows to overflow the tab, with values that wrap,
// break inside words, hold line breaks or are missing.
function metadata(): Uint8Array {
  const encoder = new WPILOGEncoder("AdvantageScope");
  let nextEntry = 1;
  const put = (name: string, value: string) => {
    const entry = nextEntry++;
    encoder.add(WPILOGEncoderRecord.makeControlStart(0, { entry, name, type: "string", metadata: "" }));
    encoder.add(WPILOGEncoderRecord.makeString(entry, 0, value));
  };
  const battery = nextEntry++;
  encoder.add(WPILOGEncoderRecord.makeControlStart(0, { entry: battery, name: "/Battery", type: "double", metadata: "" }));
  for (let i = 0; i <= 10; i++) encoder.add(WPILOGEncoderRecord.makeDouble(battery, i * 1000000, 12.5 - i / 8));
  const build: [string, string][] = [
    ["ProjectName", "KitBot"],
    ["BuildDate", "2026-11-22 16:04:05 PST"],
    ["GitSHA", "a1b2c3d4e5f60718293a4b5c6d7e8f9001122334"],
    ["GitBranch", "feature/auto-align-to-reef-with-vision-fallback"],
    ["GitDirty", "Uncomitted changes"],
    ["RuntimeType", "kRoboRIO"],
    ["Notes", "First line\nSecond line, which goes on until it is long enough that it has to wrap onto another one, even in one column\n\nLast line"],
    ["SuperstructureConfiguration", "https://github.com/Mechanical-Advantage/AdvantageScope/blob/main/docs/docs/tab-reference/metadata.md?plain=1#L10-L20"]
  ];
  for (const [key, value] of build) put(`/RealMetadata/${key}`, value);
  for (const [key, value] of build.slice(0, 5)) put(`/ReplayMetadata/${key}`, key === "BuildDate" ? "2026-11-23 09:30:00 PST" : value);
  put("/ReplayMetadata/ReplayOnly", "only in replay");
  for (let i = 0; i < 16; i++) put(`/RealMetadata/Extra/Setting${String(i).padStart(2, "0")}`, `value ${i * 37}`);
  return encoder.getEncoded();
}

export const GENERATED_LOGS: Record<string, () => Uint8Array> = { drive, metadata };
