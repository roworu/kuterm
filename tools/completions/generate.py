"""recipes for the completion specs kuterm brings, by the container image their programs are
installed in. generate.sh runs this inside that image:

    python3 generate.py <image> [command...]

writes <command>.json for every recipe of the image (or only the named ones) into /out"""

import json
import sys
from pathlib import Path

from helpspec import options, run, spec

OUT = Path("/out")

FILES = {"name": "file", "template": "filepaths", "isVariadic": True, "isOptional": True}
FILE = {"name": "file", "template": "filepaths"}
DIRS = {"name": "directory", "template": "folders", "isVariadic": True}
PACKAGES = {"name": "package", "isVariadic": True}
PATTERN = {"name": "pattern"}
UNITS = {"name": "unit", "isVariadic": True}
COLOR = {"args": {"name": "when", "isOptional": True, "suggestions": ["always", "auto", "never"]}}
SIGNALS = ["HUP", "INT", "QUIT", "KILL", "USR1", "USR2", "TERM", "CONT", "STOP"]


def simple(name, description, args=FILES, **kwargs):
    return lambda: spec(name, description, args=args, **kwargs)


def apt_like(name, description, extra_options):
    return lambda: spec(
        name,
        description,
        sections=r"commands",
        top=("install", "remove", "update", "upgrade", "search", "show", "list", "purge", "autoremove"),
        sub_args={sub: PACKAGES for sub in ("install", "reinstall", "remove", "purge", "show", "source",
                                            "download", "changelog", "build-dep", "depends", "rdepends",
                                            "policy", "showpkg", "showsrc")} | {"search": PATTERN},
        extra_options=extra_options,
        persistent=True,
    )


APT_OPTIONS = [
    {"name": ["-y", "--yes", "--assume-yes"], "description": "Answer yes to all prompts"},
    {"name": ["-s", "--simulate", "--dry-run"], "description": "Show what would happen, change nothing"},
    {"name": ["-q", "--quiet"], "description": "Print less, -qq even less"},
    {"name": "--no-install-recommends", "description": "Don't install recommended packages"},
    {"name": "--install-suggests", "description": "Also install suggested packages"},
    {"name": ["-f", "--fix-broken"], "description": "Try to fix broken dependencies"},
    {"name": "--only-upgrade", "description": "Only upgrade packages that are installed"},
    {"name": "--reinstall", "description": "Reinstall packages that are up to date"},
    {"name": ["-t", "--target-release"], "description": "Release to take packages from", "args": {"name": "release"}},
    {"name": ["-o", "--option"], "description": "Set a configuration option", "args": {"name": "option"}},
]


def pacman():
    """pacman's operations are options, each with options of its own"""
    text = "".join(run("pacman", op, "--help") for op in ("-D", "-F", "-Q", "-R", "-S", "-T", "-U"))
    operations = [
        {"name": ["-S", "--sync"], "description": "Install packages, -Syu updates the system", "priority": 76},
        {"name": ["-R", "--remove"], "description": "Remove packages", "priority": 75},
        {"name": ["-Q", "--query"], "description": "Query installed packages", "priority": 75},
        {"name": ["-U", "--upgrade"], "description": "Install package files",
         "args": {"name": "file", "template": "filepaths", "isVariadic": True}},
        {"name": ["-F", "--files"], "description": "Query the files database"},
        {"name": ["-D", "--database"], "description": "Operate on the package database"},
        {"name": ["-T", "--deptest"], "description": "Check dependencies"},
        {"name": ["-V", "--version"], "description": "Show the version"},
        {"name": ["-h", "--help"], "description": "Show help, with an operation its options"},
    ]
    result = spec("pacman", "Arch Linux package manager", args=PACKAGES, extra_options=operations)
    seen = {n for o in result.get("options", []) for n in ([o["name"]] if isinstance(o["name"], str) else o["name"])}
    for option in options(text):
        names = [option["name"]] if isinstance(option["name"], str) else option["name"]
        if not seen.intersection(names):
            seen.update(names)
            result.setdefault("options", []).append(option)
    return result


