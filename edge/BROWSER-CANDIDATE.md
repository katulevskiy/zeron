# PR526 browser backend candidate

Local implementation only. No deployment, hostname registration or secret changes are authorized by this ticket.

## Provenance and identities

Deliberately ported from canonical browser UI source `e07f952eb732278cb75b80ff220ed43a9ef2a6fa`: `src/browser-routes.ts`, `browser-sessions.ts`, `forwarded-headers.ts`, browser auth additions in `auth.ts`/`workos.ts`, cookie/host registration additions in `index.ts` and session binding/revocation in `device-room.ts`, with corresponding canonical tests. This is a source port, not a history merge or dependency on an uncommitted canonical checkout. `production.ts` deliberately replaces that revision's Trunk-only serving with React/Vite candidate serving.

Native `wrangler.jsonc` keeps `comet-native-edge`, production routes, buckets, native entry point, and v1–v4 migrations unchanged. Additive v5 binds BrowserSessionStore. Room identities remain `d2/{deviceId}` and `browser-sessions-v1`. Candidate config uses a separate worker/resources and no registered routes; it is **not** a production replacement and does not share production native hosts. A real production same-origin rollout requires a separately reviewed deployment plan preserving existing DO storage/identity, explicit host selection and AuthKit redirect registration.

## Same-origin contract

WorkOS mode requires explicit `WORKOS_CLIENT_ID`, `WORKOS_API_KEY`, `WORKOS_BROWSER_ORIGIN` (exact HTTPS origin, no trailing slash), `WORKOS_ISSUER`, `WORKOS_JWKS_URL`, `BROWSER_SESSION_KEY` and BROWSER_SESSIONS binding. Never expose provider tokens or the encryption key to Vite/JS. No inferred issuer/JWKS/native dev bearer fallback for browser auth. Missing config returns JSON 501 `browser_auth_not_configured`.

- GET `/api/browser/session`: `{authenticated:false}` or owner, expiry and CSRF token.
- POST `/api/browser/login`: exact Origin required; returns `{authorizationUrl}`, sets Secure HttpOnly transaction cookie; state + nonce + S256 PKCE are durable and single-use.
- GET `/api/browser/callback?code=…&state=…`: consumes transaction; verifies signed provider token and matching provider user; sets `__Host-comet_session` Secure HttpOnly SameSite=Strict and redirects to origin root.
- GET `/api/browser/devices`: owner-only `{devices:[{id,name?,online}]}`. Native host accepted at `/device/{id}/ws?role=host&name=…` is registered after real DO upgrade.
- GET `/api/browser/device/{id}/ws`: cookie-only WebSocket, exact Origin, forced client/random connId, owner checked by DeviceRoom before any RPC. Browser cannot assert another user, host role or session binding.
- POST activity/logout/revoke: cookie + exact Origin + `x-csrf-token`; logout durably revokes bound browser sockets without disconnecting engine host.

`production.ts` handles APIs/upgrades/native routes before asset/deep-link fallback. `run_worker_first:true`, asset `html_handling:none` and `not_found_handling:none` are required. Disabling HTML canonical redirects avoids an `/index.html` → `/` → `/index.html` redirect loop. Serve Vite `/assets/*` directly; document navigation/deep links resolve `/index.html` only at configured app origin. Browser API failures must never become HTML.

## Deterministic loopback integration (not WorkOS proof)

From `edge/`, with Node 24 on PATH and app dist built by frontend owner:

```sh
npx wrangler dev --local --ip 127.0.0.1 --port 27640 \
  --local-upstream 127.0.0.1:27640 --upstream-protocol http \
  -c wrangler.browser-candidate.jsonc \
  --var AUTH_MODE:dev \
  --var BROWSER_DEV_ORIGIN:http://127.0.0.1:27640 \
  --var BROWSER_DEV_OWNER_SUBJECT:browser-e2e-owner \
  --var BROWSER_DEV_ORGANIZATION_ID:dev-org \
  --var BROWSER_SESSION_KEY:loopback-fixture-only
```

POST `/api/browser/dev-login` with `Origin: http://127.0.0.1:27640`; retain HttpOnly `comet_dev_session` cookie and returned csrfToken. Probe session/devices with that cookie; browser upgrade uses the same cookie and exact Origin. Native engine fixture uses `ZERON_EDGE_URL=http://127.0.0.1:27640` and native dev bearer `browser-e2e-owner` when registering a real host at `/device/{deviceId}/ws?role=host`. It must not use a direct engine listener. Different engine owners cannot use the cookie to connect. Bind Wrangler only to loopback, never expose AUTH_MODE=dev publicly. The dev-cookie endpoint is not available in WorkOS mode or at non-loopback request/config origins. No dev proxy capability is required for direct same-origin integration.

## Verification commands (no remote operations)

```sh
npm ci --ignore-scripts --no-audit --no-fund
npm run typecheck
npm run typecheck:workerd
npm run test:unit
npm run test:workerd
WRANGLER_SEND_METRICS=false BROWSER_CHECK_PORT=27751 npm run test:browser-candidate
WRANGLER_SEND_METRICS=false npx wrangler deploy --dry-run -c wrangler.browser-candidate.jsonc --outdir dist/browser-candidate
```

Real WorkOS smoke needs test-environment credentials and interactive AuthKit sign-in; deterministic signed JWT/provider fixtures are separate evidence, not a claim that WorkOS was contacted. Real-engine relay/visual proof is owned by the parent and relay fixture worker; integrate its results before completing ticket03.
