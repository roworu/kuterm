"""autocomplete: zsh reports its command line, a popup of completions shows under the typed
word, tab puts the picked one into the shell's line and enter runs it"""

from harness import App, as_rgb, dominant, ink_bbox, mask_bbox, near, unique_theme

FEATURE = "autocomplete"

THEME = unique_theme(0x6a)
C = as_rgb(THEME)


def line_cells(app: App) -> int:
    """cells the first row has text over"""
    x0, y0, x1, y1 = app.cell_rect(0, 0, cols=60)
    band = app.shot()[y0:y1, x0:x1]
    box = ink_bbox(band, dominant(band))
    return 0 if box is None else round(box[2] / app.cell_width())


def popup(app: App):
    """bbox of the popup below the first row, none while it is hidden"""
    below = app.cell_rect(0, 1)[1]
    img = app.shot()[below:]
    mask = near(img, C["tab_bar_background"]) | near(img, C["tab_active_background"])
    if mask.sum() < 500:
        return None
    x0, y0, x1, y1 = mask_bbox(mask)
    return x0, y0 + below, x1, y1 + below


def zsh_app(app_factory, tmp_path, zshrc="", **autocomplete) -> App:
    # a .zshrc keeps zsh's first run wizard away, and tells when the prompt is up
    zdotdir = tmp_path / "zdotdir"
    zdotdir.mkdir()
    ready = tmp_path / "prompt"
    (zdotdir / ".zshrc").write_text(
        f"PS1='$ '\nunsetopt prompt_sp\nprecmd() {{ : > {ready} }}\n{zshrc}\n")
    work = tmp_path / "work"
    (work / "alpine").mkdir(parents=True)
    (work / "alpha_file.txt").write_text("")
    profile = {"name": "zsh", "command": {"program": "/usr/bin/zsh"},
               "working_directory": str(work), "env": {"ZDOTDIR": str(zdotdir)}}
    app = app_factory({"theme": {"mode": "dark"}, "profiles": [profile],
                       "autocomplete": {"enable": True, **autocomplete}}, themes={"dark": THEME})
    app.wait(ready.exists, msg="zsh prompt")
    return app


def test_popup_shows_and_tab_completes_into_the_line(app_factory, tmp_path):
    """typing shows the files that fit under the word, tab puts the picked one in the line"""
    app = zsh_app(app_factory, tmp_path)
    app.type("ls alp")
    box = app.wait(lambda: popup(app), msg="completion popup")
    # it starts under the typed word, past "$ ls "
    assert box[0] >= app.cell_rect(5, 0)[0] - 8, box
    app.snap("popup with a folder and a file for 'ls alp'")
    before = app.shot()
    app.key("Down")
    app.wait(lambda: (app.shot() != before).any(), msg="selection moved")
    app.key("Tab")
    # the shell's own line now holds the file, so running it lists the file
    # "$ ls alpha_file.txt" with the cursor after it
    app.wait(lambda: line_cells(app) >= len("$ ls alpha_file.txt"), msg="completed line")
    app.snap("tab put alpha_file.txt into the line")
    app.key("Return")
    app.wait_line(1, len("alpha_file.txt"))
    app.wait(lambda: popup(app) is None, msg="popup gone after enter")




def test_tab_right_after_typing_completes(app_factory, tmp_path):
    """tab typed before the popup shows still takes the first completion, with zsh options
    like ksh_arrays that change how scripts run"""
    app = zsh_app(app_factory, tmp_path, zshrc="setopt ksh_arrays sh_word_split")
    # candidates of a shorter word would put the folder in, "alph" only fits the file
    app.type("ls alph")
    app.key("Tab")
    app.wait(lambda: line_cells(app) >= len("$ ls alpha_file.txt"), msg="completed line")
    app.snap("tab right after typing put alpha_file.txt into the line")
    app.key("Return")
    app.wait_line(1, len("alpha_file.txt"))
