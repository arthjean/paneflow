#!/bin/sh
# RPM `%preun` scriptlet - runs before a package is removed.
# Cleans up package-manager repo files on FULL uninstall only
# (US-016 hygiene follow-up).
#
# Scriptlet argument conventions:
#   $1 = 0  → final uninstall (the package is going away for good)
#   $1 = 1  → upgrade in progress (an older version is being swept aside
#             so a newer one can take its place; KEEP the repo file)
#   $1 >= 2 → extremely rare parallel-install scenarios; treat like upgrade
#
# Running `rm -f` on upgrade would leave the user without a repo source
# mid-transaction - `dnf upgrade paneflow` would complete, then the very
# next `dnf check-update` would silently drop our source. So the $1 = 0
# guard is load-bearing, not ceremonial.

set -e

if [ "$1" = "0" ]; then
    rm -f /etc/yum.repos.d/paneflow.repo
    rm -f /etc/zypp/repos.d/paneflow.repo
fi

exit 0
