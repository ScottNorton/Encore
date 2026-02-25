#!/bin/bash
# Poll for device to come back up after reboot
HOST="${1:-192.168.43.1}"
SSH_CMD="sshpass -p ridiculous ssh -o HostKeyAlgorithms=+ssh-rsa -o PubkeyAcceptedKeyTypes=+ssh-rsa -o StrictHostKeyChecking=no -o ConnectTimeout=3 root@$HOST"
for i in $(seq 1 40); do
    sleep 3
    if $SSH_CMD echo UP 2>/dev/null; then
        echo "Device back after $((i*3))s"
        exit 0
    fi
    echo "try $i..."
done
echo "Timeout waiting for device"
exit 1
