import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  socialCard,
  textWidth,
  wrapTitle,
  writeSocialCard,
} from "./social-images.mjs";

test("long headings and command names stay out of the logo column", () => {
  for (const title of [
    "W".repeat(120),
    "Exact, all-or-nothing edits across many files at once",
    "jouet edit --revert-on-fail --expect-hash",
  ]) {
    const lines = wrapTitle(title);
    assert.ok(lines.length > 0);
    for (const line of lines) assert.ok(textWidth(line, 62) <= 720, line);
  }
});

test("escapes title markup and gives changed content a new image URL", () => {
  const first = socialCard('Edit <spec> & "check"');
  assert.match(first.svg, /Edit &lt;spec&gt; &amp;/);
  assert.ok(!first.svg.includes("<spec>"));
  assert.equal(first.path, socialCard('Edit <spec> & "check"').path);
  assert.notEqual(first.path, socialCard("Another page").path);
});

test("renders a self-contained 1200 by 630 PNG", () => {
  const dir = mkdtempSync(join(tmpdir(), "jouet-social-"));
  try {
    const card = socialCard("Get started");
    writeSocialCard(dir, card);
    const png = readFileSync(join(dir, card.path));
    assert.deepEqual(
      [...png.subarray(0, 8)],
      [137, 80, 78, 71, 13, 10, 26, 10],
    );
    assert.equal(png.readUInt32BE(16), 1200);
    assert.equal(png.readUInt32BE(20), 630);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
