import "./environment";
import { runHandshake } from "./download/handshake";
import { runLive, runLiveOne } from "./download/live";
import { runOffer } from "./download/offer";
import { runScript } from "./download/script";
import { runSftp } from "./download/sftp";
import { runWindow } from "./download/window";

// Ground truth for asrs-services' log downloader (P04), from upstream's real main.ts, download.ts
// and ssh2 package.
//
//   genDownload script <darwin|win32> <count> <seed>
//     main.ts's download section against a scripted ssh2 Client (see download/script.ts): a hand
//     written session, then <count> random ops.
//   genDownload sftp <count> <seed>
//     ssh2's SFTP session and fastGet against scripted packets (see download/sftp.ts): hand written
//     cases, then <count> random sessions.
//   genDownload handshake <count> <seed>
//     ssh2's parser on a server's unencrypted bytes (see download/handshake.ts), each input cut whole,
//     byte by byte and <count> times at random.
//   genDownload offer
//     ssh2's ident and KEXINIT (see download/offer.ts).
//   genDownload window
//     download.ts's start message and alert texts (see download/window.ts).
//   genDownload live [<scenario>...]
//     main.ts with the real ssh2 against a local sshd and raw servers (see download/live.ts).

const [mode, ...args] = process.argv.slice(2);
switch (mode) {
  case "script":
    await runScript(args[0] as "darwin" | "win32", Number(args[1]), Number(args[2]));
    break;
  case "live":
    await runLive(args);
    break;
  case "live-one":
    await runLiveOne(args[0]);
    break;
  case "sftp":
    await runSftp(Number(args[0]), Number(args[1]));
    break;
  case "handshake":
    await runHandshake(Number(args[0]), Number(args[1]));
    break;
  case "offer":
    await runOffer();
    break;
  case "window":
    await runWindow();
    break;
  default:
    console.error("usage: genDownload <script|live|sftp|handshake|offer|window> ...");
    process.exit(2);
}
await new Promise<void>((resolve) => process.stdout.write("", () => resolve()));
process.exit(0);
