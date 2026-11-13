#!/usr/bin/env python3
"""Exercise the Android app using Slint scene inspection and native input."""

import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

PACKAGE = "dev.kunianki.app"
ACTIVITY = f"{PACKAGE}/android.app.NativeActivity"
POLL_SECONDS = 0.4
MCP_DEVICE_PORT = 9315
MCP_MAX_ELEMENTS = 1000


class SmokeFailure(RuntimeError):
    pass


class SlintSceneNode:
    def __init__(self, element, window_properties):
        self.element = element
        self.role = element.get("accessibleRole", "Unknown")
        self.bounds = None
        self.window_bounds = None
        try:
            scale = float(window_properties.get("scaleFactor", 0))
            position = window_properties.get("position", {})
            window_size = window_properties.get("size", {})
            origin_x = int(position.get("x", 0))
            origin_y = int(position.get("y", 0))
            width = int(window_size.get("width", 0))
            height = int(window_size.get("height", 0))
            if scale > 0 and width > 0 and height > 0:
                self.window_bounds = (
                    origin_x,
                    origin_y,
                    origin_x + width,
                    origin_y + height,
                )
                absolute = element.get("absolutePosition", {})
                size = element.get("size", {})
                left = round(origin_x + float(absolute.get("x", 0)) * scale)
                top = round(origin_y + float(absolute.get("y", 0)) * scale)
                right = round(
                    origin_x
                    + (float(absolute.get("x", 0)) + float(size.get("width", 0)))
                    * scale
                )
                bottom = round(
                    origin_y
                    + (float(absolute.get("y", 0)) + float(size.get("height", 0)))
                    * scale
                )
                self.bounds = (left, top, right, bottom)
        except (KeyError, TypeError, ValueError, OverflowError):
            pass

        label = element.get("accessibleLabel", "")
        self.attrib = {
            "text": label,
            "content-desc": label,
            "value": element.get("accessibleValue", ""),
            "hint": element.get("accessiblePlaceholderText", ""),
            "description": element.get("accessibleDescription", ""),
            "role": self.role,
            "bounds": self._bounds_text(),
        }

    def _bounds_text(self):
        if self.bounds is None:
            return ""
        left, top, right, bottom = self.bounds
        return f"[{left},{top}][{right},{bottom}]"

    def iter(self):
        return iter((self,))


class SlintScene:
    def __init__(self, window_properties, response):
        self.window_properties = window_properties
        self.response = response
        self.elements = []
        seen_handles = set()
        for element in response.get("elements", []):
            handle = element.get("handle", {})
            key = (str(handle.get("index", "0")), str(handle.get("generation", "0")))
            if key in seen_handles:
                continue
            seen_handles.add(key)
            self.elements.append(SlintSceneNode(element, window_properties))

    def iter(self, tag=None):
        if tag not in (None, "node"):
            return iter(())
        return iter(self.elements)


