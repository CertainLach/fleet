{...}: {
	config.system.activationScripts.onlineActivation = ''
		if [ -z ''${FLEET_ONLINE_ACTIVATION+x} ]; then
			1>&2 echo "online activation; hello, fleet!"
		fi
	'';
}
