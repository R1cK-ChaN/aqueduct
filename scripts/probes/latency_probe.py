"""Latency probe for the Hyperliquid public API that needs no account.

Measures, from the machine it runs on:
  1. DNS, TCP connect and TLS handshake time to api.hyperliquid.xyz.
  2. Round-trip time of small POST /info requests on a kept-alive connection.
  3. Round-trip time of POST /exchange with a well-formed order action and a
     random signature. The server rejects it; this measures how long the API
     takes to accept and reject a signed request, not how long an order takes
     to reach a block.
  4. The same two round trips as WebSocket "post" requests.
  5. Market-data lag: local receive time minus the block time stamped on bbo
     and trades messages. Only meaningful if the local clock is synchronised;
     record `chronyc tracking` alongside the output.

No key, wallet or account is used. Request weight stays far below the
per-IP limit (about 300 of 1200 per minute).

Usage: python3 latency_probe.py [market_seconds]
Requires: websocket-client (Ubuntu package python3-websocket).
"""
import http.client
import json
import os
import socket
import ssl
import statistics
import sys
import threading
import time

import websocket

HOST = "api.hyperliquid.xyz"
ZERO = "0x" + "0" * 40
INFO_BODY = json.dumps({"type": "clearinghouseState", "user": ZERO})


def pct(xs, p):
    xs = sorted(xs)
    if not xs:
        return None
    k = (len(xs) - 1) * p / 100
    lo, hi = int(k), min(int(k) + 1, len(xs) - 1)
    return xs[lo] + (xs[hi] - xs[lo]) * (k - lo)


def summary(name, ms):
    if not ms:
        print(f"{name:<34} no samples")
        return
    print(
        f"{name:<34} n={len(ms):<4} min={min(ms):7.1f}  p50={pct(ms, 50):7.1f}"
        f"  p90={pct(ms, 90):7.1f}  p99={pct(ms, 99):7.1f}  max={max(ms):7.1f} ms"
    )


def bogus_exchange_body():
    action = {
        "type": "order",
        "orders": [
            {"a": 0, "b": True, "p": "1000", "s": "0.001", "r": False,
             "t": {"limit": {"tif": "Alo"}}}
        ],
        "grouping": "na",
    }
    sig = {"r": "0x" + os.urandom(32).hex(), "s": "0x" + os.urandom(32).hex(), "v": 27}
    return json.dumps({"action": action, "nonce": int(time.time() * 1000), "signature": sig})


def handshake(n=10):
    dns, tcp, tls = [], [], []
    ctx = ssl.create_default_context()
    addr = None
    for _ in range(n):
        t0 = time.perf_counter()
        infos = socket.getaddrinfo(HOST, 443, socket.AF_INET, socket.SOCK_STREAM)
        t1 = time.perf_counter()
        addr = infos[0][4]
        s = socket.create_connection(addr, timeout=5)
        t2 = time.perf_counter()
        ss = ctx.wrap_socket(s, server_hostname=HOST)
        t3 = time.perf_counter()
        ss.close()
        dns.append((t1 - t0) * 1e3)
        tcp.append((t2 - t1) * 1e3)
        tls.append((t3 - t2) * 1e3)
        time.sleep(0.2)
    ips = sorted({i[4][0] for i in socket.getaddrinfo(HOST, 443, socket.AF_INET)})
    print("resolved", HOST, "->", ", ".join(ips))
    summary("dns lookup", dns)
    summary("tcp connect", tcp)
    summary("tls handshake", tls)


def http_rtt(path, body_fn, n, label):
    conn = http.client.HTTPSConnection(HOST, timeout=5)
    conn.request("POST", path, body_fn(), {"Content-Type": "application/json"})
    first = conn.getresponse()
    first_text = first.read().decode()[:200]
    first_status = first.status
    ms = []
    for _ in range(n):
        t0 = time.perf_counter()
        conn.request("POST", path, body_fn(), {"Content-Type": "application/json"})
        r = conn.getresponse()
        r.read()
        ms.append((time.perf_counter() - t0) * 1e3)
        time.sleep(0.1)
    conn.close()
    summary(label, ms)
    print(f"{'':<34} first response: HTTP {first_status} {first_text}")


def ws_session(market_seconds, n_info=50, n_exchange=20):
    ws = websocket.create_connection(f"wss://{HOST}/ws", timeout=10)
    lock = threading.Lock()
    pending = {}
    post_ms = {"info": [], "exchange": []}
    first_reply = {}
    lag = {"bbo": [], "trades": []}
    stop = threading.Event()
    # Subscribing to trades first returns a snapshot of recent, old trades.
    # Lag is only recorded after this time to keep that snapshot out.
    measure_from = [float("inf")]

    def reader():
        while not stop.is_set():
            try:
                raw = ws.recv()
            except Exception:
                break
            now_wall = time.time() * 1e3
            now_perf = time.perf_counter()
            m = json.loads(raw)
            ch = m.get("channel")
            d = m.get("data")
            if ch == "post":
                with lock:
                    kind, t0 = pending.pop(d["id"], (None, None))
                if kind:
                    post_ms[kind].append((now_perf - t0) * 1e3)
                    first_reply.setdefault(kind, json.dumps(d["response"])[:200])
            elif now_wall < measure_from[0]:
                continue
            elif ch == "bbo":
                lag["bbo"].append(now_wall - d["time"])
            elif ch == "trades":
                for t in d:
                    lag["trades"].append(now_wall - t["time"])

    th = threading.Thread(target=reader, daemon=True)
    th.start()
    for coin in ("BTC", "ETH"):
        ws.send(json.dumps({"method": "subscribe", "subscription": {"type": "bbo", "coin": coin}}))
        ws.send(json.dumps({"method": "subscribe", "subscription": {"type": "trades", "coin": coin}}))
    time.sleep(2)
    measure_from[0] = time.time() * 1e3

    req_id = 0
    for kind, n in (("info", n_info), ("exchange", n_exchange)):
        for _ in range(n):
            req_id += 1
            if kind == "info":
                req = {"type": "info", "payload": json.loads(INFO_BODY)}
            else:
                req = {"type": "action", "payload": json.loads(bogus_exchange_body())}
            with lock:
                pending[req_id] = (kind, time.perf_counter())
            ws.send(json.dumps({"method": "post", "id": req_id, "request": req}))
            time.sleep(0.2)
    time.sleep(1)

    t_end = time.time() + market_seconds
    while time.time() < t_end:
        time.sleep(1)
    stop.set()
    ws.close()

    summary("ws post /info", post_ms["info"])
    summary("ws post /exchange (rejected)", post_ms["exchange"])
    for k, v in first_reply.items():
        print(f"{'':<34} first {k} reply: {v}")
    summary("bbo lag (recv - block time)", lag["bbo"])
    summary("trades lag (recv - block time)", lag["trades"])


def main():
    market_seconds = int(sys.argv[1]) if len(sys.argv) > 1 else 120
    print("host", socket.gethostname(), "utc", time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()))
    handshake()
    http_rtt("/info", lambda: INFO_BODY, 50, "https /info (keep-alive)")
    http_rtt("/exchange", bogus_exchange_body, 20, "https /exchange (rejected)")
    ws_session(market_seconds)


if __name__ == "__main__":
    main()
