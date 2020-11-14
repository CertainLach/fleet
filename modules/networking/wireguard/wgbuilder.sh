#!/bin/sh
key=$($WG genkey)
pub=$(echo $key | $WG pubkey)

$COREUTILS/bin/mkdir -p $out
echo $key | $RAGE $recipients >$out/key
echo $pub >$out/pub_key
