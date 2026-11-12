#!/usr/bin/env python3
"""A self-made provider for the web e2e: two proxy nodes that count what goes
through each, an origin, and a subscription naming the two.

    nodes.py PORT HITS_DIR   origin on PORT, nodes on PORT+1 (Tokyo) and PORT+2 (Seattle),
                             delay tests' origin on PORT+3

Every CONNECT a node carries is counted in HITS_DIR/<node>. 203.0.113.x (the
address used through TUN) is the origin on this machine.
"""
import asyncio, os, sys

PORT = int(sys.argv[1])
HITS = sys.argv[2]
NODES = {"tokyo": PORT + 1, "seattle": PORT + 2}


def count(node):
    path = os.path.join(HITS, node)
    n = int(open(path).read()) if os.path.exists(path) else 0
    with open(path, "w") as f:
        f.write(str(n + 1))


async def pipe(r, w):
    try:
        while data := await r.read(65536):
            w.write(data)
            await w.drain()
    except Exception:
        pass
    finally:
        w.close()


def node(name):
    async def handle(r, w):
        try:
            head = await r.readuntil(b"\r\n\r\n")
        except Exception:
            w.close()
            return
        parts = head.split(b" ")
        if parts[0] != b"CONNECT":
            w.write(b"HTTP/1.1 405 No\r\n\r\n")
            w.close()
            return
        host, _, port = parts[1].decode().rpartition(":")
        if host.startswith("203.0.113.") or host.endswith(".test"):
            host = "127.0.0.1"
        if int(port) == PORT:
            count(name)
        try:
            ur, uw = await asyncio.open_connection(host, int(port))
        except Exception:
            w.write(b"HTTP/1.1 502 Bad Gateway\r\n\r\n")
            w.close()
            return
        w.write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        await w.drain()
        await asyncio.gather(pipe(r, uw), pipe(ur, w))
    return handle


SUB = f"""proxies:
  - {{name: "🇯🇵 Tokyo 01", type: http, server: 127.0.0.1, port: {NODES['tokyo']}}}
  - {{name: "🇺🇸 Seattle 01", type: http, server: 127.0.0.1, port: {NODES['seattle']}}}
proxy-groups:
  - {{name: "Proxy", type: select, proxies: ["🇯🇵 Tokyo 01", "🇺🇸 Seattle 01"]}}
rules:
  - MATCH,Proxy
"""


async def origin(r, w):
    try:
        head = await r.readuntil(b"\r\n\r\n")
    except Exception:
        w.close()
        return
    path = head.split(b" ")[1].decode()
    if path.startswith("/sub"):
        body = SUB.encode()
        w.write(b"HTTP/1.1 200 OK\r\nContent-Type: text/yaml\r\nContent-Length: %d\r\nConnection: close\r\n\r\n" % len(body) + body)
    elif path.endswith("/generate_204"):
        w.write(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    else:
        w.write(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nhello\n")
    await w.drain()
    w.close()


async def main():
    os.makedirs(HITS, exist_ok=True)
    for name, port in NODES.items():
        await asyncio.start_server(node(name), "127.0.0.1", port)
    await asyncio.start_server(origin, "127.0.0.1", PORT)
    # Delay tests go here, so they are not counted as the person's requests.
    await asyncio.start_server(origin, "127.0.0.1", PORT + 3)
    print(f"ready http://127.0.0.1:{PORT}/sub?token=e2e-secret-4f1c9a", flush=True)
    await asyncio.Event().wait()


asyncio.run(main())
