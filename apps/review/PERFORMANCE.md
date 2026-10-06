# Performance

The service uses a small Axum HTTP layer and a single SQLite worker. A release build embeds the frontend, precompresses assets, and strips symbols. Queue reads share a serialized snapshot that is invalidated after successful database transactions. Item details load audit history on demand. There is no npm runtime, frontend framework, or ORM.

The queue endpoint includes a user-specific ETag. Browser polling sends `If-None-Match` and receives HTTP 304 if the public queue and identity are unchanged. The shared snapshot never contains session credentials; actor, capabilities, and CSRF values are attached for each request. Mutations and claim expiration invalidate the snapshot.

## Reproduce

Build and run with an imported snapshot (in a separate sandbox database):

```sh
cargo build --release --locked
DATA_PATH=data/benchmark.sqlite target/release/zeron-review import --demo --github
DATA_PATH=data/benchmark.sqlite target/release/zeron-review --demo
```

Then use the Python standard library benchmark:

```sh
python3 scripts/benchmark.py http://localhost:3080 --requests 500 --concurrency 16
```

This warms the endpoint, uses persistent connections, reads full response bodies, verifies HTTP 200, and reports observed latency and throughput. It does not send conditional headers. If a server closes a persistent connection, the script reconnects for the idempotent GET, includes the retry in latency, and reports reconnections. Benchmark a representative dataset on an idle target host before using the results for deployment sizing. Network, proxy, CPU contention, active writes, and cold snapshots affect results. GitHub API synchronization is rate-limited separately from local reads.

## Observed development measurement

On October 5, 2026, the native builds served a sandbox imported from upstream with 280 PRs and issues on an ARM64 macOS host. The host was heavily contended (load averages around 400–500). Both runs read full, 222,990-byte queue responses over persistent loopback connections after ten warmup requests; neither used conditional headers.

| Build   | Requests | Clients |   Median |        p95 |        p99 | Observed requests/s | Reconnects |
| ------- | -------: | ------: | -------: | ---------: | ---------: | ------------------: | ---------: |
| Debug   |      100 |       4 | 1.337 ms | 144.337 ms | 156.452 ms |               170.9 |          0 |
| Release |      500 |      16 | 2.049 ms |   6.536 ms |  36.685 ms |             1,471.9 |          0 |

These are local smoke measurements. The different client counts and variable contention prevent a direct debug/release speed comparison. Reproduce on an idle deployment host with a release build before setting a service target; these numbers do not estimate production capacity.

The stripped native release binary is about 5.8 MiB including frontend assets and fonts. Release smoke checks verified health, queue responses, conditional HTTP 304, compressed JavaScript, and the embedded icon. Four browser tests separately verified asset loading, review/validation, author restrictions, persistence, and mobile layout against the native debug binary; they are not latency benchmarks.

## Deployment limits

One instance owns the SQLite database. Writes and database tasks serialize through a bounded worker queue; outbound HTTP requests are asynchronous. Every committed transaction invalidates the snapshot, including background housekeeping. The optimization is most useful for multiple readers between writes. It does not remove the cost of rebuilding state after a change. Very large installations should add pagination and measure before changing databases or adding replicas.

Docker build validation requires a running Docker daemon. Native release builds and HTTP/browser tests exercise the binary, but do not validate a container image or real GitHub App installation.
