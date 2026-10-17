"""the password manager: the entries of pass, secret-tool, bitwarden or 1password, or of commands
of your own, listed in a picker whose picked password goes into the program asking for it and
nowhere else; the picker warns before typing into a program that would show it, concealed text
stays hidden until asked for, and a fill works in incognito and sandboxed tabs and after a reload"""

import math
from pathlib import Path

import numpy as np
import pytest

from harness import App, as_rgb, get_clipboard, ink, near, set_clipboard, unique_theme
from kit import (executable, integrated, listening, msg, nothing_for, notification_box, send, tabs,
                 text, wait_block)

FEATURE = "password manager"

THEME = unique_theme(0x5a)
C = as_rgb(THEME)

# no default key, so the tests bind the two actions they drive (the settings spell the keys with
# dashes, xdotool wants them with plusses)
FILL, FILL_KEY = "ctrl+shift+u", "ctrl-shift-u"
REVEAL, REVEAL_KEY = "ctrl+shift+g", "ctrl-shift-g"

# the shell line the tests run as the program asking for a secret: it clears the pane, prints
# READY (split, so the echoed command line never holds the marker), turns echo off and reads one
# line into $T2_TMP/got, saying how long it was. echo off is what tells the picker a secret is read
READ = ("clear; stty -echo; printf 'RE'; printf 'ADY'; IFS= read -r x; stty echo; "
        "printf '%s' \"$x\" > $T2_TMP/got; printf ' ok=%s\\n' \"${#x}\"")

# the same for an enter: false fill, where no newline is sent: non canonical mode hands the bytes
# over one by one, so reading exactly n of them finishes on its own
RAW = ("clear; stty -echo -icanon; printf 'RE'; printf 'ADY'; head -c {n} > $T2_TMP/got; "
       "stty echo icanon; printf ' ok=%s\\n' \"$(wc -c < $T2_TMP/got)\"")

# a program asking for two secrets in a row, saying so in between, for a second fill that finds it
# still waiting
READ_TWICE = ("clear; stty -echo; printf 'RE'; printf 'ADY'; IFS= read -r x; printf 'one '; "
              "IFS= read -r y; stty echo; printf '%s\\n%s' \"$x\" \"$y\" > $T2_TMP/got; "
              "printf ' ok=%s,%s\\n' \"${#x}\" \"${#y}\"")

# and a program that reads its line with the terminal echoing it: no secret is asked for, the
# picker has to say that the password would show
SHOWING = ("clear; printf 'RE'; printf 'ADY'; IFS= read -r x; printf '%s' \"$x\" > $T2_TMP/got; "
           "printf ' ok=%s\\n' \"${#x}\"")

# the same as READ, but for the tab lock: a program of its own waits, not a shell builtin that
# would leave the shell in front of the tty
READ_CHILD = ("clear; stty -echo; printf 'RE'; printf 'ADY'; "
              "head -1 | tr -d '\\n' > $T2_TMP/got; stty echo; "
              "printf ' ok=%s\\n' \"$(wc -c < $T2_TMP/got)\"")

# the programs of the presets, stubbed: every one logs the arguments it was run with, so the test
# can say what the app asked for, and no real password manager is ever started
PASS = """#!/bin/sh
printf '%s\\n' "$*" >> "$T2_TMP/pass_calls"
case "$*" in
"show web/example") printf 's3cret!x\\nuser: me\\n' ;;
"show work/vpn") printf 'vpn-pass\\n' ;;
esac
"""

SECRET_TOOL = """#!/bin/sh
printf '%s\\n' "$*" >> "$T2_TMP/secret-tool_calls"
if [ "$1" = "search" ]; then
  printf '[/org/freedesktop/secrets/collection/login/1]\\n'
  printf 'label = noise entry\\n'
  printf 'secret = not this\\n'
  printf 'attribute.service = chosen-service\\n'
  printf 'attribute.username = me\\n'
elif [ "$1" = "lookup" ]; then
  [ "$2 $3" = "service chosen-service" ] && printf 'tool-secret\\n'
fi
"""

