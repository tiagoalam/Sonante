'use strict';
(() => {
  const data = window.SONANTE_CONTENT;
  const byId = id => document.getElementById(id);
  const element = (tag, className, text) => {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text) node.textContent = text;
    return node;
  };
  const releasesUrl = 'https://github.com/tiagoalam/Sonante/releases/latest';
  const assetPrefix = 'https://github.com/tiagoalam/Sonante/releases/download/';
  const packageTypes = [
    { title: 'Ubuntu', description: 'Ubuntu 22.04 · amd64', type: 'DEB PACKAGE', extension: '.deb', pattern: /^sonante_[0-9][A-Za-z0-9.+_~-]*_amd64\.deb$/, installer: 'sudo apt install' },
    { title: 'Fedora', description: 'Fedora 44 · x86_64', type: 'RPM PACKAGE', extension: '.rpm', pattern: /^sonante-[0-9][A-Za-z0-9.+_~-]*\.x86_64\.rpm$/, installer: 'sudo dnf install' },
    { title: 'Arch / Manjaro', description: 'Arch Linux & Manjaro · x86_64', type: 'ARCH PACKAGE', extension: '.pkg.tar.zst', pattern: /^sonante-[0-9][A-Za-z0-9.+_~-]*-x86_64\.pkg\.tar\.zst$/, installer: 'sudo pacman -U' }
  ];
  const renderDownloads = async () => {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 10000);
    const status = byId('download-status');
    try {
      const response = await fetch('https://api.github.com/repos/tiagoalam/Sonante/releases/latest', {
        headers: { Accept: 'application/vnd.github+json' },
        signal: controller.signal,
        cache: 'no-cache'
      });
      if (!response.ok) throw new Error('Release lookup failed');
      const release = await response.json();
      if (release.draft || release.prerelease || typeof release.tag_name !== 'string' ||
          !release.tag_name.trim() || !Array.isArray(release.assets)) throw new Error('Invalid release');
      const assets = release.assets.filter(asset =>
        asset && asset.state === 'uploaded' && typeof asset.name === 'string' &&
        typeof asset.browser_download_url === 'string' &&
        asset.browser_download_url.startsWith(assetPrefix + encodeURIComponent(release.tag_name) + '/')
      );
      const cards = [];
      packageTypes.forEach((item, i) => {
        const asset = assets.find(candidate => item.pattern.test(candidate.name));
        if (!asset) return;
        const article = element('article', 'download-card');
        const type = element('div', 'package-type');
        type.append(element('span', '', item.type), element('span', '', `0${i + 1}`));
        const link = element('a', 'button', `Download ${item.extension}  ↓`);
        link.href = asset.browser_download_url;
        link.setAttribute('aria-label', `Download Sonante for ${item.title}`);
        article.append(type, element('h3', '', item.title), element('p', '', item.description), link, element('small', '', 'Complete installer · x86_64'));
        const install = element('details');
        const command = element('code', '', `${item.installer} ./${asset.name}`);
        command.style.cssText = 'display:block;font-size:10px;overflow-wrap:anywhere;padding-bottom:12px;color:#c4cfb6';
        install.append(element('summary', '', 'Installation command'), command);
        article.append(install);
        cards.push(article);
      });
      byId('downloads').replaceChildren(...cards);
      byId('hero-version').textContent = release.tag_name;
      byId('release-label').textContent = `Sonante ${release.tag_name.replace(/^v/, '')}`;
      const date = new Date(release.published_at);
      const published = release.published_at && !Number.isNaN(date.getTime())
        ? ` · Published ${new Intl.DateTimeFormat('en', { dateStyle: 'medium', timeZone: 'UTC' }).format(date)}`
        : '';
      status.textContent = cards.length
        ? `Latest stable release${published}. Packages shown below are available in this release.`
        : `Latest stable release${published}. No supported installers are available yet; check the release on GitHub.`;
      const checksum = assets.find(asset => asset.name === 'SHA256SUMS.txt');
      const checksumLink = byId('checksum-link');
      checksumLink.href = checksum ? checksum.browser_download_url : releasesUrl;
      checksumLink.textContent = checksum ? 'SHA256 checksums ↗' : 'Release files & checksums ↗';
    } catch {
      byId('hero-version').textContent = 'Latest release';
      byId('release-label').textContent = 'Download on GitHub';
      status.textContent = 'Unable to check the latest release right now. Get the current packages directly on GitHub.';
    } finally {
      clearTimeout(timeout);
    }
  };
  renderDownloads();
  data.highlights.forEach((item, i) => {
    const article = element('article', 'feature-highlight');
    article.append(element('span', 'feature-number', `0${i + 1}`), element('h3', '', item.title), element('p', '', item.text));
    byId('feature-highlights').append(article);
  });
  data.features.forEach(item => {
    const details = element('details');
    const summary = element('summary', '', item.title);
    if (item.beta) summary.append(element('span', 'beta', 'OPT-IN BETA'));
    const list = element('ul');
    item.items.forEach(text => list.append(element('li', '', text)));
    details.append(summary, list);
    byId('feature-details').append(details);
  });
  const lightbox = byId('lightbox');
  const showImage = item => {
    byId('lightbox-image').src = `assets/${item.id}.webp`;
    byId('lightbox-image').alt = item.alt;
    byId('lightbox-caption').textContent = `${item.title} Edited demonstration artwork and music metadata.`;
    lightbox.showModal();
  };
  const buttons = [];
  let selected = data.gallery[0];
  data.gallery.forEach(item => {
    const button = element('button', '', item.tab);
    button.type = 'button';
    button.setAttribute('aria-pressed', String(item === selected));
    button.addEventListener('click', () => {
      selected = item;
      byId('gallery-image').src = `assets/${item.id}.webp`;
      byId('gallery-image').alt = item.alt;
      byId('gallery-title').textContent = item.title;
      byId('gallery-description').textContent = item.description;
      buttons.forEach(b => b.setAttribute('aria-pressed', String(b === button)));
    });
    buttons.push(button);
    byId('gallery-tabs').append(button);
  });
  byId('gallery-open').addEventListener('click', () => showImage(selected));
  document.querySelector('[data-image="now-playing"]').addEventListener('click', () => showImage(data.gallery.find(i => i.id === 'now-playing')));
  byId('close-lightbox').addEventListener('click', () => lightbox.close());
  lightbox.addEventListener('click', e => { if (e.target === lightbox) { const r = lightbox.getBoundingClientRect(); if (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom) lightbox.close(); } });
})();
