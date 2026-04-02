# Deployment Guide

## Production Deployment on Linux (systemd)

### 1. Create the service user

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin unly
```

### 2. Create directory structure

```bash
sudo mkdir -p /opt/unly/bin /opt/unly/data /etc/unly
sudo chown -R unly:unly /opt/unly
sudo chmod 750 /opt/unly/data
```

### 3. Build and install the binary

```bash
cargo build --release
sudo install -m 755 target/release/unly /opt/unly/bin/unly
```

### 4. Install the configuration

```bash
sudo cp deploy/unly.env.example /etc/unly/unly.env
sudo chmod 600 /etc/unly/unly.env
sudo nano /etc/unly/unly.env   # fill in secrets
```

Generate a config file:
```bash
sudo -u unly /opt/unly/bin/unly init-config --config /etc/unly/config.toml
sudo nano /etc/unly/config.toml  # edit settings
```

### 5. Authenticate with GitHub Copilot

```bash
sudo -u unly /opt/unly/bin/unly provider-login copilot --config /etc/unly/config.toml
```

### 6. Run migrations

```bash
sudo -u unly /opt/unly/bin/unly migrate --config /etc/unly/config.toml
```

### 7. Install and start the systemd service

```bash
sudo cp deploy/unly.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable unly
sudo systemctl start unly
```

### 8. Verify

```bash
sudo systemctl status unly
sudo journalctl -u unly -f
```

---

## Upgrading

```bash
cargo build --release
sudo systemctl stop unly
sudo install -m 755 target/release/unly /opt/unly/bin/unly
sudo -u unly /opt/unly/bin/unly migrate --config /etc/unly/config.toml
sudo systemctl start unly
```

---

## Backup

### Database backup

```bash
# SQLite online backup (safe while running)
sqlite3 /opt/unly/data/unly.sqlite ".backup /backup/unly-$(date +%Y%m%d).sqlite"
```

### Configuration backup

```bash
# Version-controlled config (without secrets)
cp /etc/unly/config.toml /path/to/git-repo/
# DO NOT commit unly.env
```

---

## Health Monitoring

Check service health via the CLI:

```bash
sudo -u unly /opt/unly/bin/unly doctor --config /etc/unly/config.toml
```

Or via Telegram (admin only):
```
/status
```

---

## Log Management

Logs go to journald by default:

```bash
sudo journalctl -u unly --since "1 hour ago"
sudo journalctl -u unly -o json | jq .  # JSON mode
```

Rotate logs:
```bash
sudo journalctl --vacuum-time=30d
```

---

## Firewall Notes

Unly does not open any inbound ports by default. It connects outbound to:
- `api.github.com` (Copilot auth)
- `api.githubcopilot.com` (Copilot API)
- `api.telegram.org` (Telegram bot API)

If you enable webhooks (`webhook.enabled = true`), it opens an inbound HTTP server on the configured port. Use a reverse proxy (nginx/caddy) with TLS.