BW = """#!/bin/sh
printf '%s\\n' "$*" >> "$T2_TMP/bw_calls"
case "$*" in
"list items") printf '[{"name": "bw-alpha"}, {"name": "bw-beta"}]\\n' ;;
"get password bw-alpha") printf 'bw-secret-alpha\\n' ;;
"get password bw-beta") printf 'bw-secret-beta\\n' ;;
esac
"""

OP = """#!/bin/sh
printf '%s\\n' "$*" >> "$T2_TMP/op_calls"
case "$1 $2" in
"item list") printf '[{"title": "op-alpha"}, {"title": "op-beta"}]\\n' ;;
"item get") printf 'op-secret-%s\\n' "$3" ;;
esac
"""

# bitwarden and 1password are asked through jq; the stub runs the filter the app wrote out
JQ = """#!/usr/bin/python3
import json
import os
import sys

with open(os.environ["T2_TMP"] + "/jq_calls", "a") as log:
    log.write(" ".join(sys.argv[1:]) + "\\n")
key = {".[].name": "name", ".[].title": "title"}.get(sys.argv[-1])
if key is None:
    sys.exit("fake jq: unknown filter " + sys.argv[-1])
for item in json.load(sys.stdin):
    print(item[key])
"""

# writes its arguments out and runs what follows "--", as the real bubblewrap would
BWRAP = """#!/bin/sh
printf "%s\\n" "$@" > "$T2_TMP/bwrap_args"
while [ "$1" != "--" ]; do shift; done; shift
exec "$@"
"""

# a get of the tests' own: it logs the name it was handed, then prints a password and a second
# line that must never be typed
GET_OWN = ('printf "%s\\n" "$1" >> "$T2_TMP/get_calls"; '
           'case "$1" in bad) exit 3 ;; esac; printf "pw-%s\\nuser: me\\n" "$1"')


def pw_settings(password_manager: dict, profiles: list | None = None, **extra) -> dict:
    """bash with kuterm's integration and remote control, a numbered title and the two keys"""
    settings = integrated(window_title=["number", {"text": " "}, "icon"], icons={"default": "D"},
                          keybindings={"fill_password": FILL_KEY, "show_hidden_text": REVEAL_KEY},
                          password_manager=password_manager, **extra)
    if profiles:
        settings["profiles"] = settings["profiles"] + profiles
    return settings


def pw_app(app_factory, stubs: dict, password_manager: dict, env: dict | None = None,
           profiles: list | None = None, **extra) -> App:
    """app whose PATH starts with stub password managers, so none of the real ones can run.

    env values may be a function of the app, for paths under its own folders
    """
    app = app_factory(pw_settings(password_manager, profiles, **extra), themes={"dark": THEME},
                      start=False)
    bindir = app.tmp / "bin"
    bindir.mkdir()
    for name, script in stubs.items():
        executable(bindir / name, script)
    app.env_extra["PATH"] = f"{bindir}:/usr/local/bin:/usr/bin:/bin"
    for key, value in (env or {}).items():
        app.env_extra[key] = str(value(app) if callable(value) else value)
    app.start()
    listening(app)
    app.mouse(450, 580)
    return app


def entries(app: App, names: list) -> Path:
    """empty .gpg files in the default store, one per entry name"""
    for name in names:
        path = app.home / ".password-store" / f"{name}.gpg"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("not really encrypted")
    return app.home / ".password-store"


def store(app: App, names: list) -> Path:
    """empty .gpg files in the folder PASSWORD_STORE_DIR points at"""
    for name in names:
        path = app.home / "store" / f"{name}.gpg"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("not really encrypted")
    return app.home / "store"


def calls(app: App, program: str) -> list:
    """arguments each run of a stub program was given, one line per run"""
    path = app.file(f"{program}_calls")
    return path.read_text().splitlines() if path.exists() else []


