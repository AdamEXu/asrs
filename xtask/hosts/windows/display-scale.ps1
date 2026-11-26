# display-scale.ps1 [-Percent <n>]: prints the primary display's scale in percent, after setting it
# when -Percent is given. Runs in the desktop session (`cargo xtask host windows gui`). The change
# applies at once to per-monitor aware programs such as Electron and is kept across logons.
param([int]$Percent = 0)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class DisplayScale {
  [DllImport("user32.dll")] static extern int GetDisplayConfigBufferSizes(uint flags, out uint paths, out uint modes);
  [DllImport("user32.dll")] static extern int QueryDisplayConfig(uint flags, ref uint paths, byte[] pathArray, ref uint modes, byte[] modeArray, IntPtr topology);
  [DllImport("user32.dll")] static extern int DisplayConfigGetDeviceInfo(byte[] packet);
  [DllImport("user32.dll")] static extern int DisplayConfigSetDeviceInfo(byte[] packet);
  [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
  [DllImport("user32.dll")] static extern IntPtr MonitorFromPoint(long point, uint flags);
  [DllImport("shcore.dll")] static extern int GetDpiForMonitor(IntPtr monitor, int type, out uint x, out uint y);
  static readonly int[] Steps = {100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500};
  const int GetScale = -3, SetScale = -4, PathSize = 72, ModeSize = 64, OnlyActive = 2, SourceMode = 1;

  static byte[] Packet(int type, int size) {
    uint paths, modes;
    Check(GetDisplayConfigBufferSizes(OnlyActive, out paths, out modes), "GetDisplayConfigBufferSizes");
    byte[] pathArray = new byte[paths * PathSize], modeArray = new byte[modes * ModeSize];
    Check(QueryDisplayConfig(OnlyActive, ref paths, pathArray, ref modes, modeArray, IntPtr.Zero), "QueryDisplayConfig");
    byte[] packet = new byte[size];
    BitConverter.GetBytes(type).CopyTo(packet, 0);
    BitConverter.GetBytes(size).CopyTo(packet, 4);
    Array.Copy(pathArray, PrimaryPath(pathArray, paths, modeArray, modes) * PathSize, packet, 8, 12);
    return packet;
  }

  // The path whose source mode sits at (0, 0): the primary display, which Get reads.
  static int PrimaryPath(byte[] pathArray, uint paths, byte[] modeArray, uint modes) {
    for (int i = 0; i < paths; i++) {
      uint mode = BitConverter.ToUInt32(pathArray, i * PathSize + 12);
      if (mode >= modes) continue;
      int at = (int)mode * ModeSize;
      if (BitConverter.ToInt32(modeArray, at) == SourceMode && BitConverter.ToInt32(modeArray, at + 28) == 0 && BitConverter.ToInt32(modeArray, at + 32) == 0) return i;
    }
    return 0;
  }

  static void Check(int result, string call) {
    if (result != 0) throw new Exception(call + " failed: " + result);
  }

  public static int Get() {
    SetThreadDpiAwarenessContext(new IntPtr(-4));
    uint x, y;
    Check(GetDpiForMonitor(MonitorFromPoint(0, 1), 0, out x, out y), "GetDpiForMonitor");
    return (int)(x * 100 / 96);
  }

  public static void Set(int percent) {
    int step = Array.IndexOf(Steps, percent);
    if (step < 0) throw new Exception("no display scale of " + percent + " %");
    byte[] get = Packet(GetScale, 32);
    Check(DisplayConfigGetDeviceInfo(get), "DisplayConfigGetDeviceInfo");
    int recommended = -BitConverter.ToInt32(get, 20);
    byte[] set = Packet(SetScale, 24);
    BitConverter.GetBytes(step - recommended).CopyTo(set, 20);
    Check(DisplayConfigSetDeviceInfo(set), "DisplayConfigSetDeviceInfo");
  }
}
"@
if ($Percent -gt 0 -and [DisplayScale]::Get() -ne $Percent) {
  [DisplayScale]::Set($Percent)
  $deadline = (Get-Date).AddSeconds(20)
  while ([DisplayScale]::Get() -ne $Percent) {
    if ((Get-Date) -gt $deadline) { throw "the display did not change to $Percent %" }
    Start-Sleep -Milliseconds 200
  }
  Start-Sleep -Seconds 2
}
[DisplayScale]::Get()
