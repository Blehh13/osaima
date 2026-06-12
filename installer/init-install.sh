#!/usr/bin/env bash
# Interstellar OS installer entry point (run inside live environment)
# Calamares is installed in the live system via the builder image.

# Ensure we are in the installer root
cd /installer || { echo "Installer directory not found"; exit 1; }

# Run Calamares with our configuration
calamares --modules-dir ./calamares/modules --config ./calamares/installer.conf
