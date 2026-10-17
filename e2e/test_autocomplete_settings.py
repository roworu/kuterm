"""the autocomplete settings and their combinations: "enable", "trigger", "max_items" and
"show_descriptions", the completion keybindings and keymap "when": "completion", spec files in
the "completions" folder next to settings.jsonc, and what they do with tabs, splits, pastes and
a config reload"""

import time

import numpy as np
import pytest

from harness import (App, as_rgb, dominant, get_clipboard, ink, ink_bbox, mask_bbox, near, set_clipboard,
                     unique_theme)
from kit import nothing_for

FEATURE = "autocomplete settings"

# every ui color unique, so the popup and its selected row can be told apart in a screenshot
THEME = unique_theme(0x71)
C = as_rgb(THEME)

# what a spec file adds: a command with two subcommands
MYTOOL = {"name": "mytool", "description": "a tool of mine",
          "subcommands": [{"name": "deploy", "description": "ship it to production"},
                          {"name": "rollback", "description": "go back one version"}]}
# replaces the bundled "ls" spec: one option of its own, none of the bundled ones
MY_LS = {"name": "ls", "description": "my own ls",
         "options": [{"name": "--mine-only", "description": "only from my spec"}]}


def zsh_app(app_factory, tmp_path, autocomplete=None, *, sections=None, files=None, keybindings=None) -> App:
    """an app whose only profile is zsh with kuterm's shell integration, in a folder holding
    "alpine/" and "alpha_file.txt" to complete. autocomplete is merged over the defaults"""
    zdotdir = tmp_path / "zdotdir"
    zdotdir.mkdir(exist_ok=True)
    # the wizard stays away, compinit gives the shell completions of its own, the file says
    # when the prompt is up (zsh reports the command line over the bridge before it)
    (zdotdir / ".zshrc").write_text(
        "PS1='$ '\n"
        "unsetopt prompt_sp\n"
        'precmd() { : > "$T2_TMP/prompt" }\n'
        "autoload -Uz compinit; compinit -u 2>/dev/null\n")
    work = tmp_path / "work"
    (work / "alpine").mkdir(parents=True, exist_ok=True)
    work.joinpath("alpha_file.txt").write_text("")
    profile = {"name": "zsh", "command": {"program": "/usr/bin/zsh"},
               "working_directory": str(work), "env": {"ZDOTDIR": str(zdotdir)}}
    settings = {"theme": {"mode": "dark"}, "window_title": ["number"], "profiles": [profile],
                "autocomplete": {"enable": True, **(autocomplete or {})}}
    settings.update(sections or {})
    app = app_factory(settings, files=files, keybindings=keybindings, themes={"dark": THEME})
    app.wait_file("prompt")
    return app


def shot(app: App, top: int = 0) -> np.ndarray:
    """the screenshot, without the rows the tab bar takes when it is shown"""
    return app.shot()[top:]


def popup(app: App, top: int = 0, limit: int | None = None):
    """bbox of the completion popup, none while it is hidden. top is the tab bar height, which
    the popup never covers, limit ignores anything to the right of it (the "pasted from
    clipboard" toast in the corner shares the popup's colours)"""
    img = shot(app, top)
    if limit is not None:
        img = img[:, :limit]
    mask = near(img, C["tab_bar_background"]) | near(img, C["tab_active_background"])
    # a popup row is a wide band of them, one stray antialiased pixel of the text is not
    mask &= mask.sum(axis=1)[:, None] >= 20
    if mask.sum() < 300:
        return None
    x0, y0, x1, y1 = mask_bbox(mask)
    return x0, y0 + top, x1, y1 + top


def file_popup(app: App):
    """the popup of the two files, not the transient one of a bare command word, whose options
    make a wide list the completion engine replaces once the word has more letters"""
    box = popup(app)
    return box if box and popup_rows(app, box) == 2 else None


