import { createRequire } from "node:module";
import path from "node:path";
import url from "node:url";
import zlib from "node:zlib";

// Ground truth for the helpers asrs-services' asset downloader reproduces from node and the
// `download` package's dependency tree: node's legacy `url.parse`/`url.format`/`url.resolve`
// (IDNA through the runtime's own ada), `url-parse-lax`, the WHATWG URL pathname basename that
// `download` names files by, `path.posix`/`path.win32` dirname/basename/extname, `filenamify`,
// `content-disposition`, `file-type` 11 (download's extension guess), `archive-type` (file-type 4),
// `ext-name`'s mime lookup, and `decompress-unzip` on yauzl. Every value is computed by the real
// module from upstream's node_modules; the generator only picks inputs.
// Each record is {"fn","in":[...],"out"} where a throw is recorded as {"throws":true} and bytes
// as {"hex"}.
//   genDownloadHelpers <count> <seed>

declare const __UPSTREAM_DIR__: string;

const [countArg, seedArg] = process.argv.slice(2);
const upstreamRequire = createRequire(path.join(__UPSTREAM_DIR__, "package.json"));
const downloadRequire = createRequire(upstreamRequire.resolve("download"));
const decompressRequire = createRequire(upstreamRequire.resolve("decompress"));
const urlParseLax = createRequire(downloadRequire.resolve("got"))("url-parse-lax");
const filenamify = downloadRequire("filenamify");
const contentDisposition = downloadRequire("content-disposition");
const fileType = downloadRequire("file-type");
const archiveType = downloadRequire("archive-type");
const extName = downloadRequire("ext-name");
const decompressUnzip = decompressRequire("decompress-unzip");

let state = Number(seedArg) >>> 0 || 1;
function next(): number {
  state ^= state << 13;
  state ^= state >>> 17;
  state ^= state << 5;
  return state >>> 0;
}
const below = (n: number) => next() % n;
const pick = <T>(list: readonly T[]): T => list[below(list.length)];
const count = Number(countArg) || 0;

const hex = (bytes: Uint8Array) => ({ hex: Buffer.from(bytes).toString("hex") });
const out = (fn: string, input: unknown[], value: unknown) => process.stdout.write(JSON.stringify({ fn, in: input, out: value }) + "\n");
function attempt(run: () => unknown): unknown {
  try {
    const value = run();
    return value === undefined ? null : value;
  } catch {
    return { throws: true };
  }
}

const URL_FIELDS = ["protocol", "slashes", "auth", "host", "port", "hostname", "hash", "search", "query", "pathname", "path", "href"] as const;
const urlFields = (parsed: any) => Object.fromEntries(URL_FIELDS.map((k) => [k, parsed[k] ?? null]));

const URL_FIXED = [
  "",
  " ",
  "https://github.com/Mechanical-Advantage/AdvantageScopeAssets/releases/download/default-assets-v2/Field2d_FRCEvergreen.zip",
  "https://objects.githubusercontent.com/github-production-release-asset-2e65be/1?X-Amz-Algorithm=AWS4&response-content-disposition=attachment%3B%20filename%3DX.zip",
  "http://example.com",
  "http://example.com/",
  "HTTP://EXAMPLE.COM/A/B?C#D",
  "//example.com/x",
  "///x",
  "example.com/x",
  "localhost:8080/x",
  "localhost/x",
  "unix:/tmp/sock:/x",
  "http://unix:/tmp/sock:/x",
  "javascript:alert(1)",
  "mailto:a@b.c",
  "file:///C:/x",
  "file://host/share",
  "ftp://user:pass@host:21/x",
  "http://user@host/x",
  "http://:@host/x",
  "http://a@b@c/x",
  "http://%41@host/",
  "http://%zz@host/",
  "http://host:/x",
  "http://host:80x/y",
  "http://host:65536/",
  "http://[::1]:8080/x",
  "http://[::1/x",
  "http://[a b]/",
  "http://a b/",
  "http://a\tb/",
  "http://a\nb.com/",
  "http://a\\b/c",
  "http:\\\\host\\x",
  "http://host/a b<c>d\"e'f`g{h}i|j^k",
  "http://host/a?b c#d e",
  "http://host/a#b?c",
  "http://host/%",
  "http://host/%2e%2e/x",
  "http://host/a/./b/../c/",
  "http://host/a/b/..",
  "http://host/.",
  "http://host/x.",
  "http://host/.x",
  "http://host/a.b.c",
  "http://host/%E2%98%83.zip",
  "http://host/\u2603.zip",
  "http://bücher.de/x",
  "http://BÜCHER.de/x",
  "http://xn--bcher-kva.de/x",
  "http://xn--a.com/",
  "http://xn--/",
  "http://a\u200db.com/",
  "http://ab\u00adc.com/",
  "http://faß.de/",
  "http://\u0300x.com/",
  "http://x\u0300.com/",
  "http://ｅｘａｍｐｌｅ.com/",
  "http://a\u3002b/",
  "http://\u01c5.com/",
  "http://0x7f.1/",
  "http://1.2.3.4.5/",
  "http://a_b.com/",
  "http://a..b/",
  "http://-a-.com/",
  "http://" + "a".repeat(254) + "/x",
  "http://" + "a".repeat(255) + "/x",
  "http://" + "a".repeat(256) + "/x",
  "http://" + "a".repeat(64) + ".com/",
  "http://a%2eb/",
  "http://a%41b/",
  "\u00a0http://host/x\ufeff",
  "  http://host/x  ",
  "\thttp://host/x\n",
  "x:y",
  "a+b-c.d:rest",
  "1http://x",
  "http:x",
  "https:/x",
  "https:x/y",
  "?q",
  "#h",
  "/abs/path",
  "../rel",
  "rel/x",
  "."
];

