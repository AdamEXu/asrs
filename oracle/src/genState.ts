import "./shims";
import { toTree } from "./cve1";
import { run } from "./lazyHelpers/stateRun";

// Emits `oracle:stateRun` ground truth for asrs_log::joystick_state and
// asrs_log::mechanism_state:
//   genState joystick|mechanism
//     {"fn":"oracle:stateRun","args":[<cache>,[<op>...],[<call>...]],"ok":[<result>...]}

const mode = process.argv.slice(2)[0];

const cases: [boolean, unknown[][], unknown[][]][] = [];
const emit = (ops: unknown[][], calls: unknown[][], cache = true) => cases.push([cache, ops, calls]);

const AKIT = "/DriverStation/Joystick";
const NT = "NT:/AdvantageKit/DriverStation/Joystick";
const DSLOG = "DS:joystick";
const FIRSTDS = "DS:/Dscomm/Control/Robot/ControlData/Joysticks";

function joystickCases() {
  // AdvantageKit SDL layout, two writes so reads before the first sample
  // fall back to defaults. "Joystick12" exercises the startsWith quirk where
  // id 1 claims the "Joystick1" table prefix out of the "Joystick12" field.
  {
    const ops: unknown[][] = [
      ["putNumber", `${AKIT}0/ButtonsAvailable`, 10, 0b101101],
      ["putNumber", `${AKIT}0/ButtonValues`, 10, 0b100101],
      ["putNumber", `${AKIT}0/AxesAvailable`, 10, 0b110],
      ["putNumberArray", `${AKIT}0/AxisValues`, 10, [0.5, -0.25, 0.75, 1.5]],
      ["putNumber", `${AKIT}0/POVsAvailable`, 10, 0b101],
      ["putNumberArray", `${AKIT}0/POVValues`, 10, [1, 5, 12, -1]],
      ["putNumber", `${AKIT}0/TouchpadCount`, 10, 2],
      ["putNumber", `${AKIT}0/Touchpad/0/FingerCount`, 10, 2],
      ["putBoolean", `${AKIT}0/Touchpad/0/Finger/0/Down`, 10, true],
      ["putNumber", `${AKIT}0/Touchpad/0/Finger/0/X`, 10, 0.25],
      ["putNumber", `${AKIT}0/Touchpad/0/Finger/0/Y`, 10, -0.5],
      ["putBoolean", `${AKIT}0/Touchpad/0/Finger/1/Down`, 10, false],
      ["putNumber", `${AKIT}0/Touchpad/0/Finger/1/X`, 10, 1e300],
      ["putNumber", `${AKIT}0/Touchpad/0/Finger/1/Y`, 10, NaN],
      ["putNumber", `${AKIT}0/Touchpad/1/FingerCount`, 10, 1],
      ["putBoolean", `${AKIT}0/Touchpad/1/Finger/0/Down`, 10, true],
      ["putNumber", `${AKIT}0/Touchpad/1/Finger/0/X`, 10, -0],
      ["putNumber", `${AKIT}0/Touchpad/1/Finger/0/Y`, 10, 5e-324],
      ["putNumber", `${AKIT}12/ButtonsAvailable`, 10, 0xff],
      ["putNumber", `${AKIT}12/ButtonValues`, 10, 0xa5],
      ["putNumber", `${AKIT}0/ButtonsAvailable`, 20, 0b10],
      ["putNumber", `${AKIT}0/ButtonValues`, 20, 0b10]
    ];
    emit(ops, [
      ["getJoystickState", -1, 10],
      ["getJoystickState", 0, 5],
      ["getJoystickState", 0, 10],
      ["getJoystickState", 0, 15],
      ["getJoystickState", 0, 25],
      ["getJoystickState", 1, 10],
      ["getJoystickState", 2, 10],
      ["getJoystickState", 5, 10],
      ["getJoystickState", 6, 10],
      ["getJoystickState", 2.5, 10],
      ["getJoystickState", NaN, 10],
      ["getJoystickState", Infinity, 10],
      ["getJoystickState", -Infinity, 10],
      ["getJoystickState", -0, 10],
      ["getJoystickState", 12, 10]
    ]);
  }

  // AdvantageKit NI layout: no ButtonsAvailable, ButtonCount/ButtonValues
  // bit test through the signed 32-bit `1 << i` and raw axis/POV arrays.
  {
    const ops: unknown[][] = [
      ["putNumber", `${AKIT}3/ButtonCount`, 10, 6],
      ["putNumber", `${AKIT}3/ButtonValues`, 10, 0b110110],
      ["putNumberArray", `${AKIT}3/AxisValues`, 10, [-1, 0, 0.5]],
      ["putNumberArray", `${AKIT}3/POVs`, 10, [0, 45.5, -1]]
    ];
    emit(ops, [
      ["getJoystickState", 3, 5],
      ["getJoystickState", 3, 10],
      ["getJoystickState", 4, 10]
    ]);
  }

  // NI button counts and button-value bit patterns at the int32 edges: bit
  // positions past 31 wrap through `1 << i`, negative and fractional counts
  // bound the loop, and the value coerces with ToInt32.
  for (const [count, values] of [
    [40, -1],
    [40, 0x80000000],
    [40, 0x7fffffff],
    [40, 0xffff_ffff_ffff],
    [2.5, 0b1111],
    [-3, 0b1111],
    [NaN, 0b1111],
    [0, 0b1111],
    [5, NaN],
    [5, 1e300],
    [5, -0.0],
    [3, 4294967296]
  ] as [number, number][]) {
    emit(
      [
        ["putNumber", `${AKIT}2/ButtonCount`, 10, count],
        ["putNumber", `${AKIT}2/ButtonValues`, 10, values]
      ],
      [["getJoystickState", 2, 10]]
    );
  }

  // SDL bit packing: BigInt availability/value pairs, missing halves, wrong
  // types and the non-integer RangeErrors.
  for (const [available, values] of [
    [-1, -1],
    [0xffffffffffff, 0x555555555555],
    [1e30, 1],
    [9007199254740991, -1],
    [1, 2.5],
    [2.5, 1],
    [NaN, 1],
    [1, NaN],
    [Infinity, 1],
    [-0.5, 1],
    [1, 1e300]
  ] as [number, number][]) {
    emit(
      [
        ["putNumber", `${AKIT}0/ButtonsAvailable`, 10, available],
        ["putNumber", `${AKIT}0/ButtonValues`, 10, values]
      ],
      [["getJoystickState", 0, 10]]
    );
  }

  // A field that exists with a non-number type still forces SDL mapping but
  // reads as absent; a missing ButtonValues leaves the set bits false.
  emit(
    [
      ["putBoolean", `${AKIT}0/ButtonsAvailable`, 10, true],
      ["putNumber", `${AKIT}0/ButtonValues`, 10, 0b111]
    ],
    [["getJoystickState", 0, 10]]
  );
  emit(
    [
      ["putNumberArray", `${AKIT}0/ButtonsAvailable`, 10, [1, 2]],
      ["putNumber", `${AKIT}0/ButtonValues`, 10, 0b111]
    ],
    [["getJoystickState", 0, 10]]
  );
  emit(
    [
      ["putNumber", `${AKIT}0/ButtonsAvailable`, 10, 0b1111],
      ["putNumber", `${AKIT}0/AxesAvailable`, 10, -1],
      ["putNumberArray", `${AKIT}0/AxisValues`, 10, [0.1, 0.2, 0.3]]
    ],
    [["getJoystickState", 0, 10]]
  );

  // Axes and POV availability masks at int32 edges, array lengths over the
  // 16/8 caps, and the full decodePovBitfield table.
  for (const [available, values] of [
    [0xffff, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19]],
    [0xffffffff, [1, 2, 3]],
    [0xffff_ffff_ffff, [1, 2]],
    [3.7, [1, 2, 3, 4]],
    [NaN, [1, 2]],
    [1e10, [1, 2, 3]],
    [-0, [1]]
  ] as [number, number[]][]) {
    emit(
      [
        ["putNumber", `${AKIT}0/ButtonsAvailable`, 10, 0],
        ["putNumber", `${AKIT}0/AxesAvailable`, 10, available],
        ["putNumberArray", `${AKIT}0/AxisValues`, 10, values]
      ],
      [["getJoystickState", 0, 10]]
    );
  }
  {
    const ops: unknown[][] = [
      ["putNumber", `${AKIT}0/ButtonsAvailable`, 10, 0],
      ["putNumber", `${AKIT}0/POVsAvailable`, 10, -1],
      ["putNumberArray", `${AKIT}0/POVValues`, 10, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]]
    ];
    emit(ops, [["getJoystickState", 0, 10]]);
    emit(
      [
        ["putNumber", `${AKIT}0/ButtonsAvailable`, 10, 0],
        ["putNumber", `${AKIT}0/POVsAvailable`, 10, 0b110],
        ["putNumberArray", `${AKIT}0/POVValues`, 10, [-1, 15, 16, NaN, 1e300, 4294967295, -8, 0x7fffffff]]
      ],
      [["getJoystickState", 0, 10]]
    );
  }
  for (const v of [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, -1, -8, 255, NaN, 1e300, 4294967295, 0x80000000]) {
    emit([], [["decodePovBitfield", v]]);
  }

  // Touchpad and finger counts, including clamping past 2.
  for (const [pads, fingers] of [
    [3, 3],
    [2, 1],
    [1, 2],
    [0, 1],
    [-1, 2],
    [2.7, 1.5],
    [NaN, 1]
  ] as [number, number][]) {
    const ops: unknown[][] = [
      ["putNumber", `${AKIT}0/ButtonsAvailable`, 10, 0],
      ["putNumber", `${AKIT}0/TouchpadCount`, 10, pads],
      ["putNumber", `${AKIT}0/Touchpad/0/FingerCount`, 10, fingers],
      ["putNumber", `${AKIT}0/Touchpad/1/FingerCount`, 10, fingers],
      ["putNumber", `${AKIT}0/Touchpad/2/FingerCount`, 10, fingers],
      ["putBoolean", `${AKIT}0/Touchpad/0/Finger/0/Down`, 10, true],
      ["putNumber", `${AKIT}0/Touchpad/0/Finger/0/X`, 10, 0.4],
      ["putNumber", `${AKIT}0/Touchpad/0/Finger/0/Y`, 10, 0.6]
    ];
    emit(ops, [["getJoystickState", 0, 10]]);
  }

  // NT layout and the merge-prefixed AdvantageKit table; a merge-prefixed
  // DataLog table never matches because the stripped key keeps its slash.
  {
    const ops: unknown[][] = [
      ["putNumber", `${NT}0/ButtonsAvailable`, 10, 0b11],
      ["putNumber", `${NT}0/ButtonValues`, 10, 0b10],
      ["putNumber", "/Log1/DriverStation/Joystick2/ButtonsAvailable", 10, 0b101],
      ["putNumber", "/Log1/DriverStation/Joystick2/ButtonValues", 10, 0b100],
      ["putBooleanArray", "/Log1/DS:joystick0/buttons", 10, [true, true]],
      ["putBooleanArray", "/Log3/DriverStation/Joystick4/ButtonsAvailable", 10, [true]],
      ["putBoolean", "Log1/DriverStation/Joystick5/ButtonsAvailable", 10, true]
    ];
    emit(ops, [
      ["getJoystickState", 0, 10],
      ["getJoystickState", 2, 10],
      ["getJoystickState", 4, 10],
      ["getJoystickState", 5, 10]
    ]);
  }

  // DataLog SDL and NI layouts: "DS:opMode" presence picks the mapping, the
  // button ids shift by one without it, and povs decode only under SDL.
  {
    const ops: unknown[][] = [
      ["putBoolean", "DS:opMode", 10, true],
      ["putBooleanArray", `${DSLOG}0/buttons`, 10, [true, false, true, true]],
      ["putNumberArray", `${DSLOG}0/axes`, 10, [0.1, -0.2]],
      ["putNumberArray", `${DSLOG}0/povs`, 10, [1, 6, 9, 12]],
      ["putBooleanArray", `${DSLOG}1/buttons`, 10, [false]],
      ["putNumberArray", `${DSLOG}1/povs`, 10, [-1, 0x80000000]]
    ];
    emit(ops, [
      ["getJoystickState", 0, 10],
      ["getJoystickState", 1, 10],
      ["getJoystickState", 2, 10]
    ]);
    emit(
      [
        ["putBooleanArray", `${DSLOG}0/buttons`, 10, [true, false, true]],
        ["putNumberArray", `${DSLOG}0/axes`, 10, [0.1]],
        ["putNumberArray", `${DSLOG}0/povs`, 10, [1, 6, 9, 12, 270]]
      ],
      [["getJoystickState", 0, 10]]
    );
    emit(
      [
        ["putBooleanArray", `${DSLOG}0/buttons`, 10, []],
        ["putBooleanArray", `${DSLOG}0/povs`, 10, [true, false]],
        ["putNumber", `${DSLOG}0/axes`, 10, 0.5]
      ],
      [["getJoystickState", 0, 10]]
    );
  }

  // FIRST DS layout: 64-bit button masks split high/low, signed axis
  // scaling, packed POV nibbles and touchpad scaling to /65535.
  {
    const ops: unknown[][] = [
      ["putNumber", `${FIRSTDS}/0/AvailableButtons/high`, 10, 0x80000001],
      ["putNumber", `${FIRSTDS}/0/AvailableButtons/low`, 10, 0xffffffff],
      ["putNumber", `${FIRSTDS}/0/Buttons/high`, 10, 0x80000000],
      ["putNumber", `${FIRSTDS}/0/Buttons/low`, 10, 0x55555555],
      ["putNumber", `${FIRSTDS}/0/AvailableAxes`, 10, 0b1111],
      ["putNumberArray", `${FIRSTDS}/0/Axes`, 10, [32768, -32768, 16384, -1, 65536]],
      ["putNumber", `${FIRSTDS}/0/POVCount`, 10, 4],
      ["putNumber", `${FIRSTDS}/0/POVs`, 10, 0x9421],
      ["putNumber", `${FIRSTDS}/0/Touchpads/length`, 10, 2],
      ["putNumber", `${FIRSTDS}/0/Touchpads/0/Fingers/length`, 10, 2],
      ["putBoolean", `${FIRSTDS}/0/Touchpads/0/Fingers/0/Down`, 10, true],
      ["putNumber", `${FIRSTDS}/0/Touchpads/0/Fingers/0/X`, 10, 65535],
      ["putNumber", `${FIRSTDS}/0/Touchpads/0/Fingers/0/Y`, 10, 0],
      ["putBoolean", `${FIRSTDS}/0/Touchpads/0/Fingers/1/Down`, 10, false],
      ["putNumber", `${FIRSTDS}/0/Touchpads/0/Fingers/1/X`, 10, 70000],
      ["putNumber", `${FIRSTDS}/0/Touchpads/0/Fingers/1/Y`, 10, -5],
      ["putNumber", `${FIRSTDS}/0/Touchpads/1/Fingers/length`, 10, 0]
    ];
    emit(ops, [["getJoystickState", 0, 10], ["getJoystickState", 1, 10]]);
  }
  for (const [high, low] of [
    [-1, -1],
    [4294967296, 1],
    [1e300, 1],
    [2.5, 1],
    [NaN, 1],
    [1, 2.5]
  ] as [number, number][]) {
    emit(
      [
        ["putNumber", `${FIRSTDS}/0/AvailableButtons/high`, 10, high],
        ["putNumber", `${FIRSTDS}/0/AvailableButtons/low`, 10, low],
        ["putNumber", `${FIRSTDS}/0/Buttons/high`, 10, 0],
        ["putNumber", `${FIRSTDS}/0/Buttons/low`, 10, 0]
      ],
      [["getJoystickState", 0, 10]]
    );
  }
  // Only one half of the pair present falls back to zero, and POV/pad edge
  // counts.
  emit(
    [
      ["putNumber", `${FIRSTDS}/0/AvailableButtons/high`, 10, 0xffffffff],
      ["putNumber", `${FIRSTDS}/0/Buttons/low`, 10, 0xffffffff],
      ["putNumber", `${FIRSTDS}/0/POVCount`, 10, 12],
      ["putNumber", `${FIRSTDS}/0/POVs`, 10, -1],
      ["putNumber", `${FIRSTDS}/0/Touchpads/length`, 10, 5]
    ],
    [["getJoystickState", 0, 10]]
  );
  emit(
    [
      ["putNumber", `${FIRSTDS}/0/POVCount`, 10, -3],
      ["putNumber", `${FIRSTDS}/0/POVs`, 10, 0x89abcdef],
      ["putNumber", `${FIRSTDS}/0/AvailableAxes`, 10, 0xffff],
      ["putNumberArray", `${FIRSTDS}/0/Axes`, 10, [0, 32768, -32768, 1, -1, 65535, -65535, 2, -2, 100, -100, 5, -5, 9, -9, 16384, -16384, 7, 8, 9]]
    ],
    [["getJoystickState", 0, 10]]
  );

  // A table prefix found with a merge prefix reads through it; an akit and
  // datalog table for the same id resolve by field order.
  emit(
    [
      ["putNumber", `${DSLOG}0/opMode-shadow`, 10, 0],
      ["putBoolean", "DS:opMode", 10, true],
      ["putBooleanArray", `${DSLOG}0/buttons`, 10, [true]],
      ["putNumber", `${AKIT}0/ButtonsAvailable`, 10, 0b101],
      ["putNumber", `${AKIT}0/ButtonValues`, 10, 0b001]
    ],
    [["getJoystickState", 0, 10]]
  );
  emit(
    [
      ["putNumber", `${AKIT}0/ButtonsAvailable`, 10, 0b101],
      ["putNumber", `${AKIT}0/ButtonValues`, 10, 0b001],
      ["putBoolean", "DS:opMode", 10, true],
      ["putBooleanArray", `${DSLOG}0/buttons`, 10, [true]]
    ],
    [["getJoystickState", 0, 10]]
  );

  // Every layout with two samples per field, read before, at, between and
  // after them, with and without the timestamp-set cache; ids 1-5 of the
  // FIRST DS and NT tables alongside id 0.
  {
    const ops: unknown[][] = [
      ["putNumber", `${AKIT}1/ButtonCount`, 10, 3],
      ["putNumber", `${AKIT}1/ButtonValues`, 10, 0b101],
      ["putNumberArray", `${AKIT}1/AxisValues`, 10, [0.5]],
      ["putNumberArray", `${AKIT}1/POVs`, 10, [90]],
      ["putNumber", `${AKIT}1/ButtonCount`, 20, 4],
      ["putNumber", `${AKIT}1/ButtonValues`, 20, 0b1010],
      ["putNumberArray", `${AKIT}1/AxisValues`, 20, [-0.5, 1]],
      ["putNumberArray", `${AKIT}1/POVs`, 20, []],
      ["putBoolean", "DS:opMode", 10, false],
      ["putBooleanArray", `${DSLOG}3/buttons`, 10, [true]],
      ["putNumberArray", `${DSLOG}3/axes`, 10, [0.25]],
      ["putNumberArray", `${DSLOG}3/povs`, 10, [4]],
      ["putBooleanArray", `${DSLOG}3/buttons`, 20, [false, true]],
      ["putNumberArray", `${DSLOG}3/axes`, 20, []],
      ["putNumberArray", `${DSLOG}3/povs`, 20, [8, 3]],
      ["putNumber", `${NT}2/ButtonsAvailable`, 10, 0b1],
      ["putNumber", `${NT}2/ButtonValues`, 10, 0b1],
      ["putNumber", `${NT}2/ButtonValues`, 20, 0b0]
    ];
    for (const id of [0, 1, 2, 3, 4, 5]) {
      ops.push(
        ["putNumber", `${FIRSTDS}/${id}/AvailableButtons/high`, 10, id],
        ["putNumber", `${FIRSTDS}/${id}/AvailableButtons/low`, 10, 0xf],
        ["putNumber", `${FIRSTDS}/${id}/Buttons/high`, 10, 0],
        ["putNumber", `${FIRSTDS}/${id}/Buttons/low`, 10, 0b0101],
        ["putNumber", `${FIRSTDS}/${id}/Buttons/high`, 20, id],
        ["putNumber", `${FIRSTDS}/${id}/Buttons/low`, 20, 0b1010],
        ["putNumber", `${FIRSTDS}/${id}/POVCount`, 10, 1],
        ["putNumber", `${FIRSTDS}/${id}/POVs`, 10, id],
        ["putNumber", `${FIRSTDS}/${id}/POVs`, 20, 0xf]
      );
    }
    const calls: unknown[][] = [];
    for (const id of [0, 1, 2, 3, 4, 5]) {
      for (const t of [5, 10, 15, 20, 25]) calls.push(["getJoystickState", id, t]);
    }
    emit(ops, calls);
    emit(ops, calls, false);
  }

  // No joystick data at all, and timestamp ordering across the cache flag.
  emit(
    [["putNumber", "/Other/Value", 10, 1]],
    [["getJoystickState", 0, 10]],
    false
  );
}

