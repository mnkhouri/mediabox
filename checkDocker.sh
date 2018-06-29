# Check if docker is running
set -euo pipefail

echo "Starting run at $(date)"
if ps ax | grep "[D]ocker.app" >/dev/null; then
    if ! /usr/local/bin/docker ps 2>&1 1>/dev/null; then
       echo "'docker ps' failed, killing docker"
        ps ax | grep "[dD]ocker"
        kill -9 $(pgrep docker) || true
        kill -9 $(pgrep Docker)
        sleep 5
        echo "Starting docker"
        open /Applications/Docker.app || true
        echo "Started docker with code $?"
    else
        echo "docker ps is responding"
    fi
else
    echo "No docker processes running!"
fi
echo "Ending run at $(date)"
