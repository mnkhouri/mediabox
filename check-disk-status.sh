#!/bin/bash

if [ "$(zpool status -x)" == "all pools are healthy" ]; then
    echo "ZFS status check good"
    if [ "$(date +"%u")" != "6" ]; then
        echo "It's not weekend day, quitting"
        exit 0
    fi
    subject="Weekly ZFS report"
else
    subject="ALERT! ZFS pool in bad shape!"
    echo "$subject"
fi

body1=$(zfs list tank)
body2=$(zpool status -v)
body3=$(snapraid smart)

mailHeader=$(cat <<EOF
    <html>
    <body>
    <pre style="font: monospace">
EOF
)
mailFooter=$(cat <<EOF
    </pre>
    </body>
    </html>
EOF
)

echo -e "$mailHeader$body1\n\n$body2\n\n$body3$mailFooter" | mail -a 'Content-Type: text/html' -s "$subject" yacoutamia@gmail.com
