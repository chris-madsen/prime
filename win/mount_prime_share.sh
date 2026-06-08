#!/usr/bin/env bash
set -u

SERVER="192.168.0.37"
SHARE_NAME="prime"
SHARE="//${SERVER}/${SHARE_NAME}"
BASE_DIR="/media/ilja/DATA/programs"
MOUNT_POINT="${BASE_DIR}/win-prime"
CREDENTIALS_FILE="${BASE_DIR}/.smb-prime-credentials"
SMB_VERSION="3.0"

log() { echo "[INFO] $*"; }
warn() { echo "[WARN] $*"; }
err() { echo "[ERROR] $*"; }

run_cmd() {
  echo "+ $*"
  "$@"
}

show_available_shares() {
  warn "Trying to list available SMB shares on ${SERVER}"
  if command -v smbclient >/dev/null 2>&1; then
    smbclient -L "//${SERVER}" -N 2>&1 || true
  else
    warn "smbclient is not installed, installing samba-client tools may help: sudo apt install smbclient"
  fi
}

show_mount_debug() {
  warn "Last kernel messages related to CIFS/SMB:"
  dmesg | tail -n 50 | grep -Ei 'cifs|smb|mount' || true
}

log "Checking required base directory: ${BASE_DIR}"
if [ ! -d "${BASE_DIR}" ]; then
  warn "Base directory does not exist. Creating it now."
  run_cmd sudo mkdir -p "${BASE_DIR}" || {
    err "Failed to create base directory: ${BASE_DIR}"
    exit 1
  }
fi

if ! command -v mount.cifs >/dev/null 2>&1; then
  log "cifs-utils is not installed. Installing it now."
  run_cmd sudo apt update || { err "apt update failed"; exit 1; }
  run_cmd sudo apt install -y cifs-utils smbclient || { err "Failed to install cifs-utils/smbclient"; exit 1; }
else
  log "cifs-utils is already installed"
fi

if ! command -v smbclient >/dev/null 2>&1; then
  warn "smbclient is not installed. Share discovery will be limited."
fi

log "Ensuring mount point exists: ${MOUNT_POINT}"
run_cmd sudo mkdir -p "${MOUNT_POINT}" || {
  err "Failed to create mount point: ${MOUNT_POINT}"
  exit 1
}

if [ ! -f "${CREDENTIALS_FILE}" ]; then
  log "Creating guest credentials file: ${CREDENTIALS_FILE}"
  cat > "${CREDENTIALS_FILE}" <<CREDS
username=guest
password=
CREDS
  chmod 600 "${CREDENTIALS_FILE}" || {
    err "Failed to set permissions on credentials file"
    exit 1
  }
else
  log "Using existing credentials file: ${CREDENTIALS_FILE}"
fi

log "Checking network reachability for ${SERVER}"
if ! ping -c 1 -W 2 "${SERVER}" >/dev/null 2>&1; then
  err "Host ${SERVER} is not reachable over the network"
  show_available_shares
  exit 1
fi

log "Checking whether TCP port 445 is reachable"
if command -v nc >/dev/null 2>&1; then
  if ! nc -z -w 3 "${SERVER}" 445 >/dev/null 2>&1; then
    err "Host ${SERVER} is reachable, but SMB port 445 is closed or filtered"
    show_available_shares
    exit 1
  fi
else
  warn "nc is not installed, skipping TCP port 445 connectivity test"
fi

if mountpoint -q "${MOUNT_POINT}"; then
  log "Mount point is already mounted. Unmounting it first."
  run_cmd sudo umount "${MOUNT_POINT}" || {
    err "Failed to unmount existing mount at ${MOUNT_POINT}"
    show_mount_debug
    exit 1
  }
else
  log "Mount point is not mounted yet"
fi

log "Trying to mount ${SHARE} to ${MOUNT_POINT}"
if ! sudo mount -t cifs "${SHARE}" "${MOUNT_POINT}" \
  -o credentials="${CREDENTIALS_FILE}",uid=$(id -u),gid=$(id -g),iocharset=utf8,vers=${SMB_VERSION},file_mode=0664,dir_mode=0775,noperm; then

  err "Mount failed for ${SHARE}"
  warn "Possible reasons:"
  warn "1. The share name '${SHARE_NAME}' does not exist on ${SERVER}"
  warn "2. Guest access is disabled on the Windows host"
  warn "3. SMB version ${SMB_VERSION} is not supported by the server"
  warn "4. The path is correct but access is denied"
  echo
  show_available_shares
  echo
  show_mount_debug
  echo
  warn "You can also try these manual commands:"
  echo "sudo mount -t cifs //${SERVER}/${SHARE_NAME} ${MOUNT_POINT} -o guest,vers=3.0"
  echo "sudo mount -t cifs //${SERVER}/${SHARE_NAME} ${MOUNT_POINT} -o guest,vers=2.1"
  echo "sudo mount -t cifs //${SERVER}/${SHARE_NAME} ${MOUNT_POINT} -o username=YOUR_WINDOWS_USER,password=YOUR_PASSWORD,vers=3.0"
  exit 1
fi

log "Mount succeeded"
log "Showing mounted filesystem entry"
mount | grep "${MOUNT_POINT}" || true

echo
log "Showing first 50 entries in ${MOUNT_POINT}"
ls -la "${MOUNT_POINT}" | head -n 50

echo
log "Examples:"
echo "Copy one file:   cp /path/to/file '${MOUNT_POINT}/'"
echo "Copy one folder: cp -r /path/to/folder '${MOUNT_POINT}/'"
echo "Unmount:         sudo umount '${MOUNT_POINT}'"
