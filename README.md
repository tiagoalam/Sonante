# Sonante website

Public project website: https://tiagoalam.github.io/Sonante/

This branch contains only the static website. The application is maintained on `main`.

## Update the site

- Edit `content.js` for feature groups and gallery captions. Downloads are automatic: `site.js` queries the latest stable GitHub release when the page opens and uses its uploaded installer assets, installation filenames, publication date and checksums. Drafts and prereleases are excluded. No version or filename update is needed for new stable releases.
- Edit `index.html` for the introduction, about text, audio explanation and installation requirements.
- Edit `style.css` for visual changes; `site.js` renders content and handles the gallery and screenshot dialog.
- Place screenshots in `assets/`. Images on this site are edited demonstrations using fictional music metadata and abstract artwork, not unmodified application captures. Keep that disclosure visible.
- Treat `main/README.md` and release notes as the source of truth for functionality. Keep experimental capabilities explicitly opt-in and avoid claims of measured/verified bit-perfect output.

No build step or dependencies are required. Preview with `python3 -m http.server 8080` from this directory. GitHub Pages publishes the root of the `gh-pages` branch. Relative asset paths support the `/Sonante/` base path.

## Review before publishing

Check automatic downloads against the latest stable release assets, screenshots and their captions, all navigation links, keyboard access to the gallery/dialog and features, and narrow-screen layout. Commit and push to `gh-pages`; check the GitHub Pages deployment before claiming publication is complete.

Feature content checked against public README and release 0.6.2 on 2026-11-05. Download information is fetched live; feature descriptions still require review when functionality changes. © 2026 Tiago Alam. The project code is GPL-3.0-only; see the application repository for full license and third-party notices.

The latest-release link remains available if JavaScript is disabled, the GitHub API is unavailable or rate-limited, or the release has no supported installers. The request times out after 10 seconds. Only matching Sonante amd64/x86_64 DEB, RPM and Arch installer assets from this repository are shown; source archives are not offered as installers.
