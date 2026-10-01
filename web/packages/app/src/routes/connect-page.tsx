import { useEffect } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useFleet, useFleetRegistry } from "../state/fleet";

/** Owned-device discovery help, not manual engine pairing or direct login. */
export function ConnectPage() {
  const fleet = useFleet();
  const registry = useFleetRegistry();
  const navigate = useNavigate();
  const live = registry.engines.some((engine) => engine.state === "connected");

  useEffect(() => {
    if (live) void navigate({ to: "/", replace: true });
  }, [live, navigate]);

  return (
    <main className="connect-page">
      <h1>Connect your engines</h1>
      {fleet.configurationError !== null ? (
        <p role="alert" className="form-error">{fleet.configurationError}</p>
      ) : (
        <p className="connect-status">
          Start <code>zeron</code> on a machine signed in to the same WorkOS account
          and connected to this browser's relay environment. Owned engines appear
          automatically; offline and unregistered engines cannot receive browser prompts.
        </p>
      )}
    </main>
  );
}
