#!/usr/bin/env python3
"""PTY acceptance and resource measurements; uses only the Python standard library.

All config, sessions, workspaces, credentials, and provider requests are isolated in
temporary directories. The provider is a loopback fixture, never a paid service.
"""
import argparse
import base64
import codecs
import unicodedata
import fcntl
import http.server
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import queue
import re
import select
import shutil
import signal
import statistics
import struct
import subprocess
import tempfile
import termios
import threading
import time
import tomllib
import uuid


def completion(text="", calls=(), usage=None):
    delta = {"content": text}
    if calls:
        delta["tool_calls"] = [
            {"index": i, "id": identity, "type": "function", "function": {
                "name": name, "arguments": json.dumps(args)}}
            for i, (identity, name, args) in enumerate(calls)
        ]
    value = {"choices": [{"index": 0, "delta": delta,
                          "finish_reason": "tool_calls" if calls else "stop"}]}
    if usage is not None: value["usage"] = usage
    return "data: " + json.dumps(value, ensure_ascii=False) + "\n\ndata: [DONE]\n\n"


class Provider(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self):
        super().__init__(("127.0.0.1", 0), Handler)
        self.replies = queue.Queue()
        self.requests = []
        self.lock = threading.Lock()


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        with self.server.lock:
            self.server.requests.append(body)
        try:
            reply, delay = self.server.replies.get(timeout=5)
            time.sleep(delay)
            data = reply.encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass


class Grid:
    """Small VT screen observer for assertions against the final rendered frame."""
    def __init__(self, width, height):
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.pending = ""
        self.x = self.y = 0
        self.resize(width, height)

    def resize(self, width, height):
        self.width, self.height = width, height
        self.cells = [[" " for _ in range(width)] for _ in range(height)]
        self.x = min(self.x, width - 1)
        self.y = min(self.y, height - 1)

    def feed(self, data):
        text = self.pending + self.decoder.decode(data)
        self.pending = ""
        index = 0
        while index < len(text):
            char = text[index]
            if char == "\x1b":
                if index + 1 == len(text):
                    break
                kind = text[index + 1]
                if kind == "[":
                    end = index + 2
                    while end < len(text) and not ("@" <= text[end] <= "~"):
                        end += 1
                    if end == len(text):
                        break
                    params = text[index + 2:end]
                    command = text[end]
                    if params == "?1049" and command == "h":
                        self.cells = [[" " for _ in range(self.width)] for _ in range(self.height)]
                        self.x = self.y = 0
                    if not params.startswith(("?", ">", "<", "=")):
                        values = [int(v) if v.isdigit() else 0 for v in params.split(";")]
                        value = values[0] or 1
                        if command in ("H", "f"):
                            self.y = max(0, min(self.height - 1, value - 1))
                            self.x = max(0, min(self.width - 1, (values[1] or 1) - 1 if len(values) > 1 else 0))
                        elif command == "G": self.x = min(self.width - 1, value - 1)
                        elif command == "d": self.y = min(self.height - 1, value - 1)
                        elif command == "A": self.y = max(0, self.y - value)
                        elif command == "B": self.y = min(self.height - 1, self.y + value)
                        elif command == "C": self.x = min(self.width - 1, self.x + value)
                        elif command == "D": self.x = max(0, self.x - value)
                        elif command == "J" and values[0] in (2, 3):
                            self.cells = [[" " for _ in range(self.width)] for _ in range(self.height)]
                        elif command == "K":
                            start, stop = (0, self.width) if values[0] == 2 else ((0, self.x + 1) if values[0] == 1 else (self.x, self.width))
                            self.cells[self.y][start:stop] = [" "] * (stop - start)
                    index = end + 1
                    continue
                if kind in ("]", "P"):
                    bell = text.find("\x07", index + 2)
                    st = text.find("\x1b\\", index + 2)
                    endings = [(bell, 1), (st, 2)]
                    endings = [(at, size) for at, size in endings if at >= 0]
                    if not endings: break
                    end, size = min(endings)
                    index = end + size
                    continue
                index += 2
                continue
            if char == "\r": self.x = 0
            elif char == "\n": self.y = min(self.height - 1, self.y + 1)
            elif char == "\b": self.x = max(0, self.x - 1)
            elif not unicodedata.category(char).startswith("C"):
                if unicodedata.combining(char):
                    if self.x: self.cells[self.y][self.x - 1] += char
                else:
                    size = 2 if unicodedata.east_asian_width(char) in ("W", "F") else 1
                    if self.x >= self.width: self.x = 0; self.y = min(self.height - 1, self.y + 1)
                    self.cells[self.y][self.x] = char
                    if size == 2 and self.x + 1 < self.width: self.cells[self.y][self.x + 1] = ""
                    self.x += size
            index += 1
        self.pending = text[index:]

    def text(self):
        return "\n".join("".join(row) for row in self.cells)

    def locate(self, text):
        for y, row in enumerate(self.cells):
            # ASCII UI controls: cells retain exact terminal coordinates.
            line = "".join(c if len(c) == 1 else " " for c in row)
            x = line.find(text)
            if x >= 0: return x, y
        raise AssertionError(f"Text {text!r} not visible:\n{self.text()}")


