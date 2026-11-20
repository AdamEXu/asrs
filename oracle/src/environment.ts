export const ZONE_VARIABLE = "ASRS_ORACLE_ZONE";
export const LOCALE = "en-US";
export const POSIX_LOCALE = "en_US.UTF-8";

export function policyViolations(): string[] {
  const zone = process.env[ZONE_VARIABLE];
  if (zone === undefined) {
    return [`${ZONE_VARIABLE} is not set; run oracle entries through \`cargo xtask oracle\``];
  }
  const problems: string[] = [];
  const expectVar = (name: string, want: string) => {
    if (process.env[name] !== want) problems.push(`${name} is ${JSON.stringify(process.env[name])}, expected ${JSON.stringify(want)}`);
  };
  expectVar("TZ", zone);
  expectVar("LANG", POSIX_LOCALE);
  expectVar("LC_ALL", POSIX_LOCALE);

  let canonicalZone: string | undefined;
  try {
    canonicalZone = new Intl.DateTimeFormat(LOCALE, { timeZone: zone }).resolvedOptions().timeZone;
  } catch {
    problems.push(`${ZONE_VARIABLE} names an unknown time zone ${JSON.stringify(zone)}`);
  }
  const resolved = Intl.DateTimeFormat().resolvedOptions();
  if (canonicalZone !== undefined && resolved.timeZone !== canonicalZone) {
    problems.push(`default time zone resolves to ${JSON.stringify(resolved.timeZone)}, expected ${JSON.stringify(canonicalZone)}`);
  }
  const locales: [string, string][] = [
    ["Intl.DateTimeFormat", resolved.locale],
    ["Intl.Collator", new Intl.Collator().resolvedOptions().locale],
    ["Intl.NumberFormat", new Intl.NumberFormat().resolvedOptions().locale]
  ];
  for (const [name, locale] of locales) {
    if (locale !== LOCALE) problems.push(`${name} default locale is ${JSON.stringify(locale)}, expected ${JSON.stringify(LOCALE)}`);
  }
  return problems;
}

const problems = policyViolations();
if (problems.length > 0) {
  process.stderr.write(`oracle environment policy violated:\n  ${problems.join("\n  ")}\n`);
  process.exit(78);
}
