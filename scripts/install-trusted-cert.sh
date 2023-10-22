#!/bin/sh

set -eu

pubkey="$(sudo cat /etc/nix/private-key | nix key convert-secret-to-public)"
echo pubkey = "$pubkey"

edited_conf=$(mktemp)

remote_conf=$(ssh "$1" cat /etc/nix/nix.conf)
echo remote_conf = \"\"\"
echo "$remote_conf"
echo \"\"\"
echo "$remote_conf" > "$edited_conf"
sed -i 's/\.  Do not edit it!/\. Then it was altered by install-trusted-cert. Do not edit!/g' "$edited_conf"
sed -i "s|^trusted-public-keys =.*|& $pubkey|g" "$edited_conf"

echo edited_conf = \"\"\"
cat "$edited_conf"
echo \"\"\"

# Make nix.conf editable
ssh "$1" sudo mv /etc/nix/nix.conf /etc/nix/nix.conf.bk
ssh "$1" sudo cp /etc/nix/nix.conf.bk /etc/nix/nix.conf
ssh "$1" "cat | sudo dd of=/etc/nix/nix.conf" < "$edited_conf"
ssh "$1" sudo systemctl restart nix-daemon
