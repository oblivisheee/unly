# Operational Runbook

## Daily Operations

### Check service health
```bash
sudo systemctl status unly
sudo journalctl -u unly --since "1 hour ago" | grep -E "ERROR|WARN"
```

Via Telegram (admin): `/status`

### View audit log (last 20 entries)
```bash
sudo -u unly /opt/unly/bin/unly audit -n 20 --config /etc/unly/config.toml
```

Via Telegram: `/audit`

### View scheduled jobs
```bash
sudo -u unly /opt/unly/bin/unly job list --config /etc/unly/config.toml
```

Via Telegram: `/jobs`

---

## Incident Response

### Bot not responding

1. Check service status: `sudo systemctl status unly`
2. Check logs: `sudo journalctl -u unly -n 100`
3. Check Telegram API connectivity: `curl -s https://api.telegram.org`
4. Check provider health: `sudo -u unly /opt/unly/bin/unly doctor`
5. Restart if needed: `sudo systemctl restart unly`

### Provider authentication failure

1. Run `sudo -u unly /opt/unly/bin/unly provider-status`
2. If Copilot token expired: `sudo -u unly /opt/unly/bin/unly provider-login copilot`
3. Check token cache file: `ls -la /opt/unly/data/github_token.json`

### Database corruption

1. Stop the service: `sudo systemctl stop unly`
2. Check integrity: `sqlite3 /opt/unly/data/unly.sqlite "PRAGMA integrity_check;"`
3. If corrupt, restore from backup:
   ```bash
   cp /backup/unly-YYYYMMDD.sqlite /opt/unly/data/unly.sqlite
   sudo systemctl start unly
   ```

### Audit log review after security event

```bash
# Look for denied events
sudo -u unly /opt/unly/bin/unly audit -n 100 | grep denied

# Or query SQLite directly
sqlite3 /opt/unly/data/unly.sqlite \
  "SELECT created_at, event_type, subject, action, outcome FROM audit_log WHERE outcome='denied' ORDER BY created_at DESC LIMIT 20;"
```

### Memory store cleanup

Prune expired entries:
```bash
sudo -u unly /opt/unly/bin/unly memory prune --config /etc/unly/config.toml
```

Inspect memory for a specific chat:
```bash
sudo -u unly /opt/unly/bin/unly memory list --scope chat:<chat-uuid> -n 20
```

---

## Upgrades

See `docs/deployment.md` for the upgrade procedure.

After upgrading:
1. Run `unly doctor` to verify all subsystems are healthy
2. Run `unly audit -n 5` to confirm the audit log is working
3. Send a test message in Telegram and verify a response

---

## Configuration Changes

1. Edit `/etc/unly/config.toml`
2. Validate: `sudo -u unly /opt/unly/bin/unly validate --config /etc/unly/config.toml`
3. Reload: `sudo systemctl restart unly`

---

## Backup Schedule (Recommended)

| Item | Frequency | Method |
|---|---|---|
| SQLite database | Daily | `sqlite3 .backup` |
| config.toml | On change | Git (no secrets) |
| github_token.json | On re-auth | Manual copy |

---

## Metrics to Monitor

- Service uptime (systemd)
- Journal error/warn rate
- SQLite file size (check for unbounded growth)
- Audit log growth rate (expected: proportional to usage)
- Provider health (via `/status` or `doctor`)
