#!/usr/bin/env python3
"""record examples/presets/<preset>/demo.webp: kuterm with the preset's settings on a virtual x
display with a window manager, driven by xdotool like a person would, filmed with ffmpeg at the
screen's own resolution. "readme" records assets/showcase/showcase.webp for the readme instead:
a short 60 fps tour of the main features, with captions laid over by ffmpeg. runs inside the
image of tools/presets/Containerfile, see tools/presets/record.sh.

usage: record.py [--fps 60] [--stills] [preset or readme ...]
--stills also saves a png at every step() into /tmp/stills, to find where to click"""

import argparse
import json
import math
from datetime import datetime, timedelta
import os
import random
import shutil
import signal
import subprocess
import tempfile
import time
from pathlib import Path

SRC = Path("/src")
PRESETS = SRC / "examples/presets"
BIN = Path(os.environ.get("KUTERM_BIN", SRC / "target/podman/release/kuterm"))
DISPLAY = ":99"
DESKTOP = (1920, 1080)
# part of the desktop filmed, as size and offset, and the left (none to center it) and top of
# the window's client area in it. the readme leaves room right of the window for its captions
REGION = ((1440, 900), (240, 90), None, 84)
REGIONS = {"readme": ((1920, 1080), (0, 0), 72, 160)}
# client area of the window, the frame of the window manager comes around it
WINDOW = (1280, 760)
# videos that are not a preset's demo
OUTPUTS = {"readme": SRC / "assets/showcase/showcase.webp"}
# demos are webp for inline previews, scaled down since captions and wallpaper compress badly
# at full size
SCALE = "1200:-2"
STILLS = Path("/tmp/stills")

ZSHRC = """PS1='%F{green}dev@kuterm%f:%F{blue}%~%f$ '
unsetopt prompt_sp
alias ls='ls --color=auto'
# output goes into the terminal history, not into less, so scrolling shows it
export GIT_PAGER=cat
export PATH="$HOME/bin:$PATH"
"""

# the fake agent of the agents demo: python3 is in the "interpreters" list, so the script name
# "claude" picks the bundled agents group and its tab counts as an agent. it reports busy,
# goes idle without its command finishing, then waits for a line
AGENT = r'''#!/usr/bin/python3
import sys
import time


def progress(state: str) -> None:
    sys.stdout.write(f"\033]9;4;{state}\007")
    sys.stdout.flush()


progress("3")
time.sleep(1.2)
print("reading the codebase...", flush=True)
time.sleep(1.2)
progress("0")
print("ready for a review", flush=True)
for line in sys.stdin:
    progress("3")
    time.sleep(0.9)
    print(f"done: {line.strip()}", flush=True)
    progress("0")
    break
'''


def sh(*args: str, check: bool = True, **kw) -> str:
    return subprocess.run(args, check=check, capture_output=True, text=True, env=ENV, **kw).stdout


ENV = {**os.environ, "DISPLAY": DISPLAY}


def git(folder: Path, *args: str) -> None:
    env = {**ENV, "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_AUTHOR_NAME": "dev", "GIT_AUTHOR_EMAIL": "dev@kuterm",
           "GIT_COMMITTER_NAME": "dev", "GIT_COMMITTER_EMAIL": "dev@kuterm"}
    subprocess.run(["git", *args], cwd=folder, env=env, check=True, capture_output=True)


def commit(folder: Path, message: str, files: dict) -> None:
    for name, text in files.items():
        (folder / name).write_text(text)
    git(folder, "add", ".")
    git(folder, "commit", "-q", "-m", message)


def api_log() -> str:
    """a few hundred lines of a busy api's log, warnings, 404s and a failed order with its
    traceback, so the reading demo scrolls through something that looks real"""
    start = datetime(2026, 10, 18, 9, 12, 4)
    methods = ["GET", "POST", "GET", "GET", "PATCH", "DELETE"]
    paths = ["/health", "/users/42", "/users", "/orders", "/orders/8812",
             "/items?page=2", "/items?page=3", "/login", "/metrics"]
    lines = []
    for i in range(360):
        at = start + timedelta(seconds=3 * i)
        method = methods[i % len(methods)]
        path = paths[i % len(paths)]
        status = 500 if i % 67 == 66 else 404 if i % 41 == 40 else 200
        level = "INFO " if status == 200 else "WARN " if status == 404 else "ERROR"
        elapsed = 2 + (i * 7) % 40
        lines.append(f"{at:%Y-%m-%dT%H:%M:%S}.{at.microsecond // 1000:03d}Z {level} "
                     f"request {method:6} {path:18} {status} {elapsed:3}ms")
    lines += [
        "2026-10-18T09:30:04.000Z ERROR request POST   /orders/8812       500  12ms",
        "2026-10-18T09:30:04.001Z ERROR orders  Traceback (most recent call last):",
        '  File "/srv/api/orders.py", line 42, in create',
        "    total = sum(item.price for item in items)",
        "TypeError: unsupported operand type(s) for +: 'NoneType' and 'Decimal'",
    ]
    return "\n".join(lines) + "\n"


