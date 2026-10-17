#!/usr/bin/env python3
"""record examples/presets/<preset>/demo.mp4: kuterm with the preset's settings on a virtual x
display with a window manager, driven by xdotool like a person would, filmed with ffmpeg at the
screen's own resolution. "tour" records assets/tour/tour.mp4 for the readme instead: settings
grown from bare to most features while it runs, with captions laid over by
ffmpeg. runs inside the image of tools/presets/Containerfile, see tools/presets/record.sh.

usage: record.py [--fps 60] [--crf 16] [--stills] [preset or tour ...]
--crf is the h.264 quality, lower is sharper and bigger. --stills also saves a png at every
step() into /tmp/stills, to find where to click"""

import argparse
import json
import math
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
BIN = Path(os.environ.get("KUTERM_BIN", SRC / "target/podman/debug/kuterm"))
DISPLAY = ":99"
DESKTOP = (1920, 1080)
# part of the desktop filmed, as size and offset, and the left (none to center it) and top of
# the window's client area in it. the tour leaves room right of the window for its captions
REGION = ((1440, 900), (240, 90), None, 84)
REGIONS = {"tour": ((1920, 1080), (0, 0), 72, 160)}
# client area of the window, the frame of the window manager comes around it
WINDOW = (1280, 760)
# videos that are not a preset's demo
OUTPUTS = {"tour": SRC / "assets/tour/tour.mp4"}
STILLS = Path("/tmp/stills")

ZSHRC = """PS1='%F{green}dev@kuterm%f:%F{blue}%~%f$ '
unsetopt prompt_sp
alias ls='ls --color=auto'
# output goes into the terminal history, not into less, so scrolling shows it
export GIT_PAGER=cat
"""


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


def projects(home: Path) -> None:
    """~/api with a merged branch and a changed file, ~/web with two commits on a dev branch"""
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
    (api / "README.md").write_text("# api\n\nrun with `flask --app app run`\n")
    web = home / "web"
    web.mkdir()
    git(web, "init", "-q", "-b", "main")
    commit(web, "initial site", {"index.html": "<h1>kuterm</h1>\n", "package.json": '{ "name": "web" }\n'})
    commit(web, "add styles", {"style.css": "h1 { color: #89b4fa; }\n"})
    git(web, "checkout", "-q", "-b", "dev")
    # outside any project, so tab_groups.by_project leaves their tabs for groups made by hand
    for name, files in {"notes": ["todo.md", "ideas.md"], "logs": ["api.log", "web.log"],
                        "scratch": ["draft.txt"]}.items():
        (home / name).mkdir()
        for file in files:
            (home / name / file).write_text("")


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
# desktop wallpaper, a gradient through these colors; the tour's are the logo's slates
WALLPAPER = ["#313244", "#585b70", "#1e1e2e"]
WALLPAPERS = {"tour": ["#2a303b", "#444d5f", "#2f3542"]}
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


