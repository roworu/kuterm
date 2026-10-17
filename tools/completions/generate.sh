#!/bin/sh
# regenerate bundled completion specs from the programs themselves, inside a throwaway
# container, so nothing a recipe runs touches the host. writes into assets/completions.
#   sh tools/completions/generate.sh <ubuntu|fedora|arch|opensuse|alpine> [command...]
#   sh tools/completions/generate.sh aws
set -e
dir=$(cd "$(dirname "$0")" && pwd)
out=$(cd "$dir/../../assets/completions" && pwd)
key=${1:?usage: generate.sh <image|aws> [command...]}
shift

if [ "$key" = aws ]; then
	podman run --rm -v "$dir":/gen:ro,Z -v "$out":/out:Z docker.io/library/ubuntu:26.04 sh -c '
		apt-get update -qq >/dev/null && apt-get install -y -qq python3-venv >/dev/null 2>&1 &&
		python3 -m venv /venv && /venv/bin/pip install -q awscli &&
		/venv/bin/aws --version && /venv/bin/python /gen/aws.py | gzip -9n > /out/aws.jsonl.gz'
	exit
fi

line=$(python3 "$dir/generate.py" --images | grep "^$key	") || {
	echo "unknown image $key" >&2
	exit 1
}
image=$(printf '%s\n' "$line" | cut -f2)
install=$(printf '%s\n' "$line" | cut -f3)
podman run --rm -v "$dir":/gen:ro,Z -v "$out":/out:Z "$image" \
	sh -c "{ $install; } >/dev/null 2>&1 && cd /gen && python3 generate.py $key $*"
