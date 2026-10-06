# Exnomic deployment

Host: `root@95.85.237.142` via SSH alias `exnomic`.
Public origin: `https://zeron.exnomic.com`; DNS is proxied through Cloudflare.
Certificate: existing `/etc/letsencrypt/live/exnomic.com-wildcard/` certificate and renewal job.

Isolated paths:

- `/opt/contribution-manager/releases/<release>/zeron-review`; `current` symlink selects the release.
- `/opt/contribution-manager/policy.json` configures the repository and review requirements.
- `/var/lib/contribution-manager/review.sqlite` stores live workflow data.
- `/var/lib/contribution-manager/review.preview.sqlite` stores the separate development sandbox.
- `/var/lib/contribution-manager/github.json` and `github.pem` store GitHub App credentials, readable only by the service account.
- `/etc/contribution-manager.env` contains private owner setup access and runtime environment (root-only).
- `/etc/systemd/system/contribution-manager.service` is the dedicated service.
- `/etc/nginx/sites-available/contribution-manager.conf` is the dedicated virtual host, enabled by a symlink.

Only the new SNI route is added to `/etc/nginx/nginx.conf`; its backup goes in `/opt/contribution-manager/deployment-backup/`. All existing SNI routes, TLS listeners, sites, VPNs and firewall rules remain as they were. Nginx is gracefully reloaded after configuration validation.

Update procedure: build and test on the development machine; copy a release binary into a fresh release directory; back up the SQLite database using its backup API; switch `current`; restart only `contribution-manager.service`; check loopback `/health` and public HTTPS `/health` and `/api/state`. Restore the previous `current` target to roll back the binary. Preserve both live and preview databases.

Retrieve the owner's setup link without displaying other credentials:

```sh
ssh exnomic 'python3 - <<"PY"
from pathlib import Path
values=dict(line.split("=",1) for line in Path("/etc/contribution-manager.env").read_text().splitlines() if "=" in line)
print(values["PUBLIC_URL"]+"/setup/github?token="+values["SETUP_TOKEN"])
PY'
```

Keep that link private. It stops accepting registrations once an App is configured. The App owner must complete GitHub's registration and installation screens. The callback saves credentials server-side and the dedicated service restarts itself to load them; no secrets need to be pasted into chat.

The App requires repository permissions: Contents read, Metadata read, Issues write, Pull requests write, and Checks write. GitHub requires Pull requests write to label PRs even though those endpoints use `/issues/`. Existing installations created with Pull requests read must be updated in the App's Permissions and events settings, then the owner must approve the new permission on the repository installation. Updating the manifest does not change an existing installation.
