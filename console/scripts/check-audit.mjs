// Fails on every high or critical advisory in `npm audit` except those an unexpired entry of
// audit-exceptions.json names. An entry covers a package only when every high or critical
// advisory reaching it, through its `via` chain, is an excepted one.
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import process from "node:process";
import { fileURLToPath, URL } from "node:url";

const SEVERE = new Set(["high", "critical"]);
const GHSA = /^GHSA(-[0-9a-z]{4}){3}$/;
const DATE = /^\d{4}-\d{2}-\d{2}$/;

/** Returns the problems with the exceptions file's entries. */
export function entryProblems(exceptions) {
  const problems = [];
  const seen = new Set();
  for (const [i, e] of exceptions.entries()) {
    const at = `exceptions[${i}]`;
    if (typeof e?.id !== "string" || !GHSA.test(e.id)) problems.push(`${at}: id is not a GHSA id`);
    else if (seen.has(e.id)) problems.push(`${at}: ${e.id} is listed twice`);
    else seen.add(e.id);
    if (typeof e?.reason !== "string" || e.reason.trim() === "") problems.push(`${at}: no reason`);
    if (typeof e?.expires !== "string" || !DATE.test(e.expires) || Number.isNaN(Date.parse(e.expires))) {
      problems.push(`${at}: expires is not a YYYY-MM-DD date`);
    }
    if (typeof e?.issue !== "string" || !/#\d+$/.test(e.issue)) problems.push(`${at}: no tracking issue`);
  }
  return problems;
}

/** The ids of the high or critical advisories a package's `via` chain reaches. */
function severeAdvisories(vulnerabilities, name, seen = new Set()) {
  const ids = new Set();
  if (seen.has(name)) return ids;
  seen.add(name);
  for (const via of vulnerabilities[name]?.via ?? []) {
    if (typeof via === "string") {
      for (const id of severeAdvisories(vulnerabilities, via, seen)) ids.add(id);
    } else if (SEVERE.has(via.severity)) {
      ids.add(via.url?.split("/").pop() ?? `advisory ${via.source}`);
    }
  }
  return ids;
}

/**
 * Checks `npm audit --json` output against the exceptions on `today` (YYYY-MM-DD).
 * Returns the failures and the notes to print.
 */
export function evaluate(audit, exceptions, today) {
  const failures = entryProblems(exceptions);
  const notes = [];
  const active = new Set();
  for (const e of exceptions) {
    if (typeof e?.expires === "string" && e.expires < today) {
      failures.push(`${e.id}: the exception expired on ${e.expires} (${e.issue}); remove it or renew it`);
    } else if (typeof e?.id === "string") {
      active.add(e.id);
    }
  }
  const vulnerabilities = audit.vulnerabilities ?? {};
  const used = new Set();
  for (const [name, v] of Object.entries(vulnerabilities)) {
    if (!SEVERE.has(v.severity)) continue;
    const ids = severeAdvisories(vulnerabilities, name);
    const open = [...ids].filter((id) => !active.has(id));
    for (const id of ids) if (active.has(id)) used.add(id);
    if (ids.size === 0 || open.length > 0) {
      failures.push(`${name} (${v.severity}): ${open.length > 0 ? open.join(", ") : "no advisory found for it"}`);
    } else {
      notes.push(`${name} (${v.severity}) is covered by ${[...ids].join(", ")}`);
    }
  }
  for (const id of active) if (!used.has(id)) notes.push(`${id} matches no advisory now; remove its entry`);
  return { failures, notes };
}

function main() {
  const file = fileURLToPath(new URL("./audit-exceptions.json", import.meta.url));
  const { exceptions } = JSON.parse(readFileSync(file, "utf8"));
  // npm exits non-zero when it finds advisories; the report is still on stdout.
  const run = spawnSync("npm", ["audit", "--json"], { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  let audit;
  try {
    audit = JSON.parse(run.stdout);
  } catch {
    process.stderr.write(`npm audit gave no report (exit ${run.status}): ${run.stderr}\n`);
    process.exit(1);
  }
  if (audit.error) {
    process.stderr.write(`npm audit failed: ${audit.error.summary ?? JSON.stringify(audit.error)}\n`);
    process.exit(1);
  }
  const today = new Date().toISOString().slice(0, 10);
  const { failures, notes } = evaluate(audit, exceptions, today);
  for (const n of notes) process.stdout.write(`${n}\n`);
  if (failures.length > 0) {
    process.stderr.write(`\n${failures.length} problem(s):\n`);
    for (const f of failures) process.stderr.write(`- ${f}\n`);
    process.exit(1);
  }
  process.stdout.write("npm audit: no high or critical advisory outside the exceptions\n");
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();
