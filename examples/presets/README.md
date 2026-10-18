# presets

ready settings files, each with a video of it in use. copy one to `~/.config/kuterm/settings.jsonc`, or keep your own file and pull one in with `"import": ["path/to/preset/settings.jsonc"]`. a preset only lists what it changes, everything else keeps its default from `assets/default_settings.jsonc`.

## minimal

just the shell: no tab bar while one tab is open, no scrollbar, no popups, marks, icons or bell flashes, the desktop draws the frame.

[settings](minimal/settings.jsonc)

<video src="minimal/demo.mp4" controls muted loop width="100%"></video>

[minimal/demo.mp4](minimal/demo.mp4)

## tmux

splits and tabs on a `ctrl-a` prefix: `v` / `s` split, `h j k l` move between panes, `z` zooms, `c` opens a tab, `n` / `p` switch tabs. panes without focus are dimmed, gruvbox colors, a status bar with folder, git and time.

[settings](tmux/settings.jsonc)

<video src="tmux/demo.mp4" controls muted loop width="100%"></video>

[tmux/demo.mp4](tmux/demo.mp4)

## reading

a calm light terminal for docs and logs: solarized light, big text with tall lines, wide margins, higher minimum contrast, stretched tabs with centered titles.

[settings](reading/settings.jsonc)

<video src="reading/demo.mp4" controls muted loop width="100%"></video>

[reading/demo.mp4](reading/demo.mp4)

## showcase

most features at once: tabs grouped by git project with branch and ports in their titles, tabs dragged onto each other to group them like in chrome, a full status bar (git, cpu graph, memory, network, ports, clock), the completion popup, dimmed splits, marks next to commands, profiles for a server and podman and docker containers, and extra palette commands and keys.

[settings](showcase/settings.jsonc)

<video src="showcase/demo.mp4" controls muted loop width="100%"></video>

[showcase/demo.mp4](showcase/demo.mp4)

## videos

60 fps at 1440×900, recorded by `tools/presets/record.sh`: kuterm runs with the preset on a virtual display with a window manager inside podman, typed into and clicked like a person would, and filmed with ffmpeg. to record them again, or a few of them, sharper or smaller:

```bash
sh tools/presets/record.sh
sh tools/presets/record.sh --crf 12 showcase
```
