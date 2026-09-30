(() => {
  document.documentElement.classList.add('js');
  const views = [...document.querySelectorAll('.view')];
  const tabs = [...document.querySelectorAll('nav.tabs a')];
  const show = (id, target) => {
    for (const v of views) v.classList.toggle('on', v.id === id);
    for (const t of tabs) {
      if (t.dataset.view === id) t.setAttribute('aria-current', 'page');
      else t.removeAttribute('aria-current');
    }
    if (target) { target.open = true; target.scrollIntoView({ block: 'start' }); }
  };
  const route = () => {
    const h = decodeURIComponent(location.hash.slice(1));
    const el = h && document.getElementById(h);
    if (el && el.classList.contains('view')) return show(h);
    if (el && el.tagName === 'DETAILS') return show(el.closest('.view').id, el);
    show('overview');
  };
  addEventListener('hashchange', route);
  route();

  const rows = [...document.querySelectorAll('#rows tr')];
  const fc = document.getElementById('f-class');
  const fg = document.getElementById('f-gate');
  const ft = document.getElementById('f-text');
  const count = document.getElementById('count');
  if (!fc) return;
  const apply = () => {
    const c = fc.value, g = fg.value, q = ft.value.trim().toLowerCase();
    let n = 0;
    for (const r of rows) {
      const on = (c === '' || (c === 'nonroutine' ? r.dataset.class !== 'process' : r.dataset.class === c))
        && (g === '' || r.dataset.gate === g)
        && (q === '' || r.textContent.toLowerCase().includes(q));
      r.hidden = !on;
      if (on) n++;
    }
    count.textContent = n + ' of ' + rows.length + ' records';
  };
  for (const el of [fc, fg, ft]) el.addEventListener('input', apply);
  apply();
})();
