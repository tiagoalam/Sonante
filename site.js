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
  const releaseBase = `https://github.com/tiagoalam/Sonante/releases/download/${data.release.tag}/`;
  byId('hero-version').textContent = `v${data.release.version}`;
  byId('release-label').textContent = `Sonante ${data.release.version} · revision ${data.release.revision}`;
  byId('checksum-link').href = `${releaseBase}SHA256SUMS.txt`;
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
  data.downloads.forEach((item, i) => {
    const article = element('article', 'download-card');
    const type = element('div', 'package-type');
    type.append(element('span', '', item.type), element('span', '', `0${i + 1}`));
    const link = element('a', 'button', `Download ${i === 0 ? '.deb' : i === 1 ? '.rpm' : '.pkg.tar.zst'}  ↓`);
    link.href = releaseBase + item.file;
    link.setAttribute('aria-label', `Download Sonante for ${item.title}`);
    article.append(type, element('h3', '', item.title), element('p', '', item.description), link, element('small', '', 'Complete installer · x86_64'));
    const install = element('details');
    const command = element('code', '', item.command);
    command.style.cssText = 'display:block;font-size:10px;overflow-wrap:anywhere;padding-bottom:12px;color:#c4cfb6';
    install.append(element('summary', '', 'Installation command'), command);
    article.append(install);
    byId('downloads').append(article);
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