class Terminal:
    def __init__(self, binary, workspace, env, resume=None, observer=None, bypass=False):
        self.grid = Grid(100, 40)
        self.master, self.slave = os.openpty()
        self.original = termios.tcgetattr(self.slave)
        self.output = bytearray()
        self.dsr_count = 0
        self.observer = observer
        self.resize(100, 40)
        self.started = time.perf_counter()

        def setup():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        args = [str(binary), "--resume", resume] if resume else [str(binary), str(workspace)]
        if bypass:
            args.append("--bypass")
        # Keep a session leader alive after ruvx exits. macOS revokes a PTY when
        # its session leader exits, preventing a post-exit termios inspection.
        self.release_file = workspace.parent / ("pty-release-" + uuid.uuid4().hex)
        wrapper = ('exec 3<&0; "$@" <&3 & child=$!; printf "__RUVX_PID__%s\\n" "$child"; '
                   'wait "$child"; status=$?; printf "\\n__RUVX_CHILD_EXIT__%s\\n" "$status"; '
                   'while [ ! -f "$RUVX_PTY_RELEASE" ]; do sleep 0.02; done; exit "$status"')
        env = dict(env, RUVX_PTY_RELEASE=str(self.release_file))
        self.process = subprocess.Popen(["/bin/sh", "-c", wrapper, "ruvx-pty", *args],
                                        stdin=self.slave, stdout=self.slave,
                                        stderr=self.slave, env=env, preexec_fn=setup)
        self.wait(lambda: "PgUp/PgDn" in self.grid.text(), "initial prompt")
        self.pid = int(re.search(rb"__RUVX_PID__(\d+)", self.output).group(1))
        self.ready_ms = (time.perf_counter() - self.started) * 1000

    def resize(self, width, height):
        self.grid.resize(width, height)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
        if hasattr(self, "pid"):
            os.kill(self.pid, signal.SIGWINCH)

    def pump(self, duration=0.05):
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            readable, _, _ = select.select([self.master], [], [], min(0.02, max(0, deadline - time.monotonic())))
            if not readable:
                continue
            try:
                data = os.read(self.master, 65536)
            except OSError:
                return
            if not data:
                return
            self.output.extend(data)
            self.grid.feed(data)
            if not hasattr(self, "pid"):
                identity = re.search(rb"__RUVX_PID__(\d+)", self.output)
                if identity:
                    self.pid = int(identity.group(1))
            count = self.output.count(b"\x1b[6n")
            for _ in range(count - self.dsr_count):
                # Act as the terminal emulator for Crossterm's cursor-position query.
                os.write(self.master, b"\x1b[1;1R")
            self.dsr_count = count
            if self.observer and hasattr(self, "pid"):
                self.observer(self.pid)

    def wait(self, predicate, description, timeout=8):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if predicate():
                return
            if self.process.poll() is not None or b"__RUVX_CHILD_EXIT__" in self.output:
                self.pump()
                raise AssertionError(f"Process exited waiting for {description}: {self.output[-2500:]!r}")
            self.pump()
        raise AssertionError(f"Timed out waiting for {description}: {self.output[-2500:]!r}")

    def send(self, value):
        data = value.encode() if isinstance(value, str) else value
        while data:
            size = os.write(self.master, data)
            data = data[size:]

    def mouse(self, code, x, y, release=False):
        self.send(f"\x1b[<{code};{x + 1};{y + 1}{'m' if release else 'M'}")
        self.pump(0.08)

    def idle(self):
        status = "".join(self.grid.cells[1]).split("git: ", 1)[0].strip()
        return status == "ready" or status.endswith("· ready")

    def activity_row(self):
        for index in range(self.grid.height - 2, self.grid.height // 2 - 1, -1):
            text = "".join(self.grid.cells[index]).strip()
            if text and ("\u2800" <= text[0] <= "\u28ff" or text.startswith("? Waiting for approval")):
                return index, text
        return None, ""

    def copies(self):
        return [base64.b64decode(value).decode() for value in re.findall(rb"\x1b\]52;c;([A-Za-z0-9+/=]+)\x07", self.output)]

    def enter(self, value=""):
        self.send(value)
        self.send(b"\r")
        self.pump(0.12)

    def close(self, terminate=False):
        if b"__RUVX_CHILD_EXIT__" not in self.output:
            if terminate:
                os.kill(self.pid, signal.SIGTERM)
            else:
                self.send(b"\x15")  # Clear the current line; paste previews are atomic.
                self.enter("/quit")
        deadline = time.monotonic() + 8
        while b"__RUVX_CHILD_EXIT__" not in self.output and time.monotonic() < deadline:
            self.pump()
        if b"__RUVX_CHILD_EXIT__" not in self.output:
            self.process.kill()
            raise AssertionError("Terminal did not exit")
        self.pump()
        assert b"__RUVX_CHILD_EXIT__0" in self.output, bytes(self.output[-2000:])
        assert termios.tcgetattr(self.slave) == self.original, "Terminal flags were not restored"
        assert b"\x1b[?2004l" in self.output, "Bracketed paste mode was not disabled"
        assert b"\x1b[?1049h" in self.output, "Application did not enter alternate screen"
        assert b"\x1b[?1049l" in self.output, "Alternate screen was not restored"
        assert self.output.count(b"\x1b[>1u") == 1, "Keyboard enhancement mode was not requested once"
        assert self.output.count(b"\x1b[<1u") == 1, "Keyboard enhancement mode was not restored once"
        assert b"\x1b[?1006h" in self.output, "Mouse capture was not enabled"
        assert b"\x1b[?1006l" in self.output, "Mouse capture was not disabled"
        self.release_file.touch()
        self.process.wait(timeout=3)
        assert self.process.returncode == 0
        os.close(self.master)
        os.close(self.slave)


def events(data_root):
    result = []
    for file in (data_root / "ruvx" / "sessions").glob("*/events.jsonl"):
        try:
            result.extend(json.loads(line) for line in file.read_text().splitlines())
        except (ValueError, FileNotFoundError):
            pass
    return result


def fixture_env(root, port):
    workspace = root / "project"
    workspace.mkdir()
    home = root / "home"
    home.mkdir()
    config, data, cache = (root / name for name in ("config", "data", "cache"))
    (config / "ruvx").mkdir(parents=True)
    (config / "ruvx" / "config.toml").write_text(f'''schema_version = 1
[providers.fixture]
kind = "compatible"
base_url = "http://127.0.0.1:{port}/v1"
[providers.fixture.auth]
method = "none"
[providers.fixture.models.m]
supports_tools = true
context_limit = 16384
output_reserve_tokens = 256
''')
    env = dict(os.environ, HOME=str(home), TERM="xterm-256color", SSH_TTY="fixture-osc52-no-system-clipboard", XDG_CONFIG_HOME=str(config),
               XDG_DATA_HOME=str(data), XDG_CACHE_HOME=str(cache))
    return workspace, config, data, env


def functional(binary):
    server = Provider()
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    steps = []
    with tempfile.TemporaryDirectory(prefix="ruvx-pty-") as directory:
        root = Path(directory)
        workspace, config, data, env = fixture_env(root, server.server_port)
        git = shutil.which("git")
        git_env = {key: value for key, value in env.items() if not key.startswith("GIT_")}
        git_env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT="0")
        if git:
            subprocess.run([git, "-c", "init.templateDir=", "init", "--quiet", "--initial-branch=feature/fixture"], cwd=workspace, env=git_env, check=True, capture_output=True)
        terminal = Terminal(binary, workspace, env)
        other_terminal = None
        try:
            if git:
                terminal.wait(lambda: "git: feature/fixture" in "".join(terminal.grid.cells[1]), "Git branch in status")
                subprocess.run([git, "symbolic-ref", "HEAD", "refs/heads/feature/updated"], cwd=workspace, env=git_env, check=True, capture_output=True)
                terminal.wait(lambda: "git: feature/updated" in "".join(terminal.grid.cells[1]), "automatic branch refresh", timeout=6)
                assert not server.requests
                steps.append("Git branch in status, unborn repository and automatic external branch refresh without inference")
            terminal.send("/")
            terminal.wait(lambda: "Commands ·" in terminal.grid.text(), "slash command menu")
            assert "/help" in terminal.grid.text() and "/model" in terminal.grid.text()
            terminal.send("mo")
            terminal.wait(lambda: "/model" in terminal.grid.text() and "/mode" in terminal.grid.text(), "filtered commands")
            terminal.send("\x1b[B\r")  # /mode needs an argument, so Enter only completes it.
            terminal.pump(0.2)
            assert "Commands ·" not in terminal.grid.text()
            assert any("/mode " in "".join(row) for row in terminal.grid.cells[-3:])
            assert not any(e["kind"] == "mode.changed" for e in events(data))
            terminal.send("\x15/m\t")
            terminal.pump(0.2)
            assert "Commands ·" not in terminal.grid.text()
            assert any("/model " in "".join(row) for row in terminal.grid.cells[-3:])
            terminal.send("\x15/m")
            terminal.wait(lambda: "Commands ·" in terminal.grid.text(), "reopened commands")
            terminal.send("\x1b")
            terminal.wait(lambda: "Commands ·" not in terminal.grid.text(), "dismissed commands")
            assert any("/m" in "".join(row) for row in terminal.grid.cells[-3:])
            terminal.send("o")
            terminal.wait(lambda: "Commands ·" in terminal.grid.text(), "commands reopen after edit")
            terminal.resize(48, 16)
            terminal.pump(0.2)
            if git:
                assert "git:" in "".join(terminal.grid.cells[1]), "Small terminal hid branch metadata"
            assert "/model" in terminal.grid.text() and "/mode" in terminal.grid.text()
            terminal.send("\x15/")
            terminal.wait(lambda: "> /help" in terminal.grid.text(), "help menu choice")
            x, y = terminal.grid.locate("> /help")
            terminal.mouse(0, x, y)
            terminal.mouse(0, x, y, release=True)
            terminal.wait(lambda: "Review · Esc close" in terminal.grid.text(), "clicked help command")
            terminal.send("\x1b")
            terminal.pump(0.2)
            terminal.resize(100, 40)
            terminal.pump(0.2)
            assert not server.requests, "Command navigation started inference"
            steps.append("slash filtering, required argument, Tab completion, Escape/edit, 48x16 and help click without inference")

            # Configure extensions in Plan before any model is selected. No code is run.
            home_skill = Path(env["HOME"]) / ".agents" / "skills" / "a-home"
            home_skill.mkdir(parents=True)
            (home_skill / "SKILL.md").write_text("---\nname: a-home\ndescription: Home skill discovered automatically\n---\nPassive guidance.\n")
            terminal.enter("/skills")
            terminal.wait(lambda: "a-home [user]" in terminal.grid.text(), "automatic HOME/.agents/skills discovery with independent XDG config")
            assert "Home skill discovered automatically" in terminal.grid.text()
            assert not server.requests, "Home skill discovery started inference"
            terminal.send("\x1b")
            terminal.pump(.1)
            steps.append("automatic HOME/.agents/skills discovery independent of XDG configuration and without inference")
            skill = root / "shared skills" / "demo"
            skill.mkdir(parents=True)
            (skill / "SKILL.md").write_text("---\nname: demo\ndescription: PTY skill fixture\n---\nRead only when relevant.\n")
            config_file = config / "ruvx" / "config.toml"

            def extension_config():
                return tomllib.loads(config_file.read_text())

            terminal.enter("/skills add")
            terminal.wait(lambda: "Local skill folder" in terminal.grid.text(), "skill location dialog")
            terminal.enter(str(skill))
            assert extension_config()["skills"]["paths"] == [str(skill.resolve())]
            terminal.enter("/skills")
            terminal.wait(lambda: "Agent Skills" in terminal.grid.text() and "Search:" in terminal.grid.text(), "searchable skill browser")
            terminal.send("PTY")  # Search descriptions as well as skill names, case-insensitively.
            terminal.wait(lambda: "demo [custom]" in terminal.grid.text() and "1 of 2 skills" in terminal.grid.text(), "registered skill search without restart")
            terminal.send("\r")
            terminal.wait(lambda: "Skill details" in terminal.grid.text() and "PTY skill fixture" in terminal.grid.text(), "expanded skill metadata")
            terminal.send("\x1b")
            terminal.pump(.1)
            terminal.send("\t")
            terminal.wait(lambda: "Discovery locations" in terminal.grid.text(), "separate discovery locations")
            terminal.send("\t")
            terminal.wait(lambda: "No warnings." in terminal.grid.text(), "separate discovery warnings")
            terminal.resize(48, 16)
            terminal.pump(.1)
            assert "Warnings" in terminal.grid.text()
            terminal.send("\x1b")
            terminal.pump(.1)
            terminal.resize(100, 40)
            terminal.pump(.1)
            assert not server.requests, "Skill browser started inference"
            steps.append("searchable skills, expanded metadata, locations/warnings tabs and 48x16 layout without inference")
            terminal.enter("/skills disable")
            assert extension_config()["skills"]["enabled"] is False
            terminal.enter("/skills enable")
            terminal.enter("/mcp add")
            for value in ["fixture-tools", "/bin/sh", '["-c","touch MCP_CONFIG_SHOULD_NOT_RUN"]', '["PATH"]', "", "", ""]:
                terminal.enter(value)
            terminal.wait(lambda: "Save configuration" in terminal.grid.text(), "MCP configuration review")
            assert "fixture-tools" not in extension_config().get("mcp_servers", {})
            terminal.enter("save")
            saved = extension_config()["mcp_servers"]["fixture-tools"]
            assert saved["enabled"] and saved["timeout_seconds"] == 60 and saved["max_message_bytes"] == 1048576
            terminal.enter("/mcp edit fixture-tools")
            for value in ["", "[]", "[]", "15", "4096", "no", "save"]:
                terminal.enter(value)
            saved = extension_config()["mcp_servers"]["fixture-tools"]
            assert saved["args"] == [] and saved["inherit_env"] == [] and saved["timeout_seconds"] == 15
            assert saved["max_message_bytes"] == 4096 and saved["enabled"] is False
            terminal.enter("/mcp enable fixture-tools")
            assert extension_config()["mcp_servers"]["fixture-tools"]["enabled"] is True
            terminal.enter("/mcp disable fixture-tools")
            assert extension_config()["mcp_servers"]["fixture-tools"]["enabled"] is False
            terminal.enter("/mcp remove fixture-tools")
            assert "fixture-tools" not in extension_config().get("mcp_servers", {})
            terminal.enter("/mcp add")
            terminal.enter("canceled-setup")
            terminal.send("\x1b")
            terminal.pump(.1)
            assert "canceled-setup" not in extension_config().get("mcp_servers", {})
            terminal.enter("/skills remove")
            terminal.enter("1")
            assert extension_config()["skills"]["paths"] == [] and (skill / "SKILL.md").is_file()
            assert not (workspace / "MCP_CONFIG_SHOULD_NOT_RUN").exists()
            assert not server.requests and not any(e["kind"] == "run.started" for e in events(data))
            steps.append("skill register/list/toggle/unregister and MCP add/review/edit/toggle/remove/cancel persist immediately in Plan without model inference, server startup or deleting source files")

            terminal.send("keep this draft\x1b[Z")
            terminal.wait(lambda: "[Build]" in terminal.grid.text(), "Shift+Tab changes Plan to Build")
            assert " · Build · " in "".join(terminal.grid.cells[-3]), "Composer lost the active mode"
            assert any("keep this draft" in "".join(row) for row in terminal.grid.cells[-3:])
            terminal.send("\x1b[9;2u")
            terminal.wait(lambda: "[Plan]" in terminal.grid.text(), "CSI-u Shift+Tab changes Build to Plan")
            assert not server.requests
            terminal.send("\x15")
            terminal.pump(0.1)
            steps.append("Shift+Tab switches Plan/Build without sending or losing the draft")
            terminal.send("hello world\x1b\x7f")  # Legacy Option+Delete.
            terminal.pump(.15)
            assert "".join(terminal.grid.cells[-2]).rstrip() == "> hello"
            terminal.send("world\x1b[127;3u")  # CSI-u Option+Delete.
            terminal.pump(.15)
            assert "".join(terminal.grid.cells[-2]).rstrip() == "> hello"
            terminal.send("world\x17")  # Ctrl+W alias.
            terminal.pump(.15)
            assert "".join(terminal.grid.cells[-2]).rstrip() == "> hello"
            terminal.send("\x1b[127;9u")  # Command+Delete is intentionally ignored.
            terminal.pump(.15)
            assert "".join(terminal.grid.cells[-2]).rstrip() == "> hello"
            terminal.send("\x15")  # Only Ctrl+U deletes to the line start.
            terminal.pump(.15)
            assert "".join(terminal.grid.cells[-2]).rstrip() == ">"
            terminal.send("\x1b[200~first\nsecond word\x1b[201~\x1b[117;5u")
            terminal.pump(.15)
            assert any("first" in "".join(row) for row in terminal.grid.cells[-4:-1])
            assert not any("second word" in "".join(row) for row in terminal.grid.cells[-4:-1])
            terminal.send("new\x15\x17")  # Clear this line, then the preceding word/newline.
            terminal.pump(.15)
            assert "".join(terminal.grid.cells[-2]).rstrip() == ">"
            assert not server.requests
            steps.append("legacy/CSI-u Option+Delete, ignored Command+Delete, Ctrl+W/U and multiline prefix preservation without submission")
            # Exercise /connect and its hidden-input path without any real key/account.
            for value in ["/connect", "3", "saved", f"http://127.0.0.1:{server.server_port}/v1", "1"]:
                terminal.enter(value)
            secret = "fixture-secret-never-echo"
            terminal.send(secret)
            terminal.pump(0.2)
            assert secret.encode() not in terminal.output
            terminal.enter()
            for value in ["m", "yes", ""]:
                terminal.enter(value)
            terminal.wait(lambda: any(e["kind"] == "model.selected" for e in events(data)), "saved-key model selection")
            assert secret not in (config / "ruvx" / "config.toml").read_text()
            for file in (data / "ruvx" / "sessions").glob("*/events.jsonl"):
                assert secret not in file.read_text()
            credentials = list((config / "ruvx" / "auth").glob("*.json"))
            assert credentials and all(file.stat().st_mode & 0o777 == 0o600 for file in credentials)
            steps.append("connect wizard, hidden API key, private storage")

            terminal.enter("/model fixture m")
            terminal.enter("/effort levels low medium high")
            terminal.wait(lambda: "Reasoning effort" in terminal.grid.text(), "effort picker")
            x, y = terminal.grid.locate("high ·")
            terminal.mouse(0, x, y)
            terminal.mouse(0, x, y, release=True)
            terminal.wait(lambda: "[Effort: high]" in terminal.grid.text(), "selected model effort")
            assert not server.requests, "Changing effort started inference"
            settings = (config / "ruvx" / "config.toml").read_text()
            assert 'reasoning_effort = "high"' in settings
            server.replies.put((completion("PASTE_OK"), 0))
            terminal.send("\x1b[200~first line\nemoji 中🙂\x1b[201~")
            terminal.pump(0.2)
            assert not server.requests, "Paste submitted without Enter"
            terminal.resize(74, 28)
            terminal.pump()
            terminal.enter()
            terminal.wait(lambda: "PASTE_OK" in terminal.grid.text(), "multiline paste response")
            terminal.wait(terminal.idle, "agent returns session and becomes ready")
            for heading, body in [("USER", "first line"), ("AI · ruvx", "PASTE_OK")]:
                _, row = terminal.grid.locate(heading)
                assert not "".join(terminal.grid.cells[row + 1]).strip(), "Missing space below role label"
                assert body in "".join(terminal.grid.cells[row + 2])
            assert server.requests[0]["messages"][-1]["content"] == "first line\nemoji 中🙂"
            assert server.requests[0]["reasoning_effort"] == "high"
            terminal.wait(lambda: any(e["kind"] == "run.stopped" for e in events(data)), "completed paste run")
            steps.append("per-model effort capability setup, mouse selection, persistence and wire parameter")
            steps.append("multiline paste, Unicode, resize, single submission")

            terminal.wait(lambda: "PASTE_OK" in terminal.grid.text(), "visible streamed response")
            x, y = terminal.grid.locate("PASTE_OK")
            terminal.mouse(0, x, y)
            terminal.mouse(32, x + 4, y)
            terminal.mouse(0, x + 4, y, release=True)
            terminal.wait(lambda: terminal.copies() == ["PASTE"], "drag auto-copy via OSC52")
            terminal.send(b"\x03")
            terminal.wait(lambda: len(terminal.copies()) == 2, "Ctrl+C copies and clears selection")
            assert terminal.copies() == ["PASTE", "PASTE"]
            terminal.enter("/help")
            terminal.pump()
            original = terminal.grid.text()
            terminal.mouse(65, 10, 10)
            terminal.wait(lambda: terminal.grid.text() != original, "wheel scrolls review")
            terminal.send(b"\x1b[6~")
            terminal.pump()
            terminal.send(b"\x1b")
            terminal.pump()
            steps.append("fullscreen mouse drag auto-copy, Ctrl+C selection priority, wheel and keyboard scroll")

            # A large bracketed paste is a document node; editing does not submit it.
            payload = "".join(f"line {i} 中🙂\n" for i in range(24))
            before = len(server.requests)
            terminal.send("Before: ")
            terminal.send("\x1b[200~" + payload + "\x1b[201~")
            terminal.send(" after")
            terminal.wait(lambda: "[Paste 1" in terminal.grid.text(), "folded paste preview")
            assert len(server.requests) == before
            x, y = terminal.grid.locate("[Paste 1")
            terminal.mouse(0, x, y); terminal.mouse(0, x, y, release=True)
            terminal.wait(lambda: "Edit Paste 1" in terminal.grid.text(), "paste editor")
            terminal.send("temporary word\x1b\x7f\x15Edited: ")
            terminal.send(b"\x13")  # Ctrl+S saves without sending.
            terminal.wait(lambda: "Edit Paste" not in terminal.grid.text(), "saved paste")
            assert len(server.requests) == before
            server.replies.put((completion("LONG_PASTE_OK", usage={"prompt_tokens":50, "completion_tokens":3, "total_tokens":53, "attribution":{"internal":"hidden"}}), 0))
            terminal.enter()
            terminal.wait(lambda: "LONG_PASTE_OK" in terminal.grid.text(), "complete pasted message")
            terminal.wait(terminal.idle, "agent returns session and becomes ready")
            assert server.requests[-1]["messages"][-1]["content"] == "Before: Edited: " + payload + " after"
            assert "attribution" not in terminal.grid.text()
            terminal.wait(lambda: "Open message" in terminal.grid.text(), "folded user message")
            x, y = terminal.grid.locate("Open message")
            terminal.mouse(0, x, y); terminal.mouse(0, x, y, release=True)
            terminal.wait(lambda: "line 10" in terminal.grid.text(), "expanded user source")
            x, y = terminal.grid.locate("[Latest")
            terminal.mouse(0, x, y); terminal.mouse(0, x, y, release=True)
            terminal.wait(lambda: "LONG_PASTE_OK" in terminal.grid.text(), "jump to latest response")
            terminal.wait(lambda: "Tokens · input 50" in terminal.grid.text(), "compact token usage")
            assert "AI · ruvx" in terminal.grid.text()
            literal = "/quit\n" + "literal paste\n" * 16
            server.replies.put((completion("SLASH_LITERAL_OK"), 0))
            terminal.send("\x1b[200~" + literal + "\x1b[201~")
            terminal.pump(.15); terminal.enter()
            terminal.wait(lambda: "SLASH_LITERAL_OK" in terminal.grid.text(), "literal slash in large paste")
            terminal.wait(terminal.idle, "agent returns session and becomes ready")
            assert server.requests[-1]["messages"][-1]["content"] == literal
            before_error = len(server.requests)
            failure = {"error": {"type": "usage_limit_reached", "message": "PROVIDER_SECRET_BODY"}}
            server.replies.put(("data: " + json.dumps(failure) + "\n\n", 0))
            terminal.enter("Test provider diagnostics")
            terminal.wait(lambda: "usage_limit_reached" in terminal.grid.text(), "structured provider error identifier")
            terminal.wait(terminal.idle, "provider failure stops the activity indicator")
            assert "account usage" in terminal.grid.text()
            assert b"PROVIDER_SECRET_BODY" not in terminal.output
            assert len(server.requests) == before_error + 1, "Provider failure retried automatically"
            steps.append("structured provider error shows safe identifier and account usage guidance, stops activity and does not retry")
            before_retry = len(server.requests)
            overload = {"error": {"code": "server_is_overloaded", "type": "service_unavailable_error"}}
            server.replies.put(("data: " + json.dumps(overload) + "\n\n", 0))
            server.replies.put((completion("AUTOMATIC_RETRY_OK"), 0))
            terminal.enter("Recover from provider overload")
            terminal.wait(lambda: "automatic retry 1/3 in 2s" in terminal.grid.text(), "visible provider retry wait")
            assert "AI Working" in terminal.activity_row()[1]
            terminal.wait(lambda: "AUTOMATIC_RETRY_OK" in terminal.grid.text(), "automatic recovery from provider overload")
            terminal.wait(terminal.idle, "recovered provider run completes")
            assert len(server.requests) == before_retry + 2
            assert server.requests[-1] == server.requests[-2], "Retry changed the request"
            steps.append("provider overload retries automatically with visible delay, active bottom spinner and identical request")
            steps.append("large paste preview/edit/save, exact full payload, user-only fold/expand, literal slash and compact usage")

            terminal.enter("/mode build")
            server.replies.put((completion(calls=[("denied", "run_shell", {"command": "printf marker > denied.txt"})]), 0))
            server.replies.put((completion("DENIED_OK"), 0))
            mark = len(terminal.output)
            terminal.enter("test denial")
            terminal.wait(lambda: "Shell approval" in terminal.grid.text(), "shell review")
            assert "Waiting for approval" in terminal.activity_row()[1]
            assert "Waiting for approval" not in "".join(terminal.grid.cells[1])
            terminal.wait(lambda: "Run once" in terminal.grid.text(), "approval buttons")
            terminal.send("\x1b[Z")
            terminal.pump(0.1)
            assert "[Build]" in terminal.grid.text(), "Shift+Tab changed mode during shell approval"
            assert not (workspace / "denied.txt").exists()
            x, y = terminal.grid.locate("Run once")
            terminal.mouse(0, x, y)
            terminal.mouse(32, x + 2, y)
            terminal.mouse(0, x + 2, y, release=True)
            assert not (workspace / "denied.txt").exists(), "Drag activated shell approval"
            terminal.send(b"\x1b")
            terminal.wait(lambda: "DENIED_OK" in terminal.grid.text(), "denial result")
            terminal.wait(terminal.idle, "denied run returns session")
            assert terminal.grid.text().count("run_shell → denied") == 1
            assert "run_shell · denied" not in terminal.grid.text()
            x, y = terminal.grid.locate("run_shell → denied")
            terminal.mouse(0, x, y)
            terminal.mouse(0, x, y, release=True)
            terminal.wait(lambda: "Call ID: denied" in terminal.grid.text(), "tool ID in detail only")
            assert "Arguments:" in terminal.grid.text()
            terminal.send("\x1b")
            terminal.pump(.1)
            steps.append("one compact tool result without duplicate call-ID line; full ID and arguments in Detail")
            terminal.wait(terminal.idle, "agent returns session and becomes ready")
            assert not (workspace / "denied.txt").exists()
            steps.append("shell denied without process side effect")

            server.replies.put((completion(calls=[("approved", "run_shell", {"command": "sleep 1; printf marker > result.txt"})], usage={"prompt_tokens":10,"completion_tokens":5}), 0))
            server.replies.put((completion("BUILD_OK", usage={"total_tokens":22}), .6))
            mark = len(terminal.output)
            terminal.enter("test approval")
            terminal.wait(lambda: "Shell approval" in terminal.grid.text(), "second shell review")
            terminal.enter("run")
            terminal.wait(lambda: "AI Working" in terminal.activity_row()[1], "bottom activity while a silent shell runs")
            terminal.wait(lambda: "1 turn, 15 total tokens" in terminal.activity_row()[1], "reported token total during tool execution")
            terminal.wait(lambda: "2 turns, 15 total tokens" in terminal.activity_row()[1], "cumulative counters during next provider turn")
            terminal.wait(lambda: "BUILD_OK" in terminal.grid.text(), "approved command result")
            terminal.wait(terminal.idle, "agent returns session and becomes ready")
            assert terminal.activity_row()[0] is None
            assert (workspace / "result.txt").read_text() == "marker"
            terminal.enter("/diff")
            terminal.pump()
            terminal.send(b"\x1b")  # Dismiss a possible review pager.
            terminal.pump()
            steps.append("approved shell, recorded change, diff review")

            before = len(server.requests)
            server.replies.put((completion("SHOULD_NOT_FINISH"), 1.5))
            terminal.enter("cancel this request")
            terminal.wait(lambda: len(server.requests) > before, "active network request")
            terminal.wait(lambda: "AI Working" in terminal.activity_row()[1], "bottom activity before provider response")
            terminal.wait(lambda: re.search(r"AI Working, \d+s, 1 turn, — total tokens", terminal.activity_row()[1]), "compact run counters in bottom activity indicator")
            assert "Waiting for AI response" not in terminal.activity_row()[1]
            assert "AI Working" not in "".join(terminal.grid.cells[1])
            assert terminal.activity_row()[0] < terminal.grid.height - 2
            first_frame = terminal.activity_row()[1][0]
            terminal.pump(.3)
            assert terminal.activity_row()[1][0] != first_frame, "Activity indicator froze without provider events"
            assert "Ctrl+C cancel" in terminal.grid.text()
            terminal.send("\x1b[Z")
            terminal.pump(0.1)
            assert "[Build]" in terminal.grid.text(), "Shift+Tab changed mode during active work"
            terminal.send("\x1b[99;5u")  # Enhanced Ctrl+C still cancels the active request.
            terminal.wait(lambda: any(e["kind"] == "run.stopped" and e["payload"].get("reason") == "canceled" for e in events(data)), "durable cancellation")
            terminal.wait(terminal.idle, "activity stops after cancellation")
            assert terminal.activity_row()[0] is None
            steps.append("bottom activity above input animates during quiet provider/tool work; approval waits and cancellation are distinct; completion/cancellation stop the indicator")
            session_id = next(e["session_id"] for e in events(data) if e["kind"] == "session.created")
            terminal.close()
            steps.append("normal exit restores termios, bracketed paste, mouse capture and alternate screen")

            count = len(server.requests)
            preference_file = data / "ruvx" / "last-model.json"
            assert json.loads(preference_file.read_text()) == {"profile": "fixture", "model": "m"}
            terminal = Terminal(binary, workspace, env)
            terminal.wait(lambda: "[fixture/m]" in terminal.grid.text() and "[Effort: high]" in terminal.grid.text(), "global model on restart")
            assert "[Plan]" in terminal.grid.text(), "New session inherited Build mode"
            other_workspace = root / "other-project"
            other_workspace.mkdir()
            other_terminal = Terminal(binary, other_workspace, env)
            other_terminal.wait(lambda: "[fixture/m]" in other_terminal.grid.text() and "[Effort: high]" in other_terminal.grid.text(), "global model in another project/window")
            other_terminal.enter("/model saved m")
            other_terminal.wait(lambda: "[saved/m]" in other_terminal.grid.text(), "new global choice in another window")
            other_terminal.close()
            other_terminal = None
            assert "[fixture/m]" in terminal.grid.text(), "Another window changed the running session's model"
            terminal.enter("/new")
            terminal.wait(lambda: "[saved/m]" in terminal.grid.text() and "[Effort: default]" in terminal.grid.text(), "latest global model on /new")
            assert "[Plan]" in terminal.grid.text()
            assert len(server.requests) == count, "Restoring global preferences started inference"
            terminal.close()
            steps.append("global model and effort on restart and concurrent other-project window; /new reloads latest default without inference")

            terminal = Terminal(binary, workspace, env, resume=session_id)
            terminal.pump(0.2)
            assert "run_shell → ok" in terminal.grid.text(), "Resume lost compact tool status"
            assert "run_shell · approved" not in terminal.grid.text()
            assert "[fixture/m]" in terminal.grid.text(), "Global default replaced the resumed session's model"
            assert "[Effort: high]" in terminal.grid.text(), "Resume lost model effort"
            assert len(server.requests) == count, "Resume replayed a network request"
            assert json.loads(preference_file.read_text()) == {"profile": "saved", "model": "m"}, "Resume overwrote the global default"
            terminal.close(terminate=True)
            steps.append("resume keeps session model without replay or overwriting global default; SIGTERM cleanup")
            # Legacy storage stays untouched; ruvx starts with its own fresh state.
            cache = Path(env["XDG_CACHE_HOME"])
            (cache / "ruvx").mkdir(parents=True)
            (cache / "ruvx" / "inventory.json").write_text('{"legacy": true}\n')
            legacy_roots = []
            for base in [config, data, cache]:
                legacy = base / "kovr"
                (base / "ruvx").rename(legacy)
                legacy_roots.append(legacy)

            def legacy_snapshot():
                return {path: (path.read_bytes() if path.is_file() else None,
                               path.stat().st_mode & 0o777)
                        for legacy in legacy_roots
                        for path in [legacy, *legacy.rglob("*")]}

            legacy_before = legacy_snapshot()
            terminal = Terminal(binary, workspace, env)
            terminal.wait(lambda: "[/connect]" in terminal.grid.text(), "fresh ruvx onboarding with legacy storage present")
            assert "ruvx ·" in terminal.grid.text()
            assert "[saved/m]" not in terminal.grid.text() and "[fixture/m]" not in terminal.grid.text()
            assert not (config / "ruvx").exists(), "Legacy config or credentials were copied"
            assert not (cache / "ruvx").exists(), "Legacy cache was copied"
            assert not preference_file.exists(), "Legacy global model was copied"
            fresh_events = events(data)
            assert sum(e["kind"] == "session.created" for e in fresh_events) == 1
            assert not any(e["kind"] == "model.selected" for e in fresh_events)
            assert len(server.requests) == count, "Legacy storage triggered inference"
            terminal.close()
            assert legacy_snapshot() == legacy_before, "Legacy storage contents or permissions changed"
            steps.append("fresh ruvx onboarding and session storage without importing legacy config, credentials, model or cache; legacy contents and permissions unchanged without inference")
        finally:
            for current_terminal in [terminal, other_terminal]:
                if current_terminal is None or current_terminal.process.poll() is not None:
                    continue
                if hasattr(current_terminal, "pid"):
                    try:
                        os.kill(current_terminal.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                current_terminal.process.kill()
                current_terminal.process.wait()
    server.shutdown()
    server.server_close()
    return steps


def git_push_workflow(binary):
    """Real commit/push to a disposable local bare remote; no external remote is used."""
    git = shutil.which("git")
    if git is None:
        return {"skipped": "git is not installed"}
    server = Provider()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    terminal = None
    try:
        with tempfile.TemporaryDirectory(prefix="ruvx-git-pty-") as directory:
            root = Path(directory)
            workspace, config, data, env = fixture_env(root, server.server_port)
            env = {key: value for key, value in env.items() if not key.startswith("GIT_")}
            env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT="0")
            remote = root / "local-remote.git"
            for args in [
                [git, "-c", "init.templateDir=", "init", "--quiet", "--initial-branch=main"],
                [git, "-c", "init.templateDir=", "init", "--quiet", "--bare", str(remote)],
                [git, "remote", "add", "origin", str(remote)],
            ]:
                subprocess.run(args, cwd=workspace, env=env, check=True, capture_output=True)
            (workspace / "file.txt").write_text("fixture content\n")
            hook = workspace / ".git" / "hooks" / "pre-push"
            hook.parent.mkdir(exist_ok=True)
            hook.write_text("#!/bin/sh\nprintf started > .git/push-started\nsleep 1\n")
            hook.chmod(0o700)
            terminal = Terminal(binary, workspace, env)
            terminal.enter("/model fixture m")
            terminal.enter("/mode build")
            commit = "git add file.txt && git -c user.name=Fixture -c user.email=fixture@example.invalid -c commit.gpgsign=false commit -m 'fixture commit'"
            server.replies.put((completion(calls=[("git-commit", "run_shell", {"command": commit})]), 0))
            server.replies.put((completion(calls=[("git-push", "run_shell", {"command": "git push origin main"})]), 0))
            server.replies.put((completion("LOCAL_PUSH_OK"), 0))
            terminal.enter("Commit and push to the local test remote")
            terminal.wait(lambda: "Shell approval" in terminal.grid.text(), "commit review")
            x, y = terminal.grid.locate("Run once")
            # Both clicks arrive in the same input burst, before a new frame.
            click = f"\x1b[<0;{x + 1};{y + 1}M\x1b[<0;{x + 1};{y + 1}m"
            terminal.send(click * 2)
            terminal.wait(lambda: "Shell approval" in terminal.grid.text() and "git push origin main" in terminal.grid.text(), "push review after repeated commit approval click")
            terminal.enter("run")
            terminal.wait(lambda: (workspace / ".git" / "push-started").exists(), "push starts and enters pre-push hook")
            terminal.enter("follow-up draft")
            terminal.wait(lambda: "AI is still working" in terminal.grid.text(), "non-error feedback while push continues")
            terminal.wait(lambda: "LOCAL_PUSH_OK" in terminal.grid.text(), "push completes despite Enter during active work")
            terminal.wait(terminal.idle, "Git workflow returns to ready")
            assert b"Error: Work is active" not in terminal.output
            assert b"Run canceled; completed changes" not in terminal.output
            assert any("follow-up draft" in "".join(row) for row in terminal.grid.cells[-3:])
            assert len(server.requests) == 3, "Draft was submitted or a Git action was repeated"
            local_head = subprocess.check_output([git, "rev-parse", "HEAD"], cwd=workspace, env=env).strip()
            remote_head = subprocess.check_output([git, "--git-dir", str(remote), "rev-parse", "refs/heads/main"], env=env).strip()
            assert local_head == remote_head, "Local commit did not reach the disposable remote"
            assert subprocess.check_output([git, "rev-list", "--count", "HEAD"], cwd=workspace, env=env).strip() == b"1"
            approvals = [e for e in events(data) if e["kind"] == "approval.resolved"]
            assert len(approvals) == 2 and all(e["payload"]["approved"] for e in approvals)
            terminal.close()
            return {"local_commit_and_push": "passed", "repeated_approval_click": "ignored", "enter_during_push": "draft preserved; push continued", "external_remotes_used": False}
    finally:
        if terminal is not None and terminal.process.poll() is None:
            try:
                os.kill(terminal.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            terminal.process.kill()
            terminal.process.wait()
        server.shutdown()
        server.server_close()


def git_auth_terminal_isolation(binary):
    """An HTTP 401 must become a tool error rather than a prompt on the TUI's tty."""
    if shutil.which("git") is None:
        return {"skipped": "git is not installed"}

    class AuthRequired(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            self.send_response(401)
            self.send_header("WWW-Authenticate", 'Basic realm="local fixture"')
            self.send_header("Content-Length", "0")
            self.end_headers()

    provider = Provider()
    auth_server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), AuthRequired)
    for server in [provider, auth_server]:
        threading.Thread(target=server.serve_forever, daemon=True).start()
    terminal = None
    try:
        with tempfile.TemporaryDirectory(prefix="ruvx-auth-pty-") as directory:
            root = Path(directory)
            workspace, _, data, env = fixture_env(root, provider.server_port)
            env = {key: value for key, value in env.items() if not key.startswith("GIT_") and key != "SSH_ASKPASS"}
            # Prove ruvx sets its own non-interactive policy rather than relying on the parent.
            env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT="1", LC_ALL="C")
            terminal = Terminal(binary, workspace, env)
            terminal.enter("/model fixture m")
            terminal.enter("/mode build")
            command = f"git -c credential.helper= -c core.askPass= ls-remote http://127.0.0.1:{auth_server.server_port}/private.git"
            provider.replies.put((completion(calls=[("auth-required", "run_shell", {"command": command, "timeout_seconds": 2})]), 0))
            provider.replies.put((completion("AUTH_REQUIRED_HANDLED"), 0))
            terminal.enter("Check the local authentication fixture")
            terminal.wait(lambda: "Shell approval" in terminal.grid.text(), "authentication operation approval")
            marker = len(terminal.output)
            terminal.enter("run")
            terminal.wait(lambda: "AUTH_REQUIRED_HANDLED" in terminal.grid.text(), "authentication fails without an interactive terminal prompt")
            terminal.wait(terminal.idle, "ready after authentication failure")
            assert b"Username for 'http://" not in terminal.output[marker:], "Git wrote its credential prompt directly onto the application terminal"
            results = [e for e in events(data) if e["kind"] == "tool.finished" and e["payload"].get("call_id") == "auth-required"]
            assert len(results) == 1
            event = results[0]
            session = data / "ruvx" / "sessions" / event["session_id"]
            result = json.loads((session / "blobs" / event["payload"]["blob"]["id"]).read_text())
            assert result["status"] == "failed", result["status"]
            assert "terminal prompts disabled" in result["stderr"]
            assert "separate terminal" in result["auth_hint"]
            # Even a command that explicitly opens /dev/tty cannot borrow the TUI.
            provider.replies.put((completion(calls=[("tty-check", "run_shell", {"command": "if (printf UNEXPECTED_TTY_ACCESS > /dev/tty) 2>/dev/null; then printf borrowed; else printf detached; fi"})]), 0))
            provider.replies.put((completion("TTY_ISOLATION_HANDLED"), 0))
            terminal.enter("Check terminal isolation")
            terminal.wait(lambda: "Shell approval" in terminal.grid.text(), "terminal isolation approval")
            marker = len(terminal.output)
            terminal.enter("run")
            terminal.wait(lambda: "TTY_ISOLATION_HANDLED" in terminal.grid.text(), "terminal isolation result")
            terminal.wait(terminal.idle, "ready after terminal isolation check")
            assert b"UNEXPECTED_TTY_ACCESS" not in terminal.output[marker:]
            assert any("detached" in json.dumps(message) for message in provider.requests[-1]["messages"] if message.get("role") == "tool")
            terminal.close()
            return {"http_401": "captured failure with authentication hint", "controlling_terminal": "detached", "external_services_used": False}
    finally:
        if terminal is not None and terminal.process.poll() is None:
            try:
                os.kill(terminal.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            terminal.process.kill()
            terminal.process.wait()
        for server in [provider, auth_server]:
            server.shutdown()
            server.server_close()


def bypass_workflow(binary):
    server = Provider()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    terminal = None
    try:
        with tempfile.TemporaryDirectory(prefix="ruvx-bypass-pty-") as directory:
            root = Path(directory)
            workspace, config, data, env = fixture_env(root, server.server_port)
            fixture = (Path(__file__).resolve().parent.parent / "tests/fixtures/mcp-stdio.sh").read_text()
            config_file = config / "ruvx" / "config.toml"
            config_file.write_text(config_file.read_text() + '\n[mcp_servers.fixture]\nenabled = true\ncommand = "/bin/sh"\nargs = ["-c", ' + json.dumps(fixture) + ']\n')
            terminal = Terminal(binary, workspace, env)
            assert "BYPASS" not in "".join(terminal.grid.cells[0])
            terminal.enter("/model fixture m")
            terminal.enter("/bypass on")
            terminal.wait(lambda: "BYPASS" in "".join(terminal.grid.cells[0]) and "[Build]" in terminal.grid.text(), "TUI enables BYPASS and Build")
            for call in [
                ("list", "mcp_list_tools", {"server":"fixture"}),
                ("next", "mcp_list_tools", {"server":"fixture", "cursor":"page2"}),
                ("echo", "mcp_call_tool", {"server":"fixture", "tool":"echo", "arguments":{"text":"hello"}}),
                ("shell", "run_shell", {"command":"printf bypass > bypass.txt"}),
            ]:
                server.replies.put((completion(calls=[call]), 0))
            server.replies.put((completion("BYPASS_TUI_DONE"), 0))
            terminal.enter("Use MCP and shell with BYPASS")
            terminal.wait(lambda: "BYPASS_TUI_DONE" in terminal.grid.text(), "shell and MCP finish without approval")
            terminal.wait(terminal.idle, "BYPASS run ready")
            assert (workspace / "bypass.txt").read_text() == "bypass"
            approvals = [e for e in events(data) if e["kind"] == "approval.resolved"]
            assert len(approvals) == 4 and all(e["payload"]["policy"] == "bypass" for e in approvals)
            terminal.enter("/new")
            terminal.wait(lambda: "BYPASS" in "".join(terminal.grid.cells[0]) and "[Build]" in terminal.grid.text(), "BYPASS survives new session in the same process")
            terminal.enter("/bypass off")
            terminal.wait(lambda: "BYPASS" not in "".join(terminal.grid.cells[0]), "BYPASS disabled")
            assert "bypass" not in tomllib.loads(config_file.read_text())
            server.replies.put((completion(calls=[("ask", "run_shell", {"command":"touch must-not-exist"})]), 0))
            server.replies.put((completion("ASK_RESTORED"), 0))
            terminal.enter("Use normal approval again")
            terminal.wait(lambda: "Shell approval" in terminal.grid.text(), "approval restored after BYPASS off")
            terminal.enter("deny")
            terminal.wait(lambda: "ASK_RESTORED" in terminal.grid.text(), "denial handled")
            terminal.wait(terminal.idle, "ready after denial")
            assert not (workspace / "must-not-exist").exists()
            terminal.close()
            terminal = Terminal(binary, workspace, env, bypass=True)
            terminal.wait(lambda: "BYPASS" in "".join(terminal.grid.cells[0]) and "[Build]" in terminal.grid.text(), "CLI --bypass starts in Build")
            server.replies.put((completion(calls=[("cli", "run_shell", {"command":"printf cli > cli-bypass.txt"})]), 0))
            server.replies.put((completion("BYPASS_CLI_DONE"), 0))
            terminal.enter("Run with CLI BYPASS")
            terminal.wait(lambda: "BYPASS_CLI_DONE" in terminal.grid.text(), "CLI BYPASS shell without approval")
            terminal.wait(terminal.idle, "CLI BYPASS ready")
            assert (workspace / "cli-bypass.txt").read_text() == "cli"
            session_id = [e["session_id"] for e in events(data) if e["kind"] == "session.created"][-1]
            terminal.close()
            request_count = len(server.requests)
            terminal = Terminal(binary, workspace, env, resume=session_id)
            assert "BYPASS" not in "".join(terminal.grid.cells[0]), "BYPASS persisted after restarting without the flag"
            assert len(server.requests) == request_count, "Resume replayed BYPASS operations"
            terminal.close()
            return {"cli_and_tui": "passed", "shell_and_mcp_without_approval": "passed", "off_restores_approval": "passed", "restart_defaults_to_ask": "passed"}
    finally:
        if terminal is not None and terminal.process.poll() is None:
            try:
                os.kill(terminal.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            terminal.process.kill()
            terminal.process.wait()
        server.shutdown()
        server.server_close()


def benchmark(binary):
    samples = []
    idle_rss = []
    with tempfile.TemporaryDirectory(prefix="ruvx-perf-") as directory:
        root = Path(directory)
        workspace, _, _, env = fixture_env(root, 1)
        for index in range(20):
            terminal = Terminal(binary, workspace, env)
            samples.append(terminal.ready_ms)
            if index == 0:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    value = subprocess.check_output(["ps", "-o", "rss=", "-p", str(terminal.pid)]).strip()
                    idle_rss.append(int(value) * 1024)
                    terminal.pump(0.25)
            terminal.close()
    p95 = sorted(samples)[math.ceil(len(samples) * 0.95) - 1]
    result = {"samples": len(samples), "startup_ms": samples,
              "startup_p95_ms": round(p95, 3), "startup_median_ms": round(statistics.median(samples), 3),
              "idle_observation_seconds": 10, "idle_rss_max_bytes": max(idle_rss),
              "binary_bytes": binary.stat().st_size,
              "conditions": "warm-cache, isolated small config/new session, PTY 100x40, no discovery/inference/child shell"}
    assert p95 <= 500, result
    assert max(idle_rss) <= 100_000_000, result
    assert binary.stat().st_size <= 40_000_000, result
    return result


def load_profile(binary):
    """Observe parent RSS for streaming, repo tools, snapshots, and larger resume."""
    server = Provider()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    phase = "file_tools_and_streaming"
    observed = {phase: 0, "snapshot_and_shell": 0, "larger_resume": 0}
    last_sample = 0

    def sample(pid):
        nonlocal last_sample
        if time.monotonic() - last_sample < 0.05:
            return
        last_sample = time.monotonic()
        try:
            value = subprocess.check_output(["ps", "-o", "rss=", "-p", str(pid)],
                                            stderr=subprocess.DEVNULL).strip()
            observed[phase] = max(observed[phase], int(value) * 1024)
        except (subprocess.CalledProcessError, ValueError):
            pass

    with tempfile.TemporaryDirectory(prefix="ruvx-load-") as directory:
        root = Path(directory)
        workspace, config, data, env = fixture_env(root, server.server_port)
        config_file = config / "ruvx" / "config.toml"
        config_file.write_text(config_file.read_text().replace("16384", "262144"))
        (workspace / "src").mkdir()
        for index in range(256):
            text = (f"file {index:04d} needle\n" * 300)[:4096]
            (workspace / "src" / f"file{index:04d}.txt").write_text(text)
        terminal = Terminal(binary, workspace, env, observer=sample)
        try:
            terminal.enter("/model fixture m")
            stream_text = ("streamed text for load observation " * 1000)[:32768] + "\nLOAD_STREAM_DONE"
            server.replies.put((completion(calls=[
                ("load-list", "list_files", {}),
                ("load-search", "search_text", {"query": "needle", "glob": "src/*.txt"}),
                ("load-read", "read_file", {"path": "src/file0000.txt"}),
            ]), 0))
            server.replies.put((completion(stream_text), 0))
            terminal.enter("Inspect the workload")
            terminal.wait(lambda: "LOAD_STREAM_DONE" in terminal.grid.text(),
                          "file tools and streaming workload", timeout=30)
            terminal.wait(terminal.idle, "agent returns session and becomes ready")
            phase = "snapshot_and_shell"
            terminal.enter("/mode build")
            server.replies.put((completion(calls=[
                ("load-shell", "run_shell", {"command": "printf changed > src/file0000.txt"})
            ]), 0))
            server.replies.put((completion("LOAD_BUILD_DONE"), 0))
            marker = len(terminal.output)
            terminal.enter("Make the workload change")
            terminal.wait(lambda: "Shell approval" in terminal.grid.text(),
                          "workspace snapshot", timeout=60)
            terminal.enter("run")
            terminal.wait(lambda: "LOAD_BUILD_DONE" in terminal.grid.text(),
                          "snapshot comparison", timeout=60)
            terminal.wait(terminal.idle, "agent returns session and becomes ready")
            session_id = next(e["session_id"] for e in events(data)
                              if e["kind"] == "session.created")
            terminal.close()

            # Seed complete, synthetic historical conversations after the writer
            # has closed. This tests the same versioned journal/blob reader used
            # for real sessions, without making hundreds of model requests.
            session_dir = data / "ruvx" / "sessions" / session_id
            journal = session_dir / "events.jsonl"
            sequence = json.loads(journal.read_text().splitlines()[-1])["sequence"]
            with journal.open("a") as writer:
                for index in range(256):
                    for kind, content in [
                        ("message.user", f"Historical request {index}: " + "context " * 32),
                        ("response.completed", json.dumps({"text": f"Historical answer {index}",
                                                          "calls": [], "opaque": [],
                                                          "route": None, "usage": None})),
                    ]:
                        raw = content.encode()
                        digest = hashlib.sha256(raw).hexdigest()
                        (session_dir / "blobs" / digest).write_bytes(raw)
                        sequence += 1
                        writer.write(json.dumps({"schema_version": 1, "event_id": str(uuid.uuid4()),
                                                 "sequence": sequence,
                                                 "timestamp": "2026-11-19T00:00:00Z",
                                                 "session_id": session_id, "kind": kind,
                                                 "payload": {"blob": {"id": digest, "len": len(raw)}}}) + "\n")
            phase = "larger_resume"
            count = len(server.requests)
            terminal = Terminal(binary, workspace, env, resume=session_id, observer=sample)
            terminal.pump(0.3)
            assert len(server.requests) == count
            terminal.close()
        finally:
            if terminal.process.poll() is None:
                if hasattr(terminal, "pid"):
                    try:
                        os.kill(terminal.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                terminal.process.kill()
                terminal.process.wait()
    server.shutdown()
    server.server_close()
    assert all(observed.values()), observed
    return {"workspace_text_files": 256, "workspace_text_bytes_per_file": 4096,
            "streamed_text_bytes": len(stream_text.encode()),
            "additional_historical_conversations": 256,
            "observed_parent_rss_max_bytes_by_stage": observed,
            "method": "ps RSS samples during PTY activity, at most every 50ms; excludes child processes; observational maxima, not guaranteed instantaneous peaks"}


def paste_profile(binary):
    """Measure real PTY transfer/preview and input response, without inference."""
    results = []
    with tempfile.TemporaryDirectory(prefix="ruvx-large-paste-") as directory:
        root = Path(directory)
        workspace, _, data, env = fixture_env(root, 1)
        terminal = Terminal(binary, workspace, env)
        terminal.enter("/model fixture m")
        peak = [0]
        stopped = threading.Event()

        def sample():
            while not stopped.wait(0.05):
                try:
                    value = subprocess.check_output(["ps", "-o", "rss=", "-p", str(terminal.pid)], stderr=subprocess.DEVNULL).strip()
                    peak[0] = max(peak[0], int(value) * 1024)
                except (ValueError, subprocess.CalledProcessError):
                    pass

        observer = threading.Thread(target=sample, daemon=True)
        observer.start()
        try:
            for index, size in enumerate((1024 * 1024, 10 * 1024 * 1024), start=1):
                payload = ("long-paste-line\n" * (size // 16 + 1))[:size]
                peak[0] = 0
                started = time.perf_counter()
                terminal.send("\x1b[200~" + payload + "\x1b[201~")
                terminal.wait(lambda: f"[Paste {index}" in terminal.grid.text(), "large paste preview", timeout=30)
                preview_ms = (time.perf_counter() - started) * 1000
                started = time.perf_counter()
                terminal.send(" suffix")
                terminal.wait(lambda: "suffix" in terminal.grid.text(), "editing after large paste")
                keyboard_ms = (time.perf_counter() - started) * 1000
                result = {"paste_bytes": size, "transfer_and_preview_ms": round(preview_ms, 3),
                          "keyboard_response_ms": round(keyboard_ms, 3)}
                if index == 2:
                    terminal.enter()
                    terminal.wait(lambda: "Message not sent; draft preserved" in terminal.grid.text(), "oversized context rejected with draft intact", timeout=30)
                    assert "[Paste 2" in terminal.grid.text()
                    assert not any(event["kind"] == "message.user" for event in events(data))
                    result["preflight_context_rejected_draft_preserved"] = True
                result["observed_parent_rss_max_bytes"] = peak[0]
                results.append(result)
                terminal.send(b"\x15")
                terminal.wait(lambda: "[Paste" not in terminal.grid.text(), "clear draft references")
            terminal.close()
        finally:
            stopped.set()
            observer.join(timeout=2)
            if terminal.process.poll() is None: terminal.process.kill()
    return {"cases": results, "method": "Bracketed paste through PTY, including terminal transfer/decoding and 30 FPS rendering; ps parent RSS every 50ms; no provider inference"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/ruvx"))
    parser.add_argument("--benchmark", action="store_true")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    help_result = subprocess.run([str(binary), "--help"], capture_output=True, check=True)
    assert b"--resume" in help_result.stdout
    non_tty = subprocess.run([str(binary)], input=b"", capture_output=True)
    assert non_tty.returncode != 0 and b"Interactive terminal required" in non_tty.stderr
    report = {"platform": platform.platform(), "machine": platform.machine(),
              "python": platform.python_version(), "binary": str(binary),
              "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "functional": functional(binary), "git_workflow": git_push_workflow(binary),
              "git_auth": git_auth_terminal_isolation(binary), "bypass": bypass_workflow(binary)}
    if args.benchmark:
        report["performance"] = benchmark(binary)
        report["load_profile"] = load_profile(binary)
        report["paste_profile"] = paste_profile(binary)
    text = json.dumps(report, indent=2, ensure_ascii=False) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text)
    print(text, end="")


if __name__ == "__main__":
    main()