const URL_PIECES = [
  "http://", "https://", "HTTPS://", "//", "/", "\\", "unix:", "ftp:", "file:", "javascript:", "x+y.z-w:", "localhost", "localhost:",
  "host", "Host.COM", "bücher", "xn--bcher-kva", "faß", "\u200d", "\u0300", "ｅ", "\u3002", ".", "..", "...", "-", "_", "@", ":", "::",
  "[", "]", "[::1]", "80", "8080", "?", "#", "%", "%20", "%2e", "%E2%98%83", "%zz", "a", "Z", "0", " ", "\t", "\n", "\u00a0", "\ufeff",
  "'", "\"", "<", ">", "{", "}", "|", "^", "`", ";", "=", "&", "+", "~", "!", "*", "(", ")", ",", "$", "\u2603", "\ud83d\ude00",
  "file.zip", "dir/", "a.b", "user:pw@"
];

function randomUrl(): string {
  let s = "";
  const n = 1 + below(8);
  for (let i = 0; i < n; i++) s += pick(URL_PIECES);
  return s;
}

const urlInputs: string[] = [...URL_FIXED];
for (let i = 0; i < count; i++) urlInputs.push(randomUrl());

for (const input of urlInputs) {
  out("urlParse", [input], attempt(() => urlFields(url.parse(input))));
  out("urlParseLax", [input], attempt(() => urlFields(urlParseLax(input))));
  out(
    "urlFormatDefaultProtocol",
    [input],
    attempt(() => {
      const parsed = url.parse(input);
      return url.format(Object.assign({}, parsed, { protocol: parsed.protocol || "http:" }));
    })
  );
  let href: string | null = null;
  try {
    href = urlParseLax(input).href;
  } catch {}
  for (const target of href === null || href === input ? [input] : [input, href]) {
    out(
      "urlPathBasename",
      [target],
      attempt(() => {
        const pathname = new URL(target).pathname;
        return { posix: path.posix.basename(pathname), win32: path.win32.basename(pathname) };
      })
    );
  }
}

const RESOLVE_BASES = [
  "http://a.com/b/c/d;p?q#f",
  "https://github.com/Mechanical-Advantage/AdvantageScopeAssets/releases/download/default-assets-v2/X.zip",
  "http://user:pw@host:8080/a/b",
  "http://host",
  "http://host/",
  "https://[::1]:443/x/",
  "file:///a/b",
  "mailto:a@b",
  "javascript:x",
  "x:y/z"
];
const RESOLVE_RELATIVES = [
  "", "g", "./g", "g/", "/g", "//g", "//g/h", "?y", "g?y", "#s", "g#s", "g?y#s", ";x", "g;x", ".", "./", "..", "../", "../g", "../..",
  "../../", "../../g", "../../../g", "/./g", "/../g", "g.", ".g", "g..", "..g", "./../g", "./g/.", "g/./h", "g/../h", "http:g", "https:g",
  "http://other/x", "HTTPS://OTHER.com", "ftp://f/x", "mailto:z", "javascript:y", "\\\\host\\x", "/\\host", " ../x ", "%2e%2e/x",
  "https://objects.githubusercontent.com/x?y=1", "//bücher.de/x", "http://a b/", "http://[::1/"
];
for (const base of RESOLVE_BASES) for (const rel of RESOLVE_RELATIVES) out("urlResolve", [base, rel], attempt(() => url.resolve(base, rel)));
for (let i = 0; i < count; i++) {
  const base = below(2) === 0 ? pick(RESOLVE_BASES) : randomUrl();
  const rel = below(2) === 0 ? pick(RESOLVE_RELATIVES) : randomUrl();
  out("urlResolve", [base, rel], attempt(() => url.resolve(base, rel)));
}

