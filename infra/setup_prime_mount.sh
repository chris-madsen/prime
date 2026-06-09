#!/bin/bash

# Exit immediately if a command exits with a non-zero status
set -e

# Check if the script is run as root (sudo)
if [ "$EUID" -ne 0 ]; then
  echo "❌ Error: This script must be run with sudo privileges!"
  echo "Usage: sudo $0"
  exit 1
fi

# Path configuration
SHARE="//192.168.0.37/primes/prime"
MOUNT_POINT="/media/ilja/DATA/programs/win-prime/prime"
LINUX_USER="ilja"
CRED_FILE="/home/$LINUX_USER/.smbcredentials_prime"

echo "📂 Verifying and creating the mount directory..."
mkdir -p "$MOUNT_POINT"
chown $LINUX_USER:$LINUX_USER "$MOUNT_POINT"

# Request credentials for the Windows share
echo "--------------------------------------------------"
read -p "Enter Windows username (leave empty for guest access): " WIN_USER
echo "--------------------------------------------------"

if [ -z "$WIN_USER" ]; then
    echo "👤 Guest access selected (no password required)."
    FSTAB_OPTIONS="guest,uid=$LINUX_USER,gid=$LINUX_USER,iocharset=utf8,nofail,x-systemd.automount"
else
    read -s -p "Enter password for Windows user '$WIN_USER': " WIN_PASS
    echo "" # Newline after hidden password input

    echo "🔒 Creating secure credentials file..."
    echo "username=$WIN_USER" > "$CRED_FILE"
    echo "password=$WIN_PASS" >> "$CRED_FILE"
    
    # Set permissions: read/write only for root to secure the password
    chmod 600 "$CRED_FILE"
    chown root:root "$CRED_FILE"
    
    FSTAB_OPTIONS="credentials=$CRED_FILE,uid=$LINUX_USER,gid=$LINUX_USER,iocharset=utf8,nofail,x-systemd.automount"
fi

echo "📝 Checking /etc/fstab..."
# Escape special characters in the share path for exact matching with grep
SHARE_ESC=$(echo "$SHARE" | sed 's/[*+.^$]/\\&/g')

if grep -qs "^$SHARE_ESC" /etc/fstab; then
    echo "⚠️  An entry for $SHARE already exists in /etc/fstab. Skipping addition."
else
    echo "➕ Adding entry to /etc/fstab..."
    echo "$SHARE $MOUNT_POINT cifs $FSTAB_OPTIONS 0 0" >> /etc/fstab
    echo "✅ Entry successfully added."
fi

echo "🔄 Mounting the network share..."
# Unmount first if it was previously mounted incorrectly
umount "$MOUNT_POINT" 2>/dev/null || true
mount -a

# Verify if the mount was successful
if mountpoint -q "$MOUNT_POINT"; then
    echo "🎉 Success! The network share has been successfully mounted to:"
    echo "👉 $MOUNT_POINT"
else
    echo "❌ Something went wrong. The share is not mounted. Check logs using: dmesg | tail"
    exit 1
fi