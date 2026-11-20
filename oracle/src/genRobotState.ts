import "./shims";
import { toTree } from "./cve1";
import { Rng } from "./corpus/rng";
import * as robotState from "@upstream/shared/log/RobotState";
import { run } from "./lazyHelpers/robotStateRun";

// Emits `oracle:robotStateRun` ground truth for asrs_log::log::robot_state:
//   genRobotState <count> <seed>
//     {"fn":"oracle:robotStateRun","args":[<cache>,[<op>...],[<call>...]],"ok":[<result>...]}
// Ops are the logRun write ops; calls are RobotState exports plus
// ["getMatchInfo", nowMs], which runs with `new Date()` reporting nowMs.
// Every call inside a record returns {ok|throw}.

const [countArg, seedArg] = process.argv.slice(2);
const count = Number(countArg);
const seed = Number(seedArg);

const NOW = Date.UTC(2026, 10, 20, 12, 0, 0);
const NOW2 = Date.UTC(2031, 0, 4, 0, 0, 0);

const cases: [boolean, unknown[][], unknown[][]][] = [];
const emit = (ops: unknown[][], calls: unknown[][], cache = true) => cases.push([cache, ops, calls]);

const times = [-1, 0, 5, 10, 15, 25, 30, 35, 40, 50, NaN, Infinity, -Infinity];

function battery(extraTimes: number[] = []): unknown[][] {
  const ts = [...times, ...extraTimes];
  return [
    ["getEnabledKey"],
    ["getAutonomousKey"],
    ["getUtilityKey"],
    ["getEnabledData"],
    ["getAutonomousData"],
    ["getUtilityData"],
    ["getRobotStateRanges"],
    ["getProgramStartTime"],
    ...ts.flatMap((t) => [
      ["getIsRedAlliance", t],
      ["getDriverStation", t]
    ]),
    ["getMatchInfo", NOW],
    ["getMatchInfo", NOW2],
    ["getMatchInfo", NaN]
  ];
}

// An empty log and a log with no recognized keys at all.
emit([], battery());
emit(
  [
    ["putNumber", "unrelated", 0, 1],
    ["putString", " DriverStation/Enabled", 0, "true"],
    ["putBoolean", "DriverStation/Enabled2", 0, true]
  ],
  battery()
);

// AdvantageKit post-2027: boolean enabled plus a RobotMode string covering
// autonomous, teleop, test, utility and disabled phases; a full match-info set
// on the nanosecond EpochTime key and a ProgramStartTime.
{
  const ops: unknown[][] = [
    ["putBoolean", "/DriverStation/Enabled", 0, false],
    ["putBoolean", "/DriverStation/Enabled", 10, true],
    ["putBoolean", "/DriverStation/Enabled", 40, false],
    ["putString", "/DriverStation/RobotMode", 5, "autonomous"],
    ["putString", "/DriverStation/RobotMode", 15, "TELEOP"],
    ["putString", "/DriverStation/RobotMode", 25, "Test"],
    ["putString", "/DriverStation/RobotMode", 30, "utility"],
    ["putString", "/DriverStation/RobotMode", 35, "disabled"],
    ["putNumber", "/DriverStation/AllianceStation", 0, 2],
    ["putNumber", "/DriverStation/AllianceStation", 20, 5],
    ["putString", "/DriverStation/EventName", 0, "Test Event"],
    ["putString", "/DriverStation/EventName", 12, "Renamed Event"],
    ["putNumber", "/DriverStation/MatchType", 0, 2],
    ["putNumber", "/DriverStation/MatchNumber", 0, 14],
    ["putNumber", "/SystemStats/EpochTime", 0, Date.UTC(2026, 2, 15) * 1e6],
    ["putNumber", "NT:/Robot/ProgramStartTime", 0, 1.7e18]
  ];
  emit(ops, battery());
}

