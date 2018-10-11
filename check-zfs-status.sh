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

body=$(zpool status -v)

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

echo "$mailHeader$body$mailFooter" | mail -a 'Content-Type: text/html' -s "$subject" yacoutamia@gmail.com