def picker(app: App, img=None) -> tuple | None:
    """(x0, x1, top, divider, bottom) of the panel a pick opens over the pane, None when none is up

    the entries' text starts left of the middle, so the panel's own edges are measured from the
    rows and columns of its background color, not from a box around its middle
    """
    img = app.shot() if img is None else img
    w, h = img.shape[1], img.shape[0]
    mid = near(img[app.top() : 3 * h // 4, w * 2 // 5 : 3 * w // 5], C["tab_bar_background"], 2)
    if mid.sum() < 500:
        return None
    rows = np.flatnonzero(mid.any(axis=1)) + app.top()
    # a notification further down the pane starts a second block of rows, past a wide gap
    gaps = np.flatnonzero(np.diff(rows) > 60)
    top, bottom = int(rows[0]), int(rows[gaps[0]]) if len(gaps) else int(rows[-1])
    columns = np.flatnonzero(near(img[top:bottom], C["tab_bar_background"], 2).any(axis=0))
    x0, x1 = int(columns.min()), int(columns.max()) + 1
    lines = np.flatnonzero(near(img[top:bottom, x0 + 8 : x1 - 8], C["border"]).mean(axis=1) > 0.9)
    below = lines[lines > 5]
    if not len(below):
        return None
    return x0, x1, top, int(below.min()) + top, bottom


def placeholder_width(app: App) -> int:
    """width in pixels of the ink in the picker's query row, its placeholder text"""
    img = app.shot()
    box = picker(app, img)
    if box is None:
        return 0
    x0, x1, top, divider, _ = box
    columns = np.flatnonzero(ink(img[top + 2 : divider - 2, x0 + 2 : x1 - 2],
                                 C["tab_bar_background"], 40).any(axis=0))
    return int(columns[-1] - columns[0]) if len(columns) else 0


def listed(app: App) -> int:
    """how many entries the picker draws, one band of ink per entry"""
    img = app.shot()
    box = picker(app, img)
    if box is None:
        return 0
    x0, x1, _, divider, bottom = box
    band = ink(img[divider + 3 : bottom - 2, x0 + 2 : x1 - 2], C["tab_bar_background"], 40)
    present = band.any(axis=1).astype(np.int8)
    if not present.size:
        return 0
    return int(((present[1:] == 1) & (present[:-1] == 0)).sum()) + int(present[0])


def open_picker(app: App) -> tuple:
    """the picker the fill password key opens"""
    app.key(FILL)
    return app.wait(lambda: picker(app), msg="the entry picker")


def wait_ready(app: App) -> None:
    """the program turned echo off and waits for the secret"""
    app.wait(lambda: "READY" in text(app), msg="the program asking for a secret")


def row_columns(app: App, row: int, cols: int = 40) -> int:
    """cells of a pane row that hold painted pixels"""
    img = app.shot()
    x0, y0, _, y1 = app.cell_rect(0, row, top=app.top())
    x1 = app.cell_rect(cols - 1, row, top=app.top())[2]
    painted = np.flatnonzero(ink(img[y0:y1, x0:x1], C["terminal_background"], 40).any(axis=0))
    # glyphs stop short of their cell, so the ink up to the last painted pixel decides the count
    return math.ceil((int(painted[-1]) + 1) / app.cell_width()) if len(painted) else 0


def test_pass_preset_lists_the_store_and_types_the_first_line(app_factory):
    """the pass preset lists the entries of the store without their .gpg and types the first line
    of `pass show` for the picked one: the rest of the output never reaches the program"""
    app = pw_app(app_factory, {"pass": PASS}, {"preset": "pass", "enter": True},
                 env={"PASSWORD_STORE_DIR": lambda app: app.home / "store"})
    store(app, ["web/example", "work/vpn"])
    app.run(READ)
    wait_ready(app)
    open_picker(app)
    app.snap("the entries of the store listed by the picker")
    assert listed(app) == 2
    app.type("vpn")
    app.key("Return")
    app.wait_file("got", "vpn-pass")
    assert app.file("got").read_text() == "vpn-pass"
    # the store is the one PASSWORD_STORE_DIR names, and pass is asked for the name without .gpg
    assert calls(app, "pass") == ["show work/vpn"]
    # the row of the program still holds only its own marker: nothing of the password was echoed
    app.wait_line(0, top=app.top(), cols=len("READY ok=8"))
    assert "s3cret" not in text(app, scrollback=True)
    assert "user: me" not in text(app, scrollback=True)
    app.snap("the password filled in, the row showing only the marker of the program")


def test_secret_tool_preset_lists_the_service_attribute(app_factory):
    """the secret-tool preset asks the keyring for its services: only the service attribute lines
    become entries, the label next to them does not, and the picked one is looked up"""
    app = pw_app(app_factory, {"secret-tool": SECRET_TOOL},
                 {"preset": "secret-tool", "enter": True})
    app.run(READ)
    wait_ready(app)
    open_picker(app)
    app.snap("the one service of the keyring, its label not listed")
    assert listed(app) == 1
    app.key("Return")
    app.wait_file("got", "tool-secret")
    assert calls(app, "secret-tool") == [
        "search --all --unlock xdg:schema org.freedesktop.Secret.Generic",
        "lookup service chosen-service",
    ]
    app.wait_line(0, top=app.top(), cols=len("READY ok=11"))


def test_bitwarden_preset_lists_through_jq(app_factory):
    """the bw preset pipes `bw list items` through jq for the names and asks bw for the password"""
    app = pw_app(app_factory, {"bw": BW, "jq": JQ}, {"preset": "bw", "enter": True})
    app.run(READ)
    wait_ready(app)
    open_picker(app)
    assert listed(app) == 2
    app.type("beta")
    app.key("Return")
    app.wait_file("got", "bw-secret-beta")
    assert calls(app, "bw") == ["list items", "get password bw-beta"]
    assert calls(app, "jq") == ["-r .[].name"]
    app.wait_line(0, top=app.top(), cols=len("READY ok=14"))
    app.snap("the items of bitwarden, their password filled in")


def test_one_password_preset_lists_through_jq(app_factory):
    """the op preset lists the item titles through jq and asks op for the revealed password field"""
    app = pw_app(app_factory, {"op": OP, "jq": JQ}, {"preset": "op", "enter": True})
    app.run(READ)
    wait_ready(app)
    open_picker(app)
    assert listed(app) == 2
    app.type("alpha")
    app.key("Return")
    app.wait_file("got", "op-secret-op-alpha")
    assert calls(app, "op") == ["item list --format json",
                                "item get op-alpha --fields password --reveal"]
    assert calls(app, "jq") == ["-r .[].title"]
    app.wait_line(0, top=app.top(), cols=len("READY ok=18"))


def test_own_list_and_get_commands_run_with_the_picked_name(app_factory):
    """password_manager list and get of your own: every line of list is an entry, the picked name
    reaches get as one argument with its spaces, and only the first line of its output is typed"""
    app = pw_app(app_factory, {},
                 {"list": ["sh", "-c", "printf 'one\\ntwo\\nodd name\\npwned-$(touch $T2_TMP/pwned)\\n'"],
                  "get": ["sh", "-c", GET_OWN, "sh", "{name}"], "enter": True})
    app.run(READ_TWICE)
    wait_ready(app)
    open_picker(app)
    app.snap("the four entries a command of the test prints")
    assert listed(app) == 4
    app.type("odd")
    app.key("Return")
    # the reader took the first line and says so before it asks for the second one
    app.wait(lambda: "READYone" in text(app), msg="the reader ready for a second password")
    # a name that looks like shell code is handed over as the argument it is, never run: get sees
    # the $(touch ...) itself, nothing replaced it before the command started
    open_picker(app)
    app.type("pwned")
    app.key("Return")
    app.wait_file("got", "pw-odd name\npw-pwned-$(touch $T2_TMP/pwned)")
    assert app.file("got").read_text() == "pw-odd name\npw-pwned-$(touch $T2_TMP/pwned)"
    assert calls(app, "get") == ["odd name", "pwned-$(touch $T2_TMP/pwned)"]
    # eleven and thirty one bytes went in: both names in full, and nothing else on the row
    app.wait_line(0, top=app.top(), cols=len("READYone  ok=11,31"))
    assert "user: me" not in text(app, scrollback=True)


def test_the_palette_command_opens_the_same_picker(app_factory):
    """the bundled "fill password" command in the palette opens the picker too: the command
    palette closes and the entry list opens once the list command answered"""
    app = pw_app(app_factory, {},
                 {"list": ["sh", "-c", "sleep 1; printf 'slow-entry\\n'"],
                  "get": ["sh", "-c", "printf 'from-the-palette\\n'"], "enter": True})
    app.run(READ)
    wait_ready(app)
    app.palette("fill password")
    # the palette is closed while the list command runs, then the picker takes its place
    app.wait(lambda: picker(app) is None, msg="the palette closed after running the command")
    app.wait(lambda: picker(app), msg="the picker of the palette command")
    app.snap("the picker the palette command opened")
    app.key("Return")
    app.wait_file("got", "from-the-palette")


@pytest.mark.parametrize("enter", [True, False], ids=["enter", "no-enter"])
def test_enter_after_the_password_runs_the_program(app_factory, enter):
    """password_manager.enter true sends return right after the password, so the program goes on
    by itself; with it off the password waits in the line until return is pressed by hand"""
    app = pw_app(app_factory, {},
                 {"list": ["sh", "-c", "printf 'entry\\n'"],
                  "get": ["sh", "-c", "printf 'pw-one\\n'"], "enter": enter})
    app.run(READ)
    wait_ready(app)
    open_picker(app)
    app.key("Return")
    if enter:
        app.wait_file("got", "pw-one")
    else:
        nothing_for(2, lambda: app.file("got").exists())
        app.snap("the password typed, the program still waiting for the return key")
        app.key("Return")
        app.wait_file("got", "pw-one")
    assert app.file("got").read_text() == "pw-one"


def test_a_get_that_fails_says_so_and_types_nothing(app_factory):
    """a get command that exits nonzero is told in a notification instead of typing anything; the
    picker still fills a working entry afterwards"""
    app = pw_app(app_factory, {},
                 {"list": ["sh", "-c", "printf 'good\\nbad\\n'"],
                  "get": ["sh", "-c", GET_OWN, "sh", "{name}"], "enter": True})
    app.run(READ)
    wait_ready(app)
    open_picker(app)
    app.type("bad")
    app.key("Return")
    app.wait(lambda: notification_box(app, C), msg="the notification of the failed get")
    nothing_for(1.5, lambda: app.file("got").exists())
    assert "pw-bad" not in text(app, scrollback=True)
    app.snap("the failed get told in a notification, nothing typed into the program")
    open_picker(app)
    app.type("good")
    app.key("Return")
    app.wait_file("got", "pw-good")


def test_a_list_that_fails_says_so_instead_of_a_picker(app_factory):
    """a list command that exits nonzero is told in a notification and no picker opens, so there
    is nothing to pick a password from"""
    app = pw_app(app_factory, {},
                 {"list": ["sh", "-c", "exit 7"], "get": ["sh", "-c", "printf 'pw\\n'"]})
    app.key(FILL)
    app.wait(lambda: notification_box(app, C), msg="the notification of the failed list")
    nothing_for(1, lambda: picker(app))
    app.snap("the failed list told in a notification, no entries listed")


def test_the_picker_warns_when_the_program_shows_what_is_typed(app_factory):
    """a program that reads a line with the terminal echoing it shows the password: the picker says
    so, and for one that reads with echo off it says nothing of the sort; nothing is typed either way"""
    app = pw_app(app_factory, {"pass": PASS}, {"preset": "pass", "enter": True})
    entries(app, ["web/example"])
    # readline keeps echo off and shows the line itself, so even a plain prompt gets no warning
    open_picker(app)
    quiet = app.wait(lambda: placeholder_width(app) or None, msg="the picker's placeholder")
    app.key("Escape")
    app.wait(lambda: picker(app) is None, msg="the picker closed")

    app.run(SHOWING)
    wait_ready(app)
    open_picker(app)
    warn = app.wait(lambda: placeholder_width(app) or None, msg="the picker's placeholder")
    assert warn > quiet + 150, (quiet, warn)
    app.snap("the picker warning that the program would show the password")
    app.key("Escape")
    app.key("ctrl+c")

    app.run(READ)
    wait_ready(app)
    open_picker(app)
    plain = app.wait(lambda: placeholder_width(app) or None, msg="the picker's placeholder")
    assert plain < warn - 150, (plain, warn)
    app.snap("the same picker for a program that turned echo off, no warning")
    app.key("Escape")
    app.key("ctrl+c")
    nothing_for(1, lambda: app.file("got").exists())
    assert "s3cret" not in text(app, scrollback=True)


def test_the_password_reaches_nothing_but_the_program(app_factory):
    """the filled password is not on the screen, in the history, in the clipboard, in the window
    title, in the app's log or in a notification"""
    secret = "l34k-proof"
    app = pw_app(app_factory, {"pass": PASS.replace("s3cret!x", secret)},
                 {"preset": "pass", "enter": True})
    entries(app, ["web/example"])
    set_clipboard("before the fill")
    app.wait(lambda: get_clipboard() == "before the fill", msg="the clipboard holds its text")
    app.run(READ)
    wait_ready(app)
    open_picker(app)
    app.key("Return")
    app.wait_file("got", secret)
    assert app.file("got").read_text() == secret
    app.wait_line(0, top=app.top(), cols=len(f"READY ok={len(secret)}"))
    assert secret not in text(app, scrollback=True)
    assert secret[:4] not in app.title()
    assert secret[:4] not in app.output()
    assert get_clipboard() == "before the fill"
    nothing_for(1, lambda: notification_box(app, C))
    app.snap("the password went into the program, and nowhere else")


def test_show_hidden_text_paints_what_a_program_concealed(app_factory):
    """text a program concealed (sgr 8) is not painted at all until the show_hidden_text action,
    which paints it and hides it again"""
    app = pw_app(app_factory, {}, {"preset": "pass"})
    app.run("clear; printf 'RE'; printf 'ADY'; printf '\\033[8mTOPSECRET\\033[0m'; "
            "printf '\\n\\n\\n'; sleep 300")
    app.wait(lambda: "READY" in text(app), msg="the concealed line printed")
    # only the five letters of READY are painted, the secret after them leaves no ink
    app.wait(lambda: row_columns(app, 0) == 5, msg="only the marker painted, nothing of the secret")
    app.snap("a concealed word, none of it painted")
    app.key(REVEAL)
    app.wait(lambda: row_columns(app, 0) == 14, msg="the concealed text painted")
    app.snap("the same line with the concealed text asked for")
    app.key(REVEAL)
    app.wait(lambda: row_columns(app, 0) == 5, msg="the concealed text hidden again")
    app.key("ctrl+c")


def test_remote_control_is_never_given_concealed_text(app_factory):
    """what a program concealed is left out of the pane text and of a block, which says it held
    hidden text instead, whatever the show_hidden_text toggle is set to"""
    app = pw_app(app_factory, {}, {"preset": "pass"})
    app.run("printf 'note: \\033[8mhidden-part\\033[0m done\\n'")
    block = wait_block(app, "printf 'note:")
    assert block["hidden"] is True, block
    assert "note:" in block["output"] and "hidden-part" not in block["output"], block
    # the line the program printed comes back with the concealed word left out of it
    line = next(row for row in text(app, scrollback=True).splitlines() if row.startswith("note:"))
    assert "hidden-part" not in line, line
    app.key(REVEAL)
    assert wait_block(app, "printf 'note:")["hidden"] is True
    assert "hidden-part" not in wait_block(app, "printf 'note:")["output"]
    app.snap("a concealed word painted, still left out of what remote control reads")


def test_invisible_characters_are_marked_hidden_and_left_out(app_factory):
    """a zero width space a program printed leaves no mark on the screen, is left out of what
    remote control reads, and makes the block say it held text people do not see"""
    app = pw_app(app_factory, {}, {"preset": "pass"})
    app.run("printf 'ok\\342\\200\\213done\\n'")
    block = wait_block(app, "printf 'ok")
    assert block["hidden"] is True, block
    assert "okdone" in block["output"] and "\u200b" not in block["output"], block
    assert "okdone" in text(app, scrollback=True)
    assert "\u200b" not in text(app, scrollback=True)
    app.snap("a line with a zero width space, nothing of it handed out")


@pytest.mark.parametrize("check", [True, False], ids=["paste-check-on", "paste-check-off"])
def test_a_password_with_an_invisible_character_keeps_it(app_factory, check):
    """terminal.paste_check_invisible on or off: a filled password is not paste checked, its zero
    width space reaches the program and no dialog comes up for it"""
    secret = "pa\u200bss"
    app = pw_app(app_factory, {},
                 {"list": ["sh", "-c", "printf 'entry\\n'"],
                  "get": ["sh", "-c", "printf 'pa\u200bss\\n'"], "enter": False},
                 terminal={"paste_check_invisible": check})
    app.run(RAW.format(n=len(secret.encode())))
    wait_ready(app)
    open_picker(app)
    app.key("Return")
    app.wait_file("got", secret)
    assert app.file("got").read_text() == secret
    # the fill never goes through the paste check, so no dialog takes the picker's place
    app.wait(lambda: picker(app) is None, msg="the picker closed without asking about the paste")
    app.wait_line(0, top=app.top(), cols=len(f"READY ok={len(secret.encode())}"))
    app.snap("a password with a zero width space typed as it is")


def test_fill_password_in_an_incognito_tab(app_factory):
    """an incognito tab takes the password like any other one, while remote control still cannot
    read a thing of it"""
    app = pw_app(app_factory, {"pass": PASS}, {"preset": "pass", "enter": True})
    entries(app, ["web/example"])
    app.palette("new incognito tab")
    app.wait_title(lambda title: title.startswith("2 "))
    pane = str(next(t["id"] for t in tabs(app) if t["active"]))
    send(app, "touch $T2_TMP/private_ready", pane)
    app.wait_file("private_ready")
    before = app.title()
    send(app, READ_CHILD, pane)
    # reading the tab is refused, so the lock it puts in the title is what says a secret is read
    app.wait(lambda: app.title() if app.title() != before else None, msg="the lock while the "
             "secret is read", timeout=15)
    app.snap("an incognito tab asking for a secret, its lock in the title")
    open_picker(app)
    app.key("Return")
    app.wait_file("got", "s3cret!x")
    assert app.file("got").read_text() == "s3cret!x"
    app.wait_title(lambda title: title == before, timeout=15)
    done = msg(app, "get-text", "--pane", pane)
    assert done.returncode != 0 and "incognito" in done.stderr + done.stdout, done
    # sending to it still works, only reading is refused
    send(app, "touch $T2_TMP/typed", pane)
    app.wait(app.file("typed").exists, msg="a command sent to the incognito tab")
    app.snap("the password went into the private tab, which still reads nowhere")


def test_fill_password_in_a_sandboxed_tab(app_factory):
    """the password manager runs outside the sandbox and the password still reaches the program
    asking for it inside a sandboxed profile"""
    box = {"name": "box", "command": {"program": "/bin/bash"}, "env": {"PS1": "$ "},
           "working_directory": "/tmp",
           "sandbox": {"network": False, "write": ["cwd"], "tmp": "shared"}}
    app = pw_app(app_factory, {"pass": PASS, "bwrap": BWRAP},
                 {"preset": "pass", "enter": True}, profiles=[box])
    entries(app, ["web/example"])
    assert msg(app, "new-tab", "--profile", "box").returncode == 0
    app.wait_title(lambda title: title.startswith("2 "))
    app.wait_file("bwrap_args")
    pane = str(next(t["id"] for t in tabs(app) if t["active"]))
    send(app, "touch $T2_TMP/box_ready", pane)
    app.wait_file("box_ready")
    app.run(READ)
    wait_ready(app)
    open_picker(app)
    app.key("Return")
    app.wait_file("got", "s3cret!x")
    assert calls(app, "pass") == ["show web/example"]
    app.snap("the password filled into a program inside the sandbox")


def test_a_reloaded_password_manager_is_used_for_the_next_fill(app_factory):
    """saving settings.jsonc with another password_manager applies without a restart: the next
    fill runs the new list and get commands"""
    app = pw_app(app_factory, {},
                 {"list": ["sh", "-c", "printf 'one\\n'"],
                  "get": ["sh", "-c", "printf 'first-pass\\n'"], "enter": True})
    app.run(READ)
    wait_ready(app)
    open_picker(app)
    app.key("Return")
    app.wait_file("got", "first-pass")
    app.file("got").unlink()
    app.write("settings.jsonc", {**app.settings,
                                 "window_title": ["number", {"text": " reloaded "}, "icon"],
                                 "password_manager": {"list": ["sh", "-c", "printf 'two\\n'"],
                                                      "get": ["sh", "-c", "printf 'second-pass\\n'"],
                                                      "enter": True}})
    app.wait(lambda: "reloaded" in app.title(), msg="the saved settings applied")
    app.run(READ)
    wait_ready(app)
    open_picker(app)
    assert listed(app) == 1
    app.key("Return")
    app.wait_file("got", "second-pass")
    app.snap("the password of the reloaded manager")
    assert len(calls(app, "pass")) == 0, "no password manager of the old preset ran"
