"""e2e helpers: start the real kuterm on the xvfb display, drive it with xdotool and read pixels.

the display, window manager and session bus are started once per test session in conftest.py.
every App gets its own temp XDG_CONFIG_HOME and HOME, so cases never share config files.
"""

import contextlib
import ctypes
import io
import json
import math
import os
import re
import shutil
import signal
import subprocess
import tempfile
import time
from pathlib import Path

import numpy as np
from PIL import Image, ImageGrab

SRC = Path(os.environ.get("KUTERM_SRC", "/src"))
BIN = Path(os.environ.get("KUTERM_BIN", SRC / "target/podman/debug/kuterm"))
ASSETS = SRC / "assets"
ARTIFACTS = Path(os.environ.get("KUTERM_ARTIFACTS", SRC / "e2e/artifacts"))

# bundled jetbrains mono advance is 600 of 1000 units
JB_ADVANCE = 0.6

# the pty starts at a tiny default size until the first layout, output printed before the
# resize can get shifted, so scripts wait for the real size first
SCRIPT_PREAMBLE = (
    'i=0; while [ "$(stty size)" = "6 100" ] && [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done; sleep 0.1\n'
    # step N waits until the test calls app.step(N), to change the screen between checks
    'step() { while [ ! -f "$T2_TMP/step$1" ]; do sleep 0.02; done; }\n'
)

# interactive bash without user files, so the prompt is just "$ "
BASH = {"with_arguments": {"program": "/bin/bash", "args": ["--norc", "--noprofile", "-i"]}}


def jsonc(path: Path) -> dict:
    """parse a bundled jsonc file (full line comments and trailing commas)"""
    text = "\n".join(line for line in path.read_text().splitlines() if not line.strip().startswith("//"))
    text = re.sub(r",(\s*[}\]])", r"\1", text)
    return json.loads(text)


def rgb(value: str) -> tuple:
    """'#rrggbb' or '#rgb' to an (r, g, b) tuple"""
    value = value.lstrip("#")
    if len(value) in (3, 4):
        value = "".join(c * 2 for c in value[:3])
    return tuple(int(value[i : i + 2], 16) for i in (0, 2, 4))


def as_rgb(theme: dict) -> dict:
    """theme colors as rgb tuples, ansi and ansi_dim as lists"""
    return {k: [rgb(c) for c in v] if isinstance(v, list) else rgb(v) for k, v in theme.items()}


def bundled_theme(dark: bool) -> dict:
    return as_rgb(bundled_settings()["theme"]["themes"]["dark" if dark else "light"])


def bundled_settings() -> dict:
    return jsonc(ASSETS / "default_settings.jsonc")


def unique_theme(seed: int) -> dict:
    """a full theme where every color differs from each other and from both bundled themes"""
    colors = iter(f"#{seed:02x}{i * 37 % 256:02x}{(i * 91 + 50) % 256:02x}" for i in range(1, 60))
    keys = ["tab_bar_background", "tab_active_background", "border", "text", "text_muted",
            "terminal_background", "terminal_foreground", "cursor", "selection", "scrollbar",
            "bright_foreground", "dim_foreground", "danger_button", "danger_button_text"]
    theme = {key: next(colors) for key in keys}
    theme["ansi"] = [next(colors) for _ in range(16)]
    theme["ansi_dim"] = [next(colors) for _ in range(8)]
    return theme


def wait_until(fn, timeout: float = 10.0, interval: float = 0.1, msg: str = "condition"):
    """poll fn until it returns something truthy, fail with its last value on timeout"""
    deadline = time.monotonic() + timeout
    last = None
    while True:
        try:
            last = fn()
        except Exception as error:  # a check may race the app, retry until the deadline
            last = error
        # numpy arrays have no truth value, any array counts as found
        if not isinstance(last, Exception) and (isinstance(last, np.ndarray) or last):
            return last
        if time.monotonic() > deadline:
            raise AssertionError(f"timed out after {timeout}s waiting for {msg}, last value: {last!r}")
        time.sleep(interval)


def sh(*args: str, check: bool = True, timeout: float = 10, **kw) -> str:
    """run a command and return stdout"""
    res = subprocess.run(args, capture_output=True, text=True, timeout=timeout, **kw)
    if check and res.returncode != 0:
        raise RuntimeError(f"{args} failed ({res.returncode}): {res.stderr.strip()}")
    return res.stdout