const PATH_ATOMS = ["", ".", "..", "/", "//", "\\", "a", "b.c", ".d", "e.", "C:", "c:\\", ":", "...", "x.tar.gz", "\\\\srv\\share", "Ü.zip", " "];
const pathInputs: string[] = [...PATH_ATOMS];
for (let i = 0; i < count; i++) {
  let s = "";
  const n = 1 + below(5);
  for (let k = 0; k < n; k++) s += pick(PATH_ATOMS);
  pathInputs.push(s);
}
for (const input of pathInputs) {
  out("pathParts", [input], {
    posix: { dirname: path.posix.dirname(input), basename: path.posix.basename(input), extname: path.posix.extname(input) },
    win32: { dirname: path.win32.dirname(input), basename: path.win32.basename(input), extname: path.win32.extname(input) }
  });
}

const NAME_ATOMS = ["", ".", "..", "!", "!!", "a", "B", "<", ">", ":", "\"", "/", "\\", "|", "?", "*", "\u0000", "\u001f", "\u0080", "\u009f", "\u00a0", " ", "con", "CON", "Com1", "lpt9", "nul", "aux.txt", "\u2603", "\ud83d\ude00", "x".repeat(60), ".zip"];
const names: string[] = [...NAME_ATOMS];
for (let i = 0; i < count; i++) {
  let s = "";
  const n = 1 + below(6);
  for (let k = 0; k < n; k++) s += pick(NAME_ATOMS);
  names.push(s);
}
for (const name of names) out("filenamify", [name], attempt(() => filenamify(name)));

const DISPOSITION_FIXED = [
  "attachment; filename=X.zip",
  "attachment; filename=\"my file.dat\"",
  "attachment;filename=a",
  "inline",
  "attachment",
  "attachment; filename=",
  "attachment; filename=\"\"",
  "attachment; filename*=UTF-8''%E2%98%83.zip",
  "attachment; filename*=utf-8'en'a%20b",
  "attachment; filename*=ISO-8859-1''%E9",
  "attachment; filename*=iso-8859-1''%01%7F%9F%A0a",
  "attachment; filename*=UTF-8'en-USA-abc-def'x",
  "attachment; filename*=UTF-8'en-USA-abc-def-ghi'x",
  "attachment; filename*=UTF-8'abcdefgh'x",
  "attachment; filename*=UTF-8'abcdefghi'x",
  "attachment; filename*=UTF-8'abcd-efg'x",
  "attachment; filename*=UTF-16''x",
  "attachment; filename*=UTF-8''%ZZ",
  "attachment; filename=a; filename*=UTF-8''b",
  "attachment; filename*=UTF-8''b; filename=a",
  "attachment; filename=a; filename=b",
  "attachment; FILENAME=A",
  "attachment; filename=\"a\\\"b\"",
  "attachment; filename=\"a\\",
  "attachment; filename=\"\u00e9\"",
  "attachment; filename=\"\u2603\"",
  "attachment; filename=a b",
  "attachment; filename = a",
  "attachment ; filename=a",
  "attachment;; filename=a",
  "attachment; filename=a;",
  "attachment; foo=bar; filename=x",
  "attachment; filename*=''x",
  "attachment; filename*=UTF-8'toolonglang'x",
  "attachment; filename*=UTF-8'en-US'x",
  "\u00e9; filename=x",
  " attachment; filename=x",
  "attachment\t; filename=x",
  "attachment; filename=\"a\tb\""
];
const DISPOSITION_PIECES = ["attachment", "inline", ";", " ", "\t", "filename", "filename*", "FileName", "=", "\"", "\\", "a", "x.zip", "UTF-8", "iso-8859-1", "''", "'", "en", "%41", "%e2%98%83", "%", "\u00e9", "\u2603", ",", "/", "foo"];
const dispositions = [...DISPOSITION_FIXED];
for (let i = 0; i < count; i++) {
  let s = "";
  const n = 1 + below(10);
  for (let k = 0; k < n; k++) s += pick(DISPOSITION_PIECES);
  dispositions.push(s);
}
for (const header of dispositions) {
  out(
    "contentDisposition",
    [header],
    attempt(() => {
      const filename = contentDisposition.parse(header).parameters.filename;
      return filename === undefined ? null : filename;
    })
  );
}

