"""session restore: tabs come back on the next launch, each in the folder its shell was in"""

from harness import App

FEATURE = "session restore"


def test_tabs_and_folders_come_back(app_factory):
    """with restore_session "always", quitting and launching again opens the same tabs"""
    app: App = app_factory({"theme": {"mode": "dark"}, "restore_session": "always", "confirm_quit": "never",
                            "window_title": ["number", {"text": " "}, "folder"]})
    app.run("cd /usr/share")
    app.wait_title("1 share")
    app.key("ctrl+shift+t")
    app.wait_title("2 ~")
    app.run("cd /etc")
    app.wait_title("2 etc")
    app.palette("quit")
    app.wait(lambda: not app.alive(), msg="app quit")
    app.start()
    app.wait_title("2 etc")
    app.snap("both tabs restored, the second one in /etc")
    app.key("alt+1")
    app.wait_title("1 share")
