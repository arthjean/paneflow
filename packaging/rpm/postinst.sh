#!/bin/sh

set -e

for REPO_DIR in /etc/yum.repos.d /etc/zypp/repos.d; do
    [ -d "$REPO_DIR" ] || continue
    REPO="$REPO_DIR/paneflow.repo"
    if [ ! -f "$REPO" ]; then
        cat > "$REPO" <<'EOF'
[paneflow]
name=PaneFlow
baseurl=https://pkg.paneflow.dev/rpm
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=https://pkg.paneflow.dev/gpg
EOF
        chmod 644 "$REPO"
    fi
done

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -f /usr/share/icons/hicolor >/dev/null 2>&1 || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications >/dev/null 2>&1 || true
fi

exit 0
