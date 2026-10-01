// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

export type PreviewService = {
/**
 * Persisted identities; neither includes a listening port or process ID.
 */
id: string, projectId: string, projectName: string, projectCwd: string, deviceId: string, deviceName: string, hostname: string, name: string, port: number, pid: number, cwd: string,
/**
 * Process creation time, milliseconds since the Unix epoch.
 */
startedAt: number, zeronOwned: boolean, };