const MIME_FIXED = [
  "", "application/zip", "application/octet-stream", "application/pdf", "image/png", "image/jpeg", "image/gif", "image/webp", "image/svg+xml",
  "text/plain", "text/plain; charset=utf-8", "text/html", "application/json", "IMAGE/PNG", " image/png", "application/x-zip-compressed",
  "model/gltf-binary", "application/gzip", "application/x-tar", "audio/mpeg", "video/mp4", "application/vnd.ms-excel", "font/woff2", "x/unknown"
];
const mimeDb = createRequire(createRequire(downloadRequire.resolve("ext-name")).resolve("ext-list"))("mime-db");
const mimes = [...MIME_FIXED, ...Object.keys(mimeDb)];
for (const mime of mimes) out("extNameMime", [mime], extName.mime(mime).map((e: { ext: string }) => e.ext));

const zipBytes = (header: number[], filler: number) => {
  const bytes = new Uint8Array(600).fill(filler);
  bytes.set(header);
  return bytes;
};
const ascii = (text: string) => Array.from(Buffer.from(text, "latin1"));
const SIGNATURES: number[][] = [
  [0xff, 0xd8, 0xff], [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a], [0x89, 0x50, 0x4e, 0x47], [0x47, 0x49, 0x46], [0x46, 0x4c, 0x49, 0x46],
  [0x49, 0x49, 0x2a, 0x00, 0, 0, 0, 0, 0x43, 0x52], [0x49, 0x49, 0x52, 0x4f, 0x08, 0, 0, 0, 0x18], [0x49, 0x49, 0x2a, 0, 0x10, 0xfb, 0x86, 0x01],
  [0x49, 0x49, 0x2a, 0, 0x08, 0, 0, 0, 0x2d], [0x49, 0x49, 0x2a, 0, 0x30, 0x3d, 0x72, 0x01, 0x1c], [0x4d, 0x4d, 0, 0x2a], [0x42, 0x4d], [0x49, 0x49, 0xbc],
  [0x38, 0x42, 0x50, 0x53], [0x50, 0x4b, 0x03, 0x04], [0x50, 0x4b, 0x05, 0x06], [0x50, 0x4b, 0x07, 0x08], [0x50, 0x4b, 0x03, 0x06], [0x50, 0x4b, 0x01, 0x02],
  [0x52, 0x61, 0x72, 0x21, 0x1a, 0x07, 0x00], [0x52, 0x61, 0x72, 0x21, 0x1a, 0x07, 0x01], [0x52, 0x61, 0x72, 0x21, 0x1a, 0x07, 0x02], [0x1f, 0x8b, 0x08],
  [0x42, 0x5a, 0x68], [0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c], [0x78, 0x01], [0, 0, 0, 0, ...ascii("free")], [0, 0, 0, 0, ...ascii("mdat")], [0, 0, 0, 0, ...ascii("moov")],
  [0, 0, 0, 0, ...ascii("wide")], [0, 0, 0, 0x18, ...ascii("ftypmif1")], [0, 0, 0, 0x18, ...ascii("ftypmsf1")], [0, 0, 0, 0x18, ...ascii("ftypheic")],
  [0, 0, 0, 0x18, ...ascii("ftyphevx")], [0, 0, 0, 0x18, ...ascii("ftypqt  ")], [0, 0, 0, 0x18, ...ascii("ftypM4V ")], [0, 0, 0, 0x18, ...ascii("ftypM4P ")],
  [0, 0, 0, 0x18, ...ascii("ftypM4B ")], [0, 0, 0, 0x18, ...ascii("ftypM4A ")], [0, 0, 0, 0x18, ...ascii("ftypF4V ")], [0, 0, 0, 0x18, ...ascii("ftypF4P ")],
  [0, 0, 0, 0x18, ...ascii("ftypF4A ")], [0, 0, 0, 0x18, ...ascii("ftypF4B ")], [0, 0, 0, 0x18, ...ascii("ftyp3gp5")], [0, 0, 0, 0x18, ...ascii("ftyp3g2a")],
  [0, 0, 0, 0x18, ...ascii("ftypisom")], [0, 0, 0, 0x18, ...ascii("ftyp\u0000\u0000\u0000\u0000")], [0x4d, 0x54, 0x68, 0x64],
  [0x1a, 0x45, 0xdf, 0xa3, 0x42, 0x82, 0x88, ...ascii("matroska")], [0x1a, 0x45, 0xdf, 0xa3, 0x01, 0x42, 0x82, 0x84, ...ascii("webm")], [0x1a, 0x45, 0xdf, 0xa3],
  [...ascii("RIFF"), 0, 0, 0, 0, ...ascii("AVI ")], [...ascii("RIFF"), 0, 0, 0, 0, ...ascii("WAVE")], [...ascii("RIFF"), 0, 0, 0, 0, ...ascii("QLCM")], [...ascii("RIFF")],
  [0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9], [0, 0, 1, 0xba], [0, 0, 1, 0xb3], [...ascii("ID3")], [0xff, 0xe2], [0xff, 0xe4], [0xff, 0xf8],
  [0xff, 0xf0], [0x00, 0xff, 0xfb], [...ascii("OggS")], [...ascii("fLaC")], [...ascii("MAC ")], [...ascii("wvpk")], [...ascii("#!AMR\n")], [...ascii("%PDF-1.4")],
  [...ascii("MZ")], [...ascii("CWS")], [...ascii("FWS")], [...ascii("{\\rtf")], [0, ...ascii("asm")], [...ascii("wOFF"), 0, 1, 0, 0], [...ascii("wOFFOTTO")],
  [...ascii("wOF2"), 0, 1, 0, 0], [...ascii("wOF2OTTO")], [0, 1, 0, 0, 0], [...ascii("OTTO"), 0], [0, 0, 1, 0], [0, 0, 2, 0], [...ascii("FLV"), 1], [...ascii("%!")],
  [0xfd, ...ascii("7zXZ"), 0], [...ascii("SQLi")], [...ascii("NES"), 0x1a], [...ascii("Cr24")], [...ascii("MSCF")], [...ascii("ISc(")],
  [...ascii("!<arch>\ndebian-binary")], [...ascii("!<arch>")], [0xed, 0xab, 0xee, 0xdb], [0x1f, 0xa0], [0x1f, 0x9d], [...ascii("LZIP")],
  [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1], [0x06, 0x0e, 0x2b, 0x34, 0x02, 0x05, 0x01, 0x01, 0x0d, 0x01, 0x02, 0x01, 0x01, 0x02], [...ascii("BLENDER")],
  [...ascii("BPG"), 0xfb], [0, 0, 0, 0x0c, 0x6a, 0x50, 0x20, 0x20, 0x0d, 0x0a, 0x87, 0x0a], [...ascii("FORM")], [...ascii("<?xml ")], [...ascii("MP+")], [...ascii("MPCK")],
  [...ascii("BEGIN:")], [...ascii("glTF"), 2, 0, 0, 0], [0xd4, 0xc3, 0xb2, 0xa1], [0xa1, 0xb2, 0xc3, 0xd4], [...ascii("DSD ")],
  [0x4c, 0, 0, 0, 0x01, 0x14, 0x02, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46], [...ascii("book"), 0, 0, 0, 0, ...ascii("mark"), 0, 0, 0, 0],
  [...ascii("Creative Voice File")], [0x0b, 0x77], [0xab, ...ascii("KTX 11"), 0xbb, 0x0d, 0x0a, 0x1a, 0x0a]
];
const OFFSET_PATCHES: [number, number[]][] = [
  [8, ascii("WEBP")], [30, ascii("mimetypeapplication/epub+zip")], [30, ascii("META-INF/mozilla.rsa")], [30, ascii("mimetypeapplication/vnd.oasis.opendocument.text")],
  [30, ascii("mimetypeapplication/vnd.oasis.opendocument.spreadsheet")], [30, ascii("mimetypeapplication/vnd.oasis.opendocument.presentation")],
  [30, ascii("[Content_Types].xml")], [30, ascii("_rels/.rels")], [30, ascii("word/")], [30, ascii("ppt/")], [30, ascii("xl/")], [28, ascii("OpusHead")],
  [28, [0x80, ...ascii("theora")]], [28, [0x01, ...ascii("video"), 0]], [28, [0x7f, ...ascii("FLAC")]], [28, ascii("Speex  ")], [28, [0x01, ...ascii("vorbis")]],
  [34, ascii("LP")], [8, [0, 0, 1]], [8, [1, 0, 2]], [4, [0x47]], [192, [0x47]], [196, [0x47]], [60, ascii("BOOKMOBI")], [128, ascii("DICM")], [257, ascii("ustar")],
  [20, ascii("jp2 ")], [20, ascii("jpx ")], [20, ascii("jpm ")], [20, ascii("mj2 ")]
];