def projects(home: Path) -> None:
    """~/api with merged and mainline commits and a changed file, ~/web with a dev branch, and
    a realistic log for the reading demo"""
    api = home / "api"
    api.mkdir()
    git(api, "init", "-q", "-b", "main")
    commit(api, "initial api", {"README.md": "# api\n", "requirements.txt": "flask==3.1\n",
                                "app.py": "from flask import Flask\n\napp = Flask(__name__)\n"})
    git(api, "checkout", "-q", "-b", "feature/auth")
    commit(api, "KT-42 add token auth", {"auth.py": "def check(token: str) -> bool:\n    return token == 'secret'\n"})
    git(api, "checkout", "-q", "main")
    commit(api, "add health endpoint", {"app.py": (
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n"
        "@app.get('/health')\ndef health() -> dict:\n    \"\"\"tell load balancers we are up\"\"\"\n"
        "    return {'status': 'ok'}\n\n\n"
        "@app.get('/users/<int:user>')\ndef user(user: int) -> dict:\n    return {'id': user, 'name': 'ada'}\n")})
    git(api, "merge", "-q", "--no-ff", "feature/auth", "-m", "merge feature/auth")
    commit(api, "add request logging", {"log.py": (
        "import logging\n\nlogging.basicConfig(level=logging.INFO)\n")})
    commit(api, "handle empty orders", {"orders.py": (
        "def total(items: list) -> int:\n    return sum(item.price for item in items)\n")})
    commit(api, "bump dependencies", {"requirements.txt": "flask==3.1.1\n"})
    (api / "README.md").write_text("# api\n\nrun with `flask --app app run`\n")
    web = home / "web"
    web.mkdir()
    git(web, "init", "-q", "-b", "main")
    commit(web, "initial site", {"index.html": "<h1>kuterm</h1>\n", "package.json": '{ "name": "web" }\n'})
    commit(web, "add styles", {"style.css": "h1 { color: #89b4fa; }\n"})
    git(web, "checkout", "-q", "-b", "dev")
    commit(web, "add contact page", {"contact.html": "<h1>contact</h1>\n"})
    commit(web, "fix nav on small screens", {"style.css": "h1 { color: #89b4fa; }\nnav { display: flex; }\n"})
    # outside any project, so tab_groups.by_project leaves their tabs for groups made by hand
    for name, files in {"notes": ["todo.md", "ideas.md"], "logs": ["web.log"],
                        "scratch": ["draft.txt"]}.items():
        (home / name).mkdir()
        for file in files:
            (home / name / file).write_text("")
    (home / "logs/api.log").write_text(api_log())


