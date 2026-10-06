"""Loopback HTTP benchmark. Reports observed latency, never a production capacity claim."""
import argparse
import concurrent.futures
import http.client
import json
import time
from urllib.parse import urlsplit

parser = argparse.ArgumentParser()
parser.add_argument("url")
parser.add_argument("--requests", type=int, default=500)
parser.add_argument("--concurrency", type=int, default=16)
parser.add_argument("--path", default="/api/state")
args = parser.parse_args()
target = urlsplit(args.url)
if args.requests < 1 or args.concurrency < 1:
    parser.error("Requests and concurrency must be positive")
if target.scheme not in ("http", "https") or not target.hostname:
    parser.error("Use an HTTP(S) origin")
connection_type = http.client.HTTPSConnection if target.scheme == "https" else http.client.HTTPConnection

def run(count):
    connection = connection_type(target.hostname, target.port, timeout=30)
    timings = []
    size = 0
    reconnects = 0
    for _ in range(count):
        started = time.perf_counter()
        for attempt in range(3):
            try:
                connection.request("GET", args.path)
                response = connection.getresponse()
                payload = response.read()
                break
            except (ConnectionError, http.client.HTTPException):
                connection.close()
                if attempt == 2:
                    raise
                reconnects += 1
                connection = connection_type(target.hostname, target.port, timeout=30)
        if response.status != 200:
            raise RuntimeError(f"HTTP {response.status}")
        size = len(payload)
        timings.append((time.perf_counter() - started) * 1000)
    connection.close()
    return timings, size, reconnects

run(10)
counts = [args.requests // args.concurrency + (i < args.requests % args.concurrency)
          for i in range(args.concurrency)]
started = time.perf_counter()
with concurrent.futures.ThreadPoolExecutor(max_workers=args.concurrency) as executor:
    results = list(executor.map(run, counts))
elapsed = time.perf_counter() - started
timings = sorted(t for batch, _, _ in results for t in batch)
def percentile(q):
    return round(timings[min(len(timings) - 1, int((len(timings) - 1) * q))], 3)
print(json.dumps({"url": args.url, "path": args.path, "requests": args.requests,
                  "concurrency": args.concurrency, "seconds": round(elapsed, 3),
                  "requestsPerSecond": round(args.requests / elapsed, 1),
                  "p50Ms": percentile(.5), "p95Ms": percentile(.95), "p99Ms": percentile(.99),
                  "reconnections": sum(row[2] for row in results), "responseBytes": results[0][1]}, indent=2))