def popup_width(app: App, box: tuple) -> int:
    """cells of text the popup is wide enough for"""
    return round((box[2] - box[0]) / app.cell_width())


def popup_of(app: App, rows: int):
    """the popup once it shows `rows` rows. keys are typed one by one, so a popup for the line
    before the last key can show first"""
    box = popup(app)
    return box if box and popup_rows(app, box) == rows else None


def popup_rows(app: App, box: tuple) -> int:
    """rows the popup shows, it is that many line heights tall plus its padding"""
    return round((box[3] - box[1] - 6) / app.line_height())


def selection_y(app: App, box: tuple) -> int | None:
    """y of the row painted as selected inside the popup, none when it shows none"""
    img = app.shot()[box[1]:box[3], box[0]:box[2]]
    found = mask_bbox(near(img, C["tab_active_background"]))
    return None if found is None else box[1] + (found[1] + found[3]) // 2


def line_cells(app: App, top: int = 0) -> int:
    """cells of the first terminal row that carry text"""
    x0, y0, x1, y1 = app.cell_rect(0, 0, cols=60, top=top)
    band = app.shot()[y0:y1, x0:x1]
    box = ink_bbox(band, dominant(band))
    return 0 if box is None else round(box[2] / app.cell_width())


def pane_ink(app: App, x0: int, x1: int, row: int = 0, top: int = 0) -> np.ndarray:
    """bool mask of the pixels that differ from the background in an x range of a terminal row"""
    _, y0, _, y1 = app.cell_rect(0, row, top=top)
    band = app.shot()[y0:y1, x0:x1]
    return ink(band, dominant(band))


def test_enable_off_no_popup_and_tab_is_the_shells_completion(app_factory, tmp_path):
    """with autocomplete disabled nothing shows while typing and tab completes in the shell"""
    app = zsh_app(app_factory, tmp_path, {"enable": False})
    app.type("ls alp")
    # nothing may pop up while typing a word that would complete with the setting on
    app.wait_still()
    assert popup(app) is None
    app.snap("typing with autocomplete off, the screen stays plain")
    app.type("h")
    app.key("Tab")
    # zsh completes the file into its own line, kuterm had nothing to do with it
    app.wait(lambda: line_cells(app) >= len("$ ls alpha_file.txt"), msg="shell completed the line")
    app.snap("tab put alpha_file.txt into the line, zsh's own completion")


def test_trigger_auto_shows_while_typing_and_hides_for_the_word(app_factory, tmp_path):
    """with "trigger": "auto" the popup follows typing, escape hides it for that word only"""
    app = zsh_app(app_factory, tmp_path)
    app.type("ls alp")
    app.wait(lambda: popup(app), msg="popup while typing")
    app.snap("the popup follows the typed word")
    app.key("Escape")
    app.wait(lambda: popup(app) is None, msg="popup hidden")
    app.type("ha")
    app.wait_still()
    assert popup(app) is None, "typing more of the same word brought it back"
    app.snap("hidden for the rest of the word")
    app.type(" al")
    app.wait(lambda: popup(app), msg="popup for the next word")
    app.snap("the popup is back for the next word")


def test_trigger_key_needs_the_complete_key(app_factory, tmp_path):
    """with "trigger": "key" typing opens nothing, only the complete key does, again after escape"""
    app = zsh_app(app_factory, tmp_path, {"trigger": "key"})
    app.type("ls al")
    app.wait_still()
    assert popup(app) is None, "the popup showed while typing"
    app.snap("typing with the key trigger, nothing shows")
    app.key("ctrl+space")
    app.wait(lambda: popup(app), msg="popup after the complete key")
    app.snap("ctrl-space opened the popup")
    app.key("Escape")
    app.wait(lambda: popup(app) is None, msg="popup hidden")
    app.type("p")
    app.wait_still()
    assert popup(app) is None, "typing reopened the popup"
    app.key("ctrl+space")
    app.wait(lambda: popup(app), msg="popup again after the complete key")
    app.snap("the complete key opened it again after escape")


