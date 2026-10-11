"""notifications inside the window: shown by commands, timeout, kept until clicked, disabled"""

import time

from harness import App, as_rgb, mask_bbox, near, unique_theme

FEATURE = "notifications"

THEME = unique_theme(0xd0)
C = as_rgb(THEME)


def notify_app(app_factory, **notifications) -> App:
    app = app_factory({"theme": {"mode": "dark"}, "notifications": notifications},
                      themes={"dark": THEME},
                      commands={"commands": [{"name": "say hi", "actions": [{"notify": "hi from e2e"}]},
                                             {"name": "say many", "actions": [{"notify": f"note {i}"}
                                                                              for i in range(7)]}]})
    app.mouse(100, 100)
    return app


def notes(app: App) -> tuple | None:
    """bbox of notifications in the bottom right corner"""
    mask = near(app.shot()[300:, 400:], C["tab_bar_background"])
    box = mask_bbox(mask)
    return None if mask.sum() < 300 else (box[0] + 400, box[1] + 300, box[2] + 400, box[3] + 300)


def test_notification_shows_and_times_out(app_factory):
    """a notify action shows a notification in the bottom right corner until its timeout"""
    app = notify_app(app_factory, timeout=2)
    app.palette("say hi")
    box = app.wait(lambda: notes(app), msg="notification")
    assert box[2] >= 880 and box[3] >= 580, box
    app.snap("notification shown")
    app.wait(lambda: notes(app) is None, msg="notification timed out", timeout=5)


def test_zero_timeout_keeps_until_clicked(app_factory):
    """timeout 0 keeps a notification until it is clicked"""
    app = notify_app(app_factory, timeout=0)
    app.palette("say hi")
    x0, y0, x1, y1 = app.wait(lambda: notes(app), msg="notification")
    time.sleep(3)
    assert notes(app) is not None
    app.click((x0 + x1) // 2, (y0 + y1) // 2)
    app.wait(lambda: notes(app) is None, msg="notification closed by click")


def test_notifications_stack_up_to_five(app_factory):
    """notifications stack upwards, only the newest five stay"""
    app = notify_app(app_factory, timeout=0)
    app.palette("say hi")
    one = app.wait(lambda: notes(app), msg="one notification")
    app.palette("say many")
    app.wait(lambda: notes(app) and notes(app)[1] < one[1] - 100, msg="stacked notifications")
    app.snap("newest five notifications")
    height = notes(app)[3] - notes(app)[1]
    single = one[3] - one[1]
    # five notifications with gaps, not the eight that were sent
    assert single * 4.5 < height < single * 6.5, (height, single)


def test_disabled_notifications_show_nothing(app_factory):
    """notifications.enable false shows nothing"""
    app = notify_app(app_factory, enable=False)
    app.palette("say hi")
    time.sleep(1.5)
    assert notes(app) is None
