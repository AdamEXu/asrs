import { execFileSync } from "node:child_process";
import * as fs from "node:fs";

export type DisplayProfile =
  | { source: "macos-nsscreen"; displayId: number; name: string; bytes: number }
  | { source: "x11-root-atom"; bytes: number }
  | { source: "windows-icm"; path: string; bytes: number }
  | { source: "none"; reason: string }
  | { source: "forced-srgb" };

const MAC_SCRIPT = `
ObjC.import("AppKit");
function run(argv) {
  const want = Number(argv[0]);
  const screens = $.NSScreen.screens;
  for (let i = 0; i < screens.count; i++) {
    const screen = screens.objectAtIndex(i);
    if (Number(ObjC.unwrap(screen.deviceDescription.objectForKey("NSScreenNumber"))) !== want) continue;
    const space = screen.colorSpace;
    if (!space.ICCProfileData.writeToFileAtomically(argv[1], true)) throw new Error("write failed");
    return ObjC.unwrap(space.localizedName);
  }
  throw new Error("no NSScreen for display " + want);
}`;

const WINDOWS_SCRIPT = `
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class Icm {
  [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr hwnd, IntPtr dc);
  [DllImport("gdi32.dll", CharSet = CharSet.Unicode)] public static extern bool GetICMProfileW(IntPtr dc, ref uint size, StringBuilder name);
  public static string Profile() {
    IntPtr dc = GetDC(IntPtr.Zero);
    try {
      uint size = 1024;
      StringBuilder name = new StringBuilder(1024);
      return GetICMProfileW(dc, ref size, name) ? name.ToString() : "";
    } finally { ReleaseDC(IntPtr.Zero, dc); }
  }
}
'@
[Icm]::Profile()`;

function x11Profile(): Buffer | null {
  const out = execFileSync("xprop", ["-root", "-notype", "_ICC_PROFILE"], { encoding: "utf8" });
  const match = /^_ICC_PROFILE\s*=\s*(.*)$/m.exec(out);
  if (match === null) return null;
  return Buffer.from(match[1].split(",").map((v) => Number(v.trim()) & 0xff));
}

// capturePage returns pixels in the colour space of the display the window is on, so the
// profile is read from the OS for that display and stored beside the bitmap.
export function saveDisplayProfile(displayId: number, file: string): DisplayProfile {
  switch (process.platform) {
    case "darwin": {
      const name = execFileSync("osascript", ["-l", "JavaScript", "-e", MAC_SCRIPT, String(displayId), file], { encoding: "utf8" }).trim();
      return { source: "macos-nsscreen", displayId, name, bytes: fs.statSync(file).size };
    }
    case "linux": {
      const bytes = x11Profile();
      if (bytes === null) return { source: "none", reason: "no _ICC_PROFILE on the X root window: the display is untagged sRGB" };
      fs.writeFileSync(file, bytes);
      return { source: "x11-root-atom", bytes: bytes.length };
    }
    case "win32": {
      const path = execFileSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", WINDOWS_SCRIPT], { encoding: "utf8" }).trim();
      if (path === "") return { source: "none", reason: "GetICMProfileW returned no profile: the display is untagged sRGB" };
      fs.copyFileSync(path, file);
      return { source: "windows-icm", path, bytes: fs.statSync(file).size };
    }
    default:
      throw new Error(`no display profile reader for ${process.platform}`);
  }
}
