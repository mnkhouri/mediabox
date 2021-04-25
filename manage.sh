#!/bin/bash
# Original script here, modified by Marc
# https://blog.linuxserver.io/2019/10/01/updating-and-backing-up-docker-containers-with-version-control/
SCRIPT_PATH=$(dirname "$(realpath -s "$0")")

# Change variables here:
APPDATA_LOC="$SCRIPT_PATH/../mediabox-config"
COMPOSE_LOC="$SCRIPT_PATH/docker-compose.yml"

# Don't change variables below unless you want to customize the script
VERSIONS_LOC="${APPDATA_LOC}/versions.txt"

function update {
    if ! which yq >/dev/null; then
        echo "Please install yq first"
        exit 1
    fi
    for i in $(docker-compose -f "$COMPOSE_LOC" config --services); do
        container_name=$(yq e ".services.${i}.container_name" "$COMPOSE_LOC")
        image_name=$(docker inspect --format='{{ index .Config.Image }}' "$container_name")
        repo_digest=$(docker inspect --format='{{ index .RepoDigests 0 }}' "$(docker inspect --format='{{ .Image }}' "$container_name")")
        if [ "$?" -ne 0 ]; then echo "Skipping version for $container_name"; fi
        echo "$container_name,$image_name,$repo_digest" >> "$VERSIONS_LOC"
    done

    docker-compose -f "$COMPOSE_LOC" pull
    cp -a "$COMPOSE_LOC" "$APPDATA_LOC"/docker-compose.yml.bak
    #backup_data

    confirm "Want to update?" || exit 0
    #docker-compose -f "$COMPOSE_LOC" up -d

    docker image prune -f
}

function backup_data {
    echo "Backing up data"
    docker-compose -f "$COMPOSE_LOC" down
    APPDATA_NAME=$(echo "$APPDATA_LOC" | awk -F/ '{print $NF}')
    sudo tar -C "$APPDATA_LOC"/.. -cvzf "$APPDATA_LOC"/../appdatabackup.tar.gz "$APPDATA_NAME"
    sudo chown "${USER}":"${USER}" "$APPDATA_LOC"/../appdatabackup.tar.gz
}

function restore_data {
    echo "Restoring data"
    docker-compose -f "$COMPOSE_LOC" down
    randstr=$(< /dev/urandom tr -dc _A-Z-a-z-0-9 | head -c${1:-8};echo;)
    mv "$APPDATA_LOC" "${APPDATA_LOC}.$randstr"
    cp -a "$COMPOSE_LOC" "${COMPOSE_LOC}.$randstr"
    mkdir -p "$APPDATA_LOC"
    sudo tar xvf "$APPDATA_LOC"/../appdatabackup.tar.gz -C "$APPDATA_LOC"/../
}

function restore_last_version {
    #restore_data
    for i in $(cat "$VERSIONS_LOC"); do
        image_name=$(echo "$i" | awk -F, '{print $2}')
        repo_digest=$(echo "$i" | awk -F, '{print $3}')
        sed -i "s#image: ${image_name}#image: ${repo_digest}#g" "$COMPOSE_LOC"
    done
    docker-compose -f "$COMPOSE_LOC" pull
    docker-compose -f "$COMPOSE_LOC" up -d
}

function resume_latest {
    for i in $(cat "$VERSIONS_LOC"); do
        image_name="$(echo $i | awk -F, '{print $2}')"
        repo_digest="$(echo $i | awk -F, '{print $3}')"
        sed -i "s#image: ${repo_digest}#image: ${image_name}#g" "$COMPOSE_LOC"
    done
    docker-compose -f "$COMPOSE_LOC" pull
    docker-compose -f "$COMPOSE_LOC" up -d
}

function confirm {
    # call with a prompt string or use a default
    read -r -p "${1:-Are you sure? [y/N]} " response
    case $response in
        [yY][eE][sS]|[yY])
            true;;
        *)
            false;;
    esac
}

# Check if the function exists
if declare -f "$1" > /dev/null; then
  "$@"
else
  echo "The only valid arguments are update, restore_last_version, and resume_latest"
  exit 1
fi