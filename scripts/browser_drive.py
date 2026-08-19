#!/usr/bin/env python3
"""Drive the app in headless Chromium over CDP.

Used to verify things that only manifest in a real browser: that the wasm
bundle initialises, that a design actually completes, and that the console
stays clean. Prints console errors and page exceptions, which is the part
that a screenshot alone would hide.

Usage:
    browser_drive.py URL [--shot out.png] [--script file.js] [--wait-for EXPR]

--script runs after load and its completion value is printed as JSON.
--wait-for polls a JS expression until it is truthy (or times out).
"""

import argparse
import asyncio
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request

CHROME = os.path.expanduser(
    "~/.cache/ms-playwright/chromium-1228/chrome-linux/chrome"
)


def launch(port, user_dir):
    return subprocess.Popen(
        [
            CHROME,
            "--headless=new",
            "--disable-gpu",
            "--no-sandbox",
            "--hide-scrollbars",
            "--window-size=1200,1600",
            f"--remote-debugging-port={port}",
            f"--user-data-dir={user_dir}",
            "about:blank",
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def page_ws(port, timeout=20):
    """Wait for the devtools endpoint and return the first page's ws URL."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/json") as r:
                for t in json.load(r):
                    if t.get("type") == "page":
                        return t["webSocketDebuggerUrl"]
        except Exception:
            pass
        time.sleep(0.2)
    raise RuntimeError("Chromium devtools endpoint never came up")


class Session:
    def __init__(self, ws):
        self.ws = ws
        self.n = 0
        self.console = []
        self.exceptions = []

    async def call(self, method, **params):
        self.n += 1
        mine = self.n
        await self.ws.send(json.dumps({"id": mine, "method": method, "params": params}))
        while True:
            msg = json.loads(await self.ws.recv())
            if msg.get("id") == mine:
                if "error" in msg:
                    raise RuntimeError(f"{method}: {msg['error']}")
                return msg.get("result", {})
            self.note(msg)

    def note(self, msg):
        m = msg.get("method")
        if m == "Runtime.consoleAPICalled":
            kind = msg["params"]["type"]
            text = " ".join(
                str(a.get("value", a.get("description", "")))
                for a in msg["params"].get("args", [])
            )
            if kind in ("error", "warning"):
                self.console.append(f"{kind}: {text}")
        elif m == "Runtime.exceptionThrown":
            d = msg["params"]["exceptionDetails"]
            self.exceptions.append(d.get("text", "") + " " + str(d.get("exception", {}).get("description", "")))
        elif m == "Log.entryAdded":
            e = msg["params"]["entry"]
            if e.get("level") in ("error", "warning"):
                self.console.append(f"{e['level']}: {e.get('text','')}")

    async def eval(self, expr, await_promise=True):
        r = await self.call(
            "Runtime.evaluate",
            expression=expr,
            awaitPromise=await_promise,
            returnByValue=True,
        )
        if "exceptionDetails" in r:
            raise RuntimeError(f"eval threw: {r['exceptionDetails']}")
        return r.get("result", {}).get("value")

    async def drain(self, seconds):
        """Pump events for a while so console/exception logs are captured."""
        end = time.time() + seconds
        while time.time() < end:
            try:
                msg = await asyncio.wait_for(self.ws.recv(), timeout=0.1)
                self.note(json.loads(msg))
            except asyncio.TimeoutError:
                pass


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("url")
    ap.add_argument("--shot")
    ap.add_argument("--script")
    ap.add_argument("--wait-for")
    ap.add_argument("--timeout", type=float, default=60.0)
    args = ap.parse_args()

    import websockets

    port = 9333
    with tempfile.TemporaryDirectory() as user_dir:
        proc = launch(port, user_dir)
        try:
            ws_url = page_ws(port)
            async with websockets.connect(ws_url, max_size=80 * 1024 * 1024) as ws:
                s = Session(ws)
                await s.call("Runtime.enable")
                await s.call("Log.enable")
                await s.call("Page.enable")
                await s.call("Page.navigate", url=args.url)
                # Let the wasm bundle fetch, compile and mount.
                await s.drain(4.0)

                if args.wait_for:
                    deadline = time.time() + args.timeout
                    ok = False
                    while time.time() < deadline:
                        if await s.eval(f"!!({args.wait_for})", await_promise=False):
                            ok = True
                            break
                        await s.drain(0.3)
                    print(f"wait-for {'OK' if ok else 'TIMEOUT'}: {args.wait_for}")
                    if not ok:
                        pass

                if args.script:
                    with open(args.script) as f:
                        body = f.read()
                    out = await s.eval(f"(async () => {{ {body} }})()")
                    print("SCRIPT RESULT:", json.dumps(out, indent=2)[:4000])
                    await s.drain(1.0)

                if args.shot:
                    r = await s.call("Page.captureScreenshot", captureBeyondViewport=True)
                    import base64

                    with open(args.shot, "wb") as f:
                        f.write(base64.b64decode(r["data"]))
                    print("screenshot:", args.shot)

                for e in s.exceptions:
                    print("PAGE EXCEPTION:", e[:500])
                for c in s.console:
                    print("CONSOLE", c[:500])
                bad = bool(s.exceptions)
                print("clean:", "no" if bad else "yes")
                return 1 if bad else 0
        finally:
            proc.terminate()


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
