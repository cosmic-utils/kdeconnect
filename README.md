<!--suppress HtmlDeprecatedAttribute -->
<div align="center">
  <h1>⚠️ WORK IN PROGRESS</h1>
  <p>A native KDE Connect implementation for the COSMIC Desktop, written in Rust.<br>
  Many features are working but you may encounter bugs — please report them via <a href="https://github.com/hepp3n/kdeconnect/issues">GitHub Issues</a>.</p>
  <br>
  <img alt="KDE Connect applet on COSMIC desktop environment" src="https://raw.githubusercontent.com/hepp3n/kdeconnect/refs/heads/master/resources/screenshots/applet.png" />
</div>

---

<details>
<summary>✅ Supported Plugins</summary>

- Device Pairing / Unpairing
- Battery Monitor
- Clipboard Sync (bidirectional)
- Connectivity Report (signal strength / network type)
- Contacts Sync
- Find My Phone
- MPRIS / Media Control (exposed via D-Bus MPRIS2 to COSMIC panel)
- Notifications (receive, action, reply)
- Ping
- Run Commands
- Share Files & URLs (send files, receive files and URLs)
- SMS (conversations, send/receive)
- Plugin Enable / Disable per device
- System Volume (Partial support - May not work on certain devices - Known Mobile App Bug)
- Telephony (Know bug - Media does not resume when Ending/Canceling Call)
- SFTP / Browse Device (Requires sshfs package installed; mounts under `~/KDE Connect/<device>` so the file manager shows it with an unmount button, auto-unmounts on disconnect)

</details>

<details>
<summary>🚧 Plugins Not Yet Supported</summary>

The following plugins require RTP which is not yet supported on the COSMIC Desktop.
- MousePad / Remote Input
- Presenter Mode
- Virtual Display

</details>

---

## Installing from [COSMIC Flatpak Repository](https://github.com/pop-os/cosmic-flatpak)

```bash
flatpak remote-add --if-not-exists --user cosmic https://apt.pop-os.org/cosmic/cosmic.flatpakrepo
flatpak install --user io.github.hepp3n.kdeconnect
```

---

## Building from Source

### Prerequisites

- [rustup.rs](https://rustup.rs)
- `libxkbcommon-dev` (required on some distros — if the build fails, install this first)
- [`just`](https://github.com/casey/just) command runner

### Quick Start

```bash
git clone https://github.com/hepp3n/kdeconnect.git
cd kdeconnect
just build
just install
```

The service starts automatically on next login via D-Bus activation and XDG autostart.

### Optional: Systemd Integration

For journalctl logging and `systemctl` control instead of D-Bus activation:

```bash
just install-systemd
just enable-service
```

> **Note:** You may need to log out and back in for the applet to appear in the COSMIC panel.
> Once logged back in, go to **COSMIC Settings → Desktop → Panel → Configure Panel Applets** and add KDE Connect.

### Debug Install

Full logging for both the service and panel applet:

```bash
just install-debug
```

- Service logs → `/tmp/kdeconnect-service.log`
- Applet logs → `/tmp/kdeconnect-applet.log`

Restore to standard install with `just install`.

---

## Uninstalling

```bash
just uninstall
```

---

## Building as Flatpak

Requires `flatpak-builder`:

```bash
flatpak-builder --force-clean --user --install-deps-from=flathub --repo=repo --install builddir io.github.hepp3n.kdeconnect.json
```

# Conflicts

This KDE Connect implementation conflicts with the official KDE Connect implementation. The reason is, both use the same port range (1714-1764) for communication.
To avoid conflicts, you should uninstall the official KDE Connect package from your distribution.

# Troubleshooting

## Firewall
Some distributions enables Firewall by default. Or you are enabled it by yourself.
In this case, check what firewall you are using. And allow 1714-1764 port range for TCP and UDP connections.

For UFW firewall:

```bash
sudo ufw allow 1714:1764/udp
sudo ufw allow 1714:1764/tcp
sudo ufw reload
```

For Firewalld:

```bash
sudo firewall-cmd --permanent --zone=home --add-service=kdeconnect
sudo firewall-cmd --reload
```

For IPTables:

```bash
sudo iptables -I INPUT -i <yourinterface> -p udp --dport 1714:1764 -m state --state NEW,ESTABLISHED -j ACCEPT
sudo iptables -I INPUT -i <yourinterface> -p tcp --dport 1714:1764 -m state --state NEW,ESTABLISHED -j ACCEPT

sudo iptables -A OUTPUT -o <yourinterface> -p udp --sport 1714:1764 -m state --state NEW,ESTABLISHED -j ACCEPT
sudo iptables -A OUTPUT -o <yourinterface> -p tcp --sport 1714:1764 -m state --state NEW,ESTABLISHED -j ACCEPT
```

For more, directly from official KDEConnect userbase: [KDEConnect Firewall](https://userbase.kde.org/KDEConnect#ufw)


## Flatpak Service and Applet Logs
For contributors: There is a opt-in logger for flatpak that is helpful when troubleshooting sandbox issues.

To Enable:
```bash
flatpak override --user --env=RUST_LOG=info --env=KDECONNECT_LOG_FILE=1 io.github.hepp3n.kdeconnect
```
To Disable:
```bash
flatpak override --user --unset-env=RUST_LOG --unset-env=KDECONNECT_LOG_FILE io.github.hepp3n.kdeconnect
```

Logs will be generated in `~/.var/app/io.github.hepp3n.kdeconnect/data/`

**Note**
You may need to restart the instantace for the override to take effect. If the applet is placed in the cosmic panel you can simply kill the panel and it will restart all applets in the panel:

```bash
killall cosmic-panel
```
Using the `flatpak kill` command works as well if you perfer this method. The instance will auto restart after stopping if it's installed in the panel.

```bash
flatpak --user kill io.github.hepp3n.kdeconnect
```
