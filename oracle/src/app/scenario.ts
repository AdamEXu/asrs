export type Theme = "light" | "dark";
export type Scale = 1 | 2;
export type GlMode = "native" | "swiftshader" | "d3d11-warp";

export type Action =
  | { main: string; data?: unknown }
  | { open: string[]; merge?: boolean }
  | { eval: string }
  | { selection: [string, ...unknown[]] }
  | { wait: number }
  | { idle: true }
  | { mouse: "move" | "down" | "up" | "click" | "leave"; x: number; y: number; button?: "left" | "right" | "middle"; clickCount?: number; modifiers?: string[] }
  | {
      wheel: { x: number; y: number; deltaX?: number; deltaY?: number; wheelTicksX?: number; wheelTicksY?: number; precise?: boolean };
    }
  /** The real pointer, which reaches what Electron draws outside the page, and its wheel, in
   *  units of 120 a notch (Windows only). */
  | { pointer: "move" | "down" | "up" | "wheel"; x: number; y: number; unitsX?: number; unitsY?: number }
  /** Whether the hub is the desktop's active window (Windows only). */
  | { activate: boolean }
  | { key: string; modifiers?: string[] }
  | { dialog: number | string | null }
  | { menu: string[] }
  | { appMenu: string[] }
  | { prompt: string | null; index?: number; width?: number; height?: number }
  | { download: string | number }
  | { role: "selectAll" }
  | { copy: string }
  | { focus: boolean }
  /** Waits until the hub's title is, or stops being, this. */
  | { title: string; not?: boolean }
  | { traverse: string; count?: number; back?: boolean }
  | { hover: string }
  | { capture: string; probes?: string[]; masks?: string[] };

export type Scenario = {
  name: string;
  page: "app" | "colour-patch";
  window: { width: number; height: number };
  scales: Scale[];
  themes: Theme[];
  gl: boolean;
  prefs: Record<string, unknown>;
  userData: Record<string, unknown>;
  /** Served by robotServer.ts; the driver sends connections to it. */
  robot: boolean;
  /** Served by xtask's live server; the driver sends the RLOG connections to it. */
  live: boolean;
  actions: Action[];
};

const ACTION_KEYS = ["main", "open", "eval", "selection", "wait", "idle", "mouse", "wheel", "pointer", "activate", "key", "dialog", "menu", "appMenu", "prompt", "download", "role", "copy", "focus", "title", "traverse", "hover", "capture"];
const TOP_KEYS = ["page", "window", "scales", "themes", "gl", "prefs", "userData", "robot", "live", "scrollbars", "platforms", "actions", "about"];

export function actionKind(action: Action): string {
  return ACTION_KEYS.find((k) => k in action)!;
}

function fail(name: string, message: string): never {
  throw new Error(`scenario ${name}: ${message}`);
}

export function parseScenario(name: string, text: string): Scenario {
  const raw = JSON.parse(text);
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) fail(name, "not a JSON object");
  for (const key of Object.keys(raw)) if (!TOP_KEYS.includes(key)) fail(name, `unknown key ${JSON.stringify(key)}`);
  const page = raw.page ?? "app";
  if (page !== "app" && page !== "colour-patch") fail(name, `unknown page ${JSON.stringify(page)}`);
  const window = raw.window ?? { width: 1100, height: 650 };
  if (!Number.isInteger(window.width) || !Number.isInteger(window.height)) fail(name, "window needs integer width and height");
  const scales = raw.scales ?? [1, 2];
  if (!Array.isArray(scales) || scales.length === 0 || scales.some((s: unknown) => s !== 1 && s !== 2)) fail(name, "scales must be a subset of [1, 2]");
  const themes = raw.themes ?? ["light", "dark"];
  if (!Array.isArray(themes) || themes.length === 0 || themes.some((t: unknown) => t !== "light" && t !== "dark")) {
    fail(name, 'themes must be a subset of ["light", "dark"]');
  }
  const platforms = raw.platforms ?? [process.platform];
  if (!Array.isArray(platforms) || !platforms.includes(process.platform)) fail(name, `only runs on ${JSON.stringify(platforms)}`);
  const actions = raw.actions ?? [];
  if (!Array.isArray(actions)) fail(name, "actions must be an array");
  const captures = new Set<string>();
  actions.forEach((action: Record<string, unknown>, i: number) => {
    const kinds = Object.keys(action).filter((k) => ACTION_KEYS.includes(k));
    if (kinds.length !== 1) fail(name, `action ${i} must have exactly one of ${ACTION_KEYS.join(", ")}`);
    if (kinds[0] === "capture") {
      const label = action.capture;
      if (typeof label !== "string" || !/^[a-z0-9][a-z0-9-]*$/.test(label)) fail(name, `action ${i}: capture names are [a-z0-9-]`);
      if (captures.has(label)) fail(name, `action ${i}: capture ${label} appears twice`);
      captures.add(label);
    }
  });
  if (captures.size === 0) fail(name, "no capture action");
  return {
    name,
    page,
    window,
    scales,
    themes,
    gl: raw.gl === true,
    prefs: raw.prefs ?? {},
    userData: raw.userData ?? {},
    robot: raw.robot !== undefined,
    live: raw.live !== undefined,
    actions
  };
}
