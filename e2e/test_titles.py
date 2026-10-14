"""window title (read back from x11) and tab titles built from title blocks"""

import socket

import pytest

from harness import App, as_rgb, ink, mask_bbox, sh, unique_theme, session_env

FEATURE = "window and tab titles"


def titled(app_factory, blocks, **kwargs) -> App:
    return app_factory({"theme": {"mode": "dark"}, "window_title": blocks, **kwargs.pop("settings", {})}, **kwargs)


def test_number_block(app_factory):
    """"number" is the position of the active tab"""
    app = titled(app_factory, ["number"])
    app.wait_title("1")
    app.key("ctrl+shift+t")
    app.wait_title("2")


def test_prompt_block_is_user_at_host(app_factory):
    """"prompt" is user@host"""
    app = titled(app_factory, ["prompt"], env={"USER": "tester"})
    app.wait_title(f"tester@{socket.gethostname()}")


def test_folder_block_follows_cd(app_factory):
    """"folder" follows the shell into other folders"""
    app = titled(app_factory, ["folder"])
    (app.home / "project-x").mkdir()
    app.run(f"cd {app.home / 'project-x'}")
    app.wait_title("project-x")


def test_command_block_follows_foreground_program(app_factory):
    """"command" is the running program, back to the shell when it exits"""
    app = titled(app_factory, ["command"])
    app.wait_title("bash")
    app.run("sleep 30")
    app.wait_title("sleep")
    app.snap("sleep running")
    app.key("ctrl+c")
    app.wait_title("bash")


@pytest.mark.parametrize("osc", ["0", "2"])
def test_title_block_uses_program_title(app_factory, osc):
    """"title" is what the program set with an osc sequence"""
    app = titled(app_factory, ["title"], script=f"printf '\\033]{osc};my program title\\007'")
    app.wait_title("my program title")


def test_text_block(app_factory):
    """text blocks are copied as they are"""
    app = titled(app_factory, [{"text": "< kuterm e2e >"}])
    app.wait_title("< kuterm e2e >")


def test_exec_block_runs_in_shell_folder(app_factory):
    """exec blocks run in the tab's folder and show the first output line"""
    app = titled(app_factory, [{"exec": "basename \"$PWD\"; echo second line"}])
    (app.home / "exec-dir").mkdir()
    app.run(f"cd {app.home / 'exec-dir'}")
    app.wait_title("exec-dir")


@pytest.mark.parametrize("command", ["exit 3", "sleep 10"])
def test_failing_or_hung_exec_block_is_empty(app_factory, command):
    """a failing or hung exec block adds nothing"""
    app = titled(app_factory, [{"text": "a"}, {"exec": command}, {"text": "b"}])
    app.wait_title("ab", timeout=15)


def test_blocks_joined_in_order(app_factory):
    """all blocks together, in the order given"""
    app = titled(app_factory, ["number", {"text": ": "}, "folder", {"text": " > "}, "command"])
    app.wait_title("1: ~ > bash")


def test_icon_block_is_the_active_tab_icon(app_factory):
    """"icon" in the window title is the icon of the active tab, following its program"""
    app = titled(app_factory, ["icon", {"text": " "}, "command"], settings={"icons": {"default": "D"}},
                 tab_icons={"groups": [{"icon": "S", "commands": ["sleep"]}]})
    app.wait_title("D bash")
    app.run("sleep 30")
    app.wait_title("S sleep")
    app.key("ctrl+shift+t")
    app.wait_title("D bash")


def test_progress_block_follows_osc_9_4(app_factory):
    """"progress" fills a circle with the osc 9;4 progress and is gone once taken down"""
    app = titled(app_factory, [{"text": "["}, "progress", {"text": "]"}])
    app.wait_title("[]")
    app.run("printf '\\033]9;4;1;50\\007'")
    app.wait_title("[\U000f0aa1]")
    app.snap("half done")
    app.run("printf '\\033]9;4;0\\007'")
    app.wait_title("[]")


def test_busy_progress_block_spins(app_factory):
    """"progress" without a percent turns a spinner until it is taken down"""
    app = titled(app_factory, [{"text": "["}, "progress", {"text": "]"}])
    app.wait_title("[]")
    app.run("printf '\\033]9;4;3\\007'")
    spinner = [f"[{chr(c)}]" for c in range(0xEE06, 0xEE0C)]
    app.wait(lambda: app.title() in spinner, msg=f"spinner, title {app.title()!r}")
    seen = {app.title()}
    app.wait(lambda: seen.add(app.title()) or len(seen & set(spinner)) > 2, msg=f"spinner turns, saw {seen}")
    app.snap("busy")
    app.run("printf '\\033]9;4;0\\007'")
    app.wait_title("[]")


@pytest.mark.parametrize("blocks", [[], [{"text": ""}], ["title"]], ids=["none", "empty-text", "no-title"])
def test_empty_window_title_uses_default_title(app_factory, blocks):
    """an empty window title falls back to default_title"""
    app = titled(app_factory, blocks, settings={"default_title": "fallback title"})
    app.wait_title("fallback title")


def test_default_title_setting(app_factory):
    """default_title is the window title before the first refresh and stands in for "title\""""
    app = titled(app_factory, [{"text": "["}, "title", {"text": "]"}], settings={"default_title": "dflt"})
    app.wait_title("[dflt]")


def test_window_title_follows_active_tab(app_factory):
    """the window title shows the active tab"""
    app = titled(app_factory, ["number", {"text": " "}, "folder"])
    (app.home / "one").mkdir()
    app.run(f"cd {app.home / 'one'}")
    app.wait_title("1 one")
    app.key("ctrl+shift+t")
    app.wait_title("2 ~")
    app.key("alt+1")
    app.wait_title("1 one")


@pytest.mark.x11_only
def test_window_class_is_app_id(app_factory):
    """the x11 window class is kuterm, for docks and window rules"""
    app = titled(app_factory, ["number"])
    out = sh("xprop", "-id", app.wid, "WM_CLASS", env=session_env())
    assert '"kuterm"' in out, out


def test_tab_title_blocks_are_drawn(app_factory):
    """tab_title blocks are drawn in the tab, separate from the window title"""
    theme = unique_theme(0x70)
    colors = as_rgb(theme)
    app = titled(app_factory, [{"text": "window"}], themes={"dark": theme},
                 settings={"hide_bar_for_one_tab": False,
                           "tab_title": [{"text": "WWWWWWWWWW"}]})
    app.wait_title("window")
    h = app.bar_height()

    def title_width() -> int:
        box = mask_bbox(ink(app.shot()[1 : h - 2, :200], colors["tab_active_background"]))
        return 0 if box is None else box[2] - box[0]

    # ten wide letters at 0.875 of the ui font are well over 80px
    app.wait(lambda: title_width() > 80, msg=f"tab title drawn, width {title_width()}")