def diff(img: np.ndarray, color: tuple) -> np.ndarray:
    """per pixel max channel distance to color"""
    return np.abs(img.astype(np.int16) - np.array(color[:3], dtype=np.int16)).max(axis=-1)


def near(img: np.ndarray, color: tuple, tol: int = 2) -> np.ndarray:
    """bool mask of pixels within tol of color on every channel"""
    return diff(img, color) <= tol


def count(img: np.ndarray, color: tuple, tol: int = 2) -> int:
    return int(near(img, color, tol).sum())


def mask_bbox(mask: np.ndarray) -> tuple | None:
    ys, xs = np.nonzero(mask)
    if len(xs) == 0:
        return None
    return int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1


def median(img: np.ndarray) -> tuple:
    """median color of a region"""
    return tuple(int(v) for v in np.median(img.reshape(-1, 3), axis=0))


def dominant(img: np.ndarray) -> tuple:
    """most common exact color of a region"""
    flat = img.reshape(-1, 3).astype(np.int32)
    keys = (flat[:, 0] << 16) | (flat[:, 1] << 8) | flat[:, 2]
    values, counts = np.unique(keys, return_counts=True)
    k = int(values[counts.argmax()])
    return (k >> 16) & 255, (k >> 8) & 255, k & 255


def close_to(a: tuple, b: tuple, tol: int = 2) -> bool:
    return all(abs(int(x) - int(y)) <= tol for x, y in zip(a[:3], b[:3]))


def ink(img: np.ndarray, background: tuple, threshold: int = 40) -> np.ndarray:
    """bool mask of pixels that differ clearly from the background"""
    return diff(img, background) > threshold


def ink_bbox(img: np.ndarray, background: tuple, threshold: int = 40) -> tuple | None:
    """bbox of pixels that differ clearly from the background, e.g. antialiased text"""
    return mask_bbox(ink(img, background, threshold))


# which display server the apps run on: "x11" (xvfb and openbox) or "wayland" (headless sway)
BACKEND = os.environ.get("E2E_BACKEND", "x11")


class Session:
    display = os.environ.get("DISPLAY", ":99")
    wayland_display = "wayland-1"
    sway_socket = ""
    runtime_dir = "/tmp/e2e-runtime"
    bus_address = ""


def session_env() -> dict:
    """environment for programs talking to the test display, apps get their own on top"""
    env = {
        "PATH": os.environ.get("PATH", "/usr/local/bin:/usr/bin:/bin"),
        "XDG_RUNTIME_DIR": Session.runtime_dir,
        "LANG": "C.UTF-8",
        "USER": "root",
        "TERM": "dumb",
    }
    if BACKEND == "wayland":
        # no DISPLAY, so gpui picks wayland
        env.update({"WAYLAND_DISPLAY": Session.wayland_display, "SWAYSOCK": Session.sway_socket})
    else:
        env.update({"DISPLAY": Session.display, "WAYLAND_DISPLAY": "", "GPUI_X11_SCALE_FACTOR": "1"})
    if Session.bus_address:
        env["DBUS_SESSION_BUS_ADDRESS"] = Session.bus_address
    if "VK_ICD_FILENAMES" in os.environ:
        env["VK_ICD_FILENAMES"] = os.environ["VK_ICD_FILENAMES"]
    return env


def xdo(*args: str, check: bool = True) -> str:
    return sh("xdotool", *args, check=check, env=session_env())


# X11 keysyms of F1..F12, xdotool pads function keys with Alt on this keymap
FUNCTION_KEYSYMS = {f"F{n}": 0xFFBE + n - 1 for n in range(1, 13)}


def x11_keypress(keysym: int) -> None:
    """press and release a key by keysym through XTest, without xdotool's extra modifiers"""
    x11 = ctypes.CDLL("libX11.so.6")
    xtst = ctypes.CDLL("libXtst.so.6")
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XKeysymToKeycode.restype = ctypes.c_ubyte
    x11.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
    x11.XFlush.argtypes = [ctypes.c_void_p]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    xtst.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
    display = x11.XOpenDisplay(Session.display.encode())
    assert display, f"cannot open display {Session.display}"
    keycode = x11.XKeysymToKeycode(display, keysym)
    xtst.XTestFakeKeyEvent(display, keycode, 1, 0)
    x11.XFlush(display)
    time.sleep(0.05)
    xtst.XTestFakeKeyEvent(display, keycode, 0, 0)
    x11.XFlush(display)
    x11.XCloseDisplay(display)