@pytest.mark.parametrize("rows", [2, 8], ids=["two-rows", "eight-rows"])
def test_max_items_limits_the_rows_shown(app_factory, tmp_path, rows):
    """max_items rows are shown at once, picking past them scrolls the list, not the popup"""
    app = zsh_app(app_factory, tmp_path, {"max_items": rows})
    app.type("git ")
    box = app.wait(lambda: popup_of(app, rows), msg=f"completion popup of {rows} rows")
    app.snap(f"git completions, {rows} rows at once")
    # more candidates than rows: moving down scrolls the list, the popup keeps its height
    for _ in range(rows + 2):
        app.key("Down")
    box = app.wait(lambda: popup(app), msg="popup after moving down")
    assert popup_rows(app, box) == rows, box
    selected = selection_y(app, box)
    assert selected is not None and selected > box[1] + (rows - 1) * app.line_height(), (selected, box)
    app.snap(f"picked a row past the first {rows}, the list scrolled")


@pytest.mark.parametrize("descriptions", [True, False], ids=["with-descriptions", "without"])
def test_show_descriptions_widens_the_popup(app_factory, tmp_path, descriptions):
    """show_descriptions true paints the descriptions next to the labels, false leaves them out"""
    app = zsh_app(app_factory, tmp_path, {"show_descriptions": descriptions})
    app.type("git ")
    box = app.wait(lambda: popup(app), msg="completion popup")
    cells = popup_width(app, box)
    muted = int(near(app.shot()[box[1]:box[3], box[0]:box[2]], C["text_muted"]).sum())
    app.snap(f"git completions with descriptions {'on' if descriptions else 'off'}")
    if descriptions:
        # git's subcommand descriptions are long, the popup is far wider than the labels
        assert cells > 20, cells
        assert muted > 100, muted
    else:
        # only the labels and the padding are left
        assert cells < 20, cells
        assert muted == 0, muted


def test_completion_keys_can_be_rebound(app_factory, tmp_path):
    """all four completion keys work on their new keys, the defaults stop moving or hiding"""
    keys = {"completion_next": "ctrl-shift-j", "completion_prev": "ctrl-shift-k",
            "completion_accept": "ctrl-shift-l", "completion_dismiss": "ctrl-shift-h"}
    app = zsh_app(app_factory, tmp_path, keybindings=keys)
    app.type("ls al")
    box = app.wait(lambda: popup(app), msg="completion popup")
    app.wait_still()
    # pace: a popup can still show the result of the previous keystroke while the last one is
    # computed, and that result landing would reset the selection under the test's feet
    time.sleep(0.5)
    first = selection_y(app, box)
    assert first is not None, "no candidate is selected"
    app.key("ctrl+shift+j")
    app.wait(lambda: selection_y(app, box) != first, msg="selection moved on the new next key")
    second = selection_y(app, box)
    app.snap("the second candidate picked with the rebound next key")
    # the default keys are not bound to the actions anymore
    app.key("Down")
    app.wait_still()
    assert selection_y(app, box) == second, "the default next key still moved the selection"
    app.key("Escape")
    app.wait_still()
    assert popup(app) is not None, "the default dismiss key still hid the popup"
    app.snap("default down and escape do nothing")
    app.key("ctrl+shift+k")
    app.wait(lambda: selection_y(app, box) == first, msg="selection back on the new prev key")
    app.snap("the new prev key went back to the first candidate")
    app.key("ctrl+shift+j")
    app.wait(lambda: selection_y(app, box) == second, msg="selection on the file again")
    # the accepted candidate is the file in the second row, the shell's line holds it
    app.key("ctrl+shift+l")
    app.wait(lambda: line_cells(app) >= len("$ ls alpha_file.txt"), msg="accepted into the line")
    app.snap("the rebound accept key put the picked candidate into the line")
    app.key("ctrl+space")
    app.wait(lambda: popup(app), msg="popup back on the complete key")
    app.key("ctrl+shift+h")
    app.wait(lambda: popup(app) is None, msg="popup hidden on the new dismiss key")
    app.snap("the rebound dismiss key hid the popup")


