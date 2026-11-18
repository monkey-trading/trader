// Run with `node --test scripts/check-audit.test.mjs`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { URL } from "node:url";
import { evaluate, entryProblems } from "./check-audit.mjs";

const ROOT = "GHSA-aaaa-bbbb-cccc";
const OTHER = "GHSA-dddd-eeee-ffff";
const adv = (id, severity) => ({ severity, url: `https://github.com/advisories/${id}`, source: 1 });
const clone = (x) => JSON.parse(JSON.stringify(x));
const entry = (over = {}) => ({ id: ROOT, reason: "r", expires: "2026-12-18", issue: "o/r#1", ...over });

// `leaf` depends on `mid`, which depends on `root`, whose advisory is ROOT.
const chain = {
  vulnerabilities: {
    root: { severity: "high", via: [adv(ROOT, "high")] },
    mid: { severity: "high", via: ["root"] },
    leaf: { severity: "high", via: ["mid"] },
  },
};

test("an excepted advisory and its dependents pass", () => {
  assert.deepEqual(evaluate(chain, [entry()], "2026-11-18").failures, []);
});

test("without the entry the whole chain fails", () => {
  const { failures } = evaluate(chain, [], "2026-11-18");
  assert.equal(failures.length, 3);
});

test("a new unrelated high advisory still fails", () => {
  const audit = clone(chain);
  audit.vulnerabilities.other = { severity: "high", via: [adv(OTHER, "high")] };
  const { failures } = evaluate(audit, [entry()], "2026-11-18");
  assert.equal(failures.length, 1);
  assert.match(failures[0], new RegExp(`^other .*${OTHER}`));
});

test("a critical advisory is not excepted by a high one's entry", () => {
  const audit = { vulnerabilities: { x: { severity: "critical", via: [adv(OTHER, "critical")] } } };
  assert.equal(evaluate(audit, [entry()], "2026-11-18").failures.length, 1);
});

test("a package that also reaches an unrelated high advisory fails", () => {
  const audit = clone(chain);
  audit.vulnerabilities.other = { severity: "high", via: [adv(OTHER, "high")] };
  audit.vulnerabilities.leaf.via = ["mid", "other"];
  const { failures } = evaluate(audit, [entry()], "2026-11-18");
  assert.equal(failures.length, 2);
  assert.ok(failures.some((f) => f.startsWith("leaf ")));
});

test("a moderate advisory beside the excepted one does not fail a package", () => {
  const audit = clone(chain);
  audit.vulnerabilities.mod = { severity: "moderate", via: [adv(OTHER, "moderate")] };
  audit.vulnerabilities.leaf.via = ["mid", "mod"];
  assert.deepEqual(evaluate(audit, [entry()], "2026-11-18").failures, []);
});

test("a moderate package is not a failure", () => {
  const audit = { vulnerabilities: { m: { severity: "moderate", via: [adv(OTHER, "moderate")] } } };
  assert.deepEqual(evaluate(audit, [], "2026-11-18").failures, []);
});

test("an expired entry fails, naming it, and no longer covers", () => {
  const { failures } = evaluate(chain, [entry()], "2026-12-19");
  assert.ok(failures[0].startsWith(`${ROOT}: the exception expired on 2026-12-18`));
  assert.equal(failures.length, 4);
});

test("an entry still holds on its expiry date", () => {
  assert.deepEqual(evaluate(chain, [entry()], "2026-12-18").failures, []);
});

test("a malformed entry fails", () => {
  assert.equal(entryProblems([entry({ id: "picomatch" })]).length, 1);
  assert.equal(entryProblems([entry({ reason: " " })]).length, 1);
  assert.equal(entryProblems([entry({ expires: "soon" })]).length, 1);
  assert.equal(entryProblems([entry({ issue: "" })]).length, 1);
  assert.equal(entryProblems([entry(), entry()]).length, 1);
});

test("a cycle in via terminates", () => {
  const audit = { vulnerabilities: { a: { severity: "high", via: ["b"] }, b: { severity: "high", via: ["a", adv(ROOT, "high")] } } };
  assert.deepEqual(evaluate(audit, [entry()], "2026-11-18").failures, []);
});

test("the committed exceptions file is well formed", () => {
  const { exceptions } = JSON.parse(readFileSync(new URL("./audit-exceptions.json", import.meta.url), "utf8"));
  assert.deepEqual(entryProblems(exceptions), []);
});
