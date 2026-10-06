import { readdirSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
function check(path) {
  for (const entry of readdirSync(path, { withFileTypes: true })) {
    const file = resolve(path, entry.name);
    if (entry.isDirectory()) check(file);
    else if (/\.(mjs|js)$/.test(entry.name)) {
      const result = spawnSync(process.execPath, ["--check", file], {
        stdio: "inherit",
      });
      if (result.status) process.exit(result.status);
    }
  }
}
for (const folder of ["src", "public", "test", "scripts"]) check(folder);
console.log("JavaScript syntax checks passed.");
