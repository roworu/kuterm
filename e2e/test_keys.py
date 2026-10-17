"""keys pressed for real with xdotool: default and custom keybindings, clipboard, selection, shell input"""

import time

import pytest

from harness import App, bundled_theme, close_to, get_clipboard, ink_bbox, near, set_clipboard

FEATURE = "keyboard, keybindings and clipboard"

DARK = bundled_theme(True)


def keyed(app_factory, keybindings=None, **settings) -> App:
    return app_factory({"theme": {"mode": "dark"}, "window_title": ["number"], **settings}, keybindings=keybindings)


def test_new_and_close_tab_keys(app_factory):
    """ctrl-shift-t opens a tab, ctrl-shift-w closes the active one and activates the left one"""
    app = keyed(app_factory)
    app.key("ctrl+shift+t")
    app.wait_title("2")
    app.key("ctrl+shift+t")
    app.wait_title("3")
    app.snap("three tabs")
    app.key("alt+2")
    app.wait_title("2")
    app.key("ctrl+shift+w")
    app.wait_title("1")


def test_close_last_tab_quits(app_factory):
    """closing the last tab exits the app"""
    app = keyed(app_factory)
    app.snap("one tab left")
    app.key("ctrl+shift+w")
    app.wait(lambda: not app.alive(), msg="app exit")
    assert app.proc.returncode == 0


def test_exiting_shell_closes_its_tab(app_factory):
    """a shell that exits closes its tab"""
    app = keyed(app_factory)
    app.key("ctrl+shift+t")
    app.wait_title("2")
    app.run("exit")
    app.wait_title("1")


def test_next_tab_wraps(app_factory):
    """ctrl-tab goes to the tab on the right, from the last back to the first"""
    app = keyed(app_factory)
    for n in (2, 3):
        app.key("ctrl+shift+t")
        app.wait_title(str(n))
    app.key("ctrl+Tab")
    app.wait_title("1")
    app.key("ctrl+Tab")
    app.wait_title("2")


def test_alt_number_activates_tab(app_factory):
    """alt-n jumps to tab n, numbers past the last tab do nothing"""
    app = keyed(app_factory)
    for n in (2, 3):
        app.key("ctrl+shift+t")
        app.wait_title(str(n))
    app.key("alt+1")
    app.wait_title("1")
    app.key("alt+3")
    app.wait_title("3")
    app.key("alt+7")
    # nothing may happen, so there is nothing to wait for
    time.sleep(1.5)
    assert app.title() == "3"


def test_plain_ctrl_keys_reach_the_shell(app_factory):
    """ctrl-t and ctrl-w are not bound, the shell gets them"""
    app = keyed(app_factory)
    app.type("echo one two")
    app.key("ctrl+w")
    app.type(f"> {app.file('out')}")
    app.key("Return")
    app.wait_file("out", "one")
    assert app.title() == "1"


def test_special_keys_edit_command_line(app_factory):
    """arrows, home, end, backspace and delete edit the command line"""
    app = keyed(app_factory)
    app.type("echo bc")
    app.key("Home", "Right", "Right", "Right", "Right", "Right")
    app.type("a")
    app.key("End", "BackSpace")
    app.type(f"X > {app.file('out')}")
    app.key("Return")
    app.wait_file("out", "abX")


def test_paste_key(app_factory):
    """ctrl-shift-v pastes the clipboard, several lines too"""
    app = keyed(app_factory)
    set_clipboard(f"echo pasted > {app.file('one')}\necho second > {app.file('two')}\n")
    app.key("ctrl+shift+v")
    # bash turns on bracketed paste, so the pasted lines wait for enter
    app.key("Return")
    app.wait_file("one", "pasted")
    app.wait_file("two", "second")


@pytest.mark.parametrize("bracketed", [True, False], ids=["bracketed", "plain"])
def test_paste_is_bracketed_only_when_asked(app_factory, bracketed):
    """pasted text is wrapped in bracketed paste marks only when the program turned it on"""
    mode = "\\033[?2004h" if bracketed else "\\033[?2004l"
    app = app_factory({"theme": {"mode": "dark"}},
                      script=f"printf '{mode}'; stty raw -echo; printf ready; head -c 20 > \"$T2_TMP/got.tmp\"; "
                             'stty sane; mv "$T2_TMP/got.tmp" "$T2_TMP/got"; echo; echo got:; cat -v "$T2_TMP/got"')
    set_clipboard("hi")
    # the app read the mode once it shows what came after it, the cursor is one more cell
    app.wait_line(0, 6)
    app.wait(lambda: get_clipboard() == "hi", msg="clipboard owner up")
    app.key("ctrl+shift+v")
    app.type("x" * 20)
    got = app.wait_file("got")
    # cat -v prints what the program read on the third row
    x0, y0, _, y1 = app.cell_rect(0, 2)
    app.wait(lambda: ink_bbox(app.shot()[y0:y1, x0:], DARK["terminal_background"]), msg="what the program read")
    app.wait_still("what the program read")
    app.snap("what the program read, escapes shown by cat -v")
    assert ("\x1b[200~hi\x1b[201~" in got) == bracketed, repr(got)
    assert "hi" in got


