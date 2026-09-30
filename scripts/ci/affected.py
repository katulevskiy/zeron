#!/usr/bin/env python3
"""Decide which Linux CI jobs a change can affect.

usage: affected.py BASE_REF   (compares BASE_REF...HEAD in the current checkout)

Uses `cargo metadata --no-deps` for the workspace-internal dependency graph, maps
changed files to workspace members, takes the reverse-dependency closure, and
prints `core=`, `ui=`, `browser=` booleans plus `reason=` to stdout (and to
$GITHUB_OUTPUT when set).  ANY doubt (diff failure, toolchain/lock/profile/CI file
changed, unmapped source file) falls back to running everything.
"""
import json
import os
import subprocess
import sys
import tomllib

CORE_PKGS = {"zeron-harness", "zeron-engine", "zeron-sync", "zeron-update", "zeron-doc", "zeron-preview"}
UI_PKGS = {"zeron-ui"}
# Files that can change build/test behaviour for everything.
GLOBAL = ("Cargo.toml", "rust-toolchain.toml", "rust-toolchain", ".cargo/", ".config/",
          ".github/", "scripts/ci/", "deny.toml", "clippy.toml", "rustfmt.toml")
# Files that provably affect no Linux Rust job (docs, other platforms' apps).
INERT = ("docs/", "apps/ios/", "apps/android/", "apps/windows/", "scripts/ios/", "scripts/android/",
         "scripts/package-macos.sh", "scripts/package-windows.ps1", "scripts/test-windows", "dist/macos/", "README", "LICENSE",
         "ARCHITECTURE.md", "THIRD_PARTY_NOTICES.md", "CLAUDE.md", "AGENTS.md")
# Non-crate files read by specific Linux jobs.
BROWSER_FILES = ("scripts/test-linux-browser.sh",)
CORE_FILES = ("scripts/test-update-cursor-sdk.py", "scripts/test-cursor-sdk-auth.mjs")


def run(*cmd):
    return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout


def lock_changed(base, head, members):
    """Workspace members whose resolved dependency closure differs between two Cargo.lock files."""
    def load(ref):
        t = tomllib.loads(run("git", "show", f"{ref}:Cargo.lock"))
        ents = {}
        for p in t.get("package", []):
            ents[(p["name"], p["version"])] = (p.get("source"), p.get("checksum"), tuple(sorted(p.get("dependencies", []))))
        return ents
    a, b = load(base), load(head)
    diff = {k for k in set(a) | set(b) if a.get(k) != b.get(k)}
    if not diff:
        return set()
    by_name = {}
    for (n, v) in b:
        by_name.setdefault(n, []).append((n, v))
    changed_names = {n for n, _ in diff}
    out = set()
    for m in members:
        seen, stack = set(), [m]
        while stack:
            n = stack.pop()
            if n in seen:
                continue
            seen.add(n)
            for key in by_name.get(n, []):
                # lock dependency strings are "name" or "name version" (when ambiguous)
                for d in b[key][2]:
                    stack.append(d.split(" ")[0])
        if seen & changed_names:
            out.add(m)
    return out


def emit(core, ui, browser, reason):
    out = {"core": core, "ui": ui, "browser": browser, "reason": reason}
    for k, v in out.items():
        line = f"{k}={str(v).lower() if isinstance(v, bool) else v}"
        print(line)
        if os.environ.get("GITHUB_OUTPUT"):
            with open(os.environ["GITHUB_OUTPUT"], "a") as f:
                f.write(line + "\n")


def main():
    base = sys.argv[1]
    head = sys.argv[2] if len(sys.argv) > 2 else "HEAD"
    try:
        base = run("git", "merge-base", base, head).strip()
        files = run("git", "diff", "--name-only", base, head).split()
        meta = json.loads(run("cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"))
    except Exception as e:  # noqa: BLE001 - any failure => run everything
        return emit(True, True, True, f"fallback: {e}")
    if not files:
        return emit(True, True, True, "fallback: empty diff")

    root = meta["workspace_root"].rstrip("/") + "/"
    dirs = {}
    deps = {}
    for p in meta["packages"]:
        d = os.path.dirname(p["manifest_path"])
        rel = d[len(root):] if d.startswith(root) else ""
        dirs[p["name"]] = rel
        deps[p["name"]] = {x["name"] for x in p["dependencies"]}
    names = set(dirs)

    changed, browser_files, core_files = set(), False, False
    if "Cargo.lock" in files:
        try:
            changed |= lock_changed(base, head, names)
        except Exception as e:  # noqa: BLE001
            return emit(True, True, True, f"fallback: Cargo.lock diff failed: {e}")
        files = [f for f in files if f != "Cargo.lock"]
    if "Cargo.toml" in files:
        # A root manifest change that only touches [workspace.dependencies] is fully
        # described by the Cargo.lock diff and the member manifests; anything else
        # (profiles, patch, workspace.package, lints, members) is global.
        try:
            def rest(ref):
                t = tomllib.loads(run("git", "show", f"{ref}:Cargo.toml"))
                t.get("workspace", {}).pop("dependencies", None)
                return t
            if rest(base) == rest(head):
                files = [f for f in files if f != "Cargo.toml"]
        except Exception:  # noqa: BLE001 - keep Cargo.toml => global fallback below
            pass
    for f in files:
        if f.startswith(GLOBAL):
            return emit(True, True, True, f"fallback: global file {f}")
        hit = [n for n, d in dirs.items() if d and f.startswith(d + "/")]
        if hit:
            changed.update(hit)
        elif f.startswith(BROWSER_FILES):
            browser_files = True
        elif f.startswith(CORE_FILES):
            core_files = True
        elif f.startswith(INERT) or f.startswith("edge/"):
            continue  # edge/ is covered by the no-rust preview-coordinator job, which always runs
        else:
            return emit(True, True, True, f"fallback: unmapped file {f}")

    affected = set(changed)
    grew = True
    while grew:
        grew = False
        for n in names:
            if n not in affected and deps[n] & affected:
                affected.add(n)
                grew = True
    core = bool(affected & CORE_PKGS) or core_files
    ui = bool(affected & UI_PKGS)
    browser = ui or browser_files
    emit(core, ui, browser, "changed=" + ",".join(sorted(changed)) + " affected=" + ",".join(sorted(affected)))


if __name__ == "__main__":
    main()
