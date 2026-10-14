"""single file html report: every tested feature with its config state and screenshots"""

import base64
import html
import json
import re
import time
from dataclasses import dataclass, field
from pathlib import Path

from harness import BACKEND, BIN, SRC


@dataclass
class Entry:
    feature: str
    test: str
    description: str
    params: dict
    # apps while the test runs, frozen to their config state once closed
    states: list = field(default_factory=list)
    shots: list = field(default_factory=list)
    status: str = "not run"
    error: str = ""
    logs: list = field(default_factory=list)
    # (title, text) shown as is, for cases without a window
    notes: list = field(default_factory=list)
    duration: float = 0.0
    # place in the collected tests, workers of a parallel run report out of order
    order: int = 0

    def freeze(self, app) -> None:
        """keep only what the app was started with, the app itself goes away"""
        state = {"files": dict(app.written)}
        if app.args:
            state["command line"] = "kuterm " + " ".join(app.args)
        if app.env_extra:
            state["environment"] = json.dumps(app.env_extra, indent=2)
        self.states = [state if s is app else s for s in self.states]


class Report:
    def __init__(self) -> None:
        self.entries: dict = {}
        self.started = time.time()
        # node id to its place in the collected tests
        self.order: dict = {}

    def entry(self, item) -> Entry:
        """report entry of a test item, created on first use"""
        if item.nodeid not in self.entries:
            doc = (getattr(item, "function", None).__doc__ or "").strip()
            module = item.module
            feature = getattr(module, "FEATURE", None) or (module.__doc__ or item.module.__name__).split("\n")[0]
            params = dict(getattr(getattr(item, "callspec", None), "params", {}))
            order = self.order.get(item.nodeid, len(self.entries))
            self.entries[item.nodeid] = Entry(feature, item.name, doc, params, order=order)
        return self.entries[item.nodeid]

    def record(self, item, rep) -> None:
        """status of the test, a failure in setup or teardown counts too"""
        entry = self.entry(item)
        entry.duration += rep.duration
        if rep.when == "call" or rep.outcome != "passed":
            if entry.status in ("not run", "passed"):
                entry.status = "skipped" if rep.skipped else rep.outcome
            if rep.failed:
                entry.error += f"[{rep.when}] {rep.longreprtext}\n"
            elif rep.skipped and isinstance(rep.longrepr, tuple):
                entry.error = rep.longrepr[2]

    def dump(self, path: Path) -> None:
        """save the entries of a parallel worker for the controller to merge"""
        entries = {}
        for nodeid, entry in self.entries.items():
            data = dict(vars(entry))
            # apps not frozen yet are gone with the worker
            data["states"] = [s for s in entry.states if isinstance(s, dict)]
            data["shots"] = [(caption, str(shot)) for caption, shot in entry.shots]
            entries[nodeid] = data
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(entries))

    def load(self, path: Path) -> None:
        """add the entries a worker saved with dump()"""
        for nodeid, data in json.loads(path.read_text()).items():
            data["shots"] = [tuple(shot) for shot in data["shots"]]
            self.entries[nodeid] = Entry(**data)

    def write(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(self.render())

    def render(self) -> str:
        entries = sorted(self.entries.values(), key=lambda e: e.order)
        features: dict = {}
        for entry in entries:
            features.setdefault(entry.feature, []).append(entry)
        totals = {s: sum(e.status == s for e in entries) for s in ("passed", "failed", "skipped")}
        version = re.search(r'^version = "(.+)"', (SRC / "Cargo.toml").read_text(), re.M)
        version = f"v{version[1]}" if version else ""
        out = [HEAD]
        out.append(f"<h1>kuterm e2e report ({BACKEND})</h1>")
        out.append(f"<p>{time.strftime('%Y-%m-%d %H:%M:%S UTC', time.gmtime(self.started))}"
                   f" &middot; binary {html.escape(str(BIN))} &middot; {html.escape(version)}"
                   f" &middot; backend {BACKEND}</p>")
        out.append("<p>" + " ".join(
            f"<span class='{s}'>{n} {s}</span>" for s, n in totals.items()) + "</p>")
        out.append("<h2>features</h2><table><tr><th>feature</th><th>cases</th><th>result</th></tr>")
        for i, (feature, items) in enumerate(features.items()):
            failed = sum(e.status == "failed" for e in items)
            status = "failed" if failed else "passed"
            out.append(f"<tr><td><a href='#f{i}'>{html.escape(feature)}</a></td><td>{len(items)}</td>"
                       f"<td><span class='{status}'>{f'{failed} failed' if failed else 'ok'}</span></td></tr>")
        out.append("</table>")
        for i, (feature, items) in enumerate(features.items()):
            out.append(f"<h2 id='f{i}'>{html.escape(feature)}</h2>")
            out.extend(self.render_entry(entry) for entry in items)
        out.append("</body></html>")
        return "\n".join(out)

    def render_entry(self, entry: Entry) -> str:
        out = ["<div class=case>"]
        out.append(f"<h3><span class='{entry.status}'>{entry.status}</span> {html.escape(entry.test)}"
                   f" <small>{entry.duration:.1f}s</small></h3>")
        if entry.description:
            out.append(f"<p>{html.escape(entry.description)}</p>")
        if entry.params:
            params = ", ".join(f"{k} = {json.dumps(v, ensure_ascii=False)}" for k, v in entry.params.items())
            out.append(f"<p>{html.escape(params)}</p>")
        for n, state in enumerate(s for s in entry.states if isinstance(s, dict)):
            label = f"app {n + 1} state" if len(entry.states) > 1 else "state"
            parts = [f"<b>{html.escape(k)}</b><pre>{html.escape(v)}</pre>"
                     for k, v in state.items() if k != "files"]
            parts += [f"<b>{html.escape(name)}</b><pre>{html.escape(text)}</pre>"
                      for name, text in state["files"].items()]
            if not parts:
                parts = ["<i>no config files, bundled defaults</i>"]
            out.append(f"<details><summary>{label}</summary>{''.join(parts)}</details>")
        for title, text in entry.notes:
            out.append(f"<b>{html.escape(title)}</b><pre>{html.escape(text)}</pre>")
        if entry.error:
            out.append(f"<pre>{html.escape(entry.error[-4000:])}</pre>")
        for log in entry.logs:
            if log.strip():
                out.append(f"<details><summary>app output</summary><pre>{html.escape(log)}</pre></details>")
        for caption, path in entry.shots:
            data = base64.b64encode(Path(path).read_bytes()).decode()
            out.append(f"<figure><img loading=lazy src='data:image/png;base64,{data}'>"
                       f"<figcaption>{html.escape(caption)}</figcaption></figure>")
        out.append("</div>")
        return "\n".join(out)


HEAD = """<!doctype html><html><head><meta charset=utf-8><title>kuterm e2e report</title><style>
body { font: 14px sans-serif; margin: 1em; }
.case { border-top: 1px solid #ccc; padding: .5em 0; }
.passed { color: green; } .failed { color: red; } .skipped { color: orange; }
pre { background: #f3f3f3; padding: .5em; overflow-x: auto; }
figure { display: inline-block; margin: 0 1em 1em 0; }
figure img { width: 450px; border: 1px solid #ccc; }
td, th { padding: 0 1em; text-align: left; }
</style></head><body>"""