@pytest.mark.parametrize("flags,keys,expected", [
    ("1", ["ctrl+i", "Tab"], "\x1b[105;5u\t"),
    ("1", ["Escape", "ctrl+shift+a"], "\x1b[27u\x1b[97;6u"),
    # gpui's x11 backend loses key releases of xdotool keys, the unit tests cover them there
    pytest.param("3", ["ctrl+a"], "\x1b[97;5u\x1b[97;5:3u", marks=pytest.mark.wayland_only),
], ids=["ctrl-i_vs_tab", "escape_and_ctrl-shift-a", "release_events"])
def test_kitty_keyboard_protocol(app_factory, flags, keys, expected):
    """a program that turns on kitty's keyboard protocol gets keys it can tell apart"""
    app = app_factory({"theme": {"mode": "dark"}},
                      script=f"stty raw -echo; printf '\\033[>{flags}u'; printf ready; step 1; "
                             'timeout --foreground 2 cat > "$T2_TMP/got"; stty sane; touch "$T2_TMP/done"')
    # the app read the flags once it shows what came after them (and the cursor), keys wait
    # in the tty for cat
    app.wait_line(0, 6)
    app.step(1)
    for key in keys:
        app.key(key)
    app.wait_file("done", timeout=10)
    got = app.file("got").read_bytes().decode(errors="replace")
    assert got == expected, repr(got)


def test_keymap_send_text_chord_and_unbind(app_factory):
    """keymap entries: a chord opens a tab, send_text types escapes, unbind lets a key through"""
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number"],
                       "keymap": [{"keys": "ctrl-a c", "action": "new_tab"},
                                  {"keys": "ctrl-shift-k", "action": {"send_text": "\u001b[2Jsent"}},
                                  {"keys": "ctrl-shift-w", "action": "unbind"}]},
                      script="stty raw -echo; printf ready; step 1; "
                             'timeout --foreground 3 cat > "$T2_TMP/got"; stty sane; touch "$T2_TMP/done"')
    # keys wait in the raw tty until cat reads them, the cursor is one more cell
    app.wait_line(0, 6)
    app.step(1)
    app.key("ctrl+shift+k", "ctrl+shift+w")
    app.wait_file("done", timeout=10)
    got = app.file("got").read_bytes().decode(errors="replace")
    # the unbound ctrl-shift-w reaches the program as ctrl-w
    assert got == "\x1b[2Jsent\x17", repr(got)
    # ctrl-shift-w no longer closes the only tab, which would quit
    assert app.alive()
    app.key("ctrl+a", "c")
    app.wait_title("2")