// AdvantageKit pre-2027 via the NT:/AdvantageKit mirror: boolean Autonomous
// and Test, EpochTimeMicros and the /Timestamp /Metadata family fields.
{
  const ops: unknown[][] = [
    ["putBoolean", "NT:/AdvantageKit/DriverStation/Enabled", 1, true],
    ["putBoolean", "NT:/AdvantageKit/DriverStation/Enabled", 20, false],
    ["putBoolean", "NT:/AdvantageKit/DriverStation/Autonomous", 1, true],
    ["putBoolean", "NT:/AdvantageKit/DriverStation/Autonomous", 10, false],
    ["putBoolean", "NT:/AdvantageKit/DriverStation/Test", 30, true],
    ["putNumber", "NT:/AdvantageKit/DriverStation/AllianceStation", 0, 6],
    ["putString", "NT:/AdvantageKit/DriverStation/EventName", 0, "Pre-2027 Event"],
    ["putNumber", "NT:/AdvantageKit/DriverStation/MatchType", 0, 1],
    ["putNumber", "NT:/AdvantageKit/DriverStation/MatchNumber", 0, 3],
    ["putNumber", "NT:/AdvantageKit/SystemStats/EpochTimeMicros", 0, Date.UTC(2023, 5, 1) * 1e3],
    ["putNumber", "/Timestamp", 1, 42],
    ["putString", "/Metadata", 0, "{}"],
    ["putString", "/RealMetadata", 0, "{}"],
    ["putString", "/ReplayMetadata", 0, "{}"]
  ];
  emit(ops, battery());
}

// NT pre-2027 FMSControlData bit fields: enabled is word % 2 === 1, auto bit 1,
// utility bit 2. Negative, fractional, NaN and >2^32 words exercise JS's
// ToInt32 coercion and remainder semantics.
{
  const ops: unknown[][] = [
    ["putNumber", "NT:/FMSInfo/FMSControlData", 0, 0],
    ["putNumber", "NT:/FMSInfo/FMSControlData", 5, 1],
    ["putNumber", "NT:/FMSInfo/FMSControlData", 10, 3],
    ["putNumber", "NT:/FMSInfo/FMSControlData", 15, 5],
    ["putNumber", "NT:/FMSInfo/FMSControlData", 20, 7],
    ["putNumber", "NT:/FMSInfo/FMSControlData", 25, -1],
    ["putNumber", "NT:/FMSInfo/FMSControlData", 30, 3.5],
    ["putNumber", "NT:/FMSInfo/FMSControlData", 35, 2 ** 32 + 6],
    ["putNumber", "NT:/FMSInfo/FMSControlData", 40, NaN],
    ["putBoolean", "NT:/FMSInfo/IsRedAlliance", 0, true],
    ["putBoolean", "NT:/FMSInfo/IsRedAlliance", 18, false],
    ["putNumber", "NT:/FMSInfo/StationNumber", 0, 1],
    ["putNumber", "NT:/FMSInfo/StationNumber", 18, 2],
    ["putString", "NT:/FMSInfo/EventName", 0, "FMS Event"],
    ["putNumber", "NT:/FMSInfo/MatchType", 0, 3],
    ["putNumber", "NT:/FMSInfo/MatchNumber", 0, 8],
    ["putNumber", "systemTime", 0, Date.UTC(2025, 7, 4) * 1e6]
  ];
  emit(ops, battery());
}

// NT post-2027 control-word keys (boolean enabled, RobotMode string) with the
// matching DriverStation match-info keys.
{
  const ops: unknown[][] = [
    ["putBoolean", "NT:/DriverStation/ControlWord/enabled", 2, false],
    ["putBoolean", "NT:/DriverStation/ControlWord/enabled", 8, true],
    ["putString", "NT:/DriverStation/ControlWord/robotMode", 2, "teleop"],
    ["putString", "NT:/DriverStation/ControlWord/robotMode", 12, "autonomous"],
    ["putBoolean", "NT:/DriverStation/IsRedAlliance", 2, false],
    ["putNumber", "NT:/DriverStation/StationNumber", 2, 3],
    ["putString", "NT:/DriverStation/EventName", 2, "NT Event"],
    ["putNumber", "NT:/DriverStation/MatchType", 2, 2],
    ["putNumber", "NT:/DriverStation/MatchNumber", 2, 51]
  ];
  emit(ops, battery());
}

// DataLog/Phoenix DS:controlWord keys and plain DS: pre-2027 keys.
{
  emit(
    [
      ["putBoolean", "DS:controlWord/enabled", 0, true],
      ["putBoolean", "DS:controlWord/enabled", 12, false],
      ["putString", "DS:controlWord/robotMode", 0, "autonomous"],
      ["putString", "DS:controlWord/robotMode", 6, "teleop"]
    ],
    battery()
  );
  emit(
    [
      ["putBoolean", "DS:enabled", 0, true],
      ["putBoolean", "DS:autonomous", 0, false],
      ["putBoolean", "DS:autonomous", 9, true],
      ["putBoolean", "DS:test", 0, true],
      ["putBoolean", "DS:test", 4, false],
      ["putString", "DS:allianceStation", 0, "Blue 2"],
      ["putString", "DS:matchInfo/eventName", 0, "Phoenix Event"],
      ["putNumber", "DS:matchInfo/matchType", 0, 3],
      ["putNumber", "DS:matchInfo/matchNumber", 0, 1],
      ["putNumber", "systemTime", 0, Date.UTC(2024, 1, 29) * 1e6]
    ],
    battery()
  );
}

