"""example presets in examples/presets: every settings file loads without a complaint, and a
scene with real projects shows it off. the screenshot of each scene is also saved as
artifacts/presets-<backend>/<preset>.png, copied over examples/presets/<preset>/screenshot.png"""

import os
import subprocess
from pathlib import Path

from PIL import Image

from harness import ARTIFACTS, BACKEND, SRC, App, jsonc
from kit import listening, ready, send, tabs, wait_block, wait_text

FEATURE = "example presets"

PRESETS = SRC / "examples/presets"
SHOTS = ARTIFACTS / f"presets-{BACKEND}"
# presets with a test_<name> scene below
SCENES = ["minimal", "tmux", "reading", "showcase"]

# a colored prompt and ls, like most people have, so the screenshots look like a real shell
ZSHRC = """PS1='%F{green}dev@kuterm%f:%F{blue}%~%f$ '
unsetopt prompt_sp
alias ls='ls --color=auto'
# clear without erasing the history, which would take the command marks the scene waits on
clear() { printf '\\e[H\\e[2J' }
"""

# stderr lines the settings parser prints for a value it could not use as written
COMPLAINTS = ("invalid", "is below", "is above", "skipped", "using the default")


def git(folder: Path, *args: str) -> None:
    env = {**os.environ, "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_AUTHOR_NAME": "dev", "GIT_AUTHOR_EMAIL": "dev@kuterm",
           "GIT_COMMITTER_NAME": "dev", "GIT_COMMITTER_EMAIL": "dev@kuterm"}
    subprocess.run(["git", *args], cwd=folder, env=env, check=True, capture_output=True)


def commit(folder: Path, message: str, files: dict) -> None:
    for name, text in files.items():
        (folder / name).write_text(text)
    git(folder, "add", ".")
    git(folder, "commit", "-q", "-m", message)


def projects(home: Path) -> None:
    """~/api with a merged branch and a changed file, ~/web with two commits"""
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


def preset_app(app_factory, tmp_path: Path, name: str) -> App:
    """the preset's settings with its "system" profiles running zsh in a home with two git
    projects, remote control on so the scene can type and wait for output, 1280x800"""
    settings = jsonc(PRESETS / name / "settings.jsonc")
    zdotdir = tmp_path / "zdotdir"
    zdotdir.mkdir()
    (zdotdir / ".zshrc").write_text(ZSHRC)
    shell = {"program": "/usr/bin/zsh"}
    profiles = settings.get("profiles", [{"name": "shell", "command": "system"}])
    settings["profiles"] = [{**p, "command": shell, "env": {**p.get("env", {}), "ZDOTDIR": str(zdotdir)}}
                            if p["command"] == "system" else p for p in profiles]
    settings["remote_control"] = "socket"
    app = app_factory(settings, start=False)
    projects(app.home)
    (app.home / ".local/state/kuterm/window_size.json").write_text('{"width": 1280, "height": 800}')
    app.start()
    listening(app)
    out = app.output()
    assert not [line for line in out.splitlines() if any(c in line for c in COMPLAINTS)], out
    return app


def run(app: App, line: str, tab: str) -> None:
    """type a command line into the focused pane, whose active tab is `tab`, and wait for it to
    finish. typed, so the source marks are the ones of a person"""
    app.type(line)
    app.key("Return")
    wait_block(app, line, tab)


def new_tab(app: App, *keys: str) -> str:
    """press keys that open a tab or a split, wait for its shell and return its id"""
    before = {t["id"] for t in tabs(app)}
    app.key(*keys)
    tab = app.wait(lambda: next((str(t["id"]) for t in tabs(app) if t["id"] not in before), None), msg="new tab")
    # sent to the tab itself, a split keeps the focus on the tab it moved
    send(app, "clear", tab)
    wait_block(app, "clear", tab)
    return tab


def save(app: App, name: str, caption: str) -> None:
    """screenshot for the report and for the preset's folder"""
    app.snap(caption)
    SHOTS.mkdir(parents=True, exist_ok=True)
    Image.fromarray(app.shot()).save(SHOTS / f"{name}.png", optimize=True)


