#!/usr/bin/env bash
#
# Point a GitHub runner's apt at the fastest Ubuntu mirror, then `apt-get update`.
#
# Some runners get a very slow route to the default mirror: the package index loads at
# once, but every package file takes seconds, and a 30-second install takes half an
# hour. Time a real package file from each mirror (three fetches each, as one fast
# reply can be luck) and name the fastest in the runner's sources.
#
#   sudo build-tools/ci/apt-fast-mirror.sh && sudo apt-get install -y ...

set -uo pipefail

printf 'Acquire::Retries "5";\nAcquire::http::Timeout "30";\n' > /etc/apt/apt.conf.d/80-retries
apt-get update -q > /dev/null
file=$(apt-cache show --no-all-versions libxinerama-dev | awk '/^Filename:/ { print $2; exit }')
best=""; best_t=999
for m in http://azure.archive.ubuntu.com/ubuntu http://archive.ubuntu.com/ubuntu \
	http://mirrors.edge.kernel.org/ubuntu http://mirror.math.princeton.edu/pub/ubuntu; do
	t=0
	for _ in 1 2 3; do
		t1=$(curl -o /dev/null -sf -m 20 -w '%{time_total}' "$m/$file" || echo 999)
		t=$(awk -v a="$t" -v b="$t1" 'BEGIN { print a + b }')
	done
	echo "mirror $m: ${t}s for 3 fetches"
	if awk -v a="$t" -v b="$best_t" 'BEGIN { exit !(a < b) }'; then best=$m; best_t=$t; fi
done
if [ -n "$best" ]; then
	echo "using $best"
	# the runner's sources name a mirror list file; name the chosen mirror instead
	sed -i -E "s#^URIs: (mirror\+file:/etc/apt/apt-mirrors.txt|http://azure.archive.ubuntu.com/ubuntu/?)\$#URIs: $best#" \
		/etc/apt/sources.list.d/ubuntu.sources
fi
apt-get update -q > /dev/null
