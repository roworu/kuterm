"""title blocks added lately, read back from the window title and from list-tabs: kube, aws and
venv contexts, git, memory with its unit, network, totals, size, clock format, queue, xoff and
stopped jobs, and how they combine"""

import re
import time

import pytest

from harness import App
from kit import SOCKET, integrated, listening, msg, nothing_for, ready, send, tabs

FEATURE = "title blocks"


def titled(app_factory, blocks, **kwargs) -> App:
    settings = {**SOCKET, "window_title": blocks, **kwargs.pop("settings", {})}
    return app_factory(settings, **kwargs)


KUBECONFIG = "apiVersion: v1\nkind: Config\ncontexts:\n- name: prod\ncurrent-context: prod\n"


def test_kube_context_of_the_program_in_front(app_factory):
    """"kube" reads the current context of the kubeconfig the program in front uses"""
    app = titled(app_factory, [{"text": "["}, "kube", {"text": "]"}])
    app.file("kc").write_text(KUBECONFIG)
    app.wait_title("[]")
    app.run("KUBECONFIG=$T2_TMP/kc sleep 60")
    app.wait_title("[prod]")
    app.snap("kube context of the sleeping program")
    app.key("ctrl+c")
    app.wait_title("[]")


def test_kube_context_from_the_home_kubeconfig(app_factory):
    """without $KUBECONFIG the context comes from ~/.kube/config"""
    app = titled(app_factory, [{"text": "<"}, "kube", {"text": ">"}])
    (app.home / ".kube").mkdir()
    (app.home / ".kube/config").write_text(KUBECONFIG.replace("prod", "home-ctx"))
    app.run("sleep 60")
    app.wait_title("<home-ctx>")


@pytest.mark.parametrize("env,expected", [
    ("AWS_PROFILE=dev", "dev"),
    ("AWS_DEFAULT_PROFILE=ci", "ci"),
    ("AWS_PROFILE=first AWS_DEFAULT_PROFILE=second", "first"),
])
def test_aws_profile(app_factory, env, expected):
    app = titled(app_factory, [{"text": "aws:"}, "aws"])
    app.run(f"{env} sleep 60")
    app.wait_title(f"aws:{expected}")


@pytest.mark.parametrize("env,expected", [
    ("VIRTUAL_ENV=/x/venv", "venv"),
    ("VIRTUAL_ENV=/x/.venv VIRTUAL_ENV_PROMPT='(myproj) '", "myproj"),
    ("CONDA_DEFAULT_ENV=base", "base"),
])
def test_venv_names(app_factory, env, expected):
    app = titled(app_factory, [{"text": "py:"}, "venv"])
    app.run(f"{env} sleep 60")
    app.wait_title(f"py:{expected}")


def test_all_contexts_together_and_empty_when_unknown(app_factory):
    """the three blocks side by side, each empty when nothing is known"""
    app = titled(app_factory, ["kube", {"text": "|"}, "aws", {"text": "|"}, "venv"])
    app.file("kc").write_text(KUBECONFIG)
    app.wait_title("||")
    app.run("KUBECONFIG=$T2_TMP/kc AWS_PROFILE=dev VIRTUAL_ENV=/x/venv sleep 60")
    app.wait_title("prod|dev|venv")
    app.key("ctrl+c")
    app.run("AWS_PROFILE=only sleep 60")
    app.wait_title("|only|")


def test_background_tabs_keep_the_context_read_while_shown(app_factory):
    """only shown tabs read their context again, a tab sent to the background keeps its title"""
    app = app_factory({**SOCKET, "hide_bar_for_one_tab": False, "tab_title": ["number", {"text": ":"}, "aws"]})
    listening(app)
    app.run("AWS_PROFILE=bg sleep 60")
    app.wait(lambda: [t["title"] for t in tabs(app)] == ["1:bg"], msg="context of the shown tab")
    app.key("ctrl+shift+t")
    app.wait(lambda: [t["title"] for t in tabs(app)] == ["1:bg", "2:"], msg="titles of both tabs")
    nothing_for(2, lambda: [t["title"] for t in tabs(app)][0] != "1:bg")


def git(app: App, *args: str) -> str:
    return f"git -c user.email=e2e@x -c user.name=e2e -c init.defaultBranch=main {' '.join(args)}"


def test_git_branch_dirty_ahead_and_behind(app_factory):
    """"git" shows the branch, * for changes and arrows for commits ahead of and behind upstream"""
    app = titled(app_factory, ["git"])
    app.run(f"cd $T2_TMP && {git(app, 'init -q --bare origin.git')} && {git(app, 'clone -q origin.git work')}"
            f" && cd work && {git(app, 'commit -q --allow-empty -m one')} && {git(app, 'push -q origin main')}")
    app.wait_title("\ue0a0 main", timeout=20)
    app.run("touch new_file")
    app.wait_title("\ue0a0 main*", timeout=15)
    app.run(f"rm new_file && {git(app, 'commit -q --allow-empty -m two')}")
    app.wait_title("\ue0a0 main ↑1", timeout=15)
    app.snap("one commit ahead of its upstream")
    # someone else pushed, after a fetch the branch is behind too
    app.run(f"(cd $T2_TMP && {git(app, 'clone -q origin.git other')} && cd other && "
            f"{git(app, 'commit -q --allow-empty -m three')} && {git(app, 'push -q origin main')}) && "
            f"{git(app, 'fetch -q')}")
    app.wait_title("\ue0a0 main ↑1 ↓1", timeout=15)


