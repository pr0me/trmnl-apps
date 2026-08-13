#!/bin/sh

set -eu

if [ "$(id -u)" -ne 0 ]; then
    echo "installer must run as root" >&2
    exit 1
fi

source_dir=$(
    CDPATH=
    cd -- "$(dirname -- "$0")"
    pwd
)
install -d -m 0755 /usr/local/lib/berlin-times
install -m 0755 "$source_dir/dispatcher.py" /usr/local/lib/berlin-times/dispatcher.py
install -d -m 0700 /etc/berlin-times
if [ ! -e /etc/berlin-times/dispatcher.env ]; then
    install -m 0600 "$source_dir/dispatcher.env.example" /etc/berlin-times/dispatcher.env
fi
for unit in "$source_dir"/systemd/*; do
    install -m 0644 "$unit" "/etc/systemd/system/$(basename -- "$unit")"
done
systemctl daemon-reload

echo "installed dispatcher; configure /etc/berlin-times/dispatcher.env before enabling timers"