// FIRST DS keys: ControlWord booleans/strings, the MatchInfo group and the
// ControlData raw field, which supplies the program start time.
{
  const ops: unknown[][] = [
    ["putRaw", "DS:/Dscomm/Control/Robot/ControlData", 3, new Uint8Array([1, 2, 3])],
    ["putBoolean", "DS:/Dscomm/Control/Robot/ControlData/ControlWord/enabled", 3, true],
    ["putBoolean", "DS:/Dscomm/Control/Robot/ControlData/ControlWord/enabled", 33, false],
    ["putString", "DS:/Dscomm/Control/Robot/ControlData/ControlWord/robotMode", 3, "autonomous"],
    ["putString", "DS:/Dscomm/Control/Robot/ControlData/ControlWord/robotMode", 13, "test"],
    ["putString", "DS:/Dscomm/Control/Robot/ControlData/ControlWord/allianceStation", 3, "Red 3"],
    ["putString", "DS:/Dscomm/Control/Robot/MatchInfo/EventName", 3, "FIRST Event"],
    ["putNumber", "DS:/Dscomm/Control/Robot/MatchInfo/MatchType", 3, 2],
    ["putNumber", "DS:/Dscomm/Control/Robot/MatchInfo/MatchNumber", 3, 77]
  ];
  emit(ops, battery());
}

// NI DSLog keys: DSDisabled inverts to enabled and DSTeleop inverts to
// autonomous.
{
  const ops: unknown[][] = [
    ["putBoolean", "/DSLog/Status/DSDisabled", 0, true],
    ["putBoolean", "/DSLog/Status/DSDisabled", 7, false],
    ["putBoolean", "/DSLog/Status/DSDisabled", 30, true],
    ["putBoolean", "/DSLog/Status/DSTeleop", 7, true],
    ["putBoolean", "/DSLog/Status/DSTeleop", 17, false]
  ];
  emit(ops, battery());
}

// Phoenix bare keys and the RoadRunner RUNNING flag.
{
  emit(
    [
      ["putBoolean", "RobotEnable", 0, false],
      ["putBoolean", "RobotEnable", 11, true],
      ["putString", "RobotMode", 0, "disabled"],
      ["putString", "RobotMode", 11, "teleop"],
      ["putString", "AllianceStation", 0, "Red 1"],
      ["putString", "AllianceStation", 21, "Blue 3"]
    ],
    battery()
  );
  emit([["putBoolean", "RUNNING", 2, true], ["putBoolean", "RUNNING", 22, false]], battery());
}

// Key priority and merge prefixes: findKey picks the lowest search index, not
// the first field; /LogN prefixes strip before matching.
{
  emit(
    [
      ["putBoolean", "RUNNING", 0, false],
      ["putBoolean", "DS:enabled", 0, true],
      ["putBoolean", "DS:autonomous", 0, true]
    ],
    battery()
  );
  emit(
    [
      ["putBoolean", "/DriverStation/Enabled", 0, false],
      ["putBoolean", "/DriverStation/Enabled", 5, true],
      ["putBoolean", "DS:enabled", 0, false],
      ["putBoolean", "RUNNING", 0, true]
    ],
    battery()
  );
  emit(
    [
      ["putBoolean", "/Log1/DriverStation/Enabled", 0, true],
      ["putBoolean", "/Log1/DriverStation/Enabled", 10, false],
      ["putBoolean", "/Log1/DriverStation/RobotMode", 0, true],
      ["putBoolean", "Log2/DS:enabled", 1, true],
      ["putBoolean", "Log2/DS:autonomous", 1, false],
      ["putBoolean", "/DriverStation/Enabled", 20, true]
    ],
    battery()
  );
  emit(
    [
      ["putNumber", "/Log1/Robot/ProgramStartTime", 2, 5e17],
      ["putNumber", "/Log9/DriverStation/AllianceStation", 0, 4]
    ],
    battery()
  );
}

