import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import * as readline from "node:readline";

// The Windows desktop, which webContents has no part in: the real pointer, and the pixels of the
// screen. Electron draws the caption buttons of `titleBarOverlay` outside the page, so capturePage
// does not have them and sendInputEvent does not reach them.

const SCRIPT = `
$ErrorActionPreference = 'Stop'
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System; using System.Drawing; using System.Drawing.Imaging; using System.Runtime.InteropServices;
public static class Desktop {
  [DllImport("user32")] public static extern bool SetProcessDpiAwarenessContext(IntPtr value);
  [DllImport("user32")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32")] public static extern void mouse_event(uint flags, int x, int y, int data, UIntPtr extra);
  public static string Grab(int x, int y, int width, int height) {
    using (var bitmap = new Bitmap(width, height, PixelFormat.Format32bppRgb)) {
      using (var graphics = Graphics.FromImage(bitmap)) graphics.CopyFromScreen(x, y, 0, 0, new Size(width, height));
      var bits = bitmap.LockBits(new Rectangle(0, 0, width, height), ImageLockMode.ReadOnly, PixelFormat.Format32bppRgb);
      var bytes = new byte[width * height * 4];
      Marshal.Copy(bits.Scan0, bytes, 0, bytes.Length);
      bitmap.UnlockBits(bits);
      return Convert.ToBase64String(bytes);
    }
  }
}
'@
[void][Desktop]::SetProcessDpiAwarenessContext([IntPtr](-4))
while ($null -ne ($line = [Console]::In.ReadLine())) {
  $a = $line.Split(' ')
  $answer = 'ok'
  switch ($a[0]) {
    'move' { [void][Desktop]::SetCursorPos([int]$a[1], [int]$a[2]); [Desktop]::mouse_event(1, 0, 0, 0, [UIntPtr]::Zero) }
    'down' { [Desktop]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero) }
    'up' { [Desktop]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero) }
    'wheel' { [Desktop]::mouse_event(0x800, 0, 0, [int]$a[1], [UIntPtr]::Zero) }
    'hwheel' { [Desktop]::mouse_event(0x1000, 0, 0, [int]$a[1], [UIntPtr]::Zero) }
    'grab' { $answer = [Desktop]::Grab([int]$a[1], [int]$a[2], [int]$a[3], [int]$a[4]) }
  }
  [Console]::Out.WriteLine($answer)
  [Console]::Out.Flush()
}
`;

export class Desktop {
  private child: ChildProcessWithoutNullStreams;
  private waiting: { resolve: (line: string) => void; reject: (e: Error) => void }[] = [];
  private failure = "";

  constructor() {
    const encoded = Buffer.from(SCRIPT, "utf16le").toString("base64");
    this.child = spawn("powershell", ["-NoProfile", "-EncodedCommand", encoded]);
    readline.createInterface({ input: this.child.stdout }).on("line", (line) => this.waiting.shift()?.resolve(line));
    this.child.stderr.on("data", (data) => (this.failure += data));
    this.child.on("exit", (code) => {
      for (const { reject } of this.waiting.splice(0)) reject(new Error(`the desktop helper exited with ${code}: ${this.failure}`));
    });
  }

  private ask(request: string): Promise<string> {
    return new Promise((resolve, reject) => {
      if (this.child.exitCode !== null) return reject(new Error(`the desktop helper exited: ${this.failure}`));
      this.waiting.push({ resolve, reject });
      this.child.stdin.write(`${request}\n`);
    });
  }

  async pointer(x: number, y: number, then?: "down" | "up") {
    await this.ask(`move ${x} ${y}`);
    if (then !== undefined) await this.ask(then);
  }

  /** Wheel units, 120 a notch. */
  async wheel(unitsX: number, unitsY: number) {
    if (unitsX !== 0) await this.ask(`hwheel ${unitsX}`);
    if (unitsY !== 0) await this.ask(`wheel ${unitsY}`);
  }

  /** BGRA pixels, the alpha bytes of which say nothing. */
  async grab(x: number, y: number, width: number, height: number): Promise<Buffer> {
    return Buffer.from(await this.ask(`grab ${x} ${y} ${width} ${height}`), "base64");
  }

  close() {
    this.child.stdin.end();
  }
}
