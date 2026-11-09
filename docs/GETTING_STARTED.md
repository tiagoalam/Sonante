# Getting started with Sonante

Welcome to Sonante! This guide covers the basics so you can get from installation to your first album.

## 1. Download Sonante

Sonante is currently available for **Linux (x86_64)**. Windows is in development.

Visit the [latest release](https://github.com/tiagoalam/Sonante/releases/latest) and download the package for your distribution:

| Linux distribution | What to download |
| --- | --- |
| Ubuntu 22.04 | The `.deb` file |
| Fedora 44 | The `.rpm` file |
| Arch Linux / Manjaro | The `.pkg.tar.zst` file |

Install it with your system's package manager, then open **Sonante** from the application menu.

If you use another Linux distribution, check the [compatibility notes](LINUX_GUIDE.md#system-requirements) before installing. An AppImage isn't available yet.

## 2. Tell Sonante where your music lives

The welcome wizard helps you choose a source. You can also visit **Preferences → Sources** later.

- **Local:** choose one or more folders containing your music, including mounted network shares.
- **Plex:** connect your Plex account, then choose the server and library you want.
- **Navidrome:** enter your server address and account details to browse that library.

You can use more than one source, and Sonante keeps track of where albums come from.

## 3. Choose your audio output

Sonante offers two common ways to listen:

- **Shared:** a convenient starting point that uses your system audio setup and can coexist with other apps.
- **Direct:** lets you choose a hardware audio endpoint when available.

Start with **Shared** if you're unsure. You can change the output in Preferences later. Available devices and features depend on your Linux setup.

## 4. Find something to play

Open your music source to browse **albums**, **artists**, **years** and **genres**. Use search to find a specific track or album, or Explore to rediscover music already in your collection.

Add tracks to your queue or create a playlist when you want to save a selection.

## 5. Make it yours

Choose between two interface modes:

- **Simple:** the everyday listening essentials.
- **Full:** more ways to explore, organize and customize your experience.

You can also change themes, album layouts and the Now Playing display. Switching interface modes does not replace your music library.

## Need help?

- **A package won't install:** check that your distribution and desktop libraries meet the [Linux requirements](LINUX_GUIDE.md#system-requirements).
- **Your music isn't showing:** check the source settings and use **Check for changes** for local libraries.
- **A device won't play:** try **Shared** output first, then check the selected device.
- **Something went wrong:** [report a bug](https://github.com/tiagoalam/Sonante/issues/new/choose). Tell us your distribution, Sonante version, the steps that led to the issue and what happened.

**Privacy tip:** please review logs and screenshots before sharing them. Don't include passwords, access tokens, personal paths or server addresses.

For readers who want implementation details, build instructions or audio-route explanations, see the [Linux technical reference](LINUX_GUIDE.md).