function tarHeader(valid: boolean, signed: boolean): Uint8Array {
  const bytes = new Uint8Array(600);
  bytes.set(ascii("file.txt"), 0);
  if (signed) bytes[10] = 0x90;
  let sum = 256;
  let signedSum = 0;
  for (let i = 0; i < 512; i++) {
    if (i >= 148 && i < 156) continue;
    sum += bytes[i];
    signedSum += bytes[i] & 0x80;
  }
  const value = (signed ? sum - (signedSum << 1) : sum) + (valid ? 0 : 1);
  bytes.set(ascii(value.toString(8).padStart(6, "0")), 148);
  return bytes;
}

function asfFile(kind: number): Uint8Array {
  const bytes = new Uint8Array(200);
  bytes.set([0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9]);
  bytes.set([0x91, 0x07, 0xdc, 0xb7, 0xb7, 0xa9, 0xcf, 0x11, 0x8e, 0xe6, 0x00, 0xc0, 0x0c, 0x20, 0x53, 0x65], 30);
  bytes.set([40, 0, 0, 0, 0, 0, 0, 0], 46);
  const media = kind === 0
    ? [0x40, 0x9e, 0x69, 0xf8, 0x4d, 0x5b, 0xcf, 0x11, 0xa8, 0xfd, 0x00, 0x80, 0x5f, 0x5c, 0x44, 0x2b]
    : kind === 1
      ? [0xc0, 0xef, 0x19, 0xbc, 0x4d, 0x5b, 0xcf, 0x11, 0xa8, 0xfd, 0x00, 0x80, 0x5f, 0x5c, 0x44, 0x2b]
      : [1, 2, 3];
  bytes.set(media, 54);
  return bytes;
}