// Wrongly typed fields: an enabled key holding numbers yields no enabled data
// and no ranges; a defined autonomous key with unusable data empties the
// ranges; a RobotMode boolean satisfies neither reader.
{
  emit(
    [
      ["putNumber", "/DriverStation/Enabled", 0, 1],
      ["putBoolean", "DS:enabled", 0, true],
      ["putNumber", "/DriverStation/Autonomous", 0, 1]
    ],
    battery()
  );
  emit(
    [
      ["putBoolean", "/DriverStation/Enabled", 0, true],
      ["putBoolean", "/DriverStation/RobotMode", 0, true],
      ["putString", "/DriverStation/Autonomous", 0, "autonomous"]
    ],
    battery()
  );
  emit(
    [
      ["putBoolean", "/DriverStation/Enabled", 0, true],
      ["putNumber", "DS:autonomous", 0, 1]
    ],
    battery()
  );
}

// Blank fields and empty series: a defined enabled key with no samples, a
// ProgramStartTime with no samples, and an autonomous field whose only sample
// is cleared before the query's union is computed.
{
  emit(
    [
      ["createBlankField", "/DriverStation/Enabled", 1],
      ["createBlankField", "NT:/Robot/ProgramStartTime", 2],
      ["createBlankField", "/DriverStation/AllianceStation", 2],
      ["putBoolean", "some/other", 0, true]
    ],
    battery()
  );
  emit(
    [
      ["putBoolean", "/DriverStation/Enabled", 0, true],
      ["putBoolean", "/DriverStation/Enabled", 10, false],
      ["createBlankField", "/DriverStation/Autonomous", 1]
    ],
    battery()
  );
}

// Enabled series that never turns true: getMatchInfo returns null even with a
// complete match-info set.
{
  const ops: unknown[][] = [
    ["putBoolean", "/DriverStation/Enabled", 0, false],
    ["putBoolean", "/DriverStation/Enabled", 10, false],
    ["putString", "/DriverStation/EventName", 0, "Never Enabled"],
    ["putNumber", "/DriverStation/MatchType", 0, 2],
    ["putNumber", "/DriverStation/MatchNumber", 0, 9]
  ];
  emit(ops, battery());
}

// Match-info edges: invalid match types, missing keys, wrong-typed fields,
// missing system time, and years below 2022 or invalid.
{
  const base: unknown[][] = [
    ["putBoolean", "/DriverStation/Enabled", 0, true],
    ["putString", "/DriverStation/EventName", 0, "Event"],
    ["putNumber", "/DriverStation/MatchNumber", 0, 5]
  ];
  for (const matchType of [0, 4, -1, 2.5, NaN]) {
    emit([...base, ["putNumber", "/DriverStation/MatchType", 0, matchType]], battery());
  }
  emit([...base], battery()); // no MatchType key at all
  emit(
    [
      ["putBoolean", "/DriverStation/Enabled", 0, true],
      ["putNumber", "/DriverStation/MatchType", 0, 2],
      ["putNumber", "/DriverStation/MatchNumber", 0, 5]
    ],
    battery()
  ); // no EventName
  emit(
    [
      ["putBoolean", "/DriverStation/Enabled", 0, true],
      ["putString", "/DriverStation/EventName", 0, "Event"],
      ["putNumber", "/DriverStation/MatchType", 0, 2]
    ],
    battery()
  ); // no MatchNumber
  emit(
    [
      ["putBoolean", "/DriverStation/Enabled", 0, true],
      ["putBoolean", "/DriverStation/EventName", 0, true],
      ["putString", "/DriverStation/MatchType", 0, "qual"],
      ["putString", "/DriverStation/MatchNumber", 0, "five"]
    ],
    battery()
  ); // every match-info field wrongly typed
  emit(
    [
      ["putBoolean", "/DriverStation/Enabled", 0, true],
      ["putString", "/DriverStation/EventName", 0, "Old Event"],
      ["putNumber", "/DriverStation/MatchType", 0, 1],
      ["putNumber", "/DriverStation/MatchNumber", 0, 2],
      ["putNumber", "/SystemStats/EpochTimeMicros", 0, Date.UTC(2019, 3, 3) * 1e3]
    ],
    battery()
  ); // year 2019 < 2022 -> current year
  emit(
    [
      ["putBoolean", "/DriverStation/Enabled", 0, true],
      ["putString", "/DriverStation/EventName", 0, "NaN Event"],
      ["putNumber", "/DriverStation/MatchType", 0, 2],
      ["putNumber", "/DriverStation/MatchNumber", 0, 2],
      ["putNumber", "systemTime", 0, NaN]
    ],
    battery()
  ); // NaN epoch -> NaN year
  emit(
    [
      ["putBoolean", "/DriverStation/Enabled", 0, true],
      ["putString", "/DriverStation/EventName", 0, "Far Event"],
      ["putNumber", "/DriverStation/MatchType", 0, 2],
      ["putNumber", "/DriverStation/MatchNumber", 0, 2],
      ["putNumber", "systemTime", 0, 1e30]
    ],
    battery()
  ); // invalid Date -> NaN year
  // Samples logged only after the first enable read the defaults.
  emit(
    [
      ["putBoolean", "/DriverStation/Enabled", 0, true],
      ["putString", "/DriverStation/EventName", 30, "Late Event"],
      ["putNumber", "/DriverStation/MatchType", 30, 2],
      ["putNumber", "/DriverStation/MatchNumber", 30, 5],
      ["putNumber", "systemTime", 30, Date.UTC(2027, 0, 1) * 1e6]
    ],
    battery()
  );
}