def test_keymap_when_completion(app_factory, tmp_path):
    """a keymap entry with "when": "completion" fires only while the popup shows"""
    app = zsh_app(app_factory, tmp_path,
                  sections={"keymap": [{"keys": "ctrl-shift-y", "action": "new_tab",
                                        "when": "completion"}]})
    app.key("ctrl+shift+y")
    nothing_for(1.0, lambda: app.title() == "2")
    assert app.title() == "1", "the key opened a tab while the popup was hidden"
    app.snap("the completion-only key does nothing while the popup is hidden")
    app.type("ls al")
    app.wait(lambda: popup(app), msg="completion popup")
    app.key("ctrl+shift+y")
    app.wait_title("2")
    app.snap("the same key opened a tab while the popup was showing")


def test_spec_file_next_to_settings_adds_a_command(app_factory, tmp_path):
    """a spec file in the "completions" folder completes the command it names"""
    app = zsh_app(app_factory, tmp_path, files={"completions/mytool.json": MYTOOL})
    app.type("mytool ")
    # the two subcommands, nothing else
    app.wait(lambda: popup_of(app, 2), msg="popup of the two subcommands")
    app.snap("the subcommands of a spec file written next to settings.jsonc")


def test_spec_file_replaces_a_bundled_one(app_factory, tmp_path):
    """a spec file named after a bundled command replaces the bundled spec"""
    app = zsh_app(app_factory, tmp_path, files={"completions/ls.json": MY_LS})
    app.type("ls -")
    # the bundled ls lists many options for "-", this one only its own
    app.wait(lambda: popup_of(app, 1), msg="popup of the one option")
    app.snap("only the option of the replacing spec is listed, none of ls's own")


def test_broken_spec_file_keeps_other_completions(app_factory, tmp_path):
    """a broken spec file counts as a missing spec, other commands complete as usual"""
    app = zsh_app(app_factory, tmp_path,
                  files={"completions/broken.json": "{ this is not json",
                         "completions/mytool.json": MYTOOL})
    app.type("ls alp")
    app.wait(lambda: popup(app), msg="popup for the filesystem candidates")
    app.snap("ls still completes files with a broken spec file in the folder")
    app.key("Escape")
    app.wait(lambda: popup(app) is None, msg="popup hidden")
    app.key("Return")
    app.type("mytool ")
    app.wait(lambda: popup_of(app, 2), msg="popup for the good spec")
    app.snap("the spec next to the broken one still works")


def test_paste_check_invisible_off_keeps_the_popup(app_factory, tmp_path):
    """with paste_check_invisible off a pasted line with a hidden character goes right in and
    the popup follows it"""
    app = zsh_app(app_factory, tmp_path, sections={"terminal": {"paste_check_invisible": False}})
    # a zero width space in the pasted command, which the check would ask about
    set_clipboard("​ls alp")
    app.wait(lambda: get_clipboard() == "​ls alp", msg="clipboard owner up")
    app.key("ctrl+shift+v")
    left = app.size()[0] // 2
    box = app.wait(lambda: popup(app, limit=left), msg="popup for the pasted line")
    app.snap("the pasted line with a zero width space, the popup still follows it")
    assert popup_rows(app, box) == 2, box


