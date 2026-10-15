"""ai agents waiting for input: an agent that stops being busy without finishing marks its tab,
and the next attention key switches to it"""

import os

from harness import App, as_rgb, count, unique_theme

FEATURE = "agents waiting"

THEME = unique_theme(0x6a)
C = as_rgb(THEME)

# busy, then idle at a prompt once the test lets it go on, like an agent asking to run a tool
AGENT = """#!/usr/bin/python3
import os, sys, time
step = os.path.join(os.environ["T2_TMP"], "step1")
sys.stdout.write("\\033]9;4;3\\007working\\n")
sys.stdout.flush()
while not os.path.exists(step):
    time.sleep(0.02)
sys.stdout.write("\\033]9;4;0\\007allow? ")
sys.stdout.flush()
input()
"""


def marked(app: App) -> int:
    """attention colored pixels in the tab bar"""
    return count(app.shot()[: app.bar_height()], C["attention"], 3)


def test_waiting_agent_marks_its_tab_and_the_key_goes_there(app_factory):
    app = app_factory({"theme": {"mode": "dark"}, "hide_bar_for_one_tab": False, "window_title": ["icon"],
                       "icons": {"default": "D"}, "notifications": {"agent_waiting": True}},
                      keybindings={"next_attention_tab": "ctrl-shift-a"},
                      tab_icons={"interpreters": ["python3"],
                                 "groups": [{"icon": "A", "agents": True, "commands": ["claude"]}]},
                      themes={"dark": THEME})
    # a script named like an ai cli, run by its interpreter like node runs the real ones
    script = app.file("agent.py")
    script.write_text(AGENT)
    script.chmod(0o755)
    os.makedirs(app.file("bin"))
    os.symlink(script, app.file("bin/claude"))
    app.run("$T2_TMP/bin/claude")
    # the title refresh saw the agent working
    app.wait_title("A")
    app.key("ctrl+shift+t")
    app.wait_title("D")
    assert marked(app) == 0

    app.step(1)
    app.wait(lambda: marked(app) > 4, msg="tab of the waiting agent marked")
    app.snap("the agent in tab 1 waits")
    app.key("ctrl+shift+a")
    app.wait_title("A")

    # typing an answer ends the wait
    app.run("y")
    app.wait(lambda: marked(app) == 0, msg="mark gone after input")
