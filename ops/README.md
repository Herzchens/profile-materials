# Production deployment

The production service runs as a native Rust binary under systemd. It listens only on `127.0.0.1:3000`; Tailscale Funnel is the public HTTPS ingress and TLS terminator.

There is intentionally no Caddy or Nginx layer in the current topology. Funnel already terminates TLS and reverse-proxies to the loopback listener, so another local proxy would add configuration and another failure point without changing the service boundary.

## Files and paths

```text
/etc/profile-service/profile-service.env      root-only secrets/config
/opt/profile-service/releases/<release-id>/   immutable release directories
/opt/profile-service/current                  atomic symlink to active release
/var/lib/profile-service/                     presence and GitHub LKG state
/etc/systemd/system/profile-service.service   systemd unit
```

The service runs as the dedicated `profile-service` user. It does not need root privileges and does not bind a public interface.

## One-time systemd setup

From a checked-out repository:

```bash
sudo ./ops/install-systemd.sh
sudoedit /etc/profile-service/profile-service.env
```

Fill in the Discord and GitHub tokens plus the target IDs. The installer keeps an existing environment file unchanged, installs the unit, creates the service account/directories, reloads systemd, and enables the unit for boot. It does not start an unconfigured service.

Configure the persistent public ingress once:

```bash
sudo tailscale funnel --bg 3000
sudo tailscale funnel status
```

Funnel provides the public HTTPS endpoint and proxies it to the service's loopback port. A background Funnel configuration survives terminal disconnects and is restored by Tailscale after reboot.

## Releases

Pushing a `v*` tag runs `.github/workflows/release.yml`. It builds the locked release binary and publishes:

```text
profile-service-linux-x86_64
profile-service-linux-x86_64.sha256
```

The checksum is verified in CI before the GitHub release is published.

Download both files on the VPS, then deploy them with a unique release ID:

```bash
sudo ./ops/deploy-release.sh \
  ./profile-service-linux-x86_64 \
  ./profile-service-linux-x86_64.sha256 \
  v0.1.0
```

The deployer verifies SHA-256 before installation, creates a new immutable release directory, atomically switches `/opt/profile-service/current`, restarts only `profile-service.service`, and waits for `/health/ready`.

If the new binary does not become ready, the deployer atomically restores the previous release and restarts it. An unverified binary is never made current.

## Secrets and state

`/etc/profile-service/profile-service.env` is installed with mode `0600` and is not stored in the repository. Do not put tokens on the command line, in release artifacts, or in the LKG files.

Presence and GitHub LKG data live under `/var/lib/profile-service`. Replacing a binary does not replace that state.

## Operational checks

```bash
systemctl status profile-service --no-pager
journalctl -u profile-service -n 50 --no-pager
curl -fsS http://127.0.0.1:3000/health/live
curl -fsS http://127.0.0.1:3000/health/ready
sudo tailscale funnel status
```

After a reboot, check both the systemd service and Funnel before treating the deployment as recovered.
