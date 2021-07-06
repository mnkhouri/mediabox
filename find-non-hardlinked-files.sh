#!/bin/bash
set -euo pipefail

# Script to find all the files which only have 1 hardlink

# NOTE: THIS IS EXTREMELY EASY TO MISUSE
#  - can't just `rm` the directories of these files, as they may have other files in there that _are_ hardlinked (e.g. season packs)
#  - can't just `rm` the files themselves, as the downloader may re-download

function filterByHardlink() {
	FN="$1"
	HlCount=$(ls -l "$FN" | awk '{print $2}')
	if [ "$HlCount" -eq 1 ]; then
		du -h "$FN"
		# dirname "$FN"
		# TODO: Check in `dirname "$FN"` to see if _any_ files have more than 1 hardlink.
		#	Only return dirs in which no files have more than 1 hardlink
	fi
}
export -f filterByHardlink
SearchDir="$1"

fdfind . --type file --size +1000m --print0 "$SearchDir" \
	| xargs -0 -I {} bash -c 'filterByHardlink "$@"' _ {} 
