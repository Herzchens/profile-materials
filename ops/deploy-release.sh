#!/usr/bin/env bash

log() {
    printf '[profile-deploy] %s\n' "$*"
}

fail() {
    printf '[profile-deploy] ERROR: %s\n' "$*" >&2
    exit 1
}

if [ "$(id -u)" -ne 0 ]; then
    fail "run this deployer as root"
fi

if [ "$#" -ne 3 ]; then
    fail "usage: $0 <profile-service-binary> <sha256-file> <release-id>"
fi

BINARY_INPUT="$1"
CHECKSUM_INPUT="$2"
RELEASE_ID="$3"
SERVICE="profile-service.service"
RELEASE_ROOT="/opt/profile-service/releases"
CURRENT_LINK="/opt/profile-service/current"
HEALTH_URL="http://127.0.0.1:3000/health/ready"

case "$RELEASE_ID" in
    ''|*[!A-Za-z0-9._-]*) fail "release id may contain only A-Z, a-z, 0-9, dot, underscore, and hyphen" ;;
esac

[ -f "$BINARY_INPUT" ] || fail "binary not found: $BINARY_INPUT"
[ -f "$CHECKSUM_INPUT" ] || fail "checksum file not found: $CHECKSUM_INPUT"
[ -f /etc/profile-service/profile-service.env ] || fail "missing /etc/profile-service/profile-service.env"

TMP_DIR="$(mktemp -d /tmp/profile-service-deploy.XXXXXX)" || fail "could not create temporary directory"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

cp "$BINARY_INPUT" "$TMP_DIR/profile-service-linux-x86_64" || fail "could not stage binary"
cp "$CHECKSUM_INPUT" "$TMP_DIR/profile-service-linux-x86_64.sha256" || fail "could not stage checksum"

log "verifying SHA-256"
(
    cd "$TMP_DIR" || exit 1
    sha256sum -c profile-service-linux-x86_64.sha256
) || fail "checksum verification failed"

RELEASE_DIR="$RELEASE_ROOT/$RELEASE_ID"
[ ! -e "$RELEASE_DIR" ] || fail "release already exists: $RELEASE_DIR"

install -d -m 0755 "$RELEASE_ROOT" || fail "could not create release root"
install -d -m 0755 "$RELEASE_DIR" || fail "could not create release directory"
install -m 0755 "$TMP_DIR/profile-service-linux-x86_64" "$RELEASE_DIR/profile-service" || fail "could not install release binary"

PREVIOUS_TARGET=""
if [ -L "$CURRENT_LINK" ]; then
    PREVIOUS_TARGET="$(readlink -f "$CURRENT_LINK" 2>/dev/null || true)"
fi

switch_current() {
    target="$1"
    tmp_link="/opt/profile-service/.current.$$.tmp"
    rm -f "$tmp_link"
    ln -s "$target" "$tmp_link" || return 1
    mv -Tf "$tmp_link" "$CURRENT_LINK" || return 1
}

wait_ready() {
    attempts="$1"
    i=1
    while [ "$i" -le "$attempts" ]; do
        if curl -fsS --max-time 2 "$HEALTH_URL" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
        i=$((i + 1))
    done
    return 1
}

log "activating $RELEASE_ID"
switch_current "$RELEASE_DIR" || fail "could not switch current release"

if ! systemctl restart "$SERVICE"; then
    log "service restart failed"
else
    if wait_ready 45; then
        log "release is ready"
        log "current -> $RELEASE_DIR"
        exit 0
    fi
    log "new release did not become ready"
fi

if [ -n "$PREVIOUS_TARGET" ] && [ -x "$PREVIOUS_TARGET/profile-service" ]; then
    log "rolling back to $PREVIOUS_TARGET"
    switch_current "$PREVIOUS_TARGET" || fail "rollback symlink failed"
    systemctl restart "$SERVICE" || fail "rollback service restart failed"
    wait_ready 45 || fail "rollback release did not become ready"
    fail "deployment failed; previous release restored"
fi

systemctl stop "$SERVICE" >/dev/null 2>&1 || true
rm -f "$CURRENT_LINK"
fail "deployment failed and no previous healthy release was available"
