#!/bin/bash
DEVICE_IP="${1:-192.168.43.1}"
SSH_OPTS="-o HostKeyAlgorithms=+ssh-rsa -o PubkeyAcceptedKeyTypes=+ssh-rsa -o StrictHostKeyChecking=no -o ConnectTimeout=3"
echo "Waiting for $DEVICE_IP..."
for i in $(seq 1 30); do
    if sshpass -p ridiculous ssh $SSH_OPTS root@$DEVICE_IP "echo alive" >/dev/null 2>&1; then
        echo "Device is back!"
        sshpass -p ridiculous ssh $SSH_OPTS root@$DEVICE_IP "uname -r; uptime"
        exit 0
    fi
    sleep 5
    echo "  ...${i}x5s"
done
echo "Timed out"
exit 1