RECIPES = {
    "ubuntu": {
        "image": "docker.io/library/ubuntu:26.04",
        "install": "apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "
        "python3 python3-pip zip unzip xz-utils curl wget openssh-client rsync systemd procps psmisc "
        "iproute2 iputils-ping diffutils cron passwd file snapd flatpak less mount util-linux"
        # a copy drops ping's file capabilities, which a rootless container can't exec
        " && cp /usr/bin/ping /usr/local/bin/ping",
        "recipes": {
            "apt": apt_like("apt", "Debian and Ubuntu package manager", APT_OPTIONS),
            "apt-get": apt_like("apt-get", "APT package handling utility", APT_OPTIONS),
            "apt-cache": apt_like("apt-cache", "Query the APT package cache", []),
            "dpkg": simple("dpkg", "Debian package manager", args=FILES, option_overrides={
                "-i": {"description": "Install .deb files", "args": {"name": "deb", "template": "filepaths", "isVariadic": True}},
                "--unpack": {"description": "Unpack .deb files without configuring them", "args": {"name": "deb", "template": "filepaths", "isVariadic": True}},
                "-A": {"description": "Record .deb files as available", "args": {"name": "deb", "template": "filepaths", "isVariadic": True}},
                "--configure": {"description": "Configure unpacked packages", "args": PACKAGES},
                "--triggers-only": {"description": "Process only pending triggers", "args": PACKAGES},
                "-r": {"description": "Remove packages, keep their config files", "args": PACKAGES},
                "-P": {"description": "Remove packages and their config files", "args": PACKAGES},
                "-L": {"args": PACKAGES}, "-s": {"args": PACKAGES}, "-S": {"args": PATTERN},
                "-l": {"args": {"name": "pattern", "isOptional": True}},
            }),
            "snap": lambda: spec("snap", "Install and manage snaps", help=("help", "--all"),
                                 sections=r"\):$|commands", sub_help=("{}", "--help"),
                                 top=("install", "remove", "list", "find", "refresh", "info"),
                                 sub_args={s: PACKAGES for s in ("install", "remove", "info", "refresh", "revert",
                                                                 "enable", "disable")} | {"find": PATTERN}),
            "flatpak": lambda: spec("flatpak", "Build, install and run applications and runtimes",
                                    sections=r"commands", sub_help=("{}", "--help"),
                                    top=("install", "run", "list", "update", "uninstall", "search", "info"),
                                    sub_args={s: {"name": "ref", "isVariadic": True}
                                              for s in ("install", "uninstall", "run", "info", "update")}
                                    | {"search": {"name": "text"}}),
            "pip": lambda: spec("pip", "Python package installer", sections=r"^commands",
                                sub_help=("{}", "--help"), top=("install", "uninstall", "list", "show", "freeze"),
                                sub_args={"install": {"name": "package", "isVariadic": True, "template": "filepaths"},
                                          "uninstall": PACKAGES, "show": PACKAGES, "download": PACKAGES}),
            "pip3": lambda: spec("pip3", "Python package installer", sections=r"^commands",
                                 sub_help=("{}", "--help"), top=("install", "uninstall", "list", "show", "freeze"),
                                 sub_args={"install": {"name": "package", "isVariadic": True, "template": "filepaths"},
                                           "uninstall": PACKAGES, "show": PACKAGES, "download": PACKAGES}),
            "systemctl": lambda: spec("systemctl", "Control the systemd system and service manager",
                                      sections=r"commands", top=("status", "start", "stop", "restart", "enable",
                                                                 "disable", "reload", "daemon-reload", "list-units"),
                                      sub_args={s: UNITS for s in ("start", "stop", "restart", "reload", "status",
                                                                   "enable", "disable", "is-active", "is-enabled",
                                                                   "mask", "unmask", "cat", "edit", "kill",
                                                                   "try-restart", "reload-or-restart", "show")},
                                      persistent=True),
            "journalctl": simple("journalctl", "Print log entries from the systemd journal", args={"name": "match", "isOptional": True, "isVariadic": True}),
            "hostnamectl": lambda: spec("hostnamectl", "Query or change the system hostname", sections=r"commands", persistent=True),
            "timedatectl": lambda: spec("timedatectl", "Query or change the system time and date", sections=r"commands", persistent=True),
            "cat": simple("cat", "Concatenate files and print them"),
            "chmod": simple("chmod", "Change file mode bits", args=[{"name": "mode"}, {**FILES, "isOptional": False}]),
            "chown": simple("chown", "Change file owner and group", args=[{"name": "owner[:group]"}, {**FILES, "isOptional": False}]),
            "chgrp": simple("chgrp", "Change group ownership", args=[{"name": "group"}, {**FILES, "isOptional": False}]),
            "ln": simple("ln", "Make links between files"),
            "touch": simple("touch", "Change file timestamps, create empty files"),
            "head": simple("head", "Print the first lines of files"),
            "tail": simple("tail", "Print the last lines of files"),
            "grep": simple("grep", "Print lines that match patterns", args=[PATTERN, FILES],
                           option_overrides={"--color": COLOR, "--colour": COLOR}),
            "sed": simple("sed", "Stream editor for filtering and transforming text", args=[{"name": "script"}, FILES]),
            "tar": simple("tar", "Archive files"),
            "gzip": simple("gzip", "Compress or expand files"),
            "gunzip": simple("gunzip", "Expand gzip files"),
            "xz": simple("xz", "Compress or expand .xz and .lzma files"),
            "zip": simple("zip", "Package and compress files", help=("-h",), columns=True),
            "unzip": simple("unzip", "List, test and extract zip archives", help=("-h",), columns=True),
            "curl": simple("curl", "Transfer data from or to a server", help=("--help", "all"),
                           args={"name": "url", "isVariadic": True}),
            "wget": simple("wget", "Download files from the web", args={"name": "url", "isVariadic": True}),
            "rsync": simple("rsync", "Fast, versatile file copying tool"),
            "kill": simple("kill", "Send a signal to processes", args={"name": "pid", "isVariadic": True},
                           option_overrides={"-s": {"args": {"name": "signal", "suggestions": SIGNALS}}}),
            "killall": simple("killall", "Kill processes by name", args={"name": "name", "isVariadic": True}),
            "ps": simple("ps", "Report a snapshot of the current processes", help=("--help", "all"), args=None),
            "free": simple("free", "Display the amount of free and used memory", args=None),
            "df": simple("df", "Report file system space usage"),
            "du": simple("du", "Estimate file space usage"),
            "diff": simple("diff", "Compare files line by line", args=[FILE, FILE]),
            "sort": simple("sort", "Sort lines of text files"),
            "uniq": simple("uniq", "Report or omit repeated lines", args=[{**FILE, "isOptional": True}, {**FILE, "isOptional": True}]),
            "wc": simple("wc", "Print line, word and byte counts"),
            "cut": simple("cut", "Remove sections from each line of files"),
            "tr": simple("tr", "Translate or delete characters", args=[{"name": "set1"}, {"name": "set2", "isOptional": True}]),
            "tee": simple("tee", "Copy standard input to files and standard output"),
            "stat": simple("stat", "Display file or file system status"),
            "file": simple("file", "Determine file type"),
            "mktemp": simple("mktemp", "Create a temporary file or directory", args={"name": "template", "isOptional": True}),
            "rmdir": simple("rmdir", "Remove empty directories", args=DIRS),
            "realpath": simple("realpath", "Print the resolved path"),
            "basename": simple("basename", "Strip directory and suffix from file names"),
            "dirname": simple("dirname", "Strip the last component from file names"),
            "date": simple("date", "Print or set the system date and time", args={"name": "+format", "isOptional": True}),
            "uname": simple("uname", "Print system information", args=None),
            "ls": simple("ls", "List directory contents", option_overrides={"--color": COLOR}),
            "cp": simple("cp", "Copy files and directories"),
            "mv": simple("mv", "Move or rename files"),
            "rm": simple("rm", "Remove files or directories"),
            "mkdir": simple("mkdir", "Make directories", args=DIRS),
            "ss": simple("ss", "Investigate sockets", args=None),
            "ping": simple("ping", "Send ICMP echo requests to network hosts", args={"name": "destination"}),
            "mount": simple("mount", "Mount a filesystem", args=[{"name": "device", "isOptional": True, "template": "filepaths"}, {"name": "mountpoint", "isOptional": True, "template": "folders"}]),
            "umount": simple("umount", "Unmount filesystems", args={"name": "mountpoint", "template": "folders", "isVariadic": True}),
            "useradd": simple("useradd", "Create a new user", args={"name": "login"}),
            "usermod": simple("usermod", "Modify a user account", args={"name": "login"}),
            "userdel": simple("userdel", "Delete a user account", args={"name": "login"}),
            "passwd": simple("passwd", "Change a user password", args={"name": "login", "isOptional": True}),
            "su": simple("su", "Run a shell as another user", args={"name": "user", "isOptional": True}),
            "crontab": simple("crontab", "Maintain crontab files", args={"name": "file", "template": "filepaths", "isOptional": True}),
        },
    },
    "fedora": {
        "image": "docker.io/library/fedora:latest",
        "install": "dnf install -y python3",
        "recipes": {
            "dnf": lambda: spec("dnf", "Fedora and RHEL package manager", sections=r"commands:|^subcommands:",
                                sub_help=("{}", "--help"),
                                top=("install", "remove", "upgrade", "search", "info", "list", "autoremove"),
                                sub_args={s: PACKAGES for s in ("install", "remove", "reinstall", "downgrade",
                                                                "upgrade", "info", "list", "provides", "swap")}
                                | {"search": PATTERN}, persistent=True),
            # yum is dnf on fedora, with its own name in the help
            "yum": lambda: spec("yum", "Fedora and RHEL package manager", sections=r"commands:|^subcommands:",
                                sub_help=("{}", "--help"),
                                top=("install", "remove", "upgrade", "search", "info", "list", "autoremove"),
                                sub_args={s: PACKAGES for s in ("install", "remove", "reinstall", "downgrade",
                                                                "upgrade", "info", "list", "provides", "swap")}
                                | {"search": PATTERN}, persistent=True),
            "rpm": simple("rpm", "RPM package manager", args={"name": "package", "isVariadic": True, "template": "filepaths"}),
        },
    },
    "arch": {
        "image": "docker.io/library/archlinux:latest",
        "install": "pacman -Sy --noconfirm python",
        "recipes": {"pacman": pacman},
    },
    "opensuse": {
        "image": "registry.opensuse.org/opensuse/tumbleweed:latest",
        "install": "zypper -n install python3",
        "recipes": {
            "zypper": lambda: spec("zypper", "openSUSE package manager", help=("help",), sections=r".",
                                   sub_help=("help", "{}"),
                                   top=("install", "remove", "update", "search", "info", "refresh", "dist-upgrade"),
                                   sub_args={s: PACKAGES for s in ("install", "remove", "info", "update")}
                                   | {"search": PATTERN}, persistent=True),
        },
    },
    "alpine": {
        "image": "docker.io/library/alpine:latest",
        "install": "apk add python3",
        "recipes": {
            "apk": lambda: spec("apk", "Alpine package manager", sections=r".", sub_help=("{}", "--help"),
                                top=("add", "del", "update", "upgrade", "search", "info"),
                                sub_args={s: PACKAGES for s in ("add", "del", "info", "fetch", "fix", "policy")}
                                | {"search": PATTERN}, persistent=True),
        },
    },
}


def main() -> None:
    image, names = sys.argv[1], sys.argv[2:]
    recipes = RECIPES[image]["recipes"]
    for name in names or recipes:
        result = recipes[name]()
        path = OUT / f"{name}.json"
        path.write_text(json.dumps(result, indent=1, ensure_ascii=False) + "\n")
        subs, opts = len(result.get("subcommands", [])), len(result.get("options", []))
        print(f"{path.name}: {subs} subcommands, {opts} options")


if __name__ == "__main__":
    if sys.argv[1:2] == ["--images"]:
        # image and install command of every recipe group, for generate.sh
        for key, value in RECIPES.items():
            print(key, value["image"], value["install"], sep="\t")
    elif sys.argv[1:2] and sys.argv[1] in RECIPES:
        main()
    else:
        raise SystemExit(f"usage: generate.py <{'|'.join(RECIPES)}> [command...]")
