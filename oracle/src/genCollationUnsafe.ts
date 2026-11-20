import "./shims";

// Recovers the two code-unit sets ICU4C consults when it skips the identical prefix of two
// UTF-16 strings (RuleBasedCollator::doCompare), because in numeric collation that skip can split
// a digit run and so changes results:
//   {"digits":[[lo,hi],...]}  units ICU's isDigit() accepts: ASCII digits below U+0660, BMP Nd above
//   {"unsafe":[[lo,hi],...]}  the other units of isUnsafeBackward(c, numeric = true)
// A unit x is unsafe when, after a common "9" or "0", it makes ICU back up over the digit even
// though the other string continues with a supplementary digit whose lead surrogate is safe
// (U+1D7F5 / U+1D7CE, lead U+D835): backing up joins the digits into one number, not backing up
// compares x alone against the supplementary digit. Every unit must classify one way only.

const collator = new Intl.Collator(undefined, { numeric: true });
const locale = collator.resolvedOptions().locale;
if (locale !== "en-US") {
  process.stderr.write(`genCollationUnsafe: default collation locale is ${locale}, expected en-US\n`);
  process.exit(1);
}

const nd = /^\p{Nd}$/u;
const isIcuDigit = (u: number) => (u < 0x660 ? u >= 0x30 && u <= 0x39 : u < 0xd800 || u > 0xdfff ? nd.test(String.fromCharCode(u)) : false);

const ranges = (members: number[]) => {
  const out: [number, number][] = [];
  for (const u of members) {
    const last = out[out.length - 1];
    if (last !== undefined && last[1] + 1 === u) last[1] = u;
    else out.push([u, u]);
  }
  return out;
};

const digits: number[] = [];
const unsafe: number[] = [];
for (let u = 0; u <= 0xffff; u++) {
  if (isIcuDigit(u)) {
    digits.push(u);
    continue;
  }
  const x = String.fromCharCode(u);
  const nine = Math.sign(collator.compare("9" + x, "9𝟵"));
  const zero = Math.sign(collator.compare("0" + x, "0𝟎"));
  const backedUp = nine === -1 && zero !== -1;
  const stayed = nine === 1 || zero === -1;
  if (backedUp === stayed) {
    process.stderr.write(`genCollationUnsafe: U+${u.toString(16)} does not classify (${nine}, ${zero})\n`);
    process.exit(1);
  }
  if (backedUp) unsafe.push(u);
}

process.stdout.write(JSON.stringify({ digits: ranges(digits) }) + "\n");
process.stdout.write(JSON.stringify({ unsafe: ranges(unsafe) }) + "\n");
