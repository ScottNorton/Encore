#!/bin/bash
# Poll for device AP, kill Encore instantly, grab diagnostics
# Override with ENCORE_DEVICE_IP if you renamed the device or mDNS doesn't work
. "$(dirname "$0")/../common.sh"
IP="$ENCORE_DEVICE_IP"
SSH_OPTS="$ENCORE_SSH_OPTS -o ConnectTimeout=2"

echo "Polling $IP port 22 every 0.2s..."
for i in $(seq 1 1200); do
  if timeout 0.5 bash -c "echo >/dev/tcp/$IP/22" 2>/dev/null; then
    echo "PORT 22 OPEN (attempt $i at $(date)) — killing Encore"
    sshpass -p $ENCORE_SSH_PASS ssh $SSH_OPTS root@$IP 'killall encore 2>/dev/null; touch /lsync/encore/disabled; echo KILLED' 2>&1
    if [ $? -eq 0 ]; then
      echo "SUCCESS — Encore killed and disabled"
      break
    fi
    echo "SSH failed, retrying..."
  fi
  sleep 0.2
done

echo "Waiting 3s for AP to stabilize..."
sleep 3

echo "=== PROCESSES ==="
sshpass -p $ENCORE_SSH_PASS ssh $SSH_OPTS root@$IP 'ps | grep -E "encore|hostapd|dnsmasq|python"' 2>&1

echo "=== ENCORE LOG (last 50 lines) ==="
sshpass -p $ENCORE_SSH_PASS ssh $SSH_OPTS root@$IP 'tail -50 /lsync/encore/encore.log 2>/dev/null' 2>&1

echo "=== CONFIG ==="
sshpass -p $ENCORE_SSH_PASS ssh $SSH_OPTS root@$IP 'cat /lsync/encore/config.toml 2>/dev/null' 2>&1

echo "=== DONE ==="
