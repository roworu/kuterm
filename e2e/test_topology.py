"""pane topology: a map of the panes whose programs talk to each other over local tcp or a unix
socket pair, a click on a pane goes there, escape closes"""

import time

import numpy as np

from harness import App, as_rgb, near, unique_theme
from kit import SOCKET, listening, msg, tabs

FEATURE = "pane topology"

THEME = unique_theme(0x9b)
C = as_rgb(THEME)

# keeps the one connection open and reads it, http.server would close it after each request
SERVER = ("python3 -c \"import socket; s = socket.socket(); "
          "s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(('127.0.0.1', {port})); "
          "s.listen(); c, _ = s.accept(); [c.recv(4096) for _ in iter(int, 1)]\"")
CLIENT = ("python3 -c \"import socket, time; s = socket.create_connection(('127.0.0.1', {port})); "
          "[time.sleep(0.3) or s.send(b'GET / HTTP/1.1\\r\\nHost: x\\r\\n\\r\\n') for _ in range(2000)]\"")
UNIX_SERVER = ("python3 -c \"import socket, os, time; p = os.environ['T2_TMP'] + '/sock'; "
               "s = socket.socket(socket.AF_UNIX); s.bind(p); s.listen(); c, _ = s.accept(); time.sleep(600)\"")
UNIX_CLIENT = ("python3 -c \"import socket, os, time; s = socket.socket(socket.AF_UNIX); "
               "s.connect(os.environ['T2_TMP'] + '/sock'); time.sleep(600)\"")


def two_tabs(app_factory, first: str, second: str) -> App:
    app = app_factory({**SOCKET, "hide_bar_for_one_tab": False, "window_title": ["number"]},
                      themes={"dark": THEME})
    listening(app)
    app.run(first)
    time.sleep(1)
    app.key("ctrl+shift+t")
    app.wait_title("2")
    app.run(second)
    return app


def line_pixels(app: App) -> int:
    """pixels of a drawn edge in the middle of the map, between the two nodes"""
    img = app.shot()
    h, w = img.shape[:2]
    # two nodes sit on the circle above and below the center, the edge runs between them
    band = img[h * 2 // 5 : h * 3 // 5, w // 2 - 6 : w // 2 + 6].astype(int)
    background = np.median(band.reshape(-1, 3), axis=0)
    # the line is thin and smoothed, so anything clearly off the background counts
    return int((np.abs(band - background).sum(axis=2) > 40).sum())


def nodes(app: App) -> int:
    """pixels of node boxes, in the tab bar color below the tab bar"""
    return int(near(app.shot()[app.bar_height() + 4 :], C["tab_bar_background"]).sum())


def open_map(app: App) -> None:
    app.mouse(5, 5)
    app.palette("show pane topology")
    app.wait(lambda: nodes(app) > 1000, msg="the map")


def test_a_client_and_its_server_are_joined(app_factory):
    app = two_tabs(app_factory, SERVER.format(port=18461), CLIENT.format(port=18461))
    open_map(app)
    app.wait(lambda: line_pixels(app) > 20, msg="an edge between the server and the client", timeout=20)
    app.snap("the client tab joined to the server tab, pulsing while they talk")


def test_unix_socket_pairs_are_joined(app_factory):
    app = two_tabs(app_factory, UNIX_SERVER, UNIX_CLIENT)
    open_map(app)
    app.wait(lambda: line_pixels(app) > 20, msg="an edge for the unix socket pair", timeout=20)


def test_unrelated_tabs_are_not_joined(app_factory):
    app = two_tabs(app_factory, SERVER.format(port=18462), "sleep 600")
    open_map(app)
    time.sleep(3)
    assert line_pixels(app) == 0
    app.snap("two tabs that don't talk, no edge")


def test_clicking_a_node_goes_to_its_tab(app_factory):
    app = two_tabs(app_factory, "sleep 600", "sleep 600")
    app.key("ctrl+Tab")
    app.wait_title("1")
    open_map(app)
    w, h = app.size()
    # the second node sits at the bottom of the circle
    mask = near(app.shot()[h // 2 : h - 30], C["tab_bar_background"])
    ys, xs = np.nonzero(mask)
    app.click(int(xs.mean()), h // 2 + int(ys.mean()))
    app.wait_title("2")
    app.wait(lambda: nodes(app) < 1000, msg="map closed")


def test_escape_closes_the_map(app_factory):
    app = two_tabs(app_factory, "sleep 600", "sleep 600")
    open_map(app)
    app.key("Escape")
    app.wait(lambda: nodes(app) < 1000, msg="map closed")
    assert len(tabs(app)) == 2 and msg(app, "list-tabs").returncode == 0
