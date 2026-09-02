#!/bin/sh

set -e

if [ "$1" = "0" ]; then
    rm -f /etc/yum.repos.d/paneflow.repo
    rm -f /etc/zypp/repos.d/paneflow.repo
fi

exit 0