function mechanismCases() {
  // Nested ligaments under three roots: a `position` root, an `x`/`y` root
  // and a root reached through generated fields.
  {
    const ops: unknown[][] = [
      ["putString", "mech/backgroundColor", 10, "#202020"],
      ["putNumberArray", "mech/dims", 10, [1.5, 1.0]],
      ["putNumber", "mech/root/angle", 10, 45],
      ["putNumber", "mech/root/length", 10, 0.5],
      ["putString", "mech/root/color", 10, "#ff0000"],
      ["putNumber", "mech/root/weight", 10, 2],
      ["putNumber", "mech/root/arm/angle", 10, -30],
      ["putNumber", "mech/root/arm/length", 10, 0.25],
      ["putString", "mech/root/arm/color", 10, "green"],
      ["putNumber", "mech/root/arm/weight", 10, 1],
      ["putNumber", "mech/root/arm/tip/angle", 10, 90],
      ["putNumber", "mech/root/arm/tip/length", 10, 0.125],
      ["putString", "mech/root/arm/tip/color", 10, "#00ffff"],
      ["putNumber", "mech/root/arm/tip/weight", 10, 0.5],
      ["putNumber", "mech/elevator/x", 10, 0.1],
      ["putNumber", "mech/elevator/y", 10, 0.2],
      ["putNumber", "mech/elevator/stage/angle", 10, 90],
      ["putNumber", "mech/elevator/stage/length", 10, 0.3],
      ["putString", "mech/elevator/stage/color", 10, "#00f"],
      ["putNumber", "mech/elevator/stage/weight", 10, 4],
      ["putNumberArray", "mech/wrist/position", 10, [0.5, 0.5, 9.9]],
      ["putNumber", "mech/wrist/joint/angle", 10, 10],
      ["putNumber", "mech/wrist/joint/length", 10, 0.2],
      ["putString", "mech/wrist/joint/color", 10, "#123"],
      ["putNumber", "mech/wrist/joint/weight", 10, 3],
      ["putString", "mech/.hidden/x", 10, "skip"],
      ["putString", "mech/root/.type", 10, "ignored-here"]
    ];
    emit(ops, [
      ["getMechanismState", "mech", 5],
      ["getMechanismState", "mech", 10],
      ["getMechanismState", "mech", 15],
      ["getMechanismState", "missing", 10],
      ["getMechanismState", "mech/", 10]
    ]);
  }

  // Missing and mistyped fields: a ligament missing `color` reads the `0`
  // default; a root missing `y` aborts the walk mid-list; a `position` node
  // with no field of its own reads the `null` fullKey.
  {
    const ops: unknown[][] = [
      ["putString", "m/backgroundColor", 10, "black"],
      ["putNumberArray", "m/dims", 10, [1, 1]],
      ["putNumber", "m/a/angle", 10, 0],
      ["putNumber", "m/a/length", 10, 1],
      ["putNumber", "m/a/weight", 10, 1],
      ["putNumber", "m/a/color", 10, 123],
      ["putNumber", "m/b/x", 10, 0.25],
      ["putNumber", "m/b/arm/angle", 10, 10],
      ["putNumber", "m/b/arm/length", 10, 1],
      ["putString", "m/b/arm/color", 10, "red"],
      ["putNumber", "m/b/arm/weight", 10, 1],
      ["putNumber", "m/c/position/x", 10, 7],
      ["putNumber", "m/c/arm/angle", 10, 20],
      ["putNumber", "m/c/arm/length", 10, 1],
      ["putString", "m/c/arm/color", 10, "blue"],
      ["putNumber", "m/c/arm/weight", 10, 2],
      ["putNumber", "m/d/x", 10, 0.5],
      ["putNumber", "m/d/y", 10, 0.5],
      ["putNumber", "m/d/angle", 10, 15],
      ["putNumber", "m/d/length", 10, 1],
      ["putString", "m/d/color", 10, "white"],
      ["putNumber", "m/d/weight", 10, 1]
    ];
    emit(ops, [["getMechanismState", "m", 10]]);
  }

  // Order: a bailing root in the middle keeps earlier lines and drops the
  // rest; integer-named roots iterate before named ones.
  {
    const ops: unknown[][] = [
      ["putString", "m/backgroundColor", 10, "black"],
      ["putNumberArray", "m/dims", 10, [1, 1]],
      ["putNumber", "m/first/x", 10, 0],
      ["putNumber", "m/first/y", 10, 0],
      ["putNumber", "m/first/ok/angle", 10, 0],
      ["putNumber", "m/first/ok/length", 10, 1],
      ["putString", "m/first/ok/color", 10, "a"],
      ["putNumber", "m/first/ok/weight", 10, 1],
      ["putNumber", "m/bad/arm/length", 10, 1],
      ["putNumber", "m/bad/x", 10, 0],
      ["putNumber", "m/bad/y", 10, 0],
      ["putNumber", "m/last/x", 10, 0],
      ["putNumber", "m/last/y", 10, 0],
      ["putNumber", "m/last/ok/angle", 10, 0],
      ["putNumber", "m/last/ok/length", 10, 1],
      ["putString", "m/last/ok/color", 10, "b"],
      ["putNumber", "m/last/ok/weight", 10, 1],
      ["putNumber", "m/2/x", 10, 0],
      ["putNumber", "m/2/y", 10, 0],
      ["putNumber", "m/2/z/angle", 10, 0],
      ["putNumber", "m/2/z/length", 10, 1],
      ["putString", "m/2/z/color", 10, "c"],
      ["putNumber", "m/2/z/weight", 10, 1],
      ["putNumber", "m/10/x", 10, 0],
      ["putNumber", "m/10/y", 10, 0],
      ["putNumber", "m/10/z/angle", 10, 0],
      ["putNumber", "m/10/z/length", 10, 1],
      ["putString", "m/10/z/color", 10, "d"],
      ["putNumber", "m/10/z/weight", 10, 1]
    ];
    emit(ops, [["getMechanismState", "m", 10]]);
  }

  // Skip lists: "x"/"y"/"position" ligament names under a root are skipped;
  // ".type" under a ligament is skipped; ligaments named "position",
  // "backgroundColor" or ".weird" inside a ligament still recurse; ".type"
  // as a root child aborts when reached.
  {
    const ops: unknown[][] = [
      ["putString", "m/backgroundColor", 10, "black"],
      ["putNumberArray", "m/dims", 10, [1, 1]],
      ["putNumber", "m/r/x", 10, 0],
      ["putNumber", "m/r/y", 10, 0],
      ["putNumber", "m/r/x/angle", 10, 90],
      ["putNumber", "m/r/x/length", 10, 9],
      ["putString", "m/r/x/color", 10, "skip"],
      ["putNumber", "m/r/x/weight", 10, 9],
      ["putNumber", "m/r/position/angle", 10, 90],
      ["putNumber", "m/r/position/length", 10, 9],
      ["putString", "m/r/position/color", 10, "skip"],
      ["putNumber", "m/r/position/weight", 10, 9],
      ["putNumber", "m/r/lig/angle", 10, 0],
      ["putNumber", "m/r/lig/length", 10, 1],
      ["putString", "m/r/lig/color", 10, "c"],
      ["putNumber", "m/r/lig/weight", 10, 1],
      ["putString", "m/r/lig/.type", 10, "skipped-child"],
      ["putNumber", "m/r/lig/x/angle", 10, 0],
      ["putNumber", "m/r/lig/x/length", 10, 1],
      ["putString", "m/r/lig/x/color", 10, "xlig"],
      ["putNumber", "m/r/lig/x/weight", 10, 1],
      ["putNumber", "m/r/lig/position/angle", 10, 0],
      ["putNumber", "m/r/lig/position/length", 10, 1],
      ["putString", "m/r/lig/position/color", 10, "plig"],
      ["putNumber", "m/r/lig/position/weight", 10, 1],
      ["putNumber", "m/r/lig/backgroundColor/angle", 10, 0],
      ["putNumber", "m/r/lig/backgroundColor/length", 10, 1],
      ["putString", "m/r/lig/backgroundColor/color", 10, "blig"],
      ["putNumber", "m/r/lig/backgroundColor/weight", 10, 1],
      ["putNumber", "m/r/lig/.weird/angle", 10, 0],
      ["putNumber", "m/r/lig/.weird/length", 10, 1],
      ["putString", "m/r/lig/.weird/color", 10, "wlig"],
      ["putNumber", "m/r/lig/.weird/weight", 10, 1]
    ];
    emit(ops, [["getMechanismState", "m", 10]]);
  }

  // Type mismatches and absent config: dims as a string, backgroundColor as
  // a number, arrays of unusual length, wrong-typed children, and reads
  // before the samples land.
  {
    const ops: unknown[][] = [
      ["putString", "a/backgroundColor", 10, "ok"],
      ["putString", "a/dims", 10, "not-an-array"],
      ["putNumber", "b/backgroundColor", 10, 5],
      ["putNumberArray", "b/dims", 10, [1, 2]],
      ["putString", "c/backgroundColor", 10, "ok"],
      ["putNumberArray", "c/dims", 10, []],
      ["putString", "d/backgroundColor", 10, "ok"],
      ["putNumberArray", "d/dims", 10, [3]],
      ["putString", "e/backgroundColor", 10, "ok"],
      ["putNumberArray", "e/dims", 10, [1, 2, 3, 4]],
      ["putNumber", "e/r/x", 10, 0],
      ["putNumber", "e/r/y", 10, 0],
      ["putString", "e/r/l/angle", 10, "wrong"],
      ["putBoolean", "e/r/l/length", 10, true],
      ["putNumberArray", "e/r/l/color", 10, [1]],
      ["putString", "e/r/l/weight", 10, "wrong"],
      ["putString", "f/backgroundColor", 20, "late"],
      ["putNumberArray", "f/dims", 20, [1, 1]],
      ["putNumber", "f/r/x", 20, 0],
      ["putNumber", "f/r/y", 20, 0],
      ["putNumber", "f/r/l/angle", 20, 0],
      ["putNumber", "f/r/l/length", 20, 1],
      ["putString", "f/r/l/color", 20, "c"],
      ["putNumber", "f/r/l/weight", 20, 1],
      ["putString", "/backgroundColor", 10, "empty-key"],
      ["putNumberArray", "/dims", 10, [1, 1]]
    ];
    emit(ops, [
      ["getMechanismState", "a", 10],
      ["getMechanismState", "b", 10],
      ["getMechanismState", "c", 10],
      ["getMechanismState", "d", 10],
      ["getMechanismState", "e", 10],
      ["getMechanismState", "e", 5],
      ["getMechanismState", "f", 10],
      ["getMechanismState", "f", 20],
      ["getMechanismState", "", 10],
      ["getMechanismState", 5, 10],
      ["getMechanismState", true, 10],
      ["getMechanismState", null, 10]
    ]);
  }

  // Generated children written by putUnknownStruct are still traversed.
  emit(
    [
      ["putString", "m/backgroundColor", 10, "s"],
      ["putNumberArray", "m/dims", 10, [2, 2]],
      ["putUnknownStruct", "m/auto", 10, { x: 1, y: 2, arm: { angle: 30, length: 0.5, color: "#fff", weight: 1 } }]
    ],
    [["getMechanismState", "m", 10]]
  );

  // Non-finite and signed-zero values: a short or long `position`, NaN and
  // infinite angle/length/weight, and -0 everywhere. Merging them runs the
  // lines through jsonCopy, which turns non-finite numbers into null.
  const edgeOps: unknown[][] = [
    ["putString", "n/backgroundColor", 10, "#000"],
    ["putNumberArray", "n/dims", 10, [3, 2]],
    ["putNumberArray", "n/empty/position", 10, []],
    ["putNumber", "n/empty/l/angle", 10, 30],
    ["putNumber", "n/empty/l/length", 10, 1],
    ["putString", "n/empty/l/color", 10, "a"],
    ["putNumber", "n/empty/l/weight", 10, -0],
    ["putNumberArray", "n/one/position", 10, [0.5]],
    ["putNumber", "n/one/l/angle", 10, 180],
    ["putNumber", "n/one/l/length", 10, 2],
    ["putString", "n/one/l/color", 10, "b"],
    ["putNumber", "n/one/l/weight", 10, NaN],
    ["putNumberArray", "n/wide/position", 10, [NaN, -0, Infinity]],
    ["putNumber", "n/wide/l/angle", 10, NaN],
    ["putNumber", "n/wide/l/length", 10, 1],
    ["putString", "n/wide/l/color", 10, "c"],
    ["putNumber", "n/wide/l/weight", 10, Infinity],
    ["putNumber", "n/wide/l/k/angle", 10, 90],
    ["putNumber", "n/wide/l/k/length", 10, 1],
    ["putString", "n/wide/l/k/color", 10, "d"],
    ["putNumber", "n/wide/l/k/weight", 10, 1],
    ["putNumber", "n/inf/x", 10, -0],
    ["putNumber", "n/inf/y", 10, 1e308],
    ["putNumber", "n/inf/l/angle", 10, 1e300],
    ["putNumber", "n/inf/l/length", 10, Infinity],
    ["putNumber", "n/inf/l/weight", 10, 5e-324],
    ["putNumber", "n/inf/l/color", 10, 7],
    ["putNumber", "n/neg/x", 10, -1],
    ["putNumber", "n/neg/y", 10, -2],
    ["putNumber", "n/neg/l/angle", 10, -720.5],
    ["putNumber", "n/neg/l/length", 10, -1.5],
    ["putString", "n/neg/l/color", 10, ""],
    ["putNumber", "n/neg/l/weight", 10, -3]
  ];
  emit(edgeOps, [
    ["getMechanismState", "n", 10],
    ["mergeMechanismStates", ["n"], 10]
  ]);

  // Merges of log-derived states as the Mechanism and 3D Field controllers
  // run them: differing and degenerate dims, background colors taken from the
  // first state, absent keys skipped, and no states at all.
  const mechanism = (key: string, bg: string, dims: number[], t = 10): unknown[][] => [
    ["putString", `${key}/backgroundColor`, t, bg],
    ["putNumberArray", `${key}/dims`, t, dims],
    ["putNumber", `${key}/r/x`, t, 0.25],
    ["putNumber", `${key}/r/y`, t, 0.5],
    ["putNumber", `${key}/r/l/angle`, t, 60],
    ["putNumber", `${key}/r/l/length`, t, 0.75],
    ["putString", `${key}/r/l/color`, t, `${bg}-line`],
    ["putNumber", `${key}/r/l/weight`, t, 2],
    ["putNumber", `${key}/r/l/k/angle`, t, -45],
    ["putNumber", `${key}/r/l/k/length`, t, 0.5],
    ["putString", `${key}/r/l/k/color`, t, `${bg}-tip`],
    ["putNumber", `${key}/r/l/k/weight`, t, 1]
  ];
  const mergeOps: unknown[][] = [
    ...mechanism("a", "#111", [2, 1]),
    ...mechanism("b", "#222", [4, 3]),
    ...mechanism("c", "#333", [1, 0.5]),
    ...mechanism("d", "#444", []),
    ...mechanism("e", "#555", [NaN, 1]),
    ...mechanism("f", "#666", [Infinity, -Infinity]),
    ...mechanism("g", "#777", [-0, -0]),
    ...mechanism("h", "#888", [5]),
    ...mechanism("late", "#999", [10, 10], 20),
    ["putString", "bare/backgroundColor", 10, "bare"],
    ["putNumberArray", "bare/dims", 10, [6, 6]]
  ];
  emit(mergeOps, [
    ["mergeMechanismStates", ["a", "b", "c"], 10],
    ["mergeMechanismStates", ["c", "b", "a"], 10],
    ["mergeMechanismStates", ["b"], 10],
    ["mergeMechanismStates", ["a", "missing", "bare"], 10],
    ["mergeMechanismStates", ["a", "d"], 10],
    ["mergeMechanismStates", ["d", "a"], 10],
    ["mergeMechanismStates", ["a", "e"], 10],
    ["mergeMechanismStates", ["a", "f"], 10],
    ["mergeMechanismStates", ["g"], 10],
    ["mergeMechanismStates", ["g", "a"], 10],
    ["mergeMechanismStates", ["h", "a"], 10],
    ["mergeMechanismStates", ["a", "late"], 10],
    ["mergeMechanismStates", ["a", "late"], 20],
    ["mergeMechanismStates", ["missing"], 10],
    ["mergeMechanismStates", [], 10]
  ]);
  emit(mergeOps, [["mergeMechanismStates", ["late", "b", "a"], 25]], false);
}

if (mode === "joystick") {
  joystickCases();
} else if (mode === "mechanism") {
  mechanismCases();
} else {
  throw new Error(`genState: unknown mode ${mode}`);
}

for (const [cache, ops, calls] of cases) {
  const args = [toTree(cache), toTree(ops), toTree(calls)];
  const result = run(cache, ops as any[][], calls as any[][]);
  process.stdout.write(JSON.stringify({ fn: "oracle:stateRun", args, ok: toTree(result) }) + "\n");
}
