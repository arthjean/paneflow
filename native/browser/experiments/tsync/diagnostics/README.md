# Presentation capability diagnostics

These Linux probes reproduce the short capability queries used during the
TSYNC investigation. They do not present frames, run a browser, change display
modes, qualify Xorg, or measure M1. A reported session environment variable is
context, not proof of the X server topology.

No compilation is required. The X11 probe uses Python 3 `ctypes` with
`libxcb.so.1`, `libxcb-present.so.0`, and `libxcb-dri3.so.0`. The Vulkan probe
uses Python 3 and a `vulkaninfo` version that reports
`VkPresentTimingSurfaceCapabilitiesEXT`; the version used in the original
query was 1.4.341. No Python packages are required.

Run inside the existing session to inspect it. Choose fresh output paths when
retaining evidence:

```sh
python3 native/browser/experiments/tsync/diagnostics/x11-present-capabilities.py
xdpyinfo -queryExtensions
python3 native/browser/experiments/tsync/diagnostics/vulkan-present-timing.py
```

To retain the exact Vulkan input and parse it again:

```sh
vulkaninfo > /absolute/new-vulkaninfo.txt 2> /absolute/new-vulkaninfo.stderr
python3 native/browser/experiments/tsync/diagnostics/vulkan-present-timing.py --input /absolute/new-vulkaninfo.txt
```

Do not use `vulkaninfo --summary`: device extension advertisement alone does
not establish support for a particular Xcb, Xlib, or Wayland surface. The
parser retains each GPU/surface heading and the capability block. It groups
Xcb/Xlib when `vulkaninfo` groups them; it does not invent separate results.
The parser supports the text output used by the original query and fails when
that output provides no per-surface timing capability blocks.

For another already running X server, use `--display :N` with the appropriate
existing Xauthority. This script does not create a server, window, or event
subscription. Root-window Present capabilities are not proof that a specific
application window supports the same presentation path. To establish a native
Xorg condition, retain the server executable/version, login session, physical
outputs and active display mode separately. XWayland, Xephyr and headless X
servers do not satisfy that condition.

## Private native Xorg diagnostic session

`run-native-xorg.py` prepares a native Xorg session using an already extracted
runtime. It installs nothing and never modifies GNOME, system Xorg files,
polkit, or persistent display modes. Launching the server requires `--run`.
Without that option it only prints its checked plan. This runner requires a
real local TTY login through PAM, with logind reporting the current user,
`Type=tty`, `Class=user`, `Active=yes`, `Remote=no`, `Seat=seat0` and the same
`VTNr` as stdin. A desktop terminal, SSH connection or Codex process outside
that session is rejected.