def swaymsg(*args: str, check: bool = True):
    """run a sway command, json replies are parsed"""
    out = sh("swaymsg", *args, check=check, env=session_env())
    return json.loads(out) if out.strip().startswith(("[", "{")) else out


def screen(x: int, y: int, w: int, h: int) -> np.ndarray:
    """grab a screen area as an rgb array, in physical pixels"""
    if BACKEND == "wayland":
        # grim takes the area in logical pixels and returns the output's physical ones
        scale = Input.scale()
        png = subprocess.run(["grim", "-g", f"{x},{y} {round(w / scale)}x{round(h / scale)}", "-"],
                             capture_output=True, env=session_env(), timeout=10, check=True).stdout
        return np.asarray(Image.open(io.BytesIO(png)).convert("RGB"))
    img = ImageGrab.grab(bbox=(x, y, x + w, y + h), xdisplay=Session.display)
    return np.asarray(img.convert("RGB"))


def set_clipboard(text: str) -> None:
    """own the clipboard, the owner keeps serving it until someone else takes it"""
    args = ["wl-copy"] if BACKEND == "wayland" else ["xclip", "-selection", "clipboard", "-i"]
    proc = subprocess.Popen(args, stdin=subprocess.PIPE, env=session_env(),
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    proc.communicate(text.encode(), timeout=5)


def get_clipboard() -> str:
    if BACKEND == "wayland":
        return sh("wl-paste", "-n", check=False, env=session_env())
    return sh("xclip", "-selection", "clipboard", "-o", check=False, env=session_env())


# xdotool button numbers to linux input codes, 4 and 5 are the wheel
WAYLAND_BUTTONS = {1: 272, 2: 274, 3: 273}


class Input:
    """the wayland session's virtual keyboard and pointer, see fake_input.py"""

    proc = None

    @classmethod
    def start(cls) -> None:
        cls.proc = subprocess.Popen(["python3", str(Path(__file__).with_name("fake_input.py"))],
                                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
                                    env={**session_env(), "PYTHONDONTWRITEBYTECODE": "1"})
        line = cls.proc.stdout.readline().strip()
        assert line == "ready", f"virtual input failed to start: {line!r}"

    @classmethod
    def send(cls, command: str) -> None:
        cls.proc.stdin.write(command + "\n")
        cls.proc.stdin.flush()
        line = cls.proc.stdout.readline().strip()
        assert line == "ok", f"virtual input: {command!r} gave {line!r}"

    @classmethod
    def output(cls) -> dict:
        """the output apps open on, tests may add more"""
        return next(o for o in swaymsg("-t", "get_outputs") if o["name"] == "HEADLESS-1")

    @classmethod
    def scale(cls) -> float:
        return float(cls.output()["scale"])

    @classmethod
    def move(cls, x: float, y: float) -> None:
        # absolute positions span the box around every output
        rects = [o["rect"] for o in swaymsg("-t", "get_outputs") if o["active"]]
        w = max(r["x"] + r["width"] for r in rects)
        h = max(r["y"] + r["height"] for r in rects)
        cls.send(f"move {x} {y} {w} {h}")


class Portal:
    """fake desktop portal answering color-scheme, 0 none, 1 dark, 2 light"""

    def __init__(self, scheme: int) -> None:
        self.proc = subprocess.Popen(
            ["python3", str(Path(__file__).with_name("fake_portal.py")), str(scheme)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            env={**session_env(), "PYTHONDONTWRITEBYTECODE": "1"},
        )
        line = self.proc.stdout.readline().strip()
        assert line == "ready", f"fake portal failed to start: {line!r}"

    def set(self, scheme: int) -> None:
        self.proc.stdin.write(f"{scheme}\n")
        self.proc.stdin.flush()
        assert self.proc.stdout.readline().strip() == "ok"

    def stop(self) -> None:
        if self.proc.poll() is None:
            self.proc.stdin.close()
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()


class App:
    """one kuterm process with its own config folder, closed with close().

    settings: dict written as settings.jsonc (the app merges it over defaults), or a raw string.
    script: sh script run by the default profile, it ends with a long sleep so tabs stay open.
    without script and without profiles in settings, an interactive bash --norc with PS1="$ " runs.
    commands, keybindings, tab_icons: dicts put into their sections of settings, commands as {"commands": [...]}.
    themes: theme colors by name, put into "theme" -> "themes" of settings, like {"dark": {...}}.
    files: extra files inside the config folder, like {"local.jsonc": {...}}.
    args: command line options.
    on_shot: called with (app, caption, png path) for every screenshot taken for the report.
    """

    def __init__(
        self,
        settings=None,
        *,
        script: str | None = None,
        keybindings=None,
        commands=None,
        tab_icons=None,
        themes: dict | None = None,
        files: dict | None = None,
        args: list | None = None,
        env: dict | None = None,
        name: str = "app",
        start: bool = True,
        on_shot=None,
    ) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix=f"e2e-{name}-"))
        self.home = self.tmp / "home"
        self.config_home = self.tmp / "config"
        self.config = self.config_home / "kuterm"
        self.home.mkdir()
        self.config.mkdir(parents=True)
        self.settings = dict(settings) if isinstance(settings, dict) else settings
        self.args = [str(a) for a in (args or [])]
        self.env_extra = env or {}
        # physical pixels per logical one, the layout helpers below return physical pixels
        self.scale = float(self.env_extra.get("GPUI_X11_SCALE_FACTOR") or 1)
        self.proc = None
        self.wid = None
        self.log_path = self.tmp / "app.log"
        self.on_shot = on_shot
        # config files as written, shown in the report as the tested state
        self.written: dict = {}

        if isinstance(self.settings, dict) and "profiles" not in self.settings:
            command = BASH
            if script is not None:
                script_path = self.tmp / "run.sh"
                script_path.write_text(SCRIPT_PREAMBLE + script + "\nexec sleep 100000\n")
                command = {"with_arguments": {"program": "/bin/sh", "args": [str(script_path)]}}
            self.settings["profiles"] = [{"name": "test", "command": command}]
        if themes and isinstance(self.settings, dict):
            # a copy, callers often share one settings dict between tests
            self.settings["theme"] = {**self.settings.get("theme", {}), "themes": themes}
        for name, content in (("keybindings", keybindings), ("commands", commands), ("tab_icons", tab_icons)):
            if content is not None:
                self.settings[name] = content["commands"] if name == "commands" else content
        if self.settings is not None:
            self.write("settings.jsonc", self.settings)
        for rel, content in (files or {}).items():
            self.write(rel, content)
        if start:
            self.start()

    def write(self, rel: str, content) -> Path:
        """write a config file relative to the config folder, or an absolute path"""
        path = self.config / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        text = content if isinstance(content, str) else json.dumps(content, indent=2, ensure_ascii=False)
        path.write_text(text)
        self.written[rel] = text
        return path

    def app_env(self) -> dict:
        env = session_env()
        env.update({"HOME": str(self.home), "XDG_CONFIG_HOME": str(self.config_home), "PS1": "$ ",
                    "T2_TMP": str(self.tmp)})
        for key, value in self.env_extra.items():
            if value is None:
                env.pop(key, None)
            else:
                env[key] = value
        return env

    def start(self) -> None:
        self.log = open(self.log_path, "w")
        self.proc = subprocess.Popen(
            [str(BIN), *self.args], cwd=self.home, env=self.app_env(), stdout=self.log, stderr=subprocess.STDOUT
        )
        self.wid = wait_until(self._find_window, timeout=30, msg="the app window")
        if BACKEND == "wayland":
            # the output scale is set by the compositor, not by the app env
            self.scale = Input.scale()
        # give it focus like a user clicking it, the window manager usually does this on map already
        self.focus()
        # first frame: the window is no longer blank black
        wait_until(self._painted, timeout=30, msg="the first painted frame")

    def _painted(self) -> bool:
        if not self.alive():
            raise AssertionError(f"app exited with {self.proc.returncode}:\n{self.output()}")
        img = self.shot()
        return count(img, (0, 0, 0), 0) < img.shape[0] * img.shape[1] // 2

    def _find_window(self):
        if self.proc.poll() is not None:
            raise AssertionError(f"app exited early with {self.proc.returncode}:\n{self.output()}")
        if BACKEND == "wayland":
            node = self._node()
            return str(node["id"]) if node and node.get("visible") else None
        out = xdo("search", "--onlyvisible", "--pid", str(self.proc.pid), check=False).split()
        return out[0] if out else None

    def _node(self) -> dict | None:
        """sway tree node of the app window"""
        stack = [swaymsg("-t", "get_tree")]
        while stack:
            node = stack.pop()
            if node.get("pid") == self.proc.pid:
                return node
            stack.extend(node.get("nodes", []) + node.get("floating_nodes", []))
        return None

    def _sway(self, command: str) -> None:
        swaymsg(f"[con_id={self.wid}] {command}", check=False)

    def alive(self) -> bool:
        return self.proc is not None and self.proc.poll() is None

    def output(self) -> str:
        """everything the app printed to stdout and stderr so far"""
        with contextlib.suppress(Exception):
            self.log.flush()
        return self.log_path.read_text(errors="replace") if self.log_path.exists() else ""

    def close(self) -> None:
        if self.proc is not None and self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        # shells and sleeps started by the app are its children, don't leak them into later tests
        sh("pkill", "-KILL", "-f", str(self.tmp), check=False)
        with contextlib.suppress(Exception):
            self.log.close()

    def cleanup(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    # process stats read from /proc, the app alone without its shells

    def cpu_seconds(self) -> float:
        """user and system cpu time of all app threads"""
        # the command name in field 2 may hold spaces, so split after its closing paren
        fields = Path(f"/proc/{self.proc.pid}/stat").read_text().rsplit(")", 1)[1].split()
        return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")

    def cpu_usage(self, seconds: float) -> float:
        """share of one core used over the next seconds, 1.0 is a fully busy core"""
        start = self.cpu_seconds()
        time.sleep(seconds)
        return (self.cpu_seconds() - start) / seconds

    def rss_mb(self) -> float:
        for line in Path(f"/proc/{self.proc.pid}/status").read_text().splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1]) / 1024
        return 0.0

    def fd_count(self) -> int:
        return len(os.listdir(f"/proc/{self.proc.pid}/fd"))

    def thread_count(self) -> int:
        return len(os.listdir(f"/proc/{self.proc.pid}/task"))

    def children(self) -> list:
        """(pid, state) of direct child processes, like shells of open tabs"""
        found = []
        for entry in os.listdir("/proc"):
            if not entry.isdigit():
                continue
            try:
                fields = Path(f"/proc/{entry}/stat").read_text().rsplit(")", 1)[1].split()
            except OSError:
                continue
            if int(fields[1]) == self.proc.pid:
                found.append((int(entry), fields[0]))
        return found

    def geometry(self) -> tuple:
        """(x, y, w, h) of the client area on screen, the size in physical pixels"""
        if BACKEND == "wayland":
            # the position stays logical, it is only passed back to screen()
            rect = self._node()["rect"]
            return rect["x"], rect["y"], round(rect["width"] * self.scale), round(rect["height"] * self.scale)
        info = sh("xwininfo", "-id", self.wid, env=session_env())
        val = dict(line.strip().split(":", 1) for line in info.splitlines() if ":" in line)
        return tuple(int(val[k]) for k in ("Absolute upper-left X", "Absolute upper-left Y", "Width", "Height"))

    def size(self) -> tuple:
        return self.geometry()[2:]

    def fullscreen(self) -> bool:
        """whether the window manager reports the window as fullscreen"""
        if BACKEND == "wayland":
            return bool(self._node().get("fullscreen_mode"))
        state = sh("xprop", "-id", self.wid, "_NET_WM_STATE", env=session_env())
        return "_NET_WM_STATE_FULLSCREEN" in state

    def shot(self) -> np.ndarray:
        """screenshot of the client area"""
        return screen(*self.geometry())

    def snap(self, caption: str) -> None:
        """screenshot for the report, showing the state reached so far"""
        if self.on_shot is not None and self.alive():
            self.on_shot(self, caption)

    def wait(self, check, msg: str = "condition", timeout: float = 10.0):
        """wait_until for this app, adding the app output when it times out"""
        try:
            return wait_until(check, timeout=timeout, msg=msg)
        except AssertionError as error:
            raise AssertionError(f"{error}\napp output:\n{self.output()[-1500:]}") from None

    def title(self) -> str:
        if BACKEND == "wayland":
            return self._node()["name"] or ""
        return xdo("getwindowname", self.wid).rstrip("\n")

    def wait_title(self, expected, timeout: float = 10) -> str:
        """wait for the window title to equal expected (or satisfy it, when callable)"""
        matches = expected if callable(expected) else (lambda t: t == expected)

        def check() -> str | None:
            title = self.title()
            return title if matches(title) else None

        return self.wait(check, timeout=timeout, msg=f"window title {expected!r} (now {self.title()!r})")

    def focus(self) -> None:
        if BACKEND == "wayland":
            self._sway("focus")
            wait_until(lambda: self._node()["focused"], timeout=5, msg="focus")
            return
        xdo("windowactivate", "--sync", self.wid, check=False)
        wait_until(lambda: xdo("getactivewindow", check=False).strip() == self.wid, timeout=5, msg="focus")

    def wm_close(self) -> None:
        """close the window like its title bar button: openbox sends WM_DELETE_WINDOW, sway
        xdg_toplevel.close"""
        if BACKEND == "wayland":
            self._sway("kill")
        else:
            sh("wmctrl", "-i", "-c", self.wid, env=session_env())

    def exit_code(self, timeout: float = 10) -> int:
        """wait for the app to exit on its own and return its exit code"""
        return self.proc.wait(timeout)

    def resize(self, w: int, h: int) -> None:
        if BACKEND == "wayland":
            self._sway(f"resize set {w} {h}")
            return
        xdo("windowsize", "--sync", self.wid, str(w), str(h), check=False)

    def key(self, *keys: str) -> None:
        """press keys named like xdotool does, e.g. key('ctrl+shift+t', 'Return')"""
        self.focus()
        if BACKEND == "wayland":
            for combo in keys:
                *mods, name = combo.split("+")
                Input.send(f"key {','.join(m.lower() for m in mods) or '-'} {name}")
                time.sleep(0.04)
            return
        for combo in keys:
            # xdotool presses Alt around function keys on this keymap, send them by keycode
            keysym = FUNCTION_KEYSYMS.get(combo)
            if keysym is None:
                xdo("key", "--clearmodifiers", "--delay", "40", combo)
            else:
                x11_keypress(keysym)

    def type(self, text: str) -> None:
        self.focus()
        if BACKEND == "wayland":
            Input.send(f"type {text.encode().hex()}")
            return
        xdo("type", "--delay", "8", "--", text)

    def run(self, command: str) -> None:
        """type a shell command and press enter"""
        self.type(command)
        self.key("Return")

    def mouse(self, x: int, y: int) -> None:
        """move the pointer to window coordinates in physical pixels"""
        if BACKEND == "wayland":
            rect = self._node()["rect"]
            Input.move(rect["x"] + x / self.scale, rect["y"] + y / self.scale)
            return
        xdo("mousemove", "--window", self.wid, str(x), str(y))

    def button(self, button: int, down: bool) -> None:
        """press or release a mouse button where the pointer is"""
        if BACKEND == "wayland":
            Input.send(f"button {WAYLAND_BUTTONS[button]} {int(down)}")
        else:
            xdo("mousedown" if down else "mouseup", str(button))

    def click(self, x: int, y: int, button: int = 1, repeat: int = 1) -> None:
        """click, several times for a double or triple click"""
        self.focus()
        self.mouse(x, y)
        time.sleep(0.05)
        for i in range(repeat):
            if i:
                time.sleep(0.08)
            self.button(button, True)
            self.button(button, False)

    @contextlib.contextmanager
    def hold(self, modifier: str):
        """keep a modifier key down, like shift for a shift+click"""
        self.focus()
        if BACKEND == "wayland":
            Input.send(f"hold {modifier}")
            try:
                yield
            finally:
                Input.send("hold -")
            return
        xdo("keydown", modifier)
        try:
            yield
        finally:
            xdo("keyup", modifier)

    def drag(self, start: tuple, end: tuple) -> None:
        self.focus()
        self.mouse(*start)
        self.button(1, True)
        for i in range(1, 6):
            self.mouse(start[0] + (end[0] - start[0]) * i // 5, start[1] + (end[1] - start[1]) * i // 5)
            time.sleep(0.03)
        self.button(1, False)

    def wheel(self, x: int, y: int, up: bool, clicks: int = 5) -> None:
        self.focus()
        self.mouse(x, y)
        if BACKEND == "wayland":
            Input.send(f"wheel {-clicks if up else clicks}")
            return
        xdo("click", "--repeat", str(clicks), "--delay", "20", "4" if up else "5")

    def palette(self, query: str, run: bool = True) -> None:
        """open the command palette with its default key, type a query and run the best match"""
        self.key("ctrl+shift+p")
        time.sleep(0.4)
        self.type(query)
        time.sleep(0.2)
        if run:
            self.key("Return")

    def step(self, n: int) -> None:
        """let the script run past `step n`"""
        (self.tmp / f"step{n}").touch()

    def file(self, name: str) -> Path:
        """path inside the case temp dir, handy for shell side effects"""
        return self.tmp / name

    def wait_file(self, name: str, content: str | None = None, timeout: float = 10) -> str:
        path = self.file(name)

        def check():
            if not path.exists():
                return None
            text = path.read_text()
            if content is None or text.strip() == content:
                return text or " "
            return None

        return self.wait(check, timeout=timeout, msg=f"file {name} with {content!r}")

    def pty_size(self, name: str = "size") -> tuple:
        """(rows, cols) the shell sees, read with stty in an interactive bash"""
        path = self.file(name)
        path.unlink(missing_ok=True)
        self.run(f"stty size > {path}")
        return tuple(map(int, self.wait_file(name).split()))

    # layout, mirrors the element code so the tests can find cells and the bar

    def _get(self, *keys, default=None):
        value = self.settings if isinstance(self.settings, dict) else {}
        for key in keys:
            if not isinstance(value, dict) or key not in value:
                return default
            value = value[key]
        return value

    def font_size(self) -> float:
        default = bundled_settings()["terminal"]["font_size"]
        size = float(self._get("terminal", "font_size", default=default))
        return default if size < 6 else min(size, 72.0)

    def ui_font_size(self) -> float:
        default = bundled_settings()["ui_font_size"]
        size = float(self._get("ui_font_size", default=default))
        return default if size < 6 else min(size, 72.0)

    def bar_height(self) -> int:
        """tab bar height with its bottom border, when shown"""
        # 2 rems, the border is drawn inside that height
        return round(round(2 * self.ui_font_size()) * self.scale)

    def _cell_width(self) -> float:
        return self.font_size() * JB_ADVANCE

    def cell_width(self) -> float:
        return self._cell_width() * self.scale

    def _line_height(self) -> int:
        lh = self._get("terminal", "line_height", default=bundled_settings()["terminal"]["line_height"])
        value = {"standard": 1.3, "comfortable": 1.618}.get(lh) if isinstance(lh, str) else float(lh["custom"])
        if value < 1:
            value = 1.3
        value = min(value, 3.0)
        # rust rounds half away from zero
        return int(math.floor(self.font_size() * value + 0.5))

    def line_height(self) -> float:
        # rounded in logical pixels, then scaled
        value = self._line_height() * self.scale
        return int(value) if value == int(value) else value

    def _scrollbar_width(self) -> float:
        bar = bundled_settings()["terminal"]["scrollbar"]
        if self._get("terminal", "scrollbar", "enable", default=bar["enable"]) == "off":
            return 0.0
        width = float(self._get("terminal", "scrollbar", "width", default=bar["width"]))
        return float(bar["width"]) if width < 2 else min(width, 64.0)

    def scrollbar_width(self) -> float:
        return self._scrollbar_width() * self.scale

    def expected_pty(self, w: int, h: int, top: int = 0) -> tuple:
        """(rows, cols) for a window of w x h logical pixels with `top` of them taken by the tab bar"""
        cw, lh = self._cell_width(), self._line_height()
        cols = int(max(w - cw - self._scrollbar_width(), cw * 2) // cw)
        rows = int(max(h - top, lh) // lh)
        return rows, cols

    def cell_rect(self, col: int, row: int, cols: int = 1, rows: int = 1, top: int = 0) -> tuple:
        """(x0, y0, x1, y1) of a block of cells, top is the tab bar height when shown"""
        cw, lh = self.cell_width(), self.line_height()
        # cell (0, 0) sits one cell width in from the left edge, after a left scrollbar
        ox, oy = math.floor(cw), top
        if self._get("terminal", "scrollbar", "placement") == "left":
            ox = math.floor(cw + self.scrollbar_width())
        return (
            ox + math.floor(col * cw),
            oy + math.floor(row * lh),
            ox + math.ceil((col + cols) * cw),
            oy + math.ceil((row + rows) * lh),
        )

    def cells_color(self, col: int, row: int, cols: int, img=None, top: int = 0) -> tuple:
        """median color of the inside of a block of cells on one row, away from glyph edges"""
        img = self.shot() if img is None else img
        x0, y0, x1, y1 = self.cell_rect(col, row, cols, 1, top)
        lh = y1 - y0
        return median(img[y0 + lh // 3 : y1 - lh // 3, x0 + 3 : x1 - 3])
