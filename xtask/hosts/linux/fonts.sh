#!/usr/bin/env bash
# Unpacks the Linux render host's font packages, and Ubuntu 24.04's fontconfig rules over them
# alone, into <dir>. Text vectors only apply to these exact files, and Ubuntu's mirrors drop old
# package versions, so they come from Launchpad's permanent archive and are checked by hash.
#
#   xtask/hosts/linux/fonts.sh <download cache> <dir>
#   export FONTCONFIG_FILE=<dir>/fonts.conf
set -euo pipefail

cache=$(realpath -m "$1")
dir=$(realpath -m "$2")
archive=https://launchpad.net/ubuntu/+archive/primary/+files
packages=(
  "40049660c194f3b8a2541fc7369efebb10e9f94bdac836a2f38fafedd10fa73a fonts-dejavu-core_2.37-8_all.deb"
  "8a599d6553307db7ecb795d2f0e5a301e03234afc75c7358b0ba43466454c89a fonts-dejavu-mono_2.37-8_all.deb"
  "065c2ab1abc9108b17d401016dc594b79750904390f095845c93bb06e1153acc fonts-liberation_2.1.5-3_all.deb"
  "b102b62ce6a7313315223cff5e052bdf8fc0ad162c9e961ce0dc2202bc139ce2 fonts-noto-color-emoji_2.047-0ubuntu0.24.04.1_all.deb"
  "9df0579cd298a1f2b7a40efd519c849745aecd193a09733cf4d1032284a62bbf fontconfig-config_2.15.0-1.1ubuntu2_amd64.deb"
)

if [[ -e $dir ]]; then
  echo "$dir exists" >&2
  exit 1
fi
mkdir -p "$cache" "$dir/root" "$dir/conf.d"
for package in "${packages[@]}"; do
  read -r hash file <<<"$package"
  if ! sha256sum --status -c <<<"$hash  $cache/$file"; then
    curl --fail --silent --show-error --location --retry 5 --retry-all-errors \
      -o "$cache/$file" "$archive/$file"
    sha256sum -c <<<"$hash  $cache/$file"
  fi
  member=$(ar t "$cache/$file" | grep '^data\.tar')
  ar p "$cache/$file" "$member" >"$dir/$member"
  tar -xf "$dir/$member" -C "$dir/root"
  unlink "$dir/$member"
done

# The links point at /usr/share and /etc, which may hold another Ubuntu's rules or none. The two
# left out read the user's and the machine's own configuration.
for rule in "$dir"/root/etc/fonts/conf.d/*.conf; do
  name=${rule##*/}
  case $name in 50-user.conf | 51-local.conf) continue ;; esac
  target=$(readlink "$rule" || true)
  case $target in
    "") ;;
    /*) rule=$dir/root$target ;;
    *) rule=$dir/root/etc/fonts/conf.d/$target ;;
  esac
  cp "$rule" "$dir/conf.d/$name"
done

{
  grep -v '<dir\|<include\|<cachedir\|</fontconfig>' "$dir/root/etc/fonts/fonts.conf"
  printf '<dir>%s</dir>\n' "$dir/root/usr/share/fonts"
  printf '<include>%s</include>\n' "$dir/conf.d"
  printf '<cachedir>%s</cachedir>\n</fontconfig>\n' "$dir/cache"
} >"$dir/fonts.conf"
