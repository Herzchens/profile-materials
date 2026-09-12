#!/usr/bin/env bash

fail() {
    printf '%s\n' "$*" >&2
    exit 1
}

if [ "$(id -u)" -ne 0 ]; then
    fail "run this installer as root"
fi

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)" || fail "could not resolve repository root"
UNIT_SOURCE="$ROOT_DIR/ops/profile-service.service"
ENV_SOURCE="$ROOT_DIR/ops/profile-service.env.example"
UNIT_TARGET="/etc/systemd/system/profile-service.service"
ENV_DIR="/etc/profile-service"
ENV_TARGET="$ENV_DIR/profile-service.env"
RELEASE_ROOT="/opt/profile-service/releases"
STATE_DIR="/var/lib/profile-service"

if ! id profile-service >/dev/null 2>&1; then
    useradd --system --home-dir "$STATE_DIR" --create-home --shell /usr/sbin/nologin profile-service || fail "could not create service user"
fi

install -d -m 0755 /opt/profile-service "$RELEASE_ROOT" || fail "could not create release directories"
install -d -m 0700 "$ENV_DIR" || fail "could not create config directory"
install -d -o profile-service -g profile-service -m 0700 "$STATE_DIR" || fail "could not create state directory"
install -m 0644 "$UNIT_SOURCE" "$UNIT_TARGET" || fail "could not install systemd unit"

if [ ! -e "$ENV_TARGET" ]; then
    install -m 0600 "$ENV_SOURCE" "$ENV_TARGET" || fail "could not create environment file"
    printf 'created %s; fill in the secret values before starting the service\n' "$ENV_TARGET"
else
    printf 'kept existing %s\n' "$ENV_TARGET"
fi

systemctl daemon-reload || fail "systemd daemon-reload failed"
systemctl enable profile-service.service || fail "could not enable profile-service.service"

printf 'systemd unit installed and enabled\n'
printf 'next: configure %s, then deploy a verified release artifact\n' "$ENV_TARGET"