The prepared Fedora runtime is under
`~/.cache/paneflow-cef-tsync/xorg-runtime/root`, with Xorg 21.1.24, the libinput
DDX and NVIDIA Xorg libraries 610.57.04. The runner uses `usr/libexec/Xorg`
directly with its private module/configuration paths. The extracted
`usr/bin/Xorg` wrapper references `/usr/libexec` and must not be used.
The packaged manual's root-only description of these path options is stale:
in the [official 21.1.24 source](https://www.x.org/releases/individual/xserver/xorg-server-21.1.24.tar.xz),
`hw/xfree86/common/xf86Init.c` restricts `-modulepath`, `-logfile` and absolute
`-configdir` only when `PrivsElevated()` is true. This runner requires the
ordinary, non-setuid `libexec/Xorg` binary.
Required installed tools are Python 3, startx/xinit/xauth/mcookie, loginctl,
xdpyinfo, xrandr, glxinfo, vulkaninfo and lspci. NVIDIA SMI is collected when
available. No window manager or browser is launched.

First record the current graphical session's VT, then switch to an unused
TTY and log in as the same ordinary user. Run these commands from the
repository root in that TTY, choosing a new evidence directory whose parent
already exists:

```sh
python3 native/browser/experiments/tsync/diagnostics/run-native-xorg.py
python3 native/browser/experiments/tsync/diagnostics/run-native-xorg.py \
  --run --output /absolute/new-native-xorg-diagnostic
```

`--runtime /absolute/private-xorg-runtime` selects another extracted runtime.
`--display 2` requests a particular unused display; by default the runner
chooses the first free number starting at 2. Existing sockets, abstract
sockets and lock files are rejected and never removed. Paths must contain no
whitespace or shell glob characters because the installed startx script
splits its arguments. The preflight records existing graphical sessions and
their VTs for the manual return; it does not activate or stop them.

The runner replaces itself with startx. Its diagnostic client verifies that
the display lock PID executes the exact recorded private Xorg binary, runs
without elevated UIDs, belongs to the same PAM scope, and is a child of the
same xinit that launched the client. It rechecks the server identity and
active session after collection. The evidence includes Xorg version and log,
DRM/PCI identities, XRandR outputs/providers/monitors and complete modes,
xdpyinfo, GLX renderer, optional NVIDIA SMI, raw X11 Present capabilities,
one full `vulkaninfo` output and the parsed Vulkan timing surface blocks.
The Vulkan query points Wayland at a nonexistent private endpoint so it
cannot silently query the suspended GNOME Wayland server as another WSI.

Every query has a timeout and retains separate stdout/stderr and its exit
status. Inspect `diagnostic.json`: missing capabilities, failed probes or a
session change remain failed evidence. A successful capability collection is
still `qualification=NOT_EVALUATED`, with zero measured presentations.
The client then exits, allowing xinit/startx to stop its own server. No
global process kill or VT activation is used. Wait for startx to return before
switching back to the previously recorded graphical VT. Verify the restored
GNOME display configuration there. `Xorg.log` and `startx.log` continue to be
finalized during shutdown and are deliberately excluded from the client's
pre-exit artifact hashes.

Xauthority material is private, outside the evidence directory, and is not
included in the recorded environment or artifacts. The normal startx
authority cleanup applies. Do not include runtime authority files when
sharing the diagnostic directory. These scripts have only been prepared and
syntax checked; their presence is not evidence that this native session has
been executed successfully.

## Interpreting the next native Xorg query

If the NVIDIA Xcb/Xlib surface advertises `IMAGE_FIRST_PIXEL_OUT`, the smallest
direct observation path is an opt-in Vulkan timing bridge at the existing WSI
submission boundary. Enable `VK_EXT_present_timing` and its required features
and extensions, create the swapchain with present timing enabled, configure
the timing result queue, and attach a unique `VkPresentId2KHR` identifier and
`VkPresentTimingsInfoEXT` to each measured `vkQueuePresentKHR`. Use
`targetTime=0` so observation does not request a new presentation schedule.
Associate the identifier with the actual GPUI scene submission, then retrieve
`vkGetPastPresentationTimingEXT` asynchronously. Record the returned stage,
time domain, domain identifier, completion status and calibrated timestamp
uncertainty. Zero or missing stage timestamps are missing evidence.

The pinned wgpu 29.0.4 does not expose this EXT path. Implementing the bridge
requires a scoped wgpu Vulkan backend change or an explicit measurement layer;
it is not a switch that the diagnostics turn on. A layer still needs an exact
GPUI scene-to-present identifier contract. Repeat the surface capability query
on the actual test window and verify the returned timing semantics before
accepting M1 samples. FIRST_PIXEL_OUT is a presentation-engine observation,
not a photodiode measurement.

If only `QUEUE_OPERATIONS_END` is available, reject it as the M1 endpoint.
An independent XPresent observer can select `CompleteNotify` on the GPUI XID,
but requires an explicit mapping from GPUI submissions to the WSI's
`PresentPixmap.serial`. FIFO or timestamp proximity alone does not establish
that mapping. `NotifyMSC` is an independent clock notification; `IdleNotify`
is buffer reuse, and `_NET_WM_SYNC_REQUEST` is resize synchronization.

The XPresent route must also verify the server/driver timestamp source and
compositor behavior: copying into a redirected window can precede physical
presentation. If exact correlation and native presentation provenance cannot
be established, preserve the diagnostic as not measured and use a qualified
compositor/KMS trace path or another native test configuration. Never relabel
queue completion or an XWayland timer as presentation.

## Sources

- [Xorg Present 1.4 protocol](https://gitlab.freedesktop.org/xorg/proto/xorgproto/-/blob/master/presentproto.txt)
- [XWayland 24.1.13 completion event delivery](https://gitlab.freedesktop.org/xorg/xserver/-/blob/xwayland-24.1.13/present/present_event.c#L148)
- [XWayland 24.1.13 UST update](https://gitlab.freedesktop.org/xorg/xserver/-/blob/xwayland-24.1.13/hw/xwayland/xwayland-present.c#L533)
- [VK_EXT_present_timing](https://docs.vulkan.org/refpages/latest/refpages/source/VK_EXT_present_timing.html)
- [Per-present query and target time](https://docs.vulkan.org/refpages/latest/refpages/source/VkPresentTimingInfoEXT.html)
- [Returned presentation identity and timestamps](https://docs.vulkan.org/refpages/latest/refpages/source/VkPastPresentationTimingEXT.html)