def test_mouse_selection_and_copy(app_factory):
    """dragging selects text in the selection color, ctrl-shift-c copies it"""
    app = keyed(app_factory)
    app.run("clear; echo SELECTME-please")
    app.wait_line(0, 15)
    x0, y0, _, y1 = app.cell_rect(0, 0)
    _, _, x1, _ = app.cell_rect(7, 0)
    app.drag((x0 + 1, (y0 + y1) // 2), (x1 - 1, (y0 + y1) // 2))
    app.wait(lambda: close_to(app.cells_color(2, 0, 1), DARK["selection"], 12) or
             near(app.shot()[y0:y1, x0:x1], DARK["selection"], 3).mean() > 0.3, msg="selection painted")
    app.snap("text selected with the mouse")
    app.key("ctrl+shift+c")
    app.wait(lambda: get_clipboard() == "SELECTME", msg=f"clipboard, got {get_clipboard()!r}")


def test_word_separators(app_factory):
    """word_separators decides where a double clicked word ends"""
    app = keyed(app_factory, terminal={"word_separators": " -"})
    app.run("clear; echo alpha-bravo charlie")
    app.wait_line(0, 19)
    x0, y0, x1, y1 = app.cell_rect(8, 0)
    app.click((x0 + x1) // 2, (y0 + y1) // 2, repeat=2)
    app.key("ctrl+shift+c")
    app.wait(lambda: get_clipboard() == "bravo", msg=f"clipboard, got {get_clipboard()!r}")


@pytest.mark.parametrize("trim", [True, False])
def test_copy_trim_trailing_whitespace(app_factory, trim):
    """copy_trim_trailing_whitespace drops spaces and tabs at the end of copied lines"""
    app = keyed(app_factory, terminal={"copy_trim_trailing_whitespace": trim})
    app.run("clear; printf 'padded \\t\\n'")
    app.wait_line(0, 6)
    x0, y0, x1, y1 = app.cell_rect(2, 0)
    app.click((x0 + x1) // 2, (y0 + y1) // 2, repeat=3)
    app.key("ctrl+shift+c")
    expected = "padded\n" if trim else "padded \t\n"
    app.wait(lambda: get_clipboard() == expected, msg=f"clipboard, got {get_clipboard()!r}")


def selected(app: App, col: int, row: int, cols: int) -> bool:
    """the selection color fills the cells around their glyphs"""
    x0, y0, x1, y1 = app.cell_rect(col, row, cols)
    return near(app.shot()[y0:y1, x0:x1], DARK["selection"], 3).mean() > 0.3


def test_triple_click_selects_line(app_factory):
    """a triple click selects the whole line under the pointer"""
    app = keyed(app_factory)
    app.run("clear; echo alpha bravo charlie")
    app.wait_line(0, 19)
    x0, y0, x1, y1 = app.cell_rect(8, 0)
    app.click((x0 + x1) // 2, (y0 + y1) // 2, repeat=3)
    app.wait(lambda: selected(app, 14, 0, 3), msg="line selected")
    app.snap("line selected with a triple click")
    app.key("ctrl+shift+c")
    # line selections end with the newline, like in alacritty
    app.wait(lambda: get_clipboard() == "alpha bravo charlie\n", msg=f"clipboard, got {get_clipboard()!r}")


def test_shift_click_extends_selection(app_factory):
    """a click then a shift+click selects everything between them"""
    app = keyed(app_factory)
    app.run("clear; echo alpha bravo charlie")
    app.wait_line(0, 19)
    x0, y0, _, y1 = app.cell_rect(0, 0)
    _, _, x1, _ = app.cell_rect(18, 0)
    y = (y0 + y1) // 2
    app.click(x0 + 1, y)
    with app.hold("shift"):
        app.click(x1 - 1, y)
    app.wait(lambda: selected(app, 8, 0, 3), msg="selection extended")
    app.snap("click on alpha then shift+click on charlie")
    app.key("ctrl+shift+c")
    app.wait(lambda: get_clipboard() == "alpha bravo charlie", msg=f"clipboard, got {get_clipboard()!r}")


@pytest.mark.parametrize("action,keys", [("new_tab", "ctrl+shift+t"), ("next_tab", "ctrl+Tab"),
                                         ("activate_tab_1", "alt+1")])
def test_null_disables_action(app_factory, action, keys):
    """null in the keybindings section disables an action and its key"""
    keys_file = {action: None} if action == "new_tab" else {action: None, "new_tab": "ctrl-shift-t"}
    app = keyed(app_factory, keybindings=keys_file)
    if action != "new_tab":
        app.key("ctrl+shift+t")
        app.wait_title("2")
    before = app.title()
    app.key(keys)
    # nothing may happen, so there is nothing to wait for
    time.sleep(1.5)
    assert app.title() == before


def test_override_moves_action_to_new_key(app_factory):
    """an action bound to another key works there, the old key does nothing"""
    app = keyed(app_factory, keybindings={"new_tab": "ctrl-alt-n"})
    app.key("ctrl+shift+t")
    # the old key must do nothing, so there is nothing to wait for
    time.sleep(1.5)
    assert app.title() == "1"
    app.key("ctrl+alt+n")
    app.wait_title("2")


def test_key_sequence_binding(app_factory):
    """keys separated by spaces are pressed one after another"""
    app = keyed(app_factory, keybindings={"new_tab": "ctrl-a t"})
    app.key("ctrl+a", "t")
    app.wait_title("2")


def test_activate_tab_above_nine(app_factory):
    """activate_tab_<n> can go past 9"""
    app = keyed(app_factory, keybindings={"activate_tab_10": "alt-0"})
    for n in range(2, 11):
        app.key("ctrl+shift+t")
        app.wait_title(str(n))
    app.key("alt+1")
    app.wait_title("1")
    app.key("alt+0")
    app.wait_title("10")


# kinds of bad entries are unit tested, one shows they are reported at launch
@pytest.mark.parametrize("keys", [{"launch_rockets": "ctrl-b"}])
def test_invalid_keybindings_at_launch_are_reported(app_factory, keys):
    """a bad keybindings section at launch prints an error naming the section, defaults are used"""
    app = keyed(app_factory, keybindings=keys)
    app.wait(lambda: "invalid settings in" in app.output(), msg="error on stderr")
    assert "keybindings" in app.output()
    assert app.alive()


def test_invalid_keybindings_section_keeps_the_last_good_keys(app_factory):
    """a bad key saved in the keybindings section is reported, the running keys stay"""
    app = keyed(app_factory)
    app.write("settings.jsonc", {**app.settings, "keybindings": {"new_tab": 5}})
    app.wait(lambda: "keybindings" in app.output() and "invalid settings" in app.output(), msg="error on stderr")
    app.key("ctrl+shift+t")
    app.wait_title("2")


def test_wheel_scrolls_history_and_typing_jumps_back(app_factory):
    """the wheel scrolls into history, typing goes back to the prompt"""
    app = keyed(app_factory)
    app.run("clear; for i in $(seq 1 200); do echo line $i; done; echo end")
    app.wait_line(app.layout_pty()[0] - 2, 3)
    bottom = app.shot()
    app.wheel(450, 300, up=True, clicks=10)
    app.wait(lambda: (app.shot() != bottom).any(), msg="scrolled view")
    # smooth scrolling takes a few frames, the cursor is out of view
    app.wait_still("scrolling to end")
    app.snap("scrolled up with the wheel")
    app.type("x")
    app.wait(lambda: (app.shot()[: -app.line_height() * 2] == bottom[: -app.line_height() * 2]).mean() > 0.95,
             msg="back at the prompt")
