#!/usr/bin/env python3
"""A local stand-in for a proxy provider, for developing and demonstrating Kairo
without a real subscription.

It runs a few HTTP CONNECT proxies on 127.0.0.1 (some with added latency), an
origin server, and serves a subscription whose ~120 nodes all point at those
local proxies (a few at a closed port, to show timeouts). With --traffic it
also keeps sending requests through Kairo's mixed port so the live views move.

    python3 -I scripts/fixture.py --port 18080 [--traffic 17897]

Subscription URL: http://127.0.0.1:<port>/sub
A provider's notice instead of a config: http://127.0.0.1:<port>/notice
/down-on and /down-off: while on, /sub answers 502 (for tests)
"""

import argparse
import asyncio
import random

STATE = {"down": False}
REGIONS = [
    ("Tokyo", 6), ("Osaka", 3), ("Hong Kong", 8), ("Taipei", 4), ("Singapore", 8),
    ("Seoul", 4), ("Los Angeles", 6), ("San Jose", 4), ("Seattle", 3), ("New York", 4),
    ("London", 5), ("Frankfurt", 5), ("Amsterdam", 4), ("Paris", 3), ("Sydney", 3),
    ("Toronto", 3), ("Mumbai", 3), ("Istanbul", 2), ("São Paulo", 2), ("Dubai", 2),
]
FLAGS = {
    "Tokyo": "🇯🇵", "Osaka": "🇯🇵", "Hong Kong": "🇭🇰", "Taipei": "🇹🇼", "Singapore": "🇸🇬",
    "Seoul": "🇰🇷", "Los Angeles": "🇺🇸", "San Jose": "🇺🇸", "Seattle": "🇺🇸", "New York": "🇺🇸",
    "London": "🇬🇧", "Frankfurt": "🇩🇪", "Amsterdam": "🇳🇱", "Paris": "🇫🇷", "Sydney": "🇦🇺",
    "Toronto": "🇨🇦", "Mumbai": "🇮🇳", "Istanbul": "🇹🇷", "São Paulo": "🇧🇷", "Dubai": "🇦🇪",
}


async def pipe(reader, writer):
    try:
        while True:
            data = await reader.read(65536)
            if not data:
                break
            writer.write(data)
            await writer.drain()
    except Exception:
        pass
    finally:
        try:
            writer.close()
        except Exception:
            pass


def connect_proxy(delay):
    async def handle(reader, writer):
        try:
            head = await reader.readuntil(b"\r\n\r\n")
        except Exception:
            writer.close()
            return
        parts = head.split(b" ")
        if len(parts) < 2 or parts[0] != b"CONNECT":
            writer.write(b"HTTP/1.1 405 Method Not Allowed\r\n\r\n")
            writer.close()
            return
        host, _, port = parts[1].decode().rpartition(":")
        # The demo hosts live on this machine.
        if host.endswith(".test") or host.endswith(".local") or host.startswith("203.0.113."):
            host = "127.0.0.1"
        await asyncio.sleep(delay / 1000)
        try:
            ur, uw = await asyncio.open_connection(host.strip("[]"), int(port))
        except Exception:
            writer.write(b"HTTP/1.1 502 Bad Gateway\r\n\r\n")
            writer.close()
            return
        writer.write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        await writer.drain()
        await asyncio.gather(pipe(reader, uw), pipe(ur, writer))
    return handle


def subscription(proxy_ports, dead_port, origin):
    lines = ["mixed-port: 7890", "allow-lan: false", "log-level: info", "proxies:"]
    names = []
    i = 0
    for region, count in REGIONS:
        for n in range(1, count + 1):
            name = f"{FLAGS[region]} {region} {n:02d}"
            if i % 17 == 16:
                port = dead_port
            else:
                port = proxy_ports[i % len(proxy_ports)]
            lines.append(f'  - {{name: "{name}", type: http, server: 127.0.0.1, port: {port}}}')
            names.append(name)
            i += 1
    def q(xs):
        return "[" + ", ".join(f'"{x}"' for x in xs) + "]"
    asia = [n for n in names if any(r in n for r in ("Tokyo", "Osaka", "Hong Kong", "Taipei", "Singapore", "Seoul"))]
    us = [n for n in names if any(r in n for r in ("Los Angeles", "San Jose", "Seattle", "New York"))]
    test = f"http://{origin}/generate_204"
    lines += [
        "proxy-groups:",
        f'  - {{name: "Proxy", type: select, proxies: ["Auto", "Fallback", "Asia", "United States", "DIRECT"] }}',
        f'  - {{name: "Auto", type: url-test, proxies: {q(names)}, url: "{test}", interval: 600, tolerance: 50}}',
        f'  - {{name: "Fallback", type: fallback, proxies: {q(names[:12])}, url: "{test}", interval: 600}}',
        f'  - {{name: "Asia", type: select, proxies: {q(asia)} }}',
        f'  - {{name: "United States", type: select, proxies: {q(us)} }}',
        f'  - {{name: "Streaming", type: select, proxies: ["Asia", "United States", "Proxy"] }}',
        f'  - {{name: "AI", type: select, proxies: ["United States", "Asia", "Proxy"] }}',
        f'  - {{name: "Final", type: select, proxies: ["Proxy", "DIRECT"] }}',
        "rules:",
        "  - DOMAIN-SUFFIX,video.test,Streaming",
        "  - DOMAIN-SUFFIX,chat.test,AI",
        "  - DOMAIN-KEYWORD,ads,REJECT",
        "  - DOMAIN-SUFFIX,local,DIRECT",
        "  - IP-CIDR,10.0.0.0/8,DIRECT,no-resolve",
        "  - IP-CIDR,192.168.0.0/16,DIRECT,no-resolve",
        "  - DST-PORT,22,DIRECT",
        "  - MATCH,Final",
    ]
    return "\n".join(lines) + "\n"