def test_git_block_is_empty_outside_a_repository(app_factory):
    app = titled(app_factory, [{"text": "<"}, "git", {"text": ">"}])
    app.wait_title("<>")


@pytest.mark.parametrize("unit,pattern", [
    ("auto", r"\d+(\.\d+)? (KB|MB|GB)"),
    ("kb", r"\d+ KB"),
    ("mb", r"\d+(\.\d+)? MB"),
    ("gb", r"0\.\d\d GB"),
])
def test_memory_unit(app_factory, unit, pattern):
    """"memory" is written in "status_bar.memory_unit", also in titles"""
    app = titled(app_factory, ["memory"], settings={"status_bar": {"memory_unit": unit}})
    app.wait_title(lambda title: re.fullmatch(pattern, title), timeout=15)


def test_click_on_memory_switches_the_unit_and_remembers_it(app_factory):
    """a click on the memory block of the status bar goes auto -> kb -> mb, the next start keeps it"""
    settings = {**SOCKET, "window_title": ["memory"],
                "status_bar": {"enable": True, "left": ["memory"], "right": []}}
    app = app_factory(settings)
    app.wait_title(lambda title: re.fullmatch(r"\d+(\.\d+)? MB", title), timeout=15)
    # the status bar is the bottom row, its text starts at the left
    w, h = app.size()
    bar = round(round(1.5 * app.ui_font_size()) * app.scale)
    app.click(20, h - bar // 2)
    app.wait_title(lambda title: title.endswith(" KB"), timeout=15)
    app.click(20, h - bar // 2)
    app.wait_title(lambda title: title.endswith(" MB"), timeout=15)
    app.close()
    app.start()
    app.wait_title(lambda title: title.endswith(" MB"), timeout=15)


def test_cpu_network_and_totals(app_factory):
    """cpu, network and the totals of every tab"""
    app = titled(app_factory, ["cpu", {"text": "|"}, "network", {"text": "|"}, "total_cpu", {"text": "|"},
                               "total_memory", {"text": "|"}, "total_network"])
    pattern = r"\d+%\|↓\d+ B/s ↑\d+ B/s\|\d+%\|\d+(\.\d+)? [KMG]B\|↓[\d.]+ [KMG]?B/s ↑[\d.]+ [KMG]?B/s"
    app.wait_title(lambda title: re.fullmatch(pattern, title), timeout=15)


def test_ports_of_this_tab_and_of_all_tabs(app_factory):
    """"ports" lists the ports of the tab, "all_ports" those of every tab"""
    app = titled(app_factory, ["ports", {"text": "|"}, "all_ports"],
                 settings={"hide_bar_for_one_tab": False})
    app.run("python3 -m http.server 18431 --bind 127.0.0.1")
    app.wait_title(":18431|:18431", timeout=20)
    app.key("ctrl+shift+t")
    app.wait_title("|:18431", timeout=20)
    app.run("python3 -m http.server 18432 --bind 127.0.0.1")
    app.wait_title(":18432|:18431 :18432", timeout=20)


def test_size_follows_the_window(app_factory):
    app = titled(app_factory, ["size"])
    first = app.wait_title(lambda title: re.fullmatch(r"\d+×\d+", title))
    app.resize(600, 400)
    app.wait_title(lambda title: re.fullmatch(r"\d+×\d+", title) and title != first)


# formats are unit tested, this one mixes text and a field
@pytest.mark.parametrize("fmt,pattern", [("clock %a", r"clock \w{3}")])
def test_clock_format(app_factory, fmt, pattern):
    app = titled(app_factory, ["clock"], settings={"status_bar": {"clock_format": fmt}})
    app.wait_title(lambda title: re.fullmatch(pattern, title))


def test_queue_xoff_and_stopped_jobs_blocks(app_factory):
    """the queue count, "paused" while ctrl+s holds output, stopped jobs by name"""
    app = app_factory(integrated(window_title=["queue", {"text": "|"}, "xoff", {"text": "|"},
                                               "stopped_jobs"]))
    ready(app)
    app.wait_title("||")
    send(app, "sleep 4")
    assert msg(app, "queue", "true").returncode == 0
    assert msg(app, "queue", "true").returncode == 0
    app.wait_title("⧗ 2||")
    # both run once the sleep ends
    app.wait_title("||", timeout=20)
    app.run("stty ixon; sleep 60")
    app.wait_title("||")
    app.key("ctrl+z")
    app.wait_title("||sleep, stopped", timeout=15)
    app.snap("a stopped sleep in the title")
    app.run("kill %1")
    app.wait_title("||", timeout=15)
    app.run("while :; do echo tick; sleep 0.2; done")
    time.sleep(0.5)
    app.key("ctrl+s")
    app.wait_title("|paused (ctrl+q)|")
    app.key("ctrl+q")
    app.wait_title("||")