// Alliance/station edges: out-of-range AdvantageKit numbers, strings that are
// not "Red *", unknown Phoenix strings and a wrongly typed IsRedAlliance.
{
  for (const value of [0, 1, 3, 3.5, 4, 7, -1, NaN, 2 ** 53]) {
    emit(
      [
        ["putBoolean", "/DriverStation/Enabled", 0, true],
        ["putNumber", "/DriverStation/AllianceStation", 0, value]
      ],
      battery()
    );
  }
  for (const value of ["Red 1", "Red", "red 1", "Blue 1", "Blue 2", "Blue 3", "Red 2", "Red 3", "Green 9", "", "Reddit"]) {
    emit(
      [
        ["putBoolean", "/DriverStation/Enabled", 0, true],
        ["putString", "DS:allianceStation", 0, value]
      ],
      battery()
    );
  }
  emit(
    [
      ["putBoolean", "/DriverStation/Enabled", 0, true],
      ["putString", "NT:/DriverStation/IsRedAlliance", 0, "yes"],
      ["putNumber", "NT:/DriverStation/StationNumber", 0, 2]
    ],
    battery()
  );
  // StationNumber with the red flag over time; values outside 1-3 fall to -1.
  for (const station of [1, 2, 3, 0, 4, NaN]) {
    emit(
      [
        ["putBoolean", "/DriverStation/Enabled", 0, true],
        ["putBoolean", "NT:/DriverStation/IsRedAlliance", 0, true],
        ["putBoolean", "NT:/DriverStation/IsRedAlliance", 20, false],
        ["putNumber", "NT:/DriverStation/StationNumber", 0, station]
      ],
      battery()
    );
  }
}

// Program-start variants: the DS ControlData field has any type (its first
// timestamp wins), a wrongly typed ProgramStartTime falls to getRange, and
// both plain ProgramStartTime keys divide by 1e9.
{
  emit([["putString", "NT:/Robot/ProgramStartTime", 4, "oops"]], battery());
  emit([["putNumber", "/Robot/ProgramStartTime", 4, 6.5e17]], battery());
  emit([["putBoolean", "DS:/Dscomm/Control/Robot/ControlData", 4, true]], battery());
  emit(
    [
      ["putBoolean", "DS:/Dscomm/Control/Robot/ControlData", 4, true],
      ["putBoolean", "DS:/Dscomm/Control/Robot/ControlData", 9, false],
      ["putNumber", "NT:/Robot/ProgramStartTime", 1, 7e17]
    ],
    battery()
  ); // ProgramStartTime wins over ControlData
}

// A state timeline with a disabled head, auto, teleop, utility and a trailing
// disabled segment; and one where enabled starts inside auto.
{
  emit(
    [
      ["putBoolean", "/DriverStation/Enabled", 5, false],
      ["putBoolean", "/DriverStation/Enabled", 10, true],
      ["putBoolean", "/DriverStation/Enabled", 45, false],
      ["putBoolean", "/DriverStation/Autonomous", 0, false],
      ["putBoolean", "/DriverStation/Autonomous", 10, true],
      ["putBoolean", "/DriverStation/Autonomous", 25, false],
      ["putBoolean", "/DriverStation/Test", 35, true],
      ["putBoolean", "/DriverStation/Test", 40, false]
    ],
    battery()
  );
  emit(
    [
      ["putBoolean", "/DriverStation/Autonomous", 0, true],
      ["putBoolean", "/DriverStation/Enabled", 12, true],
      ["putBoolean", "/DriverStation/Autonomous", 14, false]
    ],
    battery()
  );
}

