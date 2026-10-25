(() => {
  'use strict';
  const data = window.LINGXI_BRAND;
  const roles = {product:'产品',feature:'功能',developer:'组件'};
  const state = {id:'code',lang:'zh',theme:'light'};
  const $ = id => document.getElementById(id);
  // All data comes from the checked-in local brand manifest, built by build.py.
  $('family-grid').innerHTML = data.products.map(p => `<article class="family-card"><img src="products/${p.id}/icon.svg" alt="${p.en} 图标"><h3>${p.descriptor}</h3><p>${p.zh}</p><p class="hex">${p.color}</p></article>`).join('');
  $('product-selector').innerHTML = data.products.map(p => `<button type="button" data-product="${p.id}" aria-pressed="${p.id===state.id}">${p.descriptor}</button>`).join('');
  $('name-rows').innerHTML = data.products.map(p => `<tr><td><img src="products/${p.id}/small.svg" alt=""><span>${p.descriptor}</span><span class="role-tag">${roles[p.role]}</span></td><td>${p.en}</td><td>${p.zh}</td><td>${p.role==='product' ? p.launcher_zh : '—'}</td></tr>`).join('');
  function render() {
    const p = data.products.find(p => p.id===state.id);
    const label = p[`launcher_${state.lang}`] || p[state.lang];
    $('sidebar-icon').src = `products/${p.id}/small.svg`;
    $('sidebar-label').textContent = label;
    $('workspace-lockup').src = `products/${p.id}/lockup-${state.theme}.svg`;
    $('workspace-lockup').alt = p.en;
    $('workspace-purpose').textContent = p.purpose;
    $('launcher-icon').src = `products/${p.id}/icon.svg`;
    $('launcher-icon').alt = `${p[state.lang]} 图标`;
    $('launcher-label').textContent = label;
    $('favicon-icon').src = `products/${p.id}/small.svg`;
    $('favicon-label').textContent = p[state.lang];
    $('context').dataset.theme = state.theme;
    $('selected-download').href = `products/${p.id}/icon.svg`;
    $('preview-status').textContent = `正在预览：${p[state.lang]} · ${state.theme==='light'?'浅色':'深色'}${p.role==='product'?'':' · 标识应用示意'}`;
    document.querySelectorAll('[data-product]').forEach(button => button.setAttribute('aria-pressed',String(button.dataset.product===state.id)));
    document.querySelectorAll('button[data-theme]').forEach(button => button.setAttribute('aria-pressed',String(button.dataset.theme===state.theme)));
    document.querySelectorAll('[data-lang]').forEach(button => button.setAttribute('aria-pressed',String(button.dataset.lang===state.lang)));
  }
  document.querySelectorAll('[data-product]').forEach(button => button.addEventListener('click',() => {state.id=button.dataset.product;render();}));
  document.querySelectorAll('button[data-theme]').forEach(button => button.addEventListener('click',() => {state.theme=button.dataset.theme;render();}));
  document.querySelectorAll('[data-lang]').forEach(button => button.addEventListener('click',() => {state.lang=button.dataset.lang;render();}));
  render();
})();