def test_split_panes_popup_belongs_to_the_focused_pane(app_factory, tmp_path):
    """in a split the popup shows in the pane with focus only"""
    app = zsh_app(app_factory, tmp_path,
                  keybindings={"split_right": "ctrl-shift-r", "focus_pane_left": "ctrl-shift-h"})
    app.key("ctrl+shift+r")
    width, _ = app.size()
    divider = width // 2
    # the pane on the right got the tab and its shell, focus went with it
    app.wait(lambda: pane_ink(app, divider + 10, divider + 60).any(), msg="prompt in the right pane")
    app.type("ls alp")
    box = app.wait(lambda: file_popup(app), msg="popup in the right pane")
    app.snap("popup in the focused right pane")
    assert box[0] > divider, box
    app.key("ctrl+shift+h")
    # the popup of the pane without focus is gone, the new left pane gets its own shell
    app.wait(lambda: popup(app) is None, msg="popup gone with the focus")
    app.wait(lambda: pane_ink(app, 10, divider - 10).any(), msg="prompt in the left pane")
    app.snap("focus moved left, no popup is left behind")
    app.type("ls alp")
    box = app.wait(lambda: file_popup(app), msg="popup in the left pane")
    app.snap("popup in the focused left pane")
    assert box[2] < divider, box


def test_a_wide_popup_stays_inside_its_pane(app_factory, tmp_path):
    """the options of a bare command word make a popup wide enough to run past a split pane; it
    must be clamped to the pane of the shell that reported the line"""
    app = zsh_app(app_factory, tmp_path,
                  keybindings={"split_right": "ctrl-shift-r", "focus_pane_left": "ctrl-shift-h"})
    app.key("ctrl+shift+r")
    width, _ = app.size()
    divider = width // 2
    app.wait(lambda: pane_ink(app, divider + 10, divider + 60).any(), msg="prompt in the right pane")
    app.key("ctrl+shift+h")
    app.wait(lambda: popup(app) is None, msg="popup gone with the focus")
    app.wait(lambda: pane_ink(app, 10, divider - 10).any(), msg="prompt in the left pane")
    app.type("ls")
    box = app.wait(lambda: popup(app), msg="the options popup")
    assert popup_rows(app, box) > 2, box  # the wide one, not the two files
    app.snap("the options popup crossing into the pane on the right")
    assert box[2] <= divider, box


def test_two_tabs_popup_belongs_to_the_focused_tab(app_factory, tmp_path):
    """the popup of a tab shows only while that tab is active"""
    app = zsh_app(app_factory, tmp_path)
    app.type("ls alp")
    app.wait(lambda: popup(app), msg="popup in the first tab")
    app.snap("popup in the first tab")
    app.key("ctrl+shift+t")
    app.wait_title("2")
    top = app.top(tabs=2)
    # the tab bar is up now, the first tab's popup is not painted in this one
    app.wait(lambda: popup(app, top) is None, msg="no popup in the second tab")
    app.wait(lambda: line_cells(app, top) >= 2, msg="prompt in the second tab")
    app.snap("the second tab, no popup of the hidden one is painted")
    app.type("ls al")
    app.wait(lambda: popup(app, top), msg="popup in the second tab")
    app.snap("popup in the second tab")
    app.key("alt+1")
    app.wait_title("1")
    # the first tab still has its line and its popup comes back with it
    app.wait(lambda: popup(app, top), msg="popup back in the first tab")
    app.snap("back in the first tab, its popup shows again")


def test_reload_applies_autocomplete_settings_live(app_factory, tmp_path):
    """saving max_items and show_descriptions in settings.jsonc shrinks the open popup at once"""
    app = zsh_app(app_factory, tmp_path)
    app.type("git ")
    box = app.wait(lambda: popup_of(app, 8), msg="completion popup of eight rows")
    app.snap("git completions with the default settings, eight rows and descriptions")
    app.write("settings.jsonc", {**app.settings,
                                 "autocomplete": {"enable": True, "trigger": "auto",
                                                  "max_items": 2, "show_descriptions": False}})
    shrunk = app.wait(lambda: (p := popup(app)) and popup_rows(app, p) == 2 and p,
                      msg="the popup shrank to two rows")
    app.snap("the same popup after the save, two rows without descriptions")
    assert popup_width(app, shrunk) < popup_width(app, box)
