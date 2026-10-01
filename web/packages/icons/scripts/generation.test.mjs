import assert from "node:assert/strict";
import { test } from "node:test";
import { cpSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../../../..");

function run(script, fixture, check = false) {
  return spawnSync(process.execPath, [join(here, script), "--repo-root", fixture, ...(check ? ["--check"] : [])], { encoding: "utf8" });
}

function success(result) {
  assert.equal(result.status, 0, result.stderr);
}

function snapshot(dir, prefix = "") {
  return Object.fromEntries(readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name)).flatMap((entry) => {
    const path = join(dir, entry.name);
    const name = `${prefix}${entry.name}`;
    return entry.isDirectory() ? Object.entries(snapshot(path, `${name}/`)) : [[name, readFileSync(path).toString("base64")]];
  }));
}

// Keep disposable fixtures inside this project; never touch shared source.
function fixture(t) {
  const parent = join(root, "target", "icon-generation-fixtures");
  mkdirSync(parent, { recursive: true });
  const dir = mkdtempSync(join(parent, "test-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

test("controls regenerate deterministically, include current glyphs and detect changed source", (t) => {
  const dir = fixture(t);
  cpSync(join(root, "crates/ui/assets/icons"), join(dir, "crates/ui/assets/icons"), { recursive: true });
  const script = "generate.mjs";
  success(run(script, dir));
  success(run(script, dir, true));
  const generated = join(dir, "web/packages/icons/src/generated");
  const original = snapshot(generated);
  const module = readFileSync(join(generated, "index.ts"), "utf8");
  assert.match(module, /fastTier:/);
  assert.match(module, /magicStick3:/);
  for (const name of ["fastTier", "moreHorizontal", "projectDefault"]) {
    const entry = module.split("\n").find(line => line.startsWith(`  ${name}:`));
    assert.ok(entry, `${name} must be exported`);
    assert.ok(!entry.includes('\\"#000\\"') && !entry.includes('\\"black\\"'), `${name} must tint, not disappear on dark surfaces`);
    assert.ok(entry.includes("currentColor"), `${name} must inherit foreground color`);
  }
  success(run(script, dir));
  assert.deepEqual(snapshot(generated), original);

  const asset = join(dir, "crates/ui/assets/icons/fast-tier.svg");
  writeFileSync(asset, readFileSync(asset, "utf8").replace("</svg>", '<circle cx="3" cy="3" r="1" fill="currentColor"/></svg>'));
  assert.equal(run(script, dir, true).status, 1);
  assert.deepEqual(snapshot(generated), original, "--check must not write");
  success(run(script, dir));
  assert.notDeepEqual(snapshot(generated), original);
  success(run(script, dir, true));
});

test("file artwork regenerates deterministically and detects changed source, manifest and output", (t) => {
  const dir = fixture(t);
  cpSync(join(root, "crates/ui/assets/file-icons"), join(dir, "crates/ui/assets/file-icons"), { recursive: true });
  mkdirSync(join(dir, "crates/ui/src"), { recursive: true });
  const manifest = join(dir, "crates/ui/src/file-icons.json");
  cpSync(join(root, "crates/ui/src/file-icons.json"), manifest);
  const script = "generate-file-icons.mjs";
  success(run(script, dir));
  success(run(script, dir, true));
  const generated = join(dir, "web");
  const original = snapshot(generated);
  success(run(script, dir));
  assert.deepEqual(snapshot(generated), original);

  const sourceDir = join(dir, "crates/ui/assets/file-icons");
  const rel = Object.keys(snapshot(sourceDir)).find((name) => name.endsWith(".svg"));
  const source = join(sourceDir, rel);
  writeFileSync(source, readFileSync(source, "utf8").replace("</svg>", '<circle cx="3" cy="3" r="1" fill="#64748B"/></svg>'));
  assert.equal(run(script, dir, true).status, 1);
  assert.deepEqual(snapshot(generated), original);
  success(run(script, dir));
  assert.notDeepEqual(snapshot(generated), original);
  const light = readFileSync(join(dir, "web/packages/app/public/file-icons", rel), "utf8");
  const dark = readFileSync(join(dir, "web/packages/app/public/file-icons/dark", rel), "utf8");
  assert.match(light, /fill="#64748B"/);
  assert.match(dark, /fill="#CBD5E1"/);
  success(run(script, dir, true));

  const data = JSON.parse(readFileSync(manifest, "utf8"));
  data.fileNames["ticket06-fixture"] = data.file;
  writeFileSync(manifest, JSON.stringify(data));
  assert.equal(run(script, dir, true).status, 1);
  success(run(script, dir));
  success(run(script, dir, true));
  const output = join(dir, "web/packages/app/public/file-icons", rel);
  writeFileSync(output, "stale");
  assert.equal(run(script, dir, true).status, 1);
  assert.equal(readFileSync(output, "utf8"), "stale");
});
