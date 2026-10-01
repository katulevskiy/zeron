import { spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { createServer } from "node:net";
import { describe, expect, test } from "vitest";
import { runCommand, startProcess, stopChild, waitForReady } from "./helpers/engine";

async function exercise(script: string, assertion: (ready: Promise<string>, child: ChildProcess, endListeners: number) => Promise<void>) {
  const child = spawn(process.execPath, ["-e", script], { stdio: ["ignore", "pipe", "pipe"] });
  const endListeners = child.stdout!.listenerCount("end");
  let timer: ReturnType<typeof setTimeout>;
  const ready = Promise.race([
    waitForReady(child, "READY ", 250),
    new Promise<string>((_, reject) => { timer = setTimeout(() => reject(new Error("test watchdog")), 1500); }),
  ]);
  try {
    await assertion(ready, child, endListeners);
  } finally {
    clearTimeout(timer!);
    if (child.exitCode === null && child.signalCode === null) {
      const exited = once(child, "exit");
      child.kill("SIGKILL");
      await exited;
    }
  }
}

describe("subprocess readiness", () => {
  test("handles split UTF-8 chunks and CRLF", async () => {
    await exercise(`process.stdout.write(Buffer.from([82,69,65,68,89,32,195])); setTimeout(() => process.stdout.write(Buffer.from([169,13,10])), 30); setInterval(()=>{},1000)`, async (ready) => {
      expect(await ready).toBe("é");
    });
  });
  test("consumes diagnostic lines before a split readiness line", async () => {
    await exercise(`process.stdout.write('diagnostic\\nsecond\\nREA'); setTimeout(()=>process.stdout.write('DY endpoint\\n'),30); setInterval(()=>{},1000)`, async (ready) => {
      expect(await ready).toBe("endpoint");
    });
  });
  test.each([0, 7])("rejects promptly on exit %s and keeps stderr", async (code) => {
    await exercise(`process.stderr.write('startup exploded'); process.exitCode=${code}`, async (ready) => {
      await expect(ready).rejects.toThrow(/startup exploded/);
    });
  });
  test("rejects stdout EOF even when the child stays alive", async () => {
    await exercise(`process.stderr.write('closed output'); process.stdout.end(); setInterval(()=>{},1000)`, async (ready) => {
      await expect(ready).rejects.toThrow(/stdout ended.*closed output/s);
    });
  });
  test("rejects when the process already exited before waiting", async () => {
    const child = spawn(process.execPath, ["-e", ""], { stdio: ["ignore", "pipe", "pipe"] });
    await once(child, "exit");
    await expect(waitForReady(child, "READY ", 250)).rejects.toThrow("exited with code 0");
  });
  test("bounds startup and removes readiness listeners", async () => {
    await exercise(`process.stderr.write('still loading'); setInterval(()=>{},1000)`, async (ready, child, endListeners) => {
      await expect(ready).rejects.toThrow(/timed out.*still loading/s);
      expect(child.stdout!.listenerCount("data")).toBe(0);
      expect(child.stdout!.listenerCount("end")).toBe(endListeners);
      expect(child.listenerCount("exit")).toBe(0);
    });
  });
  test.each(["stdout", "stderr"] as const)("rejects %s stream errors", async (stream) => {
    await exercise(`setInterval(()=>{},1000)`, async (ready, child) => {
      child[stream]!.emit("error", new Error(`broken ${stream}`));
      await expect(ready).rejects.toThrow(`broken ${stream}`);
    });
  });
  test("an occupied test-owned port reports bind failure without stopping its owner", async () => {
    const server = createServer();
    server.listen(0, "127.0.0.1");
    await once(server, "listening");
    const address = server.address();
    if (!address || typeof address === "string") throw new Error("missing port");
    try {
      await exercise(`const s=require('node:net').createServer(); s.on('error',e=>{console.error(e.code); process.exitCode=9}); s.listen(${address.port},'127.0.0.1')`, async (ready) => {
        await expect(ready).rejects.toThrow("EADDRINUSE");
        expect(server.listening).toBe(true);
      });
    } finally {
      await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
    }
  });
});

describe("fixture ownership", () => {
  test("spawn errors reject immediately with executable context", async () => {
    await expect(startProcess("zeron-nonexistent-fixture-executable", [], "READY ", 250)).rejects.toThrow(/zeron-nonexistent-fixture-executable.*ENOENT/s);
  });
  test.each([
    ["timeout", "setInterval(()=>{},1000)"],
    ["EOF", "process.stdout.end(); setInterval(()=>{},1000)"],
  ])("startup %s tears down the child before rejection", async (_name, script) => {
    const error = await startProcess(process.execPath, ["-e", `console.error('fixture-pid='+process.pid); ${script}`], "READY ", 250).catch(error => error as Error);
    expect(error).toBeInstanceOf(Error);
    const pid = Number((error as Error).message.match(/fixture-pid=(\d+)/)?.[1]);
    expect(pid).toBeGreaterThan(0);
    expect(() => process.kill(pid, 0)).toThrow();
  });
  test("normal completion and a failed test body both release the child", async () => {
    for (const fail of [false, true]) {
      const fixture = await startProcess(process.execPath, ["-e", "console.log('READY endpoint'); setInterval(()=>{},1000)"], "READY ");
      try {
        expect(fixture.line).toBe("endpoint");
        if (fail) throw new Error("test body failed");
      } catch (error) {
        expect((error as Error).message).toBe("test body failed");
      } finally {
        await fixture.stop();
      }
      await fixture.stop();
      expect(fixture.child.exitCode !== null || fixture.child.signalCode !== null).toBe(true);
      expect(fixture.child.listenerCount("exit")).toBe(0);
      expect(fixture.child.stdout!.listenerCount("data")).toBe(0);
    }
  });
  test("teardown handles an already-exited child", async () => {
    const child = spawn(process.execPath, ["-e", ""], { stdio: "ignore" });
    await once(child, "exit");
    await stopChild(child);
    expect(child.exitCode).toBe(0);
  });
  test("teardown escalates when a child ignores SIGTERM", async () => {
    const fixture = await startProcess(process.execPath, ["-e", "process.on('SIGTERM',()=>{}); console.log('READY endpoint'); setInterval(()=>{},1000)"], "READY ");
    await fixture.stop();
    expect(fixture.child.signalCode).toBe(process.platform === "win32" ? "SIGTERM" : "SIGKILL");
  });
  test("build failures preserve command, exit code and stderr", async () => {
    await expect(runCommand("cargo", ["build", "-p", "zeron-ticket12-deliberately-missing-package"], 10_000)).rejects.toThrow(/cargo build.*exited with code 101.*zeron-ticket12-deliberately-missing-package/s);
  });
  test("build timeout tears down the process and retains diagnostics", async () => {
    const error = await runCommand(process.execPath, ["-e", "console.error('fixture-pid='+process.pid); console.error('build stalled'); setInterval(()=>{},1000)"], 250).catch(error => error as Error);
    expect(error).toBeInstanceOf(Error);
    expect((error as Error).message).toMatch(/timed out after 250ms.*build stalled/s);
    const pid = Number((error as Error).message.match(/fixture-pid=(\d+)/)?.[1]);
    expect(pid).toBeGreaterThan(0);
    expect(() => process.kill(pid, 0)).toThrow();
  });
});
