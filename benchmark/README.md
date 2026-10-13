# benchmark

compares kuterm with alacritty and konsole on the same machine, with the methods terminal
projects use themselves.

```bash
# release build first, the benchmark runs target/podman/release/kuterm
podman run --rm -v "$PWD":/src:Z \
  -v kuterm-cargo-registry:/usr/local/cargo/registry \
  -v kuterm-cargo-git:/usr/local/cargo/git \
  kuterm-dev cargo build --release
# all tests, about 15 minutes. needs python 3, cargo and git on the host
python3 benchmark/run.py
# a part of it, or other binaries
python3 benchmark/run.py --tests startup,idle --kuterm /path/to/kuterm
```

results go to `results/<date>.json` (every sample summary) and `results/<date>.md` (tables).
close other heavy programs and keep the windows that pop up visible: compositors throttle
hidden windows.

## same conditions

- same font (`JetBrainsMonoNL Nerd Font Mono`, 15pt, kuterm's 20px), a 100x30 grid, checked
  with `tput` inside each terminal before measuring.
- every terminal starts with empty `XDG_CONFIG_HOME` and `XDG_STATE_HOME`, so user settings and
  saved window sizes stay out. defaults otherwise, like a first launch.
- konsole runs with `--separate`, so it is its own process and not a window of a running one.
- the command runs directly in the terminal (`-e`, or kuterm's profile command), no shell
  startup files.

## tests

**startup**, like `hyperfine 'alacritty -e true'`: one warmup run, then 20 launches.
- *launch to command*: from spawning the terminal until the command inside writes a timestamp.
  the shell is ready to read input at this point.
- *launch to exit*: from spawning until the process ends after the command exits, includes
  tear down.

**memory**: PSS of the terminal process 4 seconds after launch, with an idle command. PSS
splits shared libraries between the processes using them, so it is fairer than RSS.

**idle**: cpu time and context switches of all threads over 10 seconds with nothing printed.
wakeups cost battery. the cursor blinks by default in kuterm and not in the others.

**throughput**: [vtebench](https://github.com/alacritty/vtebench), alacritty's benchmark, run
inside each terminal. it writes payloads of at least 1 MiB (dense cells, scrolling, regions,
cursor motion, unicode) and times how fast the terminal reads them from the pty. results are
ms per sample, lower is better. vtebench only measures pty reading: it says nothing about frame
rate or latency.

## not measured

- **keyboard to screen latency**: needs a camera or [typometer](https://github.com/pavelfatin/typometer),
  which reads the screen and does not work on wayland.
- **time to first frame**: the compositor knows when the first buffer is shown, an outside
  process does not. *launch to command* is the nearest stand in.

## reading the results

- kuterm renders with vulkan through gpui. on nvidia the driver runs two threads that wake
  ~250 times a second in any vulkan app, they make most of kuterm's idle wakeups. alacritty
  draws with opengl, konsole on the cpu.
- most of kuterm's startup is gpui creating the window: it starts both vulkan and opengl and
  tries every gpu adapter (~250ms here). kuterm's own startup work takes under 15ms.
