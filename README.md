# rumb

Track software health. Spot trouble early.

A terminal dashboard for Arch Linux and Omarchy: what the next update brings, and what is already failing. It only reads and never asks for root.

<details>
<summary>
<a name="overview"><img src="docs/overview.png" alt="Overview" width="20%"></a>
<a name="updates"><img src="docs/updates.png" alt="Updates" width="20%"></a>
<a name="live"><img src="docs/live.png" alt="Live view during an update" width="20%"></a>
</summary>

<a name="overview-large"><img src="docs/overview.png" alt="Overview"></a>
<a name="updates-large"><img src="docs/updates.png" alt="Updates"></a>
<a name="live-large"><img src="docs/live.png" alt="Live view during an update"></a>
</details>

## Omarchy

```sh
omarchy pkg add rumb     # install
omarchy update           # update, with the rest of the system
omarchy pkg drop rumb    # uninstall
```

## Arch Linux, from source

```sh
sudo pacman -S --needed git make cargo jq pacman-contrib fakeroot
git clone https://github.com/HANCORE-linux/rumb && cd rumb
make && sudo make install                  # install
git pull && make && sudo make install      # update
sudo make uninstall                        # uninstall
```

Run `rumb`. The manual is `man rumb`.

[MIT](LICENSE)
