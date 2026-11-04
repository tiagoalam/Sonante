# Sonante website

Public project website: https://tiagoalam.github.io/Sonante/

This branch contains only the static website. The application is maintained on `main`.

## Update the site

- Edit `content.js` for the release version, package revision, exact release asset filenames, feature groups, and gallery captions. Keep download links consistent with the public GitHub release.
- Edit `index.html` for the introduction, about text, audio explanation and installation requirements. Update static fallback release labels when changing the version.
- Edit `style.css` for visual changes; `site.js` renders content and handles the gallery and screenshot dialog.
- Place screenshots in `assets/`. Images on this site are edited demonstrations using fictional music metadata and abstract artwork, not unmodified application captures. Keep that disclosure visible.
- Treat `main/README.md` and release notes as the source of truth for functionality. Keep experimental capabilities explicitly opt-in and avoid claims of measured/verified bit-perfect output.

No build step or dependencies are required. Preview with `python3 -m http.server 8080` from this directory. GitHub Pages publishes the root of the `gh-pages` branch. Relative asset paths support the `/Sonante/` base path.

## Review before publishing

Check download filenames against release assets, screenshots and their captions, all navigation links, keyboard access to the gallery/dialog and features, and narrow-screen layout. Commit and push to `gh-pages`; check the GitHub Pages deployment before claiming publication is complete.

Website content checked against public README and release 0.6.1 revision 4 on 2026-11-04. © 2026 Tiago Alam. The project code is GPL-3.0-only; see the application repository for full license and third-party notices.
