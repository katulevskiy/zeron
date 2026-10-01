import { spawn, type ChildProcess } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../../../..");
const diagnosticLimit = 32_768;
const tail = (text: string): string => text.slice(-diagnosticLimit);
const exited = (child: ChildProcess): boolean => child.exitCode !== null || child.signalCode !== null;

/** Read complete lines, not chunks; diagnostic output is not a readiness signal. */
export function waitForReady(child: ChildProcess, prefix: string, timeoutMs: number): Promise<string> {
  return new Promise((resolve, reject) => {
    const stdout = child.stdout!;
    const stderr = child.stderr!;
    stdout.setEncoding("utf8");
    stderr.setEncoding("utf8");
    let buffer = "";
    let output = "";
    let errors = "";
    let eof: ReturnType<typeof setImmediate> | undefined;
    const cleanup = (): void => {
      clearTimeout(timer);
      if (eof) clearImmediate(eof);
      stdout.off("data", onData);
      stdout.off("error", onError);
      stdout.off("end", onEnd);
      stderr.off("data", onStderr);
      stderr.off("error", onError);
      child.off("error", onError);
      child.off("exit", onExit);
    };
    const fail = (reason: string): void => {
      cleanup();
      reject(new Error(`${child.spawnargs.join(" ")}: ${reason} before ${JSON.stringify(prefix)}\nstdout:\n${output}\nstderr:\n${errors}`));
    };
    const onData = (chunk: string): void => {
      output = tail(output + chunk);
      buffer += chunk;
      let newline: number;
      while ((newline = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, newline).trim();
        buffer = buffer.slice(newline + 1);
        if (line.startsWith(prefix)) {
          cleanup();
          resolve(line.slice(prefix.length));
          return;
        }
      }
      // A broken fixture must not grow the runner's memory without bound.
      if (buffer.length > diagnosticLimit) fail("readiness line exceeds diagnostic limit");
    };
    const onStderr = (chunk: string): void => { errors = tail(errors + chunk); };
    const onError = (error: Error): void => fail(error.message);
    const onExit = (code: number | null, signal: NodeJS.Signals | null): void => fail(`exited with code ${code}, signal ${signal}`);
    // Give already queued stderr and exit events a turn before diagnosing EOF.
    const onEnd = (): void => { eof = setImmediate(() => fail("stdout ended")); };
    const timer = setTimeout(() => fail(`startup timed out after ${timeoutMs}ms`), timeoutMs);
    stdout.on("data", onData);
    stdout.on("error", onError);
    stdout.on("end", onEnd);
    stderr.on("data", onStderr);
    stderr.on("error", onError);
    child.on("error", onError);
    child.on("exit", onExit);
    if (exited(child)) onExit(child.exitCode, child.signalCode);
    else if (stdout.readableEnded) onEnd();
  });
}

/** Idempotent and race-safe even when the child exited before teardown. */
export async function stopChild(child: ChildProcess, processGroup = false): Promise<void> {
  if (child.pid === undefined) return;
  const signal = (value: NodeJS.Signals): void => {
    if (processGroup && process.platform !== "win32") {
      try { process.kill(-child.pid!, value); }
      catch (error) { if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error; }
    } else if (!exited(child)) child.kill(value);
  };
  if (exited(child)) { if (processGroup) signal("SIGKILL"); return; }
  await new Promise<void>((resolve, reject) => {
    const cleanup = (): void => {
      clearTimeout(force);
      clearTimeout(deadline);
      child.off("exit", onExit);
    };
    const onExit = (): void => {
      cleanup();
      // Cargo can leave compiler descendants behind when it exits on a signal.
      try { if (processGroup) signal("SIGKILL"); resolve(); }
      catch (error) { reject(error); }
    };
    child.once("exit", onExit);
    const force = setTimeout(() => {
      try { signal("SIGKILL"); } catch (error) { cleanup(); reject(error); }
    }, 1000);
    const deadline = setTimeout(() => {
      cleanup();
      reject(new Error(`could not stop fixture pid ${child.pid} after 3000ms`));
    }, 3000);
    try { signal("SIGTERM"); } catch (error) { cleanup(); reject(error); }
  });
}

export interface ProcessFixture {
  child: ChildProcess;
  line: string;
  stop(): Promise<void>;
}

/** Own the child before awaiting readiness, so failed startup cannot leak it. */
export async function startProcess(command: string, args: string[], prefix: string, timeoutMs = 15_000): Promise<ProcessFixture> {
  const child = spawn(command, args, { cwd: repoRoot, detached: process.platform !== "win32", stdio: ["ignore", "pipe", "pipe"] });
  try {
    const line = await waitForReady(child, prefix, timeoutMs);
    // Keep pipes drained after readiness, including on Windows.
    child.stdout!.resume();
    child.stderr!.resume();
    let stopping: Promise<void> | undefined;
    return { child, line, stop: () => stopping ??= stopChild(child, true) };
  } catch (error) {
    await stopChild(child, true);
    throw error;
  }
}

/** Build/metadata failures include both command context and captured Cargo stderr. */
export async function runCommand(command: string, args: string[], timeoutMs: number): Promise<string> {
  const child = spawn(command, args, {
    cwd: repoRoot,
    detached: process.platform !== "win32",
    env: { ...process.env, CARGO_BUILD_JOBS: "2" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  let errors = "";
  child.stdout!.setEncoding("utf8");
  child.stderr!.setEncoding("utf8");
  const onData = (chunk: string): void => { output += chunk; };
  const onStderr = (chunk: string): void => { errors = tail(errors + chunk); };
  child.stdout!.on("data", onData);
  child.stderr!.on("data", onStderr);
  try {
    await new Promise<void>((resolve, reject) => {
      const cleanup = (): void => {
        clearTimeout(timer);
        child.off("error", onError);
        child.off("close", onClose);
        child.stdout!.off("error", onError);
        child.stderr!.off("error", onError);
      };
      const fail = (reason: string): void => {
        cleanup();
        reject(new Error(`${command} ${args.join(" ")}: ${reason}\nstdout:\n${tail(output)}\nstderr:\n${errors}`));
      };
      const onError = (error: Error): void => fail(error.message);
      const onClose = (code: number | null, signal: NodeJS.Signals | null): void => {
        if (code === 0) { cleanup(); resolve(); }
        else fail(`exited with code ${code}, signal ${signal}`);
      };
      const timer = setTimeout(() => fail(`timed out after ${timeoutMs}ms`), timeoutMs);
      child.on("error", onError);
      child.once("close", onClose);
      child.stdout!.on("error", onError);
      child.stderr!.on("error", onError);
    });
    return output;
  } finally {
    await stopChild(child, true);
    child.stdout!.off("data", onData);
    child.stderr!.off("data", onStderr);
  }
}
