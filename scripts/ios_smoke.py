#!/usr/bin/env python3
"""Drive a freshly installed iOS Slint smoke app via its loopback test server."""

import argparse
import json
import subprocess
import time
import urllib.request
from pathlib import Path


class SmokeFailure(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise SmokeFailure(message)


def run(device, port, output):
    def call(name, **arguments):
        request = urllib.request.Request(
            f"http://127.0.0.1:{port}/mcp",
            json.dumps({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            }).encode(),
            {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"},
        )
        with urllib.request.urlopen(request, timeout=15) as response:
            message = json.load(response)
        require("result" in message, f"{name}: {message}")
        result = message["result"]
        require(not result.get("isError"), f"{name}: {result}")
        return json.loads(result["content"][0]["text"])

    window, = call("list_windows")["windowHandles"]
    root = call("get_window_properties", windowHandle=window)["rootElementHandle"]

    def nodes():
        return call("get_element_tree", elementHandle=root, maxElements=1000)["elements"]

    def has(label):
        require(any(n.get("accessibleLabel") == label for n in nodes()), f"Missing {label!r}")

    def click(label, *, prefix=False):
        for node in nodes():
            text = node.get("accessibleLabel", "")
            if text == label or (prefix and text.startswith(label)):
                call("click_element", elementHandle=node["handle"])
                return
        raise SmokeFailure(f"Cannot click {label!r}: not present in live scene")

    def screenshot(name):
        # The scene RPC sees state changes before CoreSimulator presents the next frame.
        time.sleep(0.5)
        path = output / f"{name}.png"
        subprocess.run(["xcrun", "simctl", "io", device, "screenshot", path], check=True, timeout=90)
        with path.open("rb") as image:
            require(image.read(8) == b"\x89PNG\r\n\x1a\n", f"Invalid screenshot: {path}")

    output.mkdir(parents=True, exist_ok=True)
    has("まちがえた国を復習、0問")
    screenshot("home")
    click("まちがえた国を復習、0問")
    has("クイズを始める")
    for label in ("クイズを始める", "国旗", "全地域", "10問を始める"):
        click(label)
    has("国旗を見て答えよう")
    screenshot("question")
    click("クイズを中断")
    has("続ける")
    screenshot("quit-modal")
    click("続ける")
    for number in range(1, 11):
        has(f"第 {number} 問")
        click("選択肢 1:", prefix=True)
        if number == 1:
            screenshot("answered")
            click("選択肢 1:", prefix=True)
            has("第 1 問")
        click("次の問題へ")
    has("今回の結果")
    screenshot("result")
    click("ホームへ戻る")
    has("クイズを始める")
    print("iOS Slint smoke passed: disabled review, quit/resume, 10 answers, result, return home")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", required=True, help="dedicated booted simulator UDID")
    parser.add_argument("--port", required=True, type=int, help="Slint MCP loopback port")
    parser.add_argument("--output", required=True, type=Path, help="screenshot directory")
    args = parser.parse_args()
    run(args.device, args.port, args.output)
