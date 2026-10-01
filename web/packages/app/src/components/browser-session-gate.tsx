import type { BrowserFleetStatus } from "../state/fleet";

/** No engine-backed subtree mounts until the cookie session is verified. */
export function BrowserSessionGate(props: {
  readonly status: BrowserFleetStatus;
  readonly error: string | null;
  readonly onSignIn: () => void;
  readonly onRetry: () => void;
  readonly onRetrySignOut: () => void;
}) {
  const busy = props.status === "checking" || props.status === "signing-in";
  const revocationFailed = props.status === "logout-failed";
  const title = props.status === "checking" ? "Checking your session"
    : props.status === "signing-in" ? "Redirecting to sign in"
    : revocationFailed ? "Sign-out incomplete"
    : props.status === "error" ? "Browser connection unavailable" : "Sign in to Zeron";
  return (
    <main className="browser-session-gate">
      <section className="browser-session-card" aria-busy={busy}>
        <h1>{title}</h1>
        <p>Sign in to reach the engines registered to your account.</p>
        {props.error !== null && <p role="alert" className="form-error">{props.error}</p>}
        {busy ? <p role="status">Please wait…</p> : (
          <div className="browser-session-actions">
            {revocationFailed ? (
              <button type="button" className="btn btn-primary" onClick={props.onRetrySignOut}>Retry sign-out</button>
            ) : (
              <button type="button" className="btn btn-primary" onClick={props.onSignIn}>Sign in with WorkOS</button>
            )}
            <button type="button" className="btn btn-ghost" onClick={props.onRetry}>Retry session check</button>
          </div>
        )}
      </section>
    </main>
  );
}