def first_tab(app: App) -> str:
    ready(app)
    tab = str(tabs(app)[0]["id"])
    run(app, "clear", tab)
    return tab


def test_presets_are_all_tested():
    """every preset folder has a scene below"""
    assert sorted(p.name for p in PRESETS.iterdir() if p.is_dir()) == sorted(SCENES)


def test_minimal(app_factory, tmp_path):
    """one tab, no bars: just the shell"""
    app = preset_app(app_factory, tmp_path, "minimal")
    tab = first_tab(app)
    run(app, "cd ~/api && clear", tab)
    for line in ["ls", "git log --oneline --graph --decorate", "git status -sb"]:
        run(app, line, tab)
    save(app, "minimal", "minimal: one shell, nothing around it")


def test_tmux(app_factory, tmp_path):
    """three panes made with the ctrl-a prefix: an editor, git history and a listing"""
    app = preset_app(app_factory, tmp_path, "tmux")
    first = first_tab(app)
    run(app, "cd ~/api && clear", first)
    # a split moves the focused tab to the new side and opens a shell where it was
    left = new_tab(app, "ctrl+a", "v")
    top = new_tab(app, "ctrl+a", "s")
    new_tab(app, "ctrl+a", "c")
    app.key("ctrl+a", "p")
    app.wait(lambda: next(t["active"] for t in tabs(app) if str(t["id"]) == first), msg="first tab back")
    for line in ["ls", "git status -sb"]:
        run(app, line, first)
    app.key("ctrl+a", "k")
    run(app, "git log --oneline --graph --all", top)
    app.key("ctrl+a", "h")
    app.type("nvim app.py")
    app.key("Return")
    wait_text(app, "def health", left)
    save(app, "tmux", "tmux: ctrl-a v and ctrl-a s splits, ctrl-a c a second tab, the panes without focus dimmed")


def test_reading(app_factory, tmp_path):
    """big light text with tall lines and wide margins"""
    app = preset_app(app_factory, tmp_path, "reading")
    tab = first_tab(app)
    run(app, "cd ~/api && clear", tab)
    run(app, "git log --stat -2", tab)
    web = new_tab(app, "ctrl+shift+t")
    run(app, "cd ~/web", web)
    app.key("alt+1")
    app.wait(lambda: next(t["active"] for t in tabs(app) if str(t["id"]) == tab), msg="first tab")
    save(app, "reading", "reading: solarized light, comfortable lines, centered tab titles")


def test_showcase(app_factory, tmp_path):
    """project groups, a server's port in titles and the status bar, a split and the
    completion popup"""
    app = preset_app(app_factory, tmp_path, "showcase")
    server = first_tab(app)
    run(app, "cd ~/api && clear", server)
    # the split takes the first tab to the right and keeps it focused
    api = new_tab(app, "ctrl+shift+r")
    app.type("python3 -m http.server 8000")
    app.key("Return")
    wait_text(app, "Serving HTTP", server)
    app.key("ctrl+alt+h")
    run(app, "git log --oneline --graph --decorate", api)
    web = new_tab(app, "ctrl+shift+t")
    run(app, "cd ~/web && clear && git status", web)
    docs = new_tab(app, "ctrl+shift+t")
    run(app, "cd ~/web && clear && ls", docs)
    app.key("alt+1")
    app.wait(lambda: next(t["active"] for t in tabs(app) if str(t["id"]) == api), msg="first tab")
    # the popup opens over the rows under the prompt, a lot more than the typed word and cursor
    x0, y0, x1, y1 = app.cell_rect(0, 0, cols=40, rows=20, top=app.top(tabs=2))
    before = app.shot()[y0:y1, x0:x1]
    app.type("git ch")
    app.wait(lambda: (app.shot()[y0:y1, x0:x1] != before).any(axis=2).sum() > 8000, msg="completion popup")
    save(app, "showcase", "showcase: project groups, ports, status bar, a split and completions")