// Priority ladders: for each key list, the keys from rank k down are all
// present, written lowest priority first so field order never decides, each
// with its own reader's type and distinguishable data.
{
  const LISTS: [string, string[]][] = [
    ["enabled", robotState.ENABLED_KEYS],
    ["autonomous", robotState.AUTONOMOUS_KEYS],
    ["utility", robotState.UTILITY_KEYS],
    ["alliance", robotState.ALLIANCE_KEYS],
    ["station", robotState.DRIVER_STATION_KEYS],
    ["systemTime", robotState.SYSTEM_TIME_KEYS],
    ["event", robotState.EVENT_KEYS],
    ["matchType", robotState.MATCH_TYPE_KEYS],
    ["matchNumber", robotState.MATCH_NUMBER_KEYS],
    ["programStart", robotState.PROGRAM_START_TIME_KEYS]
  ];
  const STATIONS = ["Red 1", "Blue 3", "Red 2", "Blue 1", "Red 3", "Blue 2", "Red 1"];
  const writeKey = (ops: unknown[][], list: string, key: string, rank: number) => {
    const t = rank + 1;
    if (key.endsWith("FMSControlData")) {
      ops.push(["putNumber", key, t, [1, 3, 5, 7, 0][rank % 5]], ["putNumber", key, 40 + t, 0]);
    } else if (list === "enabled" || ((list === "autonomous" || list === "utility") && !key.toLowerCase().endsWith("robotmode"))) {
      ops.push(["putBoolean", key, t, true], ["putBoolean", key, 40 + t, false]);
    } else if (list === "autonomous" || list === "utility") {
      ops.push(["putString", key, t, ["autonomous", "test", "utility", "teleop"][rank % 4]], ["putString", key, 40 + t, "disabled"]);
    } else if (key.endsWith("DriverStation/AllianceStation") || key.endsWith("StationNumber")) {
      ops.push(["putNumber", key, 0, (rank % 6) + 1]);
    } else if (key.endsWith("IsRedAlliance")) {
      ops.push(["putBoolean", key, 0, rank % 2 === 0]);
    } else if (list === "alliance" || list === "station") {
      ops.push(["putString", key, 0, STATIONS[rank]]);
    } else if (list === "systemTime") {
      const ms = Date.UTC(2022 + rank, rank, 1);
      ops.push(["putNumber", key, 0, key.endsWith("Micros") ? ms * 1e3 : ms * 1e6]);
    } else if (list === "event") {
      ops.push(["putString", key, 0, `Event ${rank}`]);
    } else if (list === "matchType") {
      ops.push(["putNumber", key, 0, (rank % 3) + 1]);
    } else if (list === "matchNumber") {
      ops.push(["putNumber", key, 0, rank + 10]);
    } else if (key.endsWith("ProgramStartTime")) {
      ops.push(["putNumber", key, t, (rank + 1) * 1e18]);
    } else {
      ops.push(["putRaw", key, t, new Uint8Array([rank])]);
    }
  };
  const MATCH_BASE: { [list: string]: unknown[][] } = {
    systemTime: [["putString", "DS:matchInfo/eventName", 0, "Base"], ["putNumber", "DS:matchInfo/matchType", 0, 2], ["putNumber", "DS:matchInfo/matchNumber", 0, 4]],
    event: [["putNumber", "DS:matchInfo/matchType", 0, 2], ["putNumber", "DS:matchInfo/matchNumber", 0, 4]],
    matchType: [["putString", "DS:matchInfo/eventName", 0, "Base"], ["putNumber", "DS:matchInfo/matchNumber", 0, 4]],
    matchNumber: [["putString", "DS:matchInfo/eventName", 0, "Base"], ["putNumber", "DS:matchInfo/matchType", 0, 2]]
  };
  for (const [list, keys] of LISTS) {
    for (let k = 0; k < keys.length; k++) {
      const ops: unknown[][] = [];
      const base = MATCH_BASE[list];
      if (base !== undefined) ops.push(["putBoolean", "RUNNING", 0, true], ...base);
      for (let rank = keys.length - 1; rank >= k; rank--) writeKey(ops, list, keys[rank], rank);
      emit(ops, battery());
    }
  }
}