class Desktop:
    """xvfb, a session bus, xfwm4 drawing frames and shadows, a gradient wallpaper"""

    def __init__(self) -> None:
        self.procs = [subprocess.Popen(["Xvfb", DISPLAY, "-screen", "0", f"{DESKTOP[0]}x{DESKTOP[1]}x24", "-nolisten", "tcp"],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)]
        wait(lambda: subprocess.run(["xdpyinfo"], env=ENV, capture_output=True).returncode == 0, "xvfb")
        bus = sh("dbus-daemon", "--session", "--fork", "--print-address=1").strip()
        ENV["DBUS_SESSION_BUS_ADDRESS"] = bus
        self.procs.append(subprocess.Popen(["xfwm4", "--compositor=on"], env=ENV,
                                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        time.sleep(2)

    def stop(self) -> None:
        for proc in reversed(self.procs):
            proc.terminate()


def wait(check, what: str, timeout: float = 30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.1)
    raise RuntimeError(f"timed out waiting for {what}")


def ease(t: float) -> float:
    """minimum jerk profile: how far along a hand is at time t of its move, slow at both ends"""
    return t * t * t * (10 - 15 * t + 6 * t * t)


def bezier(p0: tuple, p1: tuple, p2: tuple, p3: tuple, t: float) -> tuple:
    u = 1 - t
    return tuple(u * u * u * a + 3 * u * u * t * b + 3 * u * t * t * c + t * t * t * d
                 for a, b, c, d in zip(p0, p1, p2, p3))


def hand_path(start: tuple, end: tuple, rng: random.Random, fps: int, slow: float = 1.0) -> list:
    """screen points a hand moves the pointer through, one per frame: a curve bent to one side,
    taking longer the farther it goes (fitts' law), a bit past a far target and back"""
    dx, dy = end[0] - start[0], end[1] - start[1]
    dist = math.hypot(dx, dy)
    if dist < 1:
        return [end]
    seconds = (0.2 + 0.1 * math.log2(1 + dist / 10)) * slow * rng.uniform(0.9, 1.15)
    normal = (-dy / dist, dx / dist)
    bend = rng.uniform(0.06, 0.2) * dist * rng.choice((-1, 1))
    # far moves land a few pixels past the target and correct
    over = rng.uniform(4, 9) if dist > 250 else 0
    land = (end[0] + dx / dist * over, end[1] + dy / dist * over)
    c1 = (start[0] + dx * 0.3 + normal[0] * bend, start[1] + dy * 0.3 + normal[1] * bend)
    c2 = (start[0] + dx * 0.75 + normal[0] * bend * 0.4, start[1] + dy * 0.75 + normal[1] * bend * 0.4)
    frames = max(2, round(seconds * fps))
    points = [bezier(start, c1, c2, land, ease(i / frames)) for i in range(1, frames + 1)]
    if over:
        back = max(2, round(0.12 * fps))
        points += [(land[0] + (end[0] - land[0]) * ease(i / back), land[1] + (end[1] - land[1]) * ease(i / back))
                   for i in range(1, back + 1)]
    # a hand shakes a little, except where it stops
    shaky = [(x + rng.gauss(0, 0.4), y + rng.gauss(0, 0.4)) for x, y in points[:-1]]
    return [(round(x), round(y)) for x, y in shaky] + [end]


# caption look: white and gray on the wallpaper, in kuterm's bundled font
TEXT = (236, 238, 242, 255)
MUTED = (176, 182, 194, 255)
BOLD = SRC / "assets/fonts/jetbrains/JetBrainsMonoNLNerdFontMono-Bold.ttf"
REGULAR = SRC / "assets/fonts/jetbrains/JetBrainsMonoNLNerdFontMono-Regular.ttf"
# desktop wallpaper, a gradient through these colors; the readme's are the logo's slates
WALLPAPER = ["#313244", "#585b70", "#1e1e2e"]
WALLPAPERS = {
    "readme": ["#2a303b", "#444d5f", "#2f3542"],
    # a bright gradient, so the glass window, its padding and the desktop behind it stand out
    "glass": ["#ff6b6b", "#f7b267", "#4cc9f0", "#4361ee"],
}
# seconds overlays take to fade and slide in or out
FADE = 0.35


def wrap(draw, text: str, font, width: int) -> list:
    """lines of `text` no wider than `width`"""
    lines = []
    for paragraph in text.split("\n"):
        line = ""
        for word in paragraph.split():
            candidate = f"{line} {word}".strip()
            if line and draw.textlength(candidate, font=font) > width:
                lines.append(line)
                line = word
            else:
                line = candidate
        lines.append(line)
    return lines


def caption_png(path: Path, title: str, body: str, width: int) -> None:
    """a title and a line or two on what it is about, plain text on the wallpaper"""
    from PIL import Image, ImageDraw, ImageFont
    title_font = ImageFont.truetype(str(BOLD), 34)
    body_font = ImageFont.truetype(str(REGULAR), 21)
    probe = ImageDraw.Draw(Image.new("RGBA", (1, 1)))
    titles = wrap(probe, title, title_font, width)
    lines = wrap(probe, body, body_font, width)
    height = 46 * len(titles) + 20 + 32 * len(lines)
    img = Image.new("RGBA", (width, height))
    draw = ImageDraw.Draw(img)
    for ix, line in enumerate(titles):
        draw.text((0, 46 * ix), line, font=title_font, fill=TEXT)
    for ix, line in enumerate(lines):
        draw.text((0, 46 * len(titles) + 20 + 32 * ix), line, font=body_font, fill=MUTED)
    img.save(path)


def title_png(path: Path, size: tuple, title: str, body: str) -> None:
    """the whole frame darkened under the logo, a big title and a line or two"""
    from PIL import Image, ImageDraw, ImageFont
    img = Image.new("RGBA", size, (42, 48, 59, 235))
    draw = ImageDraw.Draw(img)
    logo = Image.open(SRC / "assets/logo/logo.png").convert("RGBA").resize((180, 180))
    top = size[1] // 2 - 230
    img.alpha_composite(logo, ((size[0] - 180) // 2, top))
    title_font = ImageFont.truetype(str(BOLD), 96)
    body_font = ImageFont.truetype(str(REGULAR), 32)
    draw.text((size[0] // 2, top + 260), title, font=title_font, fill=TEXT, anchor="mt")
    for ix, line in enumerate(body.split("\n")):
        draw.text((size[0] // 2, top + 390 + 50 * ix), line, font=body_font, fill=MUTED, anchor="mt")
    img.save(path)


def overlay_graph(overlays: list, tmp: Path, fps: int, scale: str | None = None) -> tuple:
    """ffmpeg inputs and filter graph laying `overlays` over the recording, each fading in and
    out and sliding up into place as it comes, then scaling the result when `scale` is set"""
    inputs, chain, last = [], [], "0:v"
    for ix, item in enumerate(overlays, start=1):
        start, end = item["start"], max(item["end"], item["start"] + 2 * FADE)
        duration = end - start
        inputs += ["-loop", "1", "-framerate", str(fps), "-t", f"{duration:.3f}", "-i", str(item["png"])]
        chain.append(f"[{ix}:v]format=rgba,fade=t=in:st=0:d={FADE}:alpha=1,"
                     f"fade=t=out:st={duration - FADE:.3f}:d={FADE}:alpha=1,"
                     f"setpts=PTS-STARTPTS+{start:.3f}/TB[o{ix}]")
        slide = f"{item['y']}+{item.get('slide', 0)}*pow(max(0,1-(t-{start:.3f})/{FADE}),2)"
        label = ("merged" if scale else "out") if ix == len(overlays) else f"v{ix}"
        chain.append(f"[{last}][o{ix}]overlay=x={item['x']}:y='{slide}':eof_action=pass:"
                     f"enable='between(t,{start:.3f},{end:.3f})'[{label}]")
        last = label
    if scale and overlays:
        # a simple -vf cannot be combined with -filter_complex, so scaling stays in the graph
        chain.append(f"[merged]scale={scale}[out]")
    return inputs, ";".join(chain)


class Scene:
    """one kuterm with a preset, the keys and clicks of the demo, and the recording"""

    def __init__(self, name: str, options: argparse.Namespace) -> None:
        self.name = name
        self.options = options
        self.stills = options.stills
        self.step_n = 0
        # the same scene moves and types the same way every time it is recorded
        self.rng = random.Random(name)
        self.tmp = Path(tempfile.mkdtemp(prefix=f"preset-{name}-"))
        self.size, self.offset, left, top = REGIONS.get(name, REGION)
        self.home = self.tmp / "home"
        config = self.home / ".config/kuterm"
        config.mkdir(parents=True)
        self.settings_path = config / "settings.jsonc"
        if name in README_START:
            self.settings = json.loads(json.dumps(README_START[name]))
            self.settings_path.write_text(json.dumps(self.settings, indent=2))
        else:
            shutil.copy(PRESETS / name / "settings.jsonc", self.settings_path)
        if name == "agents":
            agent = self.home / "bin/claude"
            agent.parent.mkdir()
            agent.write_text(AGENT)
            agent.chmod(0o755)
        # captions laid over the video, with their times in it
        self.overlays = []
        self.caption = None
        # time of the frame saved next to the video as its poster, none for no poster
        self.poster_at = None
        (self.home / ".zshrc").write_text(ZSHRC)
        projects(self.home)
        env = {**ENV, "HOME": str(self.home), "SHELL": "/usr/bin/zsh", "USER": "dev"}
        env.pop("XDG_CONFIG_HOME", None)
        sh("hsetroot", *(arg for color in WALLPAPERS.get(name, WALLPAPER) for arg in ("-add", color)),
           "-gradient", "135")
        self.log = open(self.tmp / "kuterm.log", "w")
        text = self.settings_path.read_text()
        client = '"decorations": "none"' in text or '"decorations": "client"' in text
        for _ in range(5):
            self.proc = subprocess.Popen([str(BIN)], cwd=self.home, env=env, stdout=self.log, stderr=subprocess.STDOUT)
            self.wid = wait(self._window, "the kuterm window", 60)
            # gpui falls back to a frame of the window manager while the compositor is not up
            # yet, as it is right after xfwm4 started. decorations absent means the default,
            # which the window manager draws
            if not client or self._decorations() == "0x0":
                break
            self.proc.terminate()
            self.proc.wait()
            time.sleep(2)
        sh("xdotool", "windowsize", "--sync", self.wid, str(WINDOW[0]), str(WINDOW[1]), check=False)
        x = self.offset[0] + (left if left is not None else (self.size[0] - WINDOW[0]) // 2)
        # the frame adds a title bar of about 28 pixels above the client area
        y = self.offset[1] + top - 28
        sh("xdotool", "windowmove", "--sync", self.wid, str(x), str(y), check=False)
        sh("xdotool", "windowactivate", "--sync", self.wid, check=False)
        # out of the way until a step needs it
        self.park()
        time.sleep(3)
        self.ffmpeg = None

    def _window(self) -> str | None:
        if self.proc.poll() is not None:
            raise RuntimeError(f"kuterm exited:\n{(self.tmp / 'kuterm.log').read_text()}")
        out = sh("xdotool", "search", "--onlyvisible", "--pid", str(self.proc.pid), check=False).split()
        return out[0] if out else None

    def _decorations(self) -> str:
        """the decorations field of the motif hints, 0x0 asks for no frame"""
        return sh("xprop", "-id", self.wid, "_MOTIF_WM_HINTS").split("=")[-1].split(",")[2].strip()

    def origin(self) -> tuple:
        """screen position of the client area, clicks are given inside it"""
        # xdotool gives the frame's place when the window manager draws one, xwininfo the client's
        info = dict(line.strip().split(":", 1) for line in sh("xwininfo", "-id", self.wid).splitlines()
                    if line.strip().startswith("Absolute upper-left"))
        return int(info["Absolute upper-left X"]), int(info["Absolute upper-left Y"])

    def record(self) -> None:
        if self.proc.poll() is not None:
            raise RuntimeError(f"kuterm exited:\n{(self.tmp / 'kuterm.log').read_text()}")
        self.raw = self.tmp / "raw.mkv"
        # lossless while filming, so encoding can't drop frames, the real encode comes after
        self.ffmpeg = subprocess.Popen(
            ["ffmpeg", "-loglevel", "error", "-f", "x11grab", "-draw_mouse", "1", "-framerate", str(self.options.fps),
             "-video_size", f"{self.size[0]}x{self.size[1]}", "-i", f"{DISPLAY}+{self.offset[0]},{self.offset[1]}",
             "-c:v", "libx264rgb", "-crf", "0", "-preset", "ultrafast", str(self.raw)],
            env=ENV, stdin=subprocess.PIPE)
        time.sleep(1)
        # overlay times count from here, about when the first frame was taken
        self.t0 = time.monotonic() - 0.5
        # captions go next to the window, found while it is still there
        ox, oy = self.origin()
        self.window_at = (ox - self.offset[0], oy - self.offset[1])

    def finish(self) -> None:
        time.sleep(1.5)
        self.ffmpeg.communicate(b"q")
        self.proc.terminate()
        self.say_end()
        self.render_captions()
        video = OUTPUTS.get(self.name, PRESETS / self.name / "demo.webp")
        video.parent.mkdir(parents=True, exist_ok=True)
        inputs, graph = overlay_graph(self.overlays, self.tmp, self.options.fps, SCALE)
        # a video with overlays ends with its last one, like the closing card
        end = ["-t", f"{max(item['end'] for item in self.overlays):.3f}"] if self.overlays else []
        # webp stores millisecond delays, so 60 fps is real here. "text" keeps glyph edges crisp,
        # and the animation loops forever like a gif
        codec = ["-c:v", "libwebp_anim", "-preset", "text", "-quality", "80",
                 "-pix_fmt", "yuv420p", "-loop", "0"]
        plain = ["-vf", f"scale={SCALE}"] if not graph else []
        subprocess.run(
            ["ffmpeg", "-loglevel", "error", "-y", "-i", str(self.raw), *inputs,
             *(["-filter_complex", graph, "-map", "[out]"] if graph else []), *end,
             "-r", str(self.options.fps), *plain, *codec, str(video)],
            check=True)
        print(f"{video}: {video.stat().st_size // 1024} KB")
        if self.poster_at is not None:
            poster = video.with_name(f"{video.stem}.png")
            subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-ss", f"{self.poster_at:.3f}", "-i", str(video),
                            "-frames:v", "1", str(poster)], check=True)
            print(f"{poster}: {poster.stat().st_size // 1024} KB")

    def now(self) -> float:
        return time.monotonic() - self.t0

    def configure(self, patch: dict, pause: float = 1.2) -> None:
        """merge `patch` into the settings and save them, kuterm applies them on its own"""
        def merge(base: dict, over: dict) -> None:
            for key, value in over.items():
                if isinstance(value, dict) and isinstance(base.get(key), dict):
                    merge(base[key], value)
                else:
                    base[key] = value
        merge(self.settings, patch)
        self.settings_path.write_text(json.dumps(self.settings, indent=2))
        time.sleep(pause)

    def say(self, title: str, body: str, wait: float = 2.0) -> None:
        """caption the next part next to the window until the next one, with time to read it
        before anything happens"""
        self.say_end()
        self.caption = {"title": title, "body": body, "start": self.now()}
        time.sleep(wait)

    def say_end(self) -> None:
        if self.caption is None:
            return
        # the result stays on screen a while with its caption
        time.sleep(1.5)
        caption, self.caption = self.caption, None
        step = sum(1 for item in self.overlays if item.get("step")) + 1
        png = self.tmp / f"caption-{step}.png"
        self.overlays.append({"step": step, "png": png, "start": caption["start"], "end": self.now(),
                              "slide": 16, "caption": caption})

    def card(self, title: str, body: str, seconds: float) -> None:
        """a title over the whole frame for `seconds`"""
        png = self.tmp / f"card-{len(self.overlays)}.png"
        title_png(png, self.size, title, body)
        self.overlays.append({"png": png, "start": self.now(), "end": self.now() + seconds, "x": 0, "y": 0})
        time.sleep(seconds)

    def render_captions(self) -> None:
        """draw the captions now that the number of steps is known, right of the window and
        centered on it"""
        from PIL import Image
        steps = [item for item in self.overlays if item.get("step")]
        wx, wy = self.window_at
        margin = 64
        x = wx + WINDOW[0] + margin
        for item in steps:
            caption = item["caption"]
            caption_png(item["png"], caption["title"], caption["body"], self.size[0] - x - margin)
            height = Image.open(item["png"]).size[1]
            item["x"] = x
            item["y"] = wy + (WINDOW[1] - height) // 2

    def step(self, label: str, pause: float = 0.8) -> None:
        """let what the last input did show for a moment"""
        time.sleep(pause)
        self.step_n += 1
        if self.stills:
            STILLS.mkdir(exist_ok=True)
            subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-f", "x11grab", "-video_size",
                            f"{self.size[0]}x{self.size[1]}", "-i", f"{DISPLAY}+{self.offset[0]},{self.offset[1]}",
                            "-frames:v", "1",
                            str(STILLS / f"{self.name}-{self.step_n:02d}-{label}.png")], env=ENV)

    def key(self, *keys: str, pause: float = 0.35) -> None:
        for combo in keys:
            sh("xdotool", "key", "--clearmodifiers", combo)
            time.sleep(pause * self.rng.uniform(0.85, 1.2))

    def type(self, text: str, speed: float = 1.0) -> None:
        """type like a person: uneven gaps between keys, longer ones around words and symbols,
        now and then a short hesitation"""
        for ch in text:
            sh("xdotool", "type", "--", ch)
            gap = min(max(self.rng.gauss(0.075, 0.025), 0.035), 0.18)
            if ch == " ":
                gap += self.rng.uniform(0.03, 0.1)
            elif not ch.isalnum():
                gap += self.rng.uniform(0.02, 0.06)
            if self.rng.random() < 0.04:
                gap += self.rng.uniform(0.15, 0.35)
            time.sleep(gap / speed)

    def run(self, command: str, pause: float = 0.9) -> None:
        self.type(command)
        time.sleep(self.rng.uniform(0.2, 0.4))
        self.key("Return", pause=pause)

    def pointer(self) -> tuple:
        here = dict(line.split("=") for line in sh("xdotool", "getmouselocation", "--shell").split())
        return int(here["X"]), int(here["Y"])

    def move(self, x: int, y: int, slow: float = 1.0) -> None:
        """move the pointer by hand to a point of the client area, so the video shows where it
        goes"""
        ox, oy = self.origin()
        for px, py in hand_path(self.pointer(), (ox + x, oy + y), self.rng, self.options.fps, slow):
            sh("xdotool", "mousemove", str(px), str(py))
            time.sleep(1 / self.options.fps)
        time.sleep(self.rng.uniform(0.1, 0.2))

    def click(self, x: int, y: int, button: int = 1, pause: float = 0.6) -> None:
        self.move(x, y)
        sh("xdotool", "click", str(button))
        time.sleep(pause)

    def drag(self, start: tuple, end: tuple, hover: float = 0.25) -> None:
        """press at start, carry to end more carefully than a plain move, wait there for `hover`
        so what the drop would do shows, and let go"""
        self.move(*start)
        sh("xdotool", "mousedown", "1")
        time.sleep(self.rng.uniform(0.12, 0.2))
        self.move(*end, slow=1.6)
        time.sleep(hover)
        sh("xdotool", "mouseup", "1")
        time.sleep(0.5)

    def wheel(self, x: int, y: int, up: bool, clicks: int = 3) -> None:
        self.move(x, y)
        for _ in range(clicks):
            sh("xdotool", "click", "4" if up else "5")
            time.sleep(self.rng.uniform(0.08, 0.16))

    def park(self) -> None:
        sh("xdotool", "mousemove", str(self.offset[0] + self.size[0] - 4), str(self.offset[1] + self.size[1] - 4))


def minimal(s: Scene) -> None:
    s.record()
    s.run("cd ~/api")
    s.run("ls")
    s.run("git log --oneline --graph --decorate", pause=1.2)
    s.run("git status -sb", pause=1.5)
    s.step("history")
    # a second tab brings the tab bar, closing it hides the bar again
    s.key("ctrl+shift+t", pause=1.2)
    s.run("cd ~/web && git log --oneline", pause=1.4)
    s.step("second tab")
    s.key("ctrl+shift+w", pause=1.5)
    s.step("back to one tab")


def tmux(s: Scene) -> None:
    s.run("cd ~/api && clear", pause=0.5)
    s.record()
    # a split moves the focused shell to the new side and keeps the focus on it
    s.key("ctrl+a", "v", pause=1.0)
    s.run("git status -sb")
    s.key("ctrl+a", "s", pause=1.0)
    s.run("ls")
    s.key("ctrl+a", "k", pause=0.8)
    s.run("git log --oneline --graph --all", pause=1.2)
    s.key("ctrl+a", "h", pause=0.8)
    s.run("nvim app.py", pause=1.5)
    s.step("three panes")
    s.key("ctrl+a", "l", pause=0.8)
    s.key("ctrl+a", "z", pause=1.5)
    s.step("zoomed")
    s.key("ctrl+a", "z", pause=1.0)
    s.key("ctrl+a", "c", pause=1.0)
    s.run("git branch -a", pause=1.2)
    s.step("second tab")
    s.key("ctrl+a", "p", pause=1.0)
    s.key("ctrl+a", "h", pause=1.2)
    s.step("back to vim")


def reading(s: Scene) -> None:
    s.run("cd ~/api && clear", pause=0.5)
    s.record()
    # a long log to show smooth scroll: the wheel steps glide instead of jumping
    s.run("cat ~/logs/api.log", pause=1.5)
    s.wheel(600, 400, up=True, clicks=10)
    time.sleep(1.0)
    s.step("smooth scroll up", 1.0)
    s.wheel(600, 400, up=False, clicks=10)
    time.sleep(1.0)
    s.step("smooth scroll down", 0.8)
    s.run("cat README.md", pause=1.0)
    s.drag(*READING_DRAG)
    s.step("selected and copied", 2.0)
    s.key("ctrl+shift+t", pause=1.0)
    s.run("cd ~/web && ls", pause=1.2)
    s.click(*READING_FIRST_TAB, pause=1.5)
    s.park()
    s.step("first tab")


# the "run with" line printed by "cat README.md", and the first tab, in client pixels
READING_DRAG = ((38, 668), (500, 668))
READING_FIRST_TAB = (320, 16)


def showcase(s: Scene) -> None:
    s.record()
    # the tab joins a group named after the git project its shell moved into
    s.run("cd ~/api", pause=1.5)
    s.run("git log --oneline --graph --decorate", pause=1.0)
    s.step("api group")
    s.key("ctrl+shift+r", pause=1.2)
    s.run("python3 -m http.server 8000", pause=2.0)
    s.step("server port")
    s.key("ctrl+alt+h", pause=0.8)
    s.key("ctrl+shift+t", pause=1.0)
    s.run("cd ~/web && git status -sb", pause=1.5)
    s.step("web group")
    # the left pane alone, so its tab bar has room for the tabs grouped by hand below
    s.key("ctrl+shift+z", pause=1.0)
    s.key("ctrl+shift+t", pause=0.8)
    s.run("cd ~/notes && ls", pause=0.8)
    s.key("ctrl+shift+t", pause=0.8)
    s.run("cd ~/logs && ls", pause=1.0)
    s.step("before the drag")
    # like chrome: a tab dropped onto another groups the two, an unnamed group is a dot
    s.drag(SHOWCASE_LOGS, SHOWCASE_NOTES, hover=0.8)
    s.step("new group", 1.0)
    s.click(*SHOWCASE_OPS_DOT, button=3, pause=0.8)
    s.step("group menu", 0.2)
    s.click(*SHOWCASE_MENU_RENAME, pause=0.6)
    s.type("ops")
    s.key("Return", pause=1.0)
    s.step("named group")
    s.key("ctrl+shift+t", pause=0.8)
    s.run("cd ~/scratch", pause=1.0)
    # dropped between the two tabs of the group, it joins it there
    s.drag(SHOWCASE_SCRATCH, SHOWCASE_OPS_SECOND_EDGE, hover=0.8)
    s.step("joined the group", 1.2)
    # a click on a group's chip folds its tabs away and back
    s.click(*SHOWCASE_WEB_CHIP, pause=1.2)
    s.step("web folded")
    s.click(*SHOWCASE_WEB_CHIP, pause=1.0)
    s.key("ctrl+shift+z", pause=1.0)
    s.click(*SHOWCASE_API_TAB, pause=1.0)
    s.park()
    s.run("git status -sb", pause=1.2)
    s.step("api tab")
    s.key("ctrl+shift+p", pause=0.8)
    s.type("theme")
    s.key("Return", pause=1.0)
    for _ in range(4):
        s.key("Down", pause=0.9)
    s.step("theme picker")
    s.key("Escape", pause=1.0)
    s.type("git ch", speed=0.7)
    time.sleep(1.2)
    s.key("Down", pause=0.6)
    s.step("completions")
    s.key("Tab", pause=1.2)
    s.key("ctrl+u", pause=0.8)


# where things are in the zoomed left pane's tab bar, in client pixels, found with --stills:
# the notes and logs tabs before grouping, the dot of their new group, the rename row of its
# menu, the scratch tab, the left edge of the logs tab once the group is named "ops"
SHOWCASE_BAR_Y = 26
SHOWCASE_NOTES = (610, SHOWCASE_BAR_Y)
SHOWCASE_LOGS = (810, SHOWCASE_BAR_Y)
SHOWCASE_OPS_DOT = (523, SHOWCASE_BAR_Y)
SHOWCASE_MENU_RENAME = (560, 60)
SHOWCASE_SCRATCH = (1060, SHOWCASE_BAR_Y)
SHOWCASE_OPS_SECOND_EDGE = (775, SHOWCASE_BAR_Y)
SHOWCASE_WEB_CHIP = (284, SHOWCASE_BAR_Y)
SHOWCASE_API_TAB = (159, SHOWCASE_BAR_Y)

# the readme starts from this: a dark theme, titles with icons and git, a split key
README_START = {"readme": {
    "hide_bar_for_one_tab": False,
    "tab_title": ["icon", "folder", {"text": " "}, "git"],
    "tab_width": 200,
    "window_title": ["folder"],
    "theme": {"mode": "dark", "dark": "catppuccin-mocha"},
    "window": {"decorations": "server", "menu_bar": "never", "padding": {"x": 14, "y": 10}},
    "terminal": {"cursor_shape": "block", "scrollbar": {"enable": "off"}},
    "autocomplete": {"enable": False},
    "icons": {"dynamic": True},
    # "config reloaded" says each save worked, without piling up
    "notifications": {"timeout": 2},
    "tab_groups": {"by_project": True},
    "keybindings": {"split_right": "ctrl-shift-r", "focus_pane_left": "ctrl-alt-h"},
}}

# tabs in the tab bar, found with --stills: the notes and logs tabs before they are grouped,
# and the api tab once it has its project chip
README_NOTES = (610, 18)
README_LOGS = (810, 18)
README_API_TAB = (155, 18)


def readme(s: Scene) -> None:
    s.run("cd ~/api && clear", pause=0.3)
    s.record()
    s.say("Tab groups", "tabs of one git project group up on their own. drag a tab onto another "
          "to group them by hand, like in chrome", wait=1.2)
    s.key("ctrl+shift+t", pause=0.4)
    s.run("cd ~/web", pause=0.4)
    s.key("ctrl+shift+t", pause=0.4)
    s.run("cd ~/notes", pause=0.3)
    s.key("ctrl+shift+t", pause=0.4)
    s.run("cd ~/logs", pause=0.5)
    s.step("before the drag")
    s.drag(README_LOGS, README_NOTES, hover=0.6)
    s.step("grouped", 0.8)
    s.say("Splits and live metrics", "split a pane with a key, or drop a tab on a pane's edge. "
          "the status bar follows cpu, memory, traffic and listening ports", wait=1.2)
    s.configure({"panes": {"dim_inactive": 0.3}, "status_bar": {"enable": True}}, pause=0.4)
    s.click(*README_API_TAB, pause=0.5)
    s.park()
    s.key("ctrl+shift+r", pause=0.4)
    s.run("python3 -m http.server 8000", pause=1.2)
    s.step("split")
    s.say("Completions", "commands, options and paths with what they do, from bundled specs. "
          "nothing is run to find them", wait=1.2)
    s.configure({"autocomplete": {"enable": True}}, pause=0.3)
    s.key("ctrl+alt+h", pause=0.3)
    # tabs opened after the change pick it up
    s.key("ctrl+shift+t", pause=0.4)
    s.type("git ch", speed=0.7)
    time.sleep(0.8)
    s.key("Down", pause=0.4)
    s.key("Down", pause=0.4)
    s.step("completions", 0.4)
    s.key("Tab", pause=0.6)
    s.key("ctrl+u", pause=0.3)
    s.say("Themes", "pick a theme in the command palette and the whole window follows as you "
          "move", wait=1.2)
    s.key("ctrl+shift+p", pause=0.6)
    s.type("theme")
    s.key("Return", pause=0.6)
    for _ in range(3):
        s.key("Down", pause=0.6)
    s.step("themes")
    s.key("Escape", pause=0.6)
    s.say_end()


def agents(s: Scene) -> None:
    s.run("cd ~/api && clear", pause=0.3)
    s.record()
    s.run("claude", pause=0.2)
    # stay on the tab long enough for the title refresh to see the agent busy
    time.sleep(2.0)
    s.key("ctrl+shift+t", pause=0.5)
    s.run("cd ~/web && git status -sb", pause=0.5)
    # it went idle without finishing, so its tab is marked and a notification shows
    time.sleep(1.2)
    s.step("agent waiting", 1.2)
    s.key("ctrl+shift+j", pause=0.8)
    s.type("yes")
    s.key("Return", pause=1.2)
    s.step("answered")


def glass(s: Scene) -> None:
    s.run("cd ~/api && clear", pause=0.4)
    s.record()
    s.run("git log --oneline --graph --decorate", pause=1.4)
    s.wheel(600, 400, up=True, clicks=3)
    s.step("history", 1.0)
    s.run("ls", pause=0.8)
    s.key("ctrl+shift+t", pause=0.8)
    s.run("cd ~/web && ls", pause=1.0)
    s.step("second tab")


def compact(s: Scene) -> None:
    s.run("cd ~/api && clear", pause=0.3)
    s.record()
    s.run("ls -la", pause=0.7)
    s.run("git log --oneline --graph --all", pause=1.0)
    s.step("history")
    s.key("ctrl+shift+r", pause=0.5)
    s.run("git status -sb", pause=0.8)
    s.step("split")
    s.key("ctrl+alt+h", pause=0.4)
    s.key("ctrl+shift+t", pause=0.5)
    s.run("cd ~/web && ls", pause=0.8)
    s.type("git ch", speed=0.7)
    time.sleep(1.0)
    s.key("Down", pause=0.5)
    s.step("completions")


def dev(s: Scene) -> None:
    s.run("cd ~/api && clear", pause=0.3)
    s.record()
    s.key("ctrl+shift+r", pause=0.5)
    s.run("python3 -m http.server 8000", pause=1.6)
    s.step("server port")
    s.key("ctrl+alt+h", pause=0.4)
    s.run("git log --oneline --graph --decorate", pause=1.0)
    s.key("ctrl+shift+d", pause=0.5)
    s.run("git status -sb", pause=0.8)
    s.step("three panes")
    s.key("ctrl+shift+p", pause=0.7)
    s.type("checkout")
    s.step("palette")
    s.key("Escape", pause=0.6)
    s.type("git ch", speed=0.7)
    time.sleep(1.0)
    s.key("Down", pause=0.5)
    s.step("completions", 0.8)


SCENES = {"minimal": minimal, "tmux": tmux, "reading": reading, "showcase": showcase,
          "agents": agents, "glass": glass, "compact": compact, "dev": dev, "readme": readme}


def main() -> None:
    parser = argparse.ArgumentParser(description="record preset demos and the readme showcase")
    parser.add_argument("--fps", type=int, default=60)
    parser.add_argument("--stills", action="store_true")
    parser.add_argument("presets", nargs="*", help=f"some of {', '.join(SCENES)}, all by default")
    options = parser.parse_args()
    # the glass demo comes from a live compositor: x11 does not show the window opacity, so
    # it is only recorded when named
    names = options.presets or [name for name in SCENES if name != "glass"]
    for name in names:
        if name not in SCENES:
            parser.error(f"no scene for {name!r}")
    desktop = Desktop()
    try:
        for name in names:
            scene = Scene(name, options)
            try:
                SCENES[name](scene)
                scene.finish()
            finally:
                scene.proc.terminate()
                if scene.ffmpeg and scene.ffmpeg.poll() is None:
                    scene.ffmpeg.send_signal(signal.SIGINT)
    finally:
        desktop.stop()


if __name__ == "__main__":
    main()
