#!/bin/bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

# It changes the machine's boot, login and desktop.
if [[ $(. /etc/os-release && echo "$ID-$VERSION_ID") != ubuntu-24.04 || $(id -un) != admin ]]; then
  echo 'this provisions the asrs-linux VM only' >&2
  exit 1
fi

# An upgrade can change the libraries and the kernel the references were recorded with. One that is
# already running is left to finish: stopping it leaves half-installed packages.
sudo systemctl mask -q --now apt-daily.timer apt-daily-upgrade.timer
sudo systemctl mask -q apt-daily.service apt-daily-upgrade.service unattended-upgrades.service
printf 'APT::Periodic::Update-Package-Lists "0";\nAPT::Periodic::Unattended-Upgrade "0";\n' |
  sudo tee /etc/apt/apt.conf.d/20auto-upgrades >/dev/null
while pgrep -f /usr/bin/unattended-upgrade >/dev/null; do sleep 2; done
sudo -E dpkg --configure -a
sudo -E apt-get update -q -o DPkg::Lock::Timeout=1800
sudo -E apt-get install -y -q --no-install-recommends --no-upgrade -o DPkg::Lock::Timeout=1800 \
  xserver-xorg-core xserver-xorg-input-libinput xinit x11-xserver-utils x11-utils xauth \
  xfce4-session xfwm4 xfdesktop4 xfce4-panel xfce4-settings xfconf xfce4-terminal \
  lightdm lightdm-gtk-greeter dbus-x11 at-spi2-core gnome-themes-extra \
  mesa-utils mesa-vulkan-drivers libgl1-mesa-dri vulkan-tools libvulkan1 libegl1 \
  imagemagick xdotool \
  build-essential clang cmake pkg-config git curl ca-certificates zstd unzip rsync jq \
  libssl-dev libfontconfig-dev libfreetype-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libvulkan-dev libx11-dev libx11-xcb-dev libxcb1-dev libxcb-xkb-dev libxi-dev libxrandr-dev \
  libxcursor-dev libasound2-dev libzstd-dev \
  libnss3 libgtk-3-0t64 libasound2t64 libgbm1 libxss1 libxtst6 libatk-bridge2.0-0t64 libdrm2 \
  libnotify4 libsecret-1-0 libcups2t64 binutils \
  fontconfig

if ! command -v node >/dev/null || [[ "$(node --version)" != v24.* ]]; then
  curl -fsSL https://deb.nodesource.com/setup_24.x | sudo -E bash -
  sudo -E apt-get install -y -q nodejs
fi

if [[ ! -x "$HOME/.cargo/bin/rustup" ]]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
fi

# npm fetches upstream's git dependencies over ssh://git@github.com, and the host has no GitHub key.
git config --global --unset-all url."https://github.com/".insteadOf || true
git config --global --add url."https://github.com/".insteadOf "ssh://git@github.com/"
git config --global --add url."https://github.com/".insteadOf "git@github.com:"

# Ubuntu 24.04 blocks the unprivileged user namespaces Chromium's sandbox needs.
echo "kernel.apparmor_restrict_unprivileged_userns=0" | sudo tee /etc/sysctl.d/60-asrs-electron.conf >/dev/null
sudo sysctl -q --system

if [[ ! -f /swapfile ]]; then
  sudo fallocate -l 8G /swapfile
  sudo chmod 600 /swapfile
  sudo mkswap /swapfile >/dev/null
  echo "/swapfile none swap sw 0 0" | sudo tee -a /etc/fstab >/dev/null
fi
sudo swapon -a

sudo groupadd -f -r autologin
sudo gpasswd -a "$USER" autologin >/dev/null
sudo mkdir -p /etc/lightdm/lightdm.conf.d
sudo tee /etc/lightdm/lightdm.conf.d/50-asrs.conf >/dev/null <<EOF
[Seat:*]
autologin-user=$USER
autologin-user-timeout=0
user-session=xfce
display-setup-script=/usr/local/bin/asrs-display-setup
EOF
sudo tee /usr/local/bin/asrs-display-setup >/dev/null <<'EOF'
#!/bin/sh
xset s off
xset s noblank
xset -dpms
EOF
sudo chmod 755 /usr/local/bin/asrs-display-setup
sudo systemctl set-default graphical.target >/dev/null
sudo systemctl enable lightdm >/dev/null 2>&1 || true

mkdir -p "$HOME/.config/xfce4/xfconf/xfce-perchannel-xml"
cat > "$HOME/.config/xfce4/xfconf/xfce-perchannel-xml/xsettings.xml" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<channel name="xsettings" version="1.0">
  <property name="Net" type="empty">
    <property name="ThemeName" type="string" value="Adwaita"/>
  </property>
  <property name="Xft" type="empty">
    <property name="DPI" type="int" value="96"/>
    <property name="Antialias" type="int" value="1"/>
    <property name="Hinting" type="int" value="1"/>
    <property name="HintStyle" type="string" value="hintslight"/>
    <property name="RGBA" type="string" value="none"/>
  </property>
  <property name="Gdk" type="empty">
    <property name="WindowScalingFactor" type="int" value="1"/>
  </property>
</channel>
EOF
cat > "$HOME/.config/xfce4/xfconf/xfce-perchannel-xml/xfce4-power-manager.xml" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<channel name="xfce4-power-manager" version="1.0">
  <property name="xfce4-power-manager" type="empty">
    <property name="dpms-enabled" type="bool" value="false"/>
    <property name="blank-on-ac" type="int" value="0"/>
  </property>
</channel>
EOF

if ! systemctl is-active -q lightdm; then
  sudo systemctl start lightdm
fi
echo "provisioned"