// findKey edges: a leading "/" is stripped once before matching, the merge
// prefix needs no separator after its digits, ties at one rank go to the first
// field, and FMSControlData of the wrong type or with no samples.
{
  for (const key of ["/RUNNING", "//DriverStation/Enabled", "/DS:enabled", "/Log3RUNNING", "Log4//RobotEnable", "/NT:/FMSInfo/FMSControlData"]) {
    emit(
      [
        [key.endsWith("FMSControlData") ? "putNumber" : "putBoolean", key, 2, key.endsWith("FMSControlData") ? 3 : true],
        ["putString", "/RobotMode", 2, "autonomous"],
        ["putString", "/AllianceStation", 0, "Red 2"],
        ["putNumber", "/systemTime", 0, Date.UTC(2024, 0, 1) * 1e6],
        ["putString", "/DS:matchInfo/eventName", 0, "Slash"],
        ["putNumber", "/DS:matchInfo/matchType", 0, 3],
        ["putNumber", "/DS:matchInfo/matchNumber", 0, 6]
      ],
      battery()
    );
  }
  emit(
    [
      ["putBoolean", "/Log1/DriverStation/Enabled", 0, true],
      ["putBoolean", "/Log0/DriverStation/Enabled", 0, false],
      ["putBoolean", "/Log0/DriverStation/Enabled", 7, true],
      ["putNumber", "/Log0/DriverStation/AllianceStation", 0, 5],
      ["putNumber", "/Log1/DriverStation/AllianceStation", 0, 2]
    ],
    battery()
  );
  emit(
    [
      ["putBoolean", "/DriverStation/Enabled", 0, false],
      ["putBoolean", "/Log0/DriverStation/Enabled", 0, true]
    ],
    battery()
  );
  emit([["putBoolean", "NT:/FMSInfo/FMSControlData", 0, true]], battery());
  emit([["createBlankField", "NT:/FMSInfo/FMSControlData", 1]], battery());
  emit(
    [
      ["createBlankField", "NT:/FMSInfo/FMSControlData", 1],
      ["putBoolean", "RUNNING", 0, true]
    ],
    battery()
  );
}

// Year boundaries: 2022 is kept and 2021 resets to the clock's year; the last
// valid Date (8.64e15 ms) keeps its year, one step past it is NaN.
{
  const epochs = [
    Date.UTC(2022, 0, 1) * 1e6,
    (Date.UTC(2022, 0, 1) - 1) * 1e6,
    8.64e15 * 1e6,
    8.6400000000001e15 * 1e6,
    -8.64e15 * 1e6,
    -0.5e6,
    -Infinity
  ];
  for (const epoch of epochs) {
    emit(
      [
        ["putBoolean", "/DriverStation/Enabled", 0, true],
        ["putString", "/DriverStation/EventName", 0, "Year"],
        ["putNumber", "/DriverStation/MatchType", 0, 1],
        ["putNumber", "/DriverStation/MatchNumber", 0, 1],
        ["putNumber", "/SystemStats/EpochTime", 0, epoch]
      ],
      [["getMatchInfo", NOW], ["getMatchInfo", NOW2], ["getMatchInfo", NaN]]
    );
  }
}

// getElimMatchString over every playoff type, unknown type values and match
// numbers in and out of each bracket's range. Empty ops; the log is unused.
{
  const calls: unknown[][] = [];
  for (const type of [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, -0, -1, 12, 99, 1.5, NaN, Infinity]) {
    for (const n of [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 30, 36, 37, 42, 43, 47, 48, 49, 60, -0, -1, 1.5, 2 ** 53, 1e21, NaN, Infinity, -Infinity]) {
      calls.push(["getElimMatchString", type, n]);
    }
  }
  emit([], calls);
}