async def origin_handler(reader, writer, sub):
    try:
        head = await reader.readuntil(b"\r\n\r\n")
    except Exception:
        writer.close()
        return
    path = head.split(b" ")[1].decode() if b" " in head else "/"
    if path.endswith("/generate_204"):
        writer.write(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    elif path.endswith("/sub") and STATE["down"]:
        writer.write(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    elif path.endswith("/sub"):
        body = sub().encode()
        writer.write(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/yaml; charset=utf-8\r\n"
            + b"subscription-userinfo: upload=4831838208; download=52613349376; total=214748364800; expire=1798761600\r\n"
            + b"profile-update-interval: 12\r\n"
            + b"content-disposition: attachment; filename*=UTF-8''Demo%20Cloud.yaml\r\n"
            + f"Content-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode()
            + body
        )
    elif path.endswith("/down-on") or path.endswith("/down-off"):
        # A switch for tests: while it is on, /sub answers like a provider whose server is having trouble.
        STATE["down"] = path.endswith("/down-on")
        writer.write(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    elif path.endswith("/notice"):
        # A provider that answers with a message instead of a config (fetching
        # not enabled on its website, an expired plan).
        body = "请登录网站后台，在订阅页面开启订阅获取。每次开启后，订阅可获取有效时间为：10 分钟".encode()
        writer.write(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\n" + f"Content-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode() + body)
    elif path.startswith("/stream"):
        size = random.randint(1_000_000, 6_000_000)
        writer.write(f"HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n".encode())
        sent = 0
        try:
            while sent < size:
                chunk = min(random.randint(8_000, 40_000), size - sent)
                writer.write(b"k" * chunk)
                await writer.drain()
                sent += chunk
                await asyncio.sleep(0.1)
        except Exception:
            pass
    else:
        writer.write(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nkairo-ok")
    try:
        await writer.drain()
        writer.close()
    except Exception:
        pass


HOSTS = ["api.github.test", "video.test", "cdn.video.test", "chat.test", "www.example.test",
         "updates.local", "fonts.example.test", "ads.tracker.test", "mail.example.test", "registry.npm.test"]


async def traffic(mixed_port, origin):
    """Sends requests through Kairo so connections and traffic move."""
    oh, op = origin.split(":")
    async def one():
        host = random.choice(HOSTS)
        path = "/stream" if random.random() < 0.5 else "/hello"
        try:
            r, w = await asyncio.open_connection("127.0.0.1", mixed_port)
            # Absolute-form request: the proxy resolves the host by the rules; the
            # fixture's hosts map to the origin through mihomo's `hosts`.
            w.write(f"CONNECT {host}:{op} HTTP/1.1\r\nHost: {host}:{op}\r\n\r\n".encode())
            await w.drain()
            await r.readuntil(b"\r\n\r\n")
            w.write(f"GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n".encode())
            await w.drain()
            while await r.read(65536):
                pass
            w.close()
        except Exception:
            pass
    while True:
        for _ in range(random.randint(1, 4)):
            asyncio.ensure_future(one())
        await asyncio.sleep(random.uniform(0.3, 1.2))


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=18080)
    ap.add_argument("--traffic", type=int, default=0, help="Kairo's mixed port")
    args = ap.parse_args()
    delays = [20, 45, 80, 140, 260, 520]
    ports = []
    # Fixed ports (after the origin's) keep a downloaded profile valid across restarts.
    for i, d in enumerate(delays):
        await asyncio.start_server(connect_proxy(d), "127.0.0.1", args.port + 1 + i)
        ports.append(args.port + 1 + i)
    dead_port = args.port + 1 + len(delays)  # nothing listens here
    origin = f"127.0.0.1:{args.port}"
    await asyncio.start_server(lambda r, w: origin_handler(r, w, lambda: subscription(ports, dead_port, origin) + hosts_block(args.port)), "127.0.0.1", args.port)
    print(f"subscription: http://{origin}/sub  proxies: {ports}", flush=True)
    if args.traffic:
        asyncio.ensure_future(traffic(args.traffic, origin))
    await asyncio.Event().wait()


def hosts_block(port):
    # Every demo host resolves to the local origin.
    return "hosts:\n" + "".join(f"  '{h}': 127.0.0.1\n" for h in HOSTS)


if __name__ == "__main__":
    asyncio.run(main())