const typeInputs: Uint8Array[] = [new Uint8Array(), new Uint8Array([0x50]), new Uint8Array([0x50, 0x4b]), new Uint8Array([0x50, 0x4b, 0x03]), new Uint8Array([0xff, 0xd8])];
// file-type 11's ASF scan never advances past an object whose 64-bit size reads as 0, so an ASF
// header followed by zeros would hang the generator; zero fill is skipped for that signature and
// random inputs never start with it (the hand-built ASF files below terminate).
const ASF = [0x30, 0x26, 0xb2, 0x75];
const startsWithAsf = (bytes: ArrayLike<number>) => ASF.every((b, i) => bytes[i] === b);
for (const sig of SIGNATURES) {
  typeInputs.push(new Uint8Array(sig));
  if (!startsWithAsf(sig)) typeInputs.push(zipBytes(sig, 0));
  typeInputs.push(zipBytes(sig, 0x20));
}
for (const [offset, patch] of OFFSET_PATCHES) {
  for (const head of [[0x50, 0x4b, 0x03, 0x04], [...ascii("OggS")], [0, 0, 0, 0x0c, 0x6a, 0x50, 0x20, 0x20, 0x0d, 0x0a, 0x87, 0x0a], [0x50, 0x4b, 0x05, 0x06], []]) {
    const bytes = zipBytes(head, 0);
    bytes.set(patch, offset);
    typeInputs.push(bytes);
  }
}
for (const valid of [true, false]) for (const signed of [true, false]) typeInputs.push(tarHeader(valid, signed));
for (let kind = 0; kind < 3; kind++) typeInputs.push(asfFile(kind));
{
  const docx = new Uint8Array(200);
  docx.set([0x50, 0x4b, 0x03, 0x04]);
  docx.set(ascii("[Content_Types].xml"), 30);
  docx.set([0x50, 0x4b, 0x03, 0x04], 100);
  docx.set(ascii("word/document.xml"), 130);
  typeInputs.push(docx);
  const pptx = new Uint8Array(200);
  pptx.set([0x50, 0x4b, 0x03, 0x04]);
  pptx.set(ascii("ppt/x"), 30);
  typeInputs.push(pptx);
  const xlsxLate = new Uint8Array(4200);
  xlsxLate.set([0x50, 0x4b, 0x03, 0x04]);
  xlsxLate.set([0x50, 0x4b, 0x03, 0x04], 4100);
  xlsxLate.set(ascii("xl/"), 4130);
  xlsxLate.set([0x50, 0x4b, 0x03, 0x04], 4150);
  xlsxLate.set(ascii("_rels/.rels"), 4180);
  typeInputs.push(xlsxLate);
  const mkvFar = new Uint8Array(5000);
  mkvFar.set([0x1a, 0x45, 0xdf, 0xa3]);
  mkvFar.set([0x42, 0x82, 0x88, ...ascii("matroska")], 4097);
  typeInputs.push(mkvFar);
}
const BYTE_POOL = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x1a, 0x1f, 0x20, 0x25, 0x30, 0x42, 0x47, 0x49, 0x4b, 0x4d, 0x50, 0x52, 0x57, 0x78, 0x7a, 0x80, 0x8b, 0xa1, 0xb3, 0xbc, 0xd8, 0xe2, 0xff];
for (let i = 0; i < count * 2; i++) {
  const base = below(3) === 0 ? pick(SIGNATURES) : [];
  const bytes = new Uint8Array(below(4) === 0 ? below(16) : 64 + below(600));
  for (let k = 0; k < bytes.length; k++) bytes[k] = below(4) === 0 ? below(256) : pick(BYTE_POOL);
  bytes.set(base.slice(0, bytes.length));
  if (startsWithAsf(bytes)) bytes[0] = 0x31;
  if (bytes.length > 40 && below(3) === 0) {
    const [offset, patch] = pick(OFFSET_PATCHES);
    if (offset + patch.length <= bytes.length) bytes.set(patch, offset);
  }
  typeInputs.push(bytes);
}
for (const bytes of typeInputs) {
  const ft = fileType(bytes);
  const at = archiveType(bytes);
  out("fileType", [hex(bytes)], { fileType: ft ? ft.ext : null, archiveType: at ? at.ext : null });
}