def overlay_graph(overlays: list, tmp: Path, fps: int) -> tuple:
    """ffmpeg inputs and filter graph laying `overlays` over the recording, each fading in and
    out and sliding up into place as it comes"""
    inputs, chain, last = [], [], "0:v"
    for ix, item in enumerate(overlays, start=1):
        start, end = item["start"], max(item["end"], item["start"] + 2 * FADE)
        duration = end - start
        inputs += ["-loop", "1", "-framerate", str(fps), "-t", f"{duration:.3f}", "-i", str(item["png"])]
        chain.append(f"[{ix}:v]format=rgba,fade=t=in:st=0:d={FADE}:alpha=1,"
                     f"fade=t=out:st={duration - FADE:.3f}:d={FADE}:alpha=1,"
                     f"setpts=PTS-STARTPTS+{start:.3f}/TB[o{ix}]")
        slide = f"{item['y']}+{item.get('slide', 0)}*pow(max(0,1-(t-{start:.3f})/{FADE}),2)"
        label = "out" if ix == len(overlays) else f"v{ix}"
        chain.append(f"[{last}][o{ix}]overlay=x={item['x']}:y='{slide}':eof_action=pass:"
                     f"enable='between(t,{start:.3f},{end:.3f})'[{label}]")
        last = label
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
        if name in TOUR_START:
            self.settings = json.loads(json.dumps(TOUR_START[name]))
            self.settings_path.write_text(json.dumps(self.settings, indent=2))
        else:
            shutil.copy(PRESETS / name / "settings.jsonc", self.settings_path)
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
        server = '"decorations": "server"' in self.settings_path.read_text()
        for _ in range(5):
            self.proc = subprocess.Popen([str(BIN)], cwd=self.home, env=env, stdout=self.log, stderr=subprocess.STDOUT)
            self.wid = wait(self._window, "the kuterm window", 60)
            # gpui falls back to a frame of the window manager while the compositor is not up
            # yet, as it is right after xfwm4 started
            if server or self._decorations() == "0x0":
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
        video = OUTPUTS.get(self.name, PRESETS / self.name / "demo.mp4")
        video.parent.mkdir(parents=True, exist_ok=True)
        inputs, graph = overlay_graph(self.overlays, self.tmp, self.options.fps)
        # a video with overlays ends with its last one, like the closing card
        end = ["-t", f"{max(item['end'] for item in self.overlays):.3f}"] if self.overlays else []
        # yuv420p and faststart so browsers and github play it, "animation" keeps flat
        # colors and text edges crisp
        subprocess.run(
            ["ffmpeg", "-loglevel", "error", "-y", "-i", str(self.raw), *inputs,
             *(["-filter_complex", graph, "-map", "[out]"] if graph else []), *end, "-r", str(self.options.fps),
             "-c:v", "libx264", "-preset", "slow", "-tune", "animation", "-crf", str(self.options.crf),
             "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(video)],
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

    def say(self, title: str, body: str) -> None:
        """caption the next part next to the window until the next one, with time to read it
        before anything happens"""
        self.say_end()
        self.caption = {"title": title, "body": body, "start": self.now()}
        time.sleep(2.0)

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
    s.run("git log --stat -3", pause=1.5)
    s.wheel(600, 400, up=True, clicks=4)
    s.step("scrolled up", 1.2)
    s.wheel(600, 400, up=False, clicks=4)
    s.step("scrolled down", 1.0)
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
    s.key("ctrl+shift+o", pause=1.5)
    s.key("Right", pause=0.6)
    s.key("Right", pause=0.6)
    s.step("overview")
    s.key("Escape", pause=1.0)
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

# the tour starts from this: only the shell, everything else off or out of the way
TOUR_START = {"tour": {
    "hide_bar_for_one_tab": True,
    "tab_title": ["folder"],
    "tab_width": 200,
    "window_title": ["folder"],
    "theme": {"mode": "dark", "dark": "dark"},
    "window": {"decorations": "server", "menu_bar": "never", "padding": {"x": 14, "y": 10}},
    "terminal": {"cursor_shape": "block", "scrollbar": {"enable": "off"}},
    "shell_integration": {"mark_sources": "none"},
    "autocomplete": {"enable": False},
    "icons": {"dynamic": False},
    # "config reloaded" says each save worked, without piling up
    "notifications": {"timeout": 2},
    "keybindings": {"split_right": "ctrl-shift-r", "focus_pane_left": "ctrl-alt-h",
                    "tab_overview": "ctrl-shift-o"},
}}

# tabs and chips in the tab bar, found with --stills: the notes and logs tabs before they are
# grouped, and the api tab once they are
TOUR_NOTES = (610, 18)
TOUR_LOGS = (810, 18)
TOUR_API_TAB = (155, 18)


def tour(s: Scene) -> None:
    s.run("cd ~/api && clear", pause=0.3)
    s.record()
    s.say("Just a terminal", "with almost nothing turned on kuterm is a shell in a window. "
          "every part added next is a line or two in settings.jsonc")
    s.run("ls")
    s.run("git log --oneline --graph --decorate", pause=2.0)
    s.say("Settings apply as you save", "settings.jsonc is watched: change the theme, the font or "
          "the padding and the window follows, no restart")
    s.configure({"theme": {"dark": "catppuccin-mocha"}}, pause=1.6)
    s.configure({"terminal": {"font": {"size": 18}}, "window": {"padding": {"x": 20, "y": 14}}}, pause=2.0)
    s.say("Tabs", "a second tab brings the tab bar. titles are built from blocks: icon, folder, "
          "git branch, ports, cpu, memory and more")
    s.configure({"tab_title": ["icon", "folder", {"text": " "}, "git"], "icons": {"dynamic": True}}, pause=0.4)
    s.key("ctrl+shift+t", pause=0.6)
    s.run("cd ~/web", pause=1.5)
    s.say("Tab groups", "tabs of one git project group up on their own. drag a tab onto another "
          "to group them by hand, like in chrome")
    s.configure({"tab_groups": {"by_project": True}}, pause=1.0)
    time.sleep(1.5)
    s.key("ctrl+shift+t", pause=0.6)
    s.run("cd ~/notes", pause=0.6)
    s.key("ctrl+shift+t", pause=0.6)
    s.run("cd ~/logs", pause=1.0)
    s.step("before the drag")
    s.drag(TOUR_LOGS, TOUR_NOTES, hover=0.8)
    s.step("grouped", 1.5)
    s.say("Splits", "split a pane with a key, or drop a tab on a pane's edge. panes without "
          "focus are dimmed")
    s.configure({"panes": {"dim_inactive": 0.3}}, pause=0.3)
    s.click(*TOUR_API_TAB, pause=0.8)
    s.park()
    s.key("ctrl+shift+r", pause=0.6)
    s.run("python3 -m http.server 8000", pause=2.0)
    s.step("split")
    s.say("Status bar", "git, a cpu graph, memory, traffic and listening ports of the active tab, "
          "and of all tabs together")
    s.configure({"status_bar": {"enable": True}}, pause=0.6)
    time.sleep(2.6)
    s.say("Completions", "commands, subcommands, options and paths with what they do, from bundled "
          "specs. nothing is run to find them")
    s.configure({"autocomplete": {"enable": True}}, pause=0.3)
    s.key("ctrl+alt+h", pause=0.5)
    # tabs opened after the change get it
    s.key("ctrl+shift+t", pause=0.8)
    s.type("git ch", speed=0.7)
    time.sleep(1.2)
    s.key("Down", pause=0.6)
    s.key("Down", pause=0.6)
    # the readme shows this frame: groups, a split, the status bar, the popup and a caption
    s.poster_at = s.now()
    s.step("completions", 0.6)
    s.key("Tab", pause=1.0)
    s.key("ctrl+u", pause=0.6)
    s.say("Command palette", "every action is in the palette, next to commands of your own. "
          "themes are previewed as you move through them")
    s.key("ctrl+shift+p", pause=0.8)
    s.type("theme")
    s.key("Return", pause=1.0)
    for _ in range(4):
        s.key("Down", pause=0.9)
    s.step("themes")
    s.key("Escape", pause=1.0)
    s.say("Overview", "a live grid of every tab in every pane. type to filter, enter to jump")
    s.key("ctrl+shift+o", pause=1.2)
    s.type("web")
    time.sleep(1.2)
    s.step("overview")
    s.key("Return", pause=1.5)
    s.say_end()
    s.card("kuterm", "github.com/roworu/kuterm\nready presets in examples/presets", 4.0)


SCENES = {"minimal": minimal, "tmux": tmux, "reading": reading, "showcase": showcase, "tour": tour}


def main() -> None:
    parser = argparse.ArgumentParser(description="record examples/presets/<preset>/demo.mp4")
    parser.add_argument("--fps", type=int, default=60)
    parser.add_argument("--crf", type=int, default=16)
    parser.add_argument("--stills", action="store_true")
    parser.add_argument("presets", nargs="*", help=f"some of {', '.join(SCENES)}, all by default")
    options = parser.parse_args()
    names = options.presets or list(SCENES)
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
