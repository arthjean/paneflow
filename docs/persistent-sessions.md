# Persistent sessions and updates

Closing the desktop with **Keep running** leaves local terminal processes in
the session host. Opening a saved layout or choosing **Retry** attempts to
attach the recorded session. Neither action starts a replacement shell. An
ended, missing, lost, or incompatible session keeps its identity and reports
why attachment is unavailable.

**Restart** is a deliberate launch action. It starts the recorded shell once
in a new generation, without replaying terminal input or the previous command.
The existing **Resume** actions for ended sessions also request an explicit
restart. A missing session needs a new-terminal action and a new identity.

A desktop and host from different application builds can attach when their
control protocol and terminal snapshot format match. An incompatible host is
left running. Retry after restoring a compatible desktop, or explicitly stop
its sessions and restart the host. Seamless attachment across incompatible
terminal engines is not supported.

Before an explicit host replacement, Paneflow checks the replacement host and
required hook helper. Missing or unreadable artifacts defer the action while
existing sessions continue. Restarting into a staged update follows the
session stop and host shutdown confirmation path. Cancel to keep working and
update later.

**Stop everything and quit** and **Stop everything and restart** share a
five-second response budget. If an operation cannot be confirmed, the window
stays open with the affected session identities and **Retry**, **Keep running
and quit**, and **Cancel** choices. Stops that already completed remain
completed. A timeout does not mean that an owned process was discarded; its
host retains responsibility for recovery.

Session records are saved off the terminal path. Ordinary state such as the
last screen activity is written shortly after it changes. Creating,
restarting, and accepting an agent event wait up to five seconds for the record
to reach disk and report a storage error otherwise. The final record of an
ended session is retried until it can be written, and the session shows the
pending storage error until then. Final output is retained as bounded text:
Paneflow keeps the end of a long session output, and the record says when the
text was truncated, never written, or later evicted. A session whose terminal,
input, or attachment queues are full refuses the extra work with a visible busy
error instead of growing without limit, and other sessions keep running.

If all process owners have resolved but final state could not be saved,
Paneflow reports the storage problem separately. **Quit with unsaved final
state** exits only the desktop. It does not install an update, force the host
to exit, or claim that the latest state reached disk. Retry after restoring
storage access to attempt persistence again.

On Windows, the MSI relay checks the host endpoint and access to the installed
host binary after the desktop exits. A busy or inaccessible binary defers
installation and relaunches the current version. The staged MSI remains
available. Installer Restart Manager shutdown is disabled so installation
does not terminate a retained host to release its executable. See Microsoft's
[Restart Manager property documentation](https://learn.microsoft.com/en-us/windows/win32/msi/msirestartmanagercontrol).

These controls preserve uncertainty rather than inferring successful stops
from a closed terminal window, a disconnected worker, or missing output.
Native installation and interactive qualification results must be checked for
the particular release and operating system.