// Seeded fixtures: a random key per role with a random type and a random
// sample series, plus scattered match-info fields and merge prefixes. The
// RobotState exports are total, so anything goes.
{
  const r = new Rng(seed, "robotState");
  const ENABLED = [
    "/DriverStation/Enabled",
    "NT:/AdvantageKit/DriverStation/Enabled",
    "DS:controlWord/enabled",
    "DS:enabled",
    "DS:/Dscomm/Control/Robot/ControlData/ControlWord/enabled",
    "/DSLog/Status/DSDisabled",
    "RobotEnable",
    "NT:/DriverStation/ControlWord/enabled",
    "NT:/FMSInfo/FMSControlData",
    "RUNNING"
  ];
  const MODE = [
    "/DriverStation/RobotMode",
    "NT:/AdvantageKit/DriverStation/RobotMode",
    "/DriverStation/Autonomous",
    "DS:controlWord/robotMode",
    "DS:autonomous",
    "DS:/Dscomm/Control/Robot/ControlData/ControlWord/robotMode",
    "/DSLog/Status/DSTeleop",
    "RobotMode",
    "DS:test",
    "NT:/FMSInfo/FMSControlData"
  ];
  const ALLIANCE = [
    "/DriverStation/AllianceStation",
    "DS:/Dscomm/Control/Robot/ControlData/ControlWord/allianceStation",
    "NT:/DriverStation/IsRedAlliance",
    "NT:/FMSInfo/IsRedAlliance",
    "DS:allianceStation",
    "AllianceStation",
    "NT:/DriverStation/StationNumber",
    "NT:/FMSInfo/StationNumber"
  ];
  const INFO: [string, string][] = [
    ["/DriverStation/EventName", "string"],
    ["NT:/AdvantageKit/DriverStation/EventName", "string"],
    ["DS:/Dscomm/Control/Robot/MatchInfo/EventName", "string"],
    ["NT:/FMSInfo/EventName", "string"],
    ["NT:/Netcomm/Control/MatchInfo/EventName", "string"],
    ["DS:matchInfo/eventName", "string"],
    ["/DriverStation/MatchType", "number"],
    ["NT:/AdvantageKit/DriverStation/MatchType", "number"],
    ["DS:/Dscomm/Control/Robot/MatchInfo/MatchType", "number"],
    ["NT:/Netcomm/Control/MatchInfo/MatchType", "number"],
    ["DS:matchInfo/matchType", "number"],
    ["/DriverStation/MatchNumber", "number"],
    ["NT:/FMSInfo/MatchNumber", "number"],
    ["DS:matchInfo/matchNumber", "number"],
    ["/SystemStats/EpochTime", "number"],
    ["/SystemStats/EpochTimeMicros", "number"],
    ["systemTime", "number"],
    ["NT:/Robot/ProgramStartTime", "number"],
    ["/Robot/ProgramStartTime", "number"],
    ["DS:/Dscomm/Control/Robot/ControlData", "raw"]
  ];
  const MODE_TEXT = ["autonomous", "teleop", "test", "utility", "disabled", "AUTONOMOUS", "other", ""];
  const STATION_TEXT = ["Red 1", "Red 2", "Red 3", "Blue 1", "Blue 2", "Blue 3", "Red", "red 1", "x"];
  const prefix = () => r.pick(["", "/Log0", "/Log1", "Log2", "/Log10"]);

  for (let i = 0; i < count; i++) {
    const ops: unknown[][] = [];
    const writes = (key: string, kind: string) => {
      const n = r.range(0, 4);
      let t = r.range(0, 10);
      for (let j = 0; j < n; j++) {
        t += r.range(0, 20);
        switch (kind) {
          case "boolean":
            ops.push(["putBoolean", key, t, r.bool()]);
            break;
          case "number":
            ops.push(["putNumber", key, t, r.pick([0, 1, 2, 3, 4, 5, 6, 7, -1, 3.5, NaN, r.double()])]);
            break;
          case "string":
            ops.push(["putString", key, t, r.pick([...MODE_TEXT, ...STATION_TEXT, r.ascii(8)])]);
            break;
          case "raw":
            ops.push(["putRaw", key, t, r.bytes(4)]);
            break;
          default:
            ops.push(["createBlankField", key, r.range(0, 7)]);
            break;
        }
      }
      if (n === 0) ops.push(["createBlankField", key, r.range(0, 7)]);
    };
    if (r.below(5) > 0) writes(prefix() + r.pick(ENABLED), r.pick(["boolean", "number", "string", "blank"]));
    if (r.below(3) > 0) writes(prefix() + r.pick(MODE), r.pick(["boolean", "number", "string", "blank"]));
    if (r.below(4) > 0) writes(prefix() + r.pick(MODE), r.pick(["boolean", "number", "string", "blank"]));
    if (r.below(4) > 0) writes(prefix() + r.pick(ALLIANCE), r.pick(["boolean", "number", "string", "blank"]));
    for (const [key, kind] of INFO) {
      if (r.below(6) === 0) writes(prefix() + key, r.pick([kind, "boolean", "blank"]));
    }
    const ts = [r.range(-20, 0), r.range(0, 60), r.range(20, 100), r.pick([NaN, Infinity])];
    emit(ops, battery(ts), r.below(4) !== 0);
  }
}

for (const [cache, ops, calls] of cases) {
  const args = [toTree(cache), toTree(ops), toTree(calls)];
  const result = run(cache, ops as any[][], calls as any[][]);
  process.stdout.write(JSON.stringify({ fn: "oracle:robotStateRun", args, ok: toTree(result) }) + "\n");
}
