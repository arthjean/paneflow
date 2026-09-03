---
name: aarch64 (ARM64) bug report
about: Report a PaneFlow bug that only reproduces on aarch64 Linux (Raspberry Pi, Asahi Linux, Graviton, etc.)
title: "[aarch64] "
labels: ["aarch64", "bug"]
assignees: []
---

<!--
Thanks for filing an aarch64-specific report! PaneFlow's ARM64 build
ships from a native `ubuntu-22.04-arm` CI runner without on-device
runtime testing, so bugs that only
surface on real hardware are especially valuable - please fill in as
much of the context below as you can.

If the same bug also reproduces on x86_64, use the generic bug template
instead - this one is reserved for ARM-specific issues so triage can
route them to someone with aarch64 hardware.
-->

## Environment

<!--
Privacy note: `uname -a` includes your machine hostname and `glxinfo -B`
can emit a per-GPU `Device UUID` on some Mesa versions. Before posting,
check the output below and scrub anything you'd rather not publish -
a hostname redaction is usually enough.
-->

- **Distro + version** (e.g. `Ubuntu 24.04.1 LTS`, `Fedora Asahi Remix 40`):
- **Kernel** (`uname -srvmpio` - drops hostname that `uname -a` would include):
- **CPU / SoC** (e.g. `Raspberry Pi 5`, `Apple M2 Pro`, `AWS Graviton3`):
- **GPU + driver** - try one or more of:
  - `glxinfo -B | head -10` (most distros; prints OpenGL renderer string)
  - `lspci -nnk | grep -A3 VGA` (discrete-GPU boxes; returns nothing on
    integrated SoCs like the Raspberry Pi's VideoCore VII)
  - `cat /sys/kernel/debug/dri/0/name` (last-resort on SoCs without
    discoverable PCI IDs - may require `sudo`)
  - `vcgencmd version` (Raspberry Pi firmware / VideoCore version):
- **Display server** (`echo $XDG_SESSION_TYPE` - `wayland` or `x11`):
- **Desktop environment** (e.g. GNOME 46, KDE Plasma 6.0):
- **PaneFlow version** (`paneflow --version`):
- **Install format** (pick one):
  - [ ] `.deb` (`apt install ./paneflow-*-aarch64.deb`)
  - [ ] `.rpm` (`dnf install ./paneflow-*-aarch64.rpm`)
  - [ ] AppImage (`./paneflow-*-aarch64.AppImage`)
  - [ ] tar.gz (`./paneflow.app/install.sh`)

## Reproduction

Steps to reproduce the bug, starting from a fresh `paneflow` launch:

1.
2.
3.

## Expected behaviour

<!-- What should have happened? -->

## Actual behaviour

<!-- What actually happened? Include screenshots or a photo of the screen if the bug is visual. -->

## Logs

Launch PaneFlow with `RUST_LOG=info` and paste the stderr output that
covers the bug window. If the crash is immediate, also run with
`RUST_BACKTRACE=1` and include the backtrace.

<details>
<summary>PaneFlow log output</summary>

```
paste log / backtrace here
```

</details>

## Additional context

<!--
Anything else that might help. Examples:
  - Does the bug reproduce on x86_64? (If yes, use the generic bug
    template; this template is aarch64-only.)
  - Did the same binary work on a different aarch64 machine?
  - Wayland vs X11 - does switching display servers help?
  - Distro-specific workarounds you've already tried.
-->