// Zip archives for decompress-unzip: well-formed ones built here, then byte-level mutations.
type ZipSpec = { name: Uint8Array; data: Uint8Array; method: 0 | 8; flags: number; madeBy: number; external: number; extra: Uint8Array; crcOverride?: number };

const crcTable = (() => {
  const table = new Uint32Array(256);
  for (let i = 0; i < 256; i++) {
    let c = i;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[i] = c >>> 0;
  }
  return table;
})();
const crc32 = (bytes: Uint8Array) => {
  let c = 0xffffffff;
  for (const b of bytes) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};

function buildZip(entries: ZipSpec[], comment = new Uint8Array()): Uint8Array {
  const locals: Buffer[] = [];
  const centrals: Buffer[] = [];
  let offset = 0;
  for (const e of entries) {
    const stored = e.method === 8 ? zlib.deflateRawSync(e.data) : Buffer.from(e.data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(e.flags, 6);
    local.writeUInt16LE(e.method, 8);
    local.writeUInt32LE(crc32(e.data), 14);
    local.writeUInt32LE(stored.length, 18);
    local.writeUInt32LE(e.data.length, 22);
    local.writeUInt16LE(e.name.length, 26);
    local.writeUInt16LE(0, 28);
    locals.push(local, Buffer.from(e.name), stored);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(e.madeBy, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(e.flags, 8);
    central.writeUInt16LE(e.method, 10);
    central.writeUInt32LE(crc32(e.data), 16);
    central.writeUInt32LE(stored.length, 20);
    central.writeUInt32LE(e.data.length, 24);
    central.writeUInt16LE(e.name.length, 28);
    central.writeUInt16LE(e.extra.length, 30);
    central.writeUInt32LE(e.external >>> 0, 38);
    central.writeUInt32LE(offset, 42);
    centrals.push(central, Buffer.from(e.name), Buffer.from(e.extra));
    offset += 30 + e.name.length + stored.length;
  }
  const cd = Buffer.concat(centrals);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(entries.length, 8);
  eocd.writeUInt16LE(entries.length, 10);
  eocd.writeUInt32LE(cd.length, 12);
  eocd.writeUInt32LE(offset, 16);
  eocd.writeUInt16LE(comment.length, 20);
  return new Uint8Array(Buffer.concat([...locals, cd, eocd, Buffer.from(comment)]));
}

const ZIP_NAMES = ["a.txt", "dir/", "dir/b.bin", "x\\y.txt", "../evil", "a/../b", "/abs", "C:x", "c:", "\u00fc.txt", "sp ace", "", "dir//c", "./d", "a/./b", "....", "con", "x:y"];
function randomEntry(): ZipSpec {
  const utf8 = below(2) === 0;
  let name: Uint8Array;
  if (below(4) === 0) {
    name = new Uint8Array(1 + below(6));
    for (let k = 0; k < name.length; k++) name[k] = below(3) === 0 ? 0x80 + below(128) : 0x61 + below(26);
  } else {
    const text = pick(ZIP_NAMES);
    name = new Uint8Array(Buffer.from(text, utf8 ? "utf8" : "latin1"));
  }
  const data = new Uint8Array(below(3) === 0 ? 0 : below(80));
  for (let k = 0; k < data.length; k++) data[k] = below(256);
  const unixMode = pick([0, 0o100644, 0o100755, 0o040755, 0o120777, 0o100000]);
  const madeBy = pick([0x0314, 0x0014, 0x0000, 0x0b14]);
  const external = below(4) === 0 ? pick([16, 0x10, 0x20, 0]) : (unixMode << 16) >>> 0;
  let extra = new Uint8Array();
  if (below(5) === 0) {
    const unicode = Buffer.from(pick(["\u2603.txt", "uni/\u00e9", "../up"]), "utf8");
    const field = Buffer.alloc(9 + unicode.length);
    field.writeUInt16LE(0x7075, 0);
    field.writeUInt16LE(5 + unicode.length, 2);
    field.writeUInt8(below(4) === 0 ? 2 : 1, 4);
    field.writeUInt32LE(below(4) === 0 ? 0 : crc32(name), 5);
    unicode.copy(field, 9);
    extra = new Uint8Array(field);
  }
  return { name, data, method: below(2) === 0 ? 0 : 8, flags: utf8 ? 0x800 : 0, madeBy, external, extra };
}

const zipInputs: Uint8Array[] = [
  buildZip([]),
  buildZip([{ name: new Uint8Array(Buffer.from("a.txt")), data: new Uint8Array(Buffer.from("hello")), method: 8, flags: 0, madeBy: 0x0314, external: (0o100644 << 16) >>> 0, extra: new Uint8Array() }]),
  buildZip([], new Uint8Array(Buffer.from("comment"))),
  new Uint8Array([0x50, 0x4b, 0x05, 0x06]),
  new Uint8Array([0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
  new Uint8Array([0x50, 0x4b, 0x03, 0x04])
];
for (let i = 0; i < count; i++) {
  const entries: ZipSpec[] = [];
  const n = below(4);
  for (let k = 0; k < n; k++) entries.push(randomEntry());
  let bytes = buildZip(entries, below(5) === 0 ? new Uint8Array([1, 2, 3]) : new Uint8Array());
  if (below(2) === 0 && bytes.length > 0) {
    bytes = new Uint8Array(bytes);
    const flips = 1 + below(3);
    for (let k = 0; k < flips; k++) {
      const choice = below(3);
      if (choice === 0) bytes[below(bytes.length)] = below(256);
      else if (choice === 1) bytes = bytes.slice(0, below(bytes.length + 1));
      else bytes[below(bytes.length)] ^= 1 << below(8);
    }
  }
  zipInputs.push(bytes);
}

async function unzipAll() {
  const plugin = decompressUnzip();
  for (const bytes of zipInputs) {
    let value: unknown;
    try {
      const files = await plugin(Buffer.from(bytes));
      value = files.map((f: any) => ({ path: f.path, type: f.type, mode: f.mode, data: hex(f.data) }));
    } catch {
      value = { throws: true };
    }
    out("unzip", [hex(bytes)], value);
  }
}

await unzipAll();