class AndroidSmoke:
    def __init__(self):
        self.root = Path(__file__).resolve().parents[1]
        self.out = self.root / "target/mobile-smoke/android-ci"
        self.out.mkdir(parents=True, exist_ok=True)
        self.ui_dir = self.out / "ui"
        self.screen_dir = self.out / "screenshots"
        self.diagnostic_dir = self.out / "diagnostics"
        for directory in (self.ui_dir, self.screen_dir, self.diagnostic_dir):
            directory.mkdir(parents=True, exist_ok=True)
        self.command_log = self.out / "adb-commands.log"
        self.driver_log = self.out / "driver.log"
        self.adb = shutil.which("adb")
        if not self.adb:
            raise SmokeFailure("adb is not on PATH")
        self.serial = self._select_device()
        self.report = {
            "status": "running",
            "package": PACKAGE,
            "activity": ACTIVITY,
            "device_serial": self.serial,
            "input_method": (
                "adb shell input tap using Slint MCP scene labels and geometry; "
                "no MCP click/callback"
            ),
            "native_android_accessibility": (
                "Not exposed by Slint 1.18.1 android-activity; smoke selectors use "
                "the embedded live-scene inspector"
            ),
            "rounds": [],
            "screenshots": [],
            "assertions": [],
        }
        self.mcp_host_port = None
        self.rpc_id = 0
        self.latest_scene = None
        self.latest_scene_json = ""

    def log(self, message):
        line = str(message)
        print(line, flush=True)
        with self.driver_log.open("a", encoding="utf-8") as stream:
            stream.write(line + "\n")

    def _select_device(self):
        result = subprocess.run(
            [self.adb, "devices", "-l"], capture_output=True, text=True, timeout=20
        )
        if result.returncode:
            raise SmokeFailure(f"adb devices failed: {result.stderr.strip()}")
        devices = [
            line.split()[0]
            for line in result.stdout.splitlines()
            if line.startswith("emulator-")
            and len(line.split()) > 1
            and line.split()[1] == "device"
        ]
        if len(devices) != 1:
            raise SmokeFailure(
                f"Expected one running emulator and will not touch other devices; found {devices!r}."
            )
        requested = os.environ.get("ANDROID_SERIAL")
        if requested and requested != devices[0]:
            raise SmokeFailure(
                f"ANDROID_SERIAL={requested!r} does not identify the sole running emulator {devices[0]!r}."
            )
        return devices[0]

    def adb_run(self, *args, input_data=None, timeout=30, check=True):
        command = [self.adb, "-s", self.serial, *map(str, args)]
        with self.command_log.open("a", encoding="utf-8") as stream:
            stream.write("$ " + " ".join(command) + "\n")
        try:
            result = subprocess.run(
                command,
                input=input_data,
                capture_output=True,
                timeout=timeout,
            )
        except subprocess.TimeoutExpired as error:
            raise SmokeFailure(f"Timed out running adb {args!r}") from error
        if check and result.returncode:
            detail = result.stderr.decode("utf-8", errors="replace").strip()
            raise SmokeFailure(f"adb {args!r} failed ({result.returncode}): {detail}")
        return result

    def open_mcp_forward(self):
        if self.mcp_host_port is not None:
            return
        with socket.socket() as host_socket:
            host_socket.bind(("127.0.0.1", 0))
            self.mcp_host_port = host_socket.getsockname()[1]
        self.adb_run(
            "forward",
            f"tcp:{self.mcp_host_port}",
            f"tcp:{MCP_DEVICE_PORT}",
        )
        self.log(
            f"Forwarded host TCP {self.mcp_host_port} to app-local MCP TCP {MCP_DEVICE_PORT}"
        )

    def mcp_request(self, method, params):
        if self.mcp_host_port is None:
            raise SmokeFailure("Slint MCP adb port-forward has not been created")
        self.rpc_id += 1
        request_body = json.dumps(
            {"jsonrpc": "2.0", "id": self.rpc_id, "method": method, "params": params}
        ).encode()
        request = urllib.request.Request(
            f"http://127.0.0.1:{self.mcp_host_port}/mcp",
            data=request_body,
            headers={
                "Content-Type": "application/json",
                "Accept": "application/json, text/event-stream",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=3) as response:
                payload = json.loads(response.read())
        except (urllib.error.URLError, TimeoutError, OSError, json.JSONDecodeError) as error:
            raise SmokeFailure(f"Slint MCP request {method!r} failed: {error}") from error
        if "error" in payload:
            raise SmokeFailure(f"Slint MCP request {method!r}: {payload['error']}")
        result = payload.get("result")
        if not isinstance(result, dict):
            raise SmokeFailure(f"Slint MCP request {method!r} returned no result")
        return result

    def mcp_tool(self, name, arguments):
        result = self.mcp_request(
            "tools/call", {"name": name, "arguments": arguments}
        )
        content = result.get("content", [])
        text = next(
            (block.get("text") for block in content if block.get("type") == "text"),
            None,
        )
        if result.get("isError"):
            raise SmokeFailure(f"Slint MCP tool {name!r} failed: {text or result!r}")
        if text is None:
            raise SmokeFailure(f"Slint MCP tool {name!r} returned no JSON content")
        try:
            return json.loads(text)
        except json.JSONDecodeError as error:
            raise SmokeFailure(
                f"Slint MCP tool {name!r} returned invalid JSON: {text[:500]!r}"
            ) from error

    def tree(self):
        windows = self.mcp_tool("list_windows", {}).get("windowHandles", [])
        if len(windows) != 1:
            raise SmokeFailure(
                f"Expected one live Slint window; MCP reported {len(windows)}"
            )
        window_handle = windows[0]
        window = self.mcp_tool(
            "get_window_properties", {"windowHandle": window_handle}
        )
        root_handle = window.get("rootElementHandle")
        if not isinstance(root_handle, dict):
            raise SmokeFailure("Slint MCP window properties omitted rootElementHandle")
        response = self.mcp_tool(
            "get_element_tree",
            {"elementHandle": root_handle, "maxElements": MCP_MAX_ELEMENTS},
        )
        if response.get("debugInfoMissing"):
            raise SmokeFailure(
                "Slint element debug metadata is missing; build the smoke feature with "
                "SLINT_EMIT_DEBUG_INFO=1"
            )
        if response.get("truncated"):
            raise SmokeFailure(
                f"Slint scene exceeds the {MCP_MAX_ELEMENTS}-element inspection limit"
            )
        self.latest_scene = SlintScene(window, response)
        self.latest_scene_json = json.dumps(
            {
                "windowHandle": window_handle,
                "windowProperties": window,
                "scene": response,
            },
            ensure_ascii=False,
            indent=2,
        )
        return self.latest_scene

    @staticmethod
    def values(tree):
        return [
            value.strip()
            for node in tree.iter("node")
            for key in ("text", "content-desc", "value", "hint", "description")
            if (value := node.attrib.get(key, "").strip())
        ]


    def wait_for(self, predicate, description, timeout=30):
        deadline = time.monotonic() + timeout
        last_scene = None
        last_error = ""
        while time.monotonic() < deadline:
            try:
                last_scene = self.tree()
                last_error = ""
                if predicate(last_scene):
                    return last_scene
            except SmokeFailure as error:
                last_error = str(error)
            time.sleep(POLL_SECONDS)
        visible = self.describe_tree(last_scene) if last_scene is not None else "no Slint scene"
        self.save_diagnostics(last_scene)
        details = f"; last Slint MCP error: {last_error}" if last_error else ""
        raise SmokeFailure(
            f"Timed out waiting for {description}{details}. Slint scene elements: {visible}"
        )

    @staticmethod
    def describe_tree(tree):
        if tree is None:
            return "unavailable"
        elements = [
            f"{node.role}: {node.attrib.get('content-desc', '')}"
            for node in tree.iter("node")
            if node.attrib.get("content-desc", "").strip()
        ]
        return repr(elements[:60]) if elements else "no labeled Slint scene elements"

    @staticmethod
    def matches(node, label, prefix=False):
        for key in ("text", "content-desc", "value", "hint"):
            value = node.attrib.get(key, "").strip()
            if value == label or (prefix and value.startswith(label)):
                return True
        return False

    def find_node(self, tree, label, prefix=False):
        matches = [
            node
            for node in tree.iter("node")
            if self.matches(node, label, prefix)
        ]
        return next(
            (node for node in matches if node.role == "Button"),
            matches[0] if matches else None,
        )

    def tap_node(self, node, description):
        if node.role != "Button":
            raise SmokeFailure(
                f"Slint element {description!r} has role {node.role!r}, not Button"
            )
        if node.bounds is None or node.window_bounds is None:
            raise SmokeFailure(
                f"Slint element {description!r} has no usable absolute geometry"
            )
        left, top, right, bottom = node.bounds
        if right <= left or bottom <= top:
            raise SmokeFailure(f"Invalid Slint geometry for {description!r}: {node.bounds}")
        x, y = (left + right) // 2, (top + bottom) // 2
        if x < 0 or y < 0:
            raise SmokeFailure(
                f"Slint element {description!r} maps to invalid screen coordinates {(x, y)}"
            )
        window_left, window_top, window_right, window_bottom = node.window_bounds
        if not (window_left <= x < window_right and window_top <= y < window_bottom):
            raise SmokeFailure(
                f"Slint element {description!r} center {(x, y)} is outside native "
                f"window bounds {node.window_bounds}"
            )
        self.adb_run("shell", "input", "tap", x, y)
        self.log(
            f"Native tap: {description} at Slint scene bounds "
            f"[{left},{top}][{right},{bottom}]"
        )

    def tap_label(self, tree, label, prefix=False):
        buttons = [
            node
            for node in tree.iter("node")
            if node.role == "Button" and self.matches(node, label, prefix)
        ]
        if len(buttons) != 1:
            raise SmokeFailure(
                f"Expected one Slint Button matching {label!r}, found {len(buttons)}; "
                f"scene: {self.describe_tree(tree)}"
            )
        self.tap_node(buttons[0], label)

    def snapshot(self, name):
        tree = self.tree()
        screenshot = self.adb_run("exec-out", "screencap", "-p").stdout
        if not screenshot.startswith(b"\x89PNG\r\n\x1a\n") or len(screenshot) < 2048:
            raise SmokeFailure(f"{name}: emulator did not return a usable raw PNG screenshot")
        (self.ui_dir / f"{name}.json").write_text(
            self.latest_scene_json, encoding="utf-8"
        )
        (self.screen_dir / f"{name}.png").write_bytes(screenshot)
        self.report["screenshots"].append(f"screenshots/{name}.png")
        self.log(f"Captured raw screenshot and live Slint scene: {name}")
        return tree

    def save_diagnostics(self, tree=None):
        try:
            if tree is None:
                tree = self.tree()
            (self.diagnostic_dir / "current-scene.json").write_text(
                self.latest_scene_json, encoding="utf-8"
            )
        except Exception as error:
            (self.diagnostic_dir / "mcp-error.txt").write_text(
                f"{error}\n", encoding="utf-8"
            )
        try:
            image = self.adb_run("exec-out", "screencap", "-p").stdout
            if image.startswith(b"\x89PNG\r\n\x1a\n"):
                (self.diagnostic_dir / "current-screen.png").write_bytes(image)
        except Exception as error:
            self.log(f"Diagnostic screenshot unavailable: {error}")

    def assert_has(self, tree, expected):
        text = "\n".join(self.values(tree))
        if expected not in text:
            raise SmokeFailure(
                f"Expected accessible Japanese text {expected!r}; visible nodes: {self.describe_tree(tree)}"
            )

    @staticmethod
    def question_number(tree):
        for value in AndroidSmoke.values(tree):
            match = re.search(r"第\s*(\d+)\s*問", value)
            if match:
                return int(match.group(1))
        return None

    @staticmethod
    def score(tree):
        for value in AndroidSmoke.values(tree):
            match = re.search(r"得点\s*(\d+)", value)
            if match:
                return int(match.group(1))
        return None

    def assert_score(self, tree, expected):
        actual = self.score(tree)
        if actual != expected:
            raise SmokeFailure(f"Visible quiz score is {actual!r}, expected {expected}")

    @staticmethod
    def choices(tree):
        result = {}
        pattern = re.compile(r"選択肢\s*(\d+)\s*:\s*(.*)", re.DOTALL)
        for node in tree.iter("node"):
            if node.role != "Button":
                continue
            label = node.attrib.get("content-desc", "").strip()
            if not label.startswith("選択肢"):
                label = node.attrib.get("text", "").strip()
            match = pattern.fullmatch(label)
            if match:
                index = int(match.group(1))
                result[index] = (match.group(2).strip(), node)
        if set(result) != {1, 2, 3, 4}:
            raise SmokeFailure(
                f"Expected four Slint Button answer labels with geometry; found {sorted(result)}. "
                "No guessed coordinates or screenshot-based selectors are permitted."
            )
        return result

    def answer_statuses(self, tree):
        statuses = {}
        for index, (_, node) in self.choices(tree).items():
            details = " ".join(
                value
                for child in node.iter()
                for key in ("text", "content-desc", "value", "hint", "description")
                if (value := child.attrib.get(key, "").strip())
            )
            if "不正解" in details:
                statuses[index] = False
            elif "正解" in details:
                statuses[index] = True
        return statuses if len(statuses) == 4 and sum(statuses.values()) == 1 else None

    def wait_question(self, number, title):
        def ready(tree):
            return self.question_number(tree) == number and title in "\n".join(self.values(tree))

        return self.wait_for(ready, f"{title}, question {number}")

    @staticmethod
    def hints_shown(tree):
        found = set()
        for value in AndroidSmoke.values(tree):
            match = re.fullmatch(r"ヒント\s*(\d+)", value)
            if match:
                found.add(int(match.group(1)))
        return found

    def assert_image(self, tree, mode):
        placeholder = "国旗を読み込みます" if mode == "国旗" else "地図を読み込みます"
        if placeholder in "\n".join(self.values(tree)):
            raise SmokeFailure(f"{mode} image is still showing its loading placeholder")
        self.report["assertions"].append(
            f"{mode} image is loaded (loading placeholder absent; raw screenshot captured)"
        )

    def result_values(self, tree, total, correct, score):
        self.assert_has(tree, "今回の結果")
        ratios = []
        for value in self.values(tree):
            match = re.fullmatch(r"\s*(\d+)\s*/\s*(\d+)\s*", value)
            if match:
                ratios.append((int(match.group(1)), int(match.group(2))))
        if (correct, total) not in ratios:
            raise SmokeFailure(f"Visible result ratio {ratios!r}; expected {correct}/{total}")
        numeric_labels = {
            int(value)
            for value in self.values(tree)
            if re.fullmatch(r"\s*\d+\s*", value)
        }
        if score not in numeric_labels:
            raise SmokeFailure(f"Visible result score omits {score}; numeric labels: {numeric_labels}")

    @staticmethod
    def progress_box_one_count(progress):
        entries = progress.get("entries")
        if not isinstance(entries, dict):
            raise SmokeFailure("Private progress.json has no entries object")
        count = 0
        for kinds in entries.values():
            if not isinstance(kinds, dict):
                raise SmokeFailure("Private progress.json contains malformed country entries")
            for entry in kinds.values():
                box = entry.get("box_number") if isinstance(entry, dict) else None
                if box not in (1, 2, 3):
                    raise SmokeFailure(f"Private progress.json has invalid Leitner box: {entry!r}")
                count += box == 1
        return count

    def read_progress(self, save_as=None):
        result = self.adb_run("shell", f"run-as {PACKAGE} cat files/progress.json")
        raw = result.stdout
        try:
            progress = json.loads(raw)
        except (json.JSONDecodeError, UnicodeDecodeError) as error:
            raise SmokeFailure(f"Could not decode app-private files/progress.json: {error}") from error
        if save_as:
            (self.out / save_as).write_bytes(raw)
        return progress, raw

    def review_count(self, tree):
        prefix = "まちがえた国を復習"
        button = self.find_node(tree, prefix, prefix=True)
        if button is None:
            raise SmokeFailure(
                f"Review control {prefix!r} is missing from the Slint scene: "
                f"{self.describe_tree(tree)}"
            )
        label = " ".join(
            button.attrib.get(key, "") for key in ("text", "content-desc", "value", "hint")
        )
        candidates = set()
        value = button.attrib.get("value", "").strip()
        match = re.fullmatch(r"\s*(\d+)\s*(?:問)?\s*", value)
        if match:
            candidates.add(int(match.group(1)))
        for match in re.finditer(r"(?<!\d)(\d+)\s*問", label):
            candidates.add(int(match.group(1)))
        for match in re.finditer(r"まちがえた国を復習[、, ]+(\d+)", label):
            candidates.add(int(match.group(1)))
        bounds_match = re.fullmatch(
            r"\[(-?\d+),(-?\d+)\]\[(-?\d+),(-?\d+)\]", button.attrib.get("bounds", "")
        )
        if bounds_match:
            left, top, right, bottom = map(int, bounds_match.groups())
            for node in tree.iter("node"):
                child_bounds = re.fullmatch(
                    r"\[(-?\d+),(-?\d+)\]\[(-?\d+),(-?\d+)\]",
                    node.attrib.get("bounds", ""),
                )
                if not child_bounds:
                    continue
                x1, y1, x2, y2 = map(int, child_bounds.groups())
                if not (left <= x1 and top <= y1 and x2 <= right and y2 <= bottom):
                    continue
                for key in ("text", "content-desc", "value", "hint"):
                    value = node.attrib.get(key, "").strip()
                    match = re.fullmatch(r"(\d+)\s*(?:問)?", value)
                    if match:
                        candidates.add(int(match.group(1)))
        if not candidates:
            raise SmokeFailure(
                "Slint accessible label exposes no review count; expected `、{count}問` "
                "or a numeric child within the labeled control."
            )
        if len(candidates) != 1:
            raise SmokeFailure(f"Review control has ambiguous accessible counts: {sorted(candidates)}")
        return candidates.pop()

    def assert_home_count(self, tree, progress):
        self.assert_has(tree, "クイズを始める")
        actual = self.review_count(tree)
        expected = self.progress_box_one_count(progress)
        if actual != expected:
            raise SmokeFailure(f"Home review count is {actual}; private progress.json implies {expected}")
        self.report["assertions"].append(f"home review count matches private progress: {actual}")

    def launch(self):
        self.adb_run("shell", "am", "start", "-n", ACTIVITY)
        return self.wait_for(
            lambda tree: self.find_node(tree, "クイズを始める") is not None,
            "Japanese home screen and start button",
        )

    def run_round(self, mode, title):
        self.log(f"Starting full {mode} round with native taps from live Slint geometry")
        tree = self.wait_for(
            lambda current: self.find_node(current, "クイズを始める") is not None,
            "home start button",
        )
        self.assert_has(tree, "旗・地図・3つのヒント")
        self.snapshot(f"home-before-{mode}")
        self.tap_label(tree, "クイズを始める")
        tree = self.wait_for(
            lambda current: "どんな方法で覚える？" in "\n".join(self.values(current)),
            "quiz selection screen",
        )
        self.snapshot(f"select-{mode}")
        self.tap_label(tree, mode)
        tree = self.tree()
        self.tap_label(tree, "全地域")
        tree = self.tree()
        self.tap_label(tree, "10問を始める")
        tree = self.wait_question(1, title)
        if mode != "3ヒント":
            self.assert_image(tree, mode)
            self.snapshot(f"{mode}-question")
        else:
            if self.hints_shown(tree) != {1}:
                raise SmokeFailure(f"Trivia should initially show hint 1: {self.hints_shown(tree)}")
            for count in (2, 3):
                self.tap_label(tree, "次のヒントを見る")
                tree = self.wait_for(
                    lambda current, count=count: self.hints_shown(current) == set(range(1, count + 1)),
                    f"trivia hint {count}",
                )
            self.snapshot("trivia-three-hints")

        expected_score = 0
        correct_count = 0
        for number in range(1, 11):
            if number > 1:
                tree = self.wait_question(number, title)
            self.assert_score(tree, expected_score)
            options = self.choices(tree)
            if number == 1 and mode == "3ヒント" and self.hints_shown(tree) != {1, 2, 3}:
                raise SmokeFailure("All three trivia hints must be visibly present before answering")
            selected = options[1][1]
            self.tap_node(selected, f"{mode} answer {number}: {options[1][0]}")
            if mode == "3ヒント":
                points = 1 if number == 1 else 3
            else:
                points = 1
            answered = self.wait_for(
                lambda current: self.answer_statuses(current) is not None,
                f"four Slint answer elements exposing 正解/不正解 descriptions after question {number}",
            )
            statuses = self.answer_statuses(answered)
            if statuses[1]:
                correct_count += 1
                expected_score += points
            self.assert_score(answered, expected_score)
            if number == 1:
                self.snapshot(f"{mode}-answer-feedback")
            self.tap_label(answered, "次の問題へ")
        result = self.wait_for(
            lambda current: "今回の結果" in "\n".join(self.values(current)), "full-round results"
        )
        self.result_values(result, 10, correct_count, expected_score)
        self.snapshot(f"{mode}-results")
        self.report["rounds"].append(
            {"mode": mode, "questions": 10, "correct": correct_count, "score": expected_score}
        )
        self.tap_label(result, "ホームへ戻る")
        home = self.wait_for(
            lambda current: self.find_node(current, "クイズを始める") is not None,
            "home after results",
        )
        progress, _ = self.read_progress(f"progress-after-{mode}.json")
        self.assert_home_count(home, progress)
        self.snapshot(f"home-after-{mode}")

    def start_review(self):
        tree = self.wait_for(
            lambda current: self.find_node(current, "まちがえた国を復習", prefix=True) is not None,
            "home review control",
        )
        self.tap_label(tree, "まちがえた国を復習", prefix=True)
        return self.wait_question(1, "国旗を見て答えよう")

    def review_fixture(self, kind):
        self.adb_run("shell", "am", "force-stop", PACKAGE)
        fixture = {"entries": {"JP": {kind: {"box_number": 1}}}}
        encoded = json.dumps(fixture, ensure_ascii=False, separators=(",", ":")).encode()
        self.adb_run(
            "shell",
            f"run-as {PACKAGE} sh -c 'cat > files/progress.json'",
            input_data=encoded,
        )
        stored, _ = self.read_progress()
        if stored != fixture:
            raise SmokeFailure(f"App-private progress fixture did not persist: {stored!r}")
        home = self.launch()
        self.assert_home_count(home, stored)
        self.log(f"Seeded app-private progress.json with one real {kind} review item")

    def review_flag_wrong_then_correct(self):
        self.review_fixture("flag")
        self.snapshot("home-review-flag-seeded")
        tree = self.start_review()
        self.assert_image(tree, "国旗")
        self.snapshot("review-flag-question")
        options = self.choices(tree)
        japan = [index for index, (name, _) in options.items() if name == "日本"]
        if len(japan) != 1:
            raise SmokeFailure(f"Seeded JP flag review did not show one 日本 choice: {options!r}")
        wrong = next(index for index in options if index != japan[0])
        self.tap_node(options[wrong][1], "deliberately wrong answer in seeded JP flag review")
        answered = self.wait_for(
            lambda current: self.answer_statuses(current) is not None,
            "wrong-answer feedback on seeded JP flag review",
        )
        if self.answer_statuses(answered)[wrong]:
            raise SmokeFailure("The deliberately non-日本 answer was unexpectedly marked correct")
        self.assert_score(answered, 0)
        self.snapshot("review-flag-wrong")
        result = self.wait_for(
            lambda current: self.find_node(current, "次の問題へ") is not None,
            "review answer continuation",
        )
        self.tap_label(result, "次の問題へ")
        result = self.wait_for(
            lambda current: "今回の結果" in "\n".join(self.values(current)), "wrong-review result"
        )
        self.result_values(result, 1, 0, 0)
        self.snapshot("review-flag-wrong-results")
        self.tap_label(result, "ホームへ戻る")
        home = self.wait_for(
            lambda current: self.find_node(current, "クイズを始める") is not None,
            "home after wrong review",
        )
        progress, _ = self.read_progress("progress-after-review-wrong.json")
        self.assert_home_count(home, progress)
        if progress["entries"]["JP"]["flag"]["box_number"] != 1:
            raise SmokeFailure("Wrong review answer did not preserve JP flag in Leitner box one")
        self.snapshot("home-after-review-wrong")

        tree = self.start_review()
        options = self.choices(tree)
        japan = [index for index, (name, _) in options.items() if name == "日本"]
        if len(japan) != 1:
            raise SmokeFailure(f"Restarted JP flag review did not show one 日本 choice: {options!r}")
        self.tap_node(options[japan[0]][1], "correct 日本 answer after restarting review")
        answered = self.wait_for(
            lambda current: self.answer_statuses(current) is not None,
            "correct-answer feedback after restarting JP review",
        )
        if not self.answer_statuses(answered)[japan[0]]:
            raise SmokeFailure("The known 日本 answer was not marked correct in the restarted review")
        self.assert_score(answered, 1)
        self.snapshot("review-flag-correct")
        result = self.wait_for(
            lambda current: self.find_node(current, "次の問題へ") is not None,
            "correct review answer continuation",
        )
        self.tap_label(result, "次の問題へ")
        result = self.wait_for(
            lambda current: "今回の結果" in "\n".join(self.values(current)), "correct-review result"
        )
        self.result_values(result, 1, 1, 1)
        self.snapshot("review-flag-correct-results")
        self.tap_label(result, "ホームへ戻る")
        home = self.wait_for(
            lambda current: self.find_node(current, "クイズを始める") is not None,
            "home after correct review",
        )
        progress, _ = self.read_progress("progress-after-review-correct.json")
        self.assert_home_count(home, progress)
        if progress["entries"]["JP"]["flag"]["box_number"] != 2:
            raise SmokeFailure("Correct review answer did not advance JP flag out of box one")

    def review_trivia_hints_and_score(self):
        self.review_fixture("trivia")
        tree = self.wait_for(
            lambda current: self.find_node(current, "まちがえた国を復習", prefix=True) is not None,
            "JP trivia review control",
        )
        self.snapshot("home-review-trivia-seeded")
        self.tap_label(tree, "まちがえた国を復習", prefix=True)
        tree = self.wait_question(1, "ヒントから国を当てよう")
        if self.hints_shown(tree) != {1}:
            raise SmokeFailure(f"Seeded trivia should begin with hint 1: {self.hints_shown(tree)}")
        self.assert_score(tree, 0)
        for count in (2, 3):
            self.tap_label(tree, "次のヒントを見る")
            tree = self.wait_for(
                lambda current, count=count: self.hints_shown(current) == set(range(1, count + 1)),
                f"seeded JP review trivia hint {count}",
            )
        self.snapshot("review-trivia-three-hints")
        options = self.choices(tree)
        japan = [index for index, (name, _) in options.items() if name == "日本"]
        if len(japan) != 1:
            raise SmokeFailure(f"Seeded JP trivia review did not show one 日本 choice: {options!r}")
        self.tap_node(options[japan[0]][1], "correct 日本 answer after three trivia hints")
        answered = self.wait_for(
            lambda current: self.answer_statuses(current) is not None,
            "correct-answer feedback after three JP trivia hints",
        )
        if not self.answer_statuses(answered)[japan[0]]:
            raise SmokeFailure("The known 日本 trivia answer was not marked correct")
        self.assert_score(answered, 1)
        self.snapshot("review-trivia-correct")
        result = self.wait_for(
            lambda current: self.find_node(current, "次の問題へ") is not None,
            "trivia review answer continuation",
        )
        self.tap_label(result, "次の問題へ")
        result = self.wait_for(
            lambda current: "今回の結果" in "\n".join(self.values(current)), "trivia-review result"
        )
        self.result_values(result, 1, 1, 1)
        self.snapshot("review-trivia-results")
        self.tap_label(result, "ホームへ戻る")
        home = self.wait_for(
            lambda current: self.find_node(current, "クイズを始める") is not None,
            "home after trivia review",
        )
        trivia_progress, _ = self.read_progress()
        self.assert_home_count(home, trivia_progress)
        if trivia_progress["entries"]["JP"]["trivia"]["box_number"] != 2:
            raise SmokeFailure("Correct three-hint review did not advance JP trivia to box two")

        self.review_fixture("flag")
        tree = self.start_review()
        options = self.choices(tree)
        japan = [index for index, (name, _) in options.items() if name == "日本"]
        if len(japan) != 1:
            raise SmokeFailure(f"Seeded JP flag review did not show one 日本 choice: {options!r}")
        wrong = next(index for index in options if index != japan[0])
        self.tap_node(options[wrong][1], "deliberately wrong answer before final relaunch")
        answered = self.wait_for(
            lambda current: self.answer_statuses(current) is not None,
            "wrong-answer feedback before final relaunch",
        )
        if self.answer_statuses(answered)[wrong]:
            raise SmokeFailure("The deliberately non-日本 answer was unexpectedly marked correct")
        self.assert_score(answered, 0)
        result = self.wait_for(
            lambda current: self.find_node(current, "次の問題へ") is not None,
            "wrong-review answer continuation before final relaunch",
        )
        self.tap_label(result, "次の問題へ")
        result = self.wait_for(
            lambda current: "今回の結果" in "\n".join(self.values(current)),
            "wrong-review result before final relaunch",
        )
        self.result_values(result, 1, 0, 0)
        self.tap_label(result, "ホームへ戻る")
        home = self.wait_for(
            lambda current: self.find_node(current, "クイズを始める") is not None,
            "home with a nonzero wrong-review count",
        )
        progress, before = self.read_progress("progress-before-relaunch.json")
        self.assert_home_count(home, progress)
        review_count = self.review_count(home)
        if review_count <= 0:
            raise SmokeFailure("Wrong JP flag answer did not leave a nonzero review count")
        if progress["entries"]["JP"]["flag"]["box_number"] != 1:
            raise SmokeFailure("Wrong JP flag answer did not preserve the review item in box one")

        self.adb_run("shell", "am", "force-stop", PACKAGE)
        after_stop, after_stop_raw = self.read_progress("progress-while-force-stopped.json")
        if after_stop != progress or after_stop_raw != before:
            raise SmokeFailure("App-private progress.json changed across force-stop")
        home = self.launch()
        after_launch, after_launch_raw = self.read_progress("progress-after-relaunch.json")
        if after_launch != progress or after_launch_raw != before:
            raise SmokeFailure("App-private progress.json did not survive force-stop and relaunch")
        self.assert_home_count(home, after_launch)
        if self.review_count(home) != review_count:
            raise SmokeFailure("Nonzero home review count changed after force-stop/relaunch")
        self.snapshot("home-after-relaunch")
        self.report["assertions"].append(
            "nonzero review count from a real wrong answer and byte-identical progress.json "
            "survived force-stop/relaunch"
        )

    def execute(self):
        apk = self.root / "android/app/build/outputs/apk/debug/app-debug.apk"
        if not apk.is_file():
            raise SmokeFailure(f"Expected built debug APK at {apk}")
        self.adb_run("install", "-r", apk)
        self.open_mcp_forward()
        self.report["device_model"] = self.adb_run(
            "shell", "getprop", "ro.product.model"
        ).stdout.decode().strip()
        self.report["android_api"] = self.adb_run(
            "shell", "getprop", "ro.build.version.sdk"
        ).stdout.decode().strip()
        self.report["apk"] = str(apk.relative_to(self.root))
        home = self.launch()
        self.assert_has(home, "旗・地図・3つのヒント")
        self.snapshot("home-initial")
        for mode, title in (
            ("国旗", "国旗を見て答えよう"),
            ("地図", "地図を見て答えよう"),
            ("3ヒント", "ヒントから国を当てよう"),
        ):
            self.run_round(mode, title)
        self.review_flag_wrong_then_correct()
        self.review_trivia_hints_and_score()
        self.log("Android emulator smoke passed")

    def collect_logs(self, include_crashes=False):
        chunks = []
        pid = self.adb_run("shell", "pidof", "-s", PACKAGE, check=False).stdout.decode().strip()
        if pid:
            app_log = self.adb_run(
                "logcat", "-d", "-v", "threadtime", f"--pid={pid}", check=False
            ).stdout
            chunks.append(b"--- app process log ---\n" + app_log)
        if include_crashes:
            crash_log = self.adb_run(
                "logcat", "-d", "-v", "threadtime", "-s", "AndroidRuntime", "DEBUG", "libc", check=False
            ).stdout
            chunks.append(b"\n--- Android runtime/native crash log ---\n" + crash_log)
        if chunks:
            (self.out / "android.log").write_bytes(b"".join(chunks))


    def close(self):
        if self.mcp_host_port is None:
            return
        self.adb_run(
            "forward", "--remove", f"tcp:{self.mcp_host_port}", check=False
        )
        self.mcp_host_port = None

    def finish(self, status, error=None):
        self.report["status"] = status
        self.report["error"] = error
        (self.out / "result.json").write_text(
            json.dumps(self.report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )


def main():
    smoke = None
    try:
        smoke = AndroidSmoke()
        smoke.execute()
        smoke.collect_logs()
        smoke.finish("passed")
    except Exception as error:
        message = f"Android emulator smoke failed: {error}"
        print(message, file=sys.stderr, flush=True)
        if smoke is not None:
            smoke.log(message)
            smoke.save_diagnostics()
            smoke.collect_logs(include_crashes=True)
            smoke.finish("failed", str(error))
            return 1
    finally:
        if smoke is not None:
            smoke.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
